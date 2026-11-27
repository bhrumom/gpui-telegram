//! Interactive Apple Arcade / Games runtime sheet (`ActiveGameSession`, `render_game_runtime_sheet`)
//! driving all 7 `GameSpec` variants via `MiniGameRuntime`, native GPUI `canvas()` rendering
//! for real-time arcade/snake arenas, and `BotAttachWebView` fallback for `MiniAppHtml`.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::creations::{
    CreationPayload, CreationReviewStatus, GameAchievementDef, GameEngineKind, GamePayload,
    GameUserState, SubmitGameScoreRequest, UpsertCreationStateRequest, UserCreationDto,
    UserCreationStateDto,
};

use crate::i18n::tr_str;
use crate::theme::TdesktopThemePalette;
use crate::views::app_shell::modal_container::{
    macos_primary_pill_btn, macos_secondary_pill_btn,
};
use crate::views::content_hub::apple_primitives::{
    apple_grouped_card, format_duration_ms, parse_hex_color,
};
use crate::views::content_hub::game_runtime::{
    ArcadeEntityKind, CardFaceState, GameInput, GameRuntimeSnapshot, GameRuntimeState, GameStatus,
    MiniGameRuntime,
};
use crate::views::content_hub::{ContentHubEvent, ContentHubWorkspace};

pub const GAMES_ACCENT: u32 = 0x5856D6;

/// Active interactive game session mounted inside `ContentHubWorkspace`.
#[derive(Debug, Clone)]
pub struct ActiveGameSession {
    pub creation_id: String,
    pub title: String,
    pub developer: String,
    pub controls_hint: String,
    pub supports_leaderboard: bool,
    pub is_published: bool,
    pub achievements: Vec<GameAchievementDef>,
    pub runtime: MiniGameRuntime,
    pub score_submitted: bool,
    pub prev_user_state: Option<GameUserState>,
}

impl ActiveGameSession {
    pub fn launch(
        creation: &UserCreationDto,
        payload: &GamePayload,
        saved_state: Option<&UserCreationStateDto>,
        seed: u64,
    ) -> Self {
        let prev_user_state = saved_state
            .and_then(|s| serde_json::from_value::<GameUserState>(s.state_json.clone()).ok());
        let mut runtime = MiniGameRuntime::from_payload(payload, seed);
        // Hydrate prior high score if available by starting fresh session
        runtime.apply_input(GameInput::Start);
        let developer = if !payload.developer.trim().is_empty() {
            payload.developer.clone()
        } else {
            creation.author_display_name.clone()
        };
        Self {
            creation_id: creation.creation_id.clone(),
            title: creation.title.clone(),
            developer,
            controls_hint: payload.controls_hint.clone(),
            supports_leaderboard: payload.supports_leaderboard,
            is_published: creation.review_status == CreationReviewStatus::Published,
            achievements: payload.achievements.clone(),
            runtime,
            score_submitted: false,
            prev_user_state,
        }
    }

    pub fn snapshot(&self) -> GameRuntimeSnapshot {
        self.runtime.snapshot()
    }

    pub fn apply_input(&mut self, input: GameInput) -> GameRuntimeSnapshot {
        self.runtime.apply_input(input);
        self.runtime.snapshot()
    }

    pub fn step(&mut self, dt_ms: u64) -> GameRuntimeSnapshot {
        self.runtime.step(dt_ms);
        self.runtime.snapshot()
    }

    pub fn restart(&mut self) -> GameRuntimeSnapshot {
        self.runtime.restart();
        self.runtime.apply_input(GameInput::Start);
        self.score_submitted = false;
        self.runtime.snapshot()
    }

    pub fn needs_realtime_ticks(&self) -> bool {
        let snap = self.runtime.snapshot();
        if snap.status != GameStatus::Playing {
            return false;
        }
        matches!(
            snap.state,
            GameRuntimeState::ArcadeDodge { .. }
                | GameRuntimeState::SnakeGrid { .. }
                | GameRuntimeState::CardMatch {
                    mismatch_timer_ms: 1..,
                    ..
                }
                | GameRuntimeState::CardMatch {
                    remaining_ms: Some(_),
                    ..
                }
                | GameRuntimeState::TriviaQuiz {
                    remaining_ms: Some(_),
                    ..
                }
        )
    }

    pub fn recommended_tick_ms(&self) -> u64 {
        match self.runtime.snapshot().state {
            GameRuntimeState::ArcadeDodge { tick_ms, .. } => u64::from(tick_ms.clamp(24, 120)),
            GameRuntimeState::SnakeGrid { tick_ms, .. } => u64::from(tick_ms.clamp(60, 260)),
            _ => 100,
        }
    }

    pub fn to_upsert_state_request(&self) -> UpsertCreationStateRequest {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let state = self
            .runtime
            .to_game_user_state(self.prev_user_state.as_ref(), now_ms);
        let snap = self.runtime.snapshot();
        let progress = match snap.status {
            GameStatus::Won => 100.0,
            _ if snap.score > 0 => 50.0,
            _ => 10.0,
        };
        UpsertCreationStateRequest {
            in_library: Some(true),
            favorite: None,
            progress_percent: Some(progress),
            state_json: serde_json::to_value(state).ok(),
        }
    }

    pub fn to_submit_score_request(&self) -> Option<SubmitGameScoreRequest> {
        let snap = self.runtime.snapshot();
        let best = snap.score.max(snap.high_score);
        if best <= 0 {
            return None;
        }
        Some(SubmitGameScoreRequest {
            score: best,
            metadata: Some(serde_json::json!({
                "engine_kind": snap.engine_kind.as_str(),
                "status": format!("{:?}", snap.status).to_lowercase(),
                "combo": snap.combo,
                "elapsed_ms": snap.elapsed_ms,
                "achievements": snap.unlocked_achievements,
            })),
        })
    }
}

pub fn extract_game_payload(creation: &UserCreationDto) -> Option<GamePayload> {
    match &creation.payload {
        CreationPayload::Game(gp) => Some(gp.clone()),
        _ => None,
    }
}

fn fill_circle(cx: f32, cy: f32, radius: f32, color: Rgba, window: &mut Window) {
    let steps = 16usize;
    let mut builder = PathBuilder::fill();
    for i in 0..steps {
        let angle = (i as f32 / steps as f32) * std::f32::consts::TAU;
        let pt = point(px(cx + angle.cos() * radius), px(cy + angle.sin() * radius));
        if i == 0 {
            builder.move_to(pt);
        } else {
            builder.line_to(pt);
        }
    }
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn render_arcade_dodge_canvas(
    arena_width: u32,
    arena_height: u32,
    player_x: f32,
    player_y: f32,
    player_radius: f32,
    invulnerable_ms: u64,
    theme_accent_hex: Option<String>,
    entities: Vec<crate::views::content_hub::game_runtime::ArcadeEntitySnapshot>,
) -> AnyElement {
    let accent = theme_accent_hex
        .as_deref()
        .map(|h| parse_hex_color(h, GAMES_ACCENT))
        .unwrap_or(GAMES_ACCENT);
    let aw = (arena_width.max(100)) as f32;
    let ah = (arena_height.max(100)) as f32;

    div()
        .id("game-arcade-dodge-canvas-wrap")
        .w(px(520.0))
        .max_w_full()
        .h(px(340.0))
        .rounded(px(14.0))
        .overflow_hidden()
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(accent, 0.55))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    // Arena dark arcade background
                    window.paint_quad(gpui::fill(bounds, rgb(0x101322)));
                    let ox: f32 = bounds.origin.x.into();
                    let oy: f32 = bounds.origin.y.into();
                    let bw: f32 = bounds.size.width.into();
                    let bh: f32 = bounds.size.height.into();
                    let sx = bw / aw;
                    let sy = bh / ah;

                    // Subtle neon grid lines
                    let grid_col = TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.05);
                    for i in 1..8 {
                        let gx = ox + bw * (i as f32 / 8.0);
                        window.paint_quad(gpui::fill(
                            Bounds::new(point(px(gx), px(oy)), size(px(1.0), px(bh))),
                            grid_col,
                        ));
                    }
                    for j in 1..6 {
                        let gy = oy + bh * (j as f32 / 6.0);
                        window.paint_quad(gpui::fill(
                            Bounds::new(point(px(ox), px(gy)), size(px(bw), px(1.0))),
                            grid_col,
                        ));
                    }

                    // Entities (collectibles & hazards)
                    for ent in &entities {
                        let ex = ox + ent.x * sx;
                        let ey = oy + ent.y * sy;
                        let er = (ent.radius * sx.min(sy)).clamp(4.0, 26.0);
                        let fallback = match ent.kind {
                            ArcadeEntityKind::Collectible => 0x30D158,
                            ArcadeEntityKind::Hazard => 0xFF453A,
                        };
                        let col = parse_hex_color(&ent.color_hex, fallback);
                        fill_circle(ex, ey, er, rgb(col), window);
                    }

                    // Player orb with optional invulnerability shield ring
                    let px_pos = ox + player_x * sx;
                    let py_pos = oy + player_y * sy;
                    let pr = (player_radius * sx.min(sy)).clamp(6.0, 24.0);
                    if invulnerable_ms > 0 {
                        fill_circle(
                            px_pos,
                            py_pos,
                            pr + 5.0,
                            TdesktopThemePalette::rgba_with_alpha(0x64D2FF, 0.45),
                            window,
                        );
                    }
                    fill_circle(px_pos, py_pos, pr, rgb(accent), window);
                    fill_circle(px_pos, py_pos, pr * 0.45, rgb(0xFFFFFF), window);
                },
            )
            .size_full(),
        )
        .into_any_element()
}

fn render_snake_grid_canvas(
    grid_width: usize,
    grid_height: usize,
    segments: Vec<crate::views::content_hub::game_runtime::GridCoord>,
    food: Option<crate::views::content_hub::game_runtime::GridCoord>,
) -> AnyElement {
    let gw = grid_width.max(4) as f32;
    let gh = grid_height.max(4) as f32;

    div()
        .id("game-snake-grid-canvas-wrap")
        .w(px(460.0))
        .max_w_full()
        .h(px(340.0))
        .rounded(px(14.0))
        .overflow_hidden()
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(0x30D158, 0.55))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    window.paint_quad(gpui::fill(bounds, rgb(0x0E1612)));
                    let ox: f32 = bounds.origin.x.into();
                    let oy: f32 = bounds.origin.y.into();
                    let bw: f32 = bounds.size.width.into();
                    let bh: f32 = bounds.size.height.into();
                    let cell_w = bw / gw;
                    let cell_h = bh / gh;

                    let grid_col = TdesktopThemePalette::rgba_with_alpha(0x30D158, 0.08);
                    for c in 1..grid_width {
                        let gx = ox + c as f32 * cell_w;
                        window.paint_quad(gpui::fill(
                            Bounds::new(point(px(gx), px(oy)), size(px(1.0), px(bh))),
                            grid_col,
                        ));
                    }
                    for r in 1..grid_height {
                        let gy = oy + r as f32 * cell_h;
                        window.paint_quad(gpui::fill(
                            Bounds::new(point(px(ox), px(gy)), size(px(bw), px(1.0))),
                            grid_col,
                        ));
                    }

                    // Glowing food orb
                    if let Some(f) = food {
                        let fx = ox + (f.x as f32 + 0.5) * cell_w;
                        let fy = oy + (f.y as f32 + 0.5) * cell_h;
                        let fr = (cell_w.min(cell_h) * 0.36).clamp(3.0, 14.0);
                        fill_circle(fx, fy, fr, rgb(0xFF375F), window);
                    }

                    // Snake segments (head brighter)
                    for (idx, seg) in segments.iter().enumerate() {
                        let sx = ox + seg.x as f32 * cell_w + 1.5;
                        let sy = oy + seg.y as f32 * cell_h + 1.5;
                        let sw = (cell_w - 3.0).max(2.0);
                        let sh = (cell_h - 3.0).max(2.0);
                        let color = if idx == 0 {
                            rgb(0x34C759)
                        } else {
                            TdesktopThemePalette::rgba_with_alpha(0x30D158, 0.72)
                        };
                        window.paint_quad(gpui::fill(
                            Bounds::new(point(px(sx), px(sy)), size(px(sw), px(sh))),
                            color,
                        ));
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
}

fn dispatch_game_input_and_sync(
    ws: &mut ContentHubWorkspace,
    input: GameInput,
    cx: &mut Context<ContentHubWorkspace>,
) {
    let mut emit_score: Option<(String, SubmitGameScoreRequest)> = None;
    let mut emit_state: Option<(String, UpsertCreationStateRequest)> = None;
    if let Some(ref mut session) = ws.active_game {
        let prev_status = session.snapshot().status;
        let next_snap = session.apply_input(input);
        let ended = matches!(next_snap.status, GameStatus::Won | GameStatus::GameOver);
        if ended && prev_status != next_snap.status {
            emit_state = Some((session.creation_id.clone(), session.to_upsert_state_request()));
            if session.is_published && session.supports_leaderboard && !session.score_submitted {
                if let Some(req) = session.to_submit_score_request() {
                    session.score_submitted = true;
                    emit_score = Some((session.creation_id.clone(), req));
                }
            }
        }
    }
    if let Some((cid, req)) = emit_state {
        cx.emit(ContentHubEvent::UpsertCreationState {
            creation_id: cid,
            req,
        });
    }
    if let Some((cid, req)) = emit_score {
        cx.emit(ContentHubEvent::SubmitGameScore {
            creation_id: cid,
            req,
        });
    }
    cx.notify();
}

/// Renders the embedded Apple Games / Arcade interactive runtime sheet.
pub fn render_game_runtime_sheet(
    ws: &mut ContentHubWorkspace,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let Some(session) = ws.active_game.clone() else {
        return div().into_any_element();
    };
    let pal = ws.palette;
    let lang = ws.language_id.clone();
    let snap = session.snapshot();
    let is_paused = snap.status == GameStatus::Paused;
    let is_finished = matches!(snap.status, GameStatus::Won | GameStatus::GameOver);
    let show_dpad = matches!(
        snap.engine_kind,
        GameEngineKind::ArcadeDodge | GameEngineKind::SnakeGrid | GameEngineKind::GridPuzzle
    );

    let engine_label = match snap.engine_kind {
        GameEngineKind::GridPuzzle => tr_str(&lang, "网格益智", "Grid Puzzle"),
        GameEngineKind::TriviaQuiz => tr_str(&lang, "知识问答", "Trivia Quiz"),
        GameEngineKind::InteractiveFiction => tr_str(&lang, "互动小说", "Story RPG"),
        GameEngineKind::ArcadeDodge => tr_str(&lang, "街机闪避", "Arcade Dodge"),
        GameEngineKind::SnakeGrid => tr_str(&lang, "贪吃蛇", "Snake Arcade"),
        GameEngineKind::CardMatch => tr_str(&lang, "记忆翻牌", "Card Match"),
        GameEngineKind::MiniAppHtml => "Mini App HTML",
    };

    let ob = pal.openbot_palette();
    let warn_soft = gpui::Rgba { a: 0.14, ..ob.warning() };
    let danger_soft = gpui::Rgba { a: 0.14, ..ob.danger() };
    let succ_soft = gpui::Rgba { a: 0.14, ..ob.success() };
    let banner_border_win = gpui::Rgba { a: 0.36, ..ob.success() };
    let banner_border_loss = gpui::Rgba { a: 0.36, ..ob.danger() };

    div()
        .id("apple-game-runtime-sheet")
        .size_full()
        .bg(ob.bg_canvas())
        .flex()
        .flex_col()
        .overflow_hidden()
        // Top OpenBot monochrome Game HUD bar
        .child(
            div()
                .flex_shrink_0()
                .w_full()
                .px(px(20.0))
                .py(px(10.0))
                .bg(ob.bg_surface())
                .border_b_1()
                .border_color(ob.border_hairline())
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(
                            macos_secondary_pill_btn(
                                "game-sheet-close-btn",
                                tr_str(&lang, "← 退出游戏", "← Exit"),
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if let Some(ref s) = this.active_game {
                                        let req = s.to_upsert_state_request();
                                        let cid = s.creation_id.clone();
                                        cx.emit(ContentHubEvent::UpsertCreationState {
                                            creation_id: cid,
                                            req,
                                        });
                                    }
                                    this.active_game = None;
                                    cx.emit(ContentHubEvent::CloseGameSession);
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(14.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(ob.text_primary())
                                        .child(session.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(ob.text_muted())
                                        .child(format!("{} · {}", engine_label, session.developer)),
                                ),
                        ),
                )
                // Live Score / High Score / Lives / Combo / Time HUD chips
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .px(px(10.0))
                                .py(px(4.0))
                                .rounded(px(8.0))
                                .bg(ob.bg_control_active())
                                .border_1()
                                .border_color(ob.border_hairline())
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.text_primary())
                                .child(format!("{} {}", tr_str(&lang, "得分", "SCORE"), snap.score)),
                        )
                        .child(
                            div()
                                .px(px(10.0))
                                .py(px(4.0))
                                .rounded(px(8.0))
                                .bg(warn_soft)
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.warning())
                                .child(format!(
                                    "{} {}",
                                    tr_str(&lang, "最高", "BEST"),
                                    snap.high_score.max(snap.score)
                                )),
                        )
                        .when(snap.max_lives > 0, |el| {
                            el.child(
                                div()
                                    .px(px(9.0))
                                    .py(px(4.0))
                                    .rounded(px(8.0))
                                    .bg(danger_soft)
                                    .text_size(px(12.0))
                                    .text_color(ob.danger())
                                    .child(format!("♥ {}/{}", snap.lives, snap.max_lives)),
                            )
                        })
                        .when(snap.combo > 1, |el| {
                            el.child(
                                div()
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(8.0))
                                    .bg(succ_soft)
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ob.success())
                                    .child(format!("x{}", snap.combo)),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(ob.text_muted())
                                .child(format_duration_ms(snap.elapsed_ms)),
                        ),
                )
                // Pause / Restart / Submit Score buttons
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            macos_secondary_pill_btn(
                                "game-sheet-pause-btn",
                                if is_paused {
                                    tr_str(&lang, "继续", "Resume")
                                } else {
                                    tr_str(&lang, "暂停", "Pause")
                                },
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if let Some(ref mut s) = this.active_game {
                                        if s.snapshot().status == GameStatus::Paused {
                                            s.runtime.resume();
                                        } else {
                                            s.runtime.pause();
                                        }
                                    }
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            macos_secondary_pill_btn(
                                "game-sheet-restart-btn",
                                tr_str(&lang, "重开", "Restart"),
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if let Some(ref mut s) = this.active_game {
                                        s.restart();
                                    }
                                    cx.notify();
                                }),
                            ),
                        )
                        .when(
                            session.is_published
                                && session.supports_leaderboard
                                && snap.score.max(snap.high_score) > 0,
                            |el| {
                                el.child(
                                    macos_primary_pill_btn(
                                        "game-sheet-submit-score-btn",
                                        tr_str(&lang, "提交排行", "Submit Score"),
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            if let Some(ref mut s) = this.active_game {
                                                if let Some(req) = s.to_submit_score_request() {
                                                    s.score_submitted = true;
                                                    cx.emit(ContentHubEvent::SubmitGameScore {
                                                        creation_id: s.creation_id.clone(),
                                                        req,
                                                    });
                                                }
                                            }
                                            cx.notify();
                                        }),
                                    ),
                                )
                            },
                        ),
                ),
        )
        // Main Game Stage
        .child(
            div()
                .id("game-runtime-stage-scroll")
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_y_scroll()
                .p(px(24.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(16.0))
                // Win / GameOver banner
                .when(is_finished, |el| {
                    let won = snap.status == GameStatus::Won;
                    let banner_fg = if won { ob.success() } else { ob.danger() };
                    let banner_bg = if won { succ_soft } else { danger_soft };
                    let banner_border = if won { banner_border_win } else { banner_border_loss };
                    el.child(
                        div()
                            .w_full()
                            .max_w(px(560.0))
                            .p(px(16.0))
                            .rounded(px(14.0))
                            .bg(banner_bg)
                            .border_1()
                            .border_color(banner_border)
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .text_size(px(16.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(banner_fg)
                                            .child(if won {
                                                tr_str(&lang, "🎉 恭喜通关！", "🎉 Victory!")
                                            } else {
                                                tr_str(&lang, "游戏结束", "Game Over")
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(ob.text_primary())
                                            .child(format!(
                                                "{}: {} · {}: {}",
                                                tr_str(&lang, "本局得分", "Final Score"),
                                                snap.score,
                                                tr_str(&lang, "已解锁成就", "Achievements"),
                                                snap.unlocked_achievements.len()
                                            )),
                                    ),
                            )
                            .child(
                                macos_primary_pill_btn(
                                    "game-sheet-play-again-btn",
                                    tr_str(&lang, "再来一局", "Play Again"),
                                    pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(ref mut s) = this.active_game {
                                            s.restart();
                                        }
                                        cx.notify();
                                    }),
                                ),
                            ),
                    )
                })
                // Engine-specific interactive board / canvas
                .child(match snap.state.clone() {
                    GameRuntimeState::ArcadeDodge {
                        arena_width,
                        arena_height,
                        player_x,
                        player_y,
                        player_radius,
                        invulnerable_ms,
                        theme_accent_hex,
                        entities,
                        ..
                    } => render_arcade_dodge_canvas(
                        arena_width,
                        arena_height,
                        player_x,
                        player_y,
                        player_radius,
                        invulnerable_ms,
                        theme_accent_hex,
                        entities,
                    ),
                    GameRuntimeState::SnakeGrid {
                        grid_width,
                        grid_height,
                        segments,
                        food,
                        ..
                    } => render_snake_grid_canvas(grid_width, grid_height, segments, food),
                    GameRuntimeState::GridPuzzle {
                        level_index,
                        total_levels,
                        level_title,
                        grid,
                        moves_used,
                        max_moves,
                        hint,
                        ..
                    } => apple_grouped_card(pal)
                        .w_full()
                        .max_w(px(520.0))
                        .p(px(18.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(12.0))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(format!(
                                            "{} {}/{} · {}",
                                            tr_str(&lang, "关卡", "Level"),
                                            level_index + 1,
                                            total_levels,
                                            level_title
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(match max_moves {
                                            Some(m) => format!(
                                                "{}: {moves_used}/{m}",
                                                tr_str(&lang, "步数", "Moves")
                                            ),
                                            None => format!(
                                                "{}: {moves_used}",
                                                tr_str(&lang, "步数", "Moves")
                                            ),
                                        }),
                                ),
                        )
                        .child(div().max_w_full().overflow_hidden().flex().flex_col().gap(px(6.0)).children(
                            grid.iter().enumerate().map(|(r, row)| {
                                div().flex().gap(px(6.0)).children(row.iter().enumerate().map(
                                    |(c, &val)| {
                                        let is_empty = val == 0;
                                        div()
                                            .id(SharedString::from(format!("gp-cell-{r}-{c}")))
                                            .cursor_pointer()
                                            .w(px(58.0))
                                            .h(px(58.0))
                                            .rounded(px(10.0))
                                            .bg(if is_empty {
                                                TdesktopThemePalette::rgba_with_alpha(
                                                    pal.text_primary,
                                                    0.04,
                                                )
                                            } else {
                                                TdesktopThemePalette::rgba_with_alpha(
                                                    GAMES_ACCENT,
                                                    0.22,
                                                )
                                            })
                                            .border_1()
                                            .border_color(if is_empty {
                                                pal.separator_rgba()
                                            } else {
                                                TdesktopThemePalette::rgba_with_alpha(
                                                    GAMES_ACCENT,
                                                    0.55,
                                                )
                                            })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(18.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    dispatch_game_input_and_sync(
                                                        this,
                                                        GameInput::ClickCell { row: r, col: c },
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .child(if is_empty {
                                                String::new()
                                            } else {
                                                val.to_string()
                                            })
                                    },
                                ))
                            }),
                        ))
                        .when_some(hint, |el, h| {
                            el.child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(format!("💡 {h}")),
                            )
                        })
                        .into_any_element(),
                    GameRuntimeState::TriviaQuiz {
                        current_question_index,
                        total_questions,
                        prompt,
                        choices,
                        selected_choice,
                        correct_index,
                        last_answer_correct,
                        explanation,
                        awaiting_next,
                        remaining_ms,
                        ..
                    } => apple_grouped_card(pal)
                        .w_full()
                        .max_w(px(620.0))
                        .p(px(20.0))
                        .flex()
                        .flex_col()
                        .gap(px(14.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(GAMES_ACCENT))
                                        .child(format!(
                                            "Q {} / {}",
                                            current_question_index + 1,
                                            total_questions.max(1)
                                        )),
                                )
                                .when_some(remaining_ms, |el, ms| {
                                    el.child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(rgb(0xFF9F0A))
                                            .child(format!("⏱ {}", format_duration_ms(ms))),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(17.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(prompt),
                        )
                        .child(div().flex().flex_col().gap(px(8.0)).children(
                            choices.iter().enumerate().map(|(idx, choice)| {
                                let is_sel = selected_choice == Some(idx);
                                let is_correct = correct_index == Some(idx);
                                let bg = if awaiting_next && is_correct {
                                    TdesktopThemePalette::rgba_with_alpha(0x30D158, 0.22)
                                } else if awaiting_next && is_sel && !is_correct {
                                    TdesktopThemePalette::rgba_with_alpha(0xFF453A, 0.22)
                                } else if is_sel {
                                    TdesktopThemePalette::rgba_with_alpha(GAMES_ACCENT, 0.22)
                                } else {
                                    TdesktopThemePalette::rgba_with_alpha(pal.text_primary, 0.05)
                                };
                                let letter = (b'A' + (idx as u8).min(25)) as char;
                                div()
                                    .id(SharedString::from(format!("trivia-choice-{idx}")))
                                    .cursor_pointer()
                                    .px(px(14.0))
                                    .py(px(11.0))
                                    .rounded(px(10.0))
                                    .bg(bg)
                                    .border_1()
                                    .border_color(pal.separator_rgba())
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(pal.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            dispatch_game_input_and_sync(
                                                this,
                                                GameInput::SelectOption(idx),
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(format!("{letter}. {choice}"))
                            }),
                        ))
                        .when(awaiting_next, |el| {
                            el.child(
                                div()
                                    .p(px(12.0))
                                    .rounded(px(10.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(
                                        if last_answer_correct == Some(true) {
                                            0x30D158
                                        } else {
                                            0xFF9F0A
                                        },
                                        0.14,
                                    ))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(12.0))
                                            .text_color(rgb(pal.text_primary))
                                            .child(explanation.unwrap_or_else(|| {
                                                if last_answer_correct == Some(true) {
                                                    tr_str(&lang, "回答正确！", "Correct!")
                                                        .to_string()
                                                } else {
                                                    tr_str(&lang, "继续加油！", "Keep going!")
                                                        .to_string()
                                                }
                                            })),
                                    )
                                    .child(
                                        macos_primary_pill_btn(
                                            "trivia-next-btn",
                                            tr_str(&lang, "下一题 →", "Next →"),
                                            pal,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                dispatch_game_input_and_sync(
                                                    this,
                                                    GameInput::Confirm,
                                                    cx,
                                                );
                                            }),
                                        ),
                                    ),
                            )
                        })
                        .into_any_element(),
                    GameRuntimeState::InteractiveFiction {
                        title,
                        body,
                        choices,
                        visited_node_ids,
                        is_ending,
                        ending_badge,
                        ..
                    } => apple_grouped_card(pal)
                        .w_full()
                        .max_w(px(640.0))
                        .p(px(22.0))
                        .flex()
                        .flex_col()
                        .gap(px(14.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(18.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!(
                                            "{} {}",
                                            visited_node_ids.len(),
                                            tr_str(&lang, "节点", "scenes")
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(14.0))
                                .line_height(px(22.0))
                                .text_color(rgb(pal.text_primary))
                                .child(body),
                        )
                        .when_some(ending_badge.filter(|_| is_ending), |el, badge| {
                            el.child(
                                div()
                                    .px(px(12.0))
                                    .py(px(6.0))
                                    .rounded(px(999.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(0xFF9F0A, 0.18))
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xFF9F0A))
                                    .child(format!("🏆 {badge}")),
                            )
                        })
                        .child(div().flex().flex_col().gap(px(8.0)).children(
                            choices.iter().map(|ch| {
                                let idx = ch.index;
                                let delta_lbl = if ch.score_delta > 0 {
                                    format!(" (+{})", ch.score_delta)
                                } else {
                                    String::new()
                                };
                                div()
                                    .id(SharedString::from(format!("story-choice-{idx}")))
                                    .cursor_pointer()
                                    .px(px(14.0))
                                    .py(px(10.0))
                                    .rounded(px(10.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(GAMES_ACCENT, 0.14))
                                    .border_1()
                                    .border_color(TdesktopThemePalette::rgba_with_alpha(
                                        GAMES_ACCENT,
                                        0.38,
                                    ))
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            dispatch_game_input_and_sync(
                                                this,
                                                GameInput::SelectOption(idx),
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(format!("▸ {}{}", ch.label, delta_lbl))
                            }),
                        ))
                        .into_any_element(),
                    GameRuntimeState::CardMatch {
                        cols,
                        cards,
                        moves,
                        matched_pairs,
                        total_pairs,
                        ..
                    } => {
                        let grid_w = ((cols.max(2) as f32) * 86.0).min(560.0);
                        apple_grouped_card(pal)
                            .w_full()
                            .max_w(px(620.0))
                            .p(px(18.0))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(format!(
                                        "{}: {matched_pairs}/{total_pairs} · {}: {moves}",
                                        tr_str(&lang, "已配对", "Matched"),
                                        tr_str(&lang, "步数", "Moves")
                                    )),
                            )
                            .child(div().w(px(grid_w)).max_w_full().flex().flex_wrap().justify_center().gap(px(8.0)).children(
                                cards.iter().map(|card| {
                                    let r = card.row;
                                    let c = card.col;
                                    let revealed = card.face != CardFaceState::FaceDown;
                                    let matched = card.face == CardFaceState::Matched;
                                    let bg = if matched {
                                        TdesktopThemePalette::rgba_with_alpha(0x30D158, 0.22)
                                    } else if revealed {
                                        let tint = card
                                            .accent_hex
                                            .as_deref()
                                            .map(|h| parse_hex_color(h, GAMES_ACCENT))
                                            .unwrap_or(GAMES_ACCENT);
                                        TdesktopThemePalette::rgba_with_alpha(tint, 0.25)
                                    } else {
                                        TdesktopThemePalette::rgba_with_alpha(GAMES_ACCENT, 0.14)
                                    };
                                    div()
                                        .id(SharedString::from(format!(
                                            "card-match-{}-{}",
                                            card.index, card.pair_id
                                        )))
                                        .cursor_pointer()
                                        .w(px(76.0))
                                        .h(px(86.0))
                                        .rounded(px(10.0))
                                        .bg(bg)
                                        .border_1()
                                        .border_color(if matched {
                                            TdesktopThemePalette::rgba_with_alpha(0x30D158, 0.65)
                                        } else {
                                            pal.separator_rgba()
                                        })
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .justify_center()
                                        .gap(px(4.0))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                dispatch_game_input_and_sync(
                                                    this,
                                                    GameInput::ClickCell { row: r, col: c },
                                                    cx,
                                                );
                                            }),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(22.0))
                                                .text_color(rgb(pal.text_primary))
                                                .child(if revealed {
                                                    if card.symbol.trim().is_empty() {
                                                        "★".to_string()
                                                    } else {
                                                        card.symbol.clone()
                                                    }
                                                } else {
                                                    "✦".to_string()
                                                }),
                                        )
                                        .when(revealed, |el| {
                                            el.child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(pal.text_secondary))
                                                    .truncate()
                                                    .child(card.label.clone()),
                                            )
                                        })
                                }),
                            ))
                            .into_any_element()
                    }
                    GameRuntimeState::MiniAppLaunch {
                        bot_username,
                        short_name,
                        ..
                    } => {
                        let b = bot_username.clone();
                        let s = short_name.clone();
                        apple_grouped_card(pal)
                            .max_w(px(480.0))
                            .p(px(24.0))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .text_size(px(15.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(format!("Mini App Game · @{bot_username}/{short_name}")),
                            )
                            .child(
                                macos_primary_pill_btn(
                                    "game-launch-miniapp-webview-btn",
                                    tr_str(&lang, "在 WebView 中启动游戏", "Launch in WebView"),
                                    pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |_this, _, _, cx| {
                                        cx.emit(ContentHubEvent::LaunchMiniApp {
                                            bot_username: b.clone(),
                                            short_name: s.clone(),
                                            web_url: None,
                                        });
                                    }),
                                ),
                            )
                            .into_any_element()
                    }
                })
                // On-screen D-Pad + keyboard hint for arcade/snake/grid games
                .when(show_dpad, |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                macos_secondary_pill_btn("game-dpad-up", "↑", pal).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        dispatch_game_input_and_sync(this, GameInput::MoveUp, cx);
                                    }),
                                ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        macos_secondary_pill_btn("game-dpad-left", "←", pal)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    dispatch_game_input_and_sync(
                                                        this,
                                                        GameInput::MoveLeft,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                    )
                                    .child(
                                        macos_secondary_pill_btn("game-dpad-down", "↓", pal)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    dispatch_game_input_and_sync(
                                                        this,
                                                        GameInput::MoveDown,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                    )
                                    .child(
                                        macos_secondary_pill_btn("game-dpad-right", "→", pal)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    dispatch_game_input_and_sync(
                                                        this,
                                                        GameInput::MoveRight,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(if session.controls_hint.trim().is_empty() {
                                        tr_str(
                                            &lang,
                                            "支持键盘方向键 / WASD 操作，Esc 退出",
                                            "Use Arrow keys / WASD to play, Esc to exit",
                                        )
                                        .to_string()
                                    } else {
                                        session.controls_hint.clone()
                                    }),
                            ),
                    )
                }),
        )
        .into_any_element()
}
