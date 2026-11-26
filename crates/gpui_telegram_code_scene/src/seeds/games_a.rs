//! Playable Games G1–G3 (Flappy Cyber-Bird, Starfield Space Shooter, Cyberpunk Brick Breaker).

use gpui_telegram_proto::{AssetKind, CodeSceneAssetDto};

use crate::seed_library::{
    OFFICIAL_ARCADE_FORGE_BOT_ID, OFFICIAL_NEON_SHADER_BOT_ID, SeedSpec, build_seed_asset,
};

const BASE_TS: i64 = 1_760_000_000_000;

pub fn assets() -> Vec<CodeSceneAssetDto> {
    vec![
        build_seed_asset(SeedSpec {
            scene_id: "seed_game_flappy_bird",
            title: "霓虹赛博飞鸟 · Neon Flappy Cyber-Bird",
            description: "点击或按 Space/W 振翅穿越霓虹能量管道，实时计分与最高分记录。",
            kind: AssetKind::Game,
            bot_id: OFFICIAL_ARCADE_FORGE_BOT_ID,
            tags: &["game", "arcade", "flappy", "neon", "reflex"],
            duration_sec: None,
            seed: 3001,
            controls_hint: "Space / ↑ / 点击 振翅 · R 重开",
            plays_count: 1850,
            likes_count: 348,
            bookmarks_count: 142,
            comments_count: 54,
            shares_count: 72,
            remixes_count: 29,
            created_at_ms: BASE_TS - 86_400_000 * 8,
            source_code: GAME_FLAPPY_BIRD,
        }),
        build_seed_asset(SeedSpec {
            scene_id: "seed_game_space_shooter",
            title: "星际雷电射击 · Starfield Space Shooter",
            description: "操控霓虹星际战机左右走位并连续发射激光炮，击毁敌机赚取高分。",
            kind: AssetKind::Game,
            bot_id: OFFICIAL_ARCADE_FORGE_BOT_ID,
            tags: &["game", "shooter", "space", "arcade", "scifi"],
            duration_sec: None,
            seed: 3002,
            controls_hint: "←/→ 或拖拽移动 · Space/按住射击 · R 重开",
            plays_count: 960,
            likes_count: 188,
            bookmarks_count: 74,
            comments_count: 26,
            shares_count: 35,
            remixes_count: 15,
            created_at_ms: BASE_TS - 86_400_000 * 5,
            source_code: GAME_SPACE_SHOOTER,
        }),
        build_seed_asset(SeedSpec {
            scene_id: "seed_game_brick_breaker",
            title: "赛博霓虹打砖块 · Cyberpunk Brick Breaker",
            description: "经典 5x5 霓虹矩阵打砖块，滑动挡板反弹能量球击碎砖块矩阵。",
            kind: AssetKind::Game,
            bot_id: OFFICIAL_NEON_SHADER_BOT_ID,
            tags: &["game", "breakout", "neon", "arcade", "classic"],
            duration_sec: None,
            seed: 3003,
            controls_hint: "←/→ 或移动指针控制挡板 · R 重开",
            plays_count: 420,
            likes_count: 92,
            bookmarks_count: 38,
            comments_count: 15,
            shares_count: 20,
            remixes_count: 9,
            created_at_ms: BASE_TS - 86_400_000 * 3,
            source_code: GAME_BRICK_BREAKER,
        }),
    ]
}

const GAME_FLAPPY_BIRD: &str = r#"
fn on_init(state) {
    state.bird_x = 96;
    state.bird_y = 260;
    state.bird_vy = 0;
    state.flap_cd = 0;
    state.pipes = [
        { x: 268, gap_y: 265, w: 46, passed: false },
        { x: 432, gap_y: 295, w: 46, passed: false },
        { x: 596, gap_y: 245, w: 46, passed: false }
    ];
    state.score = 0;
    state.high_score = 0;
    state.game_over = false;
    state.status_text = "SPACE / CLICK TO FLAP";
}

fn on_update(state, dt, input) {
    if state.game_over {
        return;
    }
    state.flap_cd = max(0, state.flap_cd - dt);
    if (input.up || input.action || input.clicked) && state.flap_cd <= 0 {
        state.bird_vy = -265;
        state.flap_cd = 0.14;
        tone(540, 0.06, "triangle", 0.45);
    }
    state.bird_vy += 680 * dt;
    state.bird_y += state.bird_vy * dt;

    let gap_h = 156;
    for i in 0..len(state.pipes) {
        let p = state.pipes[i];
        p.x -= 135 * dt;
        if p.x + p.w < -12 {
            p.x += 492;
            p.gap_y = rand_range(175, 435);
            p.passed = false;
        }
        if !p.passed && p.x + p.w < state.bird_x {
            p.passed = true;
            state.score += 1;
            if state.score > state.high_score {
                state.high_score = state.score;
            }
            state.status_text = "SCORE " + str(state.score);
            tone(780, 0.08, "sine", 0.5);
        }
        let top_h = p.gap_y - gap_h * 0.5;
        let bot_y = p.gap_y + gap_h * 0.5;
        if circle_rect_hit(state.bird_x, state.bird_y, 12, p.x, 0, p.w, top_h)
            || circle_rect_hit(state.bird_x, state.bird_y, 12, p.x, bot_y, p.w, 640 - bot_y)
        {
            state.game_over = true;
            state.status_text = "CRASHED · PRESS R";
            tone(140, 0.24, "saw", 0.6);
        }
    }
    if state.bird_y < 14 || state.bird_y > 602 {
        state.game_over = true;
        state.status_text = "OUT OF BOUNDS · PRESS R";
        tone(125, 0.24, "saw", 0.6);
    }
}

fn on_render(state, t, canvas) {
    clear_grad(rgb(8, 12, 32), rgb(24, 14, 52), 180);
    let gap_h = 156;
    for i in 0..len(state.pipes) {
        let p = state.pipes[i];
        let top_h = p.gap_y - gap_h * 0.5;
        let bot_y = p.gap_y + gap_h * 0.5;
        round_rect(p.x, 0, p.w, top_h, 6, rgb(34, 215, 190));
        stroke_rect(p.x, 0, p.w, top_h, rgb(140, 255, 240), 2.0);
        round_rect(p.x, bot_y, p.w, 608 - bot_y, 6, rgb(34, 215, 190));
        stroke_rect(p.x, bot_y, p.w, 608 - bot_y, rgb(140, 255, 240), 2.0);
    }
    rect(0, 608, 360, 32, rgb(255, 70, 165));
    glow_circle(state.bird_x, state.bird_y, 20, rgba(255, 215, 75, 0.8), 1.4);
    circle(state.bird_x, state.bird_y, 13, rgb(255, 232, 95));
    circle(state.bird_x + 5, state.bird_y - 3, 3, rgb(18, 22, 42));
    text(180, 74, str(state.score), 34, rgb(255, 255, 255), "center");
    if state.game_over {
        round_rect(48, 268, 264, 88, 16, rgba(10, 12, 26, 0.85));
        text(180, 304, "GAME OVER", 24, rgb(255, 95, 155), "center");
        text(180, 336, "SCORE " + str(state.score) + " · BEST " + str(state.high_score), 14, rgb(225, 235, 255), "center");
    }
}
"#;

const GAME_SPACE_SHOOTER: &str = r#"
fn on_init(state) {
    state.px = 180;
    state.hp = 3;
    state.fire_cd = 0;
    state.spawn_t = 0;
    state.bullets = [];
    state.enemies = [
        { x: 180, y: 115, r: 16, vy: 115 },
        { x: 108, y: 48, r: 15, vy: 122 },
        { x: 252, y: 26, r: 15, vy: 128 }
    ];
    state.score = 0;
    state.high_score = 0;
    state.game_over = false;
    state.status_text = "HP 3 · FIRE WITH SPACE";
}

fn on_update(state, dt, input) {
    if state.game_over {
        return;
    }
    if input.left { state.px -= 260 * dt; }
    if input.right { state.px += 260 * dt; }
    if input.pointer_active && input.mouse_down {
        state.px = lerp(state.px, input.mouse_x, clamp(dt * 12, 0, 1));
    }
    state.px = clamp(state.px, 24, 336);

    state.fire_cd = max(0, state.fire_cd - dt);
    if (input.action || input.up || input.mouse_down) && state.fire_cd <= 0 {
        push(state.bullets, { x: state.px, y: 534, alive: true });
        state.fire_cd = 0.18;
        tone(880, 0.05, "square", 0.38);
    }

    state.spawn_t += dt;
    if state.spawn_t >= 0.85 {
        state.spawn_t = 0;
        push(state.enemies, {
            x: rand_range(34, 326),
            y: -14,
            r: 15,
            vy: rand_range(115, 175)
        });
    }

    for b in 0..len(state.bullets) {
        state.bullets[b].y -= 470 * dt;
    }
    let next_enemies = [];
    for e in 0..len(state.enemies) {
        let en = state.enemies[e];
        en.y += en.vy * dt;
        let hit = false;
        for b in 0..len(state.bullets) {
            let bl = state.bullets[b];
            if bl.alive && circle_hit(bl.x, bl.y, 6, en.x, en.y, en.r) {
                bl.alive = false;
                hit = true;
                state.score += 10;
                if state.score > state.high_score { state.high_score = state.score; }
                tone(420, 0.08, "saw", 0.45);
                break;
            }
        }
        if !hit {
            if circle_hit(en.x, en.y, en.r, state.px, 558, 18) || en.y > 608 {
                state.hp -= 1;
                tone(150, 0.2, "noise", 0.55);
                if state.hp <= 0 {
                    state.game_over = true;
                    state.status_text = "SHIP DESTROYED · PRESS R";
                } else {
                    state.status_text = "HP " + str(state.hp) + " · SCORE " + str(state.score);
                }
            } else {
                push(next_enemies, en);
            }
        }
    }
    state.enemies = next_enemies;

    let next_bullets = [];
    for b in 0..len(state.bullets) {
        let bl = state.bullets[b];
        if bl.alive && bl.y > -10 {
            push(next_bullets, bl);
        }
    }
    state.bullets = next_bullets;
}

fn on_render(state, t, canvas) {
    clear_grad(rgb(5, 8, 22), rgb(14, 18, 44), 180);
    for s in 0..20 {
        let sx = hash2(s, 2.1) * 360;
        let sy = (hash2(s, 7.7) * 640 + t * 110) % 640;
        circle(sx, sy, 1.4, rgba(190, 220, 255, 0.55));
    }
    for b in 0..len(state.bullets) {
        let bl = state.bullets[b];
        glow_circle(bl.x, bl.y, 5, rgb(85, 250, 255), 1.3);
        rect(bl.x - 2, bl.y - 8, 4, 14, rgb(225, 255, 255));
    }
    for e in 0..len(state.enemies) {
        let en = state.enemies[e];
        glow_circle(en.x, en.y, en.r * 1.25, rgba(255, 65, 130, 0.7), 1.3);
        star(en.x, en.y, en.r, en.r * 0.5, 5, rgb(255, 95, 155), t * 2.5);
    }
    polygon([[state.px, 536], [state.px + 20, 574], [state.px, 564], [state.px - 20, 574]], rgb(110, 240, 255));
    text(22, 46, "SCORE " + str(state.score), 16, rgb(245, 250, 255), "left");
    text(338, 46, "HP " + str(state.hp), 16, rgb(255, 120, 165), "right");
    if state.game_over {
        round_rect(48, 270, 264, 84, 16, rgba(8, 10, 24, 0.85));
        text(180, 306, "MISSION FAILED", 22, rgb(255, 90, 140), "center");
        text(180, 336, "SCORE " + str(state.score) + " · BEST " + str(state.high_score), 14, rgb(225, 238, 255), "center");
    }
}
"#;

const GAME_BRICK_BREAKER: &str = r#"
fn on_init(state) {
    state.paddle_x = 180;
    state.paddle_w = 88;
    state.bx = 180;
    state.by = 430;
    state.vx = 105;
    state.vy = -275;
    state.lives = 2;
    let bricks = [];
    for r in 0..5 {
        for c in 0..5 {
            push(bricks, {
                x: 20 + c * 65,
                y: 88 + r * 26,
                w: 58,
                h: 18,
                alive: true,
                hue: 185 + r * 34
            });
        }
    }
    state.bricks = bricks;
    state.score = 0;
    state.high_score = 0;
    state.game_over = false;
    state.status_text = "LIVES 2 · BREAK ALL BRICKS";
}

fn on_update(state, dt, input) {
    if state.game_over {
        return;
    }
    if input.left { state.paddle_x -= 320 * dt; }
    if input.right { state.paddle_x += 320 * dt; }
    if input.pointer_active {
        state.paddle_x = lerp(state.paddle_x, input.mouse_x, clamp(dt * 14, 0, 1));
    }
    let half = state.paddle_w * 0.5;
    state.paddle_x = clamp(state.paddle_x, half + 6, 354 - half);

    state.bx += state.vx * dt;
    state.by += state.vy * dt;
    if state.bx < 10 { state.bx = 10; state.vx = abs(state.vx); }
    if state.bx > 350 { state.bx = 350; state.vx = -abs(state.vx); }
    if state.by < 56 { state.by = 56; state.vy = abs(state.vy); }

    if state.vy > 0 && circle_rect_hit(state.bx, state.by, 8, state.paddle_x - half, 558, state.paddle_w, 14) {
        let offset = clamp((state.bx - state.paddle_x) / half, -1, 1);
        state.vx = offset * 240;
        state.vy = -abs(state.vy);
        tone(520, 0.05, "triangle", 0.45);
    }

    for i in 0..len(state.bricks) {
        let br = state.bricks[i];
        if br.alive && circle_rect_hit(state.bx, state.by, 8, br.x, br.y, br.w, br.h) {
            br.alive = false;
            state.vy = -state.vy;
            state.score += 10;
            if state.score > state.high_score { state.high_score = state.score; }
            state.status_text = "LIVES " + str(state.lives) + " · SCORE " + str(state.score);
            tone(660 + (i % 5) * 45, 0.06, "sine", 0.45);
            break;
        }
    }

    if state.by > 626 {
        state.lives -= 1;
        tone(145, 0.2, "saw", 0.55);
        if state.lives <= 0 {
            state.game_over = true;
            state.status_text = "NO LIVES LEFT · PRESS R";
        } else {
            state.bx = 180;
            state.by = 430;
            state.vx = 85;
            state.vy = 260;
            state.status_text = "LIVES " + str(state.lives) + " · SCORE " + str(state.score);
        }
    }
}

fn on_render(state, t, canvas) {
    clear_grad(rgb(10, 8, 28), rgb(22, 12, 46), 180);
    for i in 0..len(state.bricks) {
        let br = state.bricks[i];
        if br.alive {
            round_rect(br.x, br.y, br.w, br.h, 4, hsla(br.hue, 0.92, 0.6, 0.95));
        }
    }
    let half = state.paddle_w * 0.5;
    round_rect(state.paddle_x - half, 558, state.paddle_w, 14, 7, rgb(95, 245, 255));
    glow_circle(state.bx, state.by, 12, rgba(255, 110, 215, 0.8), 1.4);
    circle(state.bx, state.by, 7.5, rgb(255, 240, 250));
    text(22, 42, "SCORE " + str(state.score), 15, rgb(245, 250, 255), "left");
    text(338, 42, "LIVES " + str(state.lives), 15, rgb(120, 245, 255), "right");
    if state.game_over {
        round_rect(48, 270, 264, 84, 16, rgba(10, 10, 26, 0.85));
        text(180, 306, "GAME OVER", 22, rgb(255, 95, 170), "center");
        text(180, 336, "SCORE " + str(state.score) + " · BEST " + str(state.high_score), 14, rgb(225, 238, 255), "center");
    }
}
"#;
