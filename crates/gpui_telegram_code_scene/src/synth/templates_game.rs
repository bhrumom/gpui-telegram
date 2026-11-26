//! 5 playable interactive Game procedural templates (`CS-F08`).
//!
//! Every Game implements keyboard + pointer controls, real collision detection,
//! live HUD (`state.score`, `state.high_score`, `state.game_over`, `state.status_text`),
//! capped particle/entity lists, and clean `VmInstance::restart()` high-score preservation.

use crate::synth::intent::SceneArchetype;
use crate::synth::templates_video::{TemplateParams, escape_cs1_str, format_params_header};

pub fn generate_game_template(p: &TemplateParams) -> String {
    let header = format_params_header(p);
    let title = escape_cs1_str(&p.headline, 28);
    let body = match p.archetype {
        SceneArchetype::GameNeonPinballBreakout => gen_pinball_breakout(p, &title),
        SceneArchetype::GameSpaceShooter => gen_space_shooter(p, &title),
        SceneArchetype::GameCyberRunnerDodge => gen_cyber_runner(p, &title),
        SceneArchetype::GameFlappyCyber => gen_flappy_cyber(p, &title),
        SceneArchetype::GameNeonSnake => gen_neon_snake(p, &title),
        _ => gen_pinball_breakout(p, &title),
    };
    format!("{header}\n{body}")
}

fn gen_pinball_breakout(p: &TemplateParams, title: &str) -> String {
    let (r1, g1, b1) = p.bg_top;
    let (r2, g2, b2) = p.bg_bot;
    format!(
        r#"fn on_init(state) {{
    state.score = 0;
    if state.high_score == null {{ state.high_score = 0; }}
    state.game_over = false;
    state.lives = 2;
    state.status_text = "{title}";
    state.px = 180.0;
    state.pw = clamp(92.0 - DIFFICULTY * 10.0, 60.0, 96.0);
    state.bx = 180.0;
    state.by = 460.0;
    state.vx = 78.0 * SPEED_MUL;
    state.vy = -310.0 * SPEED_MUL;
    state.sparks = [];
    state.bricks = [];
    let rows = clamp(floor(3 + DENSITY), 3, 5);
    for r in 0..rows {{
        for c in 0..6 {{
            push(state.bricks, {{
                x: 18.0 + c * 55.0,
                y: 92.0 + r * 26.0,
                w: 49.0,
                h: 18.0,
                alive: true,
                hue: (PALETTE_HUE + r * 28 + c * 9) % 360
            }});
        }}
    }}
    state.bumpers = [];
    let bcnt = clamp(OBSTACLES, 1, 7);
    for k in 0..bcnt {{
        let bx = 56.0 + ((k + 0.5) * (248.0 / bcnt));
        let by = 275.0 + (k % 2) * 58.0;
        push(state.bumpers, {{ x: bx, y: by, r: 15.0, flash: 0.0 }});
    }}
}}

fn spawn_sparks(state, x, y, hue) {{
    let cap = clamp(PARTICLES, 6, 24);
    for i in 0..4 {{
        if len(state.sparks) < cap {{
            let a = rand_range(0, TAU);
            let s = rand_range(60, 170);
            push(state.sparks, {{ x: x, y: y, vx: cos(a) * s, vy: sin(a) * s, life: 0.45, hue: hue }});
        }}
    }}
}}

fn on_update(state, dt, input) {{
    if state.game_over {{ return; }}
    let step = min(dt, 0.04);
    let move_spd = 310.0 * SPEED_MUL;
    if input.left {{ state.px -= move_spd * step; }}
    if input.right {{ state.px += move_spd * step; }}
    if input.pointer_active {{ state.px = lerp(state.px, input.mouse_x, 0.35); }}
    state.px = clamp(state.px, state.pw * 0.5 + 8.0, 352.0 - state.pw * 0.5);

    if input.action || input.clicked {{
        let max_spd = min(420.0 * SPEED_MUL, 620.0);
        state.vy = clamp(state.vy * 1.02, -max_spd, max_spd);
        state.vx = clamp(state.vx, -max_spd, max_spd);
    }}

    state.bx += state.vx * step;
    state.by += state.vy * step;
    if state.bx < 12.0 {{ state.bx = 12.0; state.vx = abs(state.vx); }}
    if state.bx > 348.0 {{ state.bx = 348.0; state.vx = -abs(state.vx); }}
    if state.by < 76.0 {{ state.by = 76.0; state.vy = abs(state.vy); }}

    if state.vy > 0 && circle_rect_hit(state.bx, state.by, 9.0, state.px - state.pw * 0.5, 560.0, state.pw, 14.0) {{
        state.by = 550.0;
        let off = (state.bx - state.px) / (state.pw * 0.5);
        state.vx = clamp(off * 240.0 * SPEED_MUL, -290.0, 290.0);
        state.vy = -abs(state.vy);
        tone(340.0, 0.05, "triangle", 0.45);
    }}

    for bm in state.bumpers {{
        bm.flash = max(0.0, bm.flash - step * 3.0);
        if circle_hit(state.bx, state.by, 9.0, bm.x, bm.y, bm.r) {{
            let dx = state.bx - bm.x;
            let dy = state.by - bm.y;
            let d = max(1.0, hypot(dx, dy));
            let spd = clamp(hypot(state.vx, state.vy) * 1.03, 240.0, min(440.0 * SPEED_MUL, 620.0));
            state.vx = (dx / d) * spd;
            state.vy = (dy / d) * spd;
            state.bx = bm.x + (dx / d) * (bm.r + 10.5);
            state.by = bm.y + (dy / d) * (bm.r + 10.5);
            bm.flash = 1.0;
            state.score += 15;
            if state.score > state.high_score {{ state.high_score = state.score; }}
            spawn_sparks(state, bm.x, bm.y, ACCENT_HUE);
            tone(520.0 + SHADER_PAL * 40.0, 0.06, "sine", 0.5);
        }}
    }}

    let remaining = 0;
    for bk in state.bricks {{
        if bk.alive {{
            remaining += 1;
            if circle_rect_hit(state.bx, state.by, 9.0, bk.x, bk.y, bk.w, bk.h) {{
                bk.alive = false;
                state.vy = -state.vy;
                state.score += 10;
                if state.score > state.high_score {{ state.high_score = state.score; }}
                spawn_sparks(state, bk.x + bk.w * 0.5, bk.y + bk.h * 0.5, bk.hue);
                tone(440.0 + (state.score % 120), 0.05, "square", 0.4);
                break;
            }}
        }}
    }}
    if remaining == 0 {{
        for bk in state.bricks {{ bk.alive = true; }}
    }}

    if state.by > 648.0 {{
        state.lives -= 1;
        tone(140.0, 0.14, "saw", 0.5);
        if state.lives <= 0 {{
            state.game_over = true;
            state.status_text = "GAME OVER · SCORE " + str(state.score);
        }} else {{
            state.bx = state.px;
            state.by = 480.0;
            state.vx = 95.0 * SPEED_MUL;
            state.vy = -300.0 * SPEED_MUL;
        }}
    }}

    let kept = [];
    for sp in state.sparks {{
        sp.x += sp.vx * step;
        sp.y += sp.vy * step;
        sp.life -= step;
        if sp.life > 0.02 {{ push(kept, sp); }}
    }}
    state.sparks = kept;
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    clear_grad(rgb({r1}, {g1}, {b1}), rgb({r2}, {g2}, {b2}), 180);
    stroke_rect(6, 72, w - 12, h - 78, hsla(PALETTE_HUE, 0.85, 0.55, 0.35), 2.0);

    for bk in state.bricks {{
        if bk.alive {{
            round_rect(bk.x, bk.y, bk.w, bk.h, 4, hsla(bk.hue, 0.92, 0.58, 0.92));
            stroke_rect(bk.x, bk.y, bk.w, bk.h, rgba(255, 255, 255, 0.35), 1.0);
        }}
    }}

    for bm in state.bumpers {{
        let r = bm.r + bm.flash * 5.0;
        glow_circle(bm.x, bm.y, r + 4.0, hsla(ACCENT_HUE, 0.95, 0.62, 0.45 + bm.flash * 0.4), GLOW_LEVEL);
        star(bm.x, bm.y, r, r * 0.48, 6, hsla(ACCENT_HUE, 0.96, 0.70, 0.94), t * 1.8 + SEED_PHASE);
    }}

    for sp in state.sparks {{
        circle(sp.x, sp.y, 3.0, hsla(sp.hue, 0.95, 0.72, sp.life * 2.0));
    }}

    round_rect(state.px - state.pw * 0.5, 560, state.pw, 14, 7, hsla(PALETTE_HUE, 0.95, 0.66, 0.96));
    glow_circle(state.bx, state.by, 11.0, hsla(ACCENT_HUE, 0.98, 0.68, 0.92), GLOW_LEVEL);
    circle(state.bx, state.by, 7.0, rgba(255, 255, 255, 0.98));

    text(20, 42, "SCORE " + str(state.score), 16, hsla(PALETTE_HUE, 0.92, 0.84, 1.0), "left");
    text(w - 20, 42, "BEST " + str(state.high_score) + " · HP " + str(state.lives), 14, hsla(ACCENT_HUE, 0.92, 0.80, 1.0), "right");
    if state.game_over {{
        round_rect(42, h * 0.42, w - 84, 86, 14, rgba(8, 10, 22, 0.88));
        text(w * 0.5, h * 0.42 + 38, "GAME OVER", 22, hsla(350, 0.95, 0.66, 1.0), "center");
        text(w * 0.5, h * 0.42 + 66, "SCORE " + str(state.score), 15, rgba(255, 255, 255, 0.95), "center");
    }}
}}"#
    )
}

fn gen_space_shooter(p: &TemplateParams, title: &str) -> String {
    let (r1, g1, b1) = p.bg_top;
    let (r2, g2, b2) = p.bg_bot;
    format!(
        r#"fn on_init(state) {{
    state.score = 0;
    if state.high_score == null {{ state.high_score = 0; }}
    state.game_over = false;
    state.hp = 2;
    state.status_text = "{title}";
    state.sx = 180.0;
    state.cd = 0.0;
    state.bullets = [];
    state.enemies = [];
    state.rocks = [];
    state.sparks = [];
    for i in 0..4 {{
        push(state.enemies, {{
            x: 180.0 + (i - 1.5) * 62.0,
            y: 120.0 + (i % 2) * 44.0,
            vy: (92.0 + i * 14.0) * SPEED_MUL * DIFFICULTY
        }});
    }}
    push(state.enemies, {{ x: 180.0, y: 170.0, vy: 110.0 * SPEED_MUL * DIFFICULTY }});
    let rcnt = clamp(OBSTACLES, 1, 6);
    for k in 0..rcnt {{
        push(state.rocks, {{
            x: 42.0 + k * (276.0 / rcnt),
            y: 70.0 + (k % 3) * 50.0,
            r: 13.0,
            vy: (105.0 + k * 12.0) * SPEED_MUL * DIFFICULTY
        }});
    }}
}}

fn on_update(state, dt, input) {{
    if state.game_over {{ return; }}
    let step = min(dt, 0.04);
    let spd = 290.0 * SPEED_MUL;
    if input.left {{ state.sx -= spd * step; }}
    if input.right {{ state.sx += spd * step; }}
    if input.pointer_active {{ state.sx = lerp(state.sx, input.mouse_x, 0.30); }}
    state.sx = clamp(state.sx, 24.0, 336.0);

    state.cd = max(0.0, state.cd - step);
    if (input.action || input.up || input.clicked || input.mouse_down) && state.cd <= 0.0 && len(state.bullets) < 18 {{
        push(state.bullets, {{ x: state.sx, y: 534.0 }});
        state.cd = clamp(0.18 / DENSITY, 0.09, 0.24);
        tone(640.0 + SHADER_PAL * 35.0, 0.04, "square", 0.35);
    }}

    let nb = [];
    for b in state.bullets {{
        b.y -= 520.0 * step;
        if b.y > 48.0 {{ push(nb, b); }}
    }}
    state.bullets = nb;

    for e in state.enemies {{
        e.y += e.vy * step;
        let hit = false;
        let kept_b = [];
        for b in state.bullets {{
            if !hit && circle_hit(b.x, b.y, 7.0, e.x, e.y, 17.0) {{
                hit = true;
                state.score += 20;
                if state.score > state.high_score {{ state.high_score = state.score; }}
                let cap = clamp(PARTICLES, 6, 24);
                if len(state.sparks) < cap {{
                    push(state.sparks, {{ x: e.x, y: e.y, r: 6.0, life: 0.4 }});
                }}
                tone(420.0 + (state.score % 160), 0.06, "triangle", 0.45);
            }} else {{
                push(kept_b, b);
            }}
        }}
        state.bullets = kept_b;
        if hit {{
            e.y = 64.0;
            e.x = rand_range(34, 326);
        }} else if circle_hit(e.x, e.y, 16.0, state.sx, 560.0, 18.0) || e.y > 602.0 {{
            state.hp -= 1;
            e.y = 64.0;
            e.x = rand_range(34, 326);
            tone(130.0, 0.14, "saw", 0.5);
            if state.hp <= 0 {{
                state.game_over = true;
                state.status_text = "GAME OVER · SCORE " + str(state.score);
                return;
            }}
        }}
    }}

    for rk in state.rocks {{
        rk.y += rk.vy * step;
        if circle_hit(rk.x, rk.y, rk.r, state.sx, 560.0, 17.0) {{
            state.hp -= 1;
            rk.y = 56.0;
            rk.x = rand_range(30, 330);
            tone(115.0, 0.14, "saw", 0.5);
            if state.hp <= 0 {{
                state.game_over = true;
                state.status_text = "GAME OVER · SCORE " + str(state.score);
                return;
            }}
        }} else if rk.y > 650.0 {{
            rk.y = 56.0;
            rk.x = rand_range(30, 330);
        }}
    }}

    let ks = [];
    for sp in state.sparks {{
        sp.r += step * 80.0;
        sp.life -= step;
        if sp.life > 0.02 {{ push(ks, sp); }}
    }}
    state.sparks = ks;
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    clear_grad(rgb({r1}, {g1}, {b1}), rgb({r2}, {g2}, {b2}), 180);

    let bg_stars = clamp(PARTICLES, 8, 28);
    for s in 0..bg_stars {{
        let sx = hash2(s + 1.0, SEED_PHASE) * w;
        let sy = fract(hash2(s + 4.0, 2.0) + t * 0.28 * SPEED_MUL) * h;
        circle(sx, sy, 1.8, rgba(220, 238, 255, 0.55));
    }}

    for b in state.bullets {{
        round_rect(b.x - 2.5, b.y - 10, 5, 18, 2, hsla(PALETTE_HUE, 0.98, 0.72, 0.98));
    }}
    for e in state.enemies {{
        glow_circle(e.x, e.y, 16.0, hsla(ACCENT_HUE, 0.95, 0.58, 0.65), GLOW_LEVEL);
        star(e.x, e.y, 15.0, 7.0, 5, hsla(ACCENT_HUE, 0.96, 0.68, 0.96), t * 2.2);
    }}
    for rk in state.rocks {{
        stroke_circle(rk.x, rk.y, rk.r, hsla(34, 0.85, 0.64, 0.88), 2.5);
    }}
    for sp in state.sparks {{
        stroke_circle(sp.x, sp.y, sp.r, hsla(PALETTE_HUE, 0.98, 0.75, sp.life * 2.2), 2.4);
    }}

    let ship = [[state.sx, 536], [state.sx + 20, 576], [state.sx, 566], [state.sx - 20, 576]];
    polygon(ship, hsla(PALETTE_HUE, 0.95, 0.66, 0.96));
    glow_circle(state.sx, 572, 10.0, hsla(ACCENT_HUE, 0.96, 0.64, 0.85), GLOW_LEVEL);

    text(20, 40, "SCORE " + str(state.score), 16, hsla(PALETTE_HUE, 0.92, 0.86, 1.0), "left");
    text(w - 20, 40, "BEST " + str(state.high_score) + " · HP " + str(state.hp), 14, hsla(ACCENT_HUE, 0.92, 0.82, 1.0), "right");
    if state.game_over {{
        round_rect(42, h * 0.42, w - 84, 86, 14, rgba(8, 10, 22, 0.88));
        text(w * 0.5, h * 0.42 + 38, "GAME OVER", 22, hsla(350, 0.95, 0.66, 1.0), "center");
        text(w * 0.5, h * 0.42 + 66, "SCORE " + str(state.score), 15, rgba(255, 255, 255, 0.95), "center");
    }}
}}"#
    )
}

fn gen_cyber_runner(p: &TemplateParams, title: &str) -> String {
    let (r1, g1, b1) = p.bg_top;
    let (r2, g2, b2) = p.bg_bot;
    format!(
        r#"fn on_init(state) {{
    state.score = 0;
    if state.high_score == null {{ state.high_score = 0; }}
    state.game_over = false;
    state.shields = 2;
    state.status_text = "{title}";
    state.px = 180.0;
    state.dash = 0.0;
    state.orbs = [
        {{ x: 180.0, y: 260.0, vy: 215.0 * SPEED_MUL }},
        {{ x: 110.0, y: 120.0, vy: 215.0 * SPEED_MUL }}
    ];
    state.barriers = [];
    let bcnt = clamp(OBSTACLES + 2, 3, 9);
    for k in 0..bcnt {{
        let bx = 36.0 + ((k * 83) % 240);
        if k == 0 {{ bx = 152.0; }}
        push(state.barriers, {{
            x: bx,
            y: -40.0 - k * 145.0,
            w: clamp(56.0 + DIFFICULTY * 14.0, 52.0, 92.0),
            h: 18.0,
            vy: (185.0 + (k % 3) * 28.0) * SPEED_MUL * DIFFICULTY
        }});
    }}
}}

fn on_update(state, dt, input) {{
    if state.game_over {{ return; }}
    let step = min(dt, 0.04);
    let spd = 300.0 * SPEED_MUL;
    if input.left {{ state.px -= spd * step; }}
    if input.right {{ state.px += spd * step; }}
    if input.pointer_active {{ state.px = lerp(state.px, input.mouse_x, 0.30); }}
    state.px = clamp(state.px, 26.0, 334.0);

    state.dash = max(0.0, state.dash - step);
    if (input.action || input.clicked) && state.dash <= 0.0 {{
        state.dash = 0.45;
        tone(520.0 + SHADER_PAL * 30.0, 0.06, "sine", 0.42);
    }}

    for orb in state.orbs {{
        orb.y += orb.vy * step;
        if circle_hit(orb.x, orb.y, 11.0, state.px, 535.0, 20.0) {{
            state.score += 15;
            if state.score > state.high_score {{ state.high_score = state.score; }}
            orb.y = 40.0;
            orb.x = rand_range(44, 316);
            tone(660.0, 0.05, "triangle", 0.48);
        }} else if orb.y > 650.0 {{
            orb.y = 40.0;
            orb.x = rand_range(44, 316);
        }}
    }}

    for br in state.barriers {{
        br.y += br.vy * step;
        if state.dash <= 0.05 && circle_rect_hit(state.px, 535.0, 15.0, br.x, br.y, br.w, br.h) {{
            state.shields -= 1;
            br.y = -60.0;
            br.x = rand_range(24, 336 - br.w);
            tone(125.0, 0.14, "saw", 0.5);
            if state.shields <= 0 {{
                state.game_over = true;
                state.status_text = "GAME OVER · SCORE " + str(state.score);
                return;
            }}
        }} else if br.y > 652.0 {{
            br.y = -50.0;
            br.x = rand_range(24, 336 - br.w);
            state.score += 2;
            if state.score > state.high_score {{ state.high_score = state.score; }}
        }}
    }}
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    clear_grad(rgb({r1}, {g1}, {b1}), rgb({r2}, {g2}, {b2}), 180);

    let lanes = clamp(floor(3 + DENSITY), 3, 6);
    for l in 1..lanes {{
        let lx = l * (w / lanes);
        line(lx, 64, lx, h, hsla(PALETTE_HUE, 0.75, 0.50, 0.18), 1.4);
    }}

    let trail = clamp(PARTICLES, 6, 20);
    for p in 0..trail {{
        let ty = 552.0 + p * 4.5;
        let tx = state.px + sin(t * 14.0 + p + SEED_PHASE) * 4.0;
        circle(tx, ty, 3.0 - p * 0.1, hsla(PALETTE_HUE, 0.95, 0.68, max(0.1, 0.7 - p * 0.03)));
    }}

    for orb in state.orbs {{
        glow_circle(orb.x, orb.y, 13.0, hsla(ACCENT_HUE, 0.98, 0.68, 0.85), GLOW_LEVEL);
        circle(orb.x, orb.y, 7.0, rgba(255, 255, 255, 0.95));
    }}

    for br in state.barriers {{
        round_rect(br.x, br.y, br.w, br.h, 5, hsla(350, 0.94, 0.56, 0.94));
        stroke_rect(br.x, br.y, br.w, br.h, rgba(255, 220, 220, 0.6), 1.4);
    }}

    let craft = [[state.px, 514], [state.px + 18, 548], [state.px, 540], [state.px - 18, 548]];
    polygon(craft, hsla(PALETTE_HUE, 0.96, 0.68, 0.96));
    if state.dash > 0.05 {{
        stroke_circle(state.px, 535, 26.0, hsla(ACCENT_HUE, 0.98, 0.76, 0.9), 3.0);
    }}

    text(20, 40, "SCORE " + str(state.score), 16, hsla(PALETTE_HUE, 0.92, 0.86, 1.0), "left");
    text(w - 20, 40, "BEST " + str(state.high_score) + " · SHD " + str(state.shields), 14, hsla(ACCENT_HUE, 0.92, 0.82, 1.0), "right");
    if state.game_over {{
        round_rect(42, h * 0.42, w - 84, 86, 14, rgba(8, 10, 22, 0.88));
        text(w * 0.5, h * 0.42 + 38, "GAME OVER", 22, hsla(350, 0.95, 0.66, 1.0), "center");
        text(w * 0.5, h * 0.42 + 66, "SCORE " + str(state.score), 15, rgba(255, 255, 255, 0.95), "center");
    }}
}}"#
    )
}

fn gen_flappy_cyber(p: &TemplateParams, title: &str) -> String {
    let (r1, g1, b1) = p.bg_top;
    let (r2, g2, b2) = p.bg_bot;
    format!(
        r#"fn on_init(state) {{
    state.score = 0;
    if state.high_score == null {{ state.high_score = 0; }}
    state.game_over = false;
    state.status_text = "{title}";
    state.by = 300.0;
    state.vy = 0.0;
    state.prev_flap = false;
    state.pipes = [
        {{ x: 215.0, gap_y: 300.0, scored: false, phase: SEED_PHASE }},
        {{ x: 405.0, gap_y: 265.0, scored: false, phase: SEED_PHASE + 1.4 }},
        {{ x: 595.0, gap_y: 345.0, scored: false, phase: SEED_PHASE + 2.8 }}
    ];
    state.trails = [];
}}

fn on_update(state, dt, input) {{
    if state.game_over {{ return; }}
    let step = min(dt, 0.04);
    let want_flap = input.up || input.action || input.clicked;
    if want_flap && !state.prev_flap {{
        state.vy = -215.0;
        tone(520.0 + SHADER_PAL * 35.0, 0.05, "triangle", 0.44);
    }}
    state.prev_flap = want_flap;

    state.vy += 620.0 * SPEED_MUL * step;
    state.by += state.vy * step;

    let cap = clamp(PARTICLES, 6, 18);
    if len(state.trails) < cap {{
        push(state.trails, {{ x: 108.0, y: state.by, life: 0.45 }});
    }}
    let kt = [];
    for tr in state.trails {{
        tr.x -= 130.0 * SPEED_MUL * step;
        tr.life -= step;
        if tr.life > 0.02 {{ push(kt, tr); }}
    }}
    state.trails = kt;

    let gap_h = clamp(162.0 - DIFFICULTY * 18.0, 108.0, 168.0);
    let scroll = 135.0 * SPEED_MUL * (0.85 + DENSITY * 0.15);
    for pipe in state.pipes {{
        pipe.x -= scroll * step;
        if OBSTACLES >= 4 {{
            pipe.phase += step * 1.8;
            pipe.gap_y = clamp(pipe.gap_y + sin(pipe.phase) * 32.0 * step, 145.0, 480.0);
        }}
        if !pipe.scored && pipe.x + 26.0 < 108.0 {{
            pipe.scored = true;
            state.score += 10;
            if state.score > state.high_score {{ state.high_score = state.score; }}
            tone(680.0, 0.06, "sine", 0.48);
        }}
        if pipe.x < -56.0 {{
            pipe.x += 570.0;
            pipe.gap_y = rand_range(160, 450);
            pipe.scored = false;
        }}
        let top_h = max(0.0, pipe.gap_y - gap_h * 0.5);
        let bot_y = pipe.gap_y + gap_h * 0.5;
        if circle_rect_hit(108.0, state.by, 13.0, pipe.x, 0.0, 46.0, top_h)
            || circle_rect_hit(108.0, state.by, 13.0, pipe.x, bot_y, 46.0, 640.0 - bot_y)
        {{
            state.game_over = true;
            state.status_text = "GAME OVER · SCORE " + str(state.score);
            tone(130.0, 0.16, "saw", 0.52);
            return;
        }}
    }}

    if state.by < 14.0 || state.by > 612.0 {{
        state.game_over = true;
        state.status_text = "GAME OVER · SCORE " + str(state.score);
        tone(120.0, 0.16, "saw", 0.52);
    }}
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    clear_grad(rgb({r1}, {g1}, {b1}), rgb({r2}, {g2}, {b2}), 180);

    let gap_h = clamp(162.0 - DIFFICULTY * 18.0, 108.0, 168.0);
    for pipe in state.pipes {{
        let top_h = max(0.0, pipe.gap_y - gap_h * 0.5);
        let bot_y = pipe.gap_y + gap_h * 0.5;
        round_rect(pipe.x, 0, 46, top_h, 6, hsla(PALETTE_HUE, 0.90, 0.50, 0.92));
        round_rect(pipe.x, bot_y, 46, h - bot_y, 6, hsla(PALETTE_HUE, 0.90, 0.50, 0.92));
        stroke_rect(pipe.x, 0, 46, top_h, hsla(ACCENT_HUE, 0.95, 0.72, 0.7), 1.6);
        stroke_rect(pipe.x, bot_y, 46, h - bot_y, hsla(ACCENT_HUE, 0.95, 0.72, 0.7), 1.6);
    }}

    for tr in state.trails {{
        circle(tr.x, tr.y, 4.0 * tr.life * 2.0, hsla(ACCENT_HUE, 0.96, 0.70, tr.life * 1.8));
    }}

    glow_circle(108.0, state.by, 16.0, hsla(ACCENT_HUE, 0.98, 0.65, 0.85), GLOW_LEVEL);
    circle(108.0, state.by, 12.0, hsla(PALETTE_HUE, 0.95, 0.72, 0.98));
    circle(114.0, state.by - 3.0, 3.0, rgba(255, 255, 255, 0.98));

    rect(0, h - 22, w, 22, hsla(PALETTE_HUE, 0.85, 0.25, 0.95));
    text(20, 40, "SCORE " + str(state.score), 16, hsla(PALETTE_HUE, 0.92, 0.88, 1.0), "left");
    text(w - 20, 40, "BEST " + str(state.high_score), 15, hsla(ACCENT_HUE, 0.92, 0.84, 1.0), "right");
    if state.game_over {{
        round_rect(42, h * 0.42, w - 84, 86, 14, rgba(8, 10, 22, 0.88));
        text(w * 0.5, h * 0.42 + 38, "GAME OVER", 22, hsla(350, 0.95, 0.66, 1.0), "center");
        text(w * 0.5, h * 0.42 + 66, "SCORE " + str(state.score), 15, rgba(255, 255, 255, 0.95), "center");
    }}
}}"#
    )
}

fn gen_neon_snake(p: &TemplateParams, title: &str) -> String {
    let (r1, g1, b1) = p.bg_top;
    let (r2, g2, b2) = p.bg_bot;
    format!(
        r#"fn on_init(state) {{
    state.score = 0;
    if state.high_score == null {{ state.high_score = 0; }}
    state.game_over = false;
    state.status_text = "{title}";
    state.body = [
        {{ x: 9, y: 14 }},
        {{ x: 8, y: 14 }},
        {{ x: 7, y: 14 }},
        {{ x: 6, y: 14 }}
    ];
    state.dx = 1;
    state.dy = 0;
    state.acc = 0.0;
    state.food_x = 12;
    state.food_y = 14;
    state.blocks = [];
    let obs = clamp(OBSTACLES - 2, 0, 6);
    for k in 0..obs {{
        let bx = 3 + ((k * 4) % 12);
        let by = 6 + ((k * 5) % 16);
        if by == 14 {{ by = 18; }}
        push(state.blocks, {{ x: bx, y: by }});
    }}
}}

fn on_update(state, dt, input) {{
    if state.game_over {{ return; }}
    if input.up && state.dy == 0 {{ state.dx = 0; state.dy = -1; }}
    else if input.down && state.dy == 0 {{ state.dx = 0; state.dy = 1; }}
    else if input.left && state.dx == 0 {{ state.dx = -1; state.dy = 0; }}
    else if input.right && state.dx == 0 {{ state.dx = 1; state.dy = 0; }}

    let boost = 1.0;
    if input.action {{ boost = 1.5; }}
    let interval = clamp(0.13 / (SPEED_MUL * DIFFICULTY * boost), 0.05, 0.22);
    state.acc += min(dt, 0.05);
    if state.acc < interval {{ return; }}
    state.acc -= interval;

    let head = state.body[0];
    let nx = head.x + state.dx;
    let ny = head.y + state.dy;
    if nx < 0 || nx >= 18 || ny < 0 || ny >= 28 {{
        state.game_over = true;
        state.status_text = "GAME OVER · SCORE " + str(state.score);
        tone(125.0, 0.16, "saw", 0.5);
        return;
    }}
    for blk in state.blocks {{
        if blk.x == nx && blk.y == ny {{
            state.game_over = true;
            state.status_text = "GAME OVER · SCORE " + str(state.score);
            tone(125.0, 0.16, "saw", 0.5);
            return;
        }}
    }}
    for seg in state.body {{
        if seg.x == nx && seg.y == ny {{
            state.game_over = true;
            state.status_text = "GAME OVER · SCORE " + str(state.score);
            tone(125.0, 0.16, "saw", 0.5);
            return;
        }}
    }}

    let ate = nx == state.food_x && ny == state.food_y;
    let next_body = [{{ x: nx, y: ny }}];
    let copy_len = len(state.body) - 1;
    if ate && len(state.body) < 42 {{ copy_len = len(state.body); }}
    for i in 0..copy_len {{
        push(next_body, state.body[i]);
    }}
    state.body = next_body;

    if ate {{
        state.score += 10;
        if state.score > state.high_score {{ state.high_score = state.score; }}
        state.food_x = floor(rand_range(1, 17));
        state.food_y = floor(rand_range(2, 26));
        tone(540.0 + SHADER_PAL * 40.0, 0.06, "triangle", 0.48);
    }}
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    clear_grad(rgb({r1}, {g1}, {b1}), rgb({r2}, {g2}, {b2}), 180);

    let ox = 0.0;
    let oy = 68.0;
    let cell = 20.0;
    stroke_rect(2, oy, 356, 560, hsla(PALETTE_HUE, 0.85, 0.55, 0.45), 2.0);

    let dots = clamp(PARTICLES, 6, 20);
    for d in 0..dots {{
        let dx = hash2(d + 1.0, SEED_PHASE) * w;
        let dy = oy + hash2(d + 7.0, DENSITY) * 540.0;
        circle(dx, dy, 1.8, hsla(PALETTE_HUE, 0.8, 0.6, 0.25));
    }}

    for blk in state.blocks {{
        round_rect(ox + blk.x * cell + 2, oy + blk.y * cell + 2, cell - 4, cell - 4, 4, hsla(350, 0.92, 0.56, 0.92));
    }}

    let fx = ox + state.food_x * cell + cell * 0.5;
    let fy = oy + state.food_y * cell + cell * 0.5;
    glow_circle(fx, fy, 11.0 + sin(t * 6.0) * 2.5, hsla(ACCENT_HUE, 0.98, 0.65, 0.92), GLOW_LEVEL);
    circle(fx, fy, 6.0, rgba(255, 255, 255, 0.95));

    let n = len(state.body);
    for i in 0..n {{
        let seg = state.body[i];
        let hue = (PALETTE_HUE + i * 7) % 360;
        let pad = 2.0;
        if i == 0 {{ pad = 1.0; }}
        round_rect(ox + seg.x * cell + pad, oy + seg.y * cell + pad, cell - pad * 2.0, cell - pad * 2.0, 5, hsla(hue, 0.95, 0.64, 0.96));
    }}

    text(20, 42, "SCORE " + str(state.score), 16, hsla(PALETTE_HUE, 0.92, 0.88, 1.0), "left");
    text(w - 20, 42, "BEST " + str(state.high_score), 15, hsla(ACCENT_HUE, 0.92, 0.84, 1.0), "right");
    if state.game_over {{
        round_rect(42, h * 0.42, w - 84, 86, 14, rgba(8, 10, 22, 0.88));
        text(w * 0.5, h * 0.42 + 38, "GAME OVER", 22, hsla(350, 0.95, 0.66, 1.0), "center");
        text(w * 0.5, h * 0.42 + 66, "SCORE " + str(state.score), 15, rgba(255, 255, 255, 0.95), "center");
    }}
}}"#
    )
}
