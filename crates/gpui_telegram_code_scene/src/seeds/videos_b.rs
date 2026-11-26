//! Code Videos V4–V6 (pure-timeline O(1) deterministic seek).

use gpui_telegram_proto::{AssetKind, CodeSceneAssetDto};

use crate::seed_library::{
    OFFICIAL_ARCADE_FORGE_BOT_ID, OFFICIAL_FABUSHI_BOT_ID, OFFICIAL_PHYSICS_LAB_BOT_ID, SeedSpec,
    build_seed_asset,
};

const BASE_TS: i64 = 1_760_000_000_000;

pub fn assets() -> Vec<CodeSceneAssetDto> {
    vec![
        build_seed_asset(SeedSpec {
            scene_id: "seed_video_solar_orbit",
            title: "太阳系轨道巡礼 · Voyagers of the Solar Orbit",
            description: "开普勒轨道动力学短片：镜头从内行星拉远至土星光环与旅行者号飞掠轨迹。",
            kind: AssetKind::Video,
            bot_id: OFFICIAL_PHYSICS_LAB_BOT_ID,
            tags: &["space", "astronomy", "physics", "science", "keyframe"],
            duration_sec: Some(20.0),
            seed: 1004,
            controls_hint: "Space 播放/暂停 · 拖动进度条跳转 · 0.5x–2x 倍速",
            plays_count: 340,
            likes_count: 68,
            bookmarks_count: 29,
            comments_count: 12,
            shares_count: 15,
            remixes_count: 6,
            created_at_ms: BASE_TS - 86_400_000 * 5,
            source_code: VIDEO_SOLAR_ORBIT,
        }),
        build_seed_asset(SeedSpec {
            scene_id: "seed_video_starfighter_intro",
            title: "星际战机出击预告 · Starfighter Launch Trailer",
            description: "曲速星场着色器与战机矢量出击预告：机库倒计时、超光速跃迁、终极激光扫射。",
            kind: AssetKind::Video,
            bot_id: OFFICIAL_ARCADE_FORGE_BOT_ID,
            tags: &["scifi", "arcade", "space", "retro", "trailer"],
            duration_sec: Some(12.0),
            seed: 1005,
            controls_hint: "Space 播放/暂停 · 拖动进度条跳转 · 0.5x–2x 倍速",
            plays_count: 72,
            likes_count: 19,
            bookmarks_count: 8,
            comments_count: 4,
            shares_count: 5,
            remixes_count: 3,
            created_at_ms: BASE_TS - 86_400_000 * 2,
            source_code: VIDEO_STARFIGHTER_INTRO,
        }),
        build_seed_asset(SeedSpec {
            scene_id: "seed_video_ai_genesis",
            title: "AI 智能体诞生记 · Genesis of an AI Agent",
            description: "由 @fabushi_ai_bot 创作的神经脉冲起源短片：突触觉醒、字节码编译、灵感发布。",
            kind: AssetKind::Video,
            bot_id: OFFICIAL_FABUSHI_BOT_ID,
            tags: &["ai", "neural", "generative", "tech", "story"],
            duration_sec: Some(18.0),
            seed: 1006,
            controls_hint: "Space 播放/暂停 · 拖动进度条跳转 · 0.5x–2x 倍速",
            // Tier0 Cold-Start (< 20 plays)
            plays_count: 12,
            likes_count: 4,
            bookmarks_count: 2,
            comments_count: 1,
            shares_count: 1,
            remixes_count: 0,
            created_at_ms: BASE_TS - 3_600_000 * 5,
            source_code: VIDEO_AI_GENESIS,
        }),
    ]
}

const VIDEO_SOLAR_ORBIT: &str = r#"
fn on_init(state) {
    state.planets = [
        { r: 42, spd: 2.3, sz: 4.5, col: rgb(195, 185, 175) },
        { r: 64, spd: 1.7, sz: 6.5, col: rgb(245, 195, 120) },
        { r: 88, spd: 1.3, sz: 7.2, col: rgb(80, 175, 255) },
        { r: 114, spd: 0.95, sz: 6.0, col: rgb(235, 110, 82) },
        { r: 148, spd: 0.62, sz: 12.5, col: rgb(238, 188, 138) },
        { r: 186, spd: 0.44, sz: 10.5, col: rgb(225, 206, 152) }
    ];
}

fn on_render(state, t, canvas) {
    clear_grad(rgb(4, 6, 18), rgb(10, 16, 38), 180);

    // Background stars
    for s in 0..34 {
        let sx = hash2(s, 1.7) * 360;
        let sy = hash2(s, 8.3) * 640;
        let tw = 0.45 + 0.55 * sin(t * 2.5 + s);
        circle(sx, sy, 1.2 + (s % 2) * 0.6, rgba(220, 235, 255, tw));
    }

    let zoom = keyframe(t, [[0, 1.35], [7, 1.05, "ease_in_out"], [14, 0.82, "ease_in_out"], [20, 0.95]]);
    let cx = 180;
    let cy = 315;

    // Glowing Sun
    glow_circle(cx, cy, 28 * zoom, rgba(255, 185, 65, 0.9), 1.8);
    circle(cx, cy, 19 * zoom, rgb(255, 240, 175));

    // Planets & Kepler orbits
    for i in 0..len(state.planets) {
        let p = state.planets[i];
        let rad = p.r * zoom;
        stroke_circle(cx, cy, rad, rgba(130, 170, 230, 0.22), 1.0);
        let ang = t * p.spd + i * 1.1;
        let px = cx + cos(ang) * rad;
        let py = cy + sin(ang) * rad * 0.86;
        circle(px, py, p.sz * zoom, p.col);
        if i == 5 {
            arc(px, py, p.sz * 1.9 * zoom, -0.4, PI + 0.4, rgba(245, 224, 175, 0.8), 2.2);
        }
    }

    // Voyager hyperbolic flyby trajectory in Act 3
    if t >= 12.0 {
        let u = ease_out_cubic(clamp((t - 12.0) / 8.0, 0, 1));
        bezier(36, 520, 140, 390, 240, 250, 336, 110, rgba(95, 255, 220, 0.65), 2.0);
        let vx = lerp(36, 336, u);
        let vy = lerp(520, 110, u) - sin(u * PI) * 28;
        glow_circle(vx, vy, 7, rgb(95, 255, 220), 1.5);
        star(vx, vy, 7, 3, 4, rgb(255, 255, 255), t * 3);
    }

    let phase = "VOYAGER INTERSTELLAR FLYBY";
    if t < 7.0 { phase = "INNER TERRESTRIAL ORBITS"; } else if t < 13.5 { phase = "GAS GIANTS & SATURN RINGS"; }
    round_rect(34, 48, 292, 34, 17, rgba(8, 12, 28, 0.72));
    text(180, 70, phase, 13, rgb(165, 225, 255), "center");
    text(180, 568, "T+" + str(floor(t * 10) / 10) + "s  ZOOM " + str(floor(zoom * 100)) + "%", 12, rgba(200, 220, 255, 0.8), "center");
    rect(24, 602, 312 * clamp(t / 20.0, 0, 1), 5, rgb(255, 195, 85));
}
"#;

const VIDEO_STARFIGHTER_INTRO: &str = r#"
fn on_init(state) {
    pixel_init(96, 160);
}

fn on_render(state, t, canvas) {
    let warp = keyframe(t, [[0, 0.35], [4, 0.9], [8, 3.6, "ease_in_out"], [12, 2.2]]);
    shader_starfield(t, warp, 140);

    let ship_y = keyframe(t, [[0, 540], [4, 440, "ease_out_cubic"], [8, 360], [12, 300]]);
    let sway_amp = 6;
    if t >= 4.0 { sway_amp = 48; }
    let ship_x = 180 + sin(t * 3.1) * sway_amp;

    // Thruster flame + fighter hull polygon
    glow_circle(ship_x, ship_y + 28, 14 + sin(t * 25) * 4, rgba(80, 215, 255, 0.9), 1.6);
    polygon([
        [ship_x, ship_y - 30],
        [ship_x + 28, ship_y + 18],
        [ship_x + 9, ship_y + 12],
        [ship_x, ship_y + 24],
        [ship_x - 9, ship_y + 12],
        [ship_x - 28, ship_y + 18]
    ], rgb(225, 240, 255));
    polygon([
        [ship_x, ship_y - 16],
        [ship_x + 8, ship_y + 4],
        [ship_x - 8, ship_y + 4]
    ], rgb(65, 220, 255));

    // Act 3 boss laser sweep
    if t >= 8.0 {
        let sweep = sin((t - 8.0) * 3.2) * 140;
        glow_circle(180, 125, 26, rgba(255, 55, 110, 0.9), 1.8);
        line(180, 125, 180 + sweep, 620, rgba(255, 70, 130, 0.85), 6.0);
        line(180, 125, 180 + sweep, 620, rgb(255, 220, 235), 2.0);
        if fract(t * 3.0) < 0.08 {
            tone(520, 0.09, "square", 0.5);
        }
    } else if t < 4.0 {
        let count = 3 - floor(t);
        if count >= 1 {
            text(180, 220, "LAUNCH IN " + str(count), 28, rgb(255, 210, 80), "center");
        }
        if fract(t) < 0.06 {
            tone(340 + floor(t) * 80, 0.1, "square", 0.45);
        }
    }

    let tag = "BOSS SECTOR ALERT";
    if t < 4.0 { tag = "HANGAR IGNITION"; } else if t < 8.0 { tag = "HYPERDRIVE ENGAGED"; }
    round_rect(54, 48, 252, 34, 17, rgba(10, 14, 30, 0.72));
    text(180, 70, tag, 14, rgb(120, 240, 255), "center");
    rect(24, 602, 312 * clamp(t / 12.0, 0, 1), 5, rgb(85, 225, 255));
}
"#;

const VIDEO_AI_GENESIS: &str = r#"
fn on_init(state) {
    let nodes = [];
    for i in 0..15 {
        let layer = floor(i / 5);
        let row = i % 5;
        let nx = 72 + layer * 108;
        let ny = 155 + row * 72 + (layer % 2) * 18;
        push(nodes, { x: nx, y: ny, layer: layer });
    }
    state.nodes = nodes;
}

fn on_render(state, t, canvas) {
    clear_grad(rgb(8, 12, 28), rgb(22, 14, 46), 170);

    // Act 1 & 2 neural synapses + traveling activation packets
    let net_alpha = keyframe(t, [[0, 1.0], [11.5, 1.0], [14, 0.35], [18, 0.45]]);
    for i in 0..10 {
        let a = state.nodes[i];
        let b = state.nodes[i + 5];
        line(a.x, a.y, b.x, b.y, rgba(90, 195, 255, 0.28 * net_alpha), 1.4);
        let p = fract(t * 0.75 + i * 0.17);
        let px = lerp(a.x, b.x, p);
        let py = lerp(a.y, b.y, p);
        circle(px, py, 3.2, rgba(130, 255, 220, 0.85 * net_alpha));
    }
    for i in 0..len(state.nodes) {
        let n = state.nodes[i];
        let pulse = 0.6 + 0.4 * sin(t * 3.8 + i);
        glow_circle(n.x, n.y, 7 + pulse * 4, rgba(110, 210, 255, 0.85 * net_alpha), 1.2);
    }

    // Act 2 bytecode compilation progress bars + sacred star
    if t >= 6.0 && t < 13.0 {
        let prog = clamp((t - 6.0) / 6.0, 0, 1);
        for b in 0..5 {
            let bw = clamp(prog * 1.4 - b * 0.12, 0, 1) * 240;
            round_rect(60, 496 + b * 14, 240, 8, 4, rgba(255, 255, 255, 0.12));
            round_rect(60, 496 + b * 14, bw, 8, 4, hsla(175 + b * 24, 0.9, 0.62, 0.9));
        }
        star(180, 300, 46 * ease_out_cubic(prog), 20 * ease_out_cubic(prog), 8, rgba(255, 215, 110, 0.8), t * 1.1);
    }

    // Act 3 radiant publish burst
    if t >= 12.0 {
        let u = ease_out_elastic(clamp((t - 12.0) / 2.5, 0, 1));
        glow_circle(180, 295, 62 * u, rgba(120, 255, 215, 0.85), 1.8);
        star(180, 295, 54 * u, 24 * u, 6, rgb(255, 240, 175), t * 0.8);
        text(180, 520, "@fabushi_ai_bot", 18, rgb(140, 255, 225), "center");
    }

    if fract(t * 0.5) < 0.03 {
        tone(396 + floor(t / 6.0) * 132, 0.16, "sine", 0.45);
    }

    let head = "PHASE 3 · SCENE PUBLISHED";
    if t < 6.0 { head = "PHASE 1 · NEURAL AWAKENING"; } else if t < 12.0 { head = "PHASE 2 · COMPILING CS1"; }
    round_rect(36, 48, 288, 34, 17, rgba(8, 12, 28, 0.72));
    text(180, 70, head, 13, rgb(150, 250, 230), "center");
    rect(24, 602, 312 * clamp(t / 18.0, 0, 1), 5, rgb(120, 255, 215));
}
"#;
