//! Deterministic 64-dim dense embeddings, bilingual CJK/Latin tokenization,
//! `cs1` code feature token extraction, and NaN/zero-norm-safe vector math.

use std::collections::{BTreeSet, HashSet};

use gpui_telegram_proto::{AssetKind, CodeSceneAssetDto};
use sha2::{Digest, Sha256};

use crate::lang::limits::VmBudgets;
use crate::lang::stdlib::BuiltinId;
use crate::lang::{CompiledProgram, compile_source};

/// Fixed dense embedding dimension for both item and user interest vectors.
pub const EMBEDDING_DIM: usize = 64;

/// Generic modality echo tags skipped when deriving `effective_primary_tag`.
const GENERIC_MODALITY_TAGS: [&str; 6] = [
    "game",
    "games",
    "video",
    "videos",
    "animation",
    "animations",
];

/// Normalizes a single tag string: trims surrounding whitespace and `#` characters, lowercases.
pub fn normalize_tag(tag: &str) -> String {
    tag.trim_matches(|c: char| c.is_whitespace() || c == '#')
        .to_lowercase()
}

/// Normalizes and deduplicates a slice of tags while preserving order.
pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(tags.len());
    let mut seen = HashSet::with_capacity(tags.len());
    for raw in tags {
        let norm = normalize_tag(raw);
        if !norm.is_empty() && seen.insert(norm.clone()) {
            out.push(norm);
        }
    }
    out
}

/// Returns the first normalized tag NOT in generic modality echo tags
/// (`{"game","games","video","videos","animation","animations"}`).
pub fn first_non_generic_tag(normalized_tags: &[String]) -> Option<String> {
    normalized_tags
        .iter()
        .find(|tag| !GENERIC_MODALITY_TAGS.contains(&tag.as_str()))
        .cloned()
}

/// Derives the effective primary tag for sliding-window scatter rules.
///
/// Prefers the first normalized tag NOT in generic modality echo tags
/// (`{"game","games","video","videos","animation","animations"}`), falling back
/// to `tags.first()` if all tags are generic.
pub fn effective_primary_tag(normalized_tags: &[String]) -> Option<String> {
    if normalized_tags.is_empty() {
        return None;
    }
    first_non_generic_tag(normalized_tags).or_else(|| normalized_tags.first().cloned())
}

/// Extracts sorted, deduplicated static `cs1` feature tokens + lifecycle hooks +
/// capability family tags from an already-compiled `CompiledProgram`.
pub fn extract_compiled_feature_tokens(compiled: &CompiledProgram) -> Vec<String> {
    let mut tokens = BTreeSet::new();
    if compiled.has_function("on_init") {
        tokens.insert("hook:on_init".to_string());
    }
    if compiled.has_function("on_update") {
        tokens.insert("hook:on_update".to_string());
    }
    if compiled.has_function("on_render") {
        tokens.insert("hook:on_render".to_string());
    }
    for tok in &compiled.feature_tokens {
        tokens.insert(tok.clone());
    }
    add_capability_families(&mut tokens);
    tokens.into_iter().collect()
}

/// Extracts sorted, deduplicated static `cs1` feature tokens + capability family tags.
///
/// Uses `compile_source` as fast path (< 0.1 ms, no 5-frame shader execution),
/// falling back to a zero-panic ASCII identifier scan against `BuiltinId::from_name`
/// for synthetic/partial scripts.
pub fn extract_code_feature_tokens(source_code: &str) -> Vec<String> {
    let trimmed = source_code.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    if let Ok(compiled) = compile_source(trimmed, &VmBudgets::default()) {
        return extract_compiled_feature_tokens(&compiled);
    }

    let mut tokens = BTreeSet::new();
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &trimmed[start..i];
            if BuiltinId::from_name(ident).is_some() {
                tokens.insert(ident.to_string());
            } else if matches!(ident, "on_init" | "on_update" | "on_render") {
                tokens.insert(format!("hook:{ident}"));
            }
        } else {
            i += 1;
        }
    }

    add_capability_families(&mut tokens);
    tokens.into_iter().collect()
}

/// Derives capability family tokens from raw builtin feature tokens.
pub fn enrich_feature_tokens(base_tokens: &[String]) -> Vec<String> {
    let mut tokens: BTreeSet<String> = base_tokens.iter().cloned().collect();
    add_capability_families(&mut tokens);
    tokens.into_iter().collect()
}

fn add_capability_families(tokens: &mut BTreeSet<String>) {
    let snapshot: Vec<String> = tokens.iter().cloned().collect();
    for tok in &snapshot {
        let s = tok.as_str();
        if s.starts_with("shader_") || s.starts_with("pixel_") {
            tokens.insert("code_family:pixel_shader".to_string());
        }
        if matches!(s, "aabb_hit" | "circle_hit" | "circle_rect_hit") {
            tokens.insert("code_family:arcade_collision".to_string());
        }
        if matches!(
            s,
            "keyframe" | "ease_in_out" | "ease_out_cubic" | "ease_out_bounce" | "ease_out_elastic"
        ) {
            tokens.insert("code_family:timeline_motion".to_string());
        }
        if matches!(s, "noise2d" | "fbm2d") {
            tokens.insert("code_family:procedural_noise".to_string());
        }
        if s == "tone" {
            tokens.insert("code_family:audio_synth".to_string());
        }
        if matches!(s, "glow_circle" | "star" | "bezier" | "polygon" | "arc") {
            tokens.insert("code_family:vector_art".to_string());
        }
    }
}

fn is_cjk(ch: char) -> bool {
    matches!(ch, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}')
}

/// Bilingual concept bridge mapping common Chinese/English terms onto shared canonical IDs.
const BILINGUAL_CONCEPTS: &[(&str, &[&str])] = &[
    ("game", &["game", "games", "arcade", "playable", "游戏", "小游戏", "街机"]),
    ("video", &["video", "videos", "reel", "cinema", "film", "trailer", "视频", "短片", "预告"]),
    ("animation", &["animation", "animations", "visual", "wallpaper", "动画", "动效", "壁纸"]),
    ("neon", &["neon", "glow", "霓虹", "发光", "炫光"]),
    ("cyberpunk", &["cyberpunk", "cyber", "赛博", "赛博朋克"]),
    ("fractal", &["fractal", "mandelbrot", "julia", "分形", "曼德勃罗"]),
    ("shader", &["shader", "plasma", "voronoi", "着色器", "流体", "等离子", "细胞"]),
    ("particles", &["particles", "particle", "boids", "firefly", "swarm", "粒子", "萤火虫", "群聚"]),
    ("space", &["space", "starfield", "orbit", "solar", "galaxy", "astronomy", "太空", "星际", "星空", "轨道", "宇宙"]),
    ("physics", &["physics", "gravity", "pendulum", "fluid", "spring", "物理", "引力", "重力", "仿真"]),
    ("snake", &["snake", "贪吃蛇"]),
    ("flappy", &["flappy", "bird", "飞鸟", "像素鸟"]),
    ("shooter", &["shooter", "starfighter", "laser", "射击", "战机", "飞船"]),
    ("pong", &["pong", "paddle", "乒乓", "弹球"]),
    ("breakout", &["breakout", "brick", "pinball", "打砖块", "砖块", "弹珠"]),
    ("retro", &["retro", "synthwave", "vaporwave", "复古", "蒸汽波", "合成波"]),
    ("sakura", &["sakura", "nature", "tree", "樱花", "自然"]),
    ("aurora", &["aurora", "polar", "极光"]),
    ("music_viz", &["music_viz", "music", "audio", "spectrum", "beat", "节拍", "音乐", "频谱", "音频"]),
    ("typography", &["typography", "motion_design", "kinetic", "story", "排版", "字体", "叙事", "动态图形"]),
    ("puzzle", &["puzzle", "maze", "logic", "tetromino", "益智", "解谜", "迷宫"]),
    ("ai", &["ai", "neural", "generative", "人工智能", "神经", "生成"]),
];

/// Returns canonical bilingual concept IDs matched in `text`.
pub fn bilingual_concepts_for_text(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let ascii_words: HashSet<String> = extract_ascii_words(&lower).into_iter().collect();
    let mut concepts = Vec::new();
    for &(concept, triggers) in BILINGUAL_CONCEPTS {
        let matched = triggers.iter().any(|&trig| {
            if trig.is_ascii() {
                if trig.contains('_') {
                    lower.contains(trig)
                } else {
                    ascii_words.contains(trig)
                        || ascii_words.iter().any(|w| w.starts_with(trig) && w.len() <= trig.len() + 3)
                }
            } else {
                lower.contains(trig)
            }
        });
        if matched {
            concepts.push(concept.to_string());
        }
    }
    concepts
}

fn extract_ascii_words(lower: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            cur.push(ch);
        } else if !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Tokenizes mixed Latin/ASCII and CJK text into word tokens, CJK unigrams/bigrams,
/// and canonical bilingual concepts.
pub fn tokenize_bilingual(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    let mut ascii_buf = String::new();
    let mut cjk_run: Vec<char> = Vec::new();

    let flush_ascii = |buf: &mut String, out: &mut Vec<String>| {
        if !buf.is_empty() {
            let w = std::mem::take(buf);
            out.push(w);
        }
    };
    let flush_cjk = |run: &mut Vec<char>, out: &mut Vec<String>| {
        if run.is_empty() {
            return;
        }
        for &c in run.iter() {
            out.push(c.to_string());
        }
        for win in run.windows(2) {
            let mut bigram = String::with_capacity(8);
            bigram.push(win[0]);
            bigram.push(win[1]);
            out.push(bigram);
        }
        run.clear();
    };

    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            flush_cjk(&mut cjk_run, &mut out);
            ascii_buf.push(ch);
        } else if is_cjk(ch) {
            flush_ascii(&mut ascii_buf, &mut out);
            cjk_run.push(ch);
        } else {
            flush_ascii(&mut ascii_buf, &mut out);
            flush_cjk(&mut cjk_run, &mut out);
        }
    }
    flush_ascii(&mut ascii_buf, &mut out);
    flush_cjk(&mut cjk_run, &mut out);

    out
}

fn char_trigrams(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < 3 {
        return Vec::new();
    }
    chars
        .windows(3)
        .map(|w| w.iter().collect::<String>())
        .collect()
}

/// Deterministic 2-hash SHA-256 signed random projection into 64 dims.
pub fn project_feature(vec: &mut [f32; EMBEDDING_DIM], domain: &str, token: &str, weight: f32) {
    if !weight.is_finite() || weight == 0.0 || token.is_empty() {
        return;
    }
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0u8]);
    hasher.update(token.as_bytes());
    let d = hasher.finalize();

    let b0 = (u16::from_le_bytes([d[0], d[1]]) as usize) % EMBEDDING_DIM;
    let s0 = if d[2] & 1 == 0 { 1.0_f32 } else { -1.0_f32 };
    let b1 = (u16::from_le_bytes([d[3], d[4]]) as usize) % EMBEDDING_DIM;
    let s1 = if d[5] & 1 == 0 { 1.0_f32 } else { -1.0_f32 };

    vec[b0] += s0 * weight;
    vec[b1] += s1 * (weight * 0.70);
}

/// Computes deterministic 64-dim L2-normalized embedding for a `CodeSceneAssetDto`
/// over `kind + normalized_tags + code_feature_tokens + title + description + controls_hint`.
pub fn embed_asset(asset: &CodeSceneAssetDto, code_tokens: &[String]) -> Vec<f32> {
    let mut acc = [0.0_f32; EMBEDDING_DIM];

    project_feature(&mut acc, "kind", asset.kind.as_str(), 2.2);

    let norm_tags = normalize_tags(&asset.tags);
    let eff_primary = effective_primary_tag(&norm_tags);
    for tag in &norm_tags {
        let w = if eff_primary.as_deref() == Some(tag.as_str()) {
            2.2
        } else {
            1.8
        };
        project_feature(&mut acc, "tag", tag, w);
        for concept in bilingual_concepts_for_text(tag) {
            project_feature(&mut acc, "concept", &concept, 0.9);
        }
    }

    for tok in code_tokens {
        project_feature(&mut acc, "code", tok, 1.1);
    }

    for tok in tokenize_bilingual(&asset.title) {
        project_feature(&mut acc, "title", &tok, 1.2);
        if tok.is_ascii() {
            for tri in char_trigrams(&tok) {
                project_feature(&mut acc, "tri", &tri, 0.35);
            }
        }
    }
    for concept in bilingual_concepts_for_text(&asset.title) {
        project_feature(&mut acc, "concept", &concept, 0.85);
    }

    for tok in tokenize_bilingual(&asset.description) {
        project_feature(&mut acc, "desc", &tok, 0.55);
        if tok.is_ascii() {
            for tri in char_trigrams(&tok) {
                project_feature(&mut acc, "tri", &tri, 0.20);
            }
        }
    }
    for concept in bilingual_concepts_for_text(&asset.description) {
        project_feature(&mut acc, "concept", &concept, 0.40);
    }

    for tok in tokenize_bilingual(&asset.controls_hint) {
        project_feature(&mut acc, "ctrl", &tok, 0.25);
    }

    let mut vec = acc.to_vec();
    l2_normalize(&mut vec);
    vec
}

/// Computes deterministic 64-dim L2-normalized embedding for a user search query.
pub fn embed_query(query: &str) -> Vec<f32> {
    let mut acc = [0.0_f32; EMBEDDING_DIM];
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return acc.to_vec();
    }

    if let Some(kind) = AssetKind::parse(trimmed) {
        project_feature(&mut acc, "kind", kind.as_str(), 2.0);
    }

    for tok in tokenize_bilingual(trimmed) {
        project_feature(&mut acc, "tag", &tok, 1.4);
        project_feature(&mut acc, "title", &tok, 1.3);
        project_feature(&mut acc, "desc", &tok, 0.6);
        if tok.is_ascii() {
            for tri in char_trigrams(&tok) {
                project_feature(&mut acc, "tri", &tri, 0.4);
            }
        }
    }
    for concept in bilingual_concepts_for_text(trimmed) {
        project_feature(&mut acc, "concept", &concept, 1.8);
        project_feature(&mut acc, "tag", &concept, 1.2);
    }

    let mut vec = acc.to_vec();
    l2_normalize(&mut vec);
    vec
}

/// Scrubs non-finite values and L2-normalizes `v` in place.
/// Returns `true` if `v` has unit norm after normalization, `false` if zero-norm.
pub fn l2_normalize(v: &mut [f32]) -> bool {
    let mut sum_sq = 0.0_f64;
    for x in v.iter_mut() {
        if !x.is_finite() {
            *x = 0.0;
        }
        let xf = *x as f64;
        sum_sq += xf * xf;
    }
    if !sum_sq.is_finite() || sum_sq <= 1e-18 {
        for x in v.iter_mut() {
            *x = 0.0;
        }
        return false;
    }
    let inv_norm = (1.0 / sum_sq.sqrt()) as f32;
    if !inv_norm.is_finite() {
        for x in v.iter_mut() {
            *x = 0.0;
        }
        return false;
    }
    for x in v.iter_mut() {
        *x *= inv_norm;
    }
    true
}

/// Computes `l2_normalize(0.65 * u_short + 0.35 * u_long - 0.30 * u_neg)`.
/// Returns a 64-dim vector (all-zero if empty/zero-norm).
pub fn effective_query_vector(u_short: &[f32], u_long: &[f32], u_neg: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0_f32; EMBEDDING_DIM];
    for (i, slot) in out.iter_mut().enumerate() {
        let s = u_short.get(i).copied().filter(|x| x.is_finite()).unwrap_or(0.0);
        let l = u_long.get(i).copied().filter(|x| x.is_finite()).unwrap_or(0.0);
        let n = u_neg.get(i).copied().filter(|x| x.is_finite()).unwrap_or(0.0);
        *slot = 0.65 * s + 0.35 * l - 0.30 * n;
    }
    l2_normalize(&mut out);
    out
}

/// NaN/zero-norm-safe cosine similarity clamped to `[-1.0, 1.0]`.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0_f64;
    let mut na = 0.0_f64;
    let mut nb = 0.0_f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        if !x.is_finite() || !y.is_finite() {
            return 0.0;
        }
        let xf = x as f64;
        let yf = y as f64;
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    if na <= 1e-18 || nb <= 1e-18 {
        return 0.0;
    }
    let denom = (na * nb).sqrt();
    if !denom.is_finite() || denom <= 1e-18 {
        return 0.0;
    }
    let sim = dot / denom;
    if sim.is_finite() {
        sim.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Jaccard set similarity in `[0.0, 1.0]` over string slices (returns `0.0` if both empty).
pub fn jaccard_similarity(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let set_a: HashSet<&str> = a.iter().map(String::as_str).collect();
    let set_b: HashSet<&str> = b.iter().map(String::as_str).collect();
    let inter = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 {
        0.0
    } else {
        (inter as f64) / (union as f64)
    }
}

/// Pairwise item similarity used by MMR and ILD:
/// `0.5 * max(0, cos(v_i, v_j)) + 0.3 * Jaccard(tags_i, tags_j) + 0.2 * same_kind(i, j)`.
pub fn item_similarity(
    emb_a: &[f32],
    tags_a: &[String],
    kind_a: AssetKind,
    emb_b: &[f32],
    tags_b: &[String],
    kind_b: AssetKind,
) -> f64 {
    let cos = cosine_similarity(emb_a, emb_b).clamp(0.0, 1.0);
    let jac = jaccard_similarity(tags_a, tags_b);
    let same_kind = if kind_a == kind_b { 1.0 } else { 0.0 };
    (0.5 * cos + 0.3 * jac + 0.2 * same_kind).clamp(0.0, 1.0)
}
