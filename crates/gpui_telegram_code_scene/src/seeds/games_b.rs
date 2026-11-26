//! Playable Games G4–G6 (Turbo Neon Snake, Gravity Orbit Catcher, Neon Paddle Pong).

use gpui_telegram_proto::{AssetKind, CodeSceneAssetDto};

use crate::seed_library::{
    OFFICIAL_FABUSHI_BOT_ID, OFFICIAL_PHYSICS_LAB_BOT_ID, SeedSpec, build_seed_asset,
};

const BASE_TS: i64 = 1_760_000_000_000;

pub fn assets() -> Vec<CodeSceneAssetDto> {
    vec![
        build_seed_asset(SeedSpec {
            scene_id: "seed_game_neon_snake",
            title: "极速霓虹贪吃蛇 · Turbo Neon Snake",
            description: "18x28 赛博网格贪吃蛇，方向键转向，按住 Space 开启氮气加速。",
            kind: AssetKind::Game,
            bot_id: OFFICIAL_FABUSHI_BOT_ID,
            tags: &["game", "snake", "retro", "neon", "puzzle"],
            duration_sec: None,
            seed: 3004,
            controls_hint: "方向键转向 · Space 加速 · R 重开",
            plays_count: 680,
            likes_count: 142,
            bookmarks_count: 58,
            comments_count: 20,
            shares_count: 24,
            remixes_count: 11,
            created_at_ms: BASE_TS - 86_400_000 * 4,
            source_code: GAME_NEON_SNAKE,
        }),
        build_seed_asset(SeedSpec {
            scene_id: "seed_game_orbit_catcher",
            title: "引力轨道捕星者 · Gravity Orbit Catcher",
            description: "操控双轨道引力捕手接住坠落星辰、躲避陨石冲击，点击或按 Space 切换内外轨道。",
            kind: AssetKind::Game,
            bot_id: OFFICIAL_PHYSICS_LAB_BOT_ID,
            tags: &["game", "physics", "orbit", "space", "reflex"],
            duration_sec: None,
            seed: 3005,
            controls_hint: "←/→ 旋转 · Space/点击 切内外轨 · R 重开",
            // Tier0 Cold-Start (< 20 plays)
            plays_count: 14,
            likes_count: 5,
            bookmarks_count: 3,
            comments_count: 1,
            shares_count: 1,
            remixes_count: 0,
            created_at_ms: BASE_TS - 3_600_000 * 4,
            source_code: GAME_ORBIT_CATCHER,
        }),
        build_seed_asset(SeedSpec {
            scene_id: "seed_game_neon_pong",
            title: "霓虹极速弹球对决 · Neon Paddle Pong",
            description: "人机高速霓虹乒乓对决，接球即得分，击穿 AI 防线获得额外暴击加分。",
            kind: AssetKind::Game,
            bot_id: OFFICIAL_FABUSHI_BOT_ID,
            tags: &["game", "pong", "arcade", "physics", "competitive"],
            duration_sec: None,
            seed: 3006,
            controls_hint: "←/→ 或移动指针接球 · Space 强力扣杀 · R 重开",
            plays_count: 165,
            likes_count: 39,
            bookmarks_count: 16,
            comments_count: 7,
            shares_count: 10,
            remixes_count: 4,
            created_at_ms: BASE_TS - 86_400_000 * 2,
            source_code: GAME_NEON_PONG,
        }),
    ]
}

const GAME_NEON_SNAKE: &str = r#"
fn on_init(state) {
    state.body = [
        { x: 9, y: 14 },
        { x: 8, y: 14 },
        { x: 7, y: 14 },
        { x: 6, y: 14 }
    ];
    state.dx = 1;
    state.dy = 0;
    state.ndx = 1;
    state.ndy = 0;
    state.food_x = 12;
    state.food_y = 14;
    state.accum = 0;
    state.score = 0;
    state.high_score = 0;
    state.game_over = false;
    state.status_text = "ARROWS STEER · SPACE TURBO";
}

fn on_update(state, dt, input) {
    if state.game_over {
        return;
    }
    if input.up && state.dy != 1 { state.ndx = 0; state.ndy = -1; }
    else if input.down && state.dy != -1 { state.ndx = 0; state.ndy = 1; }
    else if input.left && state.dx != 1 { state.ndx = -1; state.ndy = 0; }
    else if input.right && state.dx != -1 { state.ndx = 1; state.ndy = 0; }

    let step_t = 0.13;
    if input.action { step_t = 0.075; }
    state.accum += dt;
    if state.accum < step_t {
        return;
    }
    state.accum -= step_t;
    state.dx = state.ndx;
    state.dy = state.ndy;

    let head = state.body[0];
    let nx = head.x + state.dx;
    let ny = head.y + state.dy;
    if nx < 0 || nx >= 18 || ny < 0 || ny >= 28 {
        state.game_over = true;
        state.status_text = "WALL HIT · PRESS R";
        tone(140, 0.22, "saw", 0.55);
        return;
    }
    for i in 0..len(state.body) {
        if state.body[i].x == nx && state.body[i].y == ny {
            state.game_over = true;
            state.status_text = "SELF HIT · PRESS R";
            tone(130, 0.22, "saw", 0.55);
            return;
        }
    }

    let ate = (nx == state.food_x && ny == state.food_y);
    let next_body = [{ x: nx, y: ny }];
    let keep = len(state.body) - 1;
    if ate { keep = len(state.body); }
    for i in 0..keep {
        push(next_body, state.body[i]);
    }
    state.body = next_body;

    if ate {
        state.score += 10;
        if state.score > state.high_score { state.high_score = state.score; }
        state.status_text = "LEN " + str(len(state.body)) + " · SCORE " + str(state.score);
        state.food_x = floor(rand_range(1, 17));
        state.food_y = floor(rand_range(1, 27));
        tone(740, 0.07, "sine", 0.5);
    }
}

fn on_render(state, t, canvas) {
    clear_grad(rgb(6, 12, 24), rgb(10, 22, 36), 180);
    stroke_rect(18, 72, 324, 504, rgba(70, 240, 210, 0.45), 2.0);
    let fx = 18 + state.food_x * 18 + 9;
    let fy = 72 + state.food_y * 18 + 9;
    glow_circle(fx, fy, 11 + sin(t * 7) * 2, rgba(255, 85, 165, 0.85), 1.4);
    circle(fx, fy, 6.5, rgb(255, 180, 220));

    for i in 0..len(state.body) {
        let seg = state.body[i];
        let sx = 18 + seg.x * 18 + 1;
        let sy = 72 + seg.y * 18 + 1;
        let col = hsla(160 + i * 6, 0.95, 0.58, 0.96);
        if i == 0 { col = rgb(185, 255, 240); }
        round_rect(sx, sy, 16, 16, 4, col);
    }
    text(22, 46, "SCORE " + str(state.score), 16, rgb(240, 255, 250), "left");
    text(338, 46, "BEST " + str(state.high_score), 16, rgb(120, 255, 220), "right");
    if state.game_over {
        round_rect(48, 270, 264, 84, 16, rgba(8, 12, 24, 0.88));
        text(180, 306, "GAME OVER", 22, rgb(255, 95, 155), "center");
        text(180, 336, "SCORE " + str(state.score) + " · BEST " + str(state.high_score), 14, rgb(225, 245, 255), "center");
    }
}
"#;

const GAME_ORBIT_CATCHER: &str = r#"
fn on_init(state) {
    state.angle = -PI * 0.5;
    state.orbit_r = 108;
    state.ring_cd = 0;
    state.shields = 2;
    state.spawn_t = 0;
    state.objs = [
        { ang: -PI * 0.5, rad: 174, spd: 112, kind: "star" },
        { ang: -PI * 0.5, rad: 238, spd: 120, kind: "meteor" },
        { ang: -PI * 0.5, rad: 306, spd: 124, kind: "meteor" }
    ];
    state.score = 0;
    state.high_score = 0;
    state.game_over = false;
    state.status_text = "SHIELDS 2 · CATCH STARS";
}

fn on_update(state, dt, input) {
    if state.game_over {
        return;
    }
    if input.left { state.angle -= 3.2 * dt; }
    if input.right { state.angle += 3.2 * dt; }
    if input.pointer_active && input.mouse_down {
        state.angle = atan2(input.mouse_y - 330, input.mouse_x - 180);
    }
    state.ring_cd = max(0, state.ring_cd - dt);
    if (input.action || input.up || input.down || input.clicked) && state.ring_cd <= 0 {
        if state.orbit_r > 90 { state.orbit_r = 76; } else { state.orbit_r = 108; }
        state.ring_cd = 0.2;
        tone(460, 0.06, "triangle", 0.4);
    }

    state.spawn_t += dt;
    if state.spawn_t >= 0.72 {
        state.spawn_t = 0;
        let kind = "star";
        if rand() > 0.55 { kind = "meteor"; }
        push(state.objs, {
            ang: rand_range(0, TAU),
            rad: 260,
            spd: rand_range(105, 145),
            kind: kind
        });
    }

    let cx = 180 + cos(state.angle) * state.orbit_r;
    let cy = 330 + sin(state.angle) * state.orbit_r;
    let next = [];
    for i in 0..len(state.objs) {
        let o = state.objs[i];
        o.rad -= o.spd * dt;
        let ox = 180 + cos(o.ang) * o.rad;
        let oy = 330 + sin(o.ang) * o.rad;
        if circle_hit(cx, cy, 16, ox, oy, 11) {
            if o.kind == "star" {
                state.score += 15;
                if state.score > state.high_score { state.high_score = state.score; }
                state.status_text = "SHIELDS " + str(state.shields) + " · SCORE " + str(state.score);
                tone(820, 0.08, "sine", 0.5);
            } else {
                state.shields -= 1;
                tone(150, 0.2, "saw", 0.55);
                if state.shields <= 0 {
                    state.game_over = true;
                    state.status_text = "SHIELDS DEPLETED · PRESS R";
                } else {
                    state.status_text = "SHIELDS " + str(state.shields) + " · SCORE " + str(state.score);
                }
            }
        } else if o.rad > 32 {
            push(next, o);
        }
    }
    state.objs = next;
}

fn on_render(state, t, canvas) {
    clear_grad(rgb(6, 8, 24), rgb(18, 14, 46), 180);
    stroke_circle(180, 330, 76, rgba(120, 210, 255, 0.28), 1.4);
    stroke_circle(180, 330, 108, rgba(120, 210, 255, 0.35), 1.6);
    glow_circle(180, 330, 32, rgba(110, 165, 255, 0.75), 1.5);
    circle(180, 330, 26, rgb(70, 125, 245));

    let cx = 180 + cos(state.angle) * state.orbit_r;
    let cy = 330 + sin(state.angle) * state.orbit_r;
    glow_circle(cx, cy, 18, rgba(95, 255, 225, 0.85), 1.4);
    circle(cx, cy, 12, rgb(180, 255, 245));

    for i in 0..len(state.objs) {
        let o = state.objs[i];
        let ox = 180 + cos(o.ang) * o.rad;
        let oy = 330 + sin(o.ang) * o.rad;
        if o.kind == "star" {
            star(ox, oy, 11, 5, 5, rgb(255, 225, 95), t * 3);
        } else {
            glow_circle(ox, oy, 11, rgba(255, 70, 110, 0.8), 1.2);
            circle(ox, oy, 8, rgb(255, 95, 120));
        }
    }
    text(22, 46, "SCORE " + str(state.score), 16, rgb(240, 250, 255), "left");
    text(338, 46, "SHIELD " + str(state.shields), 16, rgb(120, 245, 225), "right");
    if state.game_over {
        round_rect(48, 270, 264, 84, 16, rgba(8, 10, 24, 0.88));
        text(180, 306, "ORBIT LOST", 22, rgb(255, 90, 145), "center");
        text(180, 336, "SCORE " + str(state.score) + " · BEST " + str(state.high_score), 14, rgb(225, 240, 255), "center");
    }
}
"#;

const GAME_NEON_PONG: &str = r#"
fn on_init(state) {
    state.px = 180;
    state.ai_x = 180;
    state.bx = 180;
    state.by = 320;
    state.vx = 30;
    state.vy = 250;
    state.misses = 0;
    state.score = 0;
    state.high_score = 0;
    state.game_over = false;
    state.status_text = "RALLY VS AI · SPACE SMASH";
}

fn on_update(state, dt, input) {
    if state.game_over {
        return;
    }
    if input.left { state.px -= 320 * dt; }
    if input.right { state.px += 320 * dt; }
    if input.pointer_active {
        state.px = lerp(state.px, input.mouse_x, clamp(dt * 14, 0, 1));
    }
    state.px = clamp(state.px, 50, 310);

    // Velocity-capped AI paddle so player can win points
    let ai_diff = state.bx - state.ai_x;
    state.ai_x = clamp(state.ai_x + clamp(ai_diff * 4.0, -185, 185) * dt, 46, 314);

    state.bx += state.vx * dt;
    state.by += state.vy * dt;
    if state.bx < 12 { state.bx = 12; state.vx = abs(state.vx); }
    if state.bx > 348 { state.bx = 348; state.vx = -abs(state.vx); }

    // Player paddle collision (bottom)
    if state.vy > 0 && circle_rect_hit(state.bx, state.by, 8, state.px - 46, 560, 92, 14) {
        let boost = 1.04;
        if input.action { boost = 1.18; }
        state.vy = -min(abs(state.vy) * boost, 430);
        state.vx = clamp(state.vx + (state.bx - state.px) * 3.2, -260, 260);
        state.score += 10;
        if state.score > state.high_score { state.high_score = state.score; }
        state.status_text = "SCORE " + str(state.score) + " · MISS " + str(state.misses) + "/3";
        tone(660, 0.06, "square", 0.42);
    }

    // AI paddle collision (top)
    if state.vy < 0 && circle_rect_hit(state.bx, state.by, 8, state.ai_x - 42, 76, 84, 14) {
        state.vy = abs(state.vy);
        tone(440, 0.05, "square", 0.35);
    }

    if state.by < 44 {
        state.score += 50;
        if state.score > state.high_score { state.high_score = state.score; }
        state.bx = 180;
        state.by = 300;
        state.vx = 45;
        state.vy = 260;
        tone(880, 0.12, "sine", 0.55);
    } else if state.by > 616 {
        state.misses += 1;
        tone(145, 0.2, "saw", 0.55);
        if state.misses >= 3 {
            state.game_over = true;
            state.status_text = "MATCH OVER · PRESS R";
        } else {
            state.bx = 180;
            state.by = 300;
            state.vx = 35;
            state.vy = 265;
            state.status_text = "SCORE " + str(state.score) + " · MISS " + str(state.misses) + "/3";
        }
    }
}

fn on_render(state, t, canvas) {
    clear_grad(rgb(8, 10, 26), rgb(16, 12, 38), 180);
    line(16, 320, 344, 320, rgba(255, 255, 255, 0.16), 2.0);
    stroke_circle(180, 320, 44, rgba(255, 255, 255, 0.12), 1.5);
    round_rect(state.ai_x - 42, 76, 84, 14, 7, rgb(255, 85, 165));
    round_rect(state.px - 46, 560, 92, 14, 7, rgb(85, 245, 255));
    glow_circle(state.bx, state.by, 13, rgba(255, 235, 110, 0.85), 1.4);
    circle(state.bx, state.by, 7.5, rgb(255, 255, 245));
    text(22, 42, "SCORE " + str(state.score), 15, rgb(240, 250, 255), "left");
    text(338, 42, "MISS " + str(state.misses) + "/3", 15, rgb(255, 125, 175), "right");
    if state.game_over {
        round_rect(48, 270, 264, 84, 16, rgba(8, 10, 26, 0.88));
        text(180, 306, "MATCH OVER", 22, rgb(255, 95, 165), "center");
        text(180, 336, "SCORE " + str(state.score) + " · BEST " + str(state.high_score), 14, rgb(225, 240, 255), "center");
    }
}
"#;
