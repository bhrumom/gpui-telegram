//! Pure-Rust deterministic mini-game runtime supporting all 7 `GameSpec` variants.

use gpui_telegram_proto::creations::{
    CardPair, GameAchievementDef, GameEngineKind, GamePayload, GameSpec, GameUserState,
    GridPuzzleLevel, StoryNode, TriviaQuestion,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const MAX_TICKS_PER_STEP: u64 = 1_000;
const MAX_GRID_PUZZLE_DIM: usize = 64;
const MAX_CARD_MATCH_PAIRS: usize = 512;
const CARD_MISMATCH_FLIP_BACK_MS: u64 = 700;
const ARCADE_INVULNERABILITY_MS: u64 = 600;

/// Deterministic SplitMix64 PRNG (zero external `rand` dependency).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        let initial = if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        };
        Self { state: initial }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_usize(&mut self, upper: usize) -> usize {
        if upper <= 1 {
            return 0;
        }
        (self.next_u64() % (upper as u64)) as usize
    }

    fn next_f32_unit(&mut self) -> f32 {
        let bits = self.next_u64() >> 40;
        (bits as f32) / ((1u64 << 24) as f32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameStatus {
    Ready,
    Playing,
    Paused,
    Won,
    GameOver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameInput {
    Start,
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    SelectOption(usize),
    ClickCell { row: usize, col: usize },
    Confirm,
    PrimaryAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GridPuzzleMode {
    SlidingTile,
    MazeSokoban,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArcadeEntityKind {
    Hazard,
    Collectible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArcadeEntitySnapshot {
    pub entity_id: u64,
    pub kind: ArcadeEntityKind,
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub vx: f32,
    pub vy: f32,
    pub color_hex: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnakeDirection {
    Up,
    Down,
    Left,
    Right,
}

impl SnakeDirection {
    fn is_opposite(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Up, Self::Down)
                | (Self::Down, Self::Up)
                | (Self::Left, Self::Right)
                | (Self::Right, Self::Left)
        )
    }

    fn delta(self) -> (i32, i32) {
        match self {
            Self::Up => (0, -1),
            Self::Down => (0, 1),
            Self::Left => (-1, 0),
            Self::Right => (1, 0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GridCoord {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardFaceState {
    FaceDown,
    FaceUp,
    Matched,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardCellSnapshot {
    pub index: usize,
    pub row: usize,
    pub col: usize,
    pub pair_id: String,
    pub label: String,
    pub symbol: String,
    pub accent_hex: Option<String>,
    pub face: CardFaceState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoryChoiceSnapshot {
    pub index: usize,
    pub label: String,
    pub target_node_id: String,
    pub score_delta: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "variant", rename_all = "snake_case")]
pub enum GameRuntimeState {
    GridPuzzle {
        mode: GridPuzzleMode,
        rows: usize,
        cols: usize,
        level_index: usize,
        total_levels: usize,
        level_id: String,
        level_title: String,
        grid: Vec<Vec<i32>>,
        target_grid: Option<Vec<Vec<i32>>>,
        moves_used: u32,
        max_moves: Option<u32>,
        hint: Option<String>,
        player_pos: Option<(usize, usize)>,
    },
    TriviaQuiz {
        current_question_index: usize,
        total_questions: usize,
        question_id: String,
        prompt: String,
        choices: Vec<String>,
        selected_choice: Option<usize>,
        correct_index: Option<usize>,
        last_answer_correct: Option<bool>,
        explanation: Option<String>,
        awaiting_next: bool,
        correct_count: u32,
        remaining_ms: Option<u64>,
    },
    InteractiveFiction {
        current_node_id: String,
        title: String,
        body: String,
        choices: Vec<StoryChoiceSnapshot>,
        visited_node_ids: Vec<String>,
        is_ending: bool,
        ending_badge: Option<String>,
    },
    ArcadeDodge {
        arena_width: u32,
        arena_height: u32,
        tick_ms: u32,
        player_x: f32,
        player_y: f32,
        player_radius: f32,
        invulnerable_ms: u64,
        target_score: u32,
        theme_accent_hex: Option<String>,
        entities: Vec<ArcadeEntitySnapshot>,
        ticks_simulated: u64,
    },
    SnakeGrid {
        grid_width: usize,
        grid_height: usize,
        tick_ms: u32,
        wrap_walls: bool,
        target_score: u32,
        direction: SnakeDirection,
        segments: Vec<GridCoord>,
        food: Option<GridCoord>,
        ticks_simulated: u64,
    },
    CardMatch {
        rows: usize,
        cols: usize,
        cards: Vec<CardCellSnapshot>,
        moves: u32,
        matched_pairs: usize,
        total_pairs: usize,
        mismatch_timer_ms: u64,
        remaining_ms: Option<u64>,
    },
    MiniAppLaunch {
        bot_username: String,
        short_name: String,
        html_bundle_media_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameRuntimeSnapshot {
    pub engine_kind: GameEngineKind,
    pub status: GameStatus,
    pub score: i64,
    pub high_score: i64,
    pub lives: u8,
    pub max_lives: u8,
    pub combo: u32,
    pub elapsed_ms: u64,
    pub unlocked_achievements: Vec<String>,
    pub newly_unlocked_achievements: Vec<String>,
    pub save_state_json: serde_json::Value,
    pub state: GameRuntimeState,
}

#[derive(Debug, Clone)]
struct GridPuzzleState {
    default_rows: usize,
    default_cols: usize,
    levels: Vec<GridPuzzleLevel>,
    level_index: usize,
    mode: GridPuzzleMode,
    rows: usize,
    cols: usize,
    grid: Vec<Vec<i32>>,
    target_grid: Option<Vec<Vec<i32>>>,
    moves_used: u32,
    player_pos: Option<(usize, usize)>,
    walls: HashSet<(usize, usize)>,
    goals: HashSet<(usize, usize)>,
    targets: HashSet<(usize, usize)>,
    crates: HashSet<(usize, usize)>,
}

#[derive(Debug, Clone)]
struct TriviaQuizState {
    questions: Vec<TriviaQuestion>,
    current_index: usize,
    selected_choice: Option<usize>,
    last_answer_correct: Option<bool>,
    awaiting_next: bool,
    correct_count: u32,
    remaining_ms: Option<u64>,
}

#[derive(Debug, Clone)]
struct InteractiveFictionState {
    nodes_by_id: HashMap<String, StoryNode>,
    current_node_id: String,
    visited_node_ids: Vec<String>,
}

#[derive(Debug, Clone)]
struct ArcadeDodgeState {
    arena_width: u32,
    arena_height: u32,
    tick_ms: u32,
    player_speed: f32,
    obstacle_speed: f32,
    target_score: u32,
    theme_accent_hex: Option<String>,
    player_x: f32,
    player_y: f32,
    player_radius: f32,
    invulnerable_ms: u64,
    accum_ms: u64,
    spawn_cooldown_ms: u64,
    next_entity_id: u64,
    entities: Vec<ArcadeEntitySnapshot>,
    ticks_simulated: u64,
    rng: SplitMix64,
}

#[derive(Debug, Clone)]
struct SnakeGridState {
    grid_width: usize,
    grid_height: usize,
    tick_ms: u32,
    wrap_walls: bool,
    target_score: u32,
    direction: SnakeDirection,
    queued_direction: Option<SnakeDirection>,
    segments: Vec<GridCoord>,
    food: Option<GridCoord>,
    accum_ms: u64,
    ticks_simulated: u64,
    rng: SplitMix64,
}

#[derive(Debug, Clone)]
struct CardMatchState {
    rows: usize,
    cols: usize,
    cards: Vec<CardCellSnapshot>,
    moves: u32,
    matched_pairs: usize,
    total_pairs: usize,
    first_open: Option<usize>,
    pending_mismatch: Option<(usize, usize)>,
    mismatch_timer_ms: u64,
    remaining_ms: Option<u64>,
}

#[derive(Debug, Clone)]
enum EngineInner {
    GridPuzzle(GridPuzzleState),
    TriviaQuiz(TriviaQuizState),
    InteractiveFiction(InteractiveFictionState),
    ArcadeDodge(ArcadeDodgeState),
    SnakeGrid(SnakeGridState),
    CardMatch(CardMatchState),
    MiniAppHtml {
        bot_username: String,
        short_name: String,
        html_bundle_media_id: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct MiniGameRuntime {
    spec: GameSpec,
    engine_kind: GameEngineKind,
    achievements: Vec<GameAchievementDef>,
    seed: u64,
    status: GameStatus,
    score: i64,
    high_score: i64,
    lives: u8,
    max_lives: u8,
    combo: u32,
    elapsed_ms: u64,
    sessions_started: u64,
    unlocked_achievements: Vec<String>,
    newly_unlocked_achievements: Vec<String>,
    inner: EngineInner,
}

impl MiniGameRuntime {
    pub fn from_payload(payload: &GamePayload, seed: u64) -> Self {
        let mut runtime = Self {
            spec: payload.spec.clone(),
            engine_kind: payload.spec.engine_kind(),
            achievements: payload.achievements.clone(),
            seed,
            status: GameStatus::Ready,
            score: 0,
            high_score: 0,
            lives: 1,
            max_lives: 1,
            combo: 0,
            elapsed_ms: 0,
            sessions_started: 0,
            unlocked_achievements: Vec::new(),
            newly_unlocked_achievements: Vec::new(),
            inner: EngineInner::MiniAppHtml {
                bot_username: String::new(),
                short_name: String::new(),
                html_bundle_media_id: None,
            },
        };
        runtime.reset_inner();
        runtime
    }

    pub fn pause(&mut self) {
        if self.status == GameStatus::Playing {
            self.status = GameStatus::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.status == GameStatus::Paused {
            self.status = GameStatus::Playing;
        }
    }

    pub fn restart(&mut self) {
        self.high_score = self.high_score.max(self.score);
        self.score = 0;
        self.combo = 0;
        self.elapsed_ms = 0;
        self.newly_unlocked_achievements.clear();
        self.reset_inner();
    }

    fn reset_inner(&mut self) {
        self.status = GameStatus::Ready;
        self.lives = 1;
        self.max_lives = 1;
        match &self.spec {
            GameSpec::GridPuzzle { rows, cols, levels } => {
                let (state, status) = GridPuzzleState::new(*rows, *cols, levels);
                self.status = status;
                self.inner = EngineInner::GridPuzzle(state);
            }
            GameSpec::TriviaQuiz {
                time_limit_secs,
                questions,
            } => {
                let remaining_ms = time_limit_secs
                    .filter(|&s| s > 0)
                    .map(|s| (s as u64).saturating_mul(1000));
                if questions.is_empty() {
                    self.status = GameStatus::GameOver;
                }
                self.inner = EngineInner::TriviaQuiz(TriviaQuizState {
                    questions: questions.clone(),
                    current_index: 0,
                    selected_choice: None,
                    last_answer_correct: None,
                    awaiting_next: false,
                    correct_count: 0,
                    remaining_ms,
                });
            }
            GameSpec::InteractiveFiction {
                start_node_id,
                nodes,
            } => {
                let (state, status) = InteractiveFictionState::new(start_node_id, nodes);
                self.status = status;
                self.inner = EngineInner::InteractiveFiction(state);
            }
            GameSpec::ArcadeDodge {
                tick_ms,
                arena_width,
                arena_height,
                player_speed,
                obstacle_speed,
                lives,
                target_score,
                theme_accent_hex,
            } => {
                let safe_lives = (*lives).max(1);
                self.lives = safe_lives;
                self.max_lives = safe_lives;
                self.inner = EngineInner::ArcadeDodge(ArcadeDodgeState::new(
                    *tick_ms,
                    *arena_width,
                    *arena_height,
                    *player_speed,
                    *obstacle_speed,
                    *target_score,
                    theme_accent_hex.clone(),
                    self.seed,
                ));
            }
            GameSpec::SnakeGrid {
                grid_width,
                grid_height,
                tick_ms,
                wrap_walls,
                target_score,
            } => {
                self.inner = EngineInner::SnakeGrid(SnakeGridState::new(
                    *grid_width,
                    *grid_height,
                    *tick_ms,
                    *wrap_walls,
                    *target_score,
                    self.seed,
                ));
            }
            GameSpec::CardMatch {
                pairs,
                time_limit_secs,
            } => {
                let (state, status) = CardMatchState::new(pairs, *time_limit_secs, self.seed);
                self.status = status;
                self.inner = EngineInner::CardMatch(state);
            }
            GameSpec::MiniAppHtml {
                bot_username,
                short_name,
                html_bundle_media_id,
            } => {
                self.inner = EngineInner::MiniAppHtml {
                    bot_username: bot_username.clone(),
                    short_name: short_name.clone(),
                    html_bundle_media_id: html_bundle_media_id.clone(),
                };
            }
        }
        self.evaluate_achievements();
    }

    fn begin_playing_if_ready(&mut self) {
        if self.status == GameStatus::Ready {
            self.status = GameStatus::Playing;
            self.sessions_started = self.sessions_started.saturating_add(1);
        }
    }

    fn add_score(&mut self, delta: i64) {
        self.score = self.score.saturating_add(delta);
        if self.score > self.high_score {
            self.high_score = self.score;
        }
        self.evaluate_achievements();
    }

    fn evaluate_achievements(&mut self) {
        for ach in &self.achievements {
            let id = ach.achievement_id.trim();
            if id.is_empty() || self.unlocked_achievements.iter().any(|u| u == id) {
                continue;
            }
            let unlocked = match ach.target_score {
                Some(req) => self.score >= req,
                None => self.status == GameStatus::Won,
            };
            if unlocked {
                self.unlocked_achievements.push(id.to_string());
                self.newly_unlocked_achievements.push(id.to_string());
            }
        }
    }

    pub fn apply_input(&mut self, input: GameInput) {
        if matches!(
            self.status,
            GameStatus::Paused | GameStatus::Won | GameStatus::GameOver
        ) {
            return;
        }
        if matches!(self.inner, EngineInner::MiniAppHtml { .. }) {
            return;
        }
        if input == GameInput::Start {
            self.begin_playing_if_ready();
            return;
        }
        self.begin_playing_if_ready();

        let mut score_delta: i64 = 0;
        let mut next_status = self.status;
        let mut combo = self.combo;

        match &mut self.inner {
            EngineInner::GridPuzzle(gp) => {
                gp.apply_input(input, &mut score_delta, &mut combo, &mut next_status);
            }
            EngineInner::TriviaQuiz(tq) => {
                tq.apply_input(input, &mut score_delta, &mut combo, &mut next_status);
            }
            EngineInner::InteractiveFiction(inf) => {
                inf.apply_input(input, &mut score_delta, &mut next_status);
            }
            EngineInner::ArcadeDodge(ad) => {
                ad.apply_input(input);
            }
            EngineInner::SnakeGrid(sg) => {
                sg.apply_input(input);
            }
            EngineInner::CardMatch(cm) => {
                cm.apply_input(input, &mut score_delta, &mut combo, &mut next_status);
            }
            EngineInner::MiniAppHtml { .. } => {}
        }

        self.combo = combo;
        self.status = next_status;
        if score_delta != 0 {
            self.add_score(score_delta);
        } else {
            self.evaluate_achievements();
        }
    }

    pub fn step(&mut self, dt_ms: u64) {
        if self.status != GameStatus::Playing || dt_ms == 0 {
            return;
        }
        let mut score_delta: i64 = 0;
        let mut next_status = self.status;
        let mut lives = self.lives;
        let mut combo = self.combo;

        match &mut self.inner {
            EngineInner::GridPuzzle(_) | EngineInner::InteractiveFiction(_) => {
                self.elapsed_ms = self.elapsed_ms.saturating_add(dt_ms);
            }
            EngineInner::TriviaQuiz(tq) => {
                self.elapsed_ms = self.elapsed_ms.saturating_add(dt_ms);
                if let Some(rem) = tq.remaining_ms.as_mut() {
                    *rem = rem.saturating_sub(dt_ms);
                    if *rem == 0 {
                        next_status = GameStatus::GameOver;
                    }
                }
            }
            EngineInner::ArcadeDodge(ad) => {
                let cap = (ad.tick_ms as u64).saturating_mul(MAX_TICKS_PER_STEP);
                let actual_dt = dt_ms.min(cap);
                self.elapsed_ms = self.elapsed_ms.saturating_add(actual_dt);
                ad.step(
                    actual_dt,
                    self.score,
                    &mut score_delta,
                    &mut lives,
                    &mut combo,
                    &mut next_status,
                );
            }
            EngineInner::SnakeGrid(sg) => {
                let cap = (sg.tick_ms as u64).saturating_mul(MAX_TICKS_PER_STEP);
                let actual_dt = dt_ms.min(cap);
                self.elapsed_ms = self.elapsed_ms.saturating_add(actual_dt);
                sg.step(
                    actual_dt,
                    self.score,
                    &mut score_delta,
                    &mut combo,
                    &mut next_status,
                );
            }
            EngineInner::CardMatch(cm) => {
                self.elapsed_ms = self.elapsed_ms.saturating_add(dt_ms);
                cm.step(dt_ms, &mut next_status);
            }
            EngineInner::MiniAppHtml { .. } => {}
        }

        self.lives = lives;
        self.combo = combo;
        self.status = next_status;
        if score_delta != 0 {
            self.add_score(score_delta);
        } else {
            self.evaluate_achievements();
        }
    }

    pub fn snapshot(&self) -> GameRuntimeSnapshot {
        let (state, save_state_json) = match &self.inner {
            EngineInner::GridPuzzle(gp) => (gp.to_snapshot(), gp.save_state_json()),
            EngineInner::TriviaQuiz(tq) => (tq.to_snapshot(), tq.save_state_json()),
            EngineInner::InteractiveFiction(inf) => (inf.to_snapshot(), inf.save_state_json()),
            EngineInner::ArcadeDodge(ad) => (ad.to_snapshot(), ad.save_state_json()),
            EngineInner::SnakeGrid(sg) => (sg.to_snapshot(), sg.save_state_json()),
            EngineInner::CardMatch(cm) => (cm.to_snapshot(), cm.save_state_json()),
            EngineInner::MiniAppHtml {
                bot_username,
                short_name,
                html_bundle_media_id,
            } => (
                GameRuntimeState::MiniAppLaunch {
                    bot_username: bot_username.clone(),
                    short_name: short_name.clone(),
                    html_bundle_media_id: html_bundle_media_id.clone(),
                },
                serde_json::json!({
                    "bot_username": bot_username,
                    "short_name": short_name,
                    "html_bundle_media_id": html_bundle_media_id,
                }),
            ),
        };
        GameRuntimeSnapshot {
            engine_kind: self.engine_kind,
            status: self.status,
            score: self.score,
            high_score: self.high_score.max(self.score),
            lives: self.lives,
            max_lives: self.max_lives,
            combo: self.combo,
            elapsed_ms: self.elapsed_ms,
            unlocked_achievements: self.unlocked_achievements.clone(),
            newly_unlocked_achievements: self.newly_unlocked_achievements.clone(),
            save_state_json,
            state,
        }
    }

    pub fn to_game_user_state(&self, prev: Option<&GameUserState>, now_ms: i64) -> GameUserState {
        let snap = self.snapshot();
        let prev_high = prev.map_or(0, |p| p.high_score);
        let prev_plays = prev.map_or(0, |p| p.play_count);
        let mut merged_achievements = prev
            .map(|p| p.unlocked_achievements.clone())
            .unwrap_or_default();
        for id in &snap.unlocked_achievements {
            if !merged_achievements.iter().any(|existing| existing == id) {
                merged_achievements.push(id.clone());
            }
        }
        GameUserState {
            high_score: snap.high_score.max(prev_high),
            play_count: prev_plays.saturating_add(self.sessions_started.max(1)),
            unlocked_achievements: merged_achievements,
            save_state_json: Some(snap.save_state_json),
            in_library: prev.map_or(true, |p| p.in_library),
            last_played_at_ms: now_ms,
        }
    }
}

fn normalize_grid(raw: &[Vec<i32>], fallback_rows: usize, fallback_cols: usize) -> Vec<Vec<i32>> {
    let fb_r = fallback_rows.clamp(2, MAX_GRID_PUZZLE_DIM);
    let fb_c = fallback_cols.clamp(2, MAX_GRID_PUZZLE_DIM);
    if raw.is_empty() || raw.iter().all(Vec::is_empty) {
        return vec![vec![0; fb_c]; fb_r];
    }
    let rows_slice = &raw[..raw.len().min(MAX_GRID_PUZZLE_DIM)];
    if rows_slice.iter().all(Vec::is_empty) {
        return vec![vec![0; fb_c]; fb_r];
    }
    let cols = rows_slice
        .iter()
        .map(|row| row.len())
        .max()
        .unwrap_or(fb_c)
        .clamp(1, MAX_GRID_PUZZLE_DIM);
    rows_slice
        .iter()
        .map(|row| {
            let mut normalized: Vec<i32> = row.iter().take(cols).copied().collect();
            normalized.resize(cols, 0);
            normalized
        })
        .collect()
}

fn is_sliding_permutation(grid: &[Vec<i32>]) -> bool {
    let rows = grid.len();
    let cols = grid.first().map_or(0, Vec::len);
    let total = rows.saturating_mul(cols);
    if total < 2 {
        return false;
    }
    let mut seen = vec![false; total];
    for row in grid {
        for &val in row {
            if val < 0 || (val as usize) >= total {
                return false;
            }
            if seen[val as usize] {
                return false;
            }
            seen[val as usize] = true;
        }
    }
    seen.into_iter().all(|v| v)
}

fn canonical_sliding_target(rows: usize, cols: usize) -> Vec<Vec<i32>> {
    let r = rows.min(MAX_GRID_PUZZLE_DIM);
    let c = cols.min(MAX_GRID_PUZZLE_DIM);
    let total = r.saturating_mul(c);
    let mut out = vec![vec![0; c]; r];
    let mut val: usize = 1;
    for row in &mut out {
        for cell in row.iter_mut() {
            if val < total {
                *cell = i32::try_from(val).unwrap_or(i32::MAX);
                val = val.saturating_add(1);
            } else {
                *cell = 0;
            }
        }
    }
    out
}

fn detect_grid_mode(grid: &[Vec<i32>], target_grid: Option<&Vec<Vec<i32>>>) -> GridPuzzleMode {
    if target_grid.is_some() || is_sliding_permutation(grid) {
        return GridPuzzleMode::SlidingTile;
    }
    let total_cells = grid.len().saturating_mul(grid.first().map_or(0, Vec::len));
    let mut zero_count = 0usize;
    let mut has_neg_wall = false;
    let mut has_player = false;
    let mut has_maze_obj = false;
    for row in grid {
        for &v in row {
            if v == 0 {
                zero_count += 1;
            } else if v == -1 {
                has_neg_wall = true;
            } else if v == 2 {
                has_player = true;
            } else if matches!(v, 3 | 4 | 5 | 6) {
                has_maze_obj = true;
            }
        }
    }
    if has_neg_wall
        || (has_player && (has_maze_obj || zero_count > 1))
        || zero_count != 1
        || total_cells < 2
    {
        GridPuzzleMode::MazeSokoban
    } else {
        GridPuzzleMode::SlidingTile
    }
}

impl GridPuzzleState {
    fn new(rows: u8, cols: u8, levels: &[GridPuzzleLevel]) -> (Self, GameStatus) {
        let clamped_rows = (rows as usize).clamp(2, MAX_GRID_PUZZLE_DIM);
        let clamped_cols = (cols as usize).clamp(2, MAX_GRID_PUZZLE_DIM);
        let mut state = Self {
            default_rows: clamped_rows,
            default_cols: clamped_cols,
            levels: levels.to_vec(),
            level_index: 0,
            mode: GridPuzzleMode::SlidingTile,
            rows: clamped_rows,
            cols: clamped_cols,
            grid: Vec::new(),
            target_grid: None,
            moves_used: 0,
            player_pos: None,
            walls: HashSet::new(),
            goals: HashSet::new(),
            targets: HashSet::new(),
            crates: HashSet::new(),
        };
        if state.levels.is_empty() {
            return (state, GameStatus::GameOver);
        }
        state.load_level(0);
        while state.is_level_solved() {
            if state.level_index + 1 < state.levels.len() {
                let next_idx = state.level_index + 1;
                state.load_level(next_idx);
            } else {
                return (state, GameStatus::Won);
            }
        }
        if state.current_max_moves() == Some(0) {
            return (state, GameStatus::GameOver);
        }
        (state, GameStatus::Ready)
    }

    fn current_max_moves(&self) -> Option<u32> {
        self.levels.get(self.level_index).and_then(|l| l.max_moves)
    }

    fn load_level(&mut self, index: usize) {
        self.level_index = index;
        self.moves_used = 0;
        self.walls.clear();
        self.goals.clear();
        self.targets.clear();
        self.crates.clear();
        self.player_pos = None;

        let Some(level) = self.levels.get(index) else {
            return;
        };
        let grid = normalize_grid(&level.initial_grid, self.default_rows, self.default_cols);
        self.rows = grid.len();
        self.cols = grid.first().map_or(self.default_cols, Vec::len);
        let target = level
            .target_grid
            .as_ref()
            .filter(|tg| !tg.iter().take(MAX_GRID_PUZZLE_DIM).all(Vec::is_empty))
            .map(|tg| normalize_grid(tg, self.rows, self.cols));
        self.mode = detect_grid_mode(&grid, target.as_ref());
        self.grid = grid;

        match self.mode {
            GridPuzzleMode::SlidingTile => {
                self.target_grid =
                    Some(target.unwrap_or_else(|| canonical_sliding_target(self.rows, self.cols)));
            }
            GridPuzzleMode::MazeSokoban => {
                self.target_grid = None;
                for (r, row) in self.grid.iter().enumerate() {
                    for (c, &cell) in row.iter().enumerate() {
                        match cell {
                            -1 | 1 => {
                                self.walls.insert((r, c));
                            }
                            2 => {
                                self.player_pos = Some((r, c));
                            }
                            3 => {
                                self.goals.insert((r, c));
                            }
                            4 => {
                                self.crates.insert((r, c));
                            }
                            5 => {
                                self.targets.insert((r, c));
                            }
                            6 => {
                                self.targets.insert((r, c));
                                self.crates.insert((r, c));
                            }
                            _ => {}
                        }
                    }
                }
                self.rebuild_maze_grid();
            }
        }
    }

    fn rebuild_maze_grid(&mut self) {
        let mut next = vec![vec![0; self.cols]; self.rows];
        for &(r, c) in &self.walls {
            next[r][c] = -1;
        }
        for &(r, c) in &self.goals {
            next[r][c] = 3;
        }
        for &(r, c) in &self.targets {
            next[r][c] = 5;
        }
        for &(r, c) in &self.crates {
            next[r][c] = if self.targets.contains(&(r, c)) { 6 } else { 4 };
        }
        if let Some((pr, pc)) = self.player_pos {
            next[pr][pc] = 2;
        }
        self.grid = next;
    }

    fn find_blank(&self) -> Option<(usize, usize)> {
        for (r, row) in self.grid.iter().enumerate() {
            for (c, &val) in row.iter().enumerate() {
                if val == 0 {
                    return Some((r, c));
                }
            }
        }
        None
    }

    fn is_level_solved(&self) -> bool {
        match self.mode {
            GridPuzzleMode::SlidingTile => self
                .target_grid
                .as_ref()
                .map_or(false, |target| self.grid == *target),
            GridPuzzleMode::MazeSokoban => {
                if !self.targets.is_empty() && !self.goals.is_empty() {
                    self.targets.iter().all(|t| self.crates.contains(t))
                        && self
                            .player_pos
                            .map_or(false, |pos| self.goals.contains(&pos))
                } else if !self.targets.is_empty() {
                    self.targets.iter().all(|t| self.crates.contains(t))
                } else if !self.goals.is_empty() {
                    self.player_pos
                        .map_or(false, |pos| self.goals.contains(&pos))
                } else {
                    false
                }
            }
        }
    }

    fn try_sliding_move(&mut self, input: GameInput) -> bool {
        let Some((zr, zc)) = self.find_blank() else {
            return false;
        };
        let src = match input {
            GameInput::MoveUp => zr
                .checked_add(1)
                .filter(|&r| r < self.rows)
                .map(|r| (r, zc)),
            GameInput::MoveDown => zr.checked_sub(1).map(|r| (r, zc)),
            GameInput::MoveLeft => zc
                .checked_add(1)
                .filter(|&c| c < self.cols)
                .map(|c| (zr, c)),
            GameInput::MoveRight => zc.checked_sub(1).map(|c| (zr, c)),
            GameInput::ClickCell { row, col } => (row < self.rows
                && col < self.cols
                && row.abs_diff(zr).saturating_add(col.abs_diff(zc)) == 1)
                .then_some((row, col)),
            _ => None,
        };
        let Some((sr, sc)) = src else {
            return false;
        };
        let tile = self.grid[sr][sc];
        self.grid[zr][zc] = tile;
        self.grid[sr][sc] = 0;
        true
    }

    fn try_maze_move(&mut self, input: GameInput) -> bool {
        let Some((pr, pc)) = self.player_pos else {
            return false;
        };
        let delta: Option<(isize, isize)> = match input {
            GameInput::MoveUp => Some((-1, 0)),
            GameInput::MoveDown => Some((1, 0)),
            GameInput::MoveLeft => Some((0, -1)),
            GameInput::MoveRight => Some((0, 1)),
            GameInput::ClickCell { row, col } => {
                if row < self.rows
                    && col < self.cols
                    && row.abs_diff(pr).saturating_add(col.abs_diff(pc)) == 1
                {
                    Some((row as isize - pr as isize, col as isize - pc as isize))
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some((dr, dc)) = delta else {
            return false;
        };
        let nr = pr as isize + dr;
        let nc = pc as isize + dc;
        if nr < 0 || nc < 0 || nr as usize >= self.rows || nc as usize >= self.cols {
            return false;
        }
        let next_pos = (nr as usize, nc as usize);
        if self.walls.contains(&next_pos) {
            return false;
        }
        if self.crates.contains(&next_pos) {
            let push_r = nr + dr;
            let push_c = nc + dc;
            if push_r < 0
                || push_c < 0
                || push_r as usize >= self.rows
                || push_c as usize >= self.cols
            {
                return false;
            }
            let push_pos = (push_r as usize, push_c as usize);
            if self.walls.contains(&push_pos) || self.crates.contains(&push_pos) {
                return false;
            }
            self.crates.remove(&next_pos);
            self.crates.insert(push_pos);
        }
        self.player_pos = Some(next_pos);
        self.rebuild_maze_grid();
        true
    }

    fn apply_input(
        &mut self,
        input: GameInput,
        score_delta: &mut i64,
        combo: &mut u32,
        status: &mut GameStatus,
    ) {
        let moved = match self.mode {
            GridPuzzleMode::SlidingTile => self.try_sliding_move(input),
            GridPuzzleMode::MazeSokoban => self.try_maze_move(input),
        };
        if !moved {
            return;
        }
        self.moves_used = self.moves_used.saturating_add(1);
        let max_moves = self.current_max_moves();

        if max_moves.is_some_and(|limit| self.moves_used > limit) {
            *combo = 0;
            *status = GameStatus::GameOver;
            return;
        }

        if self.is_level_solved() {
            *combo = combo.saturating_add(1);
            let efficiency_bonus = max_moves
                .map(|m| (m.saturating_sub(self.moves_used) as i64).saturating_mul(10))
                .unwrap_or(0);
            *score_delta = score_delta.saturating_add(100i64.saturating_add(efficiency_bonus));
            loop {
                if self.level_index + 1 < self.levels.len() {
                    let next_idx = self.level_index + 1;
                    self.load_level(next_idx);
                    if !self.is_level_solved() {
                        if self.current_max_moves() == Some(0) {
                            *combo = 0;
                            *status = GameStatus::GameOver;
                        }
                        break;
                    }
                } else {
                    *status = GameStatus::Won;
                    break;
                }
            }
        } else if let Some(limit) = max_moves {
            if self.moves_used >= limit {
                *combo = 0;
                *status = GameStatus::GameOver;
            }
        }
    }

    fn to_snapshot(&self) -> GameRuntimeState {
        let level = self.levels.get(self.level_index);
        GameRuntimeState::GridPuzzle {
            mode: self.mode,
            rows: self.rows,
            cols: self.cols,
            level_index: self.level_index,
            total_levels: self.levels.len(),
            level_id: level.map(|l| l.level_id.clone()).unwrap_or_default(),
            level_title: level.map(|l| l.title.clone()).unwrap_or_default(),
            grid: self.grid.clone(),
            target_grid: self.target_grid.clone(),
            moves_used: self.moves_used,
            max_moves: level.and_then(|l| l.max_moves),
            hint: level.and_then(|l| l.hint.clone()),
            player_pos: self.player_pos,
        }
    }

    fn save_state_json(&self) -> serde_json::Value {
        let current_level_id = self
            .levels
            .get(self.level_index)
            .map(|l| l.level_id.as_str())
            .unwrap_or("");
        serde_json::json!({
            "current_level_id": current_level_id,
            "level_index": self.level_index,
            "moves_used": self.moves_used,
        })
    }
}

impl TriviaQuizState {
    fn apply_input(
        &mut self,
        input: GameInput,
        score_delta: &mut i64,
        combo: &mut u32,
        status: &mut GameStatus,
    ) {
        let Some(question) = self.questions.get(self.current_index) else {
            *status = GameStatus::GameOver;
            return;
        };
        match input {
            GameInput::SelectOption(idx) => {
                if self.awaiting_next || idx >= question.choices.len() {
                    return;
                }
                let correct = (question.correct_index as usize) < question.choices.len()
                    && idx == question.correct_index as usize;
                self.selected_choice = Some(idx);
                self.last_answer_correct = Some(correct);
                self.awaiting_next = true;
                if correct {
                    self.correct_count = self.correct_count.saturating_add(1);
                    *combo = combo.saturating_add(1);
                    let streak_bonus = (combo.saturating_sub(1) as i64).saturating_mul(25);
                    let award = (question.points as i64).saturating_add(streak_bonus);
                    *score_delta = score_delta.saturating_add(award);
                } else {
                    *combo = 0;
                }
            }
            GameInput::Confirm | GameInput::PrimaryAction => {
                if !self.awaiting_next && !question.choices.is_empty() {
                    return;
                }
                if self.current_index + 1 < self.questions.len() {
                    self.current_index += 1;
                    self.selected_choice = None;
                    self.last_answer_correct = None;
                    self.awaiting_next = false;
                } else {
                    *status = GameStatus::Won;
                }
            }
            _ => {}
        }
    }

    fn to_snapshot(&self) -> GameRuntimeState {
        let q = self.questions.get(self.current_index);
        let correct_idx = q.and_then(|item| {
            let idx = item.correct_index as usize;
            (idx < item.choices.len()).then_some(idx)
        });
        GameRuntimeState::TriviaQuiz {
            current_question_index: self.current_index,
            total_questions: self.questions.len(),
            question_id: q.map(|item| item.question_id.clone()).unwrap_or_default(),
            prompt: q.map(|item| item.prompt.clone()).unwrap_or_default(),
            choices: q.map(|item| item.choices.clone()).unwrap_or_default(),
            selected_choice: self.selected_choice,
            correct_index: if self.awaiting_next { correct_idx } else { None },
            last_answer_correct: self.last_answer_correct,
            explanation: if self.awaiting_next {
                q.and_then(|item| item.explanation.clone())
            } else {
                None
            },
            awaiting_next: self.awaiting_next,
            correct_count: self.correct_count,
            remaining_ms: self.remaining_ms,
        }
    }

    fn save_state_json(&self) -> serde_json::Value {
        let question_id = self
            .questions
            .get(self.current_index)
            .map(|q| q.question_id.as_str())
            .unwrap_or("");
        serde_json::json!({
            "current_question_index": self.current_index,
            "question_id": question_id,
            "correct_count": self.correct_count,
            "remaining_ms": self.remaining_ms,
        })
    }
}

impl InteractiveFictionState {
    fn new(start_node_id: &str, nodes: &[StoryNode]) -> (Self, GameStatus) {
        let mut nodes_by_id = HashMap::new();
        let mut first_id = String::new();
        for node in nodes {
            let trimmed = node.node_id.trim().to_string();
            if trimmed.is_empty() {
                continue;
            }
            if first_id.is_empty() {
                first_id = trimmed.clone();
            }
            nodes_by_id.entry(trimmed).or_insert_with(|| node.clone());
        }
        let requested_start = start_node_id.trim();
        let current_node_id = if nodes_by_id.contains_key(requested_start) {
            requested_start.to_string()
        } else {
            first_id
        };
        if current_node_id.is_empty() {
            return (
                Self {
                    nodes_by_id,
                    current_node_id,
                    visited_node_ids: Vec::new(),
                },
                GameStatus::GameOver,
            );
        }
        let status = match nodes_by_id.get(&current_node_id) {
            Some(node) if node.is_ending => GameStatus::Won,
            Some(node) if node.choices.is_empty() => GameStatus::GameOver,
            Some(_) => GameStatus::Ready,
            None => GameStatus::GameOver,
        };
        let visited_node_ids = vec![current_node_id.clone()];
        (
            Self {
                nodes_by_id,
                current_node_id,
                visited_node_ids,
            },
            status,
        )
    }

    fn apply_input(
        &mut self,
        input: GameInput,
        score_delta: &mut i64,
        status: &mut GameStatus,
    ) {
        let GameInput::SelectOption(idx) = input else {
            return;
        };
        let Some(current) = self.nodes_by_id.get(&self.current_node_id) else {
            *status = GameStatus::GameOver;
            return;
        };
        let Some(choice) = current.choices.get(idx).cloned() else {
            return;
        };
        *score_delta = score_delta.saturating_add(choice.score_delta as i64);
        let target_id = choice.target_node_id.trim().to_string();
        if let Some(target_node) = self.nodes_by_id.get(&target_id) {
            self.current_node_id = target_id.clone();
            self.visited_node_ids.push(target_id);
            if target_node.is_ending {
                *status = GameStatus::Won;
            } else if target_node.choices.is_empty() {
                *status = GameStatus::GameOver;
            }
        } else {
            *status = GameStatus::GameOver;
        }
    }

    fn to_snapshot(&self) -> GameRuntimeState {
        let node = self.nodes_by_id.get(&self.current_node_id);
        let choices = node
            .map(|n| {
                n.choices
                    .iter()
                    .enumerate()
                    .map(|(index, c)| StoryChoiceSnapshot {
                        index,
                        label: c.label.clone(),
                        target_node_id: c.target_node_id.trim().to_string(),
                        score_delta: c.score_delta,
                    })
                    .collect()
            })
            .unwrap_or_default();
        GameRuntimeState::InteractiveFiction {
            current_node_id: self.current_node_id.clone(),
            title: node.map(|n| n.title.clone()).unwrap_or_default(),
            body: node.map(|n| n.body.clone()).unwrap_or_default(),
            choices,
            visited_node_ids: self.visited_node_ids.clone(),
            is_ending: node.map_or(false, |n| n.is_ending),
            ending_badge: node.and_then(|n| n.ending_badge.clone()),
        }
    }

    fn save_state_json(&self) -> serde_json::Value {
        serde_json::json!({
            "current_node_id": self.current_node_id,
            "visited_node_ids": self.visited_node_ids,
        })
    }
}

impl ArcadeDodgeState {
    #[allow(clippy::too_many_arguments)]
    fn new(
        tick_ms: u32,
        arena_width: u32,
        arena_height: u32,
        player_speed: f32,
        obstacle_speed: f32,
        target_score: u32,
        theme_accent_hex: Option<String>,
        seed: u64,
    ) -> Self {
        let w = arena_width.clamp(100, 10_000);
        let h = arena_height.clamp(100, 10_000);
        let safe_player_speed = if player_speed.is_finite() && player_speed > 0.0 {
            player_speed.clamp(1.0, 10_000.0)
        } else {
            200.0
        };
        let safe_obstacle_speed = if obstacle_speed.is_finite() && obstacle_speed > 0.0 {
            obstacle_speed.clamp(1.0, 10_000.0)
        } else {
            140.0
        };
        let player_radius = ((w.min(h) as f32) * 0.045).clamp(8.0, 20.0);
        Self {
            arena_width: w,
            arena_height: h,
            tick_ms: tick_ms.clamp(10, 1_000),
            player_speed: safe_player_speed,
            obstacle_speed: safe_obstacle_speed,
            target_score: target_score.max(1),
            theme_accent_hex,
            player_x: (w as f32) * 0.5,
            player_y: (h as f32) * 0.82,
            player_radius,
            invulnerable_ms: 0,
            accum_ms: 0,
            spawn_cooldown_ms: 120,
            next_entity_id: 1,
            entities: Vec::new(),
            ticks_simulated: 0,
            rng: SplitMix64::new(seed),
        }
    }

    fn apply_input(&mut self, input: GameInput) {
        let dt_sec = (self.tick_ms as f32) / 1000.0;
        let step_dist = (self.player_speed * dt_sec * 3.0).max(8.0);
        match input {
            GameInput::MoveLeft => self.player_x -= step_dist,
            GameInput::MoveRight => self.player_x += step_dist,
            GameInput::MoveUp => self.player_y -= step_dist,
            GameInput::MoveDown => self.player_y += step_dist,
            _ => return,
        }
        let min_x = self.player_radius;
        let max_x = (self.arena_width as f32) - self.player_radius;
        let min_y = self.player_radius;
        let max_y = (self.arena_height as f32) - self.player_radius;
        self.player_x = self.player_x.clamp(min_x, max_x.max(min_x));
        self.player_y = self.player_y.clamp(min_y, max_y.max(min_y));
    }

    fn spawn_entity(&mut self) {
        let is_collectible = self.rng.next_usize(10) < 3;
        let radius = if is_collectible { 10.0 } else { 14.0 };
        let usable_w = ((self.arena_width as f32) - 2.0 * radius).max(1.0);
        let x = radius + self.rng.next_f32_unit() * usable_w;
        let drift = (self.rng.next_f32_unit() - 0.5) * (self.obstacle_speed * 0.3);
        let speed_factor = if is_collectible {
            0.8
        } else {
            0.85 + self.rng.next_f32_unit() * 0.4
        };
        let color_hex = if is_collectible {
            "#34C759".to_string()
        } else {
            self.theme_accent_hex
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "#FF3B30".to_string())
        };
        let entity = ArcadeEntitySnapshot {
            entity_id: self.next_entity_id,
            kind: if is_collectible {
                ArcadeEntityKind::Collectible
            } else {
                ArcadeEntityKind::Hazard
            },
            x,
            y: -radius,
            radius,
            vx: drift,
            vy: self.obstacle_speed * speed_factor,
            color_hex,
        };
        self.next_entity_id = self.next_entity_id.saturating_add(1);
        self.entities.push(entity);
    }

    fn step(
        &mut self,
        dt_ms: u64,
        base_score: i64,
        score_delta: &mut i64,
        lives: &mut u8,
        combo: &mut u32,
        status: &mut GameStatus,
    ) {
        let tick_u64 = self.tick_ms as u64;
        self.accum_ms = self.accum_ms.saturating_add(dt_ms);
        let mut ticks = 0u64;
        let dt_sec = (self.tick_ms as f32) / 1000.0;
        let max_w = self.arena_width as f32;
        let max_h = self.arena_height as f32;

        while self.accum_ms >= tick_u64 && ticks < MAX_TICKS_PER_STEP && *status == GameStatus::Playing
        {
            self.accum_ms -= tick_u64;
            ticks += 1;
            self.ticks_simulated = self.ticks_simulated.saturating_add(1);
            self.invulnerable_ms = self.invulnerable_ms.saturating_sub(tick_u64);

            if self.spawn_cooldown_ms <= tick_u64 {
                self.spawn_entity();
                self.spawn_cooldown_ms = 280 + (self.rng.next_usize(180) as u64);
            } else {
                self.spawn_cooldown_ms -= tick_u64;
            }

            let mut surviving = Vec::with_capacity(self.entities.len());
            for mut ent in self.entities.drain(..) {
                ent.x += ent.vx * dt_sec;
                ent.y += ent.vy * dt_sec;
                if ent.x < ent.radius {
                    ent.x = ent.radius;
                    ent.vx = ent.vx.abs();
                } else if ent.x > max_w - ent.radius {
                    ent.x = max_w - ent.radius;
                    ent.vx = -ent.vx.abs();
                }

                let dx = ent.x - self.player_x;
                let dy = ent.y - self.player_y;
                let hit_dist = ent.radius + self.player_radius;
                if dx * dx + dy * dy <= hit_dist * hit_dist {
                    match ent.kind {
                        ArcadeEntityKind::Collectible => {
                            *combo = combo.saturating_add(1);
                            let bonus = 50i64
                                .saturating_add((combo.saturating_sub(1) as i64).saturating_mul(10));
                            *score_delta = score_delta.saturating_add(bonus);
                            continue;
                        }
                        ArcadeEntityKind::Hazard => {
                            if self.invulnerable_ms == 0 {
                                *lives = lives.saturating_sub(1);
                                *combo = 0;
                                self.invulnerable_ms = ARCADE_INVULNERABILITY_MS;
                                if *lives == 0 {
                                    *status = GameStatus::GameOver;
                                }
                            }
                            continue;
                        }
                    }
                }

                if ent.y - ent.radius > max_h {
                    if ent.kind == ArcadeEntityKind::Hazard {
                        *combo = combo.saturating_add(1);
                        *score_delta = score_delta.saturating_add(10);
                    }
                    continue;
                }
                surviving.push(ent);
            }
            self.entities = surviving;

            if *status == GameStatus::Playing
                && base_score.saturating_add(*score_delta) >= self.target_score as i64
            {
                *status = GameStatus::Won;
                break;
            }
        }
    }

    fn to_snapshot(&self) -> GameRuntimeState {
        GameRuntimeState::ArcadeDodge {
            arena_width: self.arena_width,
            arena_height: self.arena_height,
            tick_ms: self.tick_ms,
            player_x: self.player_x,
            player_y: self.player_y,
            player_radius: self.player_radius,
            invulnerable_ms: self.invulnerable_ms,
            target_score: self.target_score,
            theme_accent_hex: self.theme_accent_hex.clone(),
            entities: self.entities.clone(),
            ticks_simulated: self.ticks_simulated,
        }
    }

    fn save_state_json(&self) -> serde_json::Value {
        serde_json::json!({
            "ticks_simulated": self.ticks_simulated,
            "player_x": self.player_x,
            "player_y": self.player_y,
            "active_entities": self.entities.len(),
        })
    }
}

impl SnakeGridState {
    fn new(
        grid_width: u16,
        grid_height: u16,
        tick_ms: u32,
        wrap_walls: bool,
        target_score: u32,
        seed: u64,
    ) -> Self {
        let w = (grid_width as usize).clamp(5, 256);
        let h = (grid_height as usize).clamp(5, 256);
        let cy = (h / 2) as i32;
        let cx = (w / 2).max(2) as i32;
        let segments = vec![
            GridCoord { x: cx, y: cy },
            GridCoord { x: cx - 1, y: cy },
            GridCoord { x: cx - 2, y: cy },
        ];
        let mut state = Self {
            grid_width: w,
            grid_height: h,
            tick_ms: tick_ms.clamp(20, 5_000),
            wrap_walls,
            target_score: target_score.max(1),
            direction: SnakeDirection::Right,
            queued_direction: None,
            segments,
            food: None,
            accum_ms: 0,
            ticks_simulated: 0,
            rng: SplitMix64::new(seed),
        };
        state.food = state.spawn_food();
        state
    }

    fn spawn_food(&mut self) -> Option<GridCoord> {
        let occupied: HashSet<GridCoord> = self.segments.iter().copied().collect();
        let mut free = Vec::new();
        for y in 0..(self.grid_height as i32) {
            for x in 0..(self.grid_width as i32) {
                let c = GridCoord { x, y };
                if !occupied.contains(&c) {
                    free.push(c);
                }
            }
        }
        if free.is_empty() {
            return None;
        }
        let idx = self.rng.next_usize(free.len());
        Some(free[idx])
    }

    fn apply_input(&mut self, input: GameInput) {
        let requested = match input {
            GameInput::MoveUp => SnakeDirection::Up,
            GameInput::MoveDown => SnakeDirection::Down,
            GameInput::MoveLeft => SnakeDirection::Left,
            GameInput::MoveRight => SnakeDirection::Right,
            _ => return,
        };
        if !self.direction.is_opposite(requested) {
            self.queued_direction = Some(requested);
        }
    }

    fn step(
        &mut self,
        dt_ms: u64,
        base_score: i64,
        score_delta: &mut i64,
        combo: &mut u32,
        status: &mut GameStatus,
    ) {
        let tick_u64 = self.tick_ms as u64;
        self.accum_ms = self.accum_ms.saturating_add(dt_ms);
        let mut ticks = 0u64;
        let w = self.grid_width as i32;
        let h = self.grid_height as i32;

        while self.accum_ms >= tick_u64 && ticks < MAX_TICKS_PER_STEP && *status == GameStatus::Playing
        {
            self.accum_ms -= tick_u64;
            ticks += 1;
            self.ticks_simulated = self.ticks_simulated.saturating_add(1);

            if let Some(next_dir) = self.queued_direction.take() {
                if !self.direction.is_opposite(next_dir) {
                    self.direction = next_dir;
                }
            }
            let Some(head) = self.segments.first().copied() else {
                *status = GameStatus::GameOver;
                break;
            };
            let (dx, dy) = self.direction.delta();
            let mut nx = head.x + dx;
            let mut ny = head.y + dy;
            if self.wrap_walls {
                nx = nx.rem_euclid(w);
                ny = ny.rem_euclid(h);
            } else if nx < 0 || ny < 0 || nx >= w || ny >= h {
                *status = GameStatus::GameOver;
                break;
            }
            let next_head = GridCoord { x: nx, y: ny };
            let will_eat = self.food == Some(next_head);

            let check_len = if will_eat {
                self.segments.len()
            } else {
                self.segments.len().saturating_sub(1)
            };
            if self.segments[..check_len].contains(&next_head) {
                *status = GameStatus::GameOver;
                break;
            }

            self.segments.insert(0, next_head);
            if will_eat {
                *combo = combo.saturating_add(1);
                *score_delta = score_delta.saturating_add(10);
                if base_score.saturating_add(*score_delta) >= self.target_score as i64 {
                    self.food = None;
                    *status = GameStatus::Won;
                    break;
                }
                self.food = self.spawn_food();
                if self.food.is_none() {
                    *status = GameStatus::Won;
                    break;
                }
            } else {
                self.segments.pop();
            }
        }
    }

    fn to_snapshot(&self) -> GameRuntimeState {
        GameRuntimeState::SnakeGrid {
            grid_width: self.grid_width,
            grid_height: self.grid_height,
            tick_ms: self.tick_ms,
            wrap_walls: self.wrap_walls,
            target_score: self.target_score,
            direction: self.direction,
            segments: self.segments.clone(),
            food: self.food,
            ticks_simulated: self.ticks_simulated,
        }
    }

    fn save_state_json(&self) -> serde_json::Value {
        serde_json::json!({
            "ticks_simulated": self.ticks_simulated,
            "snake_length": self.segments.len(),
            "wrap_walls": self.wrap_walls,
        })
    }
}

impl CardMatchState {
    fn new(pairs: &[CardPair], time_limit_secs: Option<u32>, seed: u64) -> (Self, GameStatus) {
        let remaining_ms = time_limit_secs
            .filter(|&s| s > 0)
            .map(|s| (s as u64).saturating_mul(1000));
        if pairs.is_empty() {
            return (
                Self {
                    rows: 0,
                    cols: 0,
                    cards: Vec::new(),
                    moves: 0,
                    matched_pairs: 0,
                    total_pairs: 0,
                    first_open: None,
                    pending_mismatch: None,
                    mismatch_timer_ms: 0,
                    remaining_ms,
                },
                GameStatus::GameOver,
            );
        }

        let pair_count = pairs.len().min(MAX_CARD_MATCH_PAIRS);
        let mut deck: Vec<CardPair> = Vec::with_capacity(pair_count.saturating_mul(2));
        for pair in pairs.iter().take(MAX_CARD_MATCH_PAIRS) {
            deck.push(pair.clone());
            deck.push(pair.clone());
        }
        let mut rng = SplitMix64::new(seed);
        for i in (1..deck.len()).rev() {
            let j = rng.next_usize(i + 1);
            deck.swap(i, j);
        }

        let total = deck.len();
        let cols = ((total as f64).sqrt().ceil() as usize).max(2).min(total);
        let rows = total.div_ceil(cols);
        let cards = deck
            .into_iter()
            .enumerate()
            .map(|(index, pair)| CardCellSnapshot {
                index,
                row: index / cols,
                col: index % cols,
                pair_id: pair.pair_id,
                label: pair.label,
                symbol: pair.symbol,
                accent_hex: pair.accent_hex,
                face: CardFaceState::FaceDown,
            })
            .collect();

        (
            Self {
                rows,
                cols,
                cards,
                moves: 0,
                matched_pairs: 0,
                total_pairs: pair_count,
                first_open: None,
                pending_mismatch: None,
                mismatch_timer_ms: 0,
                remaining_ms,
            },
            GameStatus::Ready,
        )
    }

    fn apply_input(
        &mut self,
        input: GameInput,
        score_delta: &mut i64,
        combo: &mut u32,
        status: &mut GameStatus,
    ) {
        if self.mismatch_timer_ms > 0 {
            return;
        }
        let idx = match input {
            GameInput::SelectOption(i) => (i < self.cards.len()).then_some(i),
            GameInput::ClickCell { row, col } => {
                if row < self.rows && col < self.cols {
                    row.checked_mul(self.cols)
                        .and_then(|base| base.checked_add(col))
                        .filter(|&i| i < self.cards.len())
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some(idx) = idx else {
            return;
        };
        if self.cards[idx].face != CardFaceState::FaceDown {
            return;
        }
        self.cards[idx].face = CardFaceState::FaceUp;

        match self.first_open {
            None => {
                self.first_open = Some(idx);
            }
            Some(first) => {
                self.first_open = None;
                self.moves = self.moves.saturating_add(1);
                if self.cards[first].pair_id == self.cards[idx].pair_id {
                    self.cards[first].face = CardFaceState::Matched;
                    self.cards[idx].face = CardFaceState::Matched;
                    self.matched_pairs = self.matched_pairs.saturating_add(1);
                    *combo = combo.saturating_add(1);
                    let bonus = (combo.saturating_sub(1) as i64).saturating_mul(25);
                    *score_delta = score_delta.saturating_add(100i64.saturating_add(bonus));
                    if self.matched_pairs >= self.total_pairs {
                        *status = GameStatus::Won;
                    }
                } else {
                    *combo = 0;
                    self.pending_mismatch = Some((first, idx));
                    self.mismatch_timer_ms = CARD_MISMATCH_FLIP_BACK_MS;
                }
            }
        }
    }

    fn step(&mut self, dt_ms: u64, status: &mut GameStatus) {
        if self.mismatch_timer_ms > 0 {
            self.mismatch_timer_ms = self.mismatch_timer_ms.saturating_sub(dt_ms);
            if self.mismatch_timer_ms == 0 {
                if let Some((a, b)) = self.pending_mismatch.take() {
                    if let Some(card) = self.cards.get_mut(a) {
                        if card.face == CardFaceState::FaceUp {
                            card.face = CardFaceState::FaceDown;
                        }
                    }
                    if let Some(card) = self.cards.get_mut(b) {
                        if card.face == CardFaceState::FaceUp {
                            card.face = CardFaceState::FaceDown;
                        }
                    }
                }
            }
        }
        if let Some(rem) = self.remaining_ms.as_mut() {
            *rem = rem.saturating_sub(dt_ms);
            if *rem == 0 && *status == GameStatus::Playing {
                *status = GameStatus::GameOver;
            }
        }
    }

    fn to_snapshot(&self) -> GameRuntimeState {
        GameRuntimeState::CardMatch {
            rows: self.rows,
            cols: self.cols,
            cards: self.cards.clone(),
            moves: self.moves,
            matched_pairs: self.matched_pairs,
            total_pairs: self.total_pairs,
            mismatch_timer_ms: self.mismatch_timer_ms,
            remaining_ms: self.remaining_ms,
        }
    }

    fn save_state_json(&self) -> serde_json::Value {
        serde_json::json!({
            "moves": self.moves,
            "matched_pairs": self.matched_pairs,
            "total_pairs": self.total_pairs,
            "remaining_ms": self.remaining_ms,
        })
    }
}
