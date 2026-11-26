use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use chrono::Utc;
use gpui_telegram_agent::{build_code_scene_generative_card, CodeSceneBotGenerator};
use gpui_telegram_code_scene::reco::embedding::{
    bilingual_concepts_for_text, effective_primary_tag, normalize_tags, tokenize_bilingual,
};
use gpui_telegram_code_scene::reco::reranker::{
    encode_cursor, parse_cursor, CursorParseResult, FeedCursorPayload, MAX_CURSOR_DELIVERED,
};
use gpui_telegram_code_scene::{
    compile_and_verify_with_budgets, find_official_bot, CodeSceneRecommender, RecommenderConfig,
    UserInterestProfile, VerifiedProgram, VmBudgets, OFFICIAL_FABUSHI_BOT_ID,
};
use gpui_telegram_core::types::{ChatId, MsgId, PeerId, UserId};
use gpui_telegram_proto::code_scene_contracts::{
    AiGenerateSceneRequest, AiGenerateSceneResponse, AiIterateSceneRequest, AiPublishSceneRequest,
    AssetKind, AssetManifest, CodeSceneAssetDto, CodeSceneCommentSort, CodeSceneFeedRequest,
    CodeSceneFeedResponse, CreateCodeSceneCommentRequest, CreateCodeSceneCommentResponse,
    ListCodeSceneAssetsQuery, ListCodeSceneCommentsQuery, PublishCodeSceneRequest,
    PublishCodeSceneResponse, RankedSceneItemDto, RemixSceneRequest, RemixSceneResponse,
    SceneBehaviorEventDto, SceneBehaviorEventKind, TelemetryIngestResponse, TelemetrySubmitPayload,
    ValidateCodeSceneRequest, ValidationDiagnosticDto, ValidationReportDto, ValidationSeverity,
    CODE_SCENE_FORMAT_CS1,
};
use gpui_telegram_proto::message::{MessageContent, MessageItem};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use parking_lot::{Mutex, MutexGuard, RwLock};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::db::code_scene_store::{
    CodeSceneBotDraftRecord, CreateCodeSceneCommentOutcome, UpsertCodeSceneOutcome,
};
use crate::db::ServerDatabase;
use crate::messaging::pipeline::MessagePipeline;
use crate::routes::{check_active_membership, err_resp};
use crate::search::hybrid_memory::HybridMemoryEngine;
use crate::search::schema::SearchDocument;
use crate::search::tantivy_indexer::TantivyIndexer;
use crate::server::{extract_message_content_text, AppState, AuthBearer};
use crate::ws::GatewayHub;

const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_TELEMETRY_BATCH: usize = 200;
const MAX_COMMENT_CHARS: usize = 1000;
const MAX_PROMPT_CHARS: usize = 4000;
const MAX_TITLE_CHARS: usize = 160;

/// Live server-side Code Scene recommender state hydrated from SQLite WAL on boot.
///
/// Strict Lock Order:
/// 1. NEVER hold `social_mutation_lock` or `recommender` lock across `.await`.
/// 2. NEVER invoke `state.db.*` while holding `recommender` lock.
/// 3. Social/publish mutations acquire `social_mutation_lock` FIRST, then execute
///    `state.db.*` (releasing SQLite lock), then `recommender.write()` (releasing it),
///    then optional profile persistence (`state.db.*`), then release `social_mutation_lock`.
pub struct CodeSceneRuntimeState {
    recommender: RwLock<CodeSceneRecommender>,
    social_mutation_lock: Mutex<()>,
    validation_semaphore: Arc<Semaphore>,
}

/// Returns strict fail-closed VM dry-run budgets for server validation.
/// Preserves all 19 hard limits (`max_ops_per_frame = 100_000`, source/AST/bytecode/heap/draw/pixel
/// bounds) and keeps `max_frame_wall_ms = 10` in release builds; in unoptimized `debug_assertions`
/// (`opt-level = 0`) builds only, widens `max_frame_wall_ms` to 50ms so benign shader loops do not
/// hit false-positive OS preemption timeouts while hostile scripts still trip `max_ops_per_frame`.
pub(crate) fn server_validation_budgets() -> VmBudgets {
    #[cfg(debug_assertions)]
    {
        let mut budgets = VmBudgets::default();
        budgets.max_frame_wall_ms = 50;
        budgets
    }
    #[cfg(not(debug_assertions))]
    {
        VmBudgets::default()
    }
}

/// Executes compile + 5-frame dry-run verification in a SINGLE pass so validation and publish
/// can never diverge or produce a contradictory `422 (ok: true)` response.
pub(crate) fn validate_cs1_single_pass(
    source_code: &str,
    manifest: &AssetManifest,
) -> (ValidationReportDto, Option<VerifiedProgram>) {
    match compile_and_verify_with_budgets(source_code, manifest, server_validation_budgets()) {
        Ok(verified) => {
            let diagnostics: Vec<ValidationDiagnosticDto> = verified
                .warnings
                .iter()
                .map(|w| w.to_dto(ValidationSeverity::Warning))
                .collect();
            let report = ValidationReportDto {
                ok: true,
                diagnostics,
                frames_executed: verified.draw_cmds_per_frame.len() as u32,
                draw_cmds_per_frame: verified.draw_cmds_per_frame.clone(),
                has_pixel_output: verified.has_pixel_output,
                peak_ops_used: verified.peak_ops_used,
                cover_hash: Some(verified.cover_hash.clone()),
                feature_tokens: verified.feature_tokens.clone(),
            };
            (report, Some(verified))
        }
        Err(err) => {
            let report = ValidationReportDto {
                ok: false,
                diagnostics: vec![err.to_dto(ValidationSeverity::Error)],
                frames_executed: 0,
                draw_cmds_per_frame: Vec::new(),
                has_pixel_output: false,
                peak_ops_used: 0,
                cover_hash: None,
                feature_tokens: Vec::new(),
            };
            (report, None)
        }
    }
}

impl CodeSceneRuntimeState {
    /// Hydrates catalog FIRST, then merges persisted `UserInterestProfile`s + relational social edges
    /// so relational tables (`code_scene_likes`, `code_scene_bookmarks`, `code_scene_follows`) are
    /// strictly authoritative and blocklists + Swing CF reverse indexes match 100% after restarts.
    pub fn hydrate_from_db(db: &ServerDatabase) -> Self {
        let mut cfg = RecommenderConfig::default();
        // Disable hard Phase-1 seen cooldown wall on live server feed so small catalogs
        // (e.g. 6 games in 18 seeds) can still honor strong late-session modality affinity
        // while pagination cursor `exclude_scene_ids` & fast-skip/fatigue penalties govern freshness.
        cfg.seen_cooldown_ms = 0;
        let mut reco = CodeSceneRecommender::with_config(cfg);

        if let Ok(assets) = db.list_all_published_code_scenes() {
            for asset in assets {
                reco.register_or_update_asset(asset, None);
            }
        }

        let mut profiles: BTreeMap<i64, UserInterestProfile> = BTreeMap::new();
        if let Ok(loaded_profiles) = db.load_all_code_scene_user_profiles() {
            for mut p in loaded_profiles {
                // Relational tables are the single source of truth for social edges.
                p.liked_scene_ids.clear();
                p.bookmarked_scene_ids.clear();
                p.followed_creators.clear();
                profiles.insert(p.user_id, p);
            }
        }

        if let Ok((likes, bookmarks, follows)) = db.load_all_code_scene_social_edges() {
            for (uid, sid) in likes {
                profiles
                    .entry(uid)
                    .or_insert_with(|| UserInterestProfile::new(uid))
                    .liked_scene_ids
                    .insert(sid);
            }
            for (uid, sid) in bookmarks {
                profiles
                    .entry(uid)
                    .or_insert_with(|| UserInterestProfile::new(uid))
                    .bookmarked_scene_ids
                    .insert(sid);
            }
            for (follower_uid, creator_uid) in follows {
                let entry = profiles
                    .entry(follower_uid)
                    .or_insert_with(|| UserInterestProfile::new(follower_uid));
                if !entry.hidden_author_ids.contains(&creator_uid) {
                    entry.followed_creators.insert(creator_uid);
                }
            }
        }

        for (_, mut profile) in profiles {
            profile.sanitize();
            reco.import_user_profile(profile);
        }

        Self {
            recommender: RwLock::new(reco),
            social_mutation_lock: Mutex::new(()),
            validation_semaphore: Arc::new(Semaphore::new(1)),
        }
    }

    pub fn recommender(&self) -> &RwLock<CodeSceneRecommender> {
        &self.recommender
    }

    pub fn lock_social_mutation(&self) -> MutexGuard<'_, ()> {
        self.social_mutation_lock.lock()
    }

    /// Runs single-pass VM dry-run validation offloaded to `spawn_blocking` under bounded
    /// concurrency (`Semaphore(1)`) so Tokio workers never stall and concurrent requests get
    /// uncontended CPU time.
    pub async fn run_validation_offloaded(
        &self,
        source_code: String,
        manifest: AssetManifest,
    ) -> (ValidationReportDto, Option<VerifiedProgram>) {
        let _permit = self.validation_semaphore.acquire().await.ok();
        match tokio::task::spawn_blocking(move || validate_cs1_single_pass(&source_code, &manifest))
            .await
        {
            Ok(res) => res,
            Err(join_err) => (
                ValidationReportDto {
                    ok: false,
                    diagnostics: vec![ValidationDiagnosticDto {
                        severity: ValidationSeverity::Error,
                        code: "E_VALIDATION_WORKER".to_string(),
                        message: format!("Validation worker failed: {join_err}"),
                        line: None,
                        column: None,
                        stage: Some("dry_run".to_string()),
                    }],
                    ..Default::default()
                },
                None,
            ),
        }
    }
}

pub(crate) fn resolve_author_username(db: &ServerDatabase, user_id: i64) -> String {
    if user_id > 0 {
        if let Ok(Some(u)) = db.get_user(UserId(user_id as u64)) {
            if let Some(un) = u.username.filter(|s| !s.trim().is_empty()) {
                return un;
            }
        }
    }
    format!("user_{user_id}")
}

pub(crate) fn resolve_creator_bot(
    db: &ServerDatabase,
    bot_id: Option<i64>,
    bot_username: Option<&str>,
    default_to_fabushi: bool,
) -> (Option<i64>, Option<String>) {
    if let Some(bid) = bot_id.filter(|&id| id > 0) {
        let uname = bot_username
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_start_matches('@').to_string())
            .or_else(|| {
                db.get_user(UserId(bid as u64))
                    .ok()
                    .flatten()
                    .and_then(|u| u.username)
                    .filter(|s| !s.trim().is_empty())
            })
            .or_else(|| find_official_bot(bid).map(|b| b.username.to_string()));
        return (Some(bid), uname);
    }

    if let Some(raw_un) = bot_username.map(str::trim).filter(|s| !s.is_empty()) {
        let clean = raw_un.trim_start_matches('@');
        if let Ok(Some(u)) = db.get_user_by_username(clean) {
            return (
                Some(u.user_id.0 as i64),
                u.username.or_else(|| Some(clean.to_string())),
            );
        }
        return (None, Some(clean.to_string()));
    }

    if default_to_fabushi {
        let uname = find_official_bot(OFFICIAL_FABUSHI_BOT_ID)
            .map(|b| b.username.to_string())
            .unwrap_or_else(|| "fabushi_ai_bot".to_string());
        return (Some(OFFICIAL_FABUSHI_BOT_ID), Some(uname));
    }

    (None, None)
}

#[derive(Debug)]
pub(crate) enum PublishSceneError {
    ValidationFailed(ValidationReportDto),
    InvalidField(String),
    Forbidden(String),
    ParentNotFound(String),
    Database(String),
}

impl PublishSceneError {
    pub(crate) fn into_response(self) -> Response {
        match self {
            Self::ValidationFailed(report) => validation_failed_resp(report),
            Self::InvalidField(msg) => err_resp(StatusCode::UNPROCESSABLE_ENTITY, "invalid_request", &msg),
            Self::Forbidden(msg) => err_resp(StatusCode::FORBIDDEN, "forbidden", &msg),
            Self::ParentNotFound(pid) => err_resp(
                StatusCode::NOT_FOUND,
                "parent_scene_not_found",
                &format!("Remix parent scene '{pid}' was not found"),
            ),
            Self::Database(_) => err_resp(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Failed to persist Code Scene asset",
            ),
        }
    }

    pub(crate) fn into_message(self) -> String {
        match self {
            Self::ValidationFailed(r) => r
                .diagnostics
                .first()
                .map(|d| format!("[{}] {}", d.code, d.message))
                .unwrap_or_else(|| "Code Scene validation failed".to_string()),
            Self::InvalidField(m) | Self::Forbidden(m) | Self::Database(m) => m,
            Self::ParentNotFound(pid) => format!("Remix parent scene '{pid}' not found"),
        }
    }
}

pub(crate) fn validation_failed_resp(report: ValidationReportDto) -> Response {
    let first_msg = report
        .diagnostics
        .first()
        .map(|d| format!("[{}] {}", d.code, d.message))
        .unwrap_or_else(|| "Code Scene script validation failed".to_string());
    let body = json!({
        "error": "validation_failed",
        "message": first_msg,
        "ok": report.ok,
        "diagnostics": report.diagnostics,
        "frames_executed": report.frames_executed,
        "draw_cmds_per_frame": report.draw_cmds_per_frame,
        "has_pixel_output": report.has_pixel_output,
        "peak_ops_used": report.peak_ops_used,
        "cover_hash": report.cover_hash,
        "feature_tokens": report.feature_tokens,
        "validation": report,
    });
    (StatusCode::UNPROCESSABLE_ENTITY, Json(body)).into_response()
}

fn validate_custom_scene_id(raw: &str) -> Result<String, PublishSceneError> {
    let id = raw.trim();
    if id.is_empty()
        || id.len() > 128
        || id.eq_ignore_ascii_case("validate")
        || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(PublishSceneError::InvalidField(
            "scene_id must be 1..=128 ASCII [a-zA-Z0-9_-] chars and cannot be 'validate'".into(),
        ));
    }
    Ok(id.to_string())
}

/// Shared atomic publish pipeline reused by REST `/assets`, `/ai/publish`, `/ai/remix`,
/// `ServerPlatformImBridge`, and direct bot turns.
/// Runs single-pass VM dry-run validation offloaded to `spawn_blocking` BEFORE acquiring any lock,
/// then serializes SQLite transaction + recommender registration via `social_mutation_lock`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn publish_validated_scene_atomic(
    db: &ServerDatabase,
    runtime: &CodeSceneRuntimeState,
    author_user_id: i64,
    req: &PublishCodeSceneRequest,
    mark_draft_id: Option<&str>,
    default_bot_to_fabushi: bool,
) -> Result<(CodeSceneAssetDto, ValidationReportDto, bool), PublishSceneError> {
    let title = req.title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
        return Err(PublishSceneError::InvalidField(format!(
            "title must be 1..={MAX_TITLE_CHARS} characters"
        )));
    }
    let code_format = req.code_format.trim();
    if code_format != CODE_SCENE_FORMAT_CS1 {
        return Err(PublishSceneError::InvalidField(format!(
            "Unsupported code_format '{code_format}'; expected '{CODE_SCENE_FORMAT_CS1}'"
        )));
    }

    let mut manifest = req.manifest.clone();
    if manifest.kind != req.kind {
        return Err(PublishSceneError::InvalidField(format!(
            "Manifest kind '{}' does not match asset kind '{}'",
            manifest.kind.as_str(),
            req.kind.as_str()
        )));
    }
    if req.kind == AssetKind::Video {
        match manifest.duration_sec {
            None => {
                return Err(PublishSceneError::InvalidField(
                    "manifest.duration_sec is required for video assets".into(),
                ));
            }
            Some(d) if !d.is_finite() || d <= 0.0 => {
                return Err(PublishSceneError::InvalidField(format!(
                    "manifest.duration_sec {d} must be finite and positive for video assets"
                )));
            }
            Some(_) => {}
        }
    } else if let Some(d) = manifest.duration_sec {
        if !d.is_finite() || d <= 0.0 {
            return Err(PublishSceneError::InvalidField(format!(
                "manifest.duration_sec {d} must be finite and positive"
            )));
        }
    }
    manifest.clamp_in_place();

    // Single-pass offloaded validation BEFORE acquiring any DB/RAM locks.
    let (report, verified_opt) = runtime
        .run_validation_offloaded(req.source_code.clone(), manifest.clone())
        .await;
    let Some(verified) = verified_opt else {
        return Err(PublishSceneError::ValidationFailed(report));
    };

    let scene_id = match req.scene_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(custom) => validate_custom_scene_id(custom)?,
        None => format!("scene_{}", Uuid::new_v4().simple()),
    };

    let author_username = resolve_author_username(db, author_user_id);
    let (creator_bot_id, creator_bot_username) = resolve_creator_bot(
        db,
        req.creator_bot_id,
        req.creator_bot_username.as_deref(),
        default_bot_to_fabushi,
    );
    let remix_parent_id = req
        .remix_parent_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let now_ms = Utc::now().timestamp_millis();
    let mut candidate = CodeSceneAssetDto {
        scene_id,
        title: title.to_string(),
        description: req.description.trim().to_string(),
        kind: req.kind,
        tags: req.tags.clone(),
        author_user_id,
        author_username,
        creator_bot_id,
        creator_bot_username,
        remix_parent_id: remix_parent_id.clone(),
        source_code: req.source_code.clone(),
        code_format: code_format.to_string(),
        manifest,
        controls_hint: req.controls_hint.trim().to_string(),
        plays_count: 0,
        likes_count: 0,
        bookmarks_count: 0,
        comments_count: 0,
        shares_count: 0,
        remixes_count: 0,
        viewer_liked: false,
        viewer_bookmarked: false,
        viewer_following_author: false,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    };
    candidate.tags = candidate.normalized_tags();

    let cover_hash = report.cover_hash.as_deref().unwrap_or("");
    let (saved_asset, created) = {
        let _social_guard = runtime.lock_social_mutation();
        let outcome = db
            .upsert_code_scene(
                &candidate,
                cover_hash,
                &report.feature_tokens,
                author_user_id,
                mark_draft_id,
            )
            .map_err(|e| PublishSceneError::Database(e.to_string()))?;

        let (saved_asset, created, parent_remixes_count) = match outcome {
            UpsertCodeSceneOutcome::Created {
                asset,
                parent_remixes_count,
                ..
            } => (asset, true, parent_remixes_count),
            UpsertCodeSceneOutcome::Updated { asset, .. } => (asset, false, None),
            UpsertCodeSceneOutcome::ForbiddenOwner => {
                return Err(PublishSceneError::Forbidden(
                    "Cannot modify a Code Scene owned by another user".into(),
                ));
            }
            UpsertCodeSceneOutcome::ParentNotFound(pid) => {
                return Err(PublishSceneError::ParentNotFound(pid));
            }
        };

        // Update live recommender strictly AFTER SQLite transaction has committed.
        {
            let mut reco = runtime.recommender().write();
            reco.register_verified_asset(saved_asset.clone(), &verified, Some(author_user_id));
            if let (Some(parent_id), Some(new_cnt)) =
                (remix_parent_id.as_deref(), parent_remixes_count)
            {
                reco.update_social_counters(parent_id, None, None, None, None, Some(new_cnt), None);
            }
        }
        (saved_asset, created)
    };

    Ok((saved_asset, report, created))
}

/// Records `SceneBehaviorEventKind::Remix` telemetry on the parent scene under
/// `social_mutation_lock`, keeping RAM `remixes_count` strictly reconciled with SQLite's
/// authoritative counter without double-incrementing or stale overwrites.
pub(crate) fn record_remix_parent_telemetry(
    db: &ServerDatabase,
    runtime: &CodeSceneRuntimeState,
    user_id: i64,
    parent_scene_id: &str,
    parent_author_user_id: i64,
) {
    let _social_guard = runtime.lock_social_mutation();
    let remix_ev = SceneBehaviorEventDto {
        event_id: format!("cs_ev_{}", Uuid::new_v4().simple()),
        user_id,
        session_id: format!("remix_{user_id}"),
        scene_id: parent_scene_id.to_string(),
        kind: SceneBehaviorEventKind::Remix,
        author_user_id: Some(parent_author_user_id),
        timestamp_ms: Utc::now().timestamp_millis(),
        ..Default::default()
    };
    let fresh_parent_remixes = db
        .get_code_scene(parent_scene_id)
        .ok()
        .flatten()
        .map(|p| p.remixes_count);
    let (reward, profile) = {
        let mut reco = runtime.recommender().write();
        let out = reco.record_behavior(&remix_ev);
        if let Some(remixes_cnt) = fresh_parent_remixes {
            reco.update_social_counters(
                parent_scene_id,
                None,
                None,
                None,
                None,
                Some(remixes_cnt),
                None,
            );
        }
        (out.reward, reco.export_user_profile(user_id))
    };
    let _ = db.persist_code_scene_telemetry_batch(
        &[(remix_ev, reward)],
        profile.as_ref(),
        &[],
        &[],
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn post_code_scene_card_to_chat_raw(
    pipeline: &MessagePipeline,
    gateway_hub: Option<&GatewayHub>,
    search_indexer: Option<&TantivyIndexer>,
    hybrid_memory: Option<&HybridMemoryEngine>,
    chat_id: ChatId,
    sender_uid: UserId,
    is_bot_sender: bool,
    asset: &CodeSceneAssetDto,
) -> Result<i64, String> {
    let card = build_code_scene_generative_card(asset);
    let content = MessageContent::GenerativeCard(card);
    let mut msg = MessageItem::new(
        MsgId(0),
        chat_id,
        sender_uid,
        PeerId::Chat(chat_id),
        content.clone(),
    );
    msg.flags.is_out = !is_bot_sender;

    let pts_update = pipeline
        .process_incoming_message_strict(msg)
        .map_err(|e| e.to_string())?;
    let now_ms = Utc::now().timestamp_millis();
    let (msg_id, date_ms, indexed_text) = match &pts_update.event {
        UpdateEvent::NewMessage(saved) => (
            saved.id.0,
            saved.date.timestamp_millis(),
            extract_message_content_text(&saved.content),
        ),
        _ => (0, now_ms, extract_message_content_text(&content)),
    };

    if let Some(idx) = search_indexer {
        idx.index_message_nonblocking(SearchDocument::new(
            msg_id.to_string(),
            chat_id.0.to_string(),
            sender_uid.0.to_string(),
            date_ms,
            indexed_text.clone(),
            None,
        ));
        idx.commit_nonblocking();
    }
    if let Some(hm) = hybrid_memory {
        let doc_id = format!("{}:{}", chat_id.0, msg_id);
        hm.index_message_or_file(&doc_id, chat_id.0, "message", &indexed_text, vec![]);
    }
    if let Some(hub) = gateway_hub {
        hub.broadcast_to_chat_and_members(chat_id, None, WsServerFrame::PtsPush(pts_update));
    }
    Ok(msg_id)
}

pub(crate) fn post_code_scene_card_to_chat(
    state: &AppState,
    chat_id: ChatId,
    caller_uid: UserId,
    bot_id: Option<i64>,
    asset: &CodeSceneAssetDto,
) -> Result<i64, Response> {
    let _ = check_active_membership(state, chat_id, caller_uid)?;
    let (sender_uid, is_bot_sender) = match bot_id.filter(|&id| id > 0) {
        Some(bid) if state.db.is_user_in_chat(chat_id, UserId(bid as u64)).unwrap_or(false) => {
            (UserId(bid as u64), true)
        }
        _ => (caller_uid, false),
    };
    post_code_scene_card_to_chat_raw(
        &state.message_pipeline,
        Some(&state.gateway_hub),
        Some(&state.search_indexer),
        Some(&state.hybrid_memory),
        chat_id,
        sender_uid,
        is_bot_sender,
        asset,
    )
    .map_err(|e| err_resp(StatusCode::INTERNAL_SERVER_ERROR, "chat_post_failed", &e))
}

fn parse_json_dto<T: DeserializeOwned>(
    bytes: &Bytes,
    allow_empty_object: bool,
) -> Result<T, Response> {
    if bytes.len() > MAX_BODY_BYTES {
        return Err(err_resp(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "Request body exceeds 256 KiB limit",
        ));
    }
    if bytes.is_empty() || bytes.iter().all(|b| b.is_ascii_whitespace()) {
        if allow_empty_object {
            return serde_json::from_str::<T>("{}").map_err(|e| {
                err_resp(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_request",
                    &format!("Invalid empty request: {e}"),
                )
            });
        }
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "empty_body",
            "Request body cannot be empty",
        ));
    }
    let val: Value = serde_json::from_slice(bytes).map_err(|e| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            &format!("Malformed JSON: {e}"),
        )
    })?;
    if !val.is_object() {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_payload",
            "Expected top-level JSON object",
        ));
    }
    serde_json::from_value::<T>(val).map_err(|e| {
        err_resp(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            &format!("Invalid request fields: {e}"),
        )
    })
}

/// Canonical bilingual query matcher aligned with `gpui_telegram_code_scene::reco::embedding`
/// (exact kind/substring + bilingual CJK/Latin tokenization + bilingual concept bridge, rejecting
/// spurious 64-dim hash-embedding collisions on nonsense queries).
fn asset_matches_feed_query(asset: &CodeSceneAssetDto, raw_query: &str) -> bool {
    let trimmed = raw_query.trim();
    if trimmed.is_empty() {
        return true;
    }
    let q = trimmed.to_lowercase();
    if AssetKind::parse(&q) == Some(asset.kind) {
        return true;
    }

    let norm_tags = normalize_tags(&asset.tags);
    if asset.title.to_lowercase().contains(&q)
        || asset.description.to_lowercase().contains(&q)
        || asset.scene_id.to_lowercase().contains(&q)
        || asset.author_username.to_lowercase().contains(&q)
        || asset.controls_hint.to_lowercase().contains(&q)
        || asset
            .creator_bot_username
            .as_deref()
            .is_some_and(|b| b.to_lowercase().contains(&q))
        || norm_tags.iter().any(|t| t == &q || t.contains(&q))
    {
        return true;
    }

    // Bilingual concept intersection (e.g. "赛博弹球" -> {"cyberpunk", "pong"}).
    let q_concepts: HashSet<String> = bilingual_concepts_for_text(trimmed).into_iter().collect();
    if !q_concepts.is_empty() {
        let mut asset_concepts: HashSet<String> = bilingual_concepts_for_text(&asset.title)
            .into_iter()
            .chain(bilingual_concepts_for_text(&asset.description))
            .collect();
        for tag in &norm_tags {
            for c in bilingual_concepts_for_text(tag) {
                asset_concepts.insert(c);
            }
        }
        asset_concepts.insert(asset.kind.as_str().to_string());
        if q_concepts.intersection(&asset_concepts).next().is_some() {
            return true;
        }
    }

    // Bilingual token intersection (ASCII words & CJK bigrams of length >= 2 to prevent
    // single-character CJK stopword collisions while supporting CJK compounds).
    let q_tokens: HashSet<String> = tokenize_bilingual(trimmed)
        .into_iter()
        .filter(|t| t.chars().count() >= 2)
        .collect();
    if !q_tokens.is_empty() {
        let asset_tokens: HashSet<String> = tokenize_bilingual(&asset.title)
            .into_iter()
            .chain(tokenize_bilingual(&asset.description))
            .chain(tokenize_bilingual(&asset.controls_hint))
            .chain(norm_tags.into_iter())
            .filter(|t| t.chars().count() >= 2)
            .collect();
        if q_tokens.intersection(&asset_tokens).next().is_some() {
            return true;
        }
    }

    false
}

/// Paginates query-filtered feed results losslessly across the entire eligible catalog using
/// canonical bilingual matching and `gpui_telegram_code_scene::reco::reranker` cursors.
fn recommend_query_feed_lossless(
    reco: &CodeSceneRecommender,
    req: &CodeSceneFeedRequest,
    query: &str,
    requested_limit: usize,
    now_ms: i64,
) -> CodeSceneFeedResponse {
    let total_catalog = reco.catalog_len();
    if total_catalog == 0 {
        return CodeSceneFeedResponse {
            items: Vec::new(),
            next_cursor: None,
            total_catalog: 0,
        };
    }

    let (numeric_skip, mut delivered_vec, delivered_set, prior_offset, mut tail_authors, mut tail_kinds, mut tail_tags) =
        match parse_cursor(req.cursor.as_deref()) {
            CursorParseResult::Invalid => {
                return CodeSceneFeedResponse {
                    items: Vec::new(),
                    next_cursor: None,
                    total_catalog,
                };
            }
            CursorParseResult::Fresh => (
                None,
                Vec::new(),
                HashSet::new(),
                0usize,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            CursorParseResult::NumericOffset(off) => (
                Some(off),
                Vec::new(),
                HashSet::new(),
                off,
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            CursorParseResult::Valid(payload) => {
                let set: HashSet<String> = payload.delivered.iter().cloned().collect();
                (
                    None,
                    payload.delivered,
                    set,
                    payload.offset,
                    payload.tail_authors,
                    payload.tail_kinds,
                    payload.tail_tags,
                )
            }
        };

    // Collect the full ranked candidate stream for this query across the eligible catalog.
    let max_pages = (total_catalog / 50).saturating_add(2);
    let mut probe_req = req.clone();
    probe_req.limit = 50;
    probe_req.cursor = None;

    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut all_matching: Vec<RankedSceneItemDto> = Vec::new();
    for _ in 0..max_pages {
        let page = reco.recommend_feed_at(&probe_req, now_ms);
        if page.items.is_empty() {
            break;
        }
        for item in page.items {
            if seen_ids.insert(item.asset.scene_id.clone())
                && asset_matches_feed_query(&item.asset, query)
            {
                all_matching.push(item);
            }
        }
        match page.next_cursor {
            Some(c) => probe_req.cursor = Some(c),
            None => break,
        }
    }

    let (page_items, has_more, next_offset) = if let Some(off) = numeric_skip {
        if off >= all_matching.len() {
            return CodeSceneFeedResponse {
                items: Vec::new(),
                next_cursor: None,
                total_catalog,
            };
        }
        for item in all_matching.iter().take(off) {
            if delivered_vec.len() < MAX_CURSOR_DELIVERED {
                delivered_vec.push(item.asset.scene_id.clone());
            }
        }
        let remaining_len = all_matching.len() - off;
        let items: Vec<RankedSceneItemDto> = all_matching
            .into_iter()
            .skip(off)
            .take(requested_limit)
            .collect();
        let more = remaining_len > items.len();
        let next_off = off.saturating_add(items.len());
        (items, more, next_off)
    } else {
        let remaining: Vec<RankedSceneItemDto> = all_matching
            .into_iter()
            .filter(|item| !delivered_set.contains(&item.asset.scene_id))
            .collect();
        let more = remaining.len() > requested_limit;
        let items: Vec<RankedSceneItemDto> = remaining.into_iter().take(requested_limit).collect();
        let next_off = prior_offset.saturating_add(items.len());
        (items, more, next_off)
    };

    for item in &page_items {
        if delivered_vec.len() < MAX_CURSOR_DELIVERED {
            delivered_vec.push(item.asset.scene_id.clone());
        }
        tail_authors.push(item.asset.author_user_id);
        if tail_authors.len() > 3 {
            tail_authors.remove(0);
        }
        tail_kinds.push(item.asset.kind);
        if tail_kinds.len() > 2 {
            tail_kinds.remove(0);
        }
        let eff_tag = effective_primary_tag(&normalize_tags(&item.asset.tags));
        tail_tags.push(eff_tag);
        if tail_tags.len() > 2 {
            tail_tags.remove(0);
        }
    }

    let next_cursor =
        if has_more && !page_items.is_empty() && delivered_vec.len() < MAX_CURSOR_DELIVERED {
            encode_cursor(&FeedCursorPayload {
                v: 1,
                offset: next_offset,
                delivered: delivered_vec,
                tail_authors,
                tail_kinds,
                tail_tags,
            })
        } else {
            None
        };

    CodeSceneFeedResponse {
        items: page_items,
        next_cursor,
        total_catalog,
    }
}

/// `POST /api/v1/code-scene/feed`
pub async fn code_scene_feed(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    body: Bytes,
) -> Response {
    let mut req: CodeSceneFeedRequest = match parse_json_dto(&body, true) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    req.user_id = auth.user_id.0 as i64;
    let requested_limit = req.clamped_limit();
    req.limit = requested_limit;
    if req.session_id.trim().is_empty() {
        req.session_id = format!("sess_{}", auth.user_id.0);
    }

    let now_ms = Utc::now().timestamp_millis();
    let query_opt = req
        .query
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let resp = {
        let reco = state.code_scene.recommender().read();
        let mut resp = if let Some(ref q) = query_opt {
            recommend_query_feed_lossless(&reco, &req, q, requested_limit, now_ms)
        } else {
            reco.recommend_feed_at(&req, now_ms)
        };

        // Report unified, page-invariant Stage-2 + Stage-3 `final_score` across all
        // non-fast-track items (Page 1, Page 2+, cold-start slots, and standard MMR slots).
        for item in &mut resp.items {
            let is_fast_track = item
                .explanation
                .recall_channels
                .iter()
                .any(|c| c == "creator_just_published");
            if !is_fast_track {
                if let Some(explained) =
                    reco.explain_asset_for_user(req.user_id, &item.asset.scene_id, now_ms)
                {
                    let mmr_penalty = item.explanation.breakdown.mmr_penalty;
                    let mut b = explained.explanation.breakdown;
                    b.mmr_penalty = mmr_penalty;
                    b.final_score = b.final_score - mmr_penalty;
                    item.explanation.breakdown = b.sanitized();
                }
            }
        }
        resp
    };
    (StatusCode::OK, Json(resp)).into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct AssetsListParams {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub author_user_id: Option<i64>,
    #[serde(default)]
    pub creator_bot_id: Option<i64>,
    #[serde(default)]
    pub bot_id: Option<i64>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

/// `GET /api/v1/code-scene/assets`
pub async fn code_scene_assets_list(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    Query(params): Query<AssetsListParams>,
) -> Response {
    let parsed_kind = match params.kind.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => match AssetKind::parse(raw) {
            Some(k) => Some(k),
            None => {
                return err_resp(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_kind",
                    &format!("Unknown asset kind '{raw}'; expected video, animation, or game"),
                );
            }
        },
        None => None,
    };
    let q = params
        .q
        .filter(|s| !s.trim().is_empty())
        .or_else(|| params.query.filter(|s| !s.trim().is_empty()));
    let creator_bot_id = params.creator_bot_id.or(params.bot_id);
    let query = ListCodeSceneAssetsQuery {
        q,
        kind: parsed_kind,
        tag: params.tag.filter(|s| !s.trim().is_empty()),
        author_user_id: params.author_user_id,
        creator_bot_id,
        limit: params.limit.map(|l| l.clamp(1, 200)),
        offset: params.offset,
    };

    let auth_uid = auth.user_id.0 as i64;
    let resp = {
        let reco = state.code_scene.recommender().read();
        let mut list = reco.list_assets(&query);
        if let Some(profile) = reco.export_user_profile(auth_uid) {
            for asset in &mut list.assets {
                asset.viewer_liked = profile.liked_scene_ids.contains(&asset.scene_id);
                asset.viewer_bookmarked = profile.bookmarked_scene_ids.contains(&asset.scene_id);
                asset.viewer_following_author =
                    profile.followed_creators.contains(&asset.author_user_id);
            }
        }
        list
    };
    (StatusCode::OK, Json(resp)).into_response()
}

/// `GET /api/v1/code-scene/assets/{id}`
pub async fn code_scene_asset_get(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    Path(scene_id): Path<String>,
) -> Response {
    let auth_uid = auth.user_id.0 as i64;
    let found = {
        let reco = state.code_scene.recommender().read();
        reco.get_asset_for_user(scene_id.trim(), auth_uid)
    };
    match found {
        Some(asset) => (StatusCode::OK, Json(asset)).into_response(),
        None => err_resp(StatusCode::NOT_FOUND, "scene_not_found", "Code Scene not found"),
    }
}

fn validation_manifest_error_ok(code: &str, message: &str) -> Response {
    (
        StatusCode::OK,
        Json(ValidationReportDto {
            ok: false,
            diagnostics: vec![ValidationDiagnosticDto {
                severity: ValidationSeverity::Error,
                code: code.to_string(),
                message: message.to_string(),
                line: None,
                column: None,
                stage: Some("manifest".to_string()),
            }],
            ..Default::default()
        }),
    )
        .into_response()
}

/// `POST /api/v1/code-scene/assets/validate`
pub async fn code_scene_assets_validate(
    State(state): State<Arc<AppState>>,
    _auth: AuthBearer,
    body: Bytes,
) -> Response {
    let raw_val: Value = match parse_json_dto(&body, false) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Some(cf_val) = raw_val.get("code_format") {
        let cf_ok = cf_val
            .as_str()
            .map(str::trim)
            .is_some_and(|s| s == CODE_SCENE_FORMAT_CS1);
        if !cf_ok {
            let got = cf_val.as_str().unwrap_or("<non-string>");
            return validation_manifest_error_ok(
                "E_UNSUPPORTED_CODE_FORMAT",
                &format!("Unsupported code_format '{got}'; expected '{CODE_SCENE_FORMAT_CS1}'"),
            );
        }
    }
    let mut req: ValidateCodeSceneRequest = match serde_json::from_value(raw_val) {
        Ok(r) => r,
        Err(e) => {
            return err_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request",
                &format!("Invalid request fields: {e}"),
            );
        }
    };
    if req.manifest.kind == AssetKind::Video {
        match req.manifest.duration_sec {
            None => {
                return validation_manifest_error_ok(
                    "E_INVALID_MANIFEST",
                    "manifest.duration_sec is required for video assets",
                );
            }
            Some(d) if !d.is_finite() || d <= 0.0 => {
                return validation_manifest_error_ok(
                    "E_INVALID_MANIFEST",
                    &format!(
                        "manifest.duration_sec {d} must be finite and positive for video assets"
                    ),
                );
            }
            Some(_) => {}
        }
    } else if let Some(d) = req.manifest.duration_sec {
        if !d.is_finite() || d <= 0.0 {
            return validation_manifest_error_ok(
                "E_INVALID_MANIFEST",
                &format!("manifest.duration_sec {d} must be finite and positive"),
            );
        }
    }
    req.manifest.clamp_in_place();
    let (report, _) = state
        .code_scene
        .run_validation_offloaded(req.source_code, req.manifest)
        .await;
    (StatusCode::OK, Json(report)).into_response()
}

/// `POST /api/v1/code-scene/assets`
pub async fn code_scene_assets_publish(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    body: Bytes,
) -> Response {
    let raw_val: Value = match parse_json_dto(&body, false) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let manifest_obj = raw_val.get("manifest").and_then(Value::as_object);
    let mut req: PublishCodeSceneRequest = match serde_json::from_value(raw_val.clone()) {
        Ok(r) => r,
        Err(e) => {
            return err_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request",
                &format!("Invalid request fields: {e}"),
            );
        }
    };
    match manifest_obj {
        None => {
            // Caller omitted `manifest` completely: populate canonical defaults for `req.kind`.
            req.manifest = AssetManifest::for_kind(req.kind);
        }
        Some(mobj) => {
            // If `manifest` was provided without its own `kind`, inherit `req.kind`;
            // if `manifest.kind` was explicitly provided and differs, `publish_validated_scene_atomic` rejects with 422!
            if !mobj.contains_key("kind") {
                req.manifest.kind = req.kind;
            }
        }
    }

    if let Some(cid) = req.share_to_chat_id {
        if cid <= 0 {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_chat_id",
                "chat_id must be a positive integer",
            );
        }
        if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
            return resp;
        }
    }

    let auth_uid = auth.user_id.0 as i64;
    let (asset, validation, created) = match publish_validated_scene_atomic(
        &state.db,
        &state.code_scene,
        auth_uid,
        &req,
        None,
        false,
    )
    .await
    {
        Ok(res) => res,
        Err(err) => return err.into_response(),
    };

    let posted_chat_message_id = if let Some(cid) = req.share_to_chat_id {
        match post_code_scene_card_to_chat(
            &state,
            ChatId(cid),
            auth.user_id,
            asset.creator_bot_id,
            &asset,
        ) {
            Ok(mid) => Some(mid),
            Err(resp) => return resp,
        }
    } else {
        None
    };

    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    (
        status,
        Json(PublishCodeSceneResponse {
            asset,
            validation,
            posted_chat_message_id,
        }),
    )
        .into_response()
}

/// `POST /api/v1/code-scene/assets/{id}/like`
pub async fn code_scene_asset_like(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    Path(scene_id): Path<String>,
) -> Response {
    let sid = scene_id.trim().to_string();
    let auth_uid = auth.user_id.0 as i64;
    let _social_guard = state.code_scene.lock_social_mutation();
    let toggle = match state.db.toggle_code_scene_like(auth_uid, &sid) {
        Ok(Some(res)) => res,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "scene_not_found", "Code Scene not found"),
        Err(_) => {
            return err_resp(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Failed to toggle like",
            );
        }
    };

    let now_ms = Utc::now().timestamp_millis();
    let ev = SceneBehaviorEventDto {
        event_id: format!("cs_ev_{}", Uuid::new_v4().simple()),
        user_id: auth_uid,
        session_id: format!("social_{auth_uid}"),
        scene_id: sid.clone(),
        kind: if toggle.liked {
            SceneBehaviorEventKind::Like
        } else {
            SceneBehaviorEventKind::Unlike
        },
        timestamp_ms: now_ms,
        ..Default::default()
    };

    let (reward, profile) = {
        let mut reco = state.code_scene.recommender().write();
        let outcome = reco.record_behavior(&ev);
        // Reconcile RAM counter & profile edge strictly to SQLite's authoritative committed state.
        reco.update_social_counters(&sid, Some(toggle.likes_count), None, None, None, None, None);
        let mut p = reco
            .export_user_profile(auth_uid)
            .unwrap_or_else(|| UserInterestProfile::new(auth_uid));
        let changed = if toggle.liked {
            p.liked_scene_ids.insert(sid.clone())
        } else {
            p.liked_scene_ids.remove(&sid)
        };
        if changed {
            p.sanitize();
            reco.import_user_profile(p.clone());
        }
        (outcome.reward, Some(p))
    };

    let _ = state
        .db
        .persist_code_scene_telemetry_batch(&[(ev, reward)], profile.as_ref(), &[], &[]);
    (StatusCode::OK, Json(toggle)).into_response()
}

/// `POST /api/v1/code-scene/assets/{id}/bookmark`
pub async fn code_scene_asset_bookmark(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    Path(scene_id): Path<String>,
) -> Response {
    let sid = scene_id.trim().to_string();
    let auth_uid = auth.user_id.0 as i64;
    let _social_guard = state.code_scene.lock_social_mutation();
    let toggle = match state.db.toggle_code_scene_bookmark(auth_uid, &sid) {
        Ok(Some(res)) => res,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "scene_not_found", "Code Scene not found"),
        Err(_) => {
            return err_resp(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Failed to toggle bookmark",
            );
        }
    };

    let now_ms = Utc::now().timestamp_millis();
    let ev = SceneBehaviorEventDto {
        event_id: format!("cs_ev_{}", Uuid::new_v4().simple()),
        user_id: auth_uid,
        session_id: format!("social_{auth_uid}"),
        scene_id: sid.clone(),
        kind: if toggle.bookmarked {
            SceneBehaviorEventKind::Bookmark
        } else {
            SceneBehaviorEventKind::Unbookmark
        },
        timestamp_ms: now_ms,
        ..Default::default()
    };

    let (reward, profile) = {
        let mut reco = state.code_scene.recommender().write();
        let outcome = reco.record_behavior(&ev);
        // Reconcile RAM counter & profile edge strictly to SQLite's authoritative committed state.
        reco.update_social_counters(
            &sid,
            None,
            Some(toggle.bookmarks_count),
            None,
            None,
            None,
            None,
        );
        let mut p = reco
            .export_user_profile(auth_uid)
            .unwrap_or_else(|| UserInterestProfile::new(auth_uid));
        let changed = if toggle.bookmarked {
            p.bookmarked_scene_ids.insert(sid.clone())
        } else {
            p.bookmarked_scene_ids.remove(&sid)
        };
        if changed {
            p.sanitize();
            reco.import_user_profile(p.clone());
        }
        (outcome.reward, Some(p))
    };

    let _ = state
        .db
        .persist_code_scene_telemetry_batch(&[(ev, reward)], profile.as_ref(), &[], &[]);
    (StatusCode::OK, Json(toggle)).into_response()
}

/// `GET /api/v1/code-scene/assets/{id}/comments`
pub async fn code_scene_comments_list(
    State(state): State<Arc<AppState>>,
    _auth: AuthBearer,
    Path(scene_id): Path<String>,
    Query(query): Query<ListCodeSceneCommentsQuery>,
) -> Response {
    let sort = query.sort.unwrap_or(CodeSceneCommentSort::Hot);
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0);
    match state
        .db
        .list_code_scene_comments(scene_id.trim(), sort, limit, offset)
    {
        Ok(Some(resp)) => (StatusCode::OK, Json(resp)).into_response(),
        Ok(None) => err_resp(StatusCode::NOT_FOUND, "scene_not_found", "Code Scene not found"),
        Err(_) => err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_error",
            "Failed to list comments",
        ),
    }
}

/// `POST /api/v1/code-scene/assets/{id}/comments`
pub async fn code_scene_comments_create(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    Path(scene_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: CreateCodeSceneCommentRequest = match parse_json_dto(&body, false) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let content = req.content.trim();
    if content.is_empty() || content.chars().count() > MAX_COMMENT_CHARS {
        return err_resp(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_comment",
            &format!("Comment content must be 1..={MAX_COMMENT_CHARS} characters"),
        );
    }

    let sid = scene_id.trim().to_string();
    let auth_uid = auth.user_id.0 as i64;
    let author_username = resolve_author_username(&state.db, auth_uid);
    let parent_id = req
        .parent_comment_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let _social_guard = state.code_scene.lock_social_mutation();
    let (comment, comments_count) = match state.db.create_code_scene_comment(
        &sid,
        auth_uid,
        &author_username,
        content,
        parent_id,
    ) {
        Ok(CreateCodeSceneCommentOutcome::Created {
            comment,
            comments_count,
        }) => (comment, comments_count),
        Ok(CreateCodeSceneCommentOutcome::SceneNotFound) => {
            return err_resp(StatusCode::NOT_FOUND, "scene_not_found", "Code Scene not found");
        }
        Ok(CreateCodeSceneCommentOutcome::ParentCommentNotFound) => {
            return err_resp(
                StatusCode::NOT_FOUND,
                "parent_comment_not_found",
                "Parent comment not found in this Code Scene",
            );
        }
        Err(_) => {
            return err_resp(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Failed to create comment",
            );
        }
    };

    let now_ms = Utc::now().timestamp_millis();
    let ev = SceneBehaviorEventDto {
        event_id: format!("cs_ev_{}", Uuid::new_v4().simple()),
        user_id: auth_uid,
        session_id: format!("social_{auth_uid}"),
        scene_id: sid.clone(),
        kind: SceneBehaviorEventKind::Comment,
        timestamp_ms: now_ms,
        ..Default::default()
    };

    let (reward, profile) = {
        let mut reco = state.code_scene.recommender().write();
        let outcome = reco.record_behavior(&ev);
        reco.update_social_counters(&sid, None, None, Some(comments_count), None, None, None);
        let p = reco.export_user_profile(auth_uid);
        (outcome.reward, p)
    };

    let _ = state
        .db
        .persist_code_scene_telemetry_batch(&[(ev, reward)], profile.as_ref(), &[], &[]);
    (
        StatusCode::CREATED,
        Json(CreateCodeSceneCommentResponse {
            comment,
            comments_count,
        }),
    )
        .into_response()
}

/// `POST /api/v1/code-scene/creators/{id}/follow`
pub async fn code_scene_creator_follow(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    Path(raw_creator_id): Path<String>,
) -> Response {
    let creator_uid: i64 = match raw_creator_id.trim().parse::<i64>() {
        Ok(id) => id,
        Err(_) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_creator_id",
                "Creator ID must be a valid integer",
            );
        }
    };
    if creator_uid <= 0 {
        return err_resp(
            StatusCode::NOT_FOUND,
            "creator_not_found",
            "Creator not found",
        );
    }

    let auth_uid = auth.user_id.0 as i64;
    let _social_guard = state.code_scene.lock_social_mutation();
    let toggle = match state.db.toggle_code_scene_follow(auth_uid, creator_uid) {
        Ok(Some(res)) => res,
        Ok(None) => {
            return err_resp(
                StatusCode::NOT_FOUND,
                "creator_not_found",
                "Creator not found",
            );
        }
        Err(_) => {
            return err_resp(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Failed to toggle follow",
            );
        }
    };

    let now_ms = Utc::now().timestamp_millis();
    let ev = SceneBehaviorEventDto {
        event_id: format!("cs_ev_{}", Uuid::new_v4().simple()),
        user_id: auth_uid,
        session_id: format!("social_{auth_uid}"),
        scene_id: String::new(),
        kind: if toggle.following {
            SceneBehaviorEventKind::FollowAuthor
        } else {
            SceneBehaviorEventKind::UnfollowAuthor
        },
        author_user_id: Some(creator_uid),
        timestamp_ms: now_ms,
        ..Default::default()
    };

    let (reward, profile) = {
        let mut reco = state.code_scene.recommender().write();
        // Explicit user follow un-hides a previously hidden author BEFORE recording FollowAuthor
        // so `code_scene_follows`, `GET /assets/{id}`, and `Following` tab remain 100% consistent.
        if toggle.following {
            if let Some(mut existing) = reco.export_user_profile(auth_uid) {
                if existing.hidden_author_ids.remove(&creator_uid) {
                    if existing
                        .creator_weights
                        .get(&creator_uid)
                        .copied()
                        .unwrap_or(0.0)
                        < 0.0
                    {
                        existing.creator_weights.remove(&creator_uid);
                    }
                    existing.sanitize();
                    reco.import_user_profile(existing);
                }
            }
        }
        let outcome = reco.record_behavior(&ev);
        let mut p = reco
            .export_user_profile(auth_uid)
            .unwrap_or_else(|| UserInterestProfile::new(auth_uid));
        let changed = if toggle.following {
            let unhid = p.hidden_author_ids.remove(&creator_uid);
            p.followed_creators.insert(creator_uid) || unhid
        } else {
            p.followed_creators.remove(&creator_uid)
        };
        if changed {
            p.sanitize();
            reco.import_user_profile(p.clone());
        }
        (outcome.reward, Some(p))
    };

    let _ = state
        .db
        .persist_code_scene_telemetry_batch(&[(ev, reward)], profile.as_ref(), &[], &[]);
    (StatusCode::OK, Json(toggle)).into_response()
}

fn validate_telemetry_event(ev: &SceneBehaviorEventDto) -> Result<(), String> {
    match ev.kind {
        SceneBehaviorEventKind::FollowAuthor
        | SceneBehaviorEventKind::UnfollowAuthor
        | SceneBehaviorEventKind::HideAuthor => {
            if ev.author_user_id.is_none_or(|id| id <= 0) && ev.scene_id.trim().is_empty() {
                return Err(format!(
                    "{} event requires positive 'author_user_id' or non-empty 'scene_id'",
                    ev.kind.as_str()
                ));
            }
        }
        SceneBehaviorEventKind::HideTag => {
            if ev.hidden_tag().is_none() {
                return Err("hide_tag event requires non-empty 'reason' or 'tags'".into());
            }
        }
        _ => {
            if ev.scene_id.trim().is_empty() {
                return Err(format!(
                    "{} event requires non-empty 'scene_id'",
                    ev.kind.as_str()
                ));
            }
        }
    }
    Ok(())
}

/// `POST /api/v1/code-scene/telemetry`
pub async fn code_scene_telemetry(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    body: Bytes,
) -> Response {
    let payload: TelemetrySubmitPayload = match parse_json_dto(&body, false) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let raw_events = payload.into_events();
    if raw_events.len() > MAX_TELEMETRY_BATCH {
        return err_resp(
            StatusCode::UNPROCESSABLE_ENTITY,
            "telemetry_batch_too_large",
            &format!("Telemetry batch exceeds maximum of {MAX_TELEMETRY_BATCH} events"),
        );
    }
    if raw_events.is_empty() {
        return (
            StatusCode::OK,
            Json(TelemetryIngestResponse {
                accepted: 0,
                reward: 0.0,
                profile_updated: false,
            }),
        )
            .into_response();
    }

    let auth_uid = auth.user_id.0 as i64;
    let now_ms = Utc::now().timestamp_millis();
    let mut prepared: Vec<(SceneBehaviorEventDto, bool)> = Vec::with_capacity(raw_events.len());
    let mut explicit_event_ids: Vec<String> = Vec::new();
    for raw in raw_events {
        let mut ev = raw.sanitized();
        if let Err(reason) = validate_telemetry_event(&ev) {
            return err_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_telemetry_event",
                &reason,
            );
        }
        ev.user_id = auth_uid;
        let had_explicit_id = !ev.event_id.trim().is_empty();
        if had_explicit_id {
            ev.event_id = ev.event_id.trim().to_string();
            explicit_event_ids.push(ev.event_id.clone());
        } else {
            ev.event_id = format!("cs_ev_{}", Uuid::new_v4().simple());
        }
        if ev.timestamp_ms <= 0 {
            ev.timestamp_ms = now_ms;
        }
        prepared.push((ev, had_explicit_id));
    }

    let _social_guard = state.code_scene.lock_social_mutation();

    // Deduplicate repeated non-empty `event_id`s per `(user_id, event_id)` against SQLite
    // and within the same batch so outbox retries never double-count plays/shares or rows.
    let existing_ids = match state
        .db
        .find_existing_code_scene_event_ids(auth_uid, &explicit_event_ids)
    {
        Ok(ids) => ids,
        Err(_) => {
            return err_resp(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Failed to check telemetry deduplication state",
            );
        }
    };
    let mut seen_in_batch: HashSet<String> = HashSet::new();
    let mut events: Vec<SceneBehaviorEventDto> = Vec::with_capacity(prepared.len());
    for (ev, had_explicit_id) in prepared {
        if had_explicit_id
            && (existing_ids.contains(&ev.event_id) || !seen_in_batch.insert(ev.event_id.clone()))
        {
            continue;
        }
        events.push(ev);
    }

    if events.is_empty() {
        return (
            StatusCode::OK,
            Json(TelemetryIngestResponse {
                accepted: 0,
                reward: 0.0,
                profile_updated: false,
            }),
        )
            .into_response();
    }

    let (
        events_with_rewards,
        total_reward,
        any_profile_updated,
        profile,
        counter_deltas,
        hidden_authors_to_unfollow,
    ) = {
        let mut reco = state.code_scene.recommender().write();

        // Snapshot authoritative relational social sets, hidden authors, & scene counters BEFORE
        // `record_behavior` so telemetry-only events update recommendation vectors/weights/rewards/
        // plays/shares without creating phantom likes/bookmarks/follows or counter drift vs SQLite.
        let (orig_liked, orig_bookmarked, orig_followed, before_hidden_authors) = reco
            .export_user_profile(auth_uid)
            .map(|p| {
                (
                    p.liked_scene_ids,
                    p.bookmarked_scene_ids,
                    p.followed_creators,
                    p.hidden_author_ids,
                )
            })
            .unwrap_or_default();
        let mut orig_scene_counters: BTreeMap<String, (u64, u64, u64, u64)> = BTreeMap::new();
        for ev in &events {
            if !ev.scene_id.is_empty() && !orig_scene_counters.contains_key(&ev.scene_id) {
                if let Some(asset) = reco.get_asset(&ev.scene_id) {
                    orig_scene_counters.insert(
                        ev.scene_id.clone(),
                        (
                            asset.likes_count,
                            asset.bookmarks_count,
                            asset.comments_count,
                            asset.remixes_count,
                        ),
                    );
                }
            }
        }

        let mut pairs = Vec::with_capacity(events.len());
        let mut sum_reward = 0.0f64;
        let mut updated = false;
        let mut deltas_by_scene: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        let mut hidden_to_unfollow_set: BTreeSet<i64> = BTreeSet::new();

        for mut ev in events {
            // Canonicalize `ev.author_user_id` to match `telemetry::resolve_author` precedence:
            // (1) `asset.author_user_id` from `scene_id` unconditionally,
            // (2) positive `ev.author_user_id`,
            // (3) positive parsed `ev.reason` for author-scoped events.
            let (before_plays, before_shares) = if !ev.scene_id.is_empty() {
                if let Some(asset) = reco.get_asset(&ev.scene_id) {
                    ev.author_user_id = Some(asset.author_user_id);
                    (asset.plays_count, asset.shares_count)
                } else {
                    (0, 0)
                }
            } else {
                (0, 0)
            };
            if ev.author_user_id.is_some_and(|id| id <= 0) {
                ev.author_user_id = None;
            }
            if ev.author_user_id.is_none()
                && matches!(
                    ev.kind,
                    SceneBehaviorEventKind::HideAuthor
                        | SceneBehaviorEventKind::FollowAuthor
                        | SceneBehaviorEventKind::UnfollowAuthor
                )
            {
                ev.author_user_id = ev
                    .reason
                    .as_deref()
                    .and_then(|s| s.trim().parse::<i64>().ok())
                    .filter(|&id| id > 0);
            }

            if ev.kind == SceneBehaviorEventKind::HideAuthor {
                if let Some(cid) = ev.author_user_id.filter(|&id| id > 0) {
                    hidden_to_unfollow_set.insert(cid);
                }
            }

            let outcome = reco.record_behavior(&ev);
            sum_reward += outcome.reward;
            if outcome.profile_updated {
                updated = true;
            }
            if !ev.scene_id.is_empty() {
                if let Some(asset) = reco.get_asset(&ev.scene_id) {
                    let dp = asset.plays_count.saturating_sub(before_plays);
                    let ds = asset.shares_count.saturating_sub(before_shares);
                    if dp > 0 || ds > 0 {
                        let entry = deltas_by_scene.entry(ev.scene_id.clone()).or_insert((0, 0));
                        entry.0 = entry.0.saturating_add(dp);
                        entry.1 = entry.1.saturating_add(ds);
                    }
                }
            }
            pairs.push((ev, outcome.reward));
        }

        // Restore DB-owned relational scene counters (preserving updated plays_count & shares_count).
        for (sid, (likes, bookmarks, comments, remixes)) in orig_scene_counters {
            reco.update_social_counters(
                &sid,
                Some(likes),
                Some(bookmarks),
                Some(comments),
                None,
                Some(remixes),
                None,
            );
        }

        // Restore DB-owned relational social edge sets on the profile (filtering any newly hidden
        // authors) while retaining ALL learned recommendation signals (`u_short`, `u_long`, `u_neg`,
        // `tag_weights`, `kind_weights`, `creator_weights`, `positive_scene_history`, `seen_ring`,
        // `disliked_scene_ids`, `hidden_author_ids`, `hidden_tags`, Swing CF).
        let prof = if let Some(mut p) = reco.export_user_profile(auth_uid) {
            for &cid in p.hidden_author_ids.difference(&before_hidden_authors) {
                if cid > 0 {
                    hidden_to_unfollow_set.insert(cid);
                }
            }
            p.liked_scene_ids = orig_liked;
            p.bookmarked_scene_ids = orig_bookmarked;
            p.followed_creators = orig_followed
                .into_iter()
                .filter(|cid| !p.hidden_author_ids.contains(cid))
                .collect();
            p.sanitize();
            reco.import_user_profile(p.clone());
            Some(p)
        } else {
            None
        };

        let deltas: Vec<(String, u64, u64)> = deltas_by_scene
            .into_iter()
            .map(|(sid, (dp, ds))| (sid, dp, ds))
            .collect();
        let hidden_to_unfollow: Vec<i64> = hidden_to_unfollow_set.into_iter().collect();
        (
            pairs,
            sum_reward,
            updated,
            prof,
            deltas,
            hidden_to_unfollow,
        )
    };

    let accepted = events_with_rewards.len();
    if state
        .db
        .persist_code_scene_telemetry_batch(
            &events_with_rewards,
            profile.as_ref(),
            &counter_deltas,
            &hidden_authors_to_unfollow,
        )
        .is_err()
    {
        return err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_error",
            "Failed to persist telemetry events",
        );
    }

    (
        StatusCode::OK,
        Json(TelemetryIngestResponse {
            accepted,
            reward: total_reward,
            profile_updated: any_profile_updated,
        }),
    )
        .into_response()
}

pub(crate) fn draft_record_from_ai_response(
    resp: &AiGenerateSceneResponse,
    user_id: i64,
    remix_parent_id: Option<String>,
    prompt_history: Vec<String>,
    created_at_ms: i64,
) -> CodeSceneBotDraftRecord {
    let now_ms = Utc::now().timestamp_millis();
    CodeSceneBotDraftRecord {
        draft_id: resp.draft_id.clone(),
        user_id,
        bot_id: resp.bot_id,
        bot_username: resp.bot_username.clone(),
        title: resp.title.clone(),
        description: resp.description.clone(),
        kind: resp.kind,
        tags: resp.tags.clone(),
        source_code: resp.source_code.clone(),
        code_format: resp.code_format.clone(),
        manifest: resp.manifest.clone(),
        controls_hint: resp.controls_hint.clone(),
        validation: resp.validation.clone(),
        engine_used: resp.engine_used.clone(),
        repair_attempts: resp.repair_attempts,
        remix_parent_id,
        prompt_history,
        published_scene_id: None,
        created_at_ms: if created_at_ms > 0 {
            created_at_ms
        } else {
            now_ms
        },
        updated_at_ms: now_ms,
    }
}

/// `POST /api/v1/code-scene/ai/generate`
pub async fn code_scene_ai_generate(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    body: Bytes,
) -> Response {
    let req: AiGenerateSceneRequest = match parse_json_dto(&body, false) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let prompt = req.prompt.trim();
    if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT_CHARS {
        return err_resp(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_prompt",
            &format!("prompt must be 1..={MAX_PROMPT_CHARS} characters"),
        );
    }

    let auth_uid = auth.user_id.0 as i64;
    let (bot_id, bot_username) = resolve_creator_bot(
        &state.db,
        req.bot_id,
        req.bot_username.as_deref(),
        true,
    );
    let user_router = state
        .fabushi_core()
        .providers_for_user(&auth.user_id.0.to_string());
    let mut outcome = CodeSceneBotGenerator::default()
        .generate(Some(&user_router), &req)
        .await;
    if !outcome.draft.validation.ok {
        let (rechecked, _) = state
            .code_scene
            .run_validation_offloaded(
                outcome.draft.source_code.clone(),
                outcome.draft.manifest.clone(),
            )
            .await;
        outcome.draft.validation = rechecked;
    }

    let draft_id = format!("draft_{}", Uuid::new_v4().simple());
    let resp = outcome.draft.into_ai_generate_response(
        draft_id,
        bot_id,
        bot_username,
        outcome.repair_attempts,
    );
    let now_ms = Utc::now().timestamp_millis();
    let rec = draft_record_from_ai_response(
        &resp,
        auth_uid,
        None,
        vec![prompt.to_string()],
        now_ms,
    );
    if state.db.upsert_code_scene_bot_draft(&rec).is_err() {
        return err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_error",
            "Failed to persist generated draft",
        );
    }
    (StatusCode::OK, Json(resp)).into_response()
}

/// `POST /api/v1/code-scene/ai/iterate`
pub async fn code_scene_ai_iterate(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    body: Bytes,
) -> Response {
    let req: AiIterateSceneRequest = match parse_json_dto(&body, false) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let instruction = req.instruction.trim();
    if instruction.is_empty() || instruction.chars().count() > MAX_PROMPT_CHARS {
        return err_resp(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_instruction",
            &format!("instruction must be 1..={MAX_PROMPT_CHARS} characters"),
        );
    }

    let auth_uid = auth.user_id.0 as i64;
    let existing_draft = match req.draft_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(did) => match state.db.get_code_scene_bot_draft(did) {
            Ok(Some(d)) if d.user_id == auth_uid => Some(d),
            Ok(Some(_)) => {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Draft belongs to another user",
                );
            }
            Ok(None) => {
                return err_resp(StatusCode::NOT_FOUND, "draft_not_found", "Draft not found");
            }
            Err(_) => {
                return err_resp(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "database_error",
                    "Failed to load draft",
                );
            }
        },
        None => None,
    };

    let (base_source, mut base_manifest, base_title, base_tags, mut history, remix_parent_id, draft_id, created_at_ms) =
        if let Some(d) = existing_draft {
            let src = req
                .source_code
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
                .unwrap_or(d.source_code);
            let man = req.manifest.clone().unwrap_or(d.manifest);
            (
                src,
                man,
                Some(d.title),
                d.tags,
                d.prompt_history,
                d.remix_parent_id,
                d.draft_id,
                d.created_at_ms,
            )
        } else if let Some(src) = req.source_code.as_deref().filter(|s| !s.trim().is_empty()) {
            let man = req
                .manifest
                .clone()
                .unwrap_or_else(|| AssetManifest::for_kind(AssetKind::Animation));
            (
                src.to_string(),
                man,
                None,
                Vec::new(),
                Vec::new(),
                None,
                format!("draft_{}", Uuid::new_v4().simple()),
                Utc::now().timestamp_millis(),
            )
        } else {
            return err_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                "missing_source",
                "ai/iterate requires either 'draft_id' or 'source_code'",
            );
        };
    base_manifest.clamp_in_place();
    history.push(instruction.to_string());

    let (bot_id, bot_username) = resolve_creator_bot(
        &state.db,
        req.bot_id,
        req.bot_username.as_deref(),
        true,
    );
    let user_router = state
        .fabushi_core()
        .providers_for_user(&auth.user_id.0.to_string());
    let mut outcome = CodeSceneBotGenerator::default()
        .iterate_or_remix(
            Some(&user_router),
            &base_source,
            &base_manifest,
            base_title.as_deref(),
            &base_tags,
            instruction,
        )
        .await;
    if !outcome.draft.validation.ok {
        let (rechecked, _) = state
            .code_scene
            .run_validation_offloaded(
                outcome.draft.source_code.clone(),
                outcome.draft.manifest.clone(),
            )
            .await;
        outcome.draft.validation = rechecked;
    }

    let resp = outcome.draft.into_ai_generate_response(
        draft_id,
        bot_id,
        bot_username,
        outcome.repair_attempts,
    );
    let rec = draft_record_from_ai_response(
        &resp,
        auth_uid,
        remix_parent_id,
        history,
        created_at_ms,
    );
    if state.db.upsert_code_scene_bot_draft(&rec).is_err() {
        return err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_error",
            "Failed to persist iterated draft",
        );
    }
    (StatusCode::OK, Json(resp)).into_response()
}

/// `POST /api/v1/code-scene/ai/publish`
pub async fn code_scene_ai_publish(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    body: Bytes,
) -> Response {
    let req: AiPublishSceneRequest = match parse_json_dto(&body, false) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    if let Some(cid) = req.chat_id {
        if cid <= 0 {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_chat_id",
                "chat_id must be a positive integer",
            );
        }
        if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
            return resp;
        }
    }

    let auth_uid = auth.user_id.0 as i64;
    let loaded_draft = match req.draft_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(did) => match state.db.get_code_scene_bot_draft(did) {
            Ok(Some(d)) if d.user_id == auth_uid => Some(d),
            Ok(Some(_)) => {
                return err_resp(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Draft belongs to another user",
                );
            }
            Ok(None) => {
                return err_resp(StatusCode::NOT_FOUND, "draft_not_found", "Draft not found");
            }
            Err(_) => {
                return err_resp(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "database_error",
                    "Failed to load draft",
                );
            }
        },
        None => None,
    };

    let (source_code, manifest, title, description, tags, controls_hint, bot_id, bot_username, remix_parent_id, mark_draft_id) =
        if let Some(d) = loaded_draft {
            let src = req
                .source_code
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(d.source_code);
            let man = req.manifest.unwrap_or(d.manifest);
            let t = req
                .title
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(d.title);
            let desc = req.description.unwrap_or(d.description);
            let tg = req.tags.unwrap_or(d.tags);
            let hint = req.controls_hint.unwrap_or(d.controls_hint);
            let bid = req.bot_id.or(d.bot_id);
            let bun = req.bot_username.or(d.bot_username);
            let parent = req.remix_parent_id.or(d.remix_parent_id);
            (src, man, t, desc, tg, hint, bid, bun, parent, Some(d.draft_id))
        } else if let Some(src) = req.source_code.filter(|s| !s.trim().is_empty()) {
            let man = req
                .manifest
                .unwrap_or_else(|| AssetManifest::for_kind(AssetKind::Animation));
            let t = req
                .title
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "AI Code Scene".to_string());
            (
                src,
                man,
                t,
                req.description.unwrap_or_default(),
                req.tags.unwrap_or_default(),
                req.controls_hint.unwrap_or_default(),
                req.bot_id,
                req.bot_username,
                req.remix_parent_id,
                None,
            )
        } else {
            return err_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                "missing_source",
                "ai/publish requires either 'draft_id' or 'source_code'",
            );
        };

    let publish_req = PublishCodeSceneRequest {
        scene_id: None,
        title,
        description,
        kind: manifest.kind,
        tags,
        source_code,
        code_format: CODE_SCENE_FORMAT_CS1.to_string(),
        manifest,
        controls_hint,
        creator_bot_id: bot_id,
        creator_bot_username: bot_username,
        remix_parent_id,
        share_to_chat_id: req.chat_id,
    };

    let (asset, validation, _created) = match publish_validated_scene_atomic(
        &state.db,
        &state.code_scene,
        auth_uid,
        &publish_req,
        mark_draft_id.as_deref(),
        true,
    )
    .await
    {
        Ok(res) => res,
        Err(err) => return err.into_response(),
    };

    let posted_chat_message_id = if let Some(cid) = req.chat_id.filter(|&id| id > 0) {
        match post_code_scene_card_to_chat(
            &state,
            ChatId(cid),
            auth.user_id,
            asset.creator_bot_id,
            &asset,
        ) {
            Ok(mid) => Some(mid),
            Err(resp) => return resp,
        }
    } else {
        None
    };

    (
        StatusCode::CREATED,
        Json(PublishCodeSceneResponse {
            asset,
            validation,
            posted_chat_message_id,
        }),
    )
        .into_response()
}

/// `POST /api/v1/code-scene/ai/remix`
pub async fn code_scene_ai_remix(
    State(state): State<Arc<AppState>>,
    auth: AuthBearer,
    body: Bytes,
) -> Response {
    let req: RemixSceneRequest = match parse_json_dto(&body, false) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let parent_id = req.parent_scene_id.trim();
    if parent_id.is_empty() {
        return err_resp(
            StatusCode::UNPROCESSABLE_ENTITY,
            "missing_parent_scene_id",
            "parent_scene_id is required",
        );
    }
    let prompt = req.prompt.trim();
    if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT_CHARS {
        return err_resp(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_prompt",
            &format!("prompt must be 1..={MAX_PROMPT_CHARS} characters"),
        );
    }

    if req.auto_publish {
        if let Some(cid) = req.chat_id {
            if cid <= 0 {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "invalid_chat_id",
                    "chat_id must be a positive integer",
                );
            }
            if let Err(resp) = check_active_membership(&state, ChatId(cid), auth.user_id) {
                return resp;
            }
        }
    }

    let parent = match state.db.get_code_scene(parent_id) {
        Ok(Some(p)) => p,
        Ok(None) => {
            return err_resp(
                StatusCode::NOT_FOUND,
                "scene_not_found",
                "Parent Code Scene not found",
            );
        }
        Err(_) => {
            return err_resp(
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "Failed to load parent scene",
            );
        }
    };

    let auth_uid = auth.user_id.0 as i64;
    let (bot_id, bot_username) = resolve_creator_bot(
        &state.db,
        req.bot_id,
        req.bot_username.as_deref(),
        true,
    );
    let user_router = state
        .fabushi_core()
        .providers_for_user(&auth.user_id.0.to_string());
    let mut outcome = CodeSceneBotGenerator::default()
        .iterate_or_remix(
            Some(&user_router),
            &parent.source_code,
            &parent.manifest,
            Some(&parent.title),
            &parent.tags,
            prompt,
        )
        .await;
    if !outcome.draft.validation.ok {
        let (rechecked, _) = state
            .code_scene
            .run_validation_offloaded(
                outcome.draft.source_code.clone(),
                outcome.draft.manifest.clone(),
            )
            .await;
        outcome.draft.validation = rechecked;
    }

    let draft_id = format!("draft_{}", Uuid::new_v4().simple());
    let draft_resp = outcome.draft.into_ai_generate_response(
        draft_id.clone(),
        bot_id,
        bot_username.clone(),
        outcome.repair_attempts,
    );
    let now_ms = Utc::now().timestamp_millis();
    let rec = draft_record_from_ai_response(
        &draft_resp,
        auth_uid,
        Some(parent.scene_id.clone()),
        vec![prompt.to_string()],
        now_ms,
    );
    if state.db.upsert_code_scene_bot_draft(&rec).is_err() {
        return err_resp(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_error",
            "Failed to persist remix draft",
        );
    }

    if !req.auto_publish {
        return (
            StatusCode::OK,
            Json(RemixSceneResponse {
                draft: draft_resp,
                published: None,
            }),
        )
            .into_response();
    }

    let publish_req = PublishCodeSceneRequest {
        scene_id: None,
        title: draft_resp.title.clone(),
        description: draft_resp.description.clone(),
        kind: draft_resp.kind,
        tags: draft_resp.tags.clone(),
        source_code: draft_resp.source_code.clone(),
        code_format: draft_resp.code_format.clone(),
        manifest: draft_resp.manifest.clone(),
        controls_hint: draft_resp.controls_hint.clone(),
        creator_bot_id: bot_id,
        creator_bot_username: bot_username,
        remix_parent_id: Some(parent.scene_id.clone()),
        share_to_chat_id: req.chat_id,
    };

    let (asset, validation, _created) = match publish_validated_scene_atomic(
        &state.db,
        &state.code_scene,
        auth_uid,
        &publish_req,
        Some(&draft_id),
        true,
    )
    .await
    {
        Ok(res) => res,
        Err(err) => return err.into_response(),
    };

    // Record Remix behavioral telemetry on parent scene & reconcile counter under social_mutation_lock.
    record_remix_parent_telemetry(
        &state.db,
        &state.code_scene,
        auth_uid,
        &parent.scene_id,
        parent.author_user_id,
    );

    let posted_chat_message_id = if let Some(cid) = req.chat_id.filter(|&id| id > 0) {
        match post_code_scene_card_to_chat(
            &state,
            ChatId(cid),
            auth.user_id,
            asset.creator_bot_id,
            &asset,
        ) {
            Ok(mid) => Some(mid),
            Err(resp) => return resp,
        }
    } else {
        None
    };

    (
        StatusCode::CREATED,
        Json(RemixSceneResponse {
            draft: draft_resp,
            published: Some(PublishCodeSceneResponse {
                asset,
                validation,
                posted_chat_message_id,
            }),
        }),
    )
        .into_response()
}
