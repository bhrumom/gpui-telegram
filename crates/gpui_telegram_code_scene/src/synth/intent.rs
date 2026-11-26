//! Bilingual (`zh-CN` & `en`) creation and remix intent analyzer (`CS-F08`).

use std::collections::HashSet;

use gpui_telegram_proto::AssetKind;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::reco::embedding::normalize_tag;
use crate::synth::SynthRequest;

/// 15 genuinely distinct procedural `cs1` archetypes (5 Videos, 5 Animations, 5 Games).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneArchetype {
    // 5 Code Videos (pure-timeline O(1) seek)
    VideoStarWarpReel,
    VideoCyberMetropolis,
    VideoKineticTypography,
    VideoAudioSpectrumShow,
    VideoSynthwaveRoadtrip,
    // 5 Code Animations (60fps pointer & click reactive)
    AnimFluidPlasma,
    AnimParticleFlowField,
    AnimFractalMandala,
    AnimBiolumVoronoi,
    AnimHarmonicWaves,
    // 5 Playable Games (keyboard + pointer, collision, score/high_score/game_over/status_text)
    GameNeonPinballBreakout,
    GameSpaceShooter,
    GameCyberRunnerDodge,
    GameFlappyCyber,
    GameNeonSnake,
}

impl SceneArchetype {
    pub const ALL: [Self; 15] = [
        Self::VideoStarWarpReel,
        Self::VideoCyberMetropolis,
        Self::VideoKineticTypography,
        Self::VideoAudioSpectrumShow,
        Self::VideoSynthwaveRoadtrip,
        Self::AnimFluidPlasma,
        Self::AnimParticleFlowField,
        Self::AnimFractalMandala,
        Self::AnimBiolumVoronoi,
        Self::AnimHarmonicWaves,
        Self::GameNeonPinballBreakout,
        Self::GameSpaceShooter,
        Self::GameCyberRunnerDodge,
        Self::GameFlappyCyber,
        Self::GameNeonSnake,
    ];

    pub const fn kind(self) -> AssetKind {
        match self {
            Self::VideoStarWarpReel
            | Self::VideoCyberMetropolis
            | Self::VideoKineticTypography
            | Self::VideoAudioSpectrumShow
            | Self::VideoSynthwaveRoadtrip => AssetKind::Video,
            Self::AnimFluidPlasma
            | Self::AnimParticleFlowField
            | Self::AnimFractalMandala
            | Self::AnimBiolumVoronoi
            | Self::AnimHarmonicWaves => AssetKind::Animation,
            Self::GameNeonPinballBreakout
            | Self::GameSpaceShooter
            | Self::GameCyberRunnerDodge
            | Self::GameFlappyCyber
            | Self::GameNeonSnake => AssetKind::Game,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VideoStarWarpReel => "video_star_warp_reel",
            Self::VideoCyberMetropolis => "video_cyber_metropolis",
            Self::VideoKineticTypography => "video_kinetic_typography",
            Self::VideoAudioSpectrumShow => "video_audio_spectrum_show",
            Self::VideoSynthwaveRoadtrip => "video_synthwave_roadtrip",
            Self::AnimFluidPlasma => "anim_fluid_plasma",
            Self::AnimParticleFlowField => "anim_particle_flow_field",
            Self::AnimFractalMandala => "anim_fractal_mandala",
            Self::AnimBiolumVoronoi => "anim_biolum_voronoi",
            Self::AnimHarmonicWaves => "anim_harmonic_waves",
            Self::GameNeonPinballBreakout => "game_neon_pinball_breakout",
            Self::GameSpaceShooter => "game_space_shooter",
            Self::GameCyberRunnerDodge => "game_cyber_runner_dodge",
            Self::GameFlappyCyber => "game_flappy_cyber",
            Self::GameNeonSnake => "game_neon_snake",
        }
    }

    pub fn from_str_id(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        Self::ALL.into_iter().find(|a| a.as_str() == trimmed)
    }

    pub const fn default_duration_sec(self) -> Option<f64> {
        match self {
            Self::VideoStarWarpReel => Some(14.0),
            Self::VideoCyberMetropolis => Some(16.0),
            Self::VideoKineticTypography => Some(12.0),
            Self::VideoAudioSpectrumShow => Some(15.0),
            Self::VideoSynthwaveRoadtrip => Some(15.0),
            _ => None,
        }
    }

    pub const fn default_tags(self) -> &'static [&'static str] {
        match self {
            Self::VideoStarWarpReel => &["space", "starfield", "scifi", "keyframe", "reel"],
            Self::VideoCyberMetropolis => &["cyberpunk", "cityscape", "neon", "keyframe", "story"],
            Self::VideoKineticTypography => &["typography", "motion_design", "minimal", "story", "art"],
            Self::VideoAudioSpectrumShow => &["music_viz", "shader", "audio_visual", "electronic", "neon"],
            Self::VideoSynthwaveRoadtrip => &["synthwave", "retro", "vaporwave", "shader", "neon"],
            Self::AnimFluidPlasma => &["shader", "plasma", "fluid", "generative", "interactive"],
            Self::AnimParticleFlowField => &["particles", "flow_field", "physics", "generative", "interactive"],
            Self::AnimFractalMandala => &["fractal", "mandelbrot", "shader", "math", "interactive"],
            Self::AnimBiolumVoronoi => &["voronoi", "shader", "ocean", "organic", "generative"],
            Self::AnimHarmonicWaves => &["waves", "matrix", "neon", "vector", "interactive"],
            Self::GameNeonPinballBreakout => &["pinball", "breakout", "arcade", "neon", "game"],
            Self::GameSpaceShooter => &["shooter", "space", "arcade", "scifi", "game"],
            Self::GameCyberRunnerDodge => &["runner", "dodge", "cyberpunk", "reflex", "game"],
            Self::GameFlappyCyber => &["flappy", "arcade", "reflex", "neon", "game"],
            Self::GameNeonSnake => &["snake", "retro", "arcade", "puzzle", "game"],
        }
    }

    pub const fn default_controls_hint(self, is_zh: bool) -> &'static str {
        if is_zh {
            match self {
                Self::VideoStarWarpReel
                | Self::VideoCyberMetropolis
                | Self::VideoKineticTypography
                | Self::VideoAudioSpectrumShow
                | Self::VideoSynthwaveRoadtrip => "拖动底部进度条精准跳转 · 空格暂停/播放 · 支持倍速",
                Self::AnimFluidPlasma
                | Self::AnimParticleFlowField
                | Self::AnimFractalMandala
                | Self::AnimBiolumVoronoi
                | Self::AnimHarmonicWaves => "移动鼠标实时扰动画面 · 点击触发能量脉冲/切换形态",
                Self::GameNeonPinballBreakout => "方向键 ←/→ 或拖动鼠标控制挡板 · 空格/点击发球",
                Self::GameSpaceShooter => "方向键 ←/→ 或移动鼠标驾驶战机 · 空格/点击发射激光",
                Self::GameCyberRunnerDodge => "方向键 ←/→ 或鼠标移动闪避障碍 · 空格/点击相位冲刺",
                Self::GameFlappyCyber => "按 ↑ / 空格 / 点击屏幕振翅飞行 · 穿越霓虹光门",
                Self::GameNeonSnake => "方向键 ↑/↓/←/→ 控制霓虹蛇转向 · 空格加速",
            }
        } else {
            match self {
                Self::VideoStarWarpReel
                | Self::VideoCyberMetropolis
                | Self::VideoKineticTypography
                | Self::VideoAudioSpectrumShow
                | Self::VideoSynthwaveRoadtrip => "Scrub timeline to seek · Space play/pause · 0.5x-2x speed",
                Self::AnimFluidPlasma
                | Self::AnimParticleFlowField
                | Self::AnimFractalMandala
                | Self::AnimBiolumVoronoi
                | Self::AnimHarmonicWaves => "Move pointer to modulate shader · Click for pulse / morph",
                Self::GameNeonPinballBreakout => "Left/Right or pointer to steer paddle · Space/Click to launch",
                Self::GameSpaceShooter => "Left/Right or pointer to pilot · Space/Click to fire lasers",
                Self::GameCyberRunnerDodge => "Left/Right or pointer to dodge · Space/Click for phase dash",
                Self::GameFlappyCyber => "Up / Space / Click to flap through neon gates",
                Self::GameNeonSnake => "Arrow keys to steer neon snake · Space for turbo boost",
            }
        }
    }

    pub const fn default_title(self, is_zh: bool) -> &'static str {
        if is_zh {
            match self {
                Self::VideoStarWarpReel => "星际跃迁代码短片",
                Self::VideoCyberMetropolis => "赛博都市霓虹短片",
                Self::VideoKineticTypography => "动态排版代码短片",
                Self::VideoAudioSpectrumShow => "律动频谱代码秀",
                Self::VideoSynthwaveRoadtrip => "合成波公路代码短片",
                Self::AnimFluidPlasma => "流光等离子流体动画",
                Self::AnimParticleFlowField => "霓虹粒子流场动画",
                Self::AnimFractalMandala => "曼德勃罗分形曼陀罗",
                Self::AnimBiolumVoronoi => "深海荧光沃罗诺伊",
                Self::AnimHarmonicWaves => "赛博谐波律动矩阵",
                Self::GameNeonPinballBreakout => "赛博霓虹弹球打砖块",
                Self::GameSpaceShooter => "星际雷霆战机射击",
                Self::GameCyberRunnerDodge => "赛博光速闪避跑酷",
                Self::GameFlappyCyber => "霓虹赛博飞鸟挑战",
                Self::GameNeonSnake => "霓虹网格贪吃蛇",
            }
        } else {
            match self {
                Self::VideoStarWarpReel => "Starlight Warp Reel",
                Self::VideoCyberMetropolis => "Cyber Metropolis Reel",
                Self::VideoKineticTypography => "Kinetic Code Typography",
                Self::VideoAudioSpectrumShow => "Audio Spectrum Live Show",
                Self::VideoSynthwaveRoadtrip => "Synthwave Horizon Cruise",
                Self::AnimFluidPlasma => "Fluid Plasma Aurora",
                Self::AnimParticleFlowField => "Neon Particle Flow Field",
                Self::AnimFractalMandala => "Fractal Mandala Abyss",
                Self::AnimBiolumVoronoi => "Bioluminescent Voronoi",
                Self::AnimHarmonicWaves => "Cyber Harmonic Waves",
                Self::GameNeonPinballBreakout => "Neon Pinball Breakout",
                Self::GameSpaceShooter => "Starfighter Space Shooter",
                Self::GameCyberRunnerDodge => "Cyber Runner Dodge",
                Self::GameFlappyCyber => "Flappy Cyber Bird",
                Self::GameNeonSnake => "Neon Grid Snake",
            }
        }
    }
}

/// 8 curated color palettes synchronized with M1 shader palette indices (`0..=3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorPaletteTheme {
    CyberpunkNeon,
    AuroraPurple,
    SunsetSynthwave,
    OceanBio,
    MatrixEmerald,
    SolarGold,
    CrimsonArcade,
    FrostMinimal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaletteSpec {
    pub primary_hue: f64,
    pub accent_hue: f64,
    pub bg_top: (u8, u8, u8),
    pub bg_bot: (u8, u8, u8),
    pub shader_pal: u8,
    pub tag: &'static str,
}

impl ColorPaletteTheme {
    pub const ALL: [Self; 8] = [
        Self::CyberpunkNeon,
        Self::AuroraPurple,
        Self::SunsetSynthwave,
        Self::OceanBio,
        Self::MatrixEmerald,
        Self::SolarGold,
        Self::CrimsonArcade,
        Self::FrostMinimal,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CyberpunkNeon => "cyberpunk_neon",
            Self::AuroraPurple => "aurora_purple",
            Self::SunsetSynthwave => "sunset_synthwave",
            Self::OceanBio => "ocean_bio",
            Self::MatrixEmerald => "matrix_emerald",
            Self::SolarGold => "solar_gold",
            Self::CrimsonArcade => "crimson_arcade",
            Self::FrostMinimal => "frost_minimal",
        }
    }

    pub fn from_str_id(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        Self::ALL.into_iter().find(|p| p.as_str() == trimmed)
    }

    pub const fn spec(self) -> PaletteSpec {
        match self {
            Self::CyberpunkNeon => PaletteSpec {
                primary_hue: 190.0,
                accent_hue: 315.0,
                bg_top: (8, 10, 28),
                bg_bot: (24, 12, 52),
                shader_pal: 1,
                tag: "cyberpunk",
            },
            Self::AuroraPurple => PaletteSpec {
                primary_hue: 278.0,
                accent_hue: 195.0,
                bg_top: (14, 8, 34),
                bg_bot: (46, 16, 76),
                shader_pal: 0,
                tag: "purple",
            },
            Self::SunsetSynthwave => PaletteSpec {
                primary_hue: 330.0,
                accent_hue: 36.0,
                bg_top: (22, 8, 42),
                bg_bot: (72, 22, 54),
                shader_pal: 2,
                tag: "synthwave",
            },
            Self::OceanBio => PaletteSpec {
                primary_hue: 178.0,
                accent_hue: 148.0,
                bg_top: (4, 14, 28),
                bg_bot: (8, 36, 54),
                shader_pal: 3,
                tag: "ocean",
            },
            Self::MatrixEmerald => PaletteSpec {
                primary_hue: 142.0,
                accent_hue: 92.0,
                bg_top: (4, 14, 10),
                bg_bot: (8, 32, 20),
                shader_pal: 3,
                tag: "emerald",
            },
            Self::SolarGold => PaletteSpec {
                primary_hue: 44.0,
                accent_hue: 20.0,
                bg_top: (24, 14, 6),
                bg_bot: (56, 28, 12),
                shader_pal: 2,
                tag: "gold",
            },
            Self::CrimsonArcade => PaletteSpec {
                primary_hue: 350.0,
                accent_hue: 45.0,
                bg_top: (24, 6, 16),
                bg_bot: (54, 12, 28),
                shader_pal: 2,
                tag: "crimson",
            },
            Self::FrostMinimal => PaletteSpec {
                primary_hue: 205.0,
                accent_hue: 185.0,
                bg_top: (12, 18, 32),
                bg_bot: (26, 38, 60),
                shader_pal: 0,
                tag: "minimal",
            },
        }
    }
}

/// Structured remix/iteration deltas parsed from natural language prompts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemixModifiers {
    pub speed_mul: f64,
    pub speed_explicit: bool,
    pub palette_override: Option<ColorPaletteTheme>,
    pub hue_override: Option<f64>,
    pub density_delta: f64,
    pub obstacle_delta: i32,
    pub difficulty_delta: f64,
    pub particle_boost: f64,
    pub glow_boost: f64,
    pub duration_sec: Option<f64>,
    pub title_override: Option<String>,
    pub kind_switch: Option<AssetKind>,
    pub archetype_switch: Option<SceneArchetype>,
    pub has_any_explicit_modifier: bool,
}

impl Default for RemixModifiers {
    fn default() -> Self {
        Self {
            speed_mul: 1.0,
            speed_explicit: false,
            palette_override: None,
            hue_override: None,
            density_delta: 0.0,
            obstacle_delta: 0,
            difficulty_delta: 0.0,
            particle_boost: 0.0,
            glow_boost: 0.0,
            duration_sec: None,
            title_override: None,
            kind_switch: None,
            archetype_switch: None,
            has_any_explicit_modifier: false,
        }
    }
}

/// Complete analyzed intent specification produced by `IntentAnalyzer`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneIntentSpec {
    pub raw_prompt: String,
    pub is_chinese: bool,
    pub kind: AssetKind,
    pub archetype: SceneArchetype,
    pub palette: ColorPaletteTheme,
    pub seed: u64,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub duration_sec: Option<f64>,
    pub modifiers: RemixModifiers,
}

/// Bilingual (`zh-CN` & `en`) natural-language prompt and remix intent parser.
#[derive(Debug, Clone, Default)]
pub struct IntentAnalyzer;

impl IntentAnalyzer {
    /// Analyzes a creation prompt + optional kind/style_tags/seed into a `SceneIntentSpec`.
    pub fn analyze_create(req: &SynthRequest) -> SceneIntentSpec {
        let prompt = req.prompt.trim().to_string();
        let combined = if req.style_tags.is_empty() {
            prompt.clone()
        } else {
            format!("{} {}", prompt, req.style_tags.join(" "))
        };
        let lower = combined.to_lowercase();
        let words = ascii_word_set(&lower);
        let is_chinese = contains_cjk(&prompt);

        let modifiers = parse_modifiers(&lower, &words, false);
        let explicit_kind = detect_explicit_kind(&lower, &words);
        let matched_arch = detect_archetype(&lower, &words, req.kind.or(explicit_kind));

        let kind = req
            .kind
            .or(explicit_kind)
            .or_else(|| matched_arch.map(SceneArchetype::kind))
            .unwrap_or(AssetKind::Animation);

        let seed = derive_deterministic_seed(&prompt, kind, req.seed);

        let archetype = match matched_arch {
            Some(a) if a.kind() == kind => a,
            _ => default_archetype_for_kind(kind, seed),
        };

        let palette = modifiers
            .palette_override
            .unwrap_or_else(|| default_palette_for_archetype(archetype, seed));

        let duration_sec = if kind == AssetKind::Video {
            Some(
                modifiers
                    .duration_sec
                    .or_else(|| archetype.default_duration_sec())
                    .unwrap_or(15.0)
                    .clamp(5.0, 120.0),
            )
        } else {
            None
        };

        let title = build_scene_title(&prompt, archetype, is_chinese);
        let description = if prompt.is_empty() {
            format!(
                "AI Bot generated {} ({})",
                archetype.default_title(is_chinese),
                archetype.as_str()
            )
        } else {
            truncate_chars(&prompt, 140)
        };

        let mut tags = Vec::new();
        let mut seen = HashSet::new();
        for &t in archetype.default_tags() {
            let norm = normalize_tag(t);
            if seen.insert(norm.clone()) {
                tags.push(norm);
            }
        }
        let pal_tag = normalize_tag(palette.spec().tag);
        if seen.insert(pal_tag.clone()) {
            tags.push(pal_tag);
        }
        for raw_tag in &req.style_tags {
            let norm = normalize_tag(raw_tag);
            if !norm.is_empty() && seen.insert(norm.clone()) && tags.len() < 8 {
                tags.push(norm);
            }
        }

        SceneIntentSpec {
            raw_prompt: prompt,
            is_chinese,
            kind,
            archetype,
            palette,
            seed,
            title,
            description,
            tags,
            duration_sec,
            modifiers,
        }
    }

    /// Analyzes a remix/iteration instruction against an existing asset's modality.
    pub fn analyze_remix(instruction: &str, existing_kind: AssetKind) -> RemixModifiers {
        let lower = instruction.trim().to_lowercase();
        let words = ascii_word_set(&lower);
        let mut mods = parse_modifiers(&lower, &words, true);

        let clauses = extract_switch_target_clauses(&lower);
        if !clauses.is_empty() {
            for target_clause in clauses.iter().rev() {
                let target_words = ascii_word_set(target_clause);
                let target_kind = detect_explicit_kind(target_clause, &target_words);
                if let Some(k) = target_kind {
                    if k != existing_kind {
                        mods.kind_switch = Some(k);
                        mods.has_any_explicit_modifier = true;
                        if let Some(a) = detect_archetype(target_clause, &target_words, Some(k)) {
                            if a.kind() == k {
                                mods.archetype_switch = Some(a);
                            }
                        }
                        break;
                    } else if let Some(a) =
                        detect_dedicated_switch_archetype(target_clause, &target_words)
                    {
                        if a.kind() == k {
                            mods.kind_switch = Some(k);
                            mods.archetype_switch = Some(a);
                            mods.has_any_explicit_modifier = true;
                            break;
                        }
                    }
                } else if let Some(a) =
                    detect_dedicated_switch_archetype(target_clause, &target_words)
                {
                    mods.archetype_switch = Some(a);
                    mods.kind_switch = Some(a.kind());
                    mods.has_any_explicit_modifier = true;
                    break;
                }
            }
        } else if let Some(k) = detect_explicit_kind(&lower, &words) {
            if k != existing_kind {
                // Only treat as explicit modality switch without a cue when accompanied
                // by a dedicated non-palette/non-modifier archetype in the new modality.
                if let Some(a) = detect_dedicated_switch_archetype(&lower, &words) {
                    if a.kind() == k {
                        mods.kind_switch = Some(k);
                        mods.archetype_switch = Some(a);
                        mods.has_any_explicit_modifier = true;
                    }
                }
            }
        }

        mods
    }
}

pub(crate) fn derive_deterministic_seed(
    prompt: &str,
    kind: AssetKind,
    caller_seed: Option<u64>,
) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"cs1-synth-v1\0");
    hasher.update(prompt.trim().to_lowercase().as_bytes());
    hasher.update([0u8]);
    hasher.update(kind.as_str().as_bytes());
    hasher.update([0u8]);
    hasher.update(caller_seed.unwrap_or(0).to_le_bytes());
    let digest = hasher.finalize();
    let val = u64::from_le_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ]);
    if val == 0 { 1 } else { val }
}

pub(crate) fn contains_cjk(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}'))
}

pub(crate) fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in s.chars().enumerate() {
        if idx >= max_chars {
            break;
        }
        out.push(ch);
    }
    out
}

fn ascii_word_set(lower: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut cur = String::new();
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            cur.push(ch);
        } else if !cur.is_empty() {
            out.insert(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.insert(cur);
    }
    out
}

fn ascii_word_spans(lower: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    for (idx, ch) in lower.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if start.is_none() {
                start = Some(idx);
            }
        } else if let Some(s) = start.take() {
            spans.push((s, idx));
        }
    }
    if let Some(s) = start {
        spans.push((s, lower.len()));
    }
    spans
}

fn is_english_switch_verb(word: &str) -> bool {
    matches!(
        word,
        "turn" | "convert" | "change" | "switch" | "transform" | "make" | "rewrite" | "remake"
    )
}

fn has_clause_boundary_punct(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            ',' | ';' | '.' | '!' | '?' | '\n' | '，' | '。' | '；' | '！' | '？'
        )
    })
}

/// Extracts all post-cue target clauses across Chinese and English switch cues
/// sorted by cue appearance order, slicing each clause up to the next cue start.
fn extract_switch_target_clauses(lower: &str) -> Vec<&str> {
    let zh_cues = ["重构为", "改写成", "改成", "换成", "变成", "转为", "改为", "变为"];
    let mut raw_cues: Vec<(usize, usize)> = Vec::new();

    for &cue in &zh_cues {
        let mut search_from = 0usize;
        while let Some(rel) = lower[search_from..].find(cue) {
            let start = search_from + rel;
            let end = start + cue.len();
            raw_cues.push((start, end));
            search_from = end;
        }
    }

    let spans = ascii_word_spans(lower);
    for (idx, &(start, end)) in spans.iter().enumerate() {
        let word = &lower[start..end];
        if word == "make" && idx + 2 < spans.len() {
            let (s1, e1) = spans[idx + 1];
            let (s2, e2) = spans[idx + 2];
            if &lower[s1..e1] == "it"
                && matches!(&lower[s2..e2], "a" | "an")
                && lower[end..s1].chars().all(|c| c.is_ascii_whitespace())
                && lower[e1..s2].chars().all(|c| c.is_ascii_whitespace())
            {
                raw_cues.push((start, e2));
                continue;
            }
        }

        let allowed_preps: &[&str] = match word {
            "turn" | "convert" | "change" | "switch" | "transform" => &["into", "to"],
            "make" => &["into"],
            "rewrite" | "remake" => &["as", "into", "to"],
            _ => continue,
        };
        let mut prev_end = end;
        for &(p_start, p_end) in spans.iter().skip(idx + 1).take(8) {
            if has_clause_boundary_punct(&lower[prev_end..p_start]) {
                break;
            }
            let pw = &lower[p_start..p_end];
            if allowed_preps.contains(&pw) {
                raw_cues.push((start, p_end));
                break;
            }
            if is_english_switch_verb(pw) {
                break;
            }
            prev_end = p_end;
        }
    }

    if raw_cues.is_empty() {
        return Vec::new();
    }

    raw_cues.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    let mut non_overlapping: Vec<(usize, usize)> = Vec::with_capacity(raw_cues.len());
    let mut last_end = 0usize;
    for (cue_start, clause_start) in raw_cues {
        if cue_start >= last_end {
            non_overlapping.push((cue_start, clause_start));
            last_end = clause_start;
        }
    }

    let mut clauses = Vec::with_capacity(non_overlapping.len());
    for i in 0..non_overlapping.len() {
        let clause_start = non_overlapping[i].1;
        let clause_end = if i + 1 < non_overlapping.len() {
            non_overlapping[i + 1].0
        } else {
            lower.len()
        };
        if clause_start <= clause_end {
            clauses.push(&lower[clause_start..clause_end]);
        }
    }
    clauses
}

fn has_any_word(words: &HashSet<String>, list: &[&str]) -> bool {
    list.iter().any(|&w| words.contains(w))
}

fn has_any_substr(lower: &str, list: &[&str]) -> bool {
    list.iter().any(|&p| lower.contains(p))
}

fn detect_explicit_kind(lower: &str, words: &HashSet<String>) -> Option<AssetKind> {
    if has_any_substr(lower, &["小游戏", "游戏", "游玩", "闯关", "街机"])
        || has_any_word(words, &["game", "games", "playable", "arcade", "minigame"])
    {
        return Some(AssetKind::Game);
    }
    if has_any_substr(lower, &["代码视频", "视频", "短片", "预告", "片头", "剪辑", "电影感"])
        || has_any_word(words, &["video", "videos", "reel", "film", "trailer", "cinema", "movie"])
    {
        return Some(AssetKind::Video);
    }
    if has_any_substr(lower, &["代码动画", "动画", "动效", "着色器", "壁纸", "屏保", "视觉艺术"])
        || has_any_word(words, &["animation", "animations", "shader", "wallpaper", "visualizer", "generative"])
    {
        return Some(AssetKind::Animation);
    }
    None
}

fn detect_archetype(
    lower: &str,
    words: &HashSet<String>,
    preferred_kind: Option<AssetKind>,
) -> Option<SceneArchetype> {
    let scored: [(SceneArchetype, i32); 15] = [
        (
            SceneArchetype::GameNeonPinballBreakout,
            score_hits(
                lower,
                words,
                &["弹球", "打砖块", "砖块", "弹珠", "pinball", "brick breaker", "block breaker"],
                &["pinball", "breakout", "brick", "bricks", "paddle", "pong"],
            ),
        ),
        (
            SceneArchetype::GameSpaceShooter,
            score_hits(
                lower,
                words,
                &["射击", "战机", "飞船", "星际射击", "space shooter", "laser shooter"],
                &["shooter", "starfighter", "spaceship", "galaga", "invaders", "laser", "blaster"],
            ),
        ),
        (
            SceneArchetype::GameCyberRunnerDodge,
            score_hits(
                lower,
                words,
                &["跑酷", "躲避", "闪避", "极速闪避", "cyber runner", "endless runner"],
                &["runner", "dodge", "dodger", "dash", "highway", "obstacles"],
            ),
        ),
        (
            SceneArchetype::GameFlappyCyber,
            score_hits(
                lower,
                words,
                &["飞鸟", "像素鸟", "管道", "振翅", "flappy bird"],
                &["flappy", "bird", "flap", "pipes"],
            ),
        ),
        (
            SceneArchetype::GameNeonSnake,
            score_hits(
                lower,
                words,
                &["贪吃蛇", "霓虹蛇", "neon snake"],
                &["snake", "serpent", "worm"],
            ),
        ),
        (
            SceneArchetype::AnimFluidPlasma,
            score_hits(
                lower,
                words,
                &["流体", "等离子", "流光", "液态", "极光", "fluid plasma", "neon plasma"],
                &["fluid", "plasma", "aurora", "liquid", "lava", "nebula"],
            ),
        ),
        (
            SceneArchetype::AnimParticleFlowField,
            score_hits(
                lower,
                words,
                &["粒子", "流场", "萤火虫", "星尘", "群聚", "flow field", "particle swarm"],
                &["particles", "particle", "boids", "swarm", "firefly", "stardust"],
            ),
        ),
        (
            SceneArchetype::AnimFractalMandala,
            score_hits(
                lower,
                words,
                &["分形", "曼德勃罗", "曼陀罗", "万花筒", "fractal mandala"],
                &["fractal", "mandelbrot", "julia", "mandala", "kaleidoscope", "sacred"],
            ),
        ),
        (
            SceneArchetype::AnimBiolumVoronoi,
            score_hits(
                lower,
                words,
                &["水母", "深海", "细胞", "沃罗诺伊", "生物荧光", "海洋"],
                &["voronoi", "jellyfish", "ocean", "reef", "bioluminescent", "organic", "coral"],
            ),
        ),
        (
            SceneArchetype::AnimHarmonicWaves,
            score_hits(
                lower,
                words,
                &["黑客帝国", "代码雨", "谐波", "波浪", "波纹", "矩阵", "matrix rain"],
                &["waves", "wave", "harmonic", "matrix", "ribbon", "sine", "oscilloscope"],
            ),
        ),
        (
            SceneArchetype::VideoStarWarpReel,
            score_hits(
                lower,
                words,
                &["星空", "穿梭", "星际", "跃迁", "宇宙", "太空", "star warp", "starfield"],
                &["starfield", "warp", "galaxy", "space", "cosmos", "stars", "hyperdrive"],
            ),
        ),
        (
            SceneArchetype::VideoCyberMetropolis,
            score_hits(
                lower,
                words,
                &["都市", "城市", "雨夜", "高楼", "摩天楼", "黎明", "cyber city"],
                &["city", "cityscape", "metropolis", "skyline", "rain", "dawn"],
            ),
        ),
        (
            SceneArchetype::VideoKineticTypography,
            score_hits(
                lower,
                words,
                &["排版", "字体", "文字", "诗歌", "包豪斯", "极简", "kinetic typography", "motion design"],
                &["typography", "kinetic", "bauhaus", "minimal", "editorial", "titles", "poetry"],
            ),
        ),
        (
            SceneArchetype::VideoAudioSpectrumShow,
            score_hits(
                lower,
                words,
                &["频谱", "音乐", "节拍", "电音", "声波", "律动", "audio spectrum", "music visualizer"],
                &["spectrum", "music", "audio", "beat", "equalizer", "dj", "bpm", "electronic"],
            ),
        ),
        (
            SceneArchetype::VideoSynthwaveRoadtrip,
            score_hits(
                lower,
                words,
                &["合成波", "蒸汽波", "公路", "落日", "地平线", "夕阳", "synthwave", "vaporwave"],
                &["synthwave", "vaporwave", "horizon", "roadtrip", "sunset", "retro", "outrun"],
            ),
        ),
    ];

    let mut best: Option<(SceneArchetype, i32)> = None;
    for (arch, base_score) in scored {
        if base_score <= 0 {
            continue;
        }
        let kind_bonus = if preferred_kind == Some(arch.kind()) {
            2
        } else {
            0
        };
        let total = base_score + kind_bonus;
        if best.is_none_or(|(_, bs)| total > bs) {
            best = Some((arch, total));
        }
    }
    best.map(|(a, _)| a)
}

/// Detects ONLY dedicated non-palette, non-modifier archetype subjects for remix switching.
///
/// Deliberately excludes palette words (`深海`, `极光`, `落日`, `合成波`, `极简`, `冰霜`,
/// `ocean`, `aurora`, `sunset`, `synthwave`, `minimal`, `frost`, `matrix`) and modifier
/// words (`粒子`, `particles`, `障碍物`, `obstacles`, `发光`, `glow`) so palette- or
/// modifier-only remixes never hijack an existing scene's modality or archetype.
fn detect_dedicated_switch_archetype(
    lower: &str,
    words: &HashSet<String>,
) -> Option<SceneArchetype> {
    let scored: [(SceneArchetype, i32); 15] = [
        (
            SceneArchetype::GameNeonPinballBreakout,
            score_hits(
                lower,
                words,
                &["弹球", "打砖块", "砖块", "弹珠", "pinball", "brick breaker", "block breaker"],
                &["pinball", "breakout", "brick", "bricks", "paddle", "pong"],
            ),
        ),
        (
            SceneArchetype::GameSpaceShooter,
            score_hits(
                lower,
                words,
                &["射击", "战机", "飞船", "星际射击", "space shooter", "laser shooter"],
                &["shooter", "starfighter", "spaceship", "galaga", "invaders", "blaster"],
            ),
        ),
        (
            SceneArchetype::GameCyberRunnerDodge,
            score_hits(
                lower,
                words,
                &["跑酷", "躲避", "闪避", "极速闪避", "cyber runner", "endless runner"],
                &["runner", "dodge", "dodger", "highway"],
            ),
        ),
        (
            SceneArchetype::GameFlappyCyber,
            score_hits(
                lower,
                words,
                &["飞鸟", "像素鸟", "管道", "振翅", "flappy bird"],
                &["flappy", "bird", "flap", "pipes"],
            ),
        ),
        (
            SceneArchetype::GameNeonSnake,
            score_hits(
                lower,
                words,
                &["贪吃蛇", "霓虹蛇", "neon snake"],
                &["snake", "serpent", "worm"],
            ),
        ),
        (
            SceneArchetype::AnimFluidPlasma,
            score_hits(
                lower,
                words,
                &["流体", "等离子", "fluid plasma", "neon plasma"],
                &["fluid", "plasma", "lava"],
            ),
        ),
        (
            SceneArchetype::AnimParticleFlowField,
            score_hits(
                lower,
                words,
                &["流场", "萤火虫", "群聚", "flow field", "particle swarm"],
                &["boids", "swarm", "firefly"],
            ),
        ),
        (
            SceneArchetype::AnimFractalMandala,
            score_hits(
                lower,
                words,
                &["分形", "曼德勃罗", "曼陀罗", "万花筒", "fractal mandala"],
                &["fractal", "mandelbrot", "julia", "mandala", "kaleidoscope"],
            ),
        ),
        (
            SceneArchetype::AnimBiolumVoronoi,
            score_hits(
                lower,
                words,
                &["水母", "细胞", "沃罗诺伊", "生物荧光"],
                &["voronoi", "jellyfish", "bioluminescent", "coral"],
            ),
        ),
        (
            SceneArchetype::AnimHarmonicWaves,
            score_hits(
                lower,
                words,
                &["黑客帝国", "代码雨", "谐波", "波浪", "波纹", "matrix rain"],
                &["waves", "harmonic", "oscilloscope"],
            ),
        ),
        (
            SceneArchetype::VideoStarWarpReel,
            score_hits(
                lower,
                words,
                &["星空穿梭", "星空", "星际跃迁", "跃迁", "star warp", "starfield"],
                &["starfield", "warp", "hyperdrive"],
            ),
        ),
        (
            SceneArchetype::VideoCyberMetropolis,
            score_hits(
                lower,
                words,
                &["摩天楼", "都市雨夜", "cyber city"],
                &["cityscape", "metropolis", "skyline"],
            ),
        ),
        (
            SceneArchetype::VideoKineticTypography,
            score_hits(
                lower,
                words,
                &["文字排版", "动态排版", "排版", "包豪斯", "kinetic typography", "motion design"],
                &["typography", "bauhaus"],
            ),
        ),
        (
            SceneArchetype::VideoAudioSpectrumShow,
            score_hits(
                lower,
                words,
                &["频谱", "音频频谱", "audio spectrum", "music visualizer"],
                &["spectrum", "equalizer"],
            ),
        ),
        (
            SceneArchetype::VideoSynthwaveRoadtrip,
            score_hits(
                lower,
                words,
                &["霓虹公路", "公路飙车", "roadtrip"],
                &["roadtrip", "outrun"],
            ),
        ),
    ];

    let mut best: Option<(SceneArchetype, i32)> = None;
    for (arch, score) in scored {
        if score <= 0 {
            continue;
        }
        if best.is_none_or(|(_, bs)| score > bs) {
            best = Some((arch, score));
        }
    }
    best.map(|(a, _)| a)
}

fn score_hits(lower: &str, words: &HashSet<String>, substrs: &[&str], exact_words: &[&str]) -> i32 {
    let mut s = 0i32;
    for &sub in substrs {
        if lower.contains(sub) {
            s += 3;
        }
    }
    for &w in exact_words {
        if words.contains(w) {
            s += 2;
        }
    }
    s
}

fn parse_modifiers(lower: &str, words: &HashSet<String>, _is_remix: bool) -> RemixModifiers {
    let mut m = RemixModifiers::default();

    // Speed modifiers
    if has_any_substr(lower, &["超快", "极速", "加速", "加快", "更快", "快一点", "double speed", "much faster"])
        || has_any_word(words, &["faster", "fast", "turbo", "speedup", "quicker"])
    {
        m.speed_mul = if has_any_substr(lower, &["超快", "极速", "double speed", "2x"]) {
            1.65
        } else {
            1.35
        };
        m.speed_explicit = true;
        m.has_any_explicit_modifier = true;
    } else if has_any_substr(lower, &["减速", "放慢", "更慢", "慢一点", "slow motion", "half speed"])
        || has_any_word(words, &["slower", "slow", "calm", "chill", "relaxing"])
    {
        m.speed_mul = 0.68;
        m.speed_explicit = true;
        m.has_any_explicit_modifier = true;
    }

    // Palette overrides
    let palette = if has_any_substr(lower, &["紫色", "紫罗兰", "极光紫", "梦幻紫", "暗紫"])
        || has_any_word(words, &["purple", "violet", "amethyst", "lavender"])
    {
        Some(ColorPaletteTheme::AuroraPurple)
    } else if has_any_substr(lower, &["金色", "金黄", "黄金", "烈日", "暖金"])
        || has_any_word(words, &["gold", "golden", "solar", "amber", "yellow"])
    {
        Some(ColorPaletteTheme::SolarGold)
    } else if has_any_substr(lower, &["翠绿", "黑客绿", "翡翠", "绿色", "荧光绿"])
        || has_any_word(words, &["emerald", "green", "matrix", "lime"])
    {
        Some(ColorPaletteTheme::MatrixEmerald)
    } else if has_any_substr(lower, &["深海", "海洋", "蔚蓝", "青蓝", "冰蓝", "水蓝"])
        || has_any_word(words, &["ocean", "teal", "aqua", "marine", "deepsea"])
    {
        Some(ColorPaletteTheme::OceanBio)
    } else if has_any_substr(lower, &["日落", "落日", "夕阳", "蒸汽波", "合成波", "粉橙"])
        || has_any_word(words, &["sunset", "synthwave", "vaporwave", "magenta", "pink"])
    {
        Some(ColorPaletteTheme::SunsetSynthwave)
    } else if has_any_substr(lower, &["绯红", "赤红", "红色", "烈焰", "猩红"])
        || has_any_word(words, &["crimson", "red", "ruby", "flame", "fire"])
    {
        Some(ColorPaletteTheme::CrimsonArcade)
    } else if has_any_substr(lower, &["极简", "冰霜", "冷白", "银白", "极寒"])
        || has_any_word(words, &["frost", "minimal", "ice", "silver", "white", "clean"])
    {
        Some(ColorPaletteTheme::FrostMinimal)
    } else if has_any_substr(lower, &["赛博朋克", "赛博", "霓虹"])
        || has_any_word(words, &["cyberpunk", "cyber", "neon"])
    {
        Some(ColorPaletteTheme::CyberpunkNeon)
    } else {
        None
    };

    if let Some(pal) = palette {
        m.palette_override = Some(pal);
        m.hue_override = Some(pal.spec().primary_hue);
        m.has_any_explicit_modifier = true;
    }

    // Obstacles
    if has_any_substr(lower, &["增加障碍", "更多障碍", "加障碍", "障碍物", "more obstacles", "add obstacles"])
        || has_any_word(words, &["obstacles", "bumpers", "hazards", "barriers"])
    {
        m.obstacle_delta = 3;
        m.has_any_explicit_modifier = true;
    } else if has_any_substr(lower, &["减少障碍", "更少障碍", "fewer obstacles", "less obstacles"]) {
        m.obstacle_delta = -2;
        m.has_any_explicit_modifier = true;
    }

    // Difficulty
    if has_any_substr(lower, &["更难", "提高难度", "地狱难度", "硬核", "hard mode", "more difficult"])
        || has_any_word(words, &["harder", "hard", "hardcore", "intense", "extreme"])
    {
        m.difficulty_delta = 0.45;
        m.has_any_explicit_modifier = true;
    } else if has_any_substr(lower, &["简单", "降低难度", "easy mode"])
        || has_any_word(words, &["easier", "easy", "casual"])
    {
        m.difficulty_delta = -0.30;
        m.has_any_explicit_modifier = true;
    }

    // Particles / Density
    if has_any_substr(lower, &["增加粒子", "更多粒子", "粒子特效", "加粒子", "more particles", "particle effects"])
        || has_any_word(words, &["particles", "sparks", "confetti", "dense", "richer"])
    {
        m.particle_boost = 1.0;
        m.density_delta = 0.45;
        m.has_any_explicit_modifier = true;
    }

    // Glow
    if has_any_substr(lower, &["更亮", "发光", "炫光", "霓虹光晕", "more glow", "brighter"])
        || has_any_word(words, &["glow", "glowing", "bloom", "brighter", "luminous"])
    {
        m.glow_boost = 0.55;
        m.has_any_explicit_modifier = true;
    }

    // Duration (e.g., "20秒", "18s", "30 sec")
    if let Some(sec) = parse_duration_sec(lower) {
        m.duration_sec = Some(sec.clamp(5.0, 120.0));
        m.has_any_explicit_modifier = true;
    }

    m
}

fn parse_duration_sec(lower: &str) -> Option<f64> {
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let num_str = &lower[start..i];
            let rest = lower[i..].trim_start();
            let is_sec_unit = rest.starts_with("秒")
                || rest.starts_with("sec")
                || (rest.starts_with('s')
                    && rest
                        .as_bytes()
                        .get(1)
                        .is_none_or(|b| !b.is_ascii_alphabetic()));
            if is_sec_unit {
                if let Ok(val) = num_str.parse::<f64>() {
                    if (5.0..=120.0).contains(&val) {
                        return Some(val);
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

fn default_archetype_for_kind(kind: AssetKind, seed: u64) -> SceneArchetype {
    let idx = (seed % 5) as usize;
    match kind {
        AssetKind::Video => [
            SceneArchetype::VideoStarWarpReel,
            SceneArchetype::VideoCyberMetropolis,
            SceneArchetype::VideoKineticTypography,
            SceneArchetype::VideoAudioSpectrumShow,
            SceneArchetype::VideoSynthwaveRoadtrip,
        ][idx],
        AssetKind::Animation => [
            SceneArchetype::AnimFluidPlasma,
            SceneArchetype::AnimParticleFlowField,
            SceneArchetype::AnimFractalMandala,
            SceneArchetype::AnimBiolumVoronoi,
            SceneArchetype::AnimHarmonicWaves,
        ][idx],
        AssetKind::Game => [
            SceneArchetype::GameNeonPinballBreakout,
            SceneArchetype::GameSpaceShooter,
            SceneArchetype::GameCyberRunnerDodge,
            SceneArchetype::GameFlappyCyber,
            SceneArchetype::GameNeonSnake,
        ][idx],
    }
}

fn default_palette_for_archetype(arch: SceneArchetype, seed: u64) -> ColorPaletteTheme {
    match arch {
        SceneArchetype::VideoStarWarpReel => ColorPaletteTheme::CyberpunkNeon,
        SceneArchetype::VideoCyberMetropolis => ColorPaletteTheme::CyberpunkNeon,
        SceneArchetype::VideoKineticTypography => ColorPaletteTheme::FrostMinimal,
        SceneArchetype::VideoAudioSpectrumShow => ColorPaletteTheme::AuroraPurple,
        SceneArchetype::VideoSynthwaveRoadtrip => ColorPaletteTheme::SunsetSynthwave,
        SceneArchetype::AnimFluidPlasma => ColorPaletteTheme::AuroraPurple,
        SceneArchetype::AnimParticleFlowField => ColorPaletteTheme::CyberpunkNeon,
        SceneArchetype::AnimFractalMandala => ColorPaletteTheme::SunsetSynthwave,
        SceneArchetype::AnimBiolumVoronoi => ColorPaletteTheme::OceanBio,
        SceneArchetype::AnimHarmonicWaves => ColorPaletteTheme::MatrixEmerald,
        SceneArchetype::GameNeonPinballBreakout => ColorPaletteTheme::CyberpunkNeon,
        SceneArchetype::GameSpaceShooter => ColorPaletteTheme::CrimsonArcade,
        SceneArchetype::GameCyberRunnerDodge => ColorPaletteTheme::SunsetSynthwave,
        SceneArchetype::GameFlappyCyber => ColorPaletteTheme::SolarGold,
        SceneArchetype::GameNeonSnake => ColorPaletteTheme::MatrixEmerald,
    }
    .also_deterministic_with(seed)
}

impl ColorPaletteTheme {
    const fn also_deterministic_with(self, _seed: u64) -> Self {
        self
    }
}

fn build_scene_title(prompt: &str, arch: SceneArchetype, is_chinese: bool) -> String {
    let cleaned: String = prompt
        .chars()
        .filter(|&c| !c.is_control() && c != '"' && c != '\\' && c != ';')
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        arch.default_title(is_chinese).to_string()
    } else if trimmed.chars().count() <= 24 {
        trimmed.to_string()
    } else {
        format!("{} · {}", arch.default_title(is_chinese), truncate_chars(trimmed, 12))
    }
}
