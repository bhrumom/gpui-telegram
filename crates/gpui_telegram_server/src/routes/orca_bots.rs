//! Orca Bot Fleet CRUD, Built-in Templates, Host-Enforced Memory Governance,
//! and Overnight Tool-Less Tidy-Up REST routes (`/api/v1/orca/bots*`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use base64::Engine as _;
use gpui_telegram_proto::orca_contracts::{
    OrcaCreateBotRequest, OrcaDeleteBotResponse, OrcaFromTemplateRequest, OrcaMemoryForgetRequest,
    OrcaMemoryUndoRequest, OrcaMemoryWriteRequest, OrcaTidyUpRequest, OrcaTidyUpUndoRequest,
    OrcaUpdateBotRequest,
};
use orca_bots::{
    BotConnectorPatch, BotMemberRemoval, BotPatch, BotTemplateInstaller, CreateBotRequest,
    TidyUpChatTranscript, TidyUpDraftProvider, TidyUpDraftResponse, TidyUpPromptInput,
    TidyUpProviderOutput, TidyUpRunOutcome, TidyUpRunRequest, TidyUpSchedule,
    UpdateAvatarStyleRequest, UploadAvatarPngRequest,
};
use orca_core::bot_ecosystem::BotTemplateCategory;
use orca_core::bot_memory::{
    utc_today_iso_date, BotMemoryAuthor, TidyUpDraft, TidyUpError, TidyUpErrorCode,
};
use orca_core::bot_types::BotAgentId;
use serde::Deserialize;
use serde_json::json;

use crate::routes::orca_error::{map_bots_err, orca_err, parse_json_body, parse_optional_json_body};
use crate::server::{AppState, AuthBearer};

fn now_epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ---------------------------------------------------------------------------
// 1. Bot Fleet CRUD (`/api/v1/orca/bots`, `/api/v1/orca/bots/{bot_id}`)
// ---------------------------------------------------------------------------

pub async fn list_bots(_auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    let bots = state.orca_bots.controller().list_bots().await;
    Json(bots).into_response()
}

pub async fn create_bot(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: OrcaCreateBotRequest = match parse_optional_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let agent = match req.agent.as_deref() {
        Some(raw) => match BotAgentId::parse_strict(raw) {
            Some(a) => a,
            None => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unknown bot agent '{raw}'"),
                );
            }
        },
        None => BotAgentId::Claude,
    };
    let create_req = CreateBotRequest {
        name: req.name,
        agent,
        mascot: req.mascot,
        avatar_color: req.avatar_color,
        avatar_outfit: req.avatar_outfit,
        custom_soul: req.custom_soul,
        permission_mode: req.permission_mode,
        launch: req.launch,
        folder_workspace_id: req.folder_workspace_id,
        template_id: req.template_id,
        first_run: req.first_run,
        ..CreateBotRequest::default()
    };
    match state.orca_bots.create_bot(create_req).await {
        Ok(bot) => (StatusCode::CREATED, Json(bot)).into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn get_bot(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
) -> Response {
    match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(bot) => Json(bot).into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn update_bot(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaUpdateBotRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let parsed_agent = match req.agent.as_deref() {
        Some(raw) => match BotAgentId::parse_strict(raw) {
            Some(a) => Some(a),
            None => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unknown bot agent '{raw}'"),
                );
            }
        },
        None => None,
    };

    let ctrl = state.orca_bots.controller();
    let mut latest = None;

    if let Some(png_b64) = req.avatar_png_b64 {
        let png_bytes = match BASE64_STD.decode(png_b64.trim()) {
            Ok(b) => b,
            Err(err) => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "bot_avatar_invalid",
                    format!("Invalid base64 in avatar_png_b64: {err}"),
                );
            }
        };
        match ctrl
            .upload_avatar_png(
                &bot_id,
                UploadAvatarPngRequest {
                    png_bytes,
                    expected_avatar_revision: req.expected_avatar_revision,
                    expected_avatar_revision_epoch: req.expected_avatar_revision_epoch.clone(),
                },
            )
            .await
        {
            Ok(bot) => latest = Some(bot),
            Err(err) => return map_bots_err(err),
        }
    } else if req.avatar.is_some() || req.avatar_color.is_some() || req.avatar_outfit.is_some() {
        match ctrl
            .update_avatar_style(
                &bot_id,
                UpdateAvatarStyleRequest {
                    avatar: req.avatar,
                    color: req.avatar_color,
                    outfit: req.avatar_outfit,
                    expected_avatar_revision: req.expected_avatar_revision,
                    expected_avatar_revision_epoch: req.expected_avatar_revision_epoch,
                },
            )
            .await
        {
            Ok(bot) => latest = Some(bot),
            Err(err) => return map_bots_err(err),
        }
    }

    let has_general_patch = req.name.is_some()
        || parsed_agent.is_some()
        || req.paused.is_some()
        || req.permission_mode.is_some()
        || req.launch.is_some()
        || req.notifications.is_some()
        || !req.add_members.is_empty()
        || req.remove_member.is_some()
        || !req.remove_members.is_empty()
        || req.connectors.is_some()
        || req.skill_projects.is_some()
        || req.sidebar_order.is_some()
        || req.chat_session_id.is_some()
        || req.look_around.is_some()
        || req.tidy_up.is_some()
        || req.items_stale.is_some()
        || req.items_seen.is_some()
        || req.first_run.is_some()
        || req.offer_answer.is_some()
        || req.accept_offer.is_some()
        || req.folder_workspace_id.is_some();

    if has_general_patch {
        let patch = BotPatch {
            name: req.name,
            agent: parsed_agent,
            paused: req.paused,
            permission_mode: req.permission_mode,
            launch: req.launch,
            notifications: req.notifications,
            add_members: req.add_members,
            remove_member: req.remove_member.map(|r| BotMemberRemoval {
                source: r.source,
                id: r.id,
                workspace_id: r.workspace_id,
            }),
            remove_members: req
                .remove_members
                .into_iter()
                .map(|r| BotMemberRemoval {
                    source: r.source,
                    id: r.id,
                    workspace_id: r.workspace_id,
                })
                .collect(),
            connectors: req.connectors.map(|c| BotConnectorPatch {
                slack: c.slack,
                slack_apps: c.slack_apps,
                linear: c.linear,
                linear_workspaces: c.linear_workspaces,
                github: c.github,
                slack_mirror: c.slack_mirror,
                google: c.google,
                google_accounts: c.google_accounts,
            }),
            skill_projects: req.skill_projects,
            sidebar_order: req.sidebar_order,
            chat_session_id: req.chat_session_id,
            look_around: req.look_around,
            tidy_up: req.tidy_up,
            items_stale: req.items_stale,
            items_seen: req.items_seen,
            first_run: req.first_run,
            offer_answer: req.offer_answer.map(Into::into),
            accept_offer: req.accept_offer,
            folder_workspace_id: req.folder_workspace_id,
        };
        match ctrl.update_bot(&bot_id, patch).await {
            Ok(bot) => latest = Some(bot),
            Err(err) => return map_bots_err(err),
        }
    }

    if let Some(host_mcp) = req.host_mcp_servers {
        match ctrl.set_host_mcp_servers(&bot_id, host_mcp).await {
            Ok(bot) => latest = Some(bot),
            Err(err) => return map_bots_err(err),
        }
    }

    match latest {
        Some(bot) => Json(bot).into_response(),
        None => match ctrl.get_bot(&bot_id).await {
            Ok(bot) => Json(bot).into_response(),
            Err(err) => map_bots_err(err),
        },
    }
}

pub async fn delete_bot(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
) -> Response {
    match state.orca_bots.delete_bot(&bot_id).await {
        Ok(release) => Json(OrcaDeleteBotResponse {
            deleted: true,
            bot_id,
            released_folder: release.folder.as_str().to_string(),
        })
        .into_response(),
        Err(err) => map_bots_err(err),
    }
}

// ---------------------------------------------------------------------------
// 2. Templates & Bundled Skills
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct TemplatesQuery {
    #[serde(default)]
    pub category: Option<String>,
}

pub async fn list_templates(
    _auth: AuthBearer,
    Query(query): Query<TemplatesQuery>,
) -> Response {
    let templates = match query.category.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => match BotTemplateCategory::parse(raw) {
            Some(cat) => BotTemplateInstaller::list_templates_by_category(cat),
            None => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unknown bot template category '{raw}'"),
                );
            }
        },
        None => BotTemplateInstaller::list_templates().iter().collect(),
    };
    let skills = BotTemplateInstaller::list_bundled_skills();
    Json(json!({
        "templates": templates,
        "skills": skills,
    }))
    .into_response()
}

pub async fn from_template(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: OrcaFromTemplateRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match state
        .orca_bots
        .install_from_template(&req.template_id, req.custom_name.as_deref())
        .await
    {
        Ok(installed) => (
            StatusCode::CREATED,
            Json(json!({
                "bot": installed.bot,
                "template_id": installed.template_id,
                "installed_skills": installed.installed_skills,
                "suggested_schedules": installed.suggested_schedules,
                "look_around_enabled": installed.look_around_enabled,
            })),
        )
            .into_response(),
        Err(err) => map_bots_err(err),
    }
}

// ---------------------------------------------------------------------------
// 3. Host-Enforced Memory Governance & 14-Day Re-Add Guard
// ---------------------------------------------------------------------------

pub async fn memory_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
) -> Response {
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let home = state.orca_bots.home_store();
    let mem_path = home.bot_dir(&bot.folder).join("MEMORY.md");
    let markdown = match home.read_text_optional(&mem_path) {
        Ok(opt) => opt.unwrap_or_default(),
        Err(err) => return map_bots_err(err),
    };
    let svc = state.orca_bots.memory_service();
    let history = match svc.list_history(&bot.id) {
        Ok(h) => h,
        Err(err) => return map_bots_err(err),
    };
    let tidy_up = match svc.get_tidy_up_state(&bot.id) {
        Ok(s) => s,
        Err(err) => return map_bots_err(err),
    };
    let preamble = svc.build_session_preamble(&bot, Some("fabushi"), None).ok().map(|(p, _)| p);
    Json(json!({
        "bot_id": bot.id,
        "markdown": markdown,
        "history": history,
        "tidy_up": tidy_up,
        "preamble": preamble,
    }))
    .into_response()
}

pub async fn memory_write(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaMemoryWriteRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let home = state.orca_bots.home_store();
    if let Err(err) = home.assert_bot_owns_home(&bot.folder, &bot.id) {
        return map_bots_err(err);
    }
    let mem_path = home.bot_dir(&bot.folder).join("MEMORY.md");
    let current_disk = match home.read_text_optional(&mem_path) {
        Ok(opt) => opt.unwrap_or_default(),
        Err(err) => return map_bots_err(err),
    };
    let before_md = req
        .before_memory_md
        .unwrap_or_else(|| current_disk.clone());

    if let Some(next_md) = req.markdown {
        if let Err(err) = home.replace_bytes(&mem_path, next_md.as_bytes()) {
            return map_bots_err(err);
        }
    } else if let Some(fact_line) = req.append_line {
        let trimmed = fact_line.trim();
        if trimmed.is_empty() {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "append_line / fact must be non-empty",
            );
        }
        let formatted = if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            trimmed.to_string()
        } else {
            format!("- {trimmed}")
        };
        let next_md = if current_disk.trim().is_empty() {
            format!("{formatted}\n")
        } else {
            format!("{}\n{formatted}\n", current_disk.trim_end())
        };
        if let Err(err) = home.replace_bytes(&mem_path, next_md.as_bytes()) {
            return map_bots_err(err);
        }
    }

    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let today = req.today.unwrap_or_else(utc_today_iso_date);
    let owner_turn_text = req.owner_turn_text.as_deref().unwrap_or("");
    let session_id = req.session_id.as_deref().unwrap_or("");

    match state.orca_bots.memory_service().apply_turn_end_memory_governance(
        &bot,
        &before_md,
        req.turn_source.as_deref(),
        owner_turn_text,
        session_id,
        req.allow_deletions,
        now_ms,
        &today,
    ) {
        Ok(res) => Json(json!({
            "final_markdown": res.final_markdown,
            "tagged_lines": res.tagged_lines,
            "restored_lines": res.restored_lines,
            "intercepted_for_archive": res.intercepted_for_archive,
            "change": res.change,
            "history_row": res.history_row,
        }))
        .into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn memory_forget(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaMemoryForgetRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let by = req.by.unwrap_or(BotMemoryAuthor::Owner);
    match state
        .orca_bots
        .memory_service()
        .forget_fact(&bot, &req.query, by, now_ms)
    {
        Ok(res) => Json(json!({
            "removed_count": res.dropped,
            "markdown": res.markdown,
            "history_entry": res.history_entry,
        }))
        .into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn memory_undo(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaMemoryUndoRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    match state
        .orca_bots
        .memory_service()
        .undo_memory_change(&bot, &req.row_id, now_ms)
    {
        Ok(res) => Json(json!({
            "undone_row_id": req.row_id,
            "markdown": res.markdown,
            "history_entry": res.history_entry,
        }))
        .into_response(),
        Err(err) => map_bots_err(err),
    }
}

// ---------------------------------------------------------------------------
// 4. Overnight Tool-Less Tidy-Up & 1-Click Undo
// ---------------------------------------------------------------------------

struct ServerTidyUpDraftProvider {
    draft: Option<TidyUpDraft>,
    raw_reply: Option<String>,
    simulate_error: Option<TidyUpErrorCode>,
}

impl TidyUpDraftProvider for ServerTidyUpDraftProvider {
    fn draft_tidy_up<'a>(
        &'a self,
        input: TidyUpPromptInput,
    ) -> Pin<
        Box<
            dyn Future<Output = std::result::Result<TidyUpProviderOutput, TidyUpError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if let Some(code) = self.simulate_error {
                return Err(TidyUpError::new(
                    code,
                    format!("Tidy-up draft failed with {}", code.as_str()),
                ));
            }
            if let Some(ref raw) = self.raw_reply {
                return Ok(TidyUpProviderOutput {
                    response: TidyUpDraftResponse::RawReply(raw.clone()),
                    elapsed_ms: 1,
                });
            }
            if let Some(ref draft) = self.draft {
                return Ok(TidyUpProviderOutput {
                    response: TidyUpDraftResponse::Draft(draft.clone()),
                    elapsed_ms: 1,
                });
            }
            // Deterministic tool-less default pass: preserves all live lines verbatim
            // (passing all 16 validators and protected owner-rule invariants).
            let index = if input.memory_md.trim().is_empty() {
                format!("- {} | Memory initialized\n", input.today)
            } else {
                input.memory_md
            };
            Ok(TidyUpProviderOutput {
                response: TidyUpDraftResponse::Draft(TidyUpDraft {
                    index,
                    topics: std::collections::BTreeMap::new(),
                    replaced: Vec::new(),
                    merged: Vec::new(),
                }),
                elapsed_ms: 1,
            })
        })
    }
}

fn parse_tidy_up_error_code(raw: &str) -> TidyUpErrorCode {
    TidyUpErrorCode::ALL
        .into_iter()
        .find(|c| c.as_str() == raw.trim())
        .unwrap_or(TidyUpErrorCode::InvalidFormat)
}

pub async fn tidy_up_run(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaTidyUpRequest = match parse_optional_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let svc = state.orca_bots.memory_service();

    if let Some(cfg) = req.configure {
        let sched = match cfg.schedule.trim().to_ascii_lowercase().as_str() {
            "night" | "nightly" => TidyUpSchedule::Night,
            "week" | "weekly" => TidyUpSchedule::Week,
            other => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unknown tidy_up schedule '{other}' (expected 'night' or 'week')"),
                );
            }
        };
        if let Err(err) = svc.configure_tidy_up(&bot.id, cfg.enabled, sched, now_ms) {
            return map_bots_err(err);
        }
        if req.run == Some(false)
            || (req.run.is_none()
                && !req.force
                && req.draft.is_none()
                && req.raw_reply.is_none()
                && req.simulate_error.is_none())
        {
            let st = match svc.get_tidy_up_state(&bot.id) {
                Ok(s) => s,
                Err(err) => return map_bots_err(err),
            };
            return Json(json!({
                "outcome": "configured",
                "state": st,
            }))
            .into_response();
        }
    }

    let mut chats: Vec<TidyUpChatTranscript> = Vec::with_capacity(req.chats.len());
    for chat_val in req.chats {
        match serde_json::from_value::<TidyUpChatTranscript>(chat_val) {
            Ok(c) => chats.push(c),
            Err(e) => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Invalid tidy-up chat transcript: {e}"),
                );
            }
        }
    }

    let provider = ServerTidyUpDraftProvider {
        draft: req.draft,
        raw_reply: req.raw_reply,
        simulate_error: req.simulate_error.as_deref().map(parse_tidy_up_error_code),
    };
    let today = req.today.unwrap_or_else(utc_today_iso_date);
    let run_req = TidyUpRunRequest {
        chats,
        now_ms,
        today,
        utc_offset_seconds: req.utc_offset_seconds.unwrap_or(0),
        allow_sleep_catchup: req.allow_sleep_catchup,
        force: req.force,
    };

    match svc.run_tidy_up(&bot, &provider, run_req).await {
        Ok(TidyUpRunOutcome::SkippedNotDue) => {
            let st = match svc.get_tidy_up_state(&bot.id) {
                Ok(s) => s,
                Err(err) => return map_bots_err(err),
            };
            Json(json!({
                "outcome": "skipped_not_due",
                "state": st,
            }))
            .into_response()
        }
        Ok(TidyUpRunOutcome::Applied {
            plan,
            home_chat_note,
        }) => {
            let st = match svc.get_tidy_up_state(&bot.id) {
                Ok(s) => s,
                Err(err) => return map_bots_err(err),
            };
            Json(json!({
                "outcome": "applied",
                "state": st,
                "home_chat_note": home_chat_note,
                "plan": plan,
            }))
            .into_response()
        }
        Err(err) => map_bots_err(err),
    }
}

pub async fn tidy_up_undo(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaTidyUpUndoRequest = match parse_optional_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let svc = state.orca_bots.memory_service();
    match svc.undo_tidy_up(&bot, req.row_id.as_deref(), now_ms) {
        Ok(res) => {
            let st = svc.get_tidy_up_state(&bot.id).ok();
            Json(json!({
                "markdown": res.markdown,
                "history_entry": res.history_entry,
                "state": st,
            }))
            .into_response()
        }
        Err(err) => map_bots_err(err),
    }
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/orca/bots", get(list_bots).post(create_bot))
        .route("/api/v1/orca/bots/templates", get(list_templates))
        .route("/api/v1/orca/bots/from-template", post(from_template))
        .route(
            "/api/v1/orca/bots/{bot_id}",
            get(get_bot).patch(update_bot).delete(delete_bot),
        )
        .route(
            "/api/v1/orca/bots/{bot_id}/memory",
            get(memory_get).post(memory_write),
        )
        .route("/api/v1/orca/bots/{bot_id}/memory/forget", post(memory_forget))
        .route("/api/v1/orca/bots/{bot_id}/memory/undo", post(memory_undo))
        .route("/api/v1/orca/bots/{bot_id}/tidy-up", post(tidy_up_run))
        .route("/api/v1/orca/bots/{bot_id}/tidy-up/undo", post(tidy_up_undo))
}
