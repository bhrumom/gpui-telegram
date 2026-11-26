//! Code Animations A4–A6 (60fps pointer-reactive shaders & recursive fractal tree).

use gpui_telegram_proto::{AssetKind, CodeSceneAssetDto};

use crate::seed_library::{
    OFFICIAL_CINEMA_CODE_BOT_ID, OFFICIAL_FABUSHI_BOT_ID, OFFICIAL_PHYSICS_LAB_BOT_ID, SeedSpec,
    build_seed_asset,
};

const BASE_TS: i64 = 1_760_000_000_000;

pub fn assets() -> Vec<CodeSceneAssetDto> {
    vec![
        build_seed_asset(SeedSpec {
            scene_id: "seed_anim_voronoi_reef",
            title: "生物荧光泰森多边形 · Bioluminescent Voronoi Reef",
            description: "深海荧光泰森多边形着色器叠合半透明矢量水母，点击切换细胞密度与荧光色系。",
            kind: AssetKind::Animation,
            bot_id: OFFICIAL_FABUSHI_BOT_ID,
            tags: &["voronoi", "shader", "ocean", "generative", "organic"],
            duration_sec: None,
            seed: 2004,
            controls_hint: "点击切换细胞密度与色系 · 移动指针引导水母",
            // Tier0 Cold-Start (< 20 plays)
            plays_count: 15,
            likes_count: 5,
            bookmarks_count: 2,
            comments_count: 1,
            shares_count: 1,
            remixes_count: 1,
            created_at_ms: BASE_TS - 3_600_000 * 3,
            source_code: ANIM_VORONOI_REEF,
        }),
        build_seed_asset(SeedSpec {
            scene_id: "seed_anim_synthwave_horizon",
            title: "复古合成波日落公路 · Outrun Synthwave Horizon",
            description: "80 年代复古合成波地平线透视网格，左右移动切换霓虹色相，上下调速，点击氮气加速。",
            kind: AssetKind::Animation,
            bot_id: OFFICIAL_CINEMA_CODE_BOT_ID,
            tags: &["synthwave", "retro", "vaporwave", "shader", "neon"],
            duration_sec: None,
            seed: 2005,
            controls_hint: "左右移动调色相 · 上下调巡航速度 · 点击加速",
            plays_count: 1120,
            likes_count: 224,
            bookmarks_count: 89,
            comments_count: 27,
            shares_count: 39,
            remixes_count: 17,
            created_at_ms: BASE_TS - 86_400_000 * 6,
            source_code: ANIM_SYNTHWAVE_HORIZON,
        }),
        build_seed_asset(SeedSpec {
            scene_id: "seed_anim_fractal_sakura",
            title: "微风分形樱花树 · Wind-Swaying Fractal Sakura",
            description: "7 层递归樱花树随指针风向摇曳，24 片樱花花瓣飘落，点击轻摇树冠触发花雨。",
            kind: AssetKind::Animation,
            bot_id: OFFICIAL_PHYSICS_LAB_BOT_ID,
            tags: &["fractal", "nature", "art", "physics", "interactive"],
            duration_sec: None,
            seed: 2006,
            controls_hint: "移动指针改变风向 · 点击摇落樱花雨",
            plays_count: 68,
            likes_count: 21,
            bookmarks_count: 10,
            comments_count: 4,
            shares_count: 6,
            remixes_count: 2,
            created_at_ms: BASE_TS - 86_400_000 * 2,
            source_code: ANIM_FRACTAL_SAKURA,
        }),
    ]
}

const ANIM_VORONOI_REEF: &str = r#"
fn on_init(state) {
    pixel_init(72, 128);
    state.mode = 1;
    state.jx = 180;
    state.jy = 300;
}

fn on_update(state, dt, input) {
    if input.clicked {
        state.mode = (state.mode + 1) % 3;
        tone(392 + state.mode * 88, 0.14, "sine", 0.45);
    }
    if input.pointer_active {
        state.jx = lerp(state.jx, input.mouse_x, clamp(dt * 4.0, 0, 1));
        state.jy = lerp(state.jy, input.mouse_y, clamp(dt * 4.0, 0, 1));
    }
}

fn on_render(state, t, canvas) {
    let cells = 6 + state.mode * 3;
    let pal = (state.mode + 3) % 4;
    shader_voronoi(t * 0.9, cells, pal);

    // Floating translucent bioluminescent jellyfish
    let jx = state.jx + sin(t * 1.1) * 28;
    let jy = state.jy + cos(t * 1.4) * 22;
    glow_circle(jx, jy, 34, rgba(95, 255, 230, 0.45), 1.5);
    arc(jx, jy, 28, PI, TAU, rgba(210, 255, 248, 0.9), 4.0);
    for k in 0..5 {
        let tx = jx - 18 + k * 9;
        let sway = sin(t * 3.5 + k * 0.7) * 10;
        bezier(tx, jy + 2, tx + sway, jy + 32, tx - sway, jy + 58, tx + sway * 0.5, jy + 82, rgba(165, 255, 240, 0.7), 1.8);
    }
    // Rising ocean bubbles
    for b in 0..8 {
        let bx = hash2(b, 3.1) * 340 + 10;
        let by = 640 - ((t * (32 + b * 7) + b * 85) % 660);
        stroke_circle(bx, by, 4 + (b % 4) * 2, rgba(220, 255, 255, 0.55), 1.3);
    }
    round_rect(42, 46, 276, 32, 16, rgba(4, 18, 28, 0.62));
    text(180, 67, "VORONOI REEF · " + str(cells) + " CELLS", 13, rgb(150, 255, 235), "center");
}
"#;

const ANIM_SYNTHWAVE_HORIZON: &str = r#"
fn on_init(state) {
    pixel_init(96, 160);
    state.hue = 315;
    state.speed = 1.6;
    state.turbo = 0;
}

fn on_update(state, dt, input) {
    if input.pointer_active {
        state.hue = clamp(input.mouse_x / 360.0, 0, 1) * 360.0;
        state.speed = 0.6 + (1.0 - clamp(input.mouse_y / 640.0, 0, 1)) * 3.4;
    }
    if input.clicked {
        state.turbo = 1.0;
        tone(220, 0.1, "saw", 0.5);
        tone(440, 0.14, "saw", 0.45);
    }
    state.turbo = max(0, state.turbo - dt * 1.4);
}

fn on_render(state, t, canvas) {
    let eff_speed = state.speed + state.turbo * 3.5;
    shader_synthwave(t, eff_speed, state.hue);

    // Silhouette mountain range along horizon
    polygon([
        [0, 358], [46, 294], [98, 338], [154, 278],
        [210, 330], [268, 286], [322, 334], [360, 304], [360, 362], [0, 362]
    ], rgba(18, 8, 36, 0.92));

    // Twinkling retro stars above sunset
    for s in 0..10 {
        let sx = 24 + s * 34;
        let sy = 88 + (s % 3) * 36;
        star(sx, sy, 4 + (s % 2) * 2, 1.8, 4, rgba(255, 225, 255, 0.7 + 0.3 * sin(t * 4 + s)), t);
    }

    if state.turbo > 0.05 {
        stroke_rect(8, 8, 344, 624, rgba(255, 120, 235, state.turbo * 0.8), 4.0);
    }
    round_rect(42, 44, 276, 32, 16, rgba(16, 6, 32, 0.68));
    text(180, 65, "OUTRUN SYNTHWAVE · " + str(floor(eff_speed * 60)) + " KM/H", 13, rgb(255, 165, 240), "center");
}
"#;

const ANIM_FRACTAL_SAKURA: &str = r#"
fn on_init(state) {
    state.wind = 0.08;
    state.shake = 0.0;
    let petals = [];
    for i in 0..24 {
        push(petals, {
            x: rand_range(40, 320),
            y: rand_range(120, 560),
            vx: rand_range(-18, 28),
            vy: rand_range(24, 58),
            sz: rand_range(3.0, 5.5),
            ph: rand_range(0, TAU)
        });
    }
    state.petals = petals;
}

fn draw_branch(x, y, length, angle, depth, wind) {
    let x2 = x + cos(angle) * length;
    let y2 = y + sin(angle) * length;
    let sw = max(1.1, depth * 1.35);
    let col = lerp_color(rgb(82, 48, 62), rgb(146, 92, 108), (7.0 - depth) / 7.0);
    line(x, y, x2, y2, col, sw);
    if depth <= 1 {
        glow_circle(x2, y2, 6.5, rgba(255, 175, 212, 0.82), 1.1);
        circle(x2, y2, 3.2, rgb(255, 228, 240));
        return;
    }
    let next_len = length * 0.72;
    draw_branch(x2, y2, next_len, angle - 0.36 + wind, depth - 1, wind * 1.12);
    draw_branch(x2, y2, next_len, angle + 0.34 + wind, depth - 1, wind * 1.12);
}

fn on_update(state, dt, input) {
    let target_wind = 0.06;
    if input.pointer_active {
        target_wind = (input.mouse_x / 360.0 - 0.5) * 0.42;
    }
    state.wind = lerp(state.wind, target_wind, clamp(dt * 4.5, 0, 1));
    if input.clicked {
        state.shake = 1.0;
        tone(528, 0.16, "sine", 0.45);
    }
    state.shake = max(0, state.shake - dt * 1.8);

    for i in 0..len(state.petals) {
        let p = state.petals[i];
        p.x += (p.vx + state.wind * 130 + sin(p.ph) * 20) * dt;
        p.y += (p.vy + state.shake * 45) * dt;
        p.ph += dt * 3.0;
        if p.y > 635 || p.x < -10 || p.x > 370 {
            p.x = rand_range(65, 295);
            p.y = rand_range(160, 340);
        }
    }
}

fn on_render(state, t, canvas) {
    clear_grad(rgb(20, 18, 42), rgb(64, 38, 72), 180);
    glow_circle(280, 138, 46, rgba(255, 220, 232, 0.55), 1.5);
    circle(280, 138, 26, rgb(255, 240, 246));

    let gust = state.wind + noise2d(t * 0.7, 1.3) * 0.14 + sin(t * 18) * state.shake * 0.12;
    draw_branch(180, 588, 108, -PI * 0.5 + gust * 0.25, 6, gust * 0.45);

    for i in 0..len(state.petals) {
        let p = state.petals[i];
        circle(p.x, p.y, p.sz, rgba(255, 192, 222, 0.88));
    }
    rect(0, 586, 360, 54, rgb(26, 18, 38));
    round_rect(48, 44, 264, 32, 16, rgba(16, 12, 32, 0.65));
    text(180, 65, "FRACTAL SAKURA · WIND ART", 13, rgb(255, 208, 230), "center");
}
"#;
