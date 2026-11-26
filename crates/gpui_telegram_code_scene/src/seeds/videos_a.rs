//! Code Videos V1–V3 (pure-timeline O(1) deterministic seek).

use gpui_telegram_proto::{AssetKind, CodeSceneAssetDto};

use crate::seed_library::{
    OFFICIAL_CINEMA_CODE_BOT_ID, OFFICIAL_NEON_SHADER_BOT_ID, SeedSpec, build_seed_asset,
};

const BASE_TS: i64 = 1_760_000_000_000;

pub fn assets() -> Vec<CodeSceneAssetDto> {
    vec![
        build_seed_asset(SeedSpec {
            scene_id: "seed_video_cyber_dawn",
            title: "赛博都市黎明 · Neon Metropolis Dawn",
            description: "三幕关键帧代码短片：午夜霓虹雨幕、探照灯飞车巡航、赛博天际线破晓。",
            kind: AssetKind::Video,
            bot_id: OFFICIAL_CINEMA_CODE_BOT_ID,
            tags: &["cyberpunk", "cityscape", "keyframe", "neon", "story"],
            duration_sec: Some(16.0),
            seed: 1001,
            controls_hint: "Space 播放/暂停 · 拖动进度条跳转 · 0.5x–2x 倍速",
            plays_count: 1420,
            likes_count: 238,
            bookmarks_count: 74,
            comments_count: 31,
            shares_count: 42,
            remixes_count: 14,
            created_at_ms: BASE_TS - 86_400_000 * 6,
            source_code: VIDEO_CYBER_DAWN,
        }),
        build_seed_asset(SeedSpec {
            scene_id: "seed_video_code_poetry",
            title: "代码诗意·动态排版 · Kinetic Code Poetry",
            description: "四章节包豪斯几何与动态排版短片，使用弹性缓动演绎纯粹代码之美。",
            kind: AssetKind::Video,
            bot_id: OFFICIAL_CINEMA_CODE_BOT_ID,
            tags: &["typography", "motion_design", "minimal", "story", "art"],
            duration_sec: Some(14.0),
            seed: 1002,
            controls_hint: "Space 播放/暂停 · 拖动进度条跳转 · 0.5x–2x 倍速",
            plays_count: 280,
            likes_count: 54,
            bookmarks_count: 21,
            comments_count: 9,
            shares_count: 11,
            remixes_count: 5,
            created_at_ms: BASE_TS - 86_400_000 * 3,
            source_code: VIDEO_CODE_POETRY,
        }),
        build_seed_asset(SeedSpec {
            scene_id: "seed_video_chromatic_beat",
            title: "霓虹节拍音画秀 · Chromatic Spectrum Reel",
            description: "120 BPM 等离子着色器与 24 段谐波频谱联动，每 5 秒切换色彩情绪。",
            kind: AssetKind::Video,
            bot_id: OFFICIAL_NEON_SHADER_BOT_ID,
            tags: &["music_viz", "neon", "shader", "audio_visual", "electronic"],
            duration_sec: Some(15.0),
            seed: 1003,
            controls_hint: "Space 播放/暂停 · 拖动进度条跳转 · 0.5x–2x 倍速",
            plays_count: 780,
            likes_count: 132,
            bookmarks_count: 45,
            comments_count: 18,
            shares_count: 26,
            remixes_count: 9,
            created_at_ms: BASE_TS - 86_400_000 * 4,
            source_code: VIDEO_CHROMATIC_BEAT,
        }),
    ]
}

const VIDEO_CYBER_DAWN: &str = r#"
fn on_init(state) {
    let towers = [];
    for i in 0..10 {
        let w = 26 + floor(hash2(i, 1.3) * 28);
        let h = 130 + floor(hash2(i, 2.7) * 220);
        push(towers, { x: i * 38 - 6, w: w, h: h, hue: 190 + (i % 4) * 35 });
    }
    state.towers = towers;
}

fn on_render(state, t, canvas) {
    let dawn = keyframe(t, [[0, 0], [10.5, 0.08], [16, 1.0, "ease_in_out"]]);
    let sky_top = lerp_color(rgb(8, 10, 28), rgb(44, 24, 78), dawn);
    let sky_bot = lerp_color(rgb(18, 28, 62), rgb(255, 118, 86), dawn);
    clear_grad(sky_top, sky_bot, 180);

    // Rising cyber sun in Act 3
    if t >= 10.0 {
        let sun_u = ease_out_cubic((t - 10.0) / 6.0);
        let sun_y = lerp(520, 215, sun_u);
        glow_circle(180, sun_y, 72, rgba(255, 150, 80, 0.85), 1.6);
        circle(180, sun_y, 44, rgb(255, 224, 148));
        // Silhouette birds via bezier
        for b in 0..3 {
            let bx = 40 + ((t - 10.0) * (36 + b * 12) + b * 65) % 340;
            let by = 150 + b * 24 + sin(t * 5 + b) * 8;
            let wing = sin(t * 10 + b) * 7;
            bezier(bx - 12, by + wing, bx - 4, by - 6, bx + 4, by - 6, bx + 12, by + wing, rgb(28, 16, 40), 2.2);
        }
    }

    // Act 2 searchlights
    if t >= 5.0 && t < 12.5 {
        for s in 0..2 {
            let sx = 90 + s * 180;
            let sweep = sin(t * (1.2 + s * 0.4) + s) * 120;
            polygon([[sx, 560], [sx + sweep - 26, 40], [sx + sweep + 26, 40]], rgba(80, 230, 255, 0.14));
        }
    }

    // Parallax skyline towers & lit windows
    for i in 0..len(state.towers) {
        let tw = state.towers[i];
        let y = 580 - tw.h;
        rect(tw.x, y, tw.w, tw.h + 60, lerp_color(rgb(12, 16, 36), rgb(26, 18, 44), dawn));
        stroke_rect(tw.x, y, tw.w, tw.h, hsla(tw.hue, 0.9, 0.6, 0.5), 1.2);
        for row in 0..6 {
            let wy = y + 14 + row * 24;
            if wy < 560 {
                let blink = hash2(i * 7 + row, floor(t * 1.5));
                if blink > 0.35 {
                    let wc = hsla(tw.hue + row * 12, 0.95, 0.68, 0.85);
                    rect(tw.x + 5, wy, tw.w - 10, 7, wc);
                }
            }
        }
    }

    // Act 1 midnight neon rain
    if t < 11.5 {
        let rain_alpha = clamp((11.5 - t) / 3.0, 0.0, 1.0);
        for r in 0..22 {
            let rx = hash2(r, 9.1) * 360;
            let ry = (hash2(r, 4.4) * 640 + t * (340 + (r % 3) * 90)) % 640;
            line(rx, ry, rx - 3, ry + 16, rgba(120, 235, 255, 0.45 * rain_alpha), 1.2);
        }
    }

    // Act 2 hover traffic trails
    if t >= 5.0 {
        for c in 0..4 {
            let cx = ((t * (110 + c * 35) + c * 95) % 420) - 30;
            let cy = 370 + c * 34;
            let col = rgb(70, 240, 255);
            if c % 2 == 0 { col = rgb(255, 70, 160); }
            line(cx - 28, cy, cx, cy, col, 3.0);
            glow_circle(cx, cy, 5, col, 1.3);
        }
    }

    // Beat audio pulse every 1s
    if fract(t) < 0.04 {
        let freq = 440;
        if t < 5.0 { freq = 220; } else if t < 11.0 { freq = 330; }
        tone(freq, 0.14, "sine", 0.45);
    }

    // Chapter caption & progress ribbon
    let cap = "ACT III · CYBER DAWN";
    if t < 5.0 { cap = "ACT I · MIDNIGHT RAIN"; } else if t < 11.0 { cap = "ACT II · NEON TRAFFIC"; }
    round_rect(36, 48, 288, 34, 17, rgba(8, 12, 26, 0.72));
    text(180, 70, cap, 14, rgb(140, 245, 255), "center");
    rect(24, 608, 312, 5, rgba(255, 255, 255, 0.18));
    rect(24, 608, 312 * clamp(t / 16.0, 0.0, 1.0), 5, rgb(255, 110, 190));
}
"#;

const VIDEO_CODE_POETRY: &str = r#"
fn on_init(state) {
    state.titles = ["01 / PURE LOGIC", "02 / MOTION CURVES", "03 / ZERO PIXEL WASTE", "04 / BUILT BY CODE"];
    state.subs = ["EVERY FRAME IS MATH", "EASING SHAPES EMOTION", "VECTOR + SHADER UNITY", "@CINEMA_CODE_BOT"];
}

fn on_render(state, t, canvas) {
    let chapter = floor(clamp(t / 3.5, 0, 3.999));
    let local = t - chapter * 3.5;
    let enter = ease_out_cubic(clamp(local / 0.85, 0, 1));
    let bounce = ease_out_elastic(clamp(local / 1.25, 0, 1));

    let bg_a = hsla(chapter * 75 + 210, 0.55, 0.12, 1.0);
    let bg_b = hsla(chapter * 75 + 250, 0.65, 0.22, 1.0);
    clear_grad(bg_a, bg_b, 145);

    // Bauhaus geometric centerpiece
    let cx = 180;
    let cy = 265;
    let rot = t * 0.9 + chapter * 0.8;
    stroke_circle(cx, cy, 108 * enter, hsla(chapter * 75 + 30, 0.9, 0.68, 0.6), 2.5);
    arc(cx, cy, 86, rot, rot + PI * 1.3, rgb(255, 210, 90), 5.0);
    star(cx, cy, 52 * bounce, 24 * bounce, 5 + chapter, hsla(chapter * 75 + 160, 0.95, 0.65, 0.9), rot);

    // Kinetic Bezier ribbon
    let w = sin(t * 2.2) * 70;
    bezier(24, 420, 120, 360 + w, 240, 480 - w, 336, 420, rgba(255, 255, 255, 0.55), 3.0);

    // Kinetic headline slides in
    let tx = lerp(-120, 180, enter);
    text(tx, 108, state.titles[chapter], 22, rgb(250, 250, 255), "center");
    text(lerp(480, 180, enter), 485, state.subs[chapter], 15, hsla(chapter * 75 + 45, 0.9, 0.78, 0.95), "center");

    // 4 chapter pills
    for i in 0..4 {
        let col = rgba(255, 255, 255, 0.25);
        if i == chapter { col = rgb(255, 215, 95); }
        round_rect(64 + i * 60, 588, 48, 6, 3, col);
    }

    if local < 0.04 {
        tone(280 + chapter * 90, 0.16, "triangle", 0.5);
    }
}
"#;

const VIDEO_CHROMATIC_BEAT: &str = r#"
fn on_init(state) {
    pixel_init(96, 160);
    state.bars = 24;
}

fn on_render(state, t, canvas) {
    let pal = floor(clamp(t / 5.0, 0, 2.99));
    let scale = keyframe(t, [[0, 3.2], [5, 5.5], [10, 4.0], [15, 6.2]]);
    shader_plasma(t * 1.4, scale, pal);

    // 120 BPM beat pulse (2 beats/sec)
    let beat = fract(t * 2.0);
    let kick = pow(1.0 - beat, 3.0);
    if beat < 0.045 {
        tone(110 + pal * 55, 0.12, "saw", 0.55);
    }

    // Center pulsing shockwave rings
    glow_circle(180, 250, 34 + kick * 36, hsla(t * 45, 0.95, 0.65, 0.9), 1.7);
    stroke_circle(180, 250, 48 + beat * 115, rgba(255, 255, 255, (1.0 - beat) * 0.75), 2.5);

    // 24 harmonic spectrum equalizer bars
    let count = state.bars;
    for i in 0..count {
        let env = 0.35 + 0.65 * abs(sin(t * 4.5 + i * 0.42)) * (0.65 + 0.55 * kick);
        let bh = 18 + env * 135;
        let bx = 24 + i * 13;
        let col = hsla(i * 12 + t * 50 + pal * 90, 0.95, 0.62, 0.92);
        round_rect(bx, 540 - bh, 9, bh, 4, col);
    }

    round_rect(48, 52, 264, 34, 17, rgba(8, 10, 22, 0.65));
    text(180, 74, "CHROMATIC BEAT · 120 BPM", 14, rgb(255, 245, 255), "center");
    rect(24, 594, 312 * clamp(t / 15.0, 0, 1), 5, rgb(120, 255, 230));
}
"#;
