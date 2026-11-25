//! Community lists, trending topics, Tantivy BM25 + SQL search, and notifications endpoints.

use crate::db::ServerDatabase;
use crate::routes::community::{map_community_err, sanitize_post_embedded_refs_conn, ActorAuth};
use crate::routes::err_resp;
use crate::server::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use chrono::Utc;
use gpui_telegram_community::model::parse_search_operators;
use gpui_telegram_community::store::ops;
use gpui_telegram_community::CommunitySearchIndexer;
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_proto::community_contracts::{
    ActorKind, CommunityActorDto, CommunityListsResponse, CommunityNotificationKind,
    CommunityPostDto, CommunitySearchRequestDto, CommunitySearchResultDto, CommunitySearchSort,
    CommunitySearchTab, CreateCommunityListRequest, DeleteEntityResponse,
    MarkNotificationsReadRequest, MarkNotificationsReadResponse, PostStatus, SubCommunityDto,
    TrendingResponse, UpdateCommunityListRequest,
};
use rusqlite::params;
use serde::Deserialize;
use std::sync::Arc;

pub async fn list_lists(auth: ActorAuth, State(state): State<Arc<AppState>>) -> Response {
    match state
        .db
        .pool()
        .with_reader(|conn| ops::list_lists_conn(conn, &auth.actor_id))
    {
        Ok(lists) => (StatusCode::OK, Json(CommunityListsResponse { lists })).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn create_list(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateCommunityListRequest>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .pool()
        .with_transaction(|tx| ops::create_list_tx(tx, &auth.actor_id, &req, now_ms))
    {
        Ok(created) => (StatusCode::CREATED, Json(created)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn update_list(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(list_id): Path<String>,
    Json(req): Json<UpdateCommunityListRequest>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .pool()
        .with_transaction(|tx| ops::update_list_tx(tx, &list_id, &auth.actor_id, &req, now_ms))
    {
        Ok(updated) => (StatusCode::OK, Json(updated)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn delete_list(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Path(list_id): Path<String>,
) -> Response {
    match state
        .db
        .pool()
        .with_transaction(|tx| ops::delete_list_tx(tx, &list_id, &auth.actor_id))
    {
        Ok(deleted) => (
            StatusCode::OK,
            Json(DeleteEntityResponse {
                id: list_id,
                deleted,
            }),
        )
            .into_response(),
        Err(e) => map_community_err(e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct TrendingQuery {
    pub limit: Option<usize>,
}

pub async fn get_trending(
    _auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<TrendingQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(12).clamp(1, 50);
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .pool()
        .with_reader(|conn| ops::get_trending_conn(conn, now_ms, limit))
    {
        Ok(topics) => (
            StatusCode::OK,
            Json(TrendingResponse {
                topics,
                generated_at_ms: now_ms,
            }),
        )
            .into_response(),
        Err(e) => map_community_err(e),
    }
}

pub fn parse_search_tab(raw: &str) -> CommunitySearchTab {
    match raw.trim().to_ascii_lowercase().as_str() {
        "latest" => CommunitySearchTab::Latest,
        "people" | "humans" => CommunitySearchTab::People,
        "bots" => CommunitySearchTab::Bots,
        "communities" | "subcommunities" => CommunitySearchTab::Communities,
        "media" => CommunitySearchTab::Media,
        "polls" => CommunitySearchTab::Polls,
        _ => CommunitySearchTab::Top,
    }
}

pub fn parse_search_sort(raw: &str) -> CommunitySearchSort {
    match raw.trim().to_ascii_lowercase().as_str() {
        "latest" | "new" => CommunitySearchSort::Latest,
        "top" => CommunitySearchSort::Top,
        _ => CommunitySearchSort::Relevance,
    }
}

fn search_actors_sql(
    conn: &rusqlite::Connection,
    viewer_actor_id: Option<&str>,
    needle: &str,
    kind_filter: Option<ActorKind>,
    limit: usize,
) -> Result<Vec<CommunityActorDto>, TelegramError> {
    let pattern = format!("%{}%", needle.trim().to_ascii_lowercase());
    let mut stmt = conn
        .prepare(
            "SELECT actor_id FROM community_actors
             WHERE is_suspended = 0
               AND (
                   ?1 = '%%'
                   OR LOWER(handle) LIKE ?1
                   OR LOWER(display_name) LIKE ?1
                   OR LOWER(bio) LIKE ?1
                   OR LOWER(COALESCE(bot_persona, '')) LIKE ?1
                   OR LOWER(bot_capabilities_json) LIKE ?1
               )
             ORDER BY followers_count DESC, post_karma DESC
             LIMIT 100",
        )
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    let ids: Vec<String> = stmt
        .query_map(params![pattern], |r| r.get::<_, String>(0))
        .map_err(|e| TelegramError::Database(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| TelegramError::Database(e.to_string()))?;

    let mut out = Vec::new();
    for id in ids {
        if let Some(actor) = ops::get_actor_conn(conn, &id, viewer_actor_id)? {
            let kind_ok = match kind_filter {
                None => true,
                Some(ActorKind::Human) => actor.actor_kind == ActorKind::Human,
                Some(ActorKind::Bot) => actor.actor_kind.is_bot_like(),
                Some(ActorKind::HybridAgent) => actor.actor_kind == ActorKind::HybridAgent,
            };
            if kind_ok {
                out.push(actor);
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    Ok(out)
}

fn search_communities_sql(
    conn: &rusqlite::Connection,
    viewer_actor_id: Option<&str>,
    needle: &str,
    limit: usize,
) -> Result<Vec<SubCommunityDto>, TelegramError> {
    let pattern = format!("%{}%", needle.trim().to_ascii_lowercase());
    let mut stmt = conn
        .prepare(
            "SELECT community_id FROM communities
             WHERE ?1 = '%%'
                OR LOWER(slug) LIKE ?1
                OR LOWER(name) LIKE ?1
                OR LOWER(description) LIKE ?1
                OR LOWER(category) LIKE ?1
             ORDER BY members_count DESC, posts_count DESC
             LIMIT ?2",
        )
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    let ids: Vec<String> = stmt
        .query_map(params![pattern, limit as i64], |r| r.get::<_, String>(0))
        .map_err(|e| TelegramError::Database(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    let mut out = Vec::with_capacity(ids.len());
    for cid in ids {
        if let Some(c) = ops::get_community_conn(conn, &cid, viewer_actor_id)? {
            out.push(c);
        }
    }
    Ok(out)
}

/// Shared search execution combining Tantivy BM25 (`CommunitySearchIndexer`) with SQL hydration
/// across posts, actors (Humans & Bots), and communities.
pub fn execute_community_search(
    db: &ServerDatabase,
    indexer: Option<&CommunitySearchIndexer>,
    viewer_actor_id: Option<&str>,
    req: &CommunitySearchRequestDto,
) -> Result<CommunitySearchResultDto, TelegramError> {
    let limit = req.limit.clamp(1, 50);
    let offset = req.offset;
    let parsed_ops = parse_search_operators(&req.query);
    let text_needle = if parsed_ops.free_text.trim().is_empty() {
        req.query.trim().to_string()
    } else {
        parsed_ops.free_text.trim().to_string()
    };

    let want_posts = matches!(
        req.tab,
        CommunitySearchTab::Top
            | CommunitySearchTab::Latest
            | CommunitySearchTab::Media
            | CommunitySearchTab::Polls
    );
    let want_actors = matches!(
        req.tab,
        CommunitySearchTab::Top | CommunitySearchTab::People | CommunitySearchTab::Bots
    );
    let want_communities = matches!(
        req.tab,
        CommunitySearchTab::Top | CommunitySearchTab::Communities
    );

    let mut effective_req = req.clone();
    effective_req.limit = limit;
    effective_req.offset = offset;
    if req.tab == CommunitySearchTab::Latest {
        effective_req.sort = CommunitySearchSort::Latest;
    } else if req.tab == CommunitySearchTab::Media {
        effective_req.has_media = Some(true);
    } else if req.tab == CommunitySearchTab::Polls {
        effective_req.has_poll = Some(true);
    }

    let mut posts: Vec<CommunityPostDto> = Vec::new();
    let mut post_total = 0usize;

    if want_posts {
        let hit_ids: Vec<String> = if let Some(idx) = indexer {
            let mut probe_req = effective_req.clone();
            probe_req.limit = limit.saturating_add(1);
            let hits = idx.search_hits(&probe_req)?;
            post_total = hits.len();
            hits.into_iter().take(limit).map(|h| h.post_id).collect()
        } else {
            db.pool().with_reader(|conn| {
                let pat = format!("%{}%", text_needle.to_ascii_lowercase());
                let mut stmt = conn
                    .prepare(
                        "SELECT post_id FROM community_posts
                         WHERE status = 'published' AND is_removed = 0
                           AND (?1 = '%%' OR LOWER(COALESCE(title,'')) LIKE ?1 OR LOWER(body_markdown) LIKE ?1)
                         ORDER BY created_at_ms DESC LIMIT ?2 OFFSET ?3",
                    )
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
                let ids: Vec<String> = stmt
                    .query_map(params![pat, limit as i64, offset as i64], |r| {
                        r.get::<_, String>(0)
                    })
                    .map_err(|e| TelegramError::Database(e.to_string()))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
                Ok(ids)
            })?
        };

        db.pool().with_reader(|conn| {
            for pid in hit_ids {
                if let Some(mut p) = ops::get_post_conn(conn, &pid, viewer_actor_id)? {
                    if p.status == PostStatus::Published && !p.is_removed {
                        sanitize_post_embedded_refs_conn(conn, &mut p);
                        posts.push(p);
                    }
                }
            }
            Ok(())
        })?;
    }

    let (actors, communities) = db.pool().with_reader(|conn| {
        let actors = if want_actors {
            let kind = match req.tab {
                CommunitySearchTab::People => Some(ActorKind::Human),
                CommunitySearchTab::Bots => Some(ActorKind::Bot),
                _ => req.actor_kind,
            };
            let lim = if req.tab == CommunitySearchTab::Top {
                limit.min(8)
            } else {
                limit
            };
            search_actors_sql(conn, viewer_actor_id, &text_needle, kind, lim)?
        } else {
            Vec::new()
        };

        let communities = if want_communities {
            let lim = if req.tab == CommunitySearchTab::Top {
                limit.min(8)
            } else {
                limit
            };
            search_communities_sql(conn, viewer_actor_id, &text_needle, lim)?
        } else {
            Vec::new()
        };

        Ok((actors, communities))
    })?;

    let has_more_posts = want_posts && post_total > limit;
    let total_hits = posts.len() + actors.len() + communities.len();
    let next_offset = has_more_posts.then(|| offset.saturating_add(posts.len()));

    Ok(CommunitySearchResultDto {
        query: req.query.clone(),
        tab: req.tab,
        posts,
        actors,
        communities,
        total_hits,
        next_offset,
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct SearchQueryParams {
    pub query: Option<String>,
    pub q: Option<String>,
    pub tab: Option<String>,
    pub sort: Option<String>,
    pub community_slug: Option<String>,
    pub community: Option<String>,
    pub author_handle: Option<String>,
    pub author: Option<String>,
    pub flair: Option<String>,
    pub actor_kind: Option<String>,
    pub has_media: Option<bool>,
    pub has_poll: Option<bool>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub async fn search_community(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQueryParams>,
) -> Response {
    let query_str = q
        .query
        .or(q.q)
        .unwrap_or_default();
    let req = CommunitySearchRequestDto {
        query: query_str,
        tab: q.tab.as_deref().map(parse_search_tab).unwrap_or_default(),
        sort: q.sort.as_deref().map(parse_search_sort).unwrap_or_default(),
        community_slug: q
            .community_slug
            .or(q.community)
            .filter(|s| !s.trim().is_empty()),
        author_handle: q
            .author_handle
            .or(q.author)
            .filter(|s| !s.trim().is_empty()),
        flair: q.flair.filter(|s| !s.trim().is_empty()),
        actor_kind: q.actor_kind.as_deref().map(str::trim).and_then(ActorKind::parse),
        has_media: q.has_media,
        has_poll: q.has_poll,
        limit: q.limit.unwrap_or(25).clamp(1, 50),
        offset: q.offset.unwrap_or(0),
    };

    match execute_community_search(
        &state.db,
        Some(&state.community_search_indexer),
        Some(&auth.actor_id),
        &req,
    ) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_community_err(e),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct NotificationsQuery {
    pub kind: Option<String>,
    pub limit: Option<usize>,
}

pub async fn list_notifications(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<NotificationsQuery>,
) -> Response {
    let kind = query
        .kind
        .as_deref()
        .map(str::trim)
        .and_then(CommunityNotificationKind::parse);
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    match state
        .db
        .pool()
        .with_reader(|conn| ops::list_notifications_conn(conn, &auth.actor_id, kind, limit))
    {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_community_err(e),
    }
}

pub async fn mark_notifications_read(
    auth: ActorAuth,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: MarkNotificationsReadRequest = if body.is_empty() {
        MarkNotificationsReadRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_json",
                    &format!("Invalid notifications/read payload: {e}"),
                )
            }
        }
    };

    let res = state.db.pool().with_transaction(|tx| {
        let marked_count =
            ops::mark_notifications_read_tx(tx, &auth.actor_id, req.notif_ids.as_deref())?;
        let remaining = ops::list_notifications_conn(tx, &auth.actor_id, None, 1)?;
        Ok(MarkNotificationsReadResponse {
            marked_count,
            unread_count: remaining.unread_count,
        })
    });
    match res {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => map_community_err(e),
    }
}
