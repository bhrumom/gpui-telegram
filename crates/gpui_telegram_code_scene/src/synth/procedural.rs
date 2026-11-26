//! Offline deterministic procedural `cs1` synthesizer & 3-case source-level remix engine (`CS-F08`).

use std::collections::HashSet;

use gpui_telegram_proto::{AssetKind, AssetManifest, CODE_SCENE_FORMAT_CS1};

use crate::lang::ast::TopItem;
use crate::lang::limits::VmBudgets;
use crate::lang::{lexer, parser};
use crate::reco::embedding::normalize_tag;
use crate::runtime::validate_to_report;
use crate::synth::intent::{
    ColorPaletteTheme, IntentAnalyzer, RemixModifiers, SceneArchetype, SceneIntentSpec,
    contains_cjk, derive_deterministic_seed, truncate_chars,
};
use crate::synth::templates_anim::generate_anim_template;
use crate::synth::templates_game::generate_game_template;
use crate::synth::templates_video::{TemplateParams, generate_video_template};
use crate::synth::{
    ENGINE_PROCEDURAL_OFFLINE, ENGINE_PROCEDURAL_REMIX, SynthRequest, SynthesizedDraft,
};

const PARAMS_MARKER: &str = "// @cs1-params";
const REMIX_MARKER: &str = "// @cs1-remix-params";

/// Offline-first deterministic `cs1` synthesizer & source-level remixer.
#[derive(Debug, Clone, Default)]
pub struct DeterministicProceduralSynthesizer;

impl DeterministicProceduralSynthesizer {
    pub fn new() -> Self {
        Self
    }

    /// Analyzes prompt + optional kind/style_tags/seed, generates verified `cs1`, and validates it.
    pub fn synthesize(&self, req: &SynthRequest) -> SynthesizedDraft {
        let intent = IntentAnalyzer::analyze_create(req);
        self.synthesize_from_intent(&intent)
    }

    /// Convenience entrypoint for direct prompt creation.
    pub fn synthesize_prompt(&self, prompt: &str, kind: Option<AssetKind>) -> SynthesizedDraft {
        let req = SynthRequest {
            prompt: prompt.to_string(),
            kind,
            style_tags: Vec::new(),
            seed: None,
        };
        self.synthesize(&req)
    }

    /// Synthesizes a validated `SynthesizedDraft` from a pre-analyzed `SceneIntentSpec`.
    pub fn synthesize_from_intent(&self, intent: &SceneIntentSpec) -> SynthesizedDraft {
        let pal = intent.palette.spec();
        let m = &intent.modifiers;
        let phase = ((intent.seed % 6_283) as f64) / 1_000.0;

        let speed_mul = finite_or(m.speed_mul, 1.0).clamp(0.4, 2.5);
        let raw_hue = m
            .hue_override
            .filter(|v| v.is_finite())
            .unwrap_or(pal.primary_hue);
        let palette_hue = finite_or(raw_hue, 195.0).rem_euclid(360.0);
        let accent_hue = finite_or(pal.accent_hue, 320.0).rem_euclid(360.0);
        let density = (1.0 + finite_or(m.density_delta, 0.0)).clamp(0.5, 2.2);
        let obstacles = 3i32.saturating_add(m.obstacle_delta).clamp(1, 10);
        let difficulty = (1.0 + finite_or(m.difficulty_delta, 0.0)).clamp(0.5, 2.2);
        let particles = 16i32
            .saturating_add((finite_or(m.particle_boost, 0.0) * 14.0).round() as i32)
            .clamp(4, 48);
        let glow_level = (1.20 + finite_or(m.glow_boost, 0.0) * 0.45).clamp(0.6, 2.0);

        let params = TemplateParams {
            archetype: intent.archetype,
            palette_id: intent.palette.as_str().to_string(),
            speed_mul,
            palette_hue,
            accent_hue,
            shader_pal: pal.shader_pal,
            density,
            obstacles,
            difficulty,
            particles,
            glow_level,
            seed_phase: phase,
            bg_top: pal.bg_top,
            bg_bot: pal.bg_bot,
            headline: intent.title.clone(),
        };

        let source_code = match intent.kind {
            AssetKind::Video => generate_video_template(&params),
            AssetKind::Animation => generate_anim_template(&params),
            AssetKind::Game => generate_game_template(&params),
        };

        let mut manifest = AssetManifest::for_kind(intent.kind);
        manifest.duration_sec = intent
            .duration_sec
            .map(|d| finite_or(d, 15.0).clamp(5.0, 120.0));
        if intent.kind == AssetKind::Video && manifest.duration_sec.is_none() {
            manifest.duration_sec = Some(15.0);
        }
        manifest.seed = intent.seed;
        manifest.clamp_in_place();

        let validation = validate_to_report(&source_code, &manifest);

        SynthesizedDraft {
            title: intent.title.clone(),
            description: intent.description.clone(),
            kind: intent.kind,
            tags: intent.tags.clone(),
            source_code,
            code_format: CODE_SCENE_FORMAT_CS1.to_string(),
            manifest,
            controls_hint: intent
                .archetype
                .default_controls_hint(intent.is_chinese)
                .to_string(),
            archetype: intent.archetype,
            palette: intent.palette,
            validation,
            engine_used: ENGINE_PROCEDURAL_OFFLINE.to_string(),
        }
    }

    /// Real 3-case source-level remix engine supporting `@cs1-params` scripts,
    /// arbitrary valid `cs1` scripts (including all 18 M1 seeds), and explicit
    /// modality/archetype switches. Always returns a validated draft.
    pub fn remix_source(
        &self,
        existing_source: &str,
        existing_manifest: &AssetManifest,
        instruction: &str,
        existing_title: Option<&str>,
        existing_tags: &[String],
    ) -> SynthesizedDraft {
        let mods = IntentAnalyzer::analyze_remix(instruction, existing_manifest.kind);
        let is_zh = contains_cjk(instruction)
            || existing_title.is_some_and(contains_cjk);
        let instr_seed = derive_deterministic_seed(
            instruction,
            existing_manifest.kind,
            Some(existing_manifest.seed),
        );

        // Case 1: Explicit modality or archetype switch
        let wants_switch = mods
            .kind_switch
            .is_some_and(|k| k != existing_manifest.kind)
            || mods.archetype_switch.is_some();
        if wants_switch {
            return self.remix_case1_switch(
                existing_manifest,
                instruction,
                existing_title,
                existing_tags,
                &mods,
                instr_seed,
            );
        }

        // Case 2: Existing script starts with genuine `@cs1-params` or `@cs1-remix-params` header
        if let Some(is_remix_hdr) = detect_header_marker(existing_source) {
            if let Some(draft) = self.remix_case2_header_update(
                existing_source,
                existing_manifest,
                instruction,
                existing_title,
                existing_tags,
                &mods,
                instr_seed,
                is_zh,
                is_remix_hdr,
            ) {
                return draft;
            }
        }

        // Case 3: Arbitrary valid cs1 script (e.g. all 18 official M1 seeds or custom scripts)
        if let Some(draft) = self.remix_case3_wrap_arbitrary(
            existing_source,
            existing_manifest,
            instruction,
            existing_title,
            existing_tags,
            &mods,
            instr_seed,
            is_zh,
        ) {
            return draft;
        }

        // Safe fallback when input source is syntactically broken or exceeds budgets
        self.remix_case1_switch(
            existing_manifest,
            instruction,
            existing_title,
            existing_tags,
            &mods,
            instr_seed,
        )
    }
}

impl DeterministicProceduralSynthesizer {
    fn remix_case1_switch(
        &self,
        existing_manifest: &AssetManifest,
        instruction: &str,
        existing_title: Option<&str>,
        existing_tags: &[String],
        mods: &RemixModifiers,
        instr_seed: u64,
    ) -> SynthesizedDraft {
        let target_kind = mods
            .kind_switch
            .or_else(|| mods.archetype_switch.map(SceneArchetype::kind))
            .unwrap_or(existing_manifest.kind);

        let style_tags: Vec<String> = if mods.palette_override.is_some() {
            existing_tags
                .iter()
                .filter(|t| !is_palette_associated_tag(&normalize_tag(t)))
                .cloned()
                .collect()
        } else {
            existing_tags.to_vec()
        };

        let req = SynthRequest {
            prompt: instruction.to_string(),
            kind: Some(target_kind),
            style_tags,
            seed: Some(instr_seed),
        };
        let mut intent = IntentAnalyzer::analyze_create(&req);
        if let Some(arch) = mods.archetype_switch {
            if arch.kind() == target_kind {
                intent.archetype = arch;
            }
        }
        if let Some(pal) = mods.palette_override {
            let pal_tag = normalize_tag(pal.spec().tag);
            intent.palette = pal;
            intent.modifiers.palette_override = Some(pal);
            intent.modifiers.hue_override = mods
                .hue_override
                .filter(|v| v.is_finite())
                .or(Some(pal.spec().primary_hue));
            intent.tags.retain(|t| {
                let nt = normalize_tag(t);
                nt == pal_tag
                    || !ColorPaletteTheme::ALL
                        .iter()
                        .any(|other| *other != pal && other.spec().tag == nt)
            });
            if !pal_tag.is_empty() && !intent.tags.iter().any(|t| normalize_tag(t) == pal_tag) {
                if intent.tags.len() >= 8 {
                    intent.tags.pop();
                }
                intent.tags.push(pal_tag);
            }
        }
        if target_kind == AssetKind::Video {
            let raw_dur = mods
                .duration_sec
                .filter(|d| d.is_finite())
                .or_else(|| existing_manifest.duration_sec.filter(|d| d.is_finite()))
                .or_else(|| intent.archetype.default_duration_sec())
                .unwrap_or(15.0);
            intent.duration_sec = Some(finite_or(raw_dur, 15.0).clamp(5.0, 120.0));
        } else {
            intent.duration_sec = None;
        }
        if let Some(t) = existing_title.map(str::trim).filter(|s| !s.is_empty()) {
            if instruction.trim().is_empty() {
                intent.title = format!("{t} · Remix");
            }
        }
        let mut draft = self.synthesize_from_intent(&intent);
        draft.engine_used = ENGINE_PROCEDURAL_REMIX.to_string();
        draft
    }

    #[allow(clippy::too_many_arguments)]
    fn remix_case2_header_update(
        &self,
        existing_source: &str,
        existing_manifest: &AssetManifest,
        instruction: &str,
        existing_title: Option<&str>,
        existing_tags: &[String],
        mods: &RemixModifiers,
        instr_seed: u64,
        is_zh: bool,
        is_remix_hdr: bool,
    ) -> Option<SynthesizedDraft> {
        let mut archetype = parse_header_token(existing_source, "archetype=")
            .and_then(SceneArchetype::from_str_id)
            .unwrap_or_else(|| fallback_archetype(existing_manifest.kind));
        let mut palette = parse_header_token(existing_source, "palette=")
            .and_then(ColorPaletteTheme::from_str_id)
            .unwrap_or(ColorPaletteTheme::CyberpunkNeon);
        let rmx_prefix = parse_header_token(existing_source, "prefix=")
            .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .unwrap_or("__CS1_RMX");

        if let Some(pal_override) = mods.palette_override {
            palette = pal_override;
        }
        if archetype.kind() != existing_manifest.kind {
            archetype = fallback_archetype(existing_manifest.kind);
        }

        let pal_spec = palette.spec();
        let nudge = !mods.has_any_explicit_modifier;
        let phase_nudge = (((instr_seed % 314) as f64) / 100.0) + 0.45;
        let mod_speed = finite_or(mods.speed_mul, 1.0);
        let mod_hue = mods.hue_override.filter(|v| v.is_finite());
        let mod_density = finite_or(mods.density_delta, 0.0);
        let mod_diff = finite_or(mods.difficulty_delta, 0.0);
        let mod_particles = finite_or(mods.particle_boost, 0.0);
        let mod_glow = finite_or(mods.glow_boost, 0.0);

        let rmx_speed_var = format!("{rmx_prefix}_SPEED");
        let rmx_hue_var = format!("{rmx_prefix}_HUE");
        let rmx_tint_var = format!("{rmx_prefix}_TINT");
        let rmx_particles_var = format!("{rmx_prefix}_PARTICLES");
        let rmx_obstacles_var = format!("{rmx_prefix}_OBSTACLES");
        let rmx_glow_var = format!("{rmx_prefix}_GLOW");
        let rmx_phase_var = format!("{rmx_prefix}_PHASE");

        let cleaned_source = existing_source.trim_start_matches('\u{feff}');
        let mut updated_lines: Vec<String> = Vec::new();
        let mut updated_param_count = 0usize;
        let mut marker_rewritten = false;

        for line in cleaned_source.lines() {
            let trimmed = line.trim_start();
            if !marker_rewritten && !is_remix_hdr && trimmed.starts_with(PARAMS_MARKER) {
                marker_rewritten = true;
                updated_lines.push(format!(
                    "{PARAMS_MARKER} archetype={} palette={}",
                    archetype.as_str(),
                    palette.as_str()
                ));
                continue;
            }
            if !marker_rewritten && is_remix_hdr && trimmed.starts_with(REMIX_MARKER) {
                marker_rewritten = true;
                if rmx_prefix == "__CS1_RMX" {
                    updated_lines.push(format!("{REMIX_MARKER} palette={}", palette.as_str()));
                } else {
                    updated_lines.push(format!(
                        "{REMIX_MARKER} palette={} prefix={rmx_prefix}",
                        palette.as_str()
                    ));
                }
                continue;
            }

            if !is_remix_hdr {
                if let Some(val) = parse_let_f64(trimmed, "SPEED_MUL") {
                    let nv = (val * mod_speed).clamp(0.4, 2.5);
                    updated_lines.push(format!("let SPEED_MUL = {nv:.2};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, "PALETTE_HUE") {
                    let nv = if let Some(h) = mod_hue {
                        h.rem_euclid(360.0)
                    } else if nudge {
                        (val + 48.0).rem_euclid(360.0)
                    } else {
                        val
                    };
                    updated_lines.push(format!("let PALETTE_HUE = {nv:.1};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, "ACCENT_HUE") {
                    let nv = if mods.palette_override.is_some() {
                        pal_spec.accent_hue.rem_euclid(360.0)
                    } else if nudge {
                        (val + 36.0).rem_euclid(360.0)
                    } else {
                        val
                    };
                    updated_lines.push(format!("let ACCENT_HUE = {nv:.1};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, "SHADER_PAL") {
                    let nv = if mods.palette_override.is_some() {
                        pal_spec.shader_pal as i32
                    } else if nudge {
                        ((val.round() as i32) + 1).rem_euclid(4)
                    } else {
                        (val.round() as i32).rem_euclid(4)
                    };
                    updated_lines.push(format!("let SHADER_PAL = {nv};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, "DENSITY") {
                    let nv = (val + mod_density + if nudge { 0.15 } else { 0.0 })
                        .clamp(0.5, 2.2);
                    updated_lines.push(format!("let DENSITY = {nv:.2};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, "OBSTACLES") {
                    let nv = (val.round() as i32)
                        .saturating_add(mods.obstacle_delta)
                        .clamp(1, 10);
                    updated_lines.push(format!("let OBSTACLES = {nv};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, "DIFFICULTY") {
                    let nv = (val + mod_diff).clamp(0.5, 2.2);
                    updated_lines.push(format!("let DIFFICULTY = {nv:.2};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, "PARTICLES") {
                    let delta = (mod_particles * 14.0).round() as i32
                        + if nudge { 6 } else { 0 };
                    let nv = (val.round() as i32).saturating_add(delta).clamp(4, 48);
                    updated_lines.push(format!("let PARTICLES = {nv};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, "GLOW_LEVEL") {
                    let nv = (val + mod_glow * 0.45 + if nudge { 0.20 } else { 0.0 })
                        .clamp(0.6, 2.0);
                    updated_lines.push(format!("let GLOW_LEVEL = {nv:.2};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, "SEED_PHASE") {
                    let nv = (val + phase_nudge).rem_euclid(std::f64::consts::TAU);
                    updated_lines.push(format!("let SEED_PHASE = {nv:.2};"));
                    updated_param_count += 1;
                    continue;
                }
            } else {
                if let Some(val) = parse_let_f64(trimmed, &rmx_speed_var) {
                    let nv = (val * mod_speed).clamp(0.4, 2.5);
                    updated_lines.push(format!("let {rmx_speed_var} = {nv:.2};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, &rmx_hue_var) {
                    let nv = if let Some(h) = mod_hue {
                        h.rem_euclid(360.0)
                    } else if nudge {
                        (val + 52.0).rem_euclid(360.0)
                    } else {
                        val
                    };
                    updated_lines.push(format!("let {rmx_hue_var} = {nv:.1};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, &rmx_tint_var) {
                    let nv = if mods.palette_override.is_some() || nudge {
                        (val + 0.08).clamp(0.14, 0.28)
                    } else {
                        val
                    };
                    updated_lines.push(format!("let {rmx_tint_var} = {nv:.2};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, &rmx_particles_var) {
                    let delta = (mod_particles * 12.0).round() as i32
                        + if nudge { 6 } else { 0 };
                    let nv = (val.round() as i32).saturating_add(delta).clamp(0, 32);
                    updated_lines.push(format!("let {rmx_particles_var} = {nv};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, &rmx_obstacles_var) {
                    let nv = (val.round() as i32)
                        .saturating_add(mods.obstacle_delta)
                        .clamp(0, 8);
                    updated_lines.push(format!("let {rmx_obstacles_var} = {nv};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, &rmx_glow_var) {
                    let nv = (val + mod_glow * 0.40 + if nudge { 0.20 } else { 0.0 })
                        .clamp(0.6, 2.0);
                    updated_lines.push(format!("let {rmx_glow_var} = {nv:.2};"));
                    updated_param_count += 1;
                    continue;
                }
                if let Some(val) = parse_let_f64(trimmed, &rmx_phase_var) {
                    let nv = (val + phase_nudge).rem_euclid(std::f64::consts::TAU);
                    updated_lines.push(format!("let {rmx_phase_var} = {nv:.2};"));
                    updated_param_count += 1;
                    continue;
                }
            }

            updated_lines.push(line.to_string());
        }

        if updated_param_count == 0 {
            return None;
        }

        let new_source = updated_lines.join("\n");
        let mut manifest = existing_manifest.clone();
        if manifest.kind == AssetKind::Video {
            if let Some(dur) = mods.duration_sec.filter(|d| d.is_finite()) {
                manifest.duration_sec = Some(dur.clamp(5.0, 120.0));
            }
        }
        manifest.seed = instr_seed;
        manifest.clamp_in_place();

        let validation = validate_to_report(&new_source, &manifest);
        if !validation.ok {
            return None;
        }

        let title = build_remix_title(existing_title, instruction, archetype, is_zh);
        let tags = merge_remix_tags(existing_tags, palette, mods);

        Some(SynthesizedDraft {
            title,
            description: format!("Remix: {}", truncate_chars(instruction.trim(), 120)),
            kind: manifest.kind,
            tags,
            source_code: new_source,
            code_format: CODE_SCENE_FORMAT_CS1.to_string(),
            manifest,
            controls_hint: archetype.default_controls_hint(is_zh).to_string(),
            archetype,
            palette,
            validation,
            engine_used: ENGINE_PROCEDURAL_REMIX.to_string(),
        })
    }
}

impl DeterministicProceduralSynthesizer {
    #[allow(clippy::too_many_arguments)]
    fn remix_case3_wrap_arbitrary(
        &self,
        existing_source: &str,
        existing_manifest: &AssetManifest,
        instruction: &str,
        existing_title: Option<&str>,
        existing_tags: &[String],
        mods: &RemixModifiers,
        instr_seed: u64,
        is_zh: bool,
    ) -> Option<SynthesizedDraft> {
        let existing_source = existing_source.trim_start_matches('\u{feff}');
        let budgets = VmBudgets::default();
        let tokens = lexer::tokenize(existing_source, &budgets).ok()?;
        let program = parser::parse(&tokens, &budgets).ok()?;

        let mut defined_fns: HashSet<String> = HashSet::new();
        let mut defined_globals: HashSet<String> = HashSet::new();
        let mut on_render_arity: Option<usize> = None;
        let mut on_update_arity: Option<usize> = None;

        for item in &program.items {
            match item {
                TopItem::Fn(decl) => {
                    defined_fns.insert(decl.name.clone());
                    if decl.name == "on_render" {
                        on_render_arity = Some(decl.params.len());
                    } else if decl.name == "on_update" {
                        on_update_arity = Some(decl.params.len());
                    }
                }
                TopItem::GlobalLet { name, .. } => {
                    defined_globals.insert(name.clone());
                }
            }
        }

        let render_arity = on_render_arity?;
        let orig_render_name = unique_fn_name("__cs1_orig_render", &defined_fns);
        let orig_update_name = unique_fn_name("__cs1_orig_update", &defined_fns);
        let rmx_prefix = unique_remix_prefix(&defined_globals, &defined_fns);

        let mut renamed = rename_top_level_fn(existing_source, "on_render", &orig_render_name)?;
        if on_update_arity.is_some() {
            renamed = rename_top_level_fn(&renamed, "on_update", &orig_update_name)?;
        }

        let palette = mods.palette_override.unwrap_or(ColorPaletteTheme::SolarGold);
        let pal_spec = palette.spec();
        let speed = finite_or(mods.speed_mul, 1.0).clamp(0.4, 2.5);
        let hue = mods
            .hue_override
            .filter(|v| v.is_finite())
            .unwrap_or(pal_spec.primary_hue)
            .rem_euclid(360.0);
        let tint = if mods.palette_override.is_some() {
            0.16
        } else if !mods.has_any_explicit_modifier {
            0.12
        } else {
            0.08
        };
        let mod_particles = finite_or(mods.particle_boost, 0.0);
        let particles = if mod_particles > 0.0 {
            10i32
                .saturating_add((mod_particles * 12.0).round() as i32)
                .clamp(6, 28)
        } else if !mods.has_any_explicit_modifier {
            10
        } else {
            6
        };
        let obstacles = mods.obstacle_delta.clamp(0, 6);
        let glow = (1.20 + finite_or(mods.glow_boost, 0.0) * 0.45).clamp(0.6, 2.0);
        let phase = ((instr_seed % 6_283) as f64) / 1_000.0;

        let mut manifest = existing_manifest.clone();
        if manifest.kind == AssetKind::Video {
            if let Some(dur) = mods.duration_sec.filter(|d| d.is_finite()) {
                manifest.duration_sec = Some(dur.clamp(5.0, 120.0));
            }
        }
        manifest.seed = instr_seed;
        manifest.clamp_in_place();

        for try_particles in [particles, 0] {
            let wrapped = build_wrapped_script(
                &renamed,
                palette.as_str(),
                &rmx_prefix,
                speed,
                hue,
                tint,
                try_particles,
                obstacles,
                glow,
                phase,
                &orig_render_name,
                render_arity,
                on_update_arity.map(|a| (orig_update_name.as_str(), a)),
            );
            let validation = validate_to_report(&wrapped, &manifest);
            if validation.ok {
                let arch = fallback_archetype(manifest.kind);
                let title = build_remix_title(existing_title, instruction, arch, is_zh);
                let tags = merge_remix_tags(existing_tags, palette, mods);
                return Some(SynthesizedDraft {
                    title,
                    description: format!("Remix: {}", truncate_chars(instruction.trim(), 120)),
                    kind: manifest.kind,
                    tags,
                    source_code: wrapped,
                    code_format: CODE_SCENE_FORMAT_CS1.to_string(),
                    manifest,
                    controls_hint: arch.default_controls_hint(is_zh).to_string(),
                    archetype: arch,
                    palette,
                    validation,
                    engine_used: ENGINE_PROCEDURAL_REMIX.to_string(),
                });
            }
        }

        None
    }
}

#[allow(clippy::too_many_arguments)]
fn build_wrapped_script(
    renamed_body: &str,
    palette_id: &str,
    rmx_prefix: &str,
    speed: f64,
    hue: f64,
    tint: f64,
    particles: i32,
    obstacles: i32,
    glow: f64,
    phase: f64,
    orig_render: &str,
    render_arity: usize,
    orig_update: Option<(&str, usize)>,
) -> String {
    let marker_line = if rmx_prefix == "__CS1_RMX" {
        format!("{REMIX_MARKER} palette={palette_id}")
    } else {
        format!("{REMIX_MARKER} palette={palette_id} prefix={rmx_prefix}")
    };
    let v_speed = format!("{rmx_prefix}_SPEED");
    let v_hue = format!("{rmx_prefix}_HUE");
    let v_tint = format!("{rmx_prefix}_TINT");
    let v_particles = format!("{rmx_prefix}_PARTICLES");
    let v_obstacles = format!("{rmx_prefix}_OBSTACLES");
    let v_glow = format!("{rmx_prefix}_GLOW");
    let v_phase = format!("{rmx_prefix}_PHASE");

    let header = format!(
        "{marker_line}\n\
         let {v_speed} = {speed:.2};\n\
         let {v_hue} = {hue:.1};\n\
         let {v_tint} = {tint:.2};\n\
         let {v_particles} = {particles};\n\
         let {v_obstacles} = {obstacles};\n\
         let {v_glow} = {glow:.2};\n\
         let {v_phase} = {phase:.2};\n\n"
    );

    let update_wrapper = match orig_update {
        Some((name, arity)) => {
            let call = format_call_with_arity(name, arity, &["state", "__dt", "input"]);
            format!(
                "\nfn on_update(state, dt, input) {{\n    \
                 let __dt = min(dt * {v_speed}, 0.08);\n    \
                 {call};\n\
                 }}\n"
            )
        }
        None => String::new(),
    };

    let render_call = format_call_with_arity(orig_render, render_arity, &["state", "__t", "canvas"]);
    let render_wrapper = format!(
        "\nfn on_render(state, t, canvas) {{\n    \
         let __t = t * {v_speed};\n    \
         if canvas.duration != null && canvas.duration > 0 {{\n        \
             __t = __t % canvas.duration;\n    \
         }}\n    \
         {render_call};\n    \
         if {v_tint} > 0.01 {{\n        \
             rect(0, 0, canvas.width, canvas.height, hsla({v_hue}, 0.85, 0.48, {v_tint}));\n        \
             stroke_rect(4, 4, canvas.width - 8, canvas.height - 8, hsla({v_hue}, 0.95, 0.65, 0.72), 2.5);\n    \
         }}\n    \
         for __i in 0..{v_particles} {{\n        \
             let __a = {v_phase} + __i * 1.15 + __t * 1.2;\n        \
             let __px = canvas.width * (0.12 + 0.76 * hash2(__i + 1.0, {v_phase}));\n        \
             let __py = canvas.height * fract(hash2(__i + 5.0, 2.0) + __t * 0.22);\n        \
             glow_circle(__px + sin(__a) * 12.0, __py, 4.0, hsla(({v_hue} + __i * 14) % 360, 0.95, 0.68, 0.78), {v_glow});\n    \
         }}\n    \
         for __k in 0..{v_obstacles} {{\n        \
             let __ox = canvas.width * (0.18 + 0.64 * hash2(__k + 3.0, 9.0));\n        \
             let __oy = 110.0 + ((__k * 92.0 + __t * 60.0) % max(120.0, canvas.height - 200.0));\n        \
             stroke_circle(__ox, __oy, 14.0, hsla(({v_hue} + 140) % 360, 0.95, 0.68, 0.72), 2.2);\n    \
         }}\n\
         }}\n"
    );

    format!("{header}{renamed_body}\n{update_wrapper}{render_wrapper}")
}

fn format_call_with_arity(fn_name: &str, arity: usize, args: &[&str; 3]) -> String {
    let take = arity.min(args.len());
    format!("{fn_name}({})", args[..take].join(", "))
}

fn unique_fn_name(base: &str, existing: &HashSet<String>) -> String {
    if !existing.contains(base) {
        return base.to_string();
    }
    let mut n = 1usize;
    loop {
        let cand = format!("{base}_{n}");
        if !existing.contains(&cand) {
            return cand;
        }
        n += 1;
    }
}

fn finite_or(v: f64, default: f64) -> f64 {
    if v.is_finite() { v } else { default }
}

const RMX_GLOBAL_SUFFIXES: [&str; 7] = [
    "SPEED",
    "HUE",
    "TINT",
    "PARTICLES",
    "OBSTACLES",
    "GLOW",
    "PHASE",
];

fn unique_remix_prefix(
    defined_globals: &HashSet<String>,
    defined_fns: &HashSet<String>,
) -> String {
    let collides = |prefix: &str| {
        RMX_GLOBAL_SUFFIXES.iter().any(|suf| {
            let name = format!("{prefix}_{suf}");
            defined_globals.contains(&name) || defined_fns.contains(&name)
        })
    };
    if !collides("__CS1_RMX") {
        return "__CS1_RMX".to_string();
    }
    let mut n = 1usize;
    loop {
        let cand = format!("__CS1_RMX_{n}");
        if !collides(&cand) {
            return cand;
        }
        n += 1;
    }
}

/// Returns `Some(false)` if the first non-empty line starts with `PARAMS_MARKER`,
/// `Some(true)` if it starts with `REMIX_MARKER`, or `None` otherwise (preventing
/// decoy string literals inside script bodies from triggering Case 2).
fn detect_header_marker(src: &str) -> Option<bool> {
    let cleaned = src.trim_start_matches('\u{feff}');
    let first_non_empty = cleaned
        .lines()
        .map(str::trim_start)
        .find(|l| !l.is_empty())?;
    if first_non_empty.starts_with(PARAMS_MARKER) {
        Some(false)
    } else if first_non_empty.starts_with(REMIX_MARKER) {
        Some(true)
    } else {
        None
    }
}

/// Comment/string-aware top-level byte-offset renamer for `fn <target>(`.
///
/// Tracks brace depth `{`/`}` and ignores comments (`//`, `/* */`) and `"..."` strings
/// (including comments placed between `fn` and `<target>`) so UTF-8 characters inside
/// strings or comments NEVER shift byte offsets.
fn rename_top_level_fn(src: &str, target: &str, replacement: &str) -> Option<String> {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut brace_depth = 0usize;

    while i < len {
        // Line comment
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Block comment
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(len);
            continue;
        }
        // Double-quoted string
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
            continue;
        }

        if bytes[i] == b'{' {
            brace_depth += 1;
            i += 1;
            continue;
        }
        if bytes[i] == b'}' {
            brace_depth = brace_depth.saturating_sub(1);
            i += 1;
            continue;
        }

        if brace_depth == 0 && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
            let ident_start = i;
            i += 1;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if &src[ident_start..i] == "fn" {
                let mut j = i;
                loop {
                    while j < len && bytes[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j + 1 < len && bytes[j] == b'/' && bytes[j + 1] == b'/' {
                        j += 2;
                        while j < len && bytes[j] != b'\n' {
                            j += 1;
                        }
                        continue;
                    }
                    if j + 1 < len && bytes[j] == b'/' && bytes[j + 1] == b'*' {
                        j += 2;
                        while j + 1 < len && !(bytes[j] == b'*' && bytes[j + 1] == b'/') {
                            j += 1;
                        }
                        j = (j + 2).min(len);
                        continue;
                    }
                    break;
                }
                let name_start = j;
                if j < len && (bytes[j].is_ascii_alphabetic() || bytes[j] == b'_') {
                    j += 1;
                    while j < len && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                        j += 1;
                    }
                    if &src[name_start..j] == target {
                        let mut out = String::with_capacity(src.len() + replacement.len());
                        out.push_str(&src[..name_start]);
                        out.push_str(replacement);
                        out.push_str(&src[j..]);
                        return Some(out);
                    }
                }
            }
            continue;
        }

        i += 1;
    }

    None
}

fn parse_header_token<'a>(src: &'a str, key: &str) -> Option<&'a str> {
    let cleaned = src.trim_start_matches('\u{feff}');
    let first_line = cleaned
        .lines()
        .map(str::trim_start)
        .find(|l| !l.is_empty())?;
    let idx = first_line.find(key)?;
    let rest = &first_line[idx + key.len()..];
    rest.split_whitespace().next()
}

fn parse_let_f64(trimmed_line: &str, var_name: &str) -> Option<f64> {
    let rest = trimmed_line.strip_prefix("let ")?.trim_start();
    let rest = rest.strip_prefix(var_name)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let num_str = rest.trim_end_matches(';').trim();
    num_str.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn fallback_archetype(kind: AssetKind) -> SceneArchetype {
    match kind {
        AssetKind::Video => SceneArchetype::VideoStarWarpReel,
        AssetKind::Animation => SceneArchetype::AnimFluidPlasma,
        AssetKind::Game => SceneArchetype::GameNeonPinballBreakout,
    }
}

fn build_remix_title(
    existing_title: Option<&str>,
    instruction: &str,
    arch: SceneArchetype,
    is_zh: bool,
) -> String {
    let base = existing_title
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.strip_suffix(" · Remix").unwrap_or(s))
        .unwrap_or_else(|| arch.default_title(is_zh));
    let hint = truncate_chars(instruction.trim(), 16);
    if hint.is_empty() {
        format!("{base} · Remix")
    } else {
        format!("{base} ({hint})")
    }
}

fn merge_remix_tags(
    existing_tags: &[String],
    palette: ColorPaletteTheme,
    mods: &RemixModifiers,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for t in existing_tags {
        let n = normalize_tag(t);
        if !n.is_empty() && seen.insert(n.clone()) {
            out.push(n);
        }
    }
    let pt = normalize_tag(palette.spec().tag);
    if seen.insert(pt.clone()) {
        out.push(pt);
    }
    if mods.particle_boost > 0.0 && seen.insert("particles".to_string()) {
        out.push("particles".to_string());
    }
    if seen.insert("remix".to_string()) {
        out.push("remix".to_string());
    }
    out.truncate(8);
    out
}

fn is_palette_associated_tag(norm_tag: &str) -> bool {
    ColorPaletteTheme::ALL
        .iter()
        .any(|p| p.spec().tag == norm_tag)
        || matches!(
            norm_tag,
            "purple"
                | "violet"
                | "amethyst"
                | "lavender"
                | "aurora"
                | "gold"
                | "golden"
                | "solar"
                | "amber"
                | "yellow"
                | "emerald"
                | "green"
                | "matrix"
                | "lime"
                | "ocean"
                | "teal"
                | "aqua"
                | "marine"
                | "deepsea"
                | "blue"
                | "sunset"
                | "synthwave"
                | "vaporwave"
                | "magenta"
                | "pink"
                | "crimson"
                | "red"
                | "ruby"
                | "flame"
                | "fire"
                | "frost"
                | "minimal"
                | "ice"
                | "silver"
                | "white"
                | "clean"
                | "cyberpunk"
                | "cyber"
                | "neon"
        )
}
