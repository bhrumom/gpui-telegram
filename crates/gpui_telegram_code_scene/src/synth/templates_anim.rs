//! 5 pointer- & click-reactive 60fps Code Animation procedural templates (`CS-F08`).

use crate::synth::intent::SceneArchetype;
use crate::synth::templates_video::{TemplateParams, escape_cs1_str, format_params_header};

pub fn generate_anim_template(p: &TemplateParams) -> String {
    let header = format_params_header(p);
    let title = escape_cs1_str(&p.headline, 36);
    let body = match p.archetype {
        SceneArchetype::AnimFluidPlasma => gen_fluid_plasma(p, &title),
        SceneArchetype::AnimParticleFlowField => gen_particle_flow_field(p, &title),
        SceneArchetype::AnimFractalMandala => gen_fractal_mandala(p, &title),
        SceneArchetype::AnimBiolumVoronoi => gen_biolum_voronoi(p, &title),
        SceneArchetype::AnimHarmonicWaves => gen_harmonic_waves(p, &title),
        _ => gen_fluid_plasma(p, &title),
    };
    format!("{header}\n{body}")
}

fn gen_fluid_plasma(_p: &TemplateParams, title: &str) -> String {
    format!(
        r#"fn on_init(state) {{
    pixel_init(72, 128);
    state.scale = 3.0 * DENSITY;
    state.phase = SEED_PHASE;
    state.pal = SHADER_PAL;
    state.ripples = [];
}}

fn on_update(state, dt, input) {{
    let step = min(dt * SPEED_MUL, 0.06);
    state.phase += step * 1.15;
    if input.pointer_active {{
        let target_s = clamp(1.8 + (input.mouse_y / 640.0) * 4.5 * DENSITY, 1.5, 7.5);
        state.scale = lerp(state.scale, target_s, 0.14);
        state.phase += (input.mouse_x / 360.0 - 0.5) * step * 2.0;
    }}
    if input.clicked || input.action {{
        state.pal = (state.pal + 1) % 4;
        if len(state.ripples) < 14 {{
            let rx = input.mouse_x;
            let ry = input.mouse_y;
            if !input.pointer_active {{ rx = 180.0; ry = 320.0; }}
            push(state.ripples, {{ x: rx, y: ry, r: 10.0, life: 1.0 }});
        }}
        tone(330.0 + state.pal * 70.0, 0.08, "sine", 0.45);
    }}
    let next_r = [];
    for rp in state.ripples {{
        rp.r += step * 190.0 * DIFFICULTY;
        rp.life -= step * 1.3;
        if rp.life > 0.02 {{ push(next_r, rp); }}
    }}
    state.ripples = next_r;
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    shader_plasma(state.phase, state.scale, state.pal);
    rect(0, 0, w, h, hsla(PALETTE_HUE, 0.78, 0.20, 0.26));

    let orbs = clamp(PARTICLES, 6, 36);
    for i in 0..orbs {{
        let ang = state.phase * 0.7 + i * (TAU / orbs);
        let rad = 52.0 + 78.0 * sin(state.phase * 0.6 + i * 0.9);
        let ox = w * 0.5 + cos(ang) * rad;
        let oy = h * 0.48 + sin(ang * 1.2) * rad * 1.25;
        glow_circle(ox, oy, 5.0 + (i % 4), hsla((PALETTE_HUE + i * 11) % 360, 0.95, 0.68, 0.72), GLOW_LEVEL);
    }}

    let rings = clamp(OBSTACLES, 1, 8);
    for k in 0..rings {{
        let rr = 48.0 + k * 28.0 + sin(t * SPEED_MUL + k) * 10.0;
        stroke_circle(w * 0.5, h * 0.48, rr, hsla(ACCENT_HUE, 0.9, 0.72, 0.25), 1.6);
    }}

    for rp in state.ripples {{
        stroke_circle(rp.x, rp.y, rp.r, hsla(ACCENT_HUE, 0.95, 0.72, rp.life), 2.8);
    }}

    round_rect(20, 24, w - 40, 44, 12, rgba(8, 10, 24, 0.68));
    text(w * 0.5, 52, "{title}", 16, hsla(PALETTE_HUE, 0.9, 0.88, 1.0), "center");
}}"#
    )
}

fn gen_particle_flow_field(p: &TemplateParams, title: &str) -> String {
    let (r1, g1, b1) = p.bg_top;
    let (r2, g2, b2) = p.bg_bot;
    format!(
        r#"fn on_init(state) {{
    state.pts = [];
    let cnt = clamp(floor(16 + PARTICLES * DENSITY * 0.75), 18, 44);
    for i in 0..cnt {{
        push(state.pts, {{
            x: rand_range(20, 340),
            y: rand_range(60, 580),
            vx: rand_range(-30, 30),
            vy: rand_range(-30, 30),
            hue: (PALETTE_HUE + i * 7) % 360
        }});
    }}
    state.pulse = 0.0;
    state.px = 180.0;
    state.py = 320.0;
}}

fn on_update(state, dt, input) {{
    let step = min(dt * SPEED_MUL, 0.05);
    state.pulse = max(0.0, state.pulse - step * 2.2);
    if input.pointer_active {{
        state.px = input.mouse_x;
        state.py = input.mouse_y;
    }}
    if input.clicked || input.action {{
        state.pulse = 1.0;
        tone(420.0 + SHADER_PAL * 60.0, 0.08, "triangle", 0.48);
    }}
    let t_seed = SEED_PHASE + state.pulse * 2.0;
    for pt in state.pts {{
        let n = fbm2d(pt.x * 0.008 + t_seed, pt.y * 0.008, 3);
        let ang = n * TAU * 1.4;
        pt.vx = lerp(pt.vx, cos(ang) * 95.0 * DIFFICULTY, 0.12);
        pt.vy = lerp(pt.vy, sin(ang) * 95.0 * DIFFICULTY, 0.12);

        let dx = state.px - pt.x;
        let dy = state.py - pt.y;
        let d = max(16.0, hypot(dx, dy));
        if d < 180.0 {{
            let force = 110.0;
            if state.pulse > 0.25 {{ force = -280.0; }}
            pt.vx += (dx / d) * force * step;
            pt.vy += (dy / d) * force * step;
        }}

        pt.x += pt.vx * step;
        pt.y += pt.vy * step;
        if pt.x < 8 {{ pt.x = 352; }} else if pt.x > 352 {{ pt.x = 8; }}
        if pt.y < 8 {{ pt.y = 632; }} else if pt.y > 632 {{ pt.y = 8; }}
    }}
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    clear_grad(rgb({r1}, {g1}, {b1}), rgb({r2}, {g2}, {b2}), 180);

    let anchors = clamp(OBSTACLES + 1, 2, 8);
    for a in 0..anchors {{
        let ax = w * 0.5 + cos(t * 0.6 * SPEED_MUL + a * (TAU / anchors)) * 110.0;
        let ay = h * 0.5 + sin(t * 0.8 * SPEED_MUL + a * 1.4) * 165.0;
        stroke_circle(ax, ay, 18.0 + state.pulse * 26.0, hsla(ACCENT_HUE, 0.9, 0.65, 0.35), 1.6);
    }}

    for pt in state.pts {{
        let col = hsla(pt.hue, 0.95, 0.68, 0.88);
        line(pt.x, pt.y, pt.x - pt.vx * 0.08, pt.y - pt.vy * 0.08, col, 2.2);
        glow_circle(pt.x, pt.y, 4.5, col, GLOW_LEVEL);
        circle(pt.x, pt.y, 2.2, rgba(255, 255, 255, 0.92));
    }}

    if state.pulse > 0.02 {{
        stroke_circle(state.px, state.py, (1.0 - state.pulse) * 160.0 + 14.0, hsla(ACCENT_HUE, 0.98, 0.72, state.pulse), 3.0);
    }}

    round_rect(20, 24, w - 40, 44, 12, rgba(8, 12, 26, 0.70));
    text(w * 0.5, 52, "{title}", 16, hsla(PALETTE_HUE, 0.92, 0.86, 1.0), "center");
}}"#
    )
}

fn gen_fractal_mandala(_p: &TemplateParams, title: &str) -> String {
    format!(
        r#"fn on_init(state) {{
    pixel_init(64, 112);
    state.landmark = SHADER_PAL % 3;
    state.zoom_t = SEED_PHASE;
    state.fx = 0.0;
    state.fy = 0.0;
}}

fn on_update(state, dt, input) {{
    let step = min(dt * SPEED_MUL, 0.06);
    state.zoom_t += step * 0.55;
    if input.pointer_active {{
        state.fx = lerp(state.fx, (input.mouse_x / 360.0 - 0.5) * 0.22, 0.15);
        state.fy = lerp(state.fy, (input.mouse_y / 640.0 - 0.5) * 0.22, 0.15);
    }}
    if input.clicked || input.action {{
        state.landmark = (state.landmark + 1) % 3;
        tone(294.0 + state.landmark * 82.0, 0.09, "sine", 0.45);
    }}
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    let cx = -0.745 + state.fx;
    let cy = 0.112 + state.fy;
    if state.landmark == 1 {{
        cx = -0.101 + state.fx;
        cy = 0.956 + state.fy;
    }} else if state.landmark == 2 {{
        cx = -0.525 + state.fx;
        cy = -0.525 + state.fy;
    }}
    let zoom = 1.6 + (1.0 + sin(state.zoom_t)) * 3.8 * DENSITY;
    shader_mandelbrot(cx, cy, zoom, 20);
    rect(0, 0, w, h, hsla(PALETTE_HUE, 0.72, 0.12, 0.28));

    let mx = w * 0.5;
    let my = h * 0.50;
    let layers = clamp(OBSTACLES + 2, 3, 7);
    for l in 0..layers {{
        let rad = 36.0 + l * 26.0;
        let rot = t * SPEED_MUL * (0.4 + l * 0.15) * DIFFICULTY;
        if l % 2 == 1 {{ rot = -rot; }}
        let hue = (PALETTE_HUE + l * 32) % 360;
        arc(mx, my, rad, rot, rot + 4.4, hsla(hue, 0.95, 0.68, 0.65), 2.2);
        star(mx, my, rad * 0.55, rad * 0.25, 6 + (l % 4), hsla((ACCENT_HUE + l * 24) % 360, 0.92, 0.66, 0.36), rot);
    }}

    let pts = clamp(PARTICLES, 6, 28);
    for p in 0..pts {{
        let a = p * (TAU / pts) + t * SPEED_MUL * 0.6 + SEED_PHASE;
        let r = 82.0 + 42.0 * sin(t * 1.4 + p);
        glow_circle(mx + cos(a) * r, my + sin(a) * r, 3.8, hsla((ACCENT_HUE + p * 9) % 360, 0.95, 0.72, 0.78), GLOW_LEVEL);
    }}

    round_rect(20, 24, w - 40, 44, 12, rgba(8, 8, 22, 0.72));
    text(mx, 52, "{title}", 16, hsla(PALETTE_HUE, 0.92, 0.88, 1.0), "center");
}}"#
    )
}

fn gen_biolum_voronoi(_p: &TemplateParams, title: &str) -> String {
    format!(
        r#"fn on_init(state) {{
    pixel_init(64, 112);
    state.pal = SHADER_PAL;
    state.jx = 180.0;
    state.jy = 320.0;
    state.mode = 0;
    state.bubbles = [];
    let cnt = clamp(PARTICLES, 8, 32);
    for i in 0..cnt {{
        push(state.bubbles, {{
            x: rand_range(24, 336),
            y: rand_range(40, 620),
            r: rand_range(2.5, 6.5),
            spd: rand_range(28, 72)
        }});
    }}
}}

fn on_update(state, dt, input) {{
    let step = min(dt * SPEED_MUL, 0.06);
    if input.pointer_active {{
        state.jx = lerp(state.jx, input.mouse_x, 0.10);
        state.jy = lerp(state.jy, input.mouse_y, 0.10);
    }} else {{
        state.jx = lerp(state.jx, 180.0 + sin(SEED_PHASE + state.mode) * 60.0, 0.04);
        state.jy = lerp(state.jy, 300.0, 0.04);
    }}
    if input.clicked || input.action {{
        state.mode = (state.mode + 1) % 3;
        state.pal = (state.pal + 1) % 4;
        tone(260.0 + state.mode * 90.0, 0.09, "sine", 0.44);
    }}
    for b in state.bubbles {{
        b.y -= b.spd * step * DIFFICULTY;
        b.x += sin(b.y * 0.03 + SEED_PHASE) * 14.0 * step;
        if b.y < -12 {{
            b.y = 652;
            b.x = rand_range(24, 336);
        }}
    }}
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    let cells = clamp(5 + floor(DENSITY * 4.0) + state.mode, 5, 12);
    shader_voronoi(t * SPEED_MUL * 0.85 + SEED_PHASE, cells, state.pal);
    rect(0, 0, w, h, hsla(PALETTE_HUE, 0.78, 0.12, 0.30));

    let jx = state.jx + sin(t * 1.4 * SPEED_MUL) * 16.0;
    let jy = state.jy + cos(t * 1.1 * SPEED_MUL) * 14.0;
    let tent = clamp(OBSTACLES + 3, 4, 9);
    for k in 0..tent {{
        let ox = (k - (tent - 1) * 0.5) * 14.0;
        let sway = sin(t * 3.0 * SPEED_MUL + k * 0.8) * 26.0;
        bezier(
            jx + ox * 0.7, jy + 14.0,
            jx + ox + sway * 0.5, jy + 62.0,
            jx + ox - sway, jy + 112.0,
            jx + ox + sway * 0.8, jy + 158.0,
            hsla((ACCENT_HUE + k * 14) % 360, 0.94, 0.68, 0.78),
            2.4
        );
    }}

    glow_circle(jx, jy, 44.0, hsla(PALETTE_HUE, 0.96, 0.66, 0.76), GLOW_LEVEL);
    stroke_circle(jx, jy, 34.0 + sin(t * 3.0) * 5.0, hsla(ACCENT_HUE, 0.95, 0.78, 0.82), 2.4);

    for b in state.bubbles {{
        stroke_circle(b.x, b.y, b.r, hsla(PALETTE_HUE, 0.9, 0.8, 0.68), 1.4);
    }}

    round_rect(20, 24, w - 40, 44, 12, rgba(4, 14, 28, 0.70));
    text(w * 0.5, 52, "{title}", 16, hsla(PALETTE_HUE, 0.92, 0.88, 1.0), "center");
}}"#
    )
}

fn gen_harmonic_waves(p: &TemplateParams, title: &str) -> String {
    let (r1, g1, b1) = p.bg_top;
    let (r2, g2, b2) = p.bg_bot;
    format!(
        r#"fn on_init(state) {{
    state.freq = 2.0 * DENSITY;
    state.amp = 52.0;
    state.pulses = [];
}}

fn on_update(state, dt, input) {{
    let step = min(dt * SPEED_MUL, 0.06);
    if input.pointer_active {{
        state.freq = lerp(state.freq, 1.2 + (input.mouse_x / 360.0) * 4.0 * DENSITY, 0.14);
        state.amp = lerp(state.amp, 24.0 + (input.mouse_y / 640.0) * 95.0, 0.14);
    }}
    if input.clicked || input.action {{
        if len(state.pulses) < 12 {{
            let px = 180.0;
            let py = 320.0;
            if input.pointer_active {{ px = input.mouse_x; py = input.mouse_y; }}
            push(state.pulses, {{ x: px, y: py, r: 8.0, life: 1.0 }});
        }}
        tone(340.0 + SHADER_PAL * 55.0, 0.08, "triangle", 0.46);
    }}
    let kept = [];
    for pl in state.pulses {{
        pl.r += step * 210.0 * DIFFICULTY;
        pl.life -= step * 1.4;
        if pl.life > 0.02 {{ push(kept, pl); }}
    }}
    state.pulses = kept;
}}

fn on_render(state, t, canvas) {{
    let w = canvas.width;
    let h = canvas.height;
    let lt = t * SPEED_MUL + SEED_PHASE;
    clear_grad(rgb({r1}, {g1}, {b1}), rgb({r2}, {g2}, {b2}), 180);

    let cols = clamp(OBSTACLES + 5, 6, 14);
    let cw = w / cols;
    for c in 0..cols {{
        let drop_y = fract(hash2(c + 1.0, 4.0) + lt * (0.28 + 0.08 * (c % 3))) * h;
        for seg in 0..6 {{
            let sy = drop_y - seg * 16.0;
            if sy >= 0 && sy <= h {{
                let alpha = (1.0 - seg / 6.0) * 0.42;
                rect(c * cw + cw * 0.35, sy, 5, 11, hsla(PALETTE_HUE, 0.9, 0.62, alpha));
            }}
        }}
    }}

    for wave in 0..3 {{
        let pts = [];
        let base_y = h * (0.34 + wave * 0.16);
        let hue = (PALETTE_HUE + wave * 42) % 360;
        for s in 0..32 {{
            let u = s / 31.0;
            let x = 14.0 + u * (w - 28.0);
            let y = base_y + sin(u * TAU * state.freq + lt * (1.8 + wave * 0.5)) * state.amp * (0.65 + wave * 0.2);
            push(pts, [x, y]);
        }}
        polyline(pts, hsla(hue, 0.95, 0.68, 0.90), 3.0 - wave * 0.4);
    }}

    let sparks = clamp(PARTICLES, 6, 32);
    for p in 0..sparks {{
        let u = p / sparks;
        let sx = 18.0 + u * (w - 36.0);
        let sy = h * 0.50 + sin(u * TAU * state.freq + lt * 2.3) * state.amp;
        glow_circle(sx, sy, 4.2, hsla((ACCENT_HUE + p * 8) % 360, 0.95, 0.72, 0.85), GLOW_LEVEL);
    }}

    for pl in state.pulses {{
        stroke_circle(pl.x, pl.y, pl.r, hsla(ACCENT_HUE, 0.96, 0.74, pl.life), 2.6);
    }}

    round_rect(20, 24, w - 40, 44, 12, rgba(6, 12, 18, 0.72));
    text(w * 0.5, 52, "{title}", 16, hsla(PALETTE_HUE, 0.92, 0.86, 1.0), "center");
}}"#
    )
}
