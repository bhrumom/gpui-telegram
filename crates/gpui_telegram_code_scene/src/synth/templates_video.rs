//! 5 pure-timeline O(1)-seek Code Video procedural templates (`CS-F08`).
//!
//! Every Video omits `on_update` so `VmInstance::seek_video(t)` executes in O(1)
//! with bit-identical frames across arbitrary seeks.

use crate::synth::intent::SceneArchetype;

/// Shared parameter bundle passed into all 15 archetype generators.
#[derive(Debug, Clone)]
pub struct TemplateParams {
    pub archetype: SceneArchetype,
    pub palette_id: String,
    pub speed_mul: f64,
    pub palette_hue: f64,
    pub accent_hue: f64,
    pub shader_pal: u8,
    pub density: f64,
    pub obstacles: i32,
    pub difficulty: f64,
    pub particles: i32,
    pub glow_level: f64,
    pub seed_phase: f64,
    pub bg_top: (u8, u8, u8),
    pub bg_bot: (u8, u8, u8),
    pub headline: String,
}

/// Escapes and truncates arbitrary prompt text for safe embedding inside a `cs1` `"..."` literal.
pub fn escape_cs1_str(raw: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for ch in raw.chars() {
        if count >= max_chars {
            break;
        }
        if ch.is_control() || ch == '"' || ch == '\\' || ch == ';' {
            continue;
        }
        out.push(ch);
        count += 1;
    }
    let trimmed = out.trim();
    if trimmed.is_empty() {
        "CODE SCENE".to_string()
    } else {
        trimmed.to_string()
    }
}

fn finite_or(v: f64, default: f64) -> f64 {
    if v.is_finite() { v } else { default }
}

/// Formats the standardized top-level `// @cs1-params` header block.
pub fn format_params_header(p: &TemplateParams) -> String {
    format!(
        "// @cs1-params archetype={arch} palette={pal}\n\
         let SPEED_MUL = {speed:.2};\n\
         let PALETTE_HUE = {hue:.1};\n\
         let ACCENT_HUE = {acc:.1};\n\
         let SHADER_PAL = {spal};\n\
         let DENSITY = {dens:.2};\n\
         let OBSTACLES = {obs};\n\
         let DIFFICULTY = {diff:.2};\n\
         let PARTICLES = {part};\n\
         let GLOW_LEVEL = {glow:.2};\n\
         let SEED_PHASE = {phase:.2};\n",
        arch = p.archetype.as_str(),
        pal = p.palette_id,
        speed = finite_or(p.speed_mul, 1.0).clamp(0.4, 2.5),
        hue = finite_or(p.palette_hue, 195.0).rem_euclid(360.0),
        acc = finite_or(p.accent_hue, 320.0).rem_euclid(360.0),
        spal = p.shader_pal % 4,
        dens = finite_or(p.density, 1.0).clamp(0.5, 2.2),
        obs = p.obstacles.clamp(1, 10),
        diff = finite_or(p.difficulty, 1.0).clamp(0.5, 2.2),
        part = p.particles.clamp(4, 48),
        glow = finite_or(p.glow_level, 1.2).clamp(0.6, 2.0),
        phase = finite_or(p.seed_phase, 0.0).rem_euclid(std::f64::consts::TAU),
    )
}

pub fn generate_video_template(p: &TemplateParams) -> String {
    let header = format_params_header(p);
    let title = escape_cs1_str(&p.headline, 36);
    let body = match p.archetype {
        SceneArchetype::VideoStarWarpReel => gen_star_warp_reel(p, &title),
        SceneArchetype::VideoCyberMetropolis => gen_cyber_metropolis(p, &title),
        SceneArchetype::VideoKineticTypography => gen_kinetic_typography(p, &title),
        SceneArchetype::VideoAudioSpectrumShow => gen_audio_spectrum_show(p, &title),
        SceneArchetype::VideoSynthwaveRoadtrip => gen_synthwave_roadtrip(p, &title),
        _ => gen_star_warp_reel(p, &title),
    };
    format!("{header}\n{body}")
}

fn gen_star_warp_reel(_p: &TemplateParams, title: &str) -> String {
    format!(
        r#"fn on_init(state) {{
    pixel_init(72, 128);
    state.last_beat = -1;
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    let dur = canvas.duration;
    if dur == null || dur <= 0 {{ dur = 14.0; }}
    let lt = (t * SPEED_MUL) % dur;
    let prog = clamp(lt / dur, 0.0, 1.0);
    let warp = keyframe(prog, [[0.0, 0.7], [0.35, 2.6, "in_out"], [0.75, 4.2, "out_cubic"], [1.0, 1.0]]);
    let stars = clamp(floor(76 * DENSITY), 36, 180);
    shader_starfield(lt + SEED_PHASE, warp, stars);
    rect(0, 0, w, h, hsla(PALETTE_HUE, 0.75, 0.14, 0.24));

    let cx = w * 0.5 + sin(lt * 1.4 + SEED_PHASE) * 28.0 * DIFFICULTY;
    let cy = h * 0.44 + cos(lt * 1.1) * 18.0;
    let rings = clamp(OBSTACLES + 2, 3, 10);
    for i in 0..rings {{
        let f = fract(lt * 0.65 + i / rings);
        let rad = 18.0 + ease_out_cubic(f) * 195.0;
        let alpha = (1.0 - f) * 0.75;
        let col = hsla((PALETTE_HUE + i * 24) % 360, 0.92, 0.64, alpha);
        stroke_circle(cx, cy, rad, col, 2.2);
    }}

    let pcnt = clamp(PARTICLES, 6, 36);
    for p in 0..pcnt {{
        let ang = SEED_PHASE + p * (TAU / pcnt) + lt * 0.9;
        let dist_r = 45.0 + fract(lt * 0.8 + hash2(p, 3.0)) * 165.0;
        let px = cx + cos(ang) * dist_r;
        let py = cy + sin(ang) * dist_r * 1.25;
        glow_circle(px, py, 3.5 + (p % 3), hsla((ACCENT_HUE + p * 7) % 360, 0.95, 0.68, 0.72), GLOW_LEVEL);
    }}

    let ship_y = h * 0.74 - keyframe(prog, [[0.0, 0], [0.5, 42, "in_out"], [1.0, 12]]);
    let ship_x = w * 0.5 + sin(lt * 2.2) * 34.0;
    let ship = [[ship_x, ship_y - 26], [ship_x + 22, ship_y + 18], [ship_x, ship_y + 8], [ship_x - 22, ship_y + 18]];
    polygon(ship, hsla(PALETTE_HUE, 0.95, 0.68, 0.95));
    glow_circle(ship_x, ship_y + 14, 14.0, hsla(ACCENT_HUE, 0.98, 0.62, 0.85), GLOW_LEVEL);
    line(ship_x, ship_y - 26, cx, cy, hsla(ACCENT_HUE, 0.95, 0.72, 0.35 + 0.3 * sin(lt * 8.0)), 2.0);

    let beat = floor(lt * 2.0);
    if beat != state.last_beat {{
        state.last_beat = beat;
        tone(220.0 + (beat % 4) * 55.0 + SHADER_PAL * 20.0, 0.08, "triangle", 0.45);
    }}

    round_rect(22, 26, w - 44, 48, 12, rgba(8, 12, 26, 0.72));
    text(w * 0.5, 56, "{title}", 16, hsla(PALETTE_HUE, 0.9, 0.85, 1.0), "center");
    rect(24, h - 22, (w - 48) * prog, 5, hsla(ACCENT_HUE, 0.95, 0.65, 0.95));
}}"#
    )
}

fn gen_cyber_metropolis(p: &TemplateParams, title: &str) -> String {
    let (r1, g1, b1) = p.bg_top;
    let (r2, g2, b2) = p.bg_bot;
    format!(
        r#"fn on_init(state) {{
    state.last_chime = -1;
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    let dur = canvas.duration;
    if dur == null || dur <= 0 {{ dur = 16.0; }}
    let lt = (t * SPEED_MUL) % dur;
    let prog = clamp(lt / dur, 0.0, 1.0);
    let dawn = keyframe(prog, [[0.0, 0.0], [0.55, 0.35, "in_out"], [0.88, 1.0, "out_cubic"], [1.0, 0.15]]);

    let sky_top = lerp_color(rgb({r1}, {g1}, {b1}), hsla(PALETTE_HUE, 0.65, 0.24, 1.0), dawn);
    let sky_bot = lerp_color(rgb({r2}, {g2}, {b2}), hsla(ACCENT_HUE, 0.82, 0.38, 1.0), dawn);
    clear_grad(sky_top, sky_bot, 180);

    let sun_y = lerp(h * 0.64, h * 0.26, dawn);
    glow_circle(w * 0.5, sun_y, 44.0 + dawn * 18.0, hsla(ACCENT_HUE, 0.95, 0.64, 0.78), GLOW_LEVEL);

    let towers = clamp(floor(8 * DENSITY), 6, 12);
    let tw = w / towers;
    let blink_t = floor(lt * 0.5);
    for i in 0..towers {{
        let seed_h = hash2(i + 1.0, SEED_PHASE + SHADER_PAL);
        let bh = 130.0 + seed_h * 220.0;
        let bx = i * tw;
        let by = h - bh;
        rect(bx + 2, by, tw - 4, bh, rgba(10, 14, 28, 0.92));
        stroke_rect(bx + 2, by, tw - 4, bh, hsla(PALETTE_HUE, 0.85, 0.55, 0.45), 1.2);
        let rows = clamp(floor(bh / 38.0), 3, 7);
        for r in 0..rows {{
            if hash2(i * 3.1, r * 5.7 + blink_t) > 0.38 {{
                let wy = by + 12.0 + r * 34.0;
                rect(bx + 7, wy, tw - 14, 9, hsla((PALETTE_HUE + r * 18) % 360, 0.95, 0.68, 0.78));
            }}
        }}
    }}

    let beams = clamp(OBSTACLES + 1, 2, 8);
    for b in 0..beams {{
        let bx = (b + 0.5) * (w / beams);
        let sway = sin(lt * (1.1 + b * 0.2) + SEED_PHASE) * 90.0 * DIFFICULTY;
        line(bx, h, bx + sway, h * 0.14, hsla(ACCENT_HUE, 0.9, 0.68, 0.28), 3.0);
    }}

    let drops = clamp(PARTICLES, 8, 42);
    for d in 0..drops {{
        let rx = hash2(d + 2.0, 9.0) * w;
        let ry = fract(hash2(d + 5.0, 3.0) + lt * (0.55 + 0.2 * (d % 3))) * h;
        line(rx, ry, rx - 3.0, ry + 14.0, hsla(PALETTE_HUE, 0.9, 0.74, 0.48), 1.4);
    }}

    let bx0 = ((lt * 70.0) % (w + 120.0)) - 60.0;
    let by0 = h * 0.34 + sin(lt * 2.5) * 26.0;
    bezier(bx0 - 30, by0 + 10, bx0 - 10, by0 - 18, bx0 + 10, by0 - 18, bx0 + 30, by0 + 8, hsla(ACCENT_HUE, 0.95, 0.72, 0.9), 2.4);

    let chime = floor(lt * 1.5);
    if chime != state.last_chime {{
        state.last_chime = chime;
        tone(196.0 + (chime % 5) * 44.0, 0.09, "sine", 0.42);
    }}

    round_rect(20, 26, w - 40, 46, 12, rgba(6, 10, 22, 0.72));
    text(w * 0.5, 55, "{title}", 16, hsla(PALETTE_HUE, 0.9, 0.86, 1.0), "center");
    rect(24, h - 20, (w - 48) * prog, 5, hsla(PALETTE_HUE, 0.95, 0.65, 0.95));
}}"#
    )
}

fn gen_kinetic_typography(p: &TemplateParams, title: &str) -> String {
    let (r1, g1, b1) = p.bg_top;
    let (r2, g2, b2) = p.bg_bot;
    format!(
        r#"fn on_init(state) {{
    state.last_chap = -1;
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    let dur = canvas.duration;
    if dur == null || dur <= 0 {{ dur = 12.0; }}
    let lt = (t * SPEED_MUL) % dur;
    let prog = clamp(lt / dur, 0.0, 1.0);
    let chap = floor(prog * 4.0);
    if chap > 3 {{ chap = 3; }}
    let local = fract(prog * 4.0);

    clear_grad(rgb({r1}, {g1}, {b1}), rgb({r2}, {g2}, {b2}), 160 + chap * 20);

    let slide = ease_out_cubic(clamp(local * 2.2, 0.0, 1.0));
    let pop = ease_out_elastic(clamp(local * 1.8, 0.0, 1.0));
    let bounce = ease_out_bounce(clamp(local * 1.6, 0.0, 1.0));

    let cx = w * 0.5;
    let cy = h * 0.44;
    let ring_r = 68.0 + pop * 48.0 * DENSITY;
    let h0 = (PALETTE_HUE + chap * 38 + SHADER_PAL * 15) % 360;
    let h1 = (ACCENT_HUE + chap * 26) % 360;

    arc(cx, cy, ring_r, lt * 1.4, lt * 1.4 + 4.2, hsla(h0, 0.92, 0.65, 0.88), 4.0);
    star(cx, cy, 24.0 + bounce * 26.0, 11.0 + bounce * 10.0, 5 + (OBSTACLES % 4), hsla(h1, 0.95, 0.68, 0.85), lt * 1.2 + SEED_PHASE);

    let ribbons = clamp(floor(PARTICLES / 5), 2, 8);
    for r in 0..ribbons {{
        let ry = h * 0.62 + r * 22.0;
        let amp = 26.0 * DIFFICULTY * sin(lt * 2.5 + r * 0.9 + SEED_PHASE);
        bezier(18, ry, w * 0.33, ry - amp, w * 0.66, ry + amp, w - 18, ry, hsla((h0 + r * 22) % 360, 0.88, 0.66, 0.72), 2.2);
    }}

    glow_circle(cx, cy, 42.0 + pop * 16.0, hsla(h0, 0.9, 0.62, 0.42), GLOW_LEVEL);

    let tx = lerp(-120.0, cx, slide);
    let label = "ACT 01 // INIT";
    if chap == 1 {{ label = "ACT 02 // RHYTHM"; }}
    else if chap == 2 {{ label = "ACT 03 // MOTION"; }}
    else if chap == 3 {{ label = "ACT 04 // FINALE"; }}

    text(tx, h * 0.20, label, 22, hsla(h1, 0.95, 0.78, 1.0), "center");
    text(cx, h * 0.27, "{title}", 16, rgba(245, 248, 255, 0.96), "center");

    for c in 0..4 {{
        let px = 42.0 + c * ((w - 84.0) / 4.0);
        let col = rgba(255, 255, 255, 0.22);
        if c == chap {{ col = hsla(h0, 0.95, 0.68, 0.95); }}
        round_rect(px, h - 54, 58, 10, 5, col);
    }}

    if chap != state.last_chap {{
        state.last_chap = chap;
        tone(262.0 + chap * 66.0, 0.10, "sine", 0.48);
    }}
    rect(24, h - 20, (w - 48) * prog, 5, hsla(h1, 0.95, 0.68, 0.95));
}}"#
    )
}

fn gen_audio_spectrum_show(_p: &TemplateParams, title: &str) -> String {
    format!(
        r#"fn on_init(state) {{
    pixel_init(72, 128);
    state.last_kick = -1;
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    let dur = canvas.duration;
    if dur == null || dur <= 0 {{ dur = 15.0; }}
    let lt = (t * SPEED_MUL) % dur;
    let prog = clamp(lt / dur, 0.0, 1.0);

    let scale = 2.6 + DENSITY * 1.4 + sin(lt * 0.9) * 0.6;
    shader_plasma(lt * 1.3 + SEED_PHASE, scale, SHADER_PAL);
    rect(0, 0, w, h, hsla(PALETTE_HUE, 0.68, 0.12, 0.42));

    let bpm_phase = fract(lt * 2.0);
    let kick = pow(1.0 - bpm_phase, 3.0);
    let cx = w * 0.5;
    let cy = h * 0.39;

    glow_circle(cx, cy, 46.0 + kick * 36.0 * DIFFICULTY, hsla(ACCENT_HUE, 0.96, 0.64, 0.72), GLOW_LEVEL);
    stroke_circle(cx, cy, 52.0 + bpm_phase * 130.0, hsla(PALETTE_HUE, 0.95, 0.70, (1.0 - bpm_phase) * 0.85), 3.0);

    let bars = clamp(16 + OBSTACLES * 2, 16, 28);
    let bw = (w - 40.0) / bars;
    for i in 0..bars {{
        let wave = abs(sin(lt * 5.2 + i * 0.55 + SEED_PHASE)) * 0.7 + abs(cos(lt * 3.1 - i * 0.35)) * 0.3;
        let bh = 18.0 + (wave * 110.0 + kick * 52.0) * DENSITY;
        let bx = 20.0 + i * bw;
        let col = hsla((PALETTE_HUE + i * 9) % 360, 0.95, 0.64, 0.92);
        round_rect(bx + 1.5, h - 78.0 - bh, bw - 3.0, bh, 3, col);
    }}

    let sparks = clamp(PARTICLES, 6, 32);
    for s in 0..sparks {{
        let a = SEED_PHASE + s * (TAU / sparks) + lt * 1.6;
        let rr = 66.0 + kick * 42.0 + sin(lt * 4.0 + s) * 14.0;
        circle(cx + cos(a) * rr, cy + sin(a) * rr, 3.2, hsla((ACCENT_HUE + s * 11) % 360, 0.95, 0.72, 0.9));
    }}

    let beat_idx = floor(lt * 2.0);
    if beat_idx != state.last_kick {{
        state.last_kick = beat_idx;
        tone(110.0 + (beat_idx % 4) * 32.0, 0.09, "saw", 0.45);
    }}

    round_rect(20, 24, w - 40, 44, 12, rgba(6, 10, 22, 0.72));
    text(cx, 52, "{title}", 16, hsla(PALETTE_HUE, 0.92, 0.86, 1.0), "center");
    rect(24, h - 22, (w - 48) * prog, 5, hsla(ACCENT_HUE, 0.95, 0.65, 0.95));
}}"#
    )
}

fn gen_synthwave_roadtrip(_p: &TemplateParams, title: &str) -> String {
    format!(
        r#"fn on_init(state) {{
    pixel_init(72, 128);
    state.last_note = -1;
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    let dur = canvas.duration;
    if dur == null || dur <= 0 {{ dur = 15.0; }}
    let lt = (t * SPEED_MUL) % dur;
    let prog = clamp(lt / dur, 0.0, 1.0);
    let cruise = keyframe(prog, [[0.0, 1.1], [0.5, 2.4, "in_out"], [0.85, 3.2, "out_cubic"], [1.0, 1.3]]);

    shader_synthwave(lt + SEED_PHASE, cruise * DENSITY, PALETTE_HUE);

    let stars_n = clamp(PARTICLES, 6, 28);
    for s in 0..stars_n {{
        let sx = hash2(s + 1.0, 7.0) * w;
        let sy = 36.0 + hash2(s + 3.0, 11.0) * (h * 0.28);
        let tw = 0.5 + 0.5 * sin(lt * 4.5 + s);
        star(sx, sy, 3.5 + tw * 2.5, 1.4, 4, hsla(ACCENT_HUE, 0.92, 0.78, 0.75), lt * 0.8);
    }}

    let hy = h * 0.48;
    let mtns = [[0, hy], [w * 0.18, hy - 54], [w * 0.36, hy - 18], [w * 0.56, hy - 68], [w * 0.78, hy - 28], [w, hy - 48], [w, hy]];
    polygon(mtns, rgba(16, 10, 36, 0.88));

    let pylons = clamp(OBSTACLES + 2, 3, 9);
    for k in 0..pylons {{
        let pf = fract(lt * 0.9 + k / pylons);
        let py = hy + pf * pf * (h - hy);
        let spread = 24.0 + pf * (w * 0.44);
        glow_circle(w * 0.5 - spread, py, 3.0 + pf * 5.0, hsla(PALETTE_HUE, 0.95, 0.65, 0.85), GLOW_LEVEL);
        glow_circle(w * 0.5 + spread, py, 3.0 + pf * 5.0, hsla(ACCENT_HUE, 0.95, 0.65, 0.85), GLOW_LEVEL);
    }}

    let car_x = w * 0.5 + sin(lt * 1.8) * 32.0 * DIFFICULTY;
    let car_y = h * 0.80;
    round_rect(car_x - 36, car_y - 16, 72, 26, 8, rgba(18, 12, 34, 0.95));
    stroke_rect(car_x - 36, car_y - 16, 72, 26, hsla(PALETTE_HUE, 0.95, 0.68, 0.9), 2.0);
    glow_circle(car_x - 22, car_y - 3, 7.0, hsla(ACCENT_HUE, 0.98, 0.62, 0.95), GLOW_LEVEL);
    glow_circle(car_x + 22, car_y - 3, 7.0, hsla(ACCENT_HUE, 0.98, 0.62, 0.95), GLOW_LEVEL);

    let note = floor(lt * 2.0);
    if note != state.last_note {{
        state.last_note = note;
        tone(165.0 + (note % 4) * 40.0 + SHADER_PAL * 12.0, 0.08, "triangle", 0.44);
    }}

    round_rect(20, 24, w - 40, 44, 12, rgba(10, 6, 24, 0.72));
    text(w * 0.5, 52, "{title}", 16, hsla(ACCENT_HUE, 0.94, 0.84, 1.0), "center");
    let kmh = floor(120 + cruise * 55);
    text(w - 28, h - 34, str(kmh) + " KM/H", 13, hsla(PALETTE_HUE, 0.95, 0.78, 0.95), "right");
    rect(24, h - 20, (w - 48) * prog, 5, hsla(PALETTE_HUE, 0.95, 0.66, 0.95));
}}"#
    )
}
