use crate::db::{ServerDatabase, SqliteResultExt};
use crate::search::hybrid_memory::HybridMemoryEngine;
use gpui_telegram_core::error::Result;
use gpui_telegram_proto::{
    BotRecommendationItem, CatalogQueryResponse, CatalogRecommendResponse, CatalogSortMode,
    CoverArtwork, CreationKind, PublicCatalogItemDto, UserCreationDto,
};
use rusqlite::{params, Row};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const CATALOG_EMBED_DIMS: usize = 64;
const MAX_CATALOG_LIMIT: usize = 100;
const MAX_RECOMMEND_LIMIT: usize = 50;
const MAX_CJK_BIGRAMS: usize = 12;

const PUBLIC_CATALOG_COLS: &str = r#"
    creation_id, owner_user_id, kind, title, subtitle, summary,
    author_display_name, language, genres_json, tags_json,
    genres_text, tags_text, searchable_text, cover_json, age_rating,
    snapshot_json, published_version, featured_rank,
    rating_avg, rating_count, engagement_count, save_or_install_count,
    popularity_score, published_at_ms, updated_at_ms
"#;

#[derive(Debug, Clone)]
struct PublicCatalogRow {
    creation_id: String,
    owner_user_id: i64,
    kind: CreationKind,
    title: String,
    subtitle: String,
    summary: String,
    author_display_name: String,
    language: String,
    genres: Vec<String>,
    tags: Vec<String>,
    genres_text: String,
    tags_text: String,
    searchable_text: String,
    cover: CoverArtwork,
    age_rating: String,
    snapshot_json: String,
    published_version: u32,
    featured_rank: Option<u32>,
    rating_avg: f32,
    rating_count: u32,
    engagement_count: u64,
    save_or_install_count: u64,
    popularity_score: f64,
    published_at_ms: i64,
    updated_at_ms: i64,
}

impl PublicCatalogRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let kind_s: String = row.get(2)?;
        let genres_s: String = row.get(8)?;
        let tags_s: String = row.get(9)?;
        let cover_s: String = row.get(13)?;
        Ok(Self {
            creation_id: row.get(0)?,
            owner_user_id: row.get(1)?,
            kind: CreationKind::parse(&kind_s).unwrap_or(CreationKind::Book),
            title: row.get(3)?,
            subtitle: row.get(4)?,
            summary: row.get(5)?,
            author_display_name: row.get(6)?,
            language: row.get(7)?,
            genres: serde_json::from_str(&genres_s).unwrap_or_default(),
            tags: serde_json::from_str(&tags_s).unwrap_or_default(),
            genres_text: row.get(10)?,
            tags_text: row.get(11)?,
            searchable_text: row.get(12)?,
            cover: serde_json::from_str(&cover_s).unwrap_or_default(),
            age_rating: row.get(14)?,
            snapshot_json: row.get(15)?,
            published_version: row.get::<_, i64>(16)?.max(1) as u32,
            featured_rank: row.get::<_, Option<i64>>(17)?.map(|v| v.max(0) as u32),
            rating_avg: row.get::<_, f64>(18)? as f32,
            rating_count: row.get::<_, i64>(19)?.max(0) as u32,
            engagement_count: row.get::<_, i64>(20)?.max(0) as u64,
            save_or_install_count: row.get::<_, i64>(21)?.max(0) as u64,
            popularity_score: row.get(22)?,
            published_at_ms: row.get(23)?,
            updated_at_ms: row.get(24)?,
        })
    }

    fn matches_genre(&self, genre: &str) -> bool {
        let g_lower = genre.trim().to_lowercase();
        if g_lower.is_empty() || g_lower == "all" {
            return true;
        }
        self.genres
            .iter()
            .any(|g| g.trim().to_lowercase() == g_lower)
    }

    fn to_dto(&self) -> PublicCatalogItemDto {
        let payload = serde_json::from_str::<UserCreationDto>(&self.snapshot_json)
            .ok()
            .map(|snap| snap.payload);
        PublicCatalogItemDto {
            creation_id: self.creation_id.clone(),
            owner_user_id: self.owner_user_id,
            kind: self.kind,
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            summary: self.summary.clone(),
            author_display_name: self.author_display_name.clone(),
            language: self.language.clone(),
            genres: self.genres.clone(),
            tags: self.tags.clone(),
            cover: self.cover.clone(),
            age_rating: self.age_rating.clone(),
            published_version: self.published_version,
            published_at_ms: self.published_at_ms,
            updated_at_ms: self.updated_at_ms,
            rating_avg: self.rating_avg,
            rating_count: self.rating_count,
            engagement_count: self.engagement_count,
            save_or_install_count: self.save_or_install_count,
            featured_rank: self.featured_rank,
            deep_link: self.kind.deep_link_uri(&self.creation_id),
            payload,
        }
    }

    fn combined_doc_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {}",
            self.title,
            self.subtitle,
            self.summary,
            self.author_display_name,
            self.genres_text,
            self.tags_text,
            self.searchable_text
        )
    }

    fn any_field_contains_lower(&self, needle_lower: &str) -> bool {
        self.title.to_lowercase().contains(needle_lower)
            || self.subtitle.to_lowercase().contains(needle_lower)
            || self.summary.to_lowercase().contains(needle_lower)
            || self.author_display_name.to_lowercase().contains(needle_lower)
            || self.genres_text.to_lowercase().contains(needle_lower)
            || self.tags_text.to_lowercase().contains(needle_lower)
            || self.searchable_text.to_lowercase().contains(needle_lower)
    }
}

pub(crate) fn is_cjk_char(c: char) -> bool {
    let cp = c as u32;
    matches!(
        cp,
        0x4E00..=0x9FFF
            | 0x3400..=0x4DBF
            | 0x20000..=0x2A6DF
            | 0x3040..=0x309F
            | 0x30A0..=0x30FF
            | 0xAC00..=0xD7AF
            | 0x3000..=0x303F
            | 0xFF00..=0xFFEF
    )
}

pub(crate) fn contains_cjk(text: &str) -> bool {
    text.chars().any(is_cjk_char)
}

pub(crate) fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

pub(crate) fn sanitize_fts5_query(query: &str) -> Option<String> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .filter(|tok| tok.chars().any(|c| c.is_alphanumeric() && !is_cjk_char(c)))
        .map(|tok| format!("\"{}\"", tok.replace('"', "\"\"")))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" "))
    }
}

fn extract_cjk_chars(text: &str) -> Vec<char> {
    text.chars().filter(|&c| is_cjk_char(c) && !matches!(c as u32, 0x3000..=0x303F | 0xFF00..=0xFFEF)).collect()
}

fn has_literal_like_wildcard(query: &str) -> bool {
    query.contains('%') || query.contains('_') || query.contains('\\')
}

fn extract_latin_alnum_terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut seen = HashSet::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() && !is_cjk_char(ch) {
            for lc in ch.to_lowercase() {
                current.push(lc);
            }
        } else if !current.is_empty() {
            if seen.insert(current.clone()) {
                terms.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }
    if !current.is_empty() && seen.insert(current.clone()) {
        terms.push(current);
    }
    terms
}

fn matches_mixed_en_cjk(
    row: &PublicCatalogRow,
    q_lower: &str,
    latin_terms: &[String],
    cjk_bigrams: &[String],
    cjk_chars: &[char],
) -> bool {
    if row.any_field_contains_lower(q_lower) {
        return true;
    }
    let doc_lower = row.combined_doc_text().to_lowercase();
    if !latin_terms
        .iter()
        .all(|term| doc_lower.contains(term.as_str()))
    {
        return false;
    }
    if !cjk_bigrams.is_empty() {
        cjk_bigrams.iter().any(|bg| doc_lower.contains(bg.as_str()))
    } else {
        cjk_chars.iter().all(|&ch| doc_lower.contains(ch))
    }
}

fn extract_cjk_bigrams(text: &str) -> Vec<String> {
    let chars = extract_cjk_chars(text);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for pair in chars.windows(2) {
        let mut s = String::with_capacity(6);
        s.push(pair[0]);
        s.push(pair[1]);
        if seen.insert(s.clone()) {
            out.push(s);
            if out.len() >= MAX_CJK_BIGRAMS {
                break;
            }
        }
    }
    out
}

fn compute_text_relevance(
    row: &PublicCatalogRow,
    q_raw: &str,
    q_lower: &str,
    cjk_bigrams: &[String],
    cjk_chars: &[char],
    query_embed: &[f32],
    fts_bonus: f64,
) -> f64 {
    let title_l = row.title.to_lowercase();
    let sub_l = row.subtitle.to_lowercase();
    let sum_l = row.summary.to_lowercase();
    let author_l = row.author_display_name.to_lowercase();
    let genres_l = row.genres_text.to_lowercase();
    let tags_l = row.tags_text.to_lowercase();
    let search_l = row.searchable_text.to_lowercase();

    let mut score = fts_bonus;
    let mut exact_hit = false;

    if title_l.contains(q_lower) {
        score += 20.0;
        exact_hit = true;
    }
    if tags_l.contains(q_lower) || genres_l.contains(q_lower) {
        score += 12.0;
        exact_hit = true;
    }
    if sub_l.contains(q_lower) || author_l.contains(q_lower) {
        score += 10.0;
        exact_hit = true;
    }
    if sum_l.contains(q_lower) {
        score += 8.0;
        exact_hit = true;
    }
    if search_l.contains(q_lower) {
        score += 5.0;
        exact_hit = true;
    }

    if !cjk_bigrams.is_empty() {
        let mut title_hits = 0usize;
        let mut meta_hits = 0usize;
        let mut body_hits = 0usize;
        for bg in cjk_bigrams {
            if row.title.contains(bg.as_str()) {
                title_hits += 1;
            }
            if row.tags_text.contains(bg.as_str())
                || row.genres_text.contains(bg.as_str())
                || row.subtitle.contains(bg.as_str())
                || row.author_display_name.contains(bg.as_str())
            {
                meta_hits += 1;
            }
            if row.summary.contains(bg.as_str()) || row.searchable_text.contains(bg.as_str()) {
                body_hits += 1;
            }
        }
        let denom = cjk_bigrams.len() as f64;
        score += (title_hits as f64 / denom) * 14.0;
        score += (meta_hits as f64 / denom) * 9.0;
        score += (body_hits as f64 / denom) * 6.0;
        if exact_hit || title_hits > 0 || meta_hits > 0 || body_hits > 0 {
            let unique_chars: HashSet<char> = cjk_chars.iter().copied().collect();
            if !unique_chars.is_empty() {
                let doc_all = row.combined_doc_text();
                let uni_hits = unique_chars.iter().filter(|&&c| doc_all.contains(c)).count();
                score += (uni_hits as f64 / unique_chars.len() as f64) * 3.0;
            }
        }
    } else if !cjk_chars.is_empty() && exact_hit {
        score += 4.0;
    }

    if !q_raw.is_empty() {
        let doc_embed = HybridMemoryEngine::embed_text(&row.combined_doc_text(), CATALOG_EMBED_DIMS);
        let sim = HybridMemoryEngine::cosine_similarity(query_embed, &doc_embed);
        if sim.is_finite() && sim > 0.0 {
            score += (sim as f64) * 4.0;
        }
    }

    score
}

fn cmp_f64_desc(a: f64, b: f64) -> Ordering {
    b.partial_cmp(&a).unwrap_or(Ordering::Equal)
}

fn cmp_featured_rank(a: Option<u32>, b: Option<u32>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn sort_scored_rows(entries: &mut [(PublicCatalogRow, f64)], sort: CatalogSortMode, has_query: bool) {
    entries.sort_by(|(a, rel_a), (b, rel_b)| match sort {
        CatalogSortMode::Featured => {
            if has_query {
                cmp_f64_desc(*rel_a, *rel_b)
                    .then_with(|| cmp_featured_rank(a.featured_rank, b.featured_rank))
                    .then_with(|| cmp_f64_desc(a.popularity_score, b.popularity_score))
                    .then_with(|| b.published_at_ms.cmp(&a.published_at_ms))
                    .then_with(|| a.creation_id.cmp(&b.creation_id))
            } else {
                cmp_featured_rank(a.featured_rank, b.featured_rank)
                    .then_with(|| cmp_f64_desc(a.popularity_score, b.popularity_score))
                    .then_with(|| b.published_at_ms.cmp(&a.published_at_ms))
                    .then_with(|| a.creation_id.cmp(&b.creation_id))
            }
        }
        CatalogSortMode::Popular => {
            let pop_a = a.engagement_count.saturating_add(a.save_or_install_count);
            let pop_b = b.engagement_count.saturating_add(b.save_or_install_count);
            pop_b
                .cmp(&pop_a)
                .then_with(|| cmp_f64_desc(a.popularity_score, b.popularity_score))
                .then_with(|| cmp_f64_desc(*rel_a, *rel_b))
                .then_with(|| b.published_at_ms.cmp(&a.published_at_ms))
                .then_with(|| a.creation_id.cmp(&b.creation_id))
        }
        CatalogSortMode::TopRated => cmp_f64_desc(a.rating_avg as f64, b.rating_avg as f64)
            .then_with(|| b.rating_count.cmp(&a.rating_count))
            .then_with(|| cmp_f64_desc(*rel_a, *rel_b))
            .then_with(|| b.published_at_ms.cmp(&a.published_at_ms))
            .then_with(|| a.creation_id.cmp(&b.creation_id)),
        CatalogSortMode::Newest => b
            .published_at_ms
            .cmp(&a.published_at_ms)
            .then_with(|| b.updated_at_ms.cmp(&a.updated_at_ms))
            .then_with(|| cmp_f64_desc(*rel_a, *rel_b))
            .then_with(|| a.creation_id.cmp(&b.creation_id)),
    });
}

/// Physically isolated public catalog search & recommendation service.
/// Reads EXCLUSIVELY from `public_catalog_entries` and `public_catalog_fts` on read-only connections.
#[derive(Debug, Clone)]
pub struct PublicCatalogSearchService {
    db: Arc<ServerDatabase>,
}

impl PublicCatalogSearchService {
    pub fn new(db: Arc<ServerDatabase>) -> Self {
        Self { db }
    }

    fn query_candidates(
        &self,
        kind: Option<CreationKind>,
        query: Option<&str>,
        genre: Option<&str>,
    ) -> Result<Vec<(PublicCatalogRow, f64)>> {
        let q_untrimmed = query.unwrap_or("");
        if q_untrimmed.contains('\0') {
            return Ok(Vec::new());
        }
        let q_raw = q_untrimmed.trim();
        let genre_clean = genre
            .map(str::trim)
            .filter(|g| !g.is_empty() && !g.eq_ignore_ascii_case("all"));

        self.db.pool().with_reader(|conn| {
            if q_raw.is_empty() {
                let rows = if let Some(k) = kind {
                    let mut stmt = conn
                        .prepare(&format!(
                            "SELECT {PUBLIC_CATALOG_COLS} FROM public_catalog_entries WHERE kind = ?1"
                        ))
                        .map_db()?;
                    let collected = stmt
                        .query_map(params![k.as_str()], PublicCatalogRow::from_row)
                        .map_db()?
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_db()?;
                    collected
                } else {
                    let mut stmt = conn
                        .prepare(&format!(
                            "SELECT {PUBLIC_CATALOG_COLS} FROM public_catalog_entries"
                        ))
                        .map_db()?;
                    let collected = stmt
                        .query_map([], PublicCatalogRow::from_row)
                        .map_db()?
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_db()?;
                    collected
                };
                return Ok(rows
                    .into_iter()
                    .filter(|r| genre_clean.is_none_or(|g| r.matches_genre(g)))
                    .map(|r| (r, 0.0))
                    .collect());
            }

            let mut candidates: HashMap<String, (PublicCatalogRow, f64)> = HashMap::new();
            let q_lower = q_raw.to_lowercase();
            let literal_wildcard = has_literal_like_wildcard(q_raw);
            let has_cjk = contains_cjk(q_raw);
            let cjk_chars = if has_cjk {
                extract_cjk_chars(q_raw)
            } else {
                Vec::new()
            };
            let cjk_bigrams = if has_cjk && !literal_wildcard {
                extract_cjk_bigrams(q_raw)
            } else {
                Vec::new()
            };
            let latin_terms = if has_cjk && !literal_wildcard {
                extract_latin_alnum_terms(q_raw)
            } else {
                Vec::new()
            };
            let is_mixed_en_cjk = !literal_wildcard && !latin_terms.is_empty() && !cjk_chars.is_empty();

            // 1. Sanitized FTS5 weighted BM25 search for Latin/alphanumeric tokens.
            // Bypass FTS5 when the query contains literal SQL LIKE wildcards (`%`, `_`, `\`)
            // so `unicode61` cannot strip them and return false matches.
            let fts_query_opt = if literal_wildcard {
                None
            } else if is_mixed_en_cjk {
                Some(
                    latin_terms
                        .iter()
                        .map(|tok| format!("\"{}\"", tok.replace('"', "\"\"")))
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            } else {
                sanitize_fts5_query(q_raw)
            };

            if let Some(fts_query) = fts_query_opt {
                let fts_sql = format!(
                    r#"
                    SELECT p.creation_id, p.owner_user_id, p.kind, p.title, p.subtitle, p.summary,
                           p.author_display_name, p.language, p.genres_json, p.tags_json,
                           p.genres_text, p.tags_text, p.searchable_text, p.cover_json, p.age_rating,
                           p.snapshot_json, p.published_version, p.featured_rank,
                           p.rating_avg, p.rating_count, p.engagement_count, p.save_or_install_count,
                           p.popularity_score, p.published_at_ms, p.updated_at_ms,
                           bm25(public_catalog_fts, 0.0, 0.0, 10.0, 5.0, 3.0, 4.0, 4.0, 5.0, 1.0) AS bm25_rank
                    FROM public_catalog_fts
                    JOIN public_catalog_entries p ON p.creation_id = public_catalog_fts.creation_id
                    WHERE public_catalog_fts MATCH ?1
                    {}
                    ORDER BY bm25_rank ASC
                    LIMIT 200
                    "#,
                    if kind.is_some() { "AND p.kind = ?2" } else { "" }
                );
                if let Ok(mut stmt) = conn.prepare(&fts_sql) {
                    let map_fts_row = |row: &Row<'_>| -> rusqlite::Result<(PublicCatalogRow, f64)> {
                        let rec = PublicCatalogRow::from_row(row)?;
                        let rank: f64 = row.get(25)?;
                        let bonus = if rank.is_finite() { (-rank).max(0.5) } else { 0.5 };
                        Ok((rec, bonus))
                    };
                    let fts_rows = if let Some(k) = kind {
                        stmt.query_map(params![fts_query, k.as_str()], map_fts_row)
                            .ok()
                            .map(|iter| iter.filter_map(std::result::Result::ok).collect::<Vec<_>>())
                            .unwrap_or_default()
                    } else {
                        stmt.query_map(params![fts_query], map_fts_row)
                            .ok()
                            .map(|iter| iter.filter_map(std::result::Result::ok).collect::<Vec<_>>())
                            .unwrap_or_default()
                    };
                    for (rec, bonus) in fts_rows {
                        candidates.insert(rec.creation_id.clone(), (rec, bonus));
                    }
                }
            }

            // 2. Deduplicated escaped LIKE substring search (handles CJK, substrings & literal wildcards).
            let mut seen_patterns: HashSet<String> = HashSet::new();
            let mut like_patterns: Vec<String> = Vec::new();
            let full_pat = format!("%{}%", escape_like(&q_lower));
            if seen_patterns.insert(full_pat.clone()) {
                like_patterns.push(full_pat);
            }
            for bg in &cjk_bigrams {
                let pat = format!("%{}%", escape_like(bg));
                if seen_patterns.insert(pat.clone()) {
                    like_patterns.push(pat);
                }
            }
            if is_mixed_en_cjk && cjk_bigrams.is_empty() {
                if let Some(&first_cjk) = cjk_chars.first() {
                    let pat = format!("%{}%", escape_like(&first_cjk.to_string()));
                    if seen_patterns.insert(pat.clone()) {
                        like_patterns.push(pat);
                    }
                }
            }

            let like_sql = format!(
                r#"
                SELECT {PUBLIC_CATALOG_COLS}
                FROM public_catalog_entries
                WHERE (
                    LOWER(title) LIKE ?1 ESCAPE '\'
                    OR LOWER(subtitle) LIKE ?1 ESCAPE '\'
                    OR LOWER(summary) LIKE ?1 ESCAPE '\'
                    OR LOWER(author_display_name) LIKE ?1 ESCAPE '\'
                    OR LOWER(genres_text) LIKE ?1 ESCAPE '\'
                    OR LOWER(tags_text) LIKE ?1 ESCAPE '\'
                    OR LOWER(searchable_text) LIKE ?1 ESCAPE '\'
                )
                {}
                LIMIT 200
                "#,
                if kind.is_some() { "AND kind = ?2" } else { "" }
            );
            let mut like_stmt = conn.prepare(&like_sql).map_db()?;
            for pat in &like_patterns {
                let rows = if let Some(k) = kind {
                    like_stmt
                        .query_map(params![pat, k.as_str()], PublicCatalogRow::from_row)
                        .map_db()?
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_db()?
                } else {
                    like_stmt
                        .query_map(params![pat], PublicCatalogRow::from_row)
                        .map_db()?
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_db()?
                };
                for rec in rows {
                    candidates.entry(rec.creation_id.clone()).or_insert((rec, 0.0));
                }
            }

            let query_embed = HybridMemoryEngine::embed_text(q_raw, CATALOG_EMBED_DIMS);

            let scored: Vec<(PublicCatalogRow, f64)> = candidates
                .into_values()
                .filter(|(r, _)| genre_clean.is_none_or(|g| r.matches_genre(g)))
                .filter(|(r, _)| !literal_wildcard || r.any_field_contains_lower(&q_lower))
                .filter(|(r, _)| {
                    !is_mixed_en_cjk
                        || matches_mixed_en_cjk(r, &q_lower, &latin_terms, &cjk_bigrams, &cjk_chars)
                })
                .map(|(r, fts_bonus)| {
                    let rel = compute_text_relevance(
                        &r,
                        q_raw,
                        &q_lower,
                        &cjk_bigrams,
                        &cjk_chars,
                        &query_embed,
                        fts_bonus,
                    );
                    (r, rel)
                })
                .collect();

            Ok(scored)
        })
    }
}

impl PublicCatalogSearchService {
    /// Searches and sorts published+public catalog entries with exact pagination `total`.
    pub fn search_catalog(
        &self,
        kind: Option<CreationKind>,
        query: Option<&str>,
        genre: Option<&str>,
        sort: CatalogSortMode,
        limit: usize,
        offset: usize,
    ) -> Result<CatalogQueryResponse> {
        let has_query = query.map(str::trim).is_some_and(|q| !q.is_empty());
        let mut entries = self.query_candidates(kind, query, genre)?;
        sort_scored_rows(&mut entries, sort, has_query);

        let total = entries.len();
        if limit == 0 || offset >= total {
            return Ok(CatalogQueryResponse {
                items: Vec::new(),
                total,
            });
        }
        let clamped_limit = limit.min(MAX_CATALOG_LIMIT);
        let items = entries
            .into_iter()
            .skip(offset)
            .take(clamped_limit)
            .map(|(row, _)| row.to_dto())
            .collect();
        Ok(CatalogQueryResponse { items, total })
    }

    /// Fetches published+public cross-vertical creations (`Book`, `Music`, `Game`) for App Store merge.
    pub fn list_cross_vertical(
        &self,
        query: Option<&str>,
        genre: Option<&str>,
        sort: CatalogSortMode,
        limit: usize,
    ) -> Result<Vec<PublicCatalogItemDto>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let has_query = query.map(str::trim).is_some_and(|q| !q.is_empty());
        let mut entries: Vec<(PublicCatalogRow, f64)> = self
            .query_candidates(None, query, genre)?
            .into_iter()
            .filter(|(r, _)| r.kind != CreationKind::App)
            .collect();
        sort_scored_rows(&mut entries, sort, has_query);
        Ok(entries
            .into_iter()
            .take(limit.min(MAX_CATALOG_LIMIT))
            .map(|(row, _)| row.to_dto())
            .collect())
    }

    /// Composite recommendation ranker over published+public catalog entries ONLY.
    pub fn recommend(
        &self,
        query: Option<&str>,
        kind: Option<CreationKind>,
        genre: Option<&str>,
        limit: usize,
    ) -> Result<CatalogRecommendResponse> {
        if limit == 0 || query.is_some_and(|q| q.contains('\0')) {
            return Ok(CatalogRecommendResponse { items: Vec::new() });
        }
        let clamped_limit = limit.min(MAX_RECOMMEND_LIMIT);
        let q_clean = query.unwrap_or("").trim();
        let genre_clean = genre
            .map(str::trim)
            .filter(|g| !g.is_empty() && !g.eq_ignore_ascii_case("all"));
        let candidates = self.query_candidates(
            kind,
            if q_clean.is_empty() {
                None
            } else {
                Some(q_clean)
            },
            genre_clean,
        )?;

        let mut ranked: Vec<(PublicCatalogRow, f64)> = candidates
            .into_iter()
            .map(|(row, rel)| {
                let bayesian = ((row.rating_avg as f64) * (row.rating_count as f64) + 3.5 * 2.0)
                    / ((row.rating_count as f64) + 2.0);
                let pop = ((row.engagement_count.saturating_add(row.save_or_install_count)) as f64).ln_1p();
                let featured_boost = match row.featured_rank {
                    Some(rank) => 4.0 / ((rank as f64) + 1.0),
                    None => 0.0,
                };
                let genre_boost = if genre_clean.is_some_and(|g| row.matches_genre(g)) {
                    2.5
                } else {
                    0.0
                };
                let composite = if q_clean.is_empty() {
                    bayesian * 2.5 + pop * 1.2 + featured_boost + genre_boost
                } else {
                    rel * 3.0 + bayesian * 1.8 + pop * 0.8 + featured_boost + genre_boost
                };
                (row, composite)
            })
            .collect();

        ranked.sort_by(|(a, sa), (b, sb)| {
            cmp_f64_desc(*sa, *sb)
                .then_with(|| b.published_at_ms.cmp(&a.published_at_ms))
                .then_with(|| a.creation_id.cmp(&b.creation_id))
        });

        let items = ranked
            .into_iter()
            .take(clamped_limit)
            .map(|(row, score)| {
                let rating_note = if row.rating_count > 0 {
                    format!("{:.1}★ ({})", row.rating_avg, row.rating_count)
                } else {
                    "New Release".to_string()
                };
                let reason = if !q_clean.is_empty() {
                    format!("Matches \"{q_clean}\" • {rating_note}")
                } else if let Some(g) = genre_clean {
                    format!("Top {g} {} • {rating_note}", row.kind.as_str())
                } else {
                    format!("Featured {} • {rating_note}", row.kind.as_str())
                };
                BotRecommendationItem {
                    deep_link: row.kind.deep_link_uri(&row.creation_id),
                    creation_id: row.creation_id,
                    kind: row.kind,
                    title: row.title,
                    subtitle: row.subtitle,
                    summary: row.summary,
                    author_display_name: row.author_display_name,
                    genres: row.genres,
                    cover: row.cover,
                    rating_avg: row.rating_avg,
                    rating_count: row.rating_count,
                    score: score as f32,
                    reason,
                }
            })
            .collect();

        Ok(CatalogRecommendResponse { items })
    }
}
