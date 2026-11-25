use crate::db::CreationOpError;
use crate::routes::hermes::caller_is_admin;
use crate::routes::{err_resp, map_telegram_err};
use crate::search::PublicCatalogSearchService;
use crate::server::{AppState, AuthBearer};
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use chrono::Utc;
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use gpui_telegram_proto::{
    AppStoreCatalogResponse, AppStoreMiniAppEntry, CatalogSortMode, CreateCreationRequest,
    CreateCreationReviewRequest, CreationKind, CreationReviewStatus, PublicCatalogItemDto,
    ReviewDecisionRequest, SubmitGameScoreRequest, SubmitReviewRequest, SubmitReviewResponse,
    UpdateCreationRequest, UpsertCreationStateRequest, UserCreationDto,
};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const ADHOC_WEBAPP_CATEGORY: &str = "__bot_inline_webapp__";

fn map_creation_err(err: CreationOpError) -> Response {
    match err {
        CreationOpError::NotFound => {
            err_resp(StatusCode::NOT_FOUND, "not_found", "Creation not found")
        }
        CreationOpError::Forbidden(msg) => err_resp(StatusCode::FORBIDDEN, "forbidden", msg),
        CreationOpError::RevisionConflict { expected, current } => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "revision_conflict",
                "message": format!("expected revision {expected} does not match current revision {current}"),
                "expected_revision": expected,
                "current_revision": current,
            })),
        )
            .into_response(),
        CreationOpError::InvalidState { code, message } => {
            err_resp(StatusCode::CONFLICT, code, &message)
        }
        CreationOpError::ValidationFailed(errors) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "validation_failed",
                "message": "Creation failed review validation",
                "validation_errors": errors,
            })),
        )
            .into_response(),
        CreationOpError::BadRequest { code, message } => {
            err_resp(StatusCode::BAD_REQUEST, code, &message)
        }
        CreationOpError::Db(e) => map_telegram_err(e),
    }
}

fn parse_required_json<T: DeserializeOwned>(body: &Bytes) -> Result<T, Response> {
    if body.is_empty() {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "empty_body",
            "Request body cannot be empty",
        ));
    }
    serde_json::from_slice(body).map_err(|e| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            &format!("Malformed JSON payload: {e}"),
        )
    })
}

#[derive(Debug, Default, serde::Deserialize)]
struct EmptyActionBody {}

fn parse_optional_json<T: DeserializeOwned + Default>(body: &Bytes) -> Result<T, Response> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            &format!("Malformed JSON payload: {e}"),
        )
    })
}

fn parse_optional_kind(params: &HashMap<String, String>) -> Result<Option<CreationKind>, Response> {
    let Some(raw) = params.get("kind").map(|s| s.trim()) else {
        return Ok(None);
    };
    if raw.is_empty() || raw.eq_ignore_ascii_case("all") {
        return Ok(None);
    }
    CreationKind::parse(raw).map(Some).ok_or_else(|| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_kind",
            &format!("Unknown creation kind '{raw}'"),
        )
    })
}

fn parse_optional_status(
    params: &HashMap<String, String>,
    default_status: Option<CreationReviewStatus>,
) -> Result<Option<CreationReviewStatus>, Response> {
    let Some(raw) = params.get("status").map(|s| s.trim()) else {
        return Ok(default_status);
    };
    if raw.is_empty() {
        return Ok(default_status);
    }
    if raw.eq_ignore_ascii_case("all") {
        return Ok(None);
    }
    CreationReviewStatus::parse(raw).map(Some).ok_or_else(|| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_status",
            &format!("Unknown review status '{raw}'"),
        )
    })
}

fn parse_sort_mode(params: &HashMap<String, String>) -> Result<CatalogSortMode, Response> {
    let Some(raw) = params.get("sort").map(|s| s.trim()) else {
        return Ok(CatalogSortMode::Featured);
    };
    if raw.is_empty() {
        return Ok(CatalogSortMode::Featured);
    }
    CatalogSortMode::parse(raw).ok_or_else(|| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_sort",
            &format!("Unknown catalog sort mode '{raw}'"),
        )
    })
}

fn parse_clamped_limit(
    params: &HashMap<String, String>,
    default_limit: usize,
    max_limit: usize,
) -> Result<usize, Response> {
    let Some(raw) = params.get("limit").map(|s| s.trim()) else {
        return Ok(default_limit);
    };
    if raw.is_empty() {
        return Ok(default_limit);
    }
    let parsed: i64 = raw.parse().map_err(|_| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_limit",
            "limit must be an integer",
        )
    })?;
    if parsed <= 0 {
        Ok(0)
    } else {
        Ok((parsed as usize).min(max_limit))
    }
}

fn parse_clamped_offset(params: &HashMap<String, String>) -> Result<usize, Response> {
    let Some(raw) = params.get("offset").map(|s| s.trim()) else {
        return Ok(0);
    };
    if raw.is_empty() {
        return Ok(0);
    }
    let parsed: i64 = raw.parse().map_err(|_| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_offset",
            "offset must be an integer",
        )
    })?;
    Ok(parsed.max(0) as usize)
}

fn emit_user_creation_changed(state: &AppState, dto: &UserCreationDto) {
    let event = UpdateEvent::UserCreationChanged {
        creation_id: dto.creation_id.clone(),
        owner_user_id: dto.owner_user_id,
        kind: dto.kind,
        review_status: dto.review_status,
        cloud_revision: dto.cloud_revision,
    };
    state.gateway_hub.broadcast_to_user(
        UserId(dto.owner_user_id as u64),
        WsServerFrame::Update(event),
    );
}

fn emit_public_catalog_changed(
    state: &AppState,
    creation_id: &str,
    kind: CreationKind,
    action: Option<&'static str>,
) {
    if let Some(act) = action {
        let event = UpdateEvent::PublicCatalogChanged {
            creation_id: creation_id.to_string(),
            kind,
            action: act.to_string(),
        };
        state
            .gateway_hub
            .broadcast_to_all(WsServerFrame::Update(event));
    }
}

// ============================================================================
// User Cloud Creations & Review FSM Handlers (/api/v1/creations/*)
// ============================================================================

pub async fn creations_create(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: CreateCreationRequest = match parse_required_json(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = Utc::now().timestamp_millis();
    match state.db.create_user_creation(auth.user_id, &req, now_ms) {
        Ok(dto) => {
            emit_user_creation_changed(&state, &dto);
            (StatusCode::CREATED, Json(dto)).into_response()
        }
        Err(err) => map_creation_err(err),
    }
}

pub async fn creations_list_my(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let kind = match parse_optional_kind(&params) {
        Ok(k) => k,
        Err(resp) => return resp,
    };
    let status = match parse_optional_status(&params, None) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let q = params.get("q").map(String::as_str);
    match state.db.list_my_creations(auth.user_id, kind, status, q) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(err) => map_telegram_err(err),
    }
}

pub async fn creations_sync(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let since_revision = match params.get("since_revision") {
        None => 0i64,
        Some(raw) if raw.trim().is_empty() => 0i64,
        Some(raw) => match raw.trim().parse::<i64>() {
            Ok(v) if v >= 0 => v,
            _ => {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_since_revision",
                    "since_revision must be a non-negative integer",
                );
            }
        },
    };
    match state.db.sync_my_creations(auth.user_id, since_revision) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(err) => map_telegram_err(err),
    }
}

pub async fn creations_review_queue(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    if !caller_is_admin(&state, auth.user_id) {
        return err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Admin reviewer privilege required",
        );
    }
    let status = match parse_optional_status(&params, Some(CreationReviewStatus::PendingReview)) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let kind = match parse_optional_kind(&params) {
        Ok(k) => k,
        Err(resp) => return resp,
    };
    match state.db.list_creation_review_queue(status, kind) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(err) => map_telegram_err(err),
    }
}

pub async fn creations_get(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
) -> Response {
    let is_admin = caller_is_admin(&state, auth.user_id);
    match state
        .db
        .get_creation_for_caller(&creation_id, auth.user_id, is_admin)
    {
        Ok(dto) => (StatusCode::OK, Json(dto)).into_response(),
        Err(err) => map_creation_err(err),
    }
}

pub async fn creations_update(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: UpdateCreationRequest = match parse_required_json(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .update_user_creation(&creation_id, auth.user_id, &req, now_ms)
    {
        Ok(dto) => {
            emit_user_creation_changed(&state, &dto);
            (StatusCode::OK, Json(dto)).into_response()
        }
        Err(err) => map_creation_err(err),
    }
}

pub async fn creations_delete(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
) -> Response {
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .soft_delete_creation(&creation_id, auth.user_id, now_ms)
    {
        Ok(res) => {
            emit_user_creation_changed(&state, &res.creation);
            emit_public_catalog_changed(
                &state,
                &res.creation.creation_id,
                res.creation.kind,
                res.catalog_ws_action,
            );
            (StatusCode::OK, Json(res.creation)).into_response()
        }
        Err(err) => map_creation_err(err),
    }
}

pub async fn creations_submit_review(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: SubmitReviewRequest = match parse_optional_json(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .submit_creation_for_review(&creation_id, auth.user_id, &req, now_ms)
    {
        Ok(creation) => {
            emit_user_creation_changed(&state, &creation);
            (
                StatusCode::OK,
                Json(SubmitReviewResponse {
                    creation,
                    validation_errors: Vec::new(),
                }),
            )
                .into_response()
        }
        Err(err) => map_creation_err(err),
    }
}

pub async fn creations_withdraw_review(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
    body: Bytes,
) -> Response {
    let _req: EmptyActionBody = match parse_optional_json(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .withdraw_creation_review(&creation_id, auth.user_id, now_ms)
    {
        Ok(dto) => {
            emit_user_creation_changed(&state, &dto);
            (StatusCode::OK, Json(dto)).into_response()
        }
        Err(err) => map_creation_err(err),
    }
}

pub async fn creations_unpublish(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
    body: Bytes,
) -> Response {
    let _req: EmptyActionBody = match parse_optional_json(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let is_admin = caller_is_admin(&state, auth.user_id);
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .unpublish_creation(&creation_id, auth.user_id, is_admin, now_ms)
    {
        Ok(res) => {
            emit_user_creation_changed(&state, &res.creation);
            emit_public_catalog_changed(
                &state,
                &res.creation.creation_id,
                res.creation.kind,
                res.catalog_ws_action,
            );
            (StatusCode::OK, Json(res.creation)).into_response()
        }
        Err(err) => map_creation_err(err),
    }
}

pub async fn creations_audit_list(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
) -> Response {
    let is_admin = caller_is_admin(&state, auth.user_id);
    match state
        .db
        .list_creation_audit(&creation_id, auth.user_id, is_admin)
    {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(err) => map_creation_err(err),
    }
}

pub async fn creations_review_decide(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
    body: Bytes,
) -> Response {
    if !caller_is_admin(&state, auth.user_id) {
        return err_resp(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Admin reviewer privilege required",
        );
    }
    let req: ReviewDecisionRequest = match parse_required_json(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .review_creation_decision(&creation_id, auth.user_id, &req, now_ms)
    {
        Ok(res) => {
            emit_user_creation_changed(&state, &res.creation);
            emit_public_catalog_changed(
                &state,
                &res.creation.creation_id,
                res.creation.kind,
                res.catalog_ws_action,
            );
            (StatusCode::OK, Json(res.creation)).into_response()
        }
        Err(err) => map_creation_err(err),
    }
}

// ============================================================================
// Public Catalog, Verticals, App Store Merge, States, Reviews & Leaderboards
// ============================================================================

fn run_catalog_query(
    state: &AppState,
    forced_kind: Option<CreationKind>,
    params: &HashMap<String, String>,
) -> Response {
    let kind = match forced_kind {
        Some(k) => Some(k),
        None => match parse_optional_kind(params) {
            Ok(k) => k,
            Err(resp) => return resp,
        },
    };
    let sort = match parse_sort_mode(params) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let limit = match parse_clamped_limit(params, 24, 100) {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let offset = match parse_clamped_offset(params) {
        Ok(o) => o,
        Err(resp) => return resp,
    };
    let q = params.get("q").map(String::as_str);
    let genre = params.get("genre").map(String::as_str);
    let svc = PublicCatalogSearchService::new(state.db.clone());
    match svc.search_catalog(kind, q, genre, sort, limit, offset) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(err) => map_telegram_err(err),
    }
}

pub async fn catalog_search(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    run_catalog_query(&state, None, &params)
}

pub async fn catalog_books(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    run_catalog_query(&state, Some(CreationKind::Book), &params)
}

pub async fn catalog_music(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    run_catalog_query(&state, Some(CreationKind::Music), &params)
}

pub async fn catalog_games(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    run_catalog_query(&state, Some(CreationKind::Game), &params)
}

pub async fn catalog_appstore(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let sort = match parse_sort_mode(&params) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let limit = match parse_clamped_limit(&params, 24, 100) {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let offset = match parse_clamped_offset(&params) {
        Ok(o) => o,
        Err(resp) => return resp,
    };
    let q = params.get("q").map(String::as_str);
    let genre = params.get("genre").map(String::as_str);

    let svc = PublicCatalogSearchService::new(state.db.clone());
    let apps = match svc.search_catalog(Some(CreationKind::App), q, genre, sort, limit, offset) {
        Ok(r) => r.items,
        Err(err) => return map_telegram_err(err),
    };
    let cross_vertical = match svc.list_cross_vertical(q, genre, sort, limit.clamp(0, 24)) {
        Ok(items) => items,
        Err(err) => return map_telegram_err(err),
    };
    let mini_apps_raw = if q.is_some_and(|s| s.contains('\0')) {
        Vec::new()
    } else {
        match state.db.search_published_mini_apps(q.unwrap_or(""), genre) {
            Ok(rows) => rows,
            Err(err) => return map_telegram_err(err),
        }
    };
    let mini_apps: Vec<AppStoreMiniAppEntry> = mini_apps_raw
        .into_iter()
        .filter(|rec| rec.category != ADHOC_WEBAPP_CATEGORY)
        .take(limit)
        .map(|rec| AppStoreMiniAppEntry {
            deep_link: format!("tg://resolve?domain={}&appname={}", rec.bot_username, rec.short_name),
            bot_user_id: rec.bot_user_id,
            bot_username: rec.bot_username,
            short_name: rec.short_name,
            title: rec.title,
            description: rec.description,
            web_url: rec.web_url,
            icon_url: None,
        })
        .collect();

    let mut seen_featured = HashSet::new();
    let mut featured: Vec<PublicCatalogItemDto> = Vec::new();
    for item in apps.iter().chain(cross_vertical.iter()) {
        if seen_featured.insert(item.creation_id.clone()) {
            featured.push(item.clone());
            if featured.len() >= 10 {
                break;
            }
        }
    }

    (
        StatusCode::OK,
        Json(AppStoreCatalogResponse {
            featured,
            apps,
            mini_apps,
            cross_vertical,
        }),
    )
        .into_response()
}

pub async fn catalog_recommend(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let kind = match parse_optional_kind(&params) {
        Ok(k) => k,
        Err(resp) => return resp,
    };
    let limit = match parse_clamped_limit(&params, 10, 50) {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let q = params.get("q").map(String::as_str);
    let genre = params.get("genre").map(String::as_str);
    let svc = PublicCatalogSearchService::new(state.db.clone());
    match svc.recommend(q, kind, genre, limit) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(err) => map_telegram_err(err),
    }
}

pub async fn catalog_my_states(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let kind = match parse_optional_kind(&params) {
        Ok(k) => k,
        Err(resp) => return resp,
    };
    match state.db.list_user_creation_states(auth.user_id, kind) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(err) => map_telegram_err(err),
    }
}

pub async fn catalog_upsert_state(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: UpsertCreationStateRequest = match parse_required_json(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .upsert_user_creation_state(&creation_id, auth.user_id, &req, now_ms)
    {
        Ok(dto) => (StatusCode::OK, Json(dto)).into_response(),
        Err(err) => map_creation_err(err),
    }
}

pub async fn catalog_upsert_review(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: CreateCreationReviewRequest = match parse_required_json(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .upsert_creation_review(&creation_id, auth.user_id, &req, now_ms)
    {
        Ok(dto) => (StatusCode::CREATED, Json(dto)).into_response(),
        Err(err) => map_creation_err(err),
    }
}

pub async fn catalog_list_reviews(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let limit = match parse_clamped_limit(&params, 50, 200) {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let offset = match parse_clamped_offset(&params) {
        Ok(o) => o,
        Err(resp) => return resp,
    };
    match state.db.list_creation_reviews(&creation_id, limit, offset) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(err) => map_creation_err(err),
    }
}

pub async fn catalog_submit_score(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: SubmitGameScoreRequest = match parse_required_json(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .submit_creation_game_score(&creation_id, auth.user_id, &req, now_ms)
    {
        Ok(dto) => (StatusCode::OK, Json(dto)).into_response(),
        Err(err) => map_creation_err(err),
    }
}

pub async fn catalog_leaderboard(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(creation_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let limit = match parse_clamped_limit(&params, 50, 200) {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    match state.db.get_creation_game_leaderboard(&creation_id, limit) {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(err) => map_creation_err(err),
    }
}
