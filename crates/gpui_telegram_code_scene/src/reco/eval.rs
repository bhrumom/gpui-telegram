//! Quantitative Offline RecSys Evaluation Harness (`CS-F07`).
//!
//! Benchmarks `CodeSceneRecommender` deterministically at fixed `EVAL_BASE_TS_MS`
//! across a 90-asset corpus (18 real M1 seeds + 72 synthetic assets spanning 6 topic
//! clusters and 17 distinct bots), 18 background community users for Swing CF,
//! 5 user personas (`117` swipes; `>= 100` required), and genuine Random & Popularity-Only baselines.

use std::collections::HashSet;

use gpui_telegram_proto::{
    AssetKind, AssetManifest, CODE_SCENE_FORMAT_CS1, CodeSceneAssetDto, CodeSceneFeedRequest,
    CodeSceneFeedTab, RankedSceneItemDto, SceneBehaviorEventDto, SceneBehaviorEventKind,
};
use serde::{Deserialize, Serialize};

use crate::reco::embedding::{effective_primary_tag, item_similarity, normalize_tags};
use crate::reco::engine::CodeSceneRecommender;
use crate::seed_library::official_seed_assets;

/// Fixed timestamp matching official M1 seeds' `BASE_TS` for 100% deterministic eval.
pub const EVAL_BASE_TS_MS: i64 = 1_760_000_000_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EvalPersona {
    GameLover,
    ShaderArtFan,
    ShortVideoViewer,
    ColdStartNewUser,
    MidSessionTopicSwitcher,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaEvalMetrics {
    pub persona: EvalPersona,
    pub swipes_simulated: usize,
    pub ndcg_at_10: f64,
    pub hit_rate_at_5: f64,
    pub binary_hit_at_5: f64,
    pub mean_ild: f64,
    pub scatter_violations: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineComparison {
    pub recommender_mean_ndcg_at_10: f64,
    pub popularity_mean_ndcg_at_10: f64,
    pub random_mean_ndcg_at_10: f64,
    pub recommender_mean_hit_rate_at_5: f64,
    pub popularity_mean_hit_rate_at_5: f64,
    pub random_mean_hit_rate_at_5: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PivotEvalMetrics {
    pub pre_pivot_topic_a_in_top5: usize,
    pub post_pivot_refreshes_needed: usize,
    pub post_pivot_topic_b_in_top5: usize,
    pub post_pivot_topic_a_in_top5: usize,
    pub post_pivot_topic_b_ndcg_at_10: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColdStartEvalMetrics {
    pub publisher_slot_index: Option<usize>,
    pub other_user_slot_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NegativeBlocklistEvalMetrics {
    pub not_interested_leaks: usize,
    pub hide_author_leaks: usize,
    pub hide_tag_leaks: usize,
    pub total_items_audited: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoEvalReport {
    pub corpus_size: usize,
    pub total_swipes_simulated: usize,
    pub personas: Vec<PersonaEvalMetrics>,
    pub comparison: BaselineComparison,
    pub pivot: PivotEvalMetrics,
    pub cold_start: ColdStartEvalMetrics,
    pub blocklist: NegativeBlocklistEvalMetrics,
    pub official_18_seeds_scatter_violations: usize,
    pub official_18_seeds_ild: f64,
    pub cold_start_new_user_distinct_kinds_in_top10: usize,
}

struct ClusterSpec {
    id_prefix: &'static str,
    title_prefix: &'static str,
    shared_tags: [&'static str; 4],
    primary_tags: [&'static str; 12],
    snippet: &'static str,
    viral_plays_base: u64,
}

const CLUSTERS: [ClusterSpec; 6] = [
    ClusterSpec {
        id_prefix: "eval_arcade",
        title_prefix: "Cyber Arcade",
        shared_tags: ["arcade", "cyber_arcade", "neon", "action"],
        primary_tags: [
            "runner", "dodger", "blaster", "laser", "boss_rush", "dash", "shield", "arena",
            "combo", "turbo", "reflex", "starfighter",
        ],
        snippet: "fn on_render(s, t, c) { clear(0x0A0A1EFF); round_rect(20, 20, 40, 20, 6, 0x00F0FFFF); glow_circle(180, 320, 18, 0xFF00AAFF, 1.2); circle_rect_hit(180, 320, 18, 20, 20, 40, 20); circle_hit(10, 10, 5, 20, 20, 5); tone(440, 0.05); }",
        viral_plays_base: 1_100,
    },
    ClusterSpec {
        id_prefix: "eval_shader",
        title_prefix: "Fractal Shader Art",
        shared_tags: ["shader", "fractal", "generative", "interactive"],
        primary_tags: [
            "mandelbrot", "julia", "plasma", "voronoi", "kaleidoscope", "caustics", "aurora",
            "nebula", "raymarch", "hyperspace", "chromatic", "interference",
        ],
        snippet: "fn on_render(s, t, c) { pixel_init(64, 96); shader_plasma(t, 3.0, 0); glow_circle(180, 320, 40, hsla(260, 0.9, 0.6, 0.5), 1.4); }",
        viral_plays_base: 900,
    },
    ClusterSpec {
        id_prefix: "eval_video",
        title_prefix: "Motion Code Cinema",
        shared_tags: ["motion_design", "keyframe", "story", "cinema"],
        primary_tags: [
            "typography", "bauhaus", "kinetic", "editorial", "infographic", "title_seq",
            "chronicle", "minimal", "storytelling", "storyboard", "montage", "credits",
        ],
        snippet: "fn on_render(s, t, c) { clear_grad(0x10142AFF, 0x221038FF, 180); let y = keyframe(t, [[0, 80], [5, 320]]); let e = ease_out_cubic(0.5); bezier(20, y, 100, 200, 260, 400, 340, y, 0x00F0FFFF, 2); text(180, y, \"CINEMA\", 22, 0xFFFFFFFF, \"center\"); }",
        viral_plays_base: 2_600,
    },
    ClusterSpec {
        id_prefix: "eval_physics",
        title_prefix: "Physics Gravity Lab",
        shared_tags: ["physics", "gravity", "particles", "simulation"],
        primary_tags: [
            "pendulum", "boids", "fluid", "springs", "orbit", "cloth", "collisions",
            "magnetism", "wave_sim", "nbody", "elastic", "vortex",
        ],
        snippet: "fn on_render(s, t, c) { clear(0x060E1EFF); let n = noise2d(t, 1.0) + fbm2d(t, 2.0, 3); let d = dist(0, 0, 10, 10) + hypot(3, 4) + atan2(1, 2); circle_hit(100, 100, 12, 110, 110, 12); circle(180 + n * 40, 320, 14, 0x44FFCCFF); }",
        viral_plays_base: 1_500,
    },
    ClusterSpec {
        id_prefix: "eval_puzzle",
        title_prefix: "Retro Grid Puzzle",
        shared_tags: ["puzzle", "retro", "logic", "grid"],
        primary_tags: [
            "tetromino", "maze", "sokoban", "match3", "mines", "automata", "life_grid",
            "labyrinth", "pathfind", "pixel_art", "tile_flip", "circuit",
        ],
        snippet: "fn on_render(s, t, c) { clear(0x120A22FF); let h = hash2(1, 2); aabb_hit(0, 0, 10, 10, 5, 5, 10, 10); round_rect(40, 80, 120, 120, 8, 0x3344AAFF); stroke_rect(40, 80, 120, 120, 0xFFFFFFFF, 2); pixel_rect(4, 4, 8, 8, 0xFFCC00FF); }",
        viral_plays_base: 1_300,
    },
    ClusterSpec {
        id_prefix: "eval_music",
        title_prefix: "Electronic Synth Viz",
        shared_tags: ["music_viz", "audio_visual", "electronic", "synthwave"],
        primary_tags: [
            "spectrum", "waveform", "drum_grid", "bass_pulse", "oscilloscope", "eq_rings",
            "dj_reel", "tempo_bars", "harmonic", "vocoder", "beat_drop", "chords",
        ],
        snippet: "fn on_render(s, t, c) { pixel_init(64, 96); shader_synthwave(t, 1.4, 310); let f = fract(t); arc(180, 320, 80, 0, 3.14, 0xFF44CCFF, 3); star(180, 200, 28, 12, 5, 0x00F5FFFF, f); tone(220, 0.08); }",
        viral_plays_base: 2_400,
    },
];

/// Builds the 90-asset deterministic evaluation corpus (18 real seeds + 72 synthetic assets).
pub fn build_eval_corpus() -> Vec<CodeSceneAssetDto> {
    let mut assets = official_seed_assets();
    for (cluster_idx, spec) in CLUSTERS.iter().enumerate() {
        for idx in 0..12usize {
            let kind = match cluster_idx {
                0 => AssetKind::Game,
                1 => AssetKind::Animation,
                2 => AssetKind::Video,
                3 => {
                    if idx % 2 == 0 {
                        AssetKind::Animation
                    } else {
                        AssetKind::Game
                    }
                }
                4 => {
                    if idx < 7 {
                        AssetKind::Game
                    } else {
                        AssetKind::Animation
                    }
                }
                _ => {
                    if idx < 7 {
                        AssetKind::Video
                    } else {
                        AssetKind::Animation
                    }
                }
            };

            let bot_slot = ((cluster_idx * 3) + idx) % 12;
            let bot_id = 880_001 + (bot_slot as i64);
            let bot_name = format!("synth_bot_{:02}", bot_slot + 1);
            let primary = spec.primary_tags[idx];

            // 4-tier traffic distribution: 2 Tier0, 2 Tier1, 4 Tier2, 4 Tier3 per cluster
            let (plays, age_ms): (u64, i64) = match idx {
                0 => (6, 2 * HOUR_MS),
                1 => (14, 5 * HOUR_MS),
                2 => (36, 16 * HOUR_MS),
                3 => (78, 30 * HOUR_MS),
                4 => (160, 2 * DAY_MS),
                5 => (230, 2 * DAY_MS + 12 * HOUR_MS),
                6 => (310, 3 * DAY_MS),
                7 => (420, 4 * DAY_MS),
                8 => (spec.viral_plays_base, 5 * DAY_MS),
                9 => (spec.viral_plays_base + 220, 6 * DAY_MS),
                10 => (spec.viral_plays_base + 480, 7 * DAY_MS),
                _ => (spec.viral_plays_base + 760, 8 * DAY_MS),
            };
            let likes = ((plays as f64) * (0.18 + 0.01 * ((idx % 4) as f64))).round() as u64;
            let bookmarks = (likes / 3).max(1);
            let comments = (likes / 7).max(if plays >= 20 { 1 } else { 0 });
            let shares = (likes / 5).max(if plays >= 20 { 1 } else { 0 });
            let remixes = likes / 12;

            let duration_sec = if kind == AssetKind::Video {
                Some(10.0 + ((idx % 6) as f64) * 2.5)
            } else {
                None
            };
            let mut manifest = AssetManifest::for_kind(kind);
            manifest.duration_sec = duration_sec;
            manifest.seed = ((cluster_idx as u64 + 1) * 1000) + (idx as u64 + 1);

            let mut tags = vec![primary.to_string()];
            for &t in &spec.shared_tags {
                if t != primary {
                    tags.push(t.to_string());
                }
            }

            let created_at_ms = EVAL_BASE_TS_MS - age_ms;
            assets.push(CodeSceneAssetDto {
                scene_id: format!("{}_{:02}_{}", spec.id_prefix, idx + 1, primary),
                title: format!("{} · {}", spec.title_prefix, primary),
                description: format!(
                    "Procedural {} code scene showcasing {} with {} effects.",
                    kind.as_str(),
                    primary,
                    spec.shared_tags[0]
                ),
                kind,
                tags,
                author_user_id: bot_id,
                author_username: bot_name.clone(),
                creator_bot_id: Some(bot_id),
                creator_bot_username: Some(bot_name),
                remix_parent_id: None,
                source_code: spec.snippet.to_string(),
                code_format: CODE_SCENE_FORMAT_CS1.to_string(),
                manifest,
                controls_hint: if kind == AssetKind::Game {
                    "Arrow keys / Space to play".to_string()
                } else {
                    "Pointer interactive".to_string()
                },
                plays_count: plays,
                likes_count: likes,
                bookmarks_count: bookmarks,
                comments_count: comments,
                shares_count: shares,
                remixes_count: remixes,
                viewer_liked: false,
                viewer_bookmarked: false,
                viewer_following_author: false,
                created_at_ms,
                updated_at_ms: created_at_ms,
            });
        }
    }
    assets
}

/// Ground-truth relevance oracle (`0.0..=3.0`) for quantitative offline evaluation.
pub fn oracle_relevance(persona: EvalPersona, asset: &CodeSceneAssetDto) -> f64 {
    let tags = normalize_tags(&asset.tags);
    let has_any = |list: &[&str]| tags.iter().any(|t| list.contains(&t.as_str()));

    match persona {
        EvalPersona::GameLover => match asset.kind {
            AssetKind::Game => {
                if has_any(&[
                    "arcade",
                    "cyber_arcade",
                    "shooter",
                    "flappy",
                    "breakout",
                    "snake",
                    "pong",
                    "reflex",
                    "action",
                ]) {
                    3.0
                } else {
                    2.5
                }
            }
            AssetKind::Video | AssetKind::Animation => {
                if has_any(&["arcade", "scifi", "retro", "space", "physics", "synthwave"]) {
                    2.0
                } else if has_any(&["neon", "cyberpunk"]) {
                    1.0
                } else {
                    0.0
                }
            }
        },
        EvalPersona::ShaderArtFan => match asset.kind {
            AssetKind::Animation => {
                if has_any(&["shader", "fractal", "plasma", "voronoi", "generative"]) {
                    3.0
                } else if has_any(&["particles", "physics", "synthwave", "music_viz"]) {
                    2.0
                } else {
                    1.0
                }
            }
            AssetKind::Video => {
                if has_any(&["shader", "music_viz", "generative", "electronic"]) {
                    2.0
                } else if has_any(&["art", "neon", "cyberpunk"]) {
                    1.0
                } else {
                    0.0
                }
            }
            AssetKind::Game => {
                if has_any(&["neon", "physics"]) {
                    1.0
                } else {
                    0.0
                }
            }
        },
        EvalPersona::ShortVideoViewer => match asset.kind {
            AssetKind::Video => {
                if has_any(&[
                    "motion_design",
                    "keyframe",
                    "story",
                    "cinema",
                    "typography",
                    "cyberpunk",
                    "space",
                    "ai",
                ]) {
                    3.0
                } else {
                    2.5
                }
            }
            AssetKind::Animation => {
                if has_any(&["synthwave", "art", "nature", "generative", "music_viz"]) {
                    2.0
                } else {
                    0.5
                }
            }
            AssetKind::Game => 0.0,
        },
        EvalPersona::ColdStartNewUser => {
            if has_any(&["physics", "gravity", "particles", "simulation"]) {
                3.0
            } else if asset.plays_count >= 100 {
                1.5
            } else {
                1.0
            }
        }
        EvalPersona::MidSessionTopicSwitcher => oracle_relevance(EvalPersona::ShaderArtFan, asset),
    }
}

/// Computes `NDCG@k` with exponential gain `2^rel - 1` against the full corpus ideal ranking.
pub fn compute_ndcg_at_k(
    ranked_assets: &[&CodeSceneAssetDto],
    all_assets: &[CodeSceneAssetDto],
    oracle: impl Fn(&CodeSceneAssetDto) -> f64,
    k: usize,
) -> f64 {
    if k == 0 || ranked_assets.is_empty() || all_assets.is_empty() {
        return 0.0;
    }
    let dcg: f64 = ranked_assets
        .iter()
        .take(k)
        .enumerate()
        .map(|(idx, asset)| {
            let rel = oracle(asset).max(0.0);
            (2.0_f64.powf(rel) - 1.0) / ((idx as f64) + 2.0).log2()
        })
        .sum();

    let mut all_rels: Vec<f64> = all_assets.iter().map(&oracle).collect();
    all_rels.sort_by(|a, b| b.total_cmp(a));
    let idcg: f64 = all_rels
        .into_iter()
        .take(k)
        .enumerate()
        .map(|(idx, rel)| (2.0_f64.powf(rel.max(0.0)) - 1.0) / ((idx as f64) + 2.0).log2())
        .sum();

    if idcg <= 1e-9 {
        0.0
    } else {
        (dcg / idcg).clamp(0.0, 1.0)
    }
}

/// Computes fractional `HitRate@5` (`count(rel >= 2.0 in top 5) / 5.0`) and `BinaryHit@5`.
pub fn compute_hit_rate_at_5(
    ranked_assets: &[&CodeSceneAssetDto],
    oracle: impl Fn(&CodeSceneAssetDto) -> f64,
) -> (f64, f64) {
    let top5: Vec<&CodeSceneAssetDto> = ranked_assets.iter().copied().take(5).collect();
    if top5.is_empty() {
        return (0.0, 0.0);
    }
    let hits = top5.iter().filter(|a| oracle(a) >= 2.0).count();
    let binary = if top5.iter().any(|a| oracle(a) >= 2.5) {
        1.0
    } else {
        0.0
    };
    ((hits as f64) / 5.0, binary)
}

/// Computes mean Intra-List Diversity (`1.0 - item_similarity`) across all unordered pairs.
pub fn compute_ild(items: &[RankedSceneItemDto], engine: &CodeSceneRecommender) -> f64 {
    if items.len() < 2 {
        return 0.0;
    }
    let mut sum_div = 0.0_f64;
    let mut pairs = 0usize;
    for i in 0..items.len() {
        let a = &items[i].asset;
        let tags_a = normalize_tags(&a.tags);
        let emb_a = engine.item_embedding(&a.scene_id).unwrap_or(&[]);
        for j in (i + 1)..items.len() {
            let b = &items[j].asset;
            let tags_b = normalize_tags(&b.tags);
            let emb_b = engine.item_embedding(&b.scene_id).unwrap_or(&[]);
            let sim = item_similarity(emb_a, &tags_a, a.kind, emb_b, &tags_b, b.kind);
            sum_div += (1.0 - sim).clamp(0.0, 1.0);
            pairs += 1;
        }
    }
    if pairs == 0 {
        0.0
    } else {
        sum_div / (pairs as f64)
    }
}

/// Counts hard sliding-window scatter violations:
/// - Author window 3 (`items[k].author == items[k-1..=k-3].author`)
/// - `> 2` consecutive same `AssetKind` (when `enforce_kind_scatter`)
/// - `> 2` consecutive identical `effective_primary_tag`
pub fn count_scatter_violations(
    items: &[RankedSceneItemDto],
    enforce_kind_scatter: bool,
) -> usize {
    let mut violations = 0usize;
    let tags: Vec<Option<String>> = items
        .iter()
        .map(|i| effective_primary_tag(&normalize_tags(&i.asset.tags)))
        .collect();

    for k in 0..items.len() {
        let author = items[k].asset.author_user_id;
        let start = k.saturating_sub(3);
        if items[start..k].iter().any(|p| p.asset.author_user_id == author) {
            violations += 1;
        }
        if enforce_kind_scatter && k >= 2 {
            let kind = items[k].asset.kind;
            if items[k - 1].asset.kind == kind && items[k - 2].asset.kind == kind {
                violations += 1;
            }
        }
        if k >= 2 {
            if let Some(ref pt) = tags[k] {
                if tags[k - 1].as_deref() == Some(pt.as_str())
                    && tags[k - 2].as_deref() == Some(pt.as_str())
                {
                    violations += 1;
                }
            }
        }
    }
    violations
}

/// Seeds 18 background community users (3 per cluster) engaging ONLY Tier1..Tier3 items
/// so R2 multi-user Swing CF is genuinely exercised without touching Tier0 cold-start items.
fn seed_background_community(engine: &mut CodeSceneRecommender, corpus: &[CodeSceneAssetDto]) {
    for (cluster_idx, spec) in CLUSTERS.iter().enumerate() {
        let non_cold: Vec<&CodeSceneAssetDto> = corpus
            .iter()
            .filter(|a| a.scene_id.starts_with(spec.id_prefix) && a.plays_count >= 20)
            .collect();
        for member in 0..3usize {
            let uid = 900_001 + ((cluster_idx * 3 + member) as i64);
            for (k, asset) in non_cold.iter().skip(member).take(6).enumerate() {
                let ts = EVAL_BASE_TS_MS - 10_000 - ((k as i64) * 500);
                let ev_kind = if asset.kind == AssetKind::Game {
                    SceneBehaviorEventKind::GameInteraction
                } else {
                    SceneBehaviorEventKind::Complete
                };
                engine.record_behavior(&SceneBehaviorEventDto {
                    user_id: uid,
                    session_id: format!("bg_{uid}"),
                    scene_id: asset.scene_id.clone(),
                    kind: ev_kind,
                    dwell_ms: 14_000,
                    completion_ratio: 1.1,
                    game_inputs: if asset.kind == AssetKind::Game { 18 } else { 0 },
                    game_play_ms: if asset.kind == AssetKind::Game { 12_000 } else { 0 },
                    game_score: if asset.kind == AssetKind::Game { Some(45.0) } else { None },
                    timestamp_ms: ts,
                    ..Default::default()
                });
                engine.record_behavior(&SceneBehaviorEventDto {
                    user_id: uid,
                    session_id: format!("bg_{uid}"),
                    scene_id: asset.scene_id.clone(),
                    kind: SceneBehaviorEventKind::Like,
                    timestamp_ms: ts + 100,
                    ..Default::default()
                });
            }
        }
    }
}

fn build_eval_engine(corpus: &[CodeSceneAssetDto]) -> CodeSceneRecommender {
    let mut engine = CodeSceneRecommender::new_empty();
    engine.set_now_ms(Some(EVAL_BASE_TS_MS));
    for asset in corpus {
        engine.register_or_update_asset(asset.clone(), None);
    }
    seed_background_community(&mut engine, corpus);
    engine
}

fn simulate_persona_swipe(
    engine: &mut CodeSceneRecommender,
    user_id: i64,
    asset: &CodeSceneAssetDto,
    rel: f64,
    ts_ms: i64,
) {
    engine.record_behavior(&SceneBehaviorEventDto {
        user_id,
        session_id: format!("eval_{user_id}"),
        scene_id: asset.scene_id.clone(),
        kind: SceneBehaviorEventKind::Impression,
        timestamp_ms: ts_ms,
        ..Default::default()
    });

    if rel >= 2.5 {
        if asset.kind == AssetKind::Game {
            engine.record_behavior(&SceneBehaviorEventDto {
                user_id,
                session_id: format!("eval_{user_id}"),
                scene_id: asset.scene_id.clone(),
                kind: SceneBehaviorEventKind::GameInteraction,
                dwell_ms: 18_000,
                game_inputs: 26,
                game_play_ms: 16_000,
                game_restarts: 1,
                game_score: Some(85.0),
                timestamp_ms: ts_ms + 200,
                ..Default::default()
            });
        } else {
            engine.record_behavior(&SceneBehaviorEventDto {
                user_id,
                session_id: format!("eval_{user_id}"),
                scene_id: asset.scene_id.clone(),
                kind: SceneBehaviorEventKind::Complete,
                dwell_ms: 16_000,
                completion_ratio: 1.4,
                loop_count: 1,
                timestamp_ms: ts_ms + 200,
                ..Default::default()
            });
        }
        engine.record_behavior(&SceneBehaviorEventDto {
            user_id,
            session_id: format!("eval_{user_id}"),
            scene_id: asset.scene_id.clone(),
            kind: SceneBehaviorEventKind::Like,
            timestamp_ms: ts_ms + 300,
            ..Default::default()
        });
    } else if rel >= 1.5 {
        engine.record_behavior(&SceneBehaviorEventDto {
            user_id,
            session_id: format!("eval_{user_id}"),
            scene_id: asset.scene_id.clone(),
            kind: SceneBehaviorEventKind::Dwell,
            dwell_ms: 8_500,
            completion_ratio: 0.72,
            timestamp_ms: ts_ms + 200,
            ..Default::default()
        });
    } else {
        engine.record_behavior(&SceneBehaviorEventDto {
            user_id,
            session_id: format!("eval_{user_id}"),
            scene_id: asset.scene_id.clone(),
            kind: SceneBehaviorEventKind::FastSkip,
            dwell_ms: 650,
            timestamp_ms: ts_ms + 200,
            ..Default::default()
        });
    }
}

fn evaluate_warm_persona(
    engine: &mut CodeSceneRecommender,
    corpus: &[CodeSceneAssetDto],
    persona: EvalPersona,
    user_id: i64,
    swipes: usize,
) -> PersonaEvalMetrics {
    let mut clock = EVAL_BASE_TS_MS;
    let mut simulated = 0usize;

    // Warm-up through genuine feed refreshes + swipes (25+ swipes per warm persona)
    while simulated < swipes {
        let resp = engine.recommend_feed_at(
            &CodeSceneFeedRequest {
                user_id,
                session_id: format!("eval_{user_id}"),
                tab: CodeSceneFeedTab::ForYou,
                limit: 12,
                ..Default::default()
            },
            clock,
        );
        if resp.items.is_empty() {
            break;
        }
        for item in resp.items.iter().take(5) {
            if simulated >= swipes {
                break;
            }
            let rel = oracle_relevance(persona, &item.asset);
            simulate_persona_swipe(engine, user_id, &item.asset, rel, clock);
            clock += 2_500;
            simulated += 1;
        }
    }

    // Evaluate steady-state top-12 ForYou page after seen cooldown expires
    let eval_ts = clock + 700_000;
    let final_resp = engine.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id,
            session_id: format!("eval_{user_id}_final"),
            tab: CodeSceneFeedTab::ForYou,
            limit: 12,
            ..Default::default()
        },
        eval_ts,
    );
    let refs: Vec<&CodeSceneAssetDto> = final_resp.items.iter().map(|i| &i.asset).collect();
    let ndcg = compute_ndcg_at_k(&refs, corpus, |a| oracle_relevance(persona, a), 10);
    let (hr5, bin5) = compute_hit_rate_at_5(&refs, |a| oracle_relevance(persona, a));
    let ild = compute_ild(&final_resp.items, engine);
    let violations = count_scatter_violations(&final_resp.items, true);

    PersonaEvalMetrics {
        persona,
        swipes_simulated: simulated,
        ndcg_at_10: ndcg,
        hit_rate_at_5: hr5,
        binary_hit_at_5: bin5,
        mean_ild: ild,
        scatter_violations: violations,
    }
}

fn popularity_baseline_rank<'a>(corpus: &'a [CodeSceneAssetDto]) -> Vec<&'a CodeSceneAssetDto> {
    let mut refs: Vec<&CodeSceneAssetDto> = corpus.iter().collect();
    refs.sort_by(|a, b| {
        b.plays_count
            .cmp(&a.plays_count)
            .then_with(|| a.scene_id.cmp(&b.scene_id))
    });
    refs
}

fn random_baseline_metrics(
    corpus: &[CodeSceneAssetDto],
    persona: EvalPersona,
) -> (f64, f64) {
    let mut sum_ndcg = 0.0;
    let mut sum_hr5 = 0.0;
    let seeds = [11_u64, 29, 47, 83, 131];
    for &seed in &seeds {
        let mut idx: Vec<usize> = (0..corpus.len()).collect();
        let mut s = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        for i in (1..idx.len()).rev() {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            let j = (z as usize) % (i + 1);
            idx.swap(i, j);
        }
        let ranked: Vec<&CodeSceneAssetDto> = idx.iter().map(|&i| &corpus[i]).collect();
        sum_ndcg += compute_ndcg_at_k(&ranked, corpus, |a| oracle_relevance(persona, a), 10);
        let (hr5, _) = compute_hit_rate_at_5(&ranked, |a| oracle_relevance(persona, a));
        sum_hr5 += hr5;
    }
    (sum_ndcg / seeds.len() as f64, sum_hr5 / seeds.len() as f64)
}

/// Runs the complete deterministic quantitative RecSys evaluation suite (`CS-F07`).
pub fn run_full_eval() -> RecoEvalReport {
    let corpus = build_eval_corpus();
    let mut engine = build_eval_engine(&corpus);

    // 1. Warm personalized personas (3 x 25 = 75 swipes)
    let warm_personas = [
        (EvalPersona::GameLover, 1_001_i64),
        (EvalPersona::ShaderArtFan, 1_002_i64),
        (EvalPersona::ShortVideoViewer, 1_003_i64),
    ];
    let mut persona_metrics = Vec::with_capacity(5);
    let mut total_swipes = 0usize;

    for &(persona, uid) in &warm_personas {
        let m = evaluate_warm_persona(&mut engine, &corpus, persona, uid, 25);
        total_swipes += m.swipes_simulated;
        persona_metrics.push(m);
    }

    // 2. ColdStartNewUser (20 swipes: Day-0 discovery diversity + rapid 3-swipe PhysicsSims adaptation)
    let (cold_metrics, distinct_kinds_top10) =
        evaluate_cold_start_new_user(&mut engine, &corpus, 1_004);
    total_swipes += cold_metrics.swipes_simulated;
    persona_metrics.push(cold_metrics);

    // 3. MidSessionTopicSwitcher (22 swipes: Phase 1 GameLover -> 4 FastSkips + 3 Deep Positives -> ShaderArtFan)
    let (pivot_persona_metrics, pivot_metrics) = evaluate_pivot(&mut engine, &corpus, 1_005);
    total_swipes += pivot_persona_metrics.swipes_simulated;
    persona_metrics.push(pivot_persona_metrics);

    // 4. Genuine PopularityOnly & Random Baselines averaged across the 3 warm personas
    let pop_ranked = popularity_baseline_rank(&corpus);
    let mut rec_ndcg = 0.0;
    let mut rec_hr5 = 0.0;
    let mut pop_ndcg = 0.0;
    let mut pop_hr5 = 0.0;
    let mut rnd_ndcg = 0.0;
    let mut rnd_hr5 = 0.0;
    for (idx, &(persona, _)) in warm_personas.iter().enumerate() {
        rec_ndcg += persona_metrics[idx].ndcg_at_10;
        rec_hr5 += persona_metrics[idx].hit_rate_at_5;
        pop_ndcg += compute_ndcg_at_k(&pop_ranked, &corpus, |a| oracle_relevance(persona, a), 10);
        let (phr, _) = compute_hit_rate_at_5(&pop_ranked, |a| oracle_relevance(persona, a));
        pop_hr5 += phr;
        let (rnd_n, rnd_h) = random_baseline_metrics(&corpus, persona);
        rnd_ndcg += rnd_n;
        rnd_hr5 += rnd_h;
    }
    let k_warm = warm_personas.len() as f64;
    let comparison = BaselineComparison {
        recommender_mean_ndcg_at_10: rec_ndcg / k_warm,
        popularity_mean_ndcg_at_10: pop_ndcg / k_warm,
        random_mean_ndcg_at_10: rnd_ndcg / k_warm,
        recommender_mean_hit_rate_at_5: rec_hr5 / k_warm,
        popularity_mean_hit_rate_at_5: pop_hr5 / k_warm,
        random_mean_hit_rate_at_5: rnd_hr5 / k_warm,
    };

    // 5. Cold-start new-asset exposure (Publisher slot 0, unrelated user slot < 10)
    let cold_start = evaluate_new_asset_cold_start(&corpus);

    // 6. Persistent hard-negative 0.0% blocklist leak audit
    let blocklist = evaluate_blocklists(&corpus);

    // 7. Standalone 18-official-seeds live feed audit
    let mut seed_engine = CodeSceneRecommender::new_with_seeds();
    seed_engine.set_now_ms(Some(EVAL_BASE_TS_MS));
    let seed_resp = seed_engine.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id: 7_001,
            session_id: "eval_seeds".to_string(),
            tab: CodeSceneFeedTab::ForYou,
            limit: 12,
            ..Default::default()
        },
        EVAL_BASE_TS_MS,
    );
    let official_18_seeds_scatter_violations = count_scatter_violations(&seed_resp.items, true);
    let official_18_seeds_ild = compute_ild(&seed_resp.items, &seed_engine);

    RecoEvalReport {
        corpus_size: corpus.len(),
        total_swipes_simulated: total_swipes,
        personas: persona_metrics,
        comparison,
        pivot: pivot_metrics,
        cold_start,
        blocklist,
        official_18_seeds_scatter_violations,
        official_18_seeds_ild,
        cold_start_new_user_distinct_kinds_in_top10: distinct_kinds_top10,
    }
}

fn evaluate_cold_start_new_user(
    engine: &mut CodeSceneRecommender,
    corpus: &[CodeSceneAssetDto],
    user_id: i64,
) -> (PersonaEvalMetrics, usize) {
    let day0 = engine.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id,
            session_id: format!("eval_{user_id}_day0"),
            tab: CodeSceneFeedTab::ForYou,
            limit: 12,
            ..Default::default()
        },
        EVAL_BASE_TS_MS,
    );
    let distinct_kinds: HashSet<AssetKind> = day0
        .items
        .iter()
        .take(10)
        .map(|i| i.asset.kind)
        .collect();
    let day0_violations = count_scatter_violations(&day0.items, true);
    let day0_ild = compute_ild(&day0.items, engine);

    // Rapid onboarding: 20 swipes guided by PhysicsSims affinity
    let mut clock = EVAL_BASE_TS_MS + 1_000;
    let mut swipes = 0usize;
    let physics_seeds: Vec<&CodeSceneAssetDto> = corpus
        .iter()
        .filter(|a| a.scene_id.starts_with("eval_physics"))
        .take(5)
        .collect();
    for asset in physics_seeds {
        simulate_persona_swipe(engine, user_id, asset, 3.0, clock);
        clock += 2_000;
        swipes += 1;
    }
    while swipes < 20 {
        let resp = engine.recommend_feed_at(
            &CodeSceneFeedRequest {
                user_id,
                session_id: format!("eval_{user_id}_warm"),
                tab: CodeSceneFeedTab::ForYou,
                limit: 12,
                ..Default::default()
            },
            clock,
        );
        for item in resp.items.iter().take(5) {
            if swipes >= 20 {
                break;
            }
            let rel = oracle_relevance(EvalPersona::ColdStartNewUser, &item.asset);
            simulate_persona_swipe(engine, user_id, &item.asset, rel, clock);
            clock += 2_000;
            swipes += 1;
        }
    }

    let post = engine.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id,
            session_id: format!("eval_{user_id}_post"),
            tab: CodeSceneFeedTab::ForYou,
            limit: 12,
            ..Default::default()
        },
        clock + 700_000,
    );
    let refs: Vec<&CodeSceneAssetDto> = post.items.iter().map(|i| &i.asset).collect();
    let ndcg = compute_ndcg_at_k(
        &refs,
        corpus,
        |a| oracle_relevance(EvalPersona::ColdStartNewUser, a),
        10,
    );
    let (hr5, bin5) = compute_hit_rate_at_5(&refs, |a| {
        oracle_relevance(EvalPersona::ColdStartNewUser, a)
    });
    let post_ild = compute_ild(&post.items, engine);
    let post_violations = count_scatter_violations(&post.items, true);

    (
        PersonaEvalMetrics {
            persona: EvalPersona::ColdStartNewUser,
            swipes_simulated: swipes,
            ndcg_at_10: ndcg,
            hit_rate_at_5: hr5,
            binary_hit_at_5: bin5,
            mean_ild: (day0_ild + post_ild) * 0.5,
            scatter_violations: day0_violations + post_violations,
        },
        distinct_kinds.len(),
    )
}

fn evaluate_pivot(
    engine: &mut CodeSceneRecommender,
    corpus: &[CodeSceneAssetDto],
    user_id: i64,
) -> (PersonaEvalMetrics, PivotEvalMetrics) {
    let mut clock = EVAL_BASE_TS_MS;
    let mut swipes = 0usize;

    // Phase 1: 15 swipes establishing strong GameLover preference (Topic A)
    let topic_a_items: Vec<&CodeSceneAssetDto> = corpus
        .iter()
        .filter(|a| a.kind == AssetKind::Game)
        .take(15)
        .collect();
    for asset in &topic_a_items {
        simulate_persona_swipe(engine, user_id, asset, 3.0, clock);
        clock += 2_000;
        swipes += 1;
    }

    let pre_resp = engine.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id,
            session_id: format!("eval_{user_id}_pre"),
            tab: CodeSceneFeedTab::ForYou,
            limit: 12,
            ..Default::default()
        },
        clock + 700_000,
    );
    let pre_pivot_topic_a_in_top5 = pre_resp
        .items
        .iter()
        .take(5)
        .filter(|i| oracle_relevance(EvalPersona::GameLover, &i.asset) >= 2.5)
        .count();

    // Phase 2: Mid-session pivot: 4 FastSkips on Topic A + 3 Deep Positives on Topic B (ShaderArtFan)
    clock += 705_000;
    let skip_games: Vec<&CodeSceneAssetDto> = pre_resp
        .items
        .iter()
        .map(|i| &i.asset)
        .filter(|a| a.kind == AssetKind::Game)
        .take(4)
        .collect();
    for g in skip_games {
        engine.record_behavior(&SceneBehaviorEventDto {
            user_id,
            session_id: format!("eval_{user_id}_pivot"),
            scene_id: g.scene_id.clone(),
            kind: SceneBehaviorEventKind::FastSkip,
            dwell_ms: 500,
            timestamp_ms: clock,
            ..Default::default()
        });
        clock += 1_000;
        swipes += 1;
    }

    let topic_b_items: Vec<&CodeSceneAssetDto> = corpus
        .iter()
        .filter(|a| a.scene_id.starts_with("eval_shader") && a.plays_count >= 20)
        .take(3)
        .collect();
    for b in topic_b_items {
        engine.record_behavior(&SceneBehaviorEventDto {
            user_id,
            session_id: format!("eval_{user_id}_pivot"),
            scene_id: b.scene_id.clone(),
            kind: SceneBehaviorEventKind::Complete,
            dwell_ms: 18_000,
            completion_ratio: 1.6,
            loop_count: 2,
            timestamp_ms: clock,
            ..Default::default()
        });
        engine.record_behavior(&SceneBehaviorEventDto {
            user_id,
            session_id: format!("eval_{user_id}_pivot"),
            scene_id: b.scene_id.clone(),
            kind: SceneBehaviorEventKind::Like,
            timestamp_ms: clock + 100,
            ..Default::default()
        });
        engine.record_behavior(&SceneBehaviorEventDto {
            user_id,
            session_id: format!("eval_{user_id}_pivot"),
            scene_id: b.scene_id.clone(),
            kind: SceneBehaviorEventKind::Bookmark,
            timestamp_ms: clock + 200,
            ..Default::default()
        });
        clock += 2_000;
        swipes += 1;
    }

    // Measure refreshes needed (1..=3) for Topic B to take top-5 majority (>=3 and > Topic A)
    let mut refreshes_needed = 3usize;
    let mut final_post_resp = engine.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id,
            session_id: format!("eval_{user_id}_r1"),
            tab: CodeSceneFeedTab::ForYou,
            limit: 12,
            ..Default::default()
        },
        clock + 1_000,
    );
    let mut post_b_top5 = 0usize;
    let mut post_a_top5 = 0usize;

    for r in 1..=2usize {
        if r > 1 {
            // One additional Topic B swipe if needed between Refresh 1 and Refresh 2
            if let Some(extra) = corpus
                .iter()
                .filter(|a| a.scene_id.starts_with("eval_shader") && a.plays_count >= 20)
                .nth(3 + r)
            {
                simulate_persona_swipe(engine, user_id, extra, 3.0, clock + (r as i64) * 3_000);
                swipes += 1;
            }
            final_post_resp = engine.recommend_feed_at(
                &CodeSceneFeedRequest {
                    user_id,
                    session_id: format!("eval_{user_id}_r{r}"),
                    tab: CodeSceneFeedTab::ForYou,
                    limit: 12,
                    ..Default::default()
                },
                clock + (r as i64) * 4_000,
            );
        }
        let b_cnt = final_post_resp
            .items
            .iter()
            .take(5)
            .filter(|i| oracle_relevance(EvalPersona::ShaderArtFan, &i.asset) >= 2.0)
            .count();
        let a_cnt = final_post_resp
            .items
            .iter()
            .take(5)
            .filter(|i| i.asset.kind == AssetKind::Game)
            .count();
        post_b_top5 = b_cnt;
        post_a_top5 = a_cnt;
        if b_cnt >= 3 && b_cnt > a_cnt {
            refreshes_needed = r;
            break;
        }
    }

    let refs: Vec<&CodeSceneAssetDto> = final_post_resp.items.iter().map(|i| &i.asset).collect();
    let post_ndcg = compute_ndcg_at_k(
        &refs,
        corpus,
        |a| oracle_relevance(EvalPersona::ShaderArtFan, a),
        10,
    );
    let (hr5, bin5) = compute_hit_rate_at_5(&refs, |a| {
        oracle_relevance(EvalPersona::ShaderArtFan, a)
    });
    let ild = compute_ild(&final_post_resp.items, engine);
    let violations = count_scatter_violations(&final_post_resp.items, true);

    (
        PersonaEvalMetrics {
            persona: EvalPersona::MidSessionTopicSwitcher,
            swipes_simulated: swipes,
            ndcg_at_10: post_ndcg,
            hit_rate_at_5: hr5,
            binary_hit_at_5: bin5,
            mean_ild: ild,
            scatter_violations: violations,
        },
        PivotEvalMetrics {
            pre_pivot_topic_a_in_top5,
            post_pivot_refreshes_needed: refreshes_needed,
            post_pivot_topic_b_in_top5: post_b_top5,
            post_pivot_topic_a_in_top5: post_a_top5,
            post_pivot_topic_b_ndcg_at_10: post_ndcg,
        },
    )
}

fn evaluate_new_asset_cold_start(corpus: &[CodeSceneAssetDto]) -> ColdStartEvalMetrics {
    let mut engine = build_eval_engine(corpus);
    // Warm up unrelated user 5002 on games so we prove even an unrelated warm user sees the new asset in slot < 10
    for g in corpus.iter().filter(|a| a.kind == AssetKind::Game).take(6) {
        simulate_persona_swipe(&mut engine, 5_002, g, 3.0, EVAL_BASE_TS_MS - 5_000);
    }

    let new_asset = CodeSceneAssetDto {
        scene_id: "ai_bot_brand_new_cold_start_01".to_string(),
        title: "AI Bot Brand New Nebula".to_string(),
        description: "Freshly generated AI bot code animation with zero initial plays".to_string(),
        kind: AssetKind::Animation,
        tags: vec!["nebula".to_string(), "bot_new".to_string(), "generative".to_string()],
        author_user_id: 5_001,
        author_username: "creator_user_5001".to_string(),
        creator_bot_id: Some(777_000),
        creator_bot_username: Some("fabushi_ai_bot".to_string()),
        remix_parent_id: None,
        source_code: "fn on_render(s, t, c) { clear(0x050816FF); glow_circle(180, 320, 48, 0x00F5FFFF, 1.4); }".to_string(),
        code_format: CODE_SCENE_FORMAT_CS1.to_string(),
        manifest: AssetManifest::for_kind(AssetKind::Animation),
        controls_hint: "Move pointer".to_string(),
        plays_count: 0,
        likes_count: 0,
        bookmarks_count: 0,
        comments_count: 0,
        shares_count: 0,
        remixes_count: 0,
        viewer_liked: false,
        viewer_bookmarked: false,
        viewer_following_author: false,
        created_at_ms: EVAL_BASE_TS_MS,
        updated_at_ms: EVAL_BASE_TS_MS,
    };
    engine.register_or_update_asset(new_asset, Some(5_001));

    let pub_resp = engine.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id: 5_001,
            session_id: "cold_pub".to_string(),
            tab: CodeSceneFeedTab::ForYou,
            limit: 12,
            ..Default::default()
        },
        EVAL_BASE_TS_MS,
    );
    let publisher_slot_index = pub_resp
        .items
        .iter()
        .position(|i| i.asset.scene_id == "ai_bot_brand_new_cold_start_01");

    let other_resp = engine.recommend_feed_at(
        &CodeSceneFeedRequest {
            user_id: 5_002,
            session_id: "cold_other".to_string(),
            tab: CodeSceneFeedTab::ForYou,
            limit: 12,
            ..Default::default()
        },
        EVAL_BASE_TS_MS,
    );
    let other_user_slot_index = other_resp
        .items
        .iter()
        .position(|i| i.asset.scene_id == "ai_bot_brand_new_cold_start_01");

    ColdStartEvalMetrics {
        publisher_slot_index,
        other_user_slot_index,
    }
}

fn evaluate_blocklists(corpus: &[CodeSceneAssetDto]) -> NegativeBlocklistEvalMetrics {
    let mut engine = build_eval_engine(corpus);
    let uid = 6_001_i64;
    let blocked_scene = "seed_game_flappy_bird";
    let blocked_author_scene = "eval_shader_01_mandelbrot";
    let blocked_author = engine
        .get_asset(blocked_author_scene)
        .map(|a| a.author_user_id)
        .unwrap_or(880_004);
    let blocked_tag = "cyber_arcade";

    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid,
        session_id: "blk".to_string(),
        scene_id: blocked_scene.to_string(),
        kind: SceneBehaviorEventKind::NotInterested,
        timestamp_ms: EVAL_BASE_TS_MS,
        ..Default::default()
    });
    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid,
        session_id: "blk".to_string(),
        scene_id: blocked_author_scene.to_string(),
        author_user_id: Some(blocked_author),
        kind: SceneBehaviorEventKind::HideAuthor,
        timestamp_ms: EVAL_BASE_TS_MS + 10,
        ..Default::default()
    });
    engine.record_behavior(&SceneBehaviorEventDto {
        user_id: uid,
        session_id: "blk".to_string(),
        scene_id: "eval_arcade_01_runner".to_string(),
        kind: SceneBehaviorEventKind::HideTag,
        reason: Some(blocked_tag.to_string()),
        timestamp_ms: EVAL_BASE_TS_MS + 20,
        ..Default::default()
    });

    let mut not_interested_leaks = 0usize;
    let mut hide_author_leaks = 0usize;
    let mut hide_tag_leaks = 0usize;
    let mut total_items_audited = 0usize;

    let mut audit_items = |items: &[RankedSceneItemDto]| {
        for item in items {
            total_items_audited += 1;
            if item.asset.scene_id == blocked_scene {
                not_interested_leaks += 1;
            }
            if item.asset.author_user_id == blocked_author {
                hide_author_leaks += 1;
            }
            if normalize_tags(&item.asset.tags).iter().any(|t| t == blocked_tag) {
                hide_tag_leaks += 1;
            }
        }
    };

    // 5 cursor pages on ForYou
    let mut cursor: Option<String> = None;
    for _ in 0..5 {
        let resp = engine.recommend_feed_at(
            &CodeSceneFeedRequest {
                user_id: uid,
                session_id: "blk_page".to_string(),
                tab: CodeSceneFeedTab::ForYou,
                cursor: cursor.clone(),
                limit: 12,
                ..Default::default()
            },
            EVAL_BASE_TS_MS + 1_000,
        );
        audit_items(&resp.items);
        cursor = resp.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    // 3 fresh refreshes + modality tabs
    for r in 0..3 {
        let resp = engine.recommend_feed_at(
            &CodeSceneFeedRequest {
                user_id: uid,
                session_id: format!("blk_ref_{r}"),
                tab: CodeSceneFeedTab::ForYou,
                limit: 12,
                ..Default::default()
            },
            EVAL_BASE_TS_MS + 5_000 + (r as i64) * 1_000,
        );
        audit_items(&resp.items);
    }
    for tab in [
        CodeSceneFeedTab::Videos,
        CodeSceneFeedTab::Animations,
        CodeSceneFeedTab::Games,
    ] {
        let resp = engine.recommend_feed_at(
            &CodeSceneFeedRequest {
                user_id: uid,
                session_id: "blk_tab".to_string(),
                tab,
                limit: 20,
                ..Default::default()
            },
            EVAL_BASE_TS_MS + 10_000,
        );
        audit_items(&resp.items);
    }

    // Catalog exhaustion wraparound test on 18 official seeds
    let mut small = CodeSceneRecommender::new_with_seeds();
    small.set_now_ms(Some(EVAL_BASE_TS_MS));
    small.record_behavior(&SceneBehaviorEventDto {
        user_id: uid,
        scene_id: blocked_scene.to_string(),
        kind: SceneBehaviorEventKind::NotInterested,
        timestamp_ms: EVAL_BASE_TS_MS,
        ..Default::default()
    });
    for step in 0..30i64 {
        let resp = small.recommend_feed_at(
            &CodeSceneFeedRequest {
                user_id: uid,
                tab: CodeSceneFeedTab::ForYou,
                limit: 12,
                ..Default::default()
            },
            EVAL_BASE_TS_MS + step * 1_000,
        );
        audit_items(&resp.items);
        if let Some(first) = resp.items.first() {
            small.record_behavior(&SceneBehaviorEventDto {
                user_id: uid,
                scene_id: first.asset.scene_id.clone(),
                kind: SceneBehaviorEventKind::Impression,
                timestamp_ms: EVAL_BASE_TS_MS + step * 1_000 + 100,
                ..Default::default()
            });
        }
    }

    NegativeBlocklistEvalMetrics {
        not_interested_leaks,
        hide_author_leaks,
        hide_tag_leaks,
        total_items_audited,
    }
}

/// Asserts all M2 `CS-F07` quantitative acceptance thresholds on `report`.
pub fn assert_eval_passes(report: &RecoEvalReport) {
    assert!(
        report.corpus_size >= 78,
        "expected corpus_size >= 78 (90), got {}",
        report.corpus_size
    );
    assert!(
        report.total_swipes_simulated >= 100,
        "expected >= 100 simulated swipes, got {}",
        report.total_swipes_simulated
    );

    for m in &report.personas {
        assert_eq!(
            m.scatter_violations, 0,
            "persona {:?} had {} scatter violations",
            m.persona, m.scatter_violations
        );
        assert!(
            m.mean_ild >= 0.30,
            "persona {:?} ILD {:.4} < 0.30",
            m.persona, m.mean_ild
        );
        if matches!(
            m.persona,
            EvalPersona::GameLover | EvalPersona::ShaderArtFan | EvalPersona::ShortVideoViewer
        ) {
            assert!(
                m.ndcg_at_10 >= 0.75,
                "warm persona {:?} NDCG@10 {:.4} < 0.75",
                m.persona, m.ndcg_at_10
            );
        }
    }

    assert!(
        report.cold_start_new_user_distinct_kinds_in_top10 == 3,
        "expected ColdStartNewUser top-10 to include all 3 modalities, got {}",
        report.cold_start_new_user_distinct_kinds_in_top10
    );

    let c = &report.comparison;
    assert!(
        c.recommender_mean_ndcg_at_10 >= 0.75,
        "recommender mean NDCG@10 {:.4} < 0.75",
        c.recommender_mean_ndcg_at_10
    );
    assert!(
        c.recommender_mean_ndcg_at_10 >= c.popularity_mean_ndcg_at_10 + 0.15,
        "recommender NDCG@10 {:.4} did not beat popularity {:.4} by >= 0.15",
        c.recommender_mean_ndcg_at_10, c.popularity_mean_ndcg_at_10
    );
    assert!(
        c.recommender_mean_ndcg_at_10 >= c.random_mean_ndcg_at_10 + 0.25,
        "recommender NDCG@10 {:.4} did not beat random {:.4} by >= 0.25",
        c.recommender_mean_ndcg_at_10, c.random_mean_ndcg_at_10
    );
    assert!(
        c.recommender_mean_hit_rate_at_5 >= c.popularity_mean_hit_rate_at_5 + 0.20,
        "recommender HitRate@5 {:.4} did not beat popularity {:.4} by >= 0.20",
        c.recommender_mean_hit_rate_at_5, c.popularity_mean_hit_rate_at_5
    );
    assert!(
        c.recommender_mean_hit_rate_at_5 >= c.random_mean_hit_rate_at_5 + 0.30,
        "recommender HitRate@5 {:.4} did not beat random {:.4} by >= 0.30",
        c.recommender_mean_hit_rate_at_5, c.random_mean_hit_rate_at_5
    );

    assert!(
        report.pivot.pre_pivot_topic_a_in_top5 >= 3,
        "pre-pivot Topic A in top-5 was {} (< 3)",
        report.pivot.pre_pivot_topic_a_in_top5
    );
    assert!(
        report.pivot.post_pivot_refreshes_needed <= 2,
        "post-pivot refreshes needed {} (> 2)",
        report.pivot.post_pivot_refreshes_needed
    );
    assert!(
        report.pivot.post_pivot_topic_b_in_top5 >= 3
            && report.pivot.post_pivot_topic_b_in_top5 > report.pivot.post_pivot_topic_a_in_top5,
        "post-pivot Topic B ({}) did not overtake Topic A ({})",
        report.pivot.post_pivot_topic_b_in_top5, report.pivot.post_pivot_topic_a_in_top5
    );

    assert_eq!(
        report.cold_start.publisher_slot_index,
        Some(0),
        "publisher fast-track expected slot 0, got {:?}",
        report.cold_start.publisher_slot_index
    );
    assert!(
        report
            .cold_start
            .other_user_slot_index
            .is_some_and(|idx| idx < 10),
        "unrelated user cold-start expected slot < 10, got {:?}",
        report.cold_start.other_user_slot_index
    );

    assert_eq!(report.blocklist.not_interested_leaks, 0);
    assert_eq!(report.blocklist.hide_author_leaks, 0);
    assert_eq!(report.blocklist.hide_tag_leaks, 0);
    assert!(report.blocklist.total_items_audited >= 150);

    assert_eq!(report.official_18_seeds_scatter_violations, 0);
    assert!(
        report.official_18_seeds_ild >= 0.30,
        "18 official seeds ILD {:.4} < 0.30",
        report.official_18_seeds_ild
    );
}
