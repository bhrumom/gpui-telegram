//! Unified RPC routing for `bots.*`/`bot.*`, `groups.*`/`group.*`,
//! `webhooks.*`/`webhook.*`, and `automations.*`/`automation.*` bridged directly
//! to `Option<Arc<orca_bots::OrcaBotsFacade>>`.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use orca_bots::wire_json::{
    bot_record_to_wire_json, chat_tree_to_wire_json, parse_usage_limit_continue,
};
use orca_bots::{
    BotMemberRemoval, BotPatch, BotTemplateInstaller, CreateAutomationRequest,
    CreateBotGroupRequest, CreateBotItemRequest, CreateBotRequest, CreateWebhookRequest,
    ExportPackageOptions, GroupMessageSender, IngestWebhookCallRequest, ListBotItemsFilter,
    OrcaBotsFacade, PostGroupMessageRequest, UpdateAvatarStyleRequest, UploadAvatarPngRequest,
    decorate_external_action_body, require_owner_asker,
};
use orca_core::BotErrorCode;
use orca_core::bot_ecosystem::{
    BotChatLiveState, BotChatNode, BotChatNodeKind, NativeChatBotCard, UsageLimitContinue,
};
use orca_core::bot_groups::{BotGroupReplyMode, BotGroupSend, MentionCandidate};
use orca_core::bot_items::{BotItemActor, BotItemStatus};
use orca_core::bot_memory::BotMemoryAuthor;
use orca_core::bot_types::{
    ActsForAsker, BotAgentId, BotItemsStale, BotLaunchConfig, BotMember, BotNotifications,
    BotPermissionMode, ConnectorAskerRole, ConnectorSource, normalize_persisted_bots,
};
use orca_core::triggers_webhooks::{
    AutomationBotResultsTo, AutomationBotRunsIn, AutomationBotTarget, AutomationEventTrigger,
    OneShotWait, WatchPrStepAction, WatchPrWakeOutcome, WebhookAdapterId, WebhookFilters,
};
use serde_json::{Value, json};

use crate::rpc_dispatcher::{RpcSubsystemHandles, bots_routes_ext, bots_routes_tools};
use crate::{OrcaIdeRuntimeError, Result, now_epoch_ms};

fn require_bots(bots: Option<&Arc<OrcaBotsFacade>>) -> Result<&Arc<OrcaBotsFacade>> {
    bots.ok_or_else(|| {
        OrcaIdeRuntimeError::bot(
            BotErrorCode::BotsDisabled,
            "orca_bots facade is not attached to this OrcaIdeFacade instance",
        )
    })
}

fn req_str<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument(format!("missing required string '{key}'")))
}

fn opt_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

fn opt_str_any<'a>(params: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for k in keys {
        if let Some(v) = opt_str(params, k) {
            return Some(v);
        }
    }
    None
}

fn req_str_any<'a>(params: &'a Value, keys: &[&str]) -> Result<&'a str> {
    opt_str_any(params, keys).ok_or_else(|| {
        OrcaIdeRuntimeError::invalid_argument(format!(
            "missing required string '{}'",
            keys.first().copied().unwrap_or("param")
        ))
    })
}

pub(super) async fn dispatch_bots_namespace(
    handles: &Arc<RpcSubsystemHandles>,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let bots = require_bots(handles.bots.as_ref())?;
    let now_ms = params
        .get("now_ms")
        .or_else(|| params.get("nowMs"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);

    if let Some((prefix, sub)) = op.split_once('.') {
        return match prefix {
            "home" => bots_routes_ext::dispatch_bots_home(bots, sub, params).await,
            "chat" => bots_routes_ext::dispatch_bots_chat(bots, sub, params).await,
            "memory" => bots_routes_ext::dispatch_bots_memory(bots, sub, params, now_ms).await,
            "tidyUp" | "tidy_up" => bots_routes_ext::dispatch_bots_tidy_up(bots, sub, params).await,
            "items" => bots_routes_ext::dispatch_bots_items(bots, sub, params, now_ms).await,
            "lookAround" | "look_around" => {
                bots_routes_ext::dispatch_bots_look_around(bots, sub, params, now_ms).await
            }
            "upNext" | "up_next" => {
                bots_routes_ext::dispatch_bots_up_next(bots, sub, params, now_ms).await
            }
            "groups" => bots_routes_ext::dispatch_bots_groups(bots, sub, params).await,
            "tools" => bots_routes_tools::dispatch_bots_tools(bots, sub, params).await,
            "storage" if sub == "loadRecords" || sub == "load_records" => {
                let raw = params
                    .get("records")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let normalized = normalize_persisted_bots(&raw);
                let out: Vec<Value> = normalized
                    .iter()
                    .map(|b| bot_record_to_wire_json(b, bots.home_store()))
                    .collect();
                Ok(json!({ "bots": out }))
            }
            "avatar" if sub == "set" => handle_avatar_set(bots, params).await,
            "connectors" => handle_bots_connectors(bots, sub, params).await,
            other => Err(OrcaIdeRuntimeError::not_found(format!(
                "unknown bots RPC sub-namespace 'bots.{other}.{sub}'"
            ))),
        };
    }

    match op {
        "list" => {
            let list = bots.controller().list_bots().await;
            let out: Vec<Value> = list
                .iter()
                .map(|b| bot_record_to_wire_json(b, bots.home_store()))
                .collect();
            Ok(Value::Array(out))
        }
        "roster" => {
            let list = bots.controller().list_bots().await;
            let out: Vec<Value> = list
                .iter()
                .map(|b| bot_record_to_wire_json(b, bots.home_store()))
                .collect();
            Ok(json!({ "bots": out }))
        }
        "get" => {
            let id = req_str_any(params, &["bot_id", "botId", "id"])?;
            let bot = bots.controller().get_bot(id).await?;
            Ok(bot_record_to_wire_json(&bot, bots.home_store()))
        }
        "create" => {
            let agent = match opt_str(params, "agent") {
                Some(raw) => BotAgentId::parse_strict(raw).ok_or_else(|| {
                    OrcaIdeRuntimeError::invalid_argument(format!("unknown bot agent '{raw}'"))
                })?,
                None => BotAgentId::Claude,
            };
            let permission_mode = params
                .get("permission_mode")
                .or_else(|| params.get("permissionMode"))
                .cloned()
                .map(serde_json::from_value::<BotPermissionMode>)
                .transpose()?;
            let launch = params
                .get("launch")
                .cloned()
                .map(serde_json::from_value::<BotLaunchConfig>)
                .transpose()?;
            let items_stale = params
                .get("items_stale")
                .or_else(|| params.get("itemsStale"))
                .cloned()
                .map(serde_json::from_value::<BotItemsStale>)
                .transpose()?;
            let req = CreateBotRequest {
                name: opt_str(params, "name").map(ToOwned::to_owned),
                agent,
                folder: opt_str(params, "folder").map(ToOwned::to_owned),
                home_path: opt_str_any(params, &["homePath", "home_path"]).map(PathBuf::from),
                custom_soul: opt_str_any(params, &["custom_soul", "customSoul", "soul"])
                    .map(ToOwned::to_owned),
                permission_mode,
                launch,
                folder_workspace_id: opt_str_any(
                    params,
                    &["folder_workspace_id", "folderWorkspaceId"],
                )
                .map(ToOwned::to_owned),
                first_run: params
                    .get("first_run")
                    .or_else(|| params.get("firstRun"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                items_stale,
                ..CreateBotRequest::default()
            };
            let bot = bots.create_bot(req).await?;
            Ok(bot_record_to_wire_json(&bot, bots.home_store()))
        }
        "update" => {
            let id = req_str_any(params, &["bot_id", "botId", "id"])?;
            let agent = match opt_str(params, "agent") {
                Some(raw) => Some(BotAgentId::parse_strict(raw).ok_or_else(|| {
                    OrcaIdeRuntimeError::invalid_argument(format!("unknown bot agent '{raw}'"))
                })?),
                None => None,
            };
            let add_members: Vec<BotMember> = params
                .get("addMembers")
                .or_else(|| params.get("add_members"))
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let remove_member: Option<BotMemberRemoval> = params
                .get("removeMember")
                .or_else(|| params.get("remove_member"))
                .cloned()
                .map(serde_json::from_value)
                .transpose()?;
            let remove_members: Vec<BotMemberRemoval> = params
                .get("removeMembers")
                .or_else(|| params.get("remove_members"))
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let permission_mode = params
                .get("permissionMode")
                .or_else(|| params.get("permission_mode"))
                .cloned()
                .map(serde_json::from_value::<Option<BotPermissionMode>>)
                .transpose()?;
            let launch = params
                .get("launch")
                .cloned()
                .map(serde_json::from_value::<Option<BotLaunchConfig>>)
                .transpose()?;
            let notifications = params
                .get("notifications")
                .cloned()
                .map(serde_json::from_value::<BotNotifications>)
                .transpose()?;
            let skill_projects = params
                .get("skillProjects")
                .or_else(|| params.get("skill_projects"))
                .cloned()
                .map(serde_json::from_value::<Option<Vec<String>>>)
                .transpose()?;
            let sidebar_order = params
                .get("sidebarOrder")
                .or_else(|| params.get("sidebar_order"))
                .cloned()
                .map(serde_json::from_value::<Option<u32>>)
                .transpose()?;
            let items_stale = params
                .get("itemsStale")
                .or_else(|| params.get("items_stale"))
                .cloned()
                .map(serde_json::from_value::<Option<BotItemsStale>>)
                .transpose()?;
            let paused = params.get("paused").and_then(Value::as_bool);
            let patch = BotPatch {
                name: opt_str(params, "name").map(ToOwned::to_owned),
                agent,
                paused,
                permission_mode,
                launch,
                notifications,
                add_members,
                remove_member,
                remove_members,
                skill_projects,
                sidebar_order,
                items_stale,
                first_run: params
                    .get("first_run")
                    .or_else(|| params.get("firstRun"))
                    .and_then(Value::as_bool),
                ..BotPatch::default()
            };
            let updated = bots.controller().update_bot(id, patch).await?;
            if paused == Some(false) {
                let _ = bots.group_runtime().flush_pending_for_bot(&updated.id);
            }
            if let Some(ul_val) = params
                .get("usageLimitContinue")
                .or_else(|| params.get("usage_limit_continue"))
            {
                let ul = parse_usage_limit_continue(ul_val, updated.agent.as_str(), now_ms)?;
                bots.set_usage_limit_continue(&updated.id, ul)?;
            }
            Ok(bot_record_to_wire_json(&updated, bots.home_store()))
        }
        "delete" => {
            let id = req_str_any(params, &["bot_id", "botId", "id"])?;
            let release = bots.delete_bot(id).await?;
            Ok(json!({
                "deleted": true,
                "bot_id": id,
                "released_folder": release.folder.as_str(),
            }))
        }
        "templates" => {
            let templates = BotTemplateInstaller::list_templates();
            Ok(json!({ "templates": templates }))
        }
        "from_template" => {
            let template_id = req_str(params, "template_id")?;
            let custom_name = opt_str(params, "custom_name").or_else(|| opt_str(params, "name"));
            let installed = bots.install_from_template(template_id, custom_name).await?;
            Ok(json!({
                "bot": installed.bot,
                "template_id": installed.template_id,
                "installed_skills": installed.installed_skills,
                "suggested_schedules": installed.suggested_schedules,
                "look_around_enabled": installed.look_around_enabled,
            }))
        }
        "items_list" => {
            let filter = ListBotItemsFilter {
                owner_bot_id: opt_str(params, "bot_id")
                    .or_else(|| opt_str(params, "owner_bot_id"))
                    .map(ToOwned::to_owned),
                include_closed: params.get("include_closed").and_then(Value::as_bool).unwrap_or(false),
                include_snoozed: params.get("include_snoozed").and_then(Value::as_bool).unwrap_or(false),
                now_ms: Some(now_ms),
                limit: params.get("limit").and_then(Value::as_u64).map(|n| n as usize),
                offset: params.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                statuses: None,
            };
            let items = bots.items_store().list_items(filter)?;
            Ok(serde_json::to_value(items)?)
        }
        "item_create" => {
            let owner_bot_id = req_str(params, "bot_id").or_else(|_| req_str(params, "owner_bot_id"))?;
            let title = req_str(params, "title")?;
            let actor = params
                .get("actor")
                .cloned()
                .map(serde_json::from_value::<BotItemActor>)
                .transpose()?
                .unwrap_or(BotItemActor::Owner);
            let mut req = CreateBotItemRequest::new(owner_bot_id, actor, title);
            req.description = opt_str(params, "description").map(ToOwned::to_owned);
            req.done_means = opt_str(params, "done_means").map(ToOwned::to_owned);
            if let Some(st_val) = params.get("status") {
                req.status = serde_json::from_value::<BotItemStatus>(st_val.clone())?;
            }
            let item = bots.items_store().create_item(req, now_ms)?;
            Ok(serde_json::to_value(item)?)
        }
        "item_close" => {
            let uid = req_str(params, "item_uid").or_else(|_| req_str(params, "uid"))?;
            let actor = params
                .get("actor")
                .cloned()
                .map(serde_json::from_value::<BotItemActor>)
                .transpose()?
                .unwrap_or(BotItemActor::Owner);
            let status = params
                .get("status")
                .cloned()
                .map(serde_json::from_value::<BotItemStatus>)
                .transpose()?
                .unwrap_or(BotItemStatus::Done);
            let close_note = opt_str(params, "close_note").map(ToOwned::to_owned);
            let expected_version = params.get("expected_version").and_then(Value::as_u64);
            let (item, outcome) = bots
                .close_item(uid, actor, status, close_note, expected_version, None, now_ms)
                .await?;
            Ok(json!({
                "item": item,
                "emitted_events": outcome.emitted_events,
                "unsnoozed_early": outcome.unsnoozed_early,
            }))
        }
        _ => dispatch_bots_namespace_part2(handles, bots, op, params, now_ms).await,
    }
}

async fn handle_avatar_set(bots: &Arc<OrcaBotsFacade>, params: &Value) -> Result<Value> {
    let bot_id = req_str_any(params, &["botId", "bot_id", "id"])?;
    let expected_rev = params
        .get("expectedAvatarRevision")
        .or_else(|| params.get("expected_avatar_revision"))
        .and_then(Value::as_u64);
    let expected_epoch = opt_str_any(
        params,
        &["expectedAvatarRevisionEpoch", "expected_avatar_revision_epoch"],
    )
    .map(ToOwned::to_owned);

    let updated = if let Some(png_b64) =
        opt_str_any(params, &["pngBase64", "png_base64", "pngB64", "avatarPngB64"])
    {
        let png_bytes = BASE64_STD.decode(png_b64.trim()).map_err(|e| {
            OrcaIdeRuntimeError::bot(
                BotErrorCode::BotAvatarInvalid,
                format!("invalid base64 in pngBase64: {e}"),
            )
        })?;
        bots.controller()
            .upload_avatar_png(
                bot_id,
                UploadAvatarPngRequest {
                    png_bytes,
                    expected_avatar_revision: expected_rev,
                    expected_avatar_revision_epoch: expected_epoch,
                },
            )
            .await?
    } else {
        let avatar = opt_str(params, "mascot")
            .map(|m| {
                if m.starts_with("mascot:") {
                    m.to_string()
                } else {
                    format!("mascot:{m}")
                }
            })
            .or_else(|| opt_str(params, "avatar").map(ToOwned::to_owned));
        let color = opt_str_any(params, &["color", "avatarColor"]).map(ToOwned::to_owned);
        let outfit = opt_str_any(params, &["outfit", "avatarOutfit"]).map(ToOwned::to_owned);
        bots.controller()
            .update_avatar_style(
                bot_id,
                UpdateAvatarStyleRequest {
                    avatar,
                    color,
                    outfit,
                    expected_avatar_revision: expected_rev,
                    expected_avatar_revision_epoch: expected_epoch,
                },
            )
            .await?
    };
    Ok(bot_record_to_wire_json(&updated, bots.home_store()))
}

async fn dispatch_bots_namespace_part2(
    handles: &Arc<RpcSubsystemHandles>,
    bots: &Arc<OrcaBotsFacade>,
    op: &str,
    params: &Value,
    now_ms: i64,
) -> Result<Value> {
    match op {
        "needs_you" => {
            let bot_id = opt_str(params, "bot_id");
            let focused = opt_str(params, "focused_uid");
            let cap = params.get("cap").and_then(Value::as_u64).map(|n| n as usize);
            let fold = bots
                .items_store()
                .needs_you_inbox(bot_id, focused, cap, now_ms)?;
            Ok(json!({
                "visible": fold.visible,
                "folded": fold.folded,
                "hidden_count": fold.hidden_count,
                "approval_count": fold.approval_count,
                "focused_preserved": fold.focused_preserved,
            }))
        }
        "look_around" => {
            let bot_id = req_str(params, "bot_id")?;
            let state = bots.up_next_store().get_state(bot_id)?;
            let suggestions = bots.up_next_store().list_suggestions(bot_id)?;
            Ok(json!({
                "state": state,
                "suggestions": suggestions,
            }))
        }
        "memory_forget" => {
            let bot_id = req_str(params, "bot_id")?;
            let query = req_str(params, "query")?;
            let bot = bots.controller().get_bot(bot_id).await?;
            let by = params
                .get("by")
                .cloned()
                .map(serde_json::from_value::<BotMemoryAuthor>)
                .transpose()?
                .unwrap_or(BotMemoryAuthor::Owner);
            let res = bots.memory_service().forget_fact(&bot, query, by, now_ms)?;
            Ok(json!({
                "removed_count": res.dropped,
                "history_entry": res.history_entry,
            }))
        }
        "memory_undo" => {
            let bot_id = req_str(params, "bot_id")?;
            let row_id = req_str(params, "row_id").or_else(|_| req_str(params, "history_id"))?;
            let bot = bots.controller().get_bot(bot_id).await?;
            let res = bots
                .memory_service()
                .undo_memory_change(&bot, row_id, now_ms)?;
            Ok(json!({
                "undone_row_id": row_id,
                "history_entry": res.history_entry,
            }))
        }
        "tidy_up" => {
            let bot_id = req_str(params, "bot_id")?;
            let state = bots.memory_service().get_tidy_up_state(bot_id)?;
            Ok(serde_json::to_value(state)?)
        }
        "access_list" => {
            let bot_id = opt_str(params, "bot_id");
            let requests = bots.access_host().list_requests(bot_id, None)?;
            Ok(serde_json::to_value(requests)?)
        }
        "package_export" => {
            let bot_id = req_str(params, "bot_id")?;
            let include_memory = params
                .get("include_memory")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let opts = ExportPackageOptions {
                include_memory_index: include_memory,
                include_memory_topics: include_memory,
                include_skills: params
                    .get("include_skills")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                include_avatar_png: params
                    .get("include_avatar")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                connectors_hint: Vec::new(),
            };
            let exported = bots.export_package(bot_id, &opts).await?;
            Ok(json!({
                "manifest": exported.validated.manifest,
                "zip_size_bytes": exported.zip_bytes.len(),
                "zip_b64": BASE64_STD.encode(&exported.zip_bytes),
            }))
        }
        "import_discover" => {
            let found = bots.discover_imports(now_ms)?;
            Ok(serde_json::to_value(found)?)
        }
        "chat_tree" | "chatTree" => {
            let bot_id = req_str_any(params, &["bot_id", "botId", "id"])?;
            let bot = bots.controller().get_bot(bot_id).await?;
            let mut nodes: Vec<BotChatNode> = params
                .get("nodes")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            if nodes.is_empty() {
                let ws_key = format!("bot:{}", bot.id);
                let mut seen = HashSet::new();
                if let Some(ref hid) = bot.chat_session_id {
                    seen.insert(hid.clone());
                    nodes.push(BotChatNode {
                        session_id: hid.clone(),
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
                if let Ok(sessions) = handles.chat_journal.list_sessions(Some(&ws_key)) {
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
            }
            let usage_limit: Option<UsageLimitContinue> = params
                .get("usage_limit")
                .or_else(|| params.get("usageLimit"))
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .or_else(|| bots.get_usage_limit_continue(&bot.id).ok().flatten());
            let cards: Vec<NativeChatBotCard> = params
                .get("cards")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let view = bots.build_chat_tree(&bot.id, &nodes, usage_limit.clone(), cards, now_ms)?;
            Ok(chat_tree_to_wire_json(&bot, &view, usage_limit.as_ref()))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown bots RPC method 'bots.{other}'"
        ))),
    }
}

pub(super) async fn dispatch_groups_namespace(
    bots_opt: Option<&Arc<OrcaBotsFacade>>,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let bots = require_bots(bots_opt)?;
    match op {
        "list" => Ok(serde_json::to_value(bots.group_runtime().list_groups()?)?),
        "create" => {
            let name = req_str(params, "name")?.to_string();
            let member_bot_ids: Vec<String> = params
                .get("member_bot_ids")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let reply_mode: BotGroupReplyMode = params
                .get("reply_mode")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or(BotGroupReplyMode::All);
            let group = bots.group_runtime().create_group(CreateBotGroupRequest {
                name,
                folder: None,
                member_bot_ids,
                reply_mode,
                remote_members: Vec::new(),
            })?;
            Ok(serde_json::to_value(group)?)
        }
        "get" => {
            let group_id = req_str(params, "group_id").or_else(|_| req_str(params, "id"))?;
            let group = bots.group_runtime().get_group(group_id)?;
            Ok(serde_json::to_value(group)?)
        }
        "send" => {
            let group_id = req_str(params, "group_id")?.to_string();
            let text = req_str(params, "text")?.to_string();
            let thread_id = opt_str(params, "thread_id").map(ToOwned::to_owned);
            let sender: GroupMessageSender = params
                .get("sender")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or(GroupMessageSender::Owner);
            let active_local_candidates: Vec<MentionCandidate> = bots
                .controller()
                .list_bots()
                .await
                .into_iter()
                .map(|b| MentionCandidate {
                    id: b.id,
                    name: b.name.as_str().to_string(),
                    is_remote: false,
                })
                .collect();
            let paused_bot_ids: HashSet<String> = bots
                .controller()
                .list_bots()
                .await
                .into_iter()
                .filter(|b| b.paused)
                .map(|b| b.id)
                .collect();
            let posted = bots.group_runtime().post_message(PostGroupMessageRequest {
                group_id,
                thread_id,
                sender,
                send: BotGroupSend {
                    text,
                    images: Vec::new(),
                },
                active_local_candidates,
                paused_bot_ids,
                requester: None,
                peer_scope: None,
                synthetic_root: None,
            })?;
            Ok(json!({
                "message": posted.message,
                "operation_ids": posted.operation_ids,
                "pending_ids": posted.pending_ids,
            }))
        }
        "messages" => {
            let group_id = req_str(params, "group_id")?;
            let thread_id = opt_str(params, "thread_id");
            let msgs = bots.group_runtime().list_messages(group_id, thread_id)?;
            Ok(serde_json::to_value(msgs)?)
        }
        "thread_create" => {
            let group_id = req_str(params, "group_id")?;
            let first_post = req_str(params, "first_post").or_else(|_| req_str(params, "title"))?;
            let thread = bots.group_runtime().create_thread(group_id, first_post)?;
            Ok(serde_json::to_value(thread)?)
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown groups RPC method 'groups.{other}'"
        ))),
    }
}

pub(super) async fn dispatch_webhooks_namespace(
    bots_opt: Option<&Arc<OrcaBotsFacade>>,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let bots = require_bots(bots_opt)?;
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);

    match op {
        // SECURITY INVARIANT: list & show ALWAYS return secret-free WebhookCliView!
        "list" => {
            let bot_id = opt_str(params, "bot_id");
            let views = bots.webhook_engine().list_cli_views(bot_id)?;
            Ok(serde_json::to_value(views)?)
        }
        "show" => {
            let hook_id = req_str(params, "hook_id").or_else(|_| req_str(params, "id"))?;
            let view = bots.webhook_engine().show_cli_view(hook_id)?;
            Ok(serde_json::to_value(view)?)
        }
        "create" => {
            let bot_id = req_str(params, "bot_id")?.to_string();
            let automation_id = req_str(params, "automation_id")?.to_string();
            let adapter: WebhookAdapterId = params
                .get("adapter")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or(WebhookAdapterId::Standard);
            let filters: WebhookFilters = params
                .get("filters")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let hourly_cap = params
                .get("hourly_cap")
                .and_then(Value::as_u64)
                .map(|v| v as u32);
            let grace_minutes = params
                .get("grace_minutes")
                .and_then(Value::as_u64)
                .map(|v| v as u32);
            let require_signature = params
                .get("require_signature")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let (cli_view, _copy_card) = bots.webhook_engine().create_hook(CreateWebhookRequest {
                bot_id,
                automation_id,
                adapter,
                filters,
                hourly_cap,
                grace_minutes,
                require_signature,
                now_ms,
            })?;
            Ok(serde_json::to_value(cli_view)?)
        }
        "ingest" => {
            let hook_id = req_str(params, "hook_id")?.to_string();
            let headers: BTreeMap<String, String> = params
                .get("headers")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let raw_body: Vec<u8> = if let Some(s) = params.get("raw_body").and_then(Value::as_str)
            {
                s.as_bytes().to_vec()
            } else if let Some(body_val) = params.get("body") {
                serde_json::to_vec(body_val)?
            } else {
                b"{}".to_vec()
            };
            let delivery_epoch = params.get("delivery_epoch").and_then(Value::as_u64);
            let idempotency_key = opt_str(params, "idempotency_key").map(ToOwned::to_owned);
            let outcome = bots.webhook_engine().ingest_call(IngestWebhookCallRequest {
                hook_id,
                headers,
                raw_body,
                delivery_epoch,
                idempotency_key,
                now_ms: now_ms.max(0) as u64,
            })?;
            Ok(json!({
                "call_id": outcome.call_id,
                "receipt_status": outcome.receipt_status,
                "state": outcome.state,
                "start_answer": outcome.start_answer,
                "trigger_event": outcome.trigger_event,
                "notice": outcome.notice,
            }))
        }
        "calls" => {
            let call_id = req_str(params, "call_id").or_else(|_| req_str(params, "callId"))?;
            let call = bots.webhook_engine().get_call(call_id)?;
            let state_str = if call.filter_rejected {
                "filtered"
            } else {
                call.state.as_str()
            };
            // SECURITY INVARIANT (SEC-03): NEVER expose spoolPath, ingest URL, secret, raw body, or raw headers.
            Ok(json!({
                "id": call.call_id,
                "callId": call.call_id,
                "hookId": call.hook_id,
                "botId": call.bot_id,
                "adapter": call.adapter,
                "epoch": call.epoch,
                "state": state_str,
                "filterRejected": call.filter_rejected,
                "receivedAtMs": call.received_at_ms,
                "updatedAtMs": call.updated_at_ms,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown webhooks RPC method 'webhooks.{other}'"
        ))),
    }
}

pub(super) async fn dispatch_automations_namespace(
    bots_opt: Option<&Arc<OrcaBotsFacade>>,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let bots = require_bots(bots_opt)?;
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);

    match op {
        "list" => {
            let bot_id = opt_str(params, "bot_id");
            let list = bots.automation_engine().list_automations(bot_id)?;
            Ok(serde_json::to_value(list)?)
        }
        "create" => {
            let name = req_str(params, "name")?.to_string();
            let prompt = req_str(params, "prompt")?.to_string();
            let bot_id = req_str(params, "bot_id")?.to_string();
            let enabled = params
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let dtstart_ms = params.get("dtstart_ms").and_then(Value::as_i64);
            let rrule = opt_str(params, "rrule").map(ToOwned::to_owned);
            let triggers: Vec<AutomationEventTrigger> = params
                .get("triggers")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let once: Option<OneShotWait> = params
                .get("once")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?;
            let record = bots
                .automation_engine()
                .register_automation(CreateAutomationRequest {
                    name,
                    prompt,
                    enabled,
                    bot_target: AutomationBotTarget {
                        bot_id,
                        runs_in: match opt_str(params, "chat_session_id") {
                            Some(sid) => AutomationBotRunsIn::Chat {
                                session_id: sid.to_string(),
                                title: None,
                                home_if_gone: None,
                                to_conversation: None,
                            },
                            None => AutomationBotRunsIn::New,
                        },
                        results_to: AutomationBotResultsTo::Chat,
                        agent_setup: None,
                        let_bot_react: None,
                        precheck: None,
                        setup_trust_hash: None,
                        missed_run_grace_minutes: None,
                    },
                    dtstart_ms,
                    rrule,
                    triggers,
                    once,
                    now_ms: now_ms.max(0) as u64,
                })?;
            Ok(serde_json::to_value(record)?)
        }
        "remove" => {
            let id = req_str(params, "automation_id").or_else(|_| req_str(params, "id"))?;
            let removed = bots.automation_engine().remove_automation(id)?;
            Ok(json!({ "removed": removed, "automation_id": id }))
        }
        "watch_pr" | "watchPr" => {
            let bot_id = req_str_any(params, &["bot_id", "botId"])?;
            let session_id = opt_str_any(params, &["session_id", "sessionId"]).unwrap_or("home");
            let pr_owned = if let Some(pr) = opt_str(params, "pr") {
                pr.to_string()
            } else if let (Some(repo), Some(num)) = (
                opt_str(params, "repo"),
                params
                    .get("prNumber")
                    .or_else(|| params.get("pr_number"))
                    .and_then(Value::as_u64),
            ) {
                format!("{repo}#{num}")
            } else {
                return Err(OrcaIdeRuntimeError::invalid_argument(
                    "missing 'pr' or 'repo'+'prNumber'",
                ));
            };
            let duration = opt_str(params, "duration");
            let ci_supported = params
                .get("ci_finished_supported")
                .or_else(|| params.get("ciFinishedSupported"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let watch = bots.automation_engine().watch_pr().start_watch(
                bot_id,
                session_id,
                &pr_owned,
                now_ms.max(0) as u64,
                duration,
                ci_supported,
            )?;
            let mut val = serde_json::to_value(&watch)?;
            if let Some(obj) = val.as_object_mut() {
                obj.insert("id".into(), json!(watch.watch_id));
            }
            Ok(val)
        }
        "triggerEvent" | "trigger_event" => {
            let aid = req_str_any(params, &["automationId", "automation_id", "watchId", "watch_id", "id"])?;
            let kind = opt_str_any(params, &["kind", "event", "outcome"])
                .unwrap_or("ci_failed")
                .trim()
                .to_ascii_lowercase();
            let coord = bots.automation_engine().watch_pr();
            if coord.get_watch(aid).is_ok() {
                let outcome = match kind.as_str() {
                    "ci_failed" | "merge_conflict" | "changes_requested" | "needs_change" | "needs_fix" => {
                        let authorized = params
                            .get("authorized")
                            .and_then(Value::as_bool)
                            .unwrap_or(true);
                        WatchPrWakeOutcome::NeedsChange { authorized }
                    }
                    "pr_merged" | "pr_closed" | "merged" | "closed" | "merged_or_closed" => {
                        WatchPrWakeOutcome::MergedOrClosed
                    }
                    "ready_to_merge" | "ready" | "ci_passed" => WatchPrWakeOutcome::ReadyToMerge,
                    _ => WatchPrWakeOutcome::StillRunningOrQuestion,
                };
                let step = coord.handle_wake(aid, outcome, now_ms.max(0) as u64)?;
                let watch = coord.get_watch(aid)?;
                let dispatched = matches!(step, WatchPrStepAction::DispatchWorkerFix { .. });
                let escalated = matches!(step, WatchPrStepAction::EscalateToOwnerAndStop { .. });
                let action_name = match step {
                    WatchPrStepAction::DispatchWorkerFix { .. } => "dispatch_worker_fix",
                    WatchPrStepAction::EscalateToOwnerAndStop { .. } => "escalate_to_owner_and_stop",
                    WatchPrStepAction::AnnounceAndStop { .. } => "announce_and_stop",
                    WatchPrStepAction::NotifyOwnerAndRearmSameAttempt { .. } => {
                        "notify_owner_and_rearm_same_attempt"
                    }
                    WatchPrStepAction::ForwardQuestionAndRearmSameAttempt { .. } => {
                        "forward_question_and_rearm_same_attempt"
                    }
                };
                return Ok(json!({
                    "automationId": aid,
                    "watchId": aid,
                    "attempts": watch.fix_dispatches_used,
                    "dispatched": dispatched,
                    "escalated": escalated,
                    "stopped": !watch.active,
                    "action": action_name,
                    "watch": watch,
                }));
            }
            // Non-watch-pr general automation event evaluation.
            let autos = bots.automation_engine().list_automations(None)?;
            let auto = autos
                .into_iter()
                .find(|a| a.id == aid)
                .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("automation '{aid}'")))?;
            if !auto.enabled {
                return Ok(json!({
                    "automationId": aid,
                    "dispatched": false,
                    "skipped": true,
                    "reason": "disabled",
                }));
            }
            Ok(json!({
                "automationId": aid,
                "dispatched": true,
                "kind": kind,
                "localOnly": auto.has_local_only_triggers(),
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown automations RPC method 'automations.{other}'"
        ))),
    }
}

async fn handle_bots_connectors(
    bots: &Arc<OrcaBotsFacade>,
    sub: &str,
    params: &Value,
) -> Result<Value> {
    match sub {
        "externalAction" | "external_action" => {
            let bot_id = req_str_any(params, &["botId", "bot_id", "id"])?;
            let action = req_str_any(params, &["action", "kind"])?;
            let text = opt_str_any(params, &["text", "body"]).unwrap_or("");
            let bot = bots.controller().get_bot(bot_id).await?;

            let acts_for = params.get("actsFor").or_else(|| params.get("acts_for"));
            let is_owner = acts_for
                .and_then(|v| v.get("isOwner").or_else(|| v.get("is_owner")))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let askers: Vec<ActsForAsker> = if is_owner {
                vec![ActsForAsker::Owner]
            } else if let Some(af) = acts_for {
                let source = af
                    .get("source")
                    .or_else(|| af.get("connector"))
                    .and_then(Value::as_str)
                    .and_then(|s| match s.trim().to_ascii_lowercase().as_str() {
                        "github" => Some(ConnectorSource::Github),
                        "linear" => Some(ConnectorSource::Linear),
                        "slack" => Some(ConnectorSource::Slack),
                        _ => None,
                    })
                    .unwrap_or(ConnectorSource::Slack);
                let user_id = af
                    .get("id")
                    .or_else(|| af.get("userId"))
                    .or_else(|| af.get("user_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("external")
                    .to_string();
                let name = af
                    .get("name")
                    .or_else(|| af.get("displayName"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                vec![ActsForAsker::Person {
                    source,
                    user_id,
                    name,
                    workspace_id: None,
                    role: Some(ConnectorAskerRole::Member),
                    place_kind: None,
                }]
            } else {
                vec![ActsForAsker::Unknown]
            };

            let action_norm = action.trim().to_ascii_lowercase();
            let owner_only = matches!(
                action_norm.as_str(),
                "merge_pr" | "approve_pr" | "merge" | "approve" | "deploy" | "delete"
            );
            if owner_only {
                require_owner_asker(&askers, action)?;
            }

            let decorated = decorate_external_action_body(text, &askers, bot.name.as_str(), is_owner);
            Ok(json!({
                "botId": bot.id,
                "action": action,
                "text": decorated,
                "attribution": decorated,
                "isOwner": is_owner,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown bots.connectors RPC method 'bots.connectors.{other}'"
        ))),
    }
}
