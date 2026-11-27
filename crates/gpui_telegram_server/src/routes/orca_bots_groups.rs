//! Orca Multi-Bot Group Rooms, "Who Can Ask" Connector Access Control,
//! Transactional Workspace Hand-Over, Bot Chat Tree, and Bot Chats/Reply
//! (`--leave-thread` anti-injection & `NO_REPLY` suppression) REST routes.

use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use gpui_telegram_proto::orca_contracts::{
    OrcaBotChatReplyRequest, OrcaChatTreeRequest, OrcaCreateAccessRequest,
    OrcaCreateBotChatRequest, OrcaCreateGroupRequest, OrcaCreateGroupThreadRequest,
    OrcaDecideAccessRequest, OrcaHandOverRequest, OrcaSendGroupMessageRequest,
};
use orca_bots::wire_json::chat_tree_to_wire_json;
use orca_bots::{
    BotPatch, ConnectorAccessRequestStatus, CreateAccessRequestInput, CreateBotGroupRequest,
    GroupMessageRequester, GroupMessageSender, HandOverTarget, LeaveThreadContext,
    LeaveThreadDecision, OrcaBotsError, PostGroupMessageRequest, WakeFrameKind,
    evaluate_leave_thread_request, suggested_bot_name,
};
use orca_core::bot_ecosystem::{
    BotChatLiveState, BotChatNode, BotChatNodeKind, BotChatNodeNote, HandOverOutcome,
};
use orca_core::bot_groups::{
    BotGroupFolderSlug, BotGroupReplyMode, BotGroupSend, MentionCandidate,
};
use orca_core::bot_preamble::{addressed_bot_words, is_quiet_reply, quiet_bot_result_text};
use orca_core::bot_types::{BotAgentId, BotMember, HandedPairedWorkspace};
use orca_core::workspace_layout::TabViewMode;
use orca_core::BotErrorCode;
use orca_ide::{AppendJournalItemRequest, CreateAgentSessionRequest};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::routes::orca_error::{
    map_bots_err, map_ide_err, orca_err, parse_json_body, parse_optional_json_body,
};
use crate::server::{AppState, AuthBearer};

fn now_epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn bot_workspace_key(bot_id: &str) -> String {
    format!("bot:{bot_id}")
}

fn parse_group_sender(val: Option<Value>) -> Result<GroupMessageSender, Response> {
    let Some(v) = val else {
        return Ok(GroupMessageSender::Owner);
    };
    if let Some(s) = v.as_str() {
        let trimmed = s.trim();
        if trimmed.eq_ignore_ascii_case("owner") {
            return Ok(GroupMessageSender::Owner);
        }
        if trimmed.eq_ignore_ascii_case("system") {
            return Ok(GroupMessageSender::System);
        }
        if let Some(bot_id) = trimmed.strip_prefix("bot:") {
            return Ok(GroupMessageSender::Bot {
                bot_id: bot_id.trim().to_string(),
            });
        }
    }
    serde_json::from_value(v).map_err(|err| {
        orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            format!("Invalid group message sender: {err}"),
        )
    })
}

fn parse_group_requester(val: Option<Value>) -> Result<Option<GroupMessageRequester>, Response> {
    let Some(v) = val else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    if let Some(s) = v.as_str() {
        let trimmed = s.trim();
        if trimmed.eq_ignore_ascii_case("owner") {
            return Ok(Some(GroupMessageRequester::Owner));
        }
        if let Some(bot_id) = trimmed.strip_prefix("bot:") {
            return Ok(Some(GroupMessageRequester::Bot {
                bot_id: bot_id.trim().to_string(),
                session_id: None,
            }));
        }
    }
    serde_json::from_value(v).map(Some).map_err(|err| {
        orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            format!("Invalid group message requester: {err}"),
        )
    })
}

// ---------------------------------------------------------------------------
// 1. Multi-Bot Group Rooms (`/api/v1/orca/groups*`)
// ---------------------------------------------------------------------------

pub async fn list_groups(_auth: AuthBearer, State(state): State<Arc<AppState>>) -> Response {
    match state.orca_bots.group_runtime().list_groups() {
        Ok(groups) => Json(groups).into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn get_group(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
) -> Response {
    match state.orca_bots.group_runtime().get_group(&group_id) {
        Ok(Some(group)) => Json(group).into_response(),
        Ok(None) => orca_err(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("Bot group '{group_id}' was not found"),
        ),
        Err(err) => map_bots_err(err),
    }
}

pub async fn create_group(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: OrcaCreateGroupRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let folder = match req.folder.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => match BotGroupFolderSlug::new(raw) {
            Ok(slug) => Some(slug),
            Err(err) => return map_bots_err(OrcaBotsError::Bot(err.into())),
        },
        None => None,
    };
    match state
        .orca_bots
        .group_runtime()
        .create_group(CreateBotGroupRequest {
            name: req.name,
            folder,
            member_bot_ids: req.member_bot_ids,
            reply_mode: req.reply_mode.unwrap_or(BotGroupReplyMode::All),
            remote_members: req.remote_members,
        }) {
        Ok(group) => (StatusCode::CREATED, Json(group)).into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn create_group_thread(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaCreateGroupThreadRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match state
        .orca_bots
        .group_runtime()
        .create_thread(&group_id, &req.first_post)
    {
        Ok(thread) => (StatusCode::CREATED, Json(thread)).into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn send_group_message(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaSendGroupMessageRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let sender = if req.sender.is_none()
        && let Some(ref bid) = req.from_bot_id
        && !bid.trim().is_empty()
    {
        GroupMessageSender::Bot {
            bot_id: bid.trim().to_string(),
        }
    } else {
        match parse_group_sender(req.sender) {
            Ok(s) => s,
            Err(resp) => return resp,
        }
    };
    let requester = match parse_group_requester(req.requester) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let all_bots = state.orca_bots.controller().list_bots().await;
    let active_local_candidates: Vec<MentionCandidate> = all_bots
        .iter()
        .map(|b| MentionCandidate {
            id: b.id.clone(),
            name: b.name.as_str().to_string(),
            is_remote: false,
        })
        .collect();
    let paused_bot_ids: HashSet<String> = all_bots
        .iter()
        .filter(|b| b.paused)
        .map(|b| b.id.clone())
        .collect();

    match state
        .orca_bots
        .group_runtime()
        .post_message(PostGroupMessageRequest {
            group_id,
            thread_id: req.thread_id,
            sender,
            send: BotGroupSend {
                text: req.text,
                images: req.images,
            },
            active_local_candidates,
            paused_bot_ids,
            requester,
            peer_scope: req.peer_scope,
            synthetic_root: req.synthetic_root,
        }) {
        Ok(posted) => Json(json!({
            "message": posted.message,
            "targets": posted.targets,
            "targetBotIds": posted.targets.dispatched_bot_ids,
            "dispatchedBotIds": posted.targets.dispatched_bot_ids,
            "heldBotIds": posted.targets.held_bot_ids,
            "pendingBotIds": posted.targets.held_bot_ids,
            "handedOff": posted.targets.handed_off || req.handoff,
            "operation_ids": posted.operation_ids,
            "pending_ids": posted.pending_ids,
            "remote_asks": posted.remote_asks,
            "remoteAsks": posted.remote_asks,
        }))
        .into_response(),
        Err(err) => map_bots_err(err),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct GroupMessagesQuery {
    #[serde(default)]
    pub thread_id: Option<String>,
}

pub async fn list_group_messages(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
    Query(query): Query<GroupMessagesQuery>,
) -> Response {
    let msgs = match state
        .orca_bots
        .group_runtime()
        .list_messages(&group_id, query.thread_id.as_deref())
    {
        Ok(m) => m,
        Err(err) => return map_bots_err(err),
    };
    let ops = state
        .orca_bots
        .group_runtime()
        .list_operations(&group_id)
        .unwrap_or_default();
    let enriched: Vec<Value> = msgs
        .into_iter()
        .map(|m| {
            let mailed = m.mailed.clone();
            let mut v = serde_json::to_value(m).unwrap_or_else(|_| json!({}));
            if let Some(obj) = v.as_object_mut() {
                obj.insert("deliveredBotIds".into(), json!(mailed));
            }
            v
        })
        .collect();
    Json(json!({
        "messages": enriched,
        "turns": ops,
        "deliveries": ops,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// 2. "Who Can Ask" Connector Access Control (`/api/v1/orca/bots/{bot_id}/access*`)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct AccessListQuery {
    #[serde(default)]
    pub bot_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

fn parse_access_status(raw: Option<&str>) -> Result<Option<ConnectorAccessRequestStatus>, Response> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    match s.to_ascii_lowercase().as_str() {
        "pending" => Ok(Some(ConnectorAccessRequestStatus::Pending)),
        "granted" => Ok(Some(ConnectorAccessRequestStatus::Granted)),
        "declined" => Ok(Some(ConnectorAccessRequestStatus::Declined)),
        other => Err(orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            format!("Unknown access request status '{other}' (expected pending|granted|declined)"),
        )),
    }
}

pub async fn access_list_all(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<AccessListQuery>,
) -> Response {
    let status = match parse_access_status(query.status.as_deref()) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    match state
        .orca_bots
        .access_host()
        .list_requests(query.bot_id.as_deref(), status)
    {
        Ok(list) => Json(list).into_response(),
        Err(err) => map_bots_err(err),
    }
}

fn access_req_to_wire_json(req: &orca_bots::ConnectorAccessRequest) -> Value {
    let mut val = serde_json::to_value(req).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("id".into(), json!(req.request_id));
    }
    val
}

pub async fn access_list(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    Query(query): Query<AccessListQuery>,
) -> Response {
    let status = match parse_access_status(query.status.as_deref()) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let filter_bot = if bot_id == "*" || bot_id.eq_ignore_ascii_case("all") {
        None
    } else {
        Some(bot_id.as_str())
    };
    match state
        .orca_bots
        .access_host()
        .list_requests(filter_bot, status)
    {
        Ok(list) => {
            let wire: Vec<Value> = list.iter().map(access_req_to_wire_json).collect();
            Json(wire).into_response()
        }
        Err(err) => map_bots_err(err),
    }
}

pub async fn access_create(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaCreateAccessRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let member = match req.member {
        Some(m) => m,
        None => {
            let Some(uid) = req
                .user_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
            else {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "Provide 'member' or flat 'externalUserId'/'user_id' for access request",
                );
            };
            let name = req
                .name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| uid.clone());
            BotMember {
                id: uid,
                name,
                source: req.source.or_else(|| Some("slack".to_string())),
                workspace_id: req.connector_workspace_id.clone(),
                avatar_url: None,
                kind: None,
            }
        }
    };
    let input = CreateAccessRequestInput {
        bot_id,
        member,
        connector_workspace_id: req.connector_workspace_id,
        ttl_ms: req.ttl_ms,
        now_ms: req.now_ms.unwrap_or_else(now_epoch_ms),
    };
    let host = state.orca_bots.access_host();
    let res = match req
        .target_host_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(target_host) => host.create_request_for_host(target_host, input),
        None => host.create_request(input),
    };
    match res {
        Ok(created) => {
            state
                .orca_wake_hub
                .notify_host(&created.host_id, WakeFrameKind::AccessRequest);
            (StatusCode::CREATED, Json(access_req_to_wire_json(&created))).into_response()
        }
        Err(err) => map_bots_err(err),
    }
}

pub async fn access_decide(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(_bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaDecideAccessRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let host = state.orca_bots.access_host();
    let ctrl = state.orca_bots.controller();

    let resolved_approve = if let Some(ref dec) = req.decision {
        match dec.trim().to_ascii_lowercase().as_str() {
            "granted" | "grant" | "approve" | "approved" | "allow" | "yes" => true,
            "declined" | "decline" | "denied" | "deny" | "reject" | "rejected" | "no" => false,
            other => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Unsupported access decision '{other}' (expected granted|declined)"),
                );
            }
        }
    } else {
        req.approve.unwrap_or(true)
    };

    let res = if let Some(token) = req
        .redeem_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        host.redeem_token(token, now_ms, ctrl).await
    } else if let Some(request_id) = req
        .request_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        host.decide_request(request_id, resolved_approve, now_ms, ctrl)
            .await
    } else {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Provide either 'redeem_token' or 'request_id' for access/decide",
        );
    };

    match res {
        Ok(outcome) => Json(json!({
            "id": outcome.request.request_id,
            "request": access_req_to_wire_json(&outcome.request),
            "added_locally": outcome.added_locally,
        }))
        .into_response(),
        Err(err) => map_bots_err(err),
    }
}

// ---------------------------------------------------------------------------
// 3. Transactional Workspace Hand-Over (`/api/v1/orca/bots/{bot_id}/hand-over`)
// ---------------------------------------------------------------------------

pub async fn hand_over(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaHandOverRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let lineage = state.orca_handover_lineage.as_ref();
    if let Some(ref initial_parent) = req.expected_initial_parent {
        let parent_opt = if initial_parent.trim().is_empty() {
            None
        } else {
            Some(initial_parent.trim().to_string())
        };
        if let Err(err) = lineage.set_workspace_parent(&req.workspace_id, parent_opt) {
            return map_bots_err(err);
        }
    }
    let notify_fail_reason: Option<String> = match req.simulate_notify_failure.as_ref() {
        Some(Value::Bool(true)) => Some("simulated notify failure".to_string()),
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    };
    if let Some(ref fail_reason) = notify_fail_reason {
        let reparent = req.simulate_concurrent_reparent.as_ref().map(|p| {
            let trimmed = p.trim();
            (
                req.workspace_id.clone(),
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                },
            )
        });
        if let Err(err) = lineage.inject_notify_failure(fail_reason.clone(), reparent) {
            return map_bots_err(err);
        }
    }
    if let Some(ref restore_reason) = req.simulate_restore_failure
        && let Err(err) = lineage.inject_restore_failure(restore_reason.clone())
    {
        return map_bots_err(err);
    }

    let name_seed = req
        .branch
        .as_deref()
        .or(req.workspace_name.as_deref())
        .unwrap_or(&req.workspace_id);
    let suggested = suggested_bot_name(
        name_seed,
        req.repo_slug.as_deref(),
        req.recent_prompt.as_deref(),
    )
    .ok()
    .map(|n| n.into_inner());

    let mode = req
        .mode
        .as_deref()
        .map(str::trim)
        .unwrap_or("existing")
        .to_ascii_lowercase();
    let target = match mode.as_str() {
        "create_new" | "new" => HandOverTarget::CreateNewBot {
            workspace_name: req
                .branch
                .clone()
                .or(req.workspace_name)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| req.workspace_id.clone()),
            repo_slug: req.repo_slug,
            recent_prompt: req.recent_prompt,
            agent: req.agent.unwrap_or(BotAgentId::Claude),
            custom_name: req.custom_name,
        },
        "existing" => HandOverTarget::ExistingBot {
            bot_id: bot_id.clone(),
            purpose: req
                .purpose
                .or(req.recent_prompt)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "Supervise workspace".to_string()),
        },
        other => {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Unsupported hand-over mode '{other}' (expected 'existing' or 'create_new')"),
            );
        }
    };

    match state
        .orca_bots
        .hand_over_workspace(lineage, &req.workspace_id, target)
        .await
    {
        Ok(outcome) => {
            let moved_and_notified = matches!(
                outcome,
                HandOverOutcome::Attached { .. } | HandOverOutcome::Created { .. }
            );
            if let HandOverOutcome::Attached { bot_id: ref bid }
            | HandOverOutcome::Created { bot_id: ref bid, .. } = outcome
            {
                let _ = state
                    .orca_bots
                    .controller()
                    .update_bot(
                        bid,
                        BotPatch {
                            add_handed_paired_workspace: Some(HandedPairedWorkspace {
                                workspace_id: req.workspace_id.clone(),
                                handed_at: now_epoch_ms(),
                            }),
                            ..BotPatch::default()
                        },
                    )
                    .await;
            }
            let mut val = serde_json::to_value(&outcome).unwrap_or_else(|_| json!({}));
            if let Some(obj) = val.as_object_mut() {
                if let Some(ref sname) = suggested {
                    obj.insert("suggestedBotName".into(), json!(sname));
                }
                obj.insert("moved".into(), json!(moved_and_notified));
                obj.insert("notified".into(), json!(moved_and_notified));
                obj.insert("workspaceId".into(), json!(req.workspace_id));
                if let HandOverOutcome::RolledBack {
                    placement: orca_core::bot_ecosystem::HandOverRollbackPlacement::MovedSince,
                    ..
                } = &outcome
                {
                    use orca_bots::WorkspaceLineageAdapter as _;
                    let cur_parent = lineage
                        .get_parent(&req.workspace_id)
                        .ok()
                        .flatten()
                        .or_else(|| req.simulate_concurrent_reparent.clone());
                    obj.insert("placement".into(), json!("moved-since"));
                    obj.insert("currentParent".into(), json!(cur_parent));
                    obj.insert("current_parent".into(), json!(cur_parent));
                    obj.insert("reason".into(), json!("lineage_parent_changed"));
                }
            }
            Json(val).into_response()
        }
        Err(err) => map_bots_err(err),
    }
}

// ---------------------------------------------------------------------------
// 4. Bot Chat Tree (`/api/v1/orca/bots/{bot_id}/chat-tree`)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ChatTreeQuery {
    #[serde(default)]
    pub now_ms: Option<i64>,
}

fn default_chat_tree_nodes(
    state: &AppState,
    bot: &orca_core::bot_types::OrcaBotRecord,
) -> Vec<BotChatNode> {
    let ws_key = bot_workspace_key(&bot.id);
    let mut nodes = Vec::new();
    let mut seen = HashSet::new();

    if let Some(ref home_id) = bot.chat_session_id {
        seen.insert(home_id.clone());
        nodes.push(BotChatNode {
            session_id: home_id.clone(),
            parent_session_id: None,
            title: format!("{} (Home)", bot.name.as_str()),
            kind: BotChatNodeKind::Home,
            live_state: BotChatLiveState::Idle,
            note: None,
            unread: false,
            needs_you: false,
            updated_at_ms: bot.updated_at,
        });
    }

    if let Ok(sessions) = state.orca_ide.chat_journal().list_sessions(Some(&ws_key)) {
        for sess in sessions {
            if !seen.insert(sess.session_id.clone()) {
                continue;
            }
            let kind = if nodes.is_empty() {
                BotChatNodeKind::Home
            } else {
                BotChatNodeKind::Side
            };
            nodes.push(BotChatNode {
                session_id: sess.session_id,
                parent_session_id: sess.parent_session_id,
                title: format!("Chat {}", sess.agent_id),
                kind,
                live_state: BotChatLiveState::Idle,
                note: None,
                unread: false,
                needs_you: false,
                updated_at_ms: sess.updated_at_ms,
            });
        }
    }

    if nodes.is_empty() {
        nodes.push(BotChatNode {
            session_id: format!("home-{}", bot.id),
            parent_session_id: None,
            title: format!("{} (Home)", bot.name.as_str()),
            kind: BotChatNodeKind::Home,
            live_state: BotChatLiveState::Idle,
            note: None,
            unread: false,
            needs_you: false,
            updated_at_ms: bot.updated_at,
        });
    }

    if let Some(ref handed) = bot.handed_paired_workspaces {
        let home_sid = nodes.first().map(|n| n.session_id.clone());
        for hw in handed {
            let sid = format!("handed:{}", hw.workspace_id);
            if seen.insert(sid.clone()) {
                nodes.push(BotChatNode {
                    session_id: sid,
                    parent_session_id: home_sid.clone(),
                    title: hw.workspace_id.clone(),
                    kind: BotChatNodeKind::Side,
                    live_state: BotChatLiveState::Idle,
                    note: Some(BotChatNodeNote::HandedOver),
                    unread: false,
                    needs_you: false,
                    updated_at_ms: hw.handed_at,
                });
            }
        }
    }

    nodes
}

pub async fn chat_tree_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    Query(query): Query<ChatTreeQuery>,
) -> Response {
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let nodes = default_chat_tree_nodes(&state, &bot);
    let now_ms = query.now_ms.unwrap_or_else(now_epoch_ms);
    let usage_limit = state
        .orca_bots
        .get_usage_limit_continue(&bot.id)
        .ok()
        .flatten();
    match state
        .orca_bots
        .build_chat_tree(&bot.id, &nodes, usage_limit.clone(), Vec::new(), now_ms)
    {
        Ok(view) => Json(chat_tree_to_wire_json(&bot, &view, usage_limit.as_ref())).into_response(),
        Err(err) => map_bots_err(err),
    }
}

pub async fn chat_tree_post(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaChatTreeRequest = match parse_optional_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let nodes = if req.nodes.is_empty() {
        default_chat_tree_nodes(&state, &bot)
    } else {
        req.nodes
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let usage_limit = req.usage_limit.or_else(|| {
        state
            .orca_bots
            .get_usage_limit_continue(&bot.id)
            .ok()
            .flatten()
    });
    match state
        .orca_bots
        .build_chat_tree(&bot.id, &nodes, usage_limit.clone(), req.cards, now_ms)
    {
        Ok(view) => Json(chat_tree_to_wire_json(&bot, &view, usage_limit.as_ref())).into_response(),
        Err(err) => map_bots_err(err),
    }
}

// ---------------------------------------------------------------------------
// 5. Bot Chats & Reply (`--leave-thread` Anti-Injection & `NO_REPLY`)
// ---------------------------------------------------------------------------

pub async fn bot_chats_list(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
) -> Response {
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let ws_key = bot_workspace_key(&bot.id);
    match state.orca_ide.chat_journal().list_sessions(Some(&ws_key)) {
        Ok(chats) => Json(json!({
            "bot_id": bot.id,
            "home_session_id": bot.chat_session_id,
            "chats": chats,
        }))
        .into_response(),
        Err(err) => map_ide_err(err),
    }
}

pub async fn bot_chats_create(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaCreateBotChatRequest = match parse_optional_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let ws_key = bot_workspace_key(&bot.id);
    let snap = match state
        .orca_ide
        .chat_journal()
        .create_session(CreateAgentSessionRequest {
            session_id: req.session_id,
            workspace_id: ws_key,
            host_id: Some(bot.host_id.clone()),
            agent_id: bot.agent.as_str().to_string(),
            view_mode: TabViewMode::Chat,
            active_cli_owner: false,
            provider_handle_chain: Vec::new(),
            adopted_native_id: None,
            now_ms: Some(now_ms),
        }) {
        Ok(s) => s,
        Err(err) => return map_ide_err(err),
    };
    if let Some(ref title) = req.title
        && !title.trim().is_empty()
    {
        let _ = state
            .orca_ide
            .chat_journal()
            .append_item(AppendJournalItemRequest {
                session_id: snap.session.session_id.clone(),
                item_id: None,
                item_kind: "chat_metadata".to_string(),
                provider_item_id: None,
                payload_json: json!({ "title": title.trim(), "bot_id": bot.id }),
                now_ms: Some(now_ms),
            });
    }
    if bot.chat_session_id.is_none() {
        let _ = state
            .orca_bots
            .controller()
            .update_bot(
                &bot.id,
                BotPatch {
                    chat_session_id: Some(Some(snap.session.session_id.clone())),
                    ..BotPatch::default()
                },
            )
            .await;
    } else {
        let _ = state.orca_bots.home_store().sync_generated_home_files(&bot);
    }
    (
        StatusCode::CREATED,
        Json(json!({
            "id": snap.session.session_id,
            "bot_id": bot.id,
            "chat_id": snap.session.session_id,
            "chatId": snap.session.session_id,
            "title": req.title,
            "session": snap.session,
            "chat_state": snap.chat_state,
        })),
    )
        .into_response()
}

pub async fn bot_reply(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: OrcaBotChatReplyRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let bot = match state.orca_bots.controller().get_bot(&bot_id).await {
        Ok(b) => b,
        Err(err) => return map_bots_err(err),
    };
    if bot.paused {
        return map_bots_err(OrcaBotsError::bot(
            BotErrorCode::BotPaused,
            format!("Bot '{}' is paused and cannot reply", bot.id),
        ));
    }

    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let journal = state.orca_ide.chat_journal();
    let ws_key = bot_workspace_key(&bot.id);

    let chat_id = match req.chat_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(explicit) => {
            if journal.get_session(explicit).is_err() {
                if bot.chat_session_id.as_deref() == Some(explicit) {
                    if let Err(err) = journal.create_session(CreateAgentSessionRequest {
                        session_id: Some(explicit.to_string()),
                        workspace_id: ws_key.clone(),
                        host_id: Some(bot.host_id.clone()),
                        agent_id: bot.agent.as_str().to_string(),
                        view_mode: TabViewMode::Chat,
                        active_cli_owner: false,
                        provider_handle_chain: Vec::new(),
                        adopted_native_id: None,
                        now_ms: Some(now_ms),
                    }) {
                        return map_ide_err(err);
                    }
                } else {
                    return map_bots_err(OrcaBotsError::bot(
                        BotErrorCode::BotChatNotFound,
                        format!("Bot chat session '{explicit}' not found"),
                    ));
                }
            }
            explicit.to_string()
        }
        None => {
            let home_id = bot
                .chat_session_id
                .clone()
                .unwrap_or_else(|| format!("home-{}", bot.id));
            if journal.get_session(&home_id).is_err() {
                if let Err(err) = journal.create_session(CreateAgentSessionRequest {
                    session_id: Some(home_id.clone()),
                    workspace_id: ws_key.clone(),
                    host_id: Some(bot.host_id.clone()),
                    agent_id: bot.agent.as_str().to_string(),
                    view_mode: TabViewMode::Chat,
                    active_cli_owner: false,
                    provider_handle_chain: Vec::new(),
                    adopted_native_id: None,
                    now_ms: Some(now_ms),
                }) {
                    return map_ide_err(err);
                }
            }
            home_id
        }
    };

    let leave_thread_decision = if req.leave_thread {
        let mut other_owned: Vec<String> = req.other_bot_names.clone();
        for other in state.orca_bots.controller().list_bots().await {
            if other.id != bot.id {
                other_owned.push(other.name.as_str().to_string());
            }
        }
        let other_refs: Vec<&str> = other_owned.iter().map(String::as_str).collect();
        let ctx = LeaveThreadContext {
            text: &req.text,
            bot_name: bot.name.as_str(),
            other_bot_names: &other_refs,
            is_forwarded: req.is_forwarded,
            is_quoted_reply: req.is_quoted_reply,
            addressed_bot_name: req.addressed_bot_name.as_deref(),
        };
        Some(evaluate_leave_thread_request(&ctx))
    } else {
        None
    };
    let left_thread = matches!(leave_thread_decision, Some(LeaveThreadDecision::Allow));

    let quiet = is_quiet_reply(&req.text);
    let remaining_words = addressed_bot_words(&req.text);
    let suppressed = quiet && remaining_words.is_none();
    let delivered_text = if suppressed {
        None
    } else if quiet {
        Some(quiet_bot_result_text(&req.text))
    } else {
        Some(req.text.clone())
    };

    let item_kind = if suppressed {
        "bot_reply_suppressed"
    } else {
        "bot_reply"
    };
    let item = match journal.append_item(AppendJournalItemRequest {
        session_id: chat_id.clone(),
        item_id: None,
        item_kind: item_kind.to_string(),
        provider_item_id: None,
        payload_json: json!({
            "bot_id": bot.id,
            "raw_text": req.text,
            "delivered_text": delivered_text,
            "quiet": quiet,
            "suppressed": suppressed,
            "leave_thread_requested": req.leave_thread,
            "left_thread": left_thread,
            "leave_thread_decision": leave_thread_decision,
        }),
        now_ms: Some(now_ms),
    }) {
        Ok(it) => it,
        Err(err) => return map_ide_err(err),
    };

    Json(json!({
        "bot_id": bot.id,
        "chat_id": chat_id,
        "item_id": item.item_id,
        "sequence": item.sequence,
        "quiet": quiet,
        "suppressed": suppressed,
        "delivered_text": delivered_text,
        "left_thread": left_thread,
        "leave_thread_decision": leave_thread_decision,
    }))
    .into_response()
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/orca/groups", get(list_groups).post(create_group))
        .route("/api/v1/orca/groups/{group_id}", get(get_group))
        .route(
            "/api/v1/orca/groups/{group_id}/threads",
            post(create_group_thread),
        )
        .route(
            "/api/v1/orca/groups/{group_id}/send",
            post(send_group_message),
        )
        .route(
            "/api/v1/orca/groups/{group_id}/messages",
            get(list_group_messages),
        )
        .route("/api/v1/orca/access", get(access_list_all))
        .route(
            "/api/v1/orca/bots/{bot_id}/access",
            get(access_list).post(access_create),
        )
        .route(
            "/api/v1/orca/bots/{bot_id}/access/decide",
            post(access_decide),
        )
        .route("/api/v1/orca/bots/{bot_id}/hand-over", post(hand_over))
        .route(
            "/api/v1/orca/bots/{bot_id}/chat-tree",
            get(chat_tree_get).post(chat_tree_post),
        )
        .route(
            "/api/v1/orca/bots/{bot_id}/chats",
            get(bot_chats_list).post(bot_chats_create),
        )
        .route("/api/v1/orca/bots/{bot_id}/reply", post(bot_reply))
}
