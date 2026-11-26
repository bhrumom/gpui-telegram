//! Code Animations A1–A3 (60fps pointer-reactive shaders & physics).

use gpui_telegram_proto::{AssetKind, CodeSceneAssetDto};

use crate::seed_library::{
    OFFICIAL_NEON_SHADER_BOT_ID, OFFICIAL_PHYSICS_LAB_BOT_ID, SeedSpec, build_seed_asset,
};

const BASE_TS: i64 = 1_760_000_000_000;

pub fn assets() -> Vec<CodeSceneAssetDto> {
    vec![
        build_seed_asset(SeedSpec {
            scene_id: "seed_anim_aurora_plasma",
            title: "极光等离子体着色器 · Aurora Plasma Shader",
            description: "实时 96x160 竖屏等离子体像素着色器，移动指针调制波长，点击切换极光色盘并激发波纹。",
            kind: AssetKind::Animation,
            bot_id: OFFICIAL_NEON_SHADER_BOT_ID,
            tags: &["shader", "plasma", "neon", "generative", "interactive"],
            duration_sec: None,
            seed: 2001,
            controls_hint: "移动指针调节波长 · 点击切换色盘并触发涟漪",
            plays_count: 1680,
            likes_count: 312,
            bookmarks_count: 118,
            comments_count: 42,
            shares_count: 58,
            remixes_count: 23,
            created_at_ms: BASE_TS - 86_400_000 * 7,
            source_code: ANIM_AURORA_PLASMA,
        }),
        build_seed_asset(SeedSpec {
            scene_id: "seed_anim_mandelbrot_abyss",
            title: "曼德博分形深渊 · Mandelbrot Abyss Zoom",
            description: "实时曼德博分形呼吸缩放，鼠标偏移引导焦点，点击切换三处经典分形海马谷地标。",
            kind: AssetKind::Animation,
            bot_id: OFFICIAL_NEON_SHADER_BOT_ID,
            tags: &["fractal", "math", "shader", "psychedelic", "interactive"],
            duration_sec: None,
            seed: 2002,
            controls_hint: "移动指针微调焦点 · 点击切换分形地标",
            plays_count: 390,
            likes_count: 84,
            bookmarks_count: 36,
            comments_count: 14,
            shares_count: 19,
            remixes_count: 8,
            created_at_ms: BASE_TS - 86_400_000 * 4,
            source_code: ANIM_MANDELBROT_ABYSS,
        }),
        build_seed_asset(SeedSpec {
            scene_id: "seed_anim_firefly_swarm",
            title: "引力萤火虫星群 · Magnetic Firefly Swarm",
            description: "42 只受柏林噪声与指针引力牵引的发光萤火虫，点击释放径向冲击波。",
            kind: AssetKind::Animation,
            bot_id: OFFICIAL_PHYSICS_LAB_BOT_ID,
            tags: &["particles", "physics", "boids", "interactive", "relaxing"],
            duration_sec: None,
            seed: 2003,
            controls_hint: "移动指针吸引星群 · 点击发射冲击波",
            plays_count: 890,
            likes_count: 175,
            bookmarks_count: 63,
            comments_count: 21,
            shares_count: 30,
            remixes_count: 12,
            created_at_ms: BASE_TS - 86_400_000 * 5,
            source_code: ANIM_FIREFLY_SWARM,
        }),
    ]
}

const ANIM_AURORA_PLASMA: &str = r#"
fn on_init(state) {
    pixel_init(96, 160);
    state.pal = 0;
    state.scale = 4.2;
    state.ripples = [];
}

fn on_update(state, dt, input) {
    if input.pointer_active {
        let target_scale = 2.0 + clamp(input.mouse_y / 640.0, 0, 1) * 7.5;
        state.scale = lerp(state.scale, target_scale, clamp(dt * 8.0, 0, 1));
    }
    if input.clicked {
        state.pal = (state.pal + 1) % 4;
        let rx = 180;
        let ry = 320;
        if input.pointer_active {
            rx = input.mouse_x;
            ry = input.mouse_y;
        }
        push(state.ripples, { x: rx, y: ry, r: 10, life: 1.0 });
        tone(330 + state.pal * 110, 0.14, "sine", 0.5);
    }
    let next = [];
    for i in 0..len(state.ripples) {
        let rp = state.ripples[i];
        rp.r += 210 * dt;
        rp.life -= dt * 1.15;
        if rp.life > 0 {
            push(next, rp);
        }
    }
    state.ripples = next;
}

fn on_render(state, t, canvas) {
    shader_plasma(t * 1.15, state.scale, state.pal);
    for i in 0..len(state.ripples) {
        let rp = state.ripples[i];
        stroke_circle(rp.x, rp.y, rp.r, rgba(255, 255, 255, rp.life * 0.85), 2.5);
        glow_circle(rp.x, rp.y, rp.r * 0.35, hsla(state.pal * 90 + 180, 0.95, 0.7, rp.life * 0.6), 1.4);
    }
    // Floating orbital accents
    for k in 0..5 {
        let a = t * (0.6 + k * 0.15) + k * 1.25;
        let px = 180 + cos(a) * (70 + k * 24);
        let py = 320 + sin(a * 1.3) * (110 + k * 18);
        glow_circle(px, py, 6 + (k % 3) * 2, rgba(255, 255, 255, 0.55), 1.2);
    }
    round_rect(42, 46, 276, 32, 16, rgba(6, 10, 22, 0.58));
    text(180, 67, "AURORA PLASMA · PAL #" + str(state.pal + 1), 13, rgb(225, 248, 255), "center");
}
"#;

const ANIM_MANDELBROT_ABYSS: &str = r#"
fn on_init(state) {
    pixel_init(96, 160);
    state.targets = [
        { cx: -0.743643, cy: 0.131825, name: "SEAHORSE VALLEY" },
        { cx: -0.1592, cy: 1.0317, name: "SPIRAL CROWN" },
        { cx: -1.749, cy: 0.0, name: "MINI BROT" }
    ];
    state.idx = 0;
    state.ox = 0;
    state.oy = 0;
}

fn on_update(state, dt, input) {
    if input.clicked {
        state.idx = (state.idx + 1) % len(state.targets);
        tone(440 + state.idx * 120, 0.12, "triangle", 0.45);
    }
    let tx = 0;
    let ty = 0;
    if input.pointer_active {
        tx = (input.mouse_x / 360.0 - 0.5) * 0.08;
        ty = (input.mouse_y / 640.0 - 0.5) * 0.08;
    }
    state.ox = lerp(state.ox, tx, clamp(dt * 5.0, 0, 1));
    state.oy = lerp(state.oy, ty, clamp(dt * 5.0, 0, 1));
}

fn on_render(state, t, canvas) {
    let tgt = state.targets[state.idx];
    let breath = 0.5 - 0.5 * cos(t * 0.45);
    let zoom = 1.2 * pow(140.0, breath);
    shader_mandelbrot(tgt.cx + state.ox / sqrt(zoom), tgt.cy + state.oy / sqrt(zoom), zoom, 28);

    // Subtle scientific reticle overlay
    stroke_circle(180, 320, 38, rgba(255, 255, 255, 0.22), 1.2);
    line(164, 320, 196, 320, rgba(255, 255, 255, 0.25), 1.0);
    line(180, 304, 180, 336, rgba(255, 255, 255, 0.25), 1.0);
    round_rect(36, 46, 288, 34, 17, rgba(8, 10, 24, 0.68));
    text(180, 68, tgt.name + " · " + str(floor(zoom * 10) / 10) + "x", 13, rgb(235, 215, 255), "center");
}
"#;

const ANIM_FIREFLY_SWARM: &str = r#"
fn on_init(state) {
    let pts = [];
    for i in 0..42 {
        push(pts, {
            x: rand_range(30, 330),
            y: rand_range(70, 570),
            vx: rand_range(-40, 40),
            vy: rand_range(-40, 40),
            hue: 145 + (i % 6) * 28,
            r: rand_range(3.5, 6.8)
        });
    }
    state.pts = pts;
    state.ax = 180;
    state.ay = 320;
    state.pulse = 0;
}

fn on_update(state, dt, input) {
    if input.pointer_active {
        state.ax = input.mouse_x;
        state.ay = input.mouse_y;
    } else {
        state.ax = 180 + sin(state.pulse * 0.9) * 105;
        state.ay = 320 + cos(state.pulse * 1.3) * 155;
    }
    state.pulse += dt;

    for i in 0..len(state.pts) {
        let p = state.pts[i];
        let dx = state.ax - p.x;
        let dy = state.ay - p.y;
        let d = max(dist(p.x, p.y, state.ax, state.ay), 14.0);
        if input.clicked {
            p.vx -= (dx / d) * 320;
            p.vy -= (dy / d) * 320;
        } else {
            let pull = 220.0 / (d * 0.08 + 1.0);
            p.vx += (dx / d) * pull * dt;
            p.vy += (dy / d) * pull * dt;
        }
        let n = noise2d(p.x * 0.012 + state.pulse * 0.4, p.y * 0.012);
        p.vx += cos(n * TAU) * 75 * dt;
        p.vy += sin(n * TAU) * 75 * dt;
        p.vx *= 0.965;
        p.vy *= 0.965;
        p.x = clamp(p.x + p.vx * dt, 10, 350);
        p.y = clamp(p.y + p.vy * dt, 20, 620);
    }
    if input.clicked {
        tone(620, 0.14, "sine", 0.5);
    }
}

fn on_render(state, t, canvas) {
    clear_grad(rgb(5, 10, 22), rgb(10, 24, 38), 180);
    stroke_circle(state.ax, state.ay, 22 + sin(t * 5) * 4, rgba(130, 255, 225, 0.45), 1.8);
    glow_circle(state.ax, state.ay, 10, rgba(130, 255, 225, 0.7), 1.3);

    for i in 0..len(state.pts) {
        let p = state.pts[i];
        let tw = 0.65 + 0.35 * sin(t * 5.0 + i);
        let col = hsla(p.hue, 0.95, 0.66, 0.9 * tw);
        line(p.x - p.vx * 0.06, p.y - p.vy * 0.06, p.x, p.y, hsla(p.hue, 0.9, 0.7, 0.35), 2.0);
        glow_circle(p.x, p.y, p.r * 1.8, col, 1.3);
        circle(p.x, p.y, p.r * 0.65, rgb(255, 255, 245));
    }
    round_rect(44, 46, 272, 32, 16, rgba(5, 12, 24, 0.65));
    text(180, 67, "MAGNETIC FIREFLY SWARM · 42", 13, rgb(155, 255, 220), "center");
}
"#;
