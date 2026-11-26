//! Live LLM Prompt Builder, Verified Few-Shot `cs1` Examples, Compiler-Diagnostic
//! Self-Repair Prompt Builder, and Resilient `LlmResponseParser` (`CS-F08`).

use gpui_telegram_proto::{
    AssetKind, AssetManifest, CODE_SCENE_FORMAT_CS1, ValidationReportDto,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::lang::compile_source;
use crate::lang::limits::VmBudgets;
use crate::model::CompileDiagnostic;
use crate::reco::embedding::normalize_tags;
use crate::runtime::validate_to_report;
use crate::synth::intent::{IntentAnalyzer, SceneArchetype};
use crate::synth::{SynthRequest, SynthesizedDraft};

/// Verified compact pure-timeline Code Video few-shot example (omits `on_update` for O(1) seek).
pub const FEW_SHOT_VIDEO_CS1: &str = r#"fn on_init(state) {
    state.last_beat = -1;
}
fn on_render(state, t, canvas) {
    let w = canvas.width;
    let h = canvas.height;
    let dur = canvas.duration;
    if dur == null || dur <= 0 { dur = 12.0; }
    let prog = clamp((t % dur) / dur, 0.0, 1.0);
    clear_grad(rgb(8, 12, 30), rgb(28, 12, 54), 180);
    let y = keyframe(prog, [[0.0, 140], [0.5, 360, "in_out"], [1.0, 140]]);
    glow_circle(w * 0.5, y, 42, hsla((190 + prog * 140) % 360, 0.95, 0.65, 0.8), 1.3);
    star(w * 0.5, y, 26, 12, 5, hsla(320, 0.95, 0.72, 0.95), t * 1.6);
    text(w * 0.5, 58, "NEON PULSE REEL", 18, rgba(245, 248, 255, 0.96), "center");
    rect(24, h - 22, (w - 48) * prog, 5, hsla(190, 0.95, 0.65, 0.95));
    let beat = floor(t * 2.0);
    if beat != state.last_beat {
        state.last_beat = beat;
        tone(220 + (beat % 4) * 55, 0.08, "triangle", 0.4);
    }
}"#;

/// Verified compact pointer-reactive Code Animation few-shot example.
pub const FEW_SHOT_ANIMATION_CS1: &str = r#"fn on_init(state) {
    pixel_init(72, 128);
    state.pal = 0;
    state.scale = 3.2;
    state.ripples = [];
}
fn on_update(state, dt, input) {
    let step = min(dt, 0.05);
    if input.pointer_active {
        state.scale = lerp(state.scale, 2.0 + (input.mouse_y / 640.0) * 4.0, 0.15);
    }
    if input.clicked || input.action {
        state.pal = (state.pal + 1) % 4;
        if len(state.ripples) < 12 {
            push(state.ripples, { x: input.mouse_x, y: input.mouse_y, r: 8.0, life: 1.0 });
        }
        tone(360 + state.pal * 60, 0.07, "sine", 0.4);
    }
    let next = [];
    for rp in state.ripples {
        rp.r += step * 180.0;
        rp.life -= step * 1.4;
        if rp.life > 0.02 { push(next, rp); }
    }
    state.ripples = next;
}
fn on_render(state, t, canvas) {
    shader_plasma(t, state.scale, state.pal);
    for rp in state.ripples {
        stroke_circle(rp.x, rp.y, rp.r, hsla(195, 0.95, 0.72, rp.life), 2.5);
    }
    text(canvas.width * 0.5, 48, "AURORA PLASMA", 16, rgba(255, 255, 255, 0.92), "center");
}"#;

/// Verified compact Playable Game few-shot example with HUD & restart-safe `high_score`.
pub const FEW_SHOT_GAME_CS1: &str = r#"fn on_init(state) {
    state.score = 0;
    if state.high_score == null { state.high_score = 0; }
    state.game_over = false;
    state.status_text = "NEON CATCH";
    state.px = 180.0;
    state.ox = 180.0;
    state.oy = 90.0;
    state.misses = 0;
}
fn on_update(state, dt, input) {
    if state.game_over { return; }
    let step = min(dt, 0.04);
    if input.left { state.px -= 280.0 * step; }
    if input.right { state.px += 280.0 * step; }
    if input.pointer_active { state.px = lerp(state.px, input.mouse_x, 0.35); }
    state.px = clamp(state.px, 44.0, 316.0);
    state.oy += 220.0 * step;
    if circle_rect_hit(state.ox, state.oy, 12.0, state.px - 38.0, 556.0, 76.0, 14.0) {
        state.score += 10;
        if state.score > state.high_score { state.high_score = state.score; }
        state.oy = 70.0;
        state.ox = rand_range(40, 320);
        tone(560, 0.06, "triangle", 0.45);
    } else if state.oy > 640.0 {
        state.misses += 1;
        state.oy = 70.0;
        state.ox = rand_range(40, 320);
        if state.misses >= 2 {
            state.game_over = true;
            state.status_text = "GAME OVER";
            tone(130, 0.14, "saw", 0.5);
        }
    }
}
fn on_render(state, t, canvas) {
    clear_grad(rgb(8, 12, 28), rgb(22, 14, 48), 180);
    glow_circle(state.ox, state.oy, 14, hsla(315, 0.96, 0.65, 0.9), 1.3);
    round_rect(state.px - 38, 556, 76, 14, 7, hsla(190, 0.95, 0.68, 0.95));
    text(20, 40, "SCORE " + str(state.score), 16, rgba(255, 255, 255, 0.95), "left");
    text(canvas.width - 20, 40, "BEST " + str(state.high_score), 15, hsla(190, 0.9, 0.8, 1.0), "right");
}"#;

/// Paired `(system, user)` prompt messages ready for `ProviderRouter` live LLM calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmPromptMessages {
    pub system: String,
    pub user: String,
}

/// Extracted LLM scene output returned by `LlmResponseParser::parse`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedLlmScene {
    pub title: Option<String>,
    pub description: Option<String>,
    pub kind: Option<AssetKind>,
    pub tags: Vec<String>,
    pub controls_hint: Option<String>,
    pub manifest: Option<AssetManifest>,
    pub source_code: String,
    /// `"json_direct"` | `"json_fenced"` | `"hybrid_json_and_fence"` | `"code_fence"` | `"raw_cs1"`
    pub extraction_mode: String,
}

impl ParsedLlmScene {
    /// Converts a parsed LLM scene into a `SynthesizedDraft` validated through `validate_to_report`.
    /// Returns `(draft, validation_ok)`.
    pub fn into_validated_draft(
        &self,
        fallback_prompt: &str,
        fallback_kind: Option<AssetKind>,
        engine_used: &str,
    ) -> (SynthesizedDraft, bool) {
        let synth_req = SynthRequest {
            prompt: fallback_prompt.to_string(),
            kind: self.kind.or(fallback_kind),
            style_tags: self.tags.clone(),
            seed: self.manifest.as_ref().map(|m| m.seed),
        };
        let intent = IntentAnalyzer::analyze_create(&synth_req);
        let resolved_kind = self.kind.or(fallback_kind).unwrap_or(intent.kind);

        let mut manifest = self
            .manifest
            .clone()
            .unwrap_or_else(|| AssetManifest::for_kind(resolved_kind));
        manifest.kind = resolved_kind;
        if resolved_kind == AssetKind::Video && manifest.duration_sec.is_none() {
            manifest.duration_sec = intent.duration_sec.or(Some(15.0));
        } else if resolved_kind != AssetKind::Video {
            manifest.duration_sec = None;
        }
        if manifest.seed == 0 {
            manifest.seed = intent.seed;
        }
        manifest.clamp_in_place();

        let validation = validate_to_report(&self.source_code, &manifest);
        let ok = validation.ok;

        let title = self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or(intent.title);
        let description = self
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or(intent.description);
        let tags = if self.tags.is_empty() {
            intent.tags
        } else {
            normalize_tags(&self.tags)
        };
        let controls_hint = self
            .controls_hint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                intent
                    .archetype
                    .default_controls_hint(intent.is_chinese)
                    .to_string()
            });
        let archetype = if intent.archetype.kind() == resolved_kind {
            intent.archetype
        } else {
            match resolved_kind {
                AssetKind::Video => SceneArchetype::VideoStarWarpReel,
                AssetKind::Animation => SceneArchetype::AnimFluidPlasma,
                AssetKind::Game => SceneArchetype::GameNeonPinballBreakout,
            }
        };

        let draft = SynthesizedDraft {
            title,
            description,
            kind: resolved_kind,
            tags,
            source_code: self.source_code.clone(),
            code_format: CODE_SCENE_FORMAT_CS1.to_string(),
            manifest,
            controls_hint,
            archetype,
            palette: intent.palette,
            validation,
            engine_used: engine_used.to_string(),
        };
        (draft, ok)
    }
}

/// Errors returned when an LLM response contains no extractable `cs1` code.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LlmParseError {
    #[error("LLM response was empty or whitespace-only")]
    EmptyResponse,
    #[error("no usable cs1 script found in LLM response (preview: {preview})")]
    NoScriptFound { preview: String },
}

/// Constructs grounded system, creation, remix, and compiler/validator repair prompts for `cs1`.
#[derive(Debug, Clone, Default)]
pub struct LlmPromptBuilder;

impl LlmPromptBuilder {
    /// Builds the complete `cs1` system prompt containing exact verified grammar,
    /// all 68 canonical stdlib builtins, `VmBudgets` limits, lifecycle hooks,
    /// JSON output schema, and 3 verified few-shot examples.
    pub fn build_system_prompt() -> String {
        format!(
            r#"You are an expert Code Scene ("代码场景") AI Bot author writing sandboxed CodeSceneScript (`cs1`) programs for a vertical 360x640 @ 60fps interactive stage.

## 1. Exact `cs1` Grammar & Semantics
- Top-level items ONLY: `let GLOBAL = expr;` and `fn name(p1, p2) {{ ... }}`. Bare top-level statements fail to compile.
- Allowed keywords (13): `let`, `fn`, `if`, `else`, `while`, `for`, `in`, `break`, `continue`, `return`, `true`, `false`, `null`.
- Predefined constants: `PI`, `TAU`.
- Identifiers: ASCII ONLY `[A-Za-z_][A-Za-z0-9_]*`. Non-ASCII/Chinese text is ONLY allowed inside double-quoted `"..."` strings and `//` or `/* */` comments.
- Control flow: `if cond {{ ... }} else if cond2 {{ ... }} else {{ ... }}` (braces required; `if` is a statement, NOT an expression), `while cond {{ ... }}`, `for i in a..b {{ ... }}` (exclusive), `for i in a..=b {{ ... }}` (inclusive), `for item in list {{ ... }}` (LISTS ONLY — for maps iterate `for k in keys(m) {{ ... }}`).
- Operators: `||`, `&&`, `== !=`, `< <= > >=`, `+ -`, `* / %`, unary `- !`, assignments `= += -= *= /= %=`, list `[a, b]`, map `{{ k: v }}`.
- Keep `return expr;` on one line (ASI returns `null` if newline follows `return`).
- DO NOT USE: `const`, `var`, `mut`, `function`, `def`, `elif`, `switch`, `match`, `loop`, `struct`, `class`, `import`, `? :` ternary, `**`, `++`/`--`, bitwise `& | ^ << >>`, single-quoted strings `'...'`, lambdas/closures, or `for k in map`.

## 2. Lifecycle Hooks, `canvas`, `input`, and Game HUD `state`
- `fn on_init(state)` (optional): initializes `state.*` fields and optional `pixel_init(w, h)`.
- `fn on_update(state, dt, input)` (optional): physics/input update. IMPORTANT: For `Video` assets, OMIT `on_update` so timeline `seek_video(t)` is O(1) deterministic!
- `fn on_render(state, t, canvas)` (REQUIRED): draws visible frame output. `canvas.width`/`canvas.w`, `canvas.height`/`canvas.h`, `canvas.fps`, `canvas.duration`.
- Canonical 10 `input` fields: `input.left`, `input.right`, `input.up`, `input.down`, `input.action`, `input.mouse_x`, `input.mouse_y`, `input.mouse_down`, `input.clicked`, `input.pointer_active`.
- Canonical 4 Game HUD `state` fields: `state.score`, `state.high_score` (preserve via `if state.high_score == null {{ state.high_score = 0; }}`), `state.game_over` (latch `if state.game_over {{ return; }}`), `state.status_text` (<=256 bytes).

## 3. Complete 68-Builtin Standard Library (Direct or `canvas.*` / collection methods)
- Math (18): `sin(x)`, `cos(x)`, `tan(x)`, `atan2(y, x)`, `sqrt(x)`, `abs(x)`, `floor(x)`, `ceil(x)`, `round(x)`, `min(a, b)`, `max(a, b)`, `clamp(x, a, b)`, `lerp(a, b, t)`, `smoothstep(e0, e1, x)`, `pow(a, b)`, `exp(x)`, `fract(x)`, `hypot(x, y)`.
- Easing & Timeline (5): `ease_in_out(t)`, `ease_out_cubic(t)`, `ease_out_bounce(t)`, `ease_out_elastic(t)`, `keyframe(t, stops)` (`stops` = list of `[time, val]` or `[time, val, "in_out"|"out_cubic"|"bounce"|"elastic"]`).
- Noise & RNG (5): `noise2d(x, y)`, `fbm2d(x, y[, oct=4])`, `rand()`, `rand_range(a, b)`, `hash2(x, y)`.
- Color (5): `rgb(r, g, b)`, `rgba(r, g, b, a)`, `hsl(h, s, l)`, `hsla(h, s, l, a)`, `lerp_color(c1, c2, t)`.
- 2D Vector Canvas & Audio (16, also `canvas.*`): `clear(col)`, `clear_grad(top, bot[, angle_deg=180])`, `rect(x, y, w, h, col)`, `round_rect(x, y, w, h, r, col)` (NOTE: `r` BEFORE `col`!), `stroke_rect(x, y, w, h, col[, sw=2])` (NOTE: `col` BEFORE `sw`!), `circle(cx, cy, r, col)`, `stroke_circle(cx, cy, r, col[, sw=2])`, `glow_circle(cx, cy, r, col[, intensity=1])`, `line(x1, y1, x2, y2, col[, sw=2])`, `polyline(pts, col[, sw=2])`, `polygon(pts, col)`, `arc(cx, cy, r, a0, a1, col[, sw=2])`, `bezier(x0, y0, cx1, cy1, cx2, cy2, x1, y1, col[, sw=2])`, `star(cx, cy, ro, ri, pts, col[, rot=0])`, `text(x, y, str, size, col[, align="left"|"center"|"right"])`, `tone(freq, dur[, wave="sine"|"square"|"triangle"|"saw"|"noise"[, gain=0.6]])`.
- Pixel & Shader Kernels (8, also `canvas.*`): `pixel_init(w, h)`, `pixel_set(x, y, col)`, `pixel_rect(x, y, w, h, col)`, `shader_plasma(t, scale, pal)`, `shader_mandelbrot(cx, cy, zoom, iter)`, `shader_voronoi(t, cells, pal)`, `shader_starfield(t, speed, density)`, `shader_synthwave(t, speed, hue_deg)`.
- Collision (4): `dist(x1, y1, x2, y2)`, `aabb_hit(ax, ay, aw, ah, bx, by, bw, bh)`, `circle_hit(x1, y1, r1, x2, y2, r2)`, `circle_rect_hit(cx, cy, cr, rx, ry, rw, rh)`.
- Utilities (7, also `list.len/push/pop`, `map.len/keys/has_key`, `str.len`): `len(x)`, `push(list, v)`, `pop(list)`, `str(v)`, `num(v)`, `keys(map)`, `has_key(map, k)`.

## 4. Hard `VmBudgets` Safety Limits
- `max_source_bytes = 65536`, `max_ast_depth = 64`, `max_bytecode_ops = 16384`, `max_ops_per_frame = 100000`, `max_frame_wall_ms = 10`, `max_draw_cmds = 2048`, `max_pixel_width = 240`, `max_pixel_height = 320`, `max_pixel_area = 76800`.
- Keep shader pixel buffers lean (e.g. `pixel_init(72, 128)`, mandelbrot `iter <= 22`, voronoi `cells <= 12`) and cap dynamic entity/particle lists `<= 48` items.

## 5. Required JSON Output Schema
Respond with a single JSON object (no surrounding prose):
{{
  "title": "Short title",
  "description": "1-sentence description",
  "kind": "video" | "animation" | "game",
  "tags": ["tag1", "tag2", "tag3"],
  "controls_hint": "Controls hint",
  "manifest": {{ "kind": "animation", "width": 360, "height": 640, "fps": 60, "duration_sec": null, "seed": 42 }},
  "source_code": "fn on_init(state) {{ ... }}\nfn on_render(state, t, canvas) {{ ... }}"
}}

## 6. Verified Few-Shot `cs1` Examples
### Video Example (`kind = "video"`, `duration_sec = 12.0`, omits `on_update`):
```cs1
{video}
```
### Animation Example (`kind = "animation"`):
```cs1
{anim}
```
### Game Example (`kind = "game"`):
```cs1
{game}
```"#,
            video = FEW_SHOT_VIDEO_CS1,
            anim = FEW_SHOT_ANIMATION_CS1,
            game = FEW_SHOT_GAME_CS1,
        )
    }

    /// Builds `(system, user)` prompt messages for creating a new Code Scene from natural language.
    pub fn build_create_prompt(
        prompt: &str,
        kind: Option<AssetKind>,
        style_tags: &[String],
        seed: Option<u64>,
    ) -> LlmPromptMessages {
        let kind_line = match kind {
            Some(k) => format!("Target modality: `{}` (REQUIRED).", k.as_str()),
            None => "Target modality: infer best fit (`video` | `animation` | `game`) from the prompt.".to_string(),
        };
        let tags_line = if style_tags.is_empty() {
            String::new()
        } else {
            format!("\nPreferred style tags: {}", style_tags.join(", "))
        };
        let seed_val = seed.unwrap_or(42);
        let user = format!(
            "Create a high-craft vertical 360x640 @ 60fps Code Scene in `cs1`.\n\
             User request: {prompt}\n\
             {kind_line}{tags_line}\n\
             Manifest seed: {seed_val}\n\
             Return ONLY the valid JSON object matching the schema with complete runnable `source_code`."
        );
        LlmPromptMessages {
            system: Self::build_system_prompt(),
            user,
        }
    }

    /// Builds `(system, user)` prompt messages for remixing/iterating an existing `cs1` script.
    pub fn build_remix_prompt(
        existing_source: &str,
        existing_manifest: &AssetManifest,
        existing_title: Option<&str>,
        instruction: &str,
    ) -> LlmPromptMessages {
        let manifest_json =
            serde_json::to_string(existing_manifest).unwrap_or_else(|_| "{}".to_string());
        let title_str = existing_title.unwrap_or("Untitled Scene");
        let user = format!(
            "Modify/remix the existing `cs1` Code Scene below according to the user's instruction.\n\
             Existing title: {title_str}\n\
             Existing manifest: {manifest_json}\n\
             Remix instruction: {instruction}\n\n\
             Existing `cs1` source:\n\
             ```cs1\n\
             {existing_source}\n\
             ```\n\n\
             Return ONLY a valid JSON object containing the COMPLETE runnable replacement `source_code` (never a partial diff)."
        );
        LlmPromptMessages {
            system: Self::build_system_prompt(),
            user,
        }
    }

    /// Builds a structured compiler/validator diagnostic self-repair prompt with line-numbered
    /// `±4` source excerpts and targeted fix hints by error code.
    pub fn build_repair_prompt(
        original_prompt: &str,
        failed_source: &str,
        manifest: &AssetManifest,
        diagnostic: Option<&CompileDiagnostic>,
        report: Option<&ValidationReportDto>,
        attempt: u32,
    ) -> LlmPromptMessages {
        let mut diag_lines = Vec::new();
        let mut focus_lines: Vec<usize> = Vec::new();
        let mut hint_codes: Vec<String> = Vec::new();

        if let Some(d) = diagnostic {
            let line_str = d.line.map(|l| l.to_string()).unwrap_or_else(|| "?".to_string());
            let col_str = d.col.map(|c| c.to_string()).unwrap_or_else(|| "?".to_string());
            diag_lines.push(format!(
                "- [{}] ({:?}) line {}, col {}: {}",
                d.code, d.stage, line_str, col_str, d.message
            ));
            if let Some(l) = d.line {
                focus_lines.push(l);
            }
            hint_codes.push(d.code.clone());
        }
        if let Some(rep) = report {
            for d in &rep.diagnostics {
                let stage = d.stage.as_deref().unwrap_or("validate");
                let line_str = d.line.map(|l| l.to_string()).unwrap_or_else(|| "?".to_string());
                let col_str = d.column.map(|c| c.to_string()).unwrap_or_else(|| "?".to_string());
                diag_lines.push(format!(
                    "- [{}] ({}) line {}, col {}: {}",
                    d.code, stage, line_str, col_str, d.message
                ));
                if let Some(l) = d.line {
                    focus_lines.push(l);
                }
                hint_codes.push(d.code.clone());
            }
        }
        if diag_lines.is_empty() {
            diag_lines.push("- [E_UNKNOWN] validation failed without diagnostic details".to_string());
        }

        let excerpt = build_line_numbered_excerpt(failed_source, &focus_lines);
        let hints = collect_targeted_fix_hints(&hint_codes);
        let manifest_json = serde_json::to_string(manifest).unwrap_or_else(|_| "{}".to_string());

        let user = format!(
            "Self-repair attempt {attempt} of 2.\n\
             Original user request: {original_prompt}\n\
             Target manifest: {manifest_json}\n\n\
             Diagnostics from `cs1` compiler/validator:\n\
             {diags}\n\n\
             Targeted fix guidance:\n\
             {hints}\n\n\
             Failing source context:\n\
             ```cs1\n\
             {excerpt}\n\
             ```\n\n\
             Fix ALL errors and return ONLY the valid JSON object with complete corrected `source_code`.",
            diags = diag_lines.join("\n"),
        );

        LlmPromptMessages {
            system: Self::build_system_prompt(),
            user,
        }
    }

    pub fn build_repair_from_diagnostic(
        original_prompt: &str,
        failed_source: &str,
        manifest: &AssetManifest,
        diagnostic: &CompileDiagnostic,
        attempt: u32,
    ) -> LlmPromptMessages {
        Self::build_repair_prompt(
            original_prompt,
            failed_source,
            manifest,
            Some(diagnostic),
            None,
            attempt,
        )
    }

    pub fn build_repair_from_report(
        original_prompt: &str,
        failed_source: &str,
        manifest: &AssetManifest,
        report: &ValidationReportDto,
        attempt: u32,
    ) -> LlmPromptMessages {
        Self::build_repair_prompt(
            original_prompt,
            failed_source,
            manifest,
            None,
            Some(report),
            attempt,
        )
    }
}

fn build_line_numbered_excerpt(source: &str, focus_lines: &[usize]) -> String {
    let lines: Vec<&str> = source.lines().collect();
    if lines.is_empty() {
        return "<empty source>".to_string();
    }
    let valid_focus: Vec<usize> = focus_lines.iter().copied().filter(|&l| l > 0).collect();
    if valid_focus.is_empty() || lines.len() <= 24 {
        let cap = if lines.len() <= 24 { lines.len() } else { 16 };
        let mut out: Vec<String> = lines
            .iter()
            .take(cap)
            .enumerate()
            .map(|(i, l)| {
                let ln = i + 1;
                let mark = if valid_focus.contains(&ln) { ">>>" } else { "   " };
                format!("{mark} {ln:4} | {l}")
            })
            .collect();
        if lines.len() > cap {
            out.push("       ...".to_string());
        }
        return out.join("\n");
    }

    let mut included = vec![false; lines.len()];
    for &fl in &valid_focus {
        let idx = (fl - 1).min(lines.len() - 1);
        let lo = idx.saturating_sub(4);
        let hi = (idx + 4).min(lines.len() - 1);
        for slot in included.iter_mut().take(hi + 1).skip(lo) {
            *slot = true;
        }
    }
    if !included.iter().any(|&x| x) {
        for slot in included.iter_mut().take(16) {
            *slot = true;
        }
    }

    let mut out = Vec::new();
    let mut prev_shown = true;
    for (i, line) in lines.iter().enumerate() {
        if included[i] {
            if !prev_shown {
                out.push("       ...".to_string());
            }
            let ln = i + 1;
            let mark = if valid_focus.contains(&ln) { ">>>" } else { "   " };
            out.push(format!("{mark} {ln:4} | {line}"));
            prev_shown = true;
        } else {
            prev_shown = false;
        }
    }
    out.join("\n")
}

fn collect_targeted_fix_hints(codes: &[String]) -> String {
    let mut hints = Vec::new();
    let mut push_unique = |h: &str| {
        if !hints.iter().any(|x: &String| x == h) {
            hints.push(h.to_string());
        }
    };
    for code in codes {
        let c = code.as_str();
        if c == "E_MISSING_ON_RENDER" {
            push_unique("- Define top-level `fn on_render(state, t, canvas) { ... }` emitting visible draw or shader calls.");
        } else if c == "E_UNDEFINED_FN" {
            push_unique("- Only call the 68 listed `cs1` builtins or functions defined in this file; check exact spelling.");
        } else if c == "E_UNDEFINED_VAR" {
            push_unique("- Declare local/global variables with `let name = ...;` before assigning to them.");
        } else if c == "E_ARITY_MISMATCH" {
            push_unique("- Match exact argument counts for builtins and user functions (e.g. `round_rect(x,y,w,h,r,col)` takes 6 args).");
        } else if c.starts_with("E_PARSE_") || c.starts_with("E_LEX_") {
            push_unique("- Use valid `cs1` syntax only: `let`/`fn`/`if {}`/`for i in a..b {}`, double-quoted strings, ASCII identifiers, NO `const`/`var`/`mut`/`? :`/`**`.");
        } else if c == "E_NO_VISIBLE_OUTPUT" {
            push_unique("- Emit at least one visible `clear`/`clear_grad`/`rect`/`circle`/`glow_circle`/`shader_*` draw inside `on_render`.");
        } else if c.starts_with("E_DRY_RUN_") {
            push_unique("- Avoid runtime errors (OOB list writes, non-map field writes, op/time budget overflow); keep shader buffers `<= 96x160` and lists `<= 48`.");
        } else if c == "E_INVALID_MANIFEST" {
            push_unique("- Ensure `manifest.width` in `160..=720`, `height` in `160..=1280`, `fps` in `15..=60`, and `Video` has `duration_sec` in `5.0..=120.0`.");
        }
    }
    if hints.is_empty() {
        hints.push("- Ensure the full `cs1` script compiles and produces visible output within `VmBudgets`.".to_string());
    }
    hints.join("\n")
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RawLlmJson {
    title: Option<String>,
    description: Option<String>,
    kind: Option<String>,
    tags: Option<Vec<String>>,
    controls_hint: Option<String>,
    manifest: Option<AssetManifest>,
    source_code: Option<String>,
    code: Option<String>,
    script: Option<String>,
}

impl RawLlmJson {
    fn take_source(&self) -> Option<String> {
        for cand in [&self.source_code, &self.code, &self.script] {
            if let Some(s) = cand.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                return Some(s.to_string());
            }
        }
        None
    }

    fn resolved_kind(&self) -> Option<AssetKind> {
        self.kind
            .as_deref()
            .and_then(AssetKind::parse)
            .or_else(|| self.manifest.as_ref().map(|m| m.kind))
    }

    fn has_any_meta(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.kind.is_some()
            || self.tags.as_ref().is_some_and(|t| !t.is_empty())
            || self.controls_hint.is_some()
            || self.manifest.is_some()
    }

    fn into_parsed(self, source_code: String, extraction_mode: &str) -> ParsedLlmScene {
        let kind = self.resolved_kind();
        ParsedLlmScene {
            title: self.title,
            description: self.description,
            kind,
            tags: self.tags.unwrap_or_default(),
            controls_hint: self.controls_hint,
            manifest: self.manifest,
            source_code,
            extraction_mode: extraction_mode.to_string(),
        }
    }
}

/// Resilient multi-pass parser extracting `cs1` code + optional metadata from live LLM output.
#[derive(Debug, Clone, Default)]
pub struct LlmResponseParser;

impl LlmResponseParser {
    /// Extracts a `ParsedLlmScene` from raw LLM output (handling `<think>` blocks,
    /// direct JSON, fenced JSON, hybrid JSON + code fence, multiple fences, unclosed
    /// trailing fences, and raw unfenced `cs1` with surrounding prose).
    pub fn parse(raw: &str) -> Result<ParsedLlmScene, LlmParseError> {
        let normalized = raw
            .trim_start_matches('\u{feff}')
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let trimmed_raw = normalized.trim();
        if trimmed_raw.is_empty() {
            return Err(LlmParseError::EmptyResponse);
        }

        let stripped = strip_think_blocks(trimmed_raw);
        let cleaned = stripped.trim().to_string();
        if cleaned.is_empty() {
            let preview: String = trimmed_raw.chars().take(120).collect();
            return Err(LlmParseError::NoScriptFound { preview });
        }

        let budgets = VmBudgets::default();
        let mut partial_meta: Option<RawLlmJson> = None;
        let mut uncompiled_json_fallback: Option<ParsedLlmScene> = None;

        let mut inspect_json = |json_obj: RawLlmJson, mode: &str| -> Option<ParsedLlmScene> {
            if let Some(src) = json_obj.take_source() {
                if looks_like_cs1(&src)
                    && compile_source(&src, &budgets)
                        .is_ok_and(|compiled| compiled.has_function("on_render"))
                {
                    return Some(json_obj.into_parsed(src, mode));
                }
                if json_obj.has_any_meta() && partial_meta.is_none() {
                    partial_meta = Some(json_obj.clone());
                }
                if looks_like_cs1(&src) && uncompiled_json_fallback.is_none() {
                    uncompiled_json_fallback = Some(json_obj.into_parsed(src, mode));
                }
            } else if json_obj.has_any_meta() && partial_meta.is_none() {
                partial_meta = Some(json_obj);
            }
            None
        };

        // Pass 1A: Direct JSON object
        if let Some(json_obj) = try_parse_raw_json(&cleaned) {
            if let Some(parsed) = inspect_json(json_obj, "json_direct") {
                return Ok(parsed);
            }
        }

        // Collect all markdown code fences once (including unclosed trailing fence!)
        let fences = extract_markdown_fences(&cleaned);

        // Pass 1B: Fenced ```json blocks
        for fence in &fences {
            let lang = fence.lang.as_str();
            if lang == "json" || lang == "jsonc" || fence.body.trim_start().starts_with('{') {
                if let Some(json_obj) = try_parse_raw_json(fence.body.trim()) {
                    if let Some(parsed) = inspect_json(json_obj, "json_fenced") {
                        return Ok(parsed);
                    }
                }
            }
        }

        // Pass 1C: Balanced `{ ... }` JSON scan inside prose
        for obj_slice in scan_balanced_json_objects(&cleaned) {
            if let Some(json_obj) = try_parse_raw_json(obj_slice) {
                if let Some(parsed) = inspect_json(json_obj, "json_direct") {
                    return Ok(parsed);
                }
            }
        }

        drop(inspect_json);

        // Pass 2: Markdown fenced code blocks (scored by real compile_source + on_render + lang tag)
        let mut best_compiled_fence: Option<(i64, usize, String)> = None;
        let mut best_uncompiled_fence: Option<(i64, usize, String)> = None;
        for fence in &fences {
            let body = fence.body.trim();
            if body.is_empty() {
                continue;
            }
            // Never mistake a JSON metadata object fence for raw cs1 code
            if matches!(fence.lang.as_str(), "json" | "jsonc") && body.starts_with('{') {
                continue;
            }
            if !looks_like_cs1(body) {
                continue;
            }
            let compiles = compile_source(body, &budgets).is_ok();
            let mut score = if compiles { 100i64 } else { 0i64 };
            if body.contains("fn on_render") {
                score += 40;
            }
            if matches!(fence.lang.as_str(), "cs1" | "code_scene" | "codescene") {
                score += 20;
            }
            if body.contains("fn on_init") {
                score += 10;
            }
            let len = body.len();
            let slot = if compiles {
                &mut best_compiled_fence
            } else {
                &mut best_uncompiled_fence
            };
            let replace = match slot {
                None => true,
                Some((bs, bl, _)) => score > *bs || (score == *bs && len > *bl),
            };
            if replace {
                *slot = Some((score, len, body.to_string()));
            }
        }

        let wrap_extracted = |code: String, meta: Option<RawLlmJson>, plain_mode: &str| {
            match meta {
                Some(m) => m.into_parsed(code, "hybrid_json_and_fence"),
                None => ParsedLlmScene {
                    title: None,
                    description: None,
                    kind: None,
                    tags: Vec::new(),
                    controls_hint: None,
                    manifest: None,
                    source_code: code,
                    extraction_mode: plain_mode.to_string(),
                },
            }
        };

        if let Some((_, _, code)) = best_compiled_fence {
            return Ok(wrap_extracted(code, partial_meta, "code_fence"));
        }

        // Pass 3: Raw unfenced `cs1` extraction (strip leading & trailing prose)
        let raw_candidate = extract_raw_unfenced_cs1(&cleaned);
        if let Some(ref raw_code) = raw_candidate {
            if compile_source(raw_code, &budgets).is_ok() {
                return Ok(wrap_extracted(raw_code.clone(), partial_meta, "raw_cs1"));
            }
        }

        // Broken-code self-repair fallbacks (prefer JSON source, then fenced, then raw)
        if let Some(json_fb) = uncompiled_json_fallback {
            return Ok(json_fb);
        }
        if let Some((_, _, code)) = best_uncompiled_fence {
            return Ok(wrap_extracted(code, partial_meta, "code_fence"));
        }
        if let Some(raw_code) = raw_candidate {
            return Ok(wrap_extracted(raw_code, partial_meta, "raw_cs1"));
        }

        let preview: String = cleaned.chars().take(120).collect();
        Err(LlmParseError::NoScriptFound { preview })
    }
}

struct MarkdownFence {
    lang: String,
    body: String,
}

fn strip_think_blocks(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut pos = 0usize;
    while pos < input.len() {
        let Some(rel_start) = lower[pos..].find("<think>") else {
            out.push_str(&input[pos..]);
            break;
        };
        let start = pos + rel_start;
        out.push_str(&input[pos..start]);
        let search_after = start + "<think>".len();
        if let Some(rel_end) = lower[search_after..].find("</think>") {
            pos = search_after + rel_end + "</think>".len();
        } else {
            // Drop trailing unclosed `<think>` scratchpad cleanly
            break;
        }
    }
    out
}

fn try_parse_raw_json(candidate: &str) -> Option<RawLlmJson> {
    if !candidate.trim_start().starts_with('{') {
        return None;
    }
    if let Ok(parsed) = serde_json::from_str::<RawLlmJson>(candidate) {
        return Some(parsed);
    }
    let repaired = escape_raw_newlines_in_json_strings(candidate);
    serde_json::from_str::<RawLlmJson>(&repaired).ok()
}

fn escape_raw_newlines_in_json_strings(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 32);
    let mut in_string = false;
    let mut escaped = false;
    for ch in src.chars() {
        if in_string {
            if escaped {
                out.push(ch);
                escaped = false;
            } else if ch == '\\' {
                out.push(ch);
                escaped = true;
            } else if ch == '"' {
                out.push(ch);
                in_string = false;
            } else if ch == '\n' {
                out.push_str("\\n");
            } else if ch == '\t' {
                out.push_str("\\t");
            } else {
                out.push(ch);
            }
        } else {
            if ch == '"' {
                in_string = true;
            }
            out.push(ch);
        }
    }
    out
}

fn extract_markdown_fences(text: &str) -> Vec<MarkdownFence> {
    let mut fences = Vec::new();
    let mut in_fence = false;
    let mut cur_lang = String::new();
    let mut cur_body = String::new();

    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if !in_fence {
                in_fence = true;
                cur_lang = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase();
                cur_body.clear();
            } else {
                fences.push(MarkdownFence {
                    lang: std::mem::take(&mut cur_lang),
                    body: std::mem::take(&mut cur_body),
                });
                in_fence = false;
            }
        } else if in_fence {
            if !cur_body.is_empty() {
                cur_body.push('\n');
            }
            cur_body.push_str(line);
        }
    }

    // Handle unclosed trailing fence truncated at EOF
    if in_fence && !cur_body.trim().is_empty() {
        fences.push(MarkdownFence {
            lang: cur_lang,
            body: cur_body,
        });
    }

    fences
}

fn scan_balanced_json_objects(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < len {
        if bytes[i] == b'{' {
            let start = i;
            let mut depth = 0usize;
            let mut in_str = false;
            let mut esc = false;
            let mut j = i;
            while j < len {
                let b = bytes[j];
                if in_str {
                    if esc {
                        esc = false;
                    } else if b == b'\\' {
                        esc = true;
                    } else if b == b'"' {
                        in_str = false;
                    }
                } else if b == b'"' {
                    in_str = true;
                } else if b == b'{' {
                    depth += 1;
                } else if b == b'}' {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let slice = &text[start..=j];
                        if slice.contains("\"source_code\"")
                            || slice.contains("\"code\"")
                            || slice.contains("\"script\"")
                            || slice.contains("\"title\"")
                        {
                            out.push(slice);
                        }
                        i = j;
                        break;
                    }
                }
                j += 1;
            }
        }
        i += 1;
    }

    out
}

fn looks_like_cs1(src: &str) -> bool {
    src.contains("fn ") && src.contains('(') && src.contains('{') && src.contains('}')
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TopItemState {
    BetweenItems,
    InFnDecl,
    InLetDecl,
}

fn skip_ws_and_comments(bytes: &[u8], mut i: usize) -> usize {
    let len = bytes.len();
    while i < len {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(len);
            continue;
        }
        break;
    }
    i
}

fn skip_ascii_ident(bytes: &[u8], i: usize) -> Option<usize> {
    let first = *bytes.get(i)?;
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    let mut end = i + 1;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    Some(end)
}

fn match_top_fn_decl(bytes: &[u8], i: usize) -> Option<usize> {
    if !bytes[i..].starts_with(b"fn") {
        return None;
    }
    if bytes
        .get(i + 2)
        .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return None;
    }
    let after_kw = skip_ws_and_comments(bytes, i + 2);
    let ident_end = skip_ascii_ident(bytes, after_kw)?;
    let after_ident = skip_ws_and_comments(bytes, ident_end);
    if bytes.get(after_ident) == Some(&b'(') {
        Some(after_ident)
    } else {
        None
    }
}

fn match_top_let_decl(bytes: &[u8], i: usize) -> Option<usize> {
    if !bytes[i..].starts_with(b"let") {
        return None;
    }
    if bytes
        .get(i + 3)
        .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return None;
    }
    let after_kw = skip_ws_and_comments(bytes, i + 3);
    let ident_end = skip_ascii_ident(bytes, after_kw)?;
    let after_ident = skip_ws_and_comments(bytes, ident_end);
    if bytes.get(after_ident) == Some(&b'=') && bytes.get(after_ident + 1) != Some(&b'=') {
        Some(after_ident + 1)
    } else {
        None
    }
}

fn skip_ws_and_comments_no_blank_line(bytes: &[u8], mut i: usize) -> Option<usize> {
    let len = bytes.len();
    loop {
        while i < len && matches!(bytes[i], b' ' | b'\t') {
            i += 1;
        }
        if i >= len || bytes[i] == b'\n' || bytes[i] == b'\r' {
            return None;
        }
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            if i >= len {
                return None;
            }
            i += 1;
            continue;
        }
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            let mut prev_nl = false;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                if bytes[i] == b'\n' {
                    if prev_nl {
                        return None;
                    }
                    prev_nl = true;
                } else if !matches!(bytes[i], b' ' | b'\t' | b'\r') {
                    prev_nl = false;
                }
                i += 1;
            }
            i = (i + 2).min(len);
            if i < len && bytes[i] == b'\n' {
                i += 1;
            }
            continue;
        }
        return Some(i);
    }
}

fn is_expr_continuation_prefix(bytes: &[u8], i: usize) -> bool {
    let Some(&b) = bytes.get(i) else {
        return false;
    };
    let next = bytes.get(i + 1).copied();
    match b {
        b'.' | b'+' | b'/' | b'%' | b'<' => true,
        b'&' => next == Some(b'&'),
        b'|' => next == Some(b'|'),
        b'=' => next == Some(b'='),
        b'!' => next == Some(b'='),
        // Exclude Markdown blockquotes (`> Note:`), bullet lists (`* Item`, `- Item`),
        // bold/emphasis (`**`), and horizontal rules (`---`).
        b'>' => !matches!(next, Some(b' ' | b'\t' | b'\n' | b'\r') | None),
        b'*' => !matches!(next, Some(b' ' | b'\t' | b'*' | b'\n' | b'\r') | None),
        b'-' => !matches!(next, Some(b' ' | b'\t' | b'-' | b'\n' | b'\r') | None),
        _ => false,
    }
}

fn is_expr_continuation_suffix(b: u8) -> bool {
    matches!(
        b,
        b'+' | b'-' | b'*' | b'/' | b'%' | b',' | b'=' | b'&' | b'|' | b'.' | b'<' | b'>'
    )
}

fn extract_raw_unfenced_cs1(text: &str) -> Option<String> {
    if !text.contains("fn ") {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut start_line: Option<usize> = None;
    for (idx, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("// @cs1-params")
            || t.starts_with("// @cs1-remix-params")
            || t.starts_with("fn on_init(")
            || t.starts_with("fn on_update(")
            || t.starts_with("fn on_render(")
            || match_top_fn_decl(t.as_bytes(), 0).is_some()
            || (match_top_let_decl(t.as_bytes(), 0).is_some() && text.contains("fn "))
        {
            start_line = Some(idx);
            break;
        }
    }
    let start_idx = start_line?;
    let suffix = lines[start_idx..].join("\n");

    // Comment/string-aware top-level declaration scanner distinguishing `fn <ident>(...) { ... }`
    // bodies from top-level `let <ident> = <expr>;` initializers (which may contain map/list literals)
    // and rejecting trailing prose (even lowercase `"let ..."` / `"fn ..."` prose with `{braces}`).
    let bytes = suffix.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut brace_depth = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut state = TopItemState::BetweenItems;
    let mut seen_fn = false;
    let mut candidate_ends: Vec<usize> = Vec::new();
    let mut let_has_expr_token = false;
    let mut let_last_non_ws: Option<(usize, u8)> = None;

    while i < len {
        if state == TopItemState::BetweenItems {
            i = skip_ws_and_comments(bytes, i);
            while i < len && bytes[i] == b';' {
                i = skip_ws_and_comments(bytes, i + 1);
            }
            if i >= len {
                break;
            }
            if let Some(paren_pos) = match_top_fn_decl(bytes, i) {
                state = TopItemState::InFnDecl;
                i = paren_pos;
                continue;
            }
            if let Some(after_eq) = match_top_let_decl(bytes, i) {
                state = TopItemState::InLetDecl;
                let_has_expr_token = false;
                let_last_non_ws = None;
                i = after_eq;
                continue;
            }
            // Non-comment, non-declaration token at top level (e.g., trailing prose)
            break;
        }

        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(len);
            continue;
        }
        if bytes[i] == b'"' {
            i += 1;
            while i < len {
                if bytes[i] == b'\\' && i + 1 < len {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            if state == TopItemState::InLetDecl {
                let_has_expr_token = true;
                let_last_non_ws = Some((i, b'"'));
            }
            continue;
        }

        let b = bytes[i];
        match b {
            b'{' => {
                brace_depth += 1;
                if state == TopItemState::InLetDecl {
                    let_has_expr_token = true;
                    let_last_non_ws = Some((i + 1, b));
                }
                i += 1;
            }
            b'}' => {
                brace_depth = brace_depth.saturating_sub(1);
                if state == TopItemState::InFnDecl && brace_depth == 0 {
                    seen_fn = true;
                    if candidate_ends.last().copied() != Some(i + 1) {
                        candidate_ends.push(i + 1);
                    }
                    state = TopItemState::BetweenItems;
                } else if state == TopItemState::InLetDecl {
                    let_has_expr_token = true;
                    let_last_non_ws = Some((i + 1, b));
                }
                i += 1;
            }
            b'(' => {
                paren_depth += 1;
                if state == TopItemState::InLetDecl {
                    let_has_expr_token = true;
                    let_last_non_ws = Some((i + 1, b));
                }
                i += 1;
            }
            b')' => {
                paren_depth = paren_depth.saturating_sub(1);
                if state == TopItemState::InLetDecl {
                    let_has_expr_token = true;
                    let_last_non_ws = Some((i + 1, b));
                }
                i += 1;
            }
            b'[' => {
                bracket_depth += 1;
                if state == TopItemState::InLetDecl {
                    let_has_expr_token = true;
                    let_last_non_ws = Some((i + 1, b));
                }
                i += 1;
            }
            b']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                if state == TopItemState::InLetDecl {
                    let_has_expr_token = true;
                    let_last_non_ws = Some((i + 1, b));
                }
                i += 1;
            }
            b';' if state == TopItemState::InLetDecl
                && brace_depth == 0
                && paren_depth == 0
                && bracket_depth == 0 =>
            {
                if seen_fn && candidate_ends.last().copied() != Some(i + 1) {
                    candidate_ends.push(i + 1);
                }
                state = TopItemState::BetweenItems;
                i += 1;
            }
            b'\n'
                if state == TopItemState::InLetDecl
                    && brace_depth == 0
                    && paren_depth == 0
                    && bracket_depth == 0
                    && let_has_expr_token =>
            {
                let continues_from_prev = let_last_non_ws
                    .is_some_and(|(_, last_b)| is_expr_continuation_suffix(last_b));
                let next_tok = skip_ws_and_comments_no_blank_line(bytes, i + 1);
                let continues_on_next =
                    next_tok.is_some_and(|pos| is_expr_continuation_prefix(bytes, pos));
                if !continues_from_prev {
                    if seen_fn {
                        if let Some((end_pos, _)) = let_last_non_ws {
                            if candidate_ends.last().copied() != Some(end_pos) {
                                candidate_ends.push(end_pos);
                            }
                        }
                    }
                    if !continues_on_next {
                        state = TopItemState::BetweenItems;
                    }
                }
                i += 1;
            }
            _ => {
                if state == TopItemState::InLetDecl && !b.is_ascii_whitespace() {
                    let_has_expr_token = true;
                    let char_end = if b.is_ascii() {
                        i + 1
                    } else {
                        let mut k = i + 1;
                        while k < len && !suffix.is_char_boundary(k) {
                            k += 1;
                        }
                        k
                    };
                    let_last_non_ws = Some((char_end, b));
                    i = char_end;
                } else {
                    i += 1;
                }
            }
        }
    }

    if state == TopItemState::InLetDecl
        && seen_fn
        && brace_depth == 0
        && paren_depth == 0
        && bracket_depth == 0
        && let_has_expr_token
    {
        if let Some((end_pos, _)) = let_last_non_ws {
            if candidate_ends.last().copied() != Some(end_pos) {
                candidate_ends.push(end_pos);
            }
        }
    }

    // Prefer the longest completed top-level prefix that compiles cleanly with `on_render`,
    // rejecting decl-like explanatory trailing prose while keeping broken `on_render`
    // functions intact for self-repair via fallback to the final candidate boundary.
    let budgets = VmBudgets::default();
    for &end in candidate_ends.iter().rev() {
        let candidate = suffix[..end].trim();
        if looks_like_cs1(candidate)
            && compile_source(candidate, &budgets)
                .is_ok_and(|compiled| compiled.has_function("on_render"))
        {
            return Some(candidate.to_string());
        }
    }

    let last_end = *candidate_ends.last()?;
    let full_candidate = suffix[..last_end].trim();
    if !full_candidate.contains("on_render") {
        for &end in candidate_ends.iter().rev() {
            let candidate = suffix[..end].trim();
            if looks_like_cs1(candidate) && compile_source(candidate, &budgets).is_ok() {
                return Some(candidate.to_string());
            }
        }
    }

    if looks_like_cs1(full_candidate) {
        Some(full_candidate.to_string())
    } else {
        None
    }
}
