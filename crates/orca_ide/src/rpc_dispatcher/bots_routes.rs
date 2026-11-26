//! Unified RPC routing for `bots.*`/`bot.*`, `groups.*`/`group.*`,
//! `webhooks.*`/`webhook.*`, and `automations.*`/`automation.*` bridged directly
//! to `Option<Arc<orca_bots::OrcaBotsFacade>>`.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use orca_bots::{
    BotPatch, BotTemplateInstaller, CreateAutomationRequest, CreateBotGroupRequest,
    CreateBotItemRequest, CreateBotRequest, CreateWebhookRequest, ExportPackageOptions,
    GroupMessageSender, IngestWebhookCallRequest, ListBotItemsFilter, OrcaBotsFacade,
    PostGroupMessageRequest,
};
use orca_core::BotErrorCode;
use orca_core::bot_ecosystem::{BotChatNode, NativeChatBotCard, UsageLimitContinue};
use orca_core::bot_groups::{BotGroupReplyMode, BotGroupSend, MentionCandidate};
use orca_core::bot_items::{BotItemActor, BotItemStatus};
use orca_core::bot_memory::BotMemoryAuthor;
use orca_core::bot_types::{BotAgentId, BotPermissionMode};
use orca_core::triggers_webhooks::{
    AutomationBotResultsTo, AutomationBotRunsIn, AutomationBotTarget, AutomationEventTrigger,
    OneShotWait, WebhookAdapterId, WebhookFilters,
};
use serde_json::{Value, json};

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

pub(super) async fn dispatch_bots_namespace(
    bots_opt: Option<&Arc<OrcaBotsFacade>>,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let bots = require_bots(bots_opt)?;
    let now_ms = params.get("now_ms").and_then(Value::as_i64).unwrap_or_else(now_epoch_ms);

    match op {
        "list" => {
            let list = bots.controller().list_bots().await;
            Ok(serde_json::to_value(list)?)
        }
        "get" => {
            let id = req_str(params, "bot_id").or_else(|_| req_str(params, "id"))?;
            let bot = bots.controller().get_bot(id).await?;
            Ok(serde_json::to_value(bot)?)
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
                .cloned()
                .map(serde_json::from_value::<BotPermissionMode>)
                .transpose()?;
            let req = CreateBotRequest {
                name: opt_str(params, "name").map(ToOwned::to_owned),
                agent,
                custom_soul: opt_str(params, "custom_soul").map(ToOwned::to_owned),
                permission_mode,
                folder_workspace_id: opt_str(params, "folder_workspace_id").map(ToOwned::to_owned),
                first_run: params.get("first_run").and_then(Value::as_bool).unwrap_or(false),
                ..CreateBotRequest::default()
            };
            let bot = bots.create_bot(req).await?;
            Ok(serde_json::to_value(bot)?)
        }
        "update" => {
            let id = req_str(params, "bot_id").or_else(|_| req_str(params, "id"))?;
            let agent = match opt_str(params, "agent") {
                Some(raw) => Some(BotAgentId::parse_strict(raw).ok_or_else(|| {
                    OrcaIdeRuntimeError::invalid_argument(format!("unknown bot agent '{raw}'"))
                })?),
                None => None,
            };
            let patch = BotPatch {
                name: opt_str(params, "name").map(ToOwned::to_owned),
                agent,
                paused: params.get("paused").and_then(Value::as_bool),
                first_run: params.get("first_run").and_then(Value::as_bool),
                ..BotPatch::default()
            };
            let updated = bots.controller().update_bot(id, patch).await?;
            Ok(serde_json::to_value(updated)?)
        }
        "delete" => {
            let id = req_str(params, "bot_id").or_else(|_| req_str(params, "id"))?;
            let release = bots.delete_bot(id).await?;
            Ok(json!({
                "deleted": true,
                "bot_id": id,
                "released_folder": release.folder.as_str(),
            }))
        }
        "templates" => {
            let templates: Vec<Value> = BotTemplateInstaller::list_templates()
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "name": t.name,
                        "category": t.category,
                        "default_agent": t.default_agent,
                    })
                })
                .collect();
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
        _ => dispatch_bots_namespace_part2(bots, op, params, now_ms).await,
    }
}

async fn dispatch_bots_namespace_part2(
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
        "chat_tree" => {
            let bot_id = req_str(params, "bot_id")?;
            let nodes: Vec<BotChatNode> = params
                .get("nodes")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let usage_limit: Option<UsageLimitContinue> = params
                .get("usage_limit")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?;
            let cards: Vec<NativeChatBotCard> = params
                .get("cards")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let view = bots.build_chat_tree(bot_id, &nodes, usage_limit, cards, now_ms)?;
            Ok(serde_json::to_value(view)?)
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
            let call_id = req_str(params, "call_id")?;
            let call = bots.webhook_engine().get_call(call_id)?;
            Ok(serde_json::to_value(call)?)
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
        "watch_pr" => {
            let bot_id = req_str(params, "bot_id")?;
            let session_id = opt_str(params, "session_id").unwrap_or("home");
            let pr = req_str(params, "pr")?;
            let duration = opt_str(params, "duration");
            let ci_supported = params
                .get("ci_finished_supported")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let watch = bots.automation_engine().watch_pr().start_watch(
                bot_id,
                session_id,
                pr,
                now_ms.max(0) as u64,
                duration,
                ci_supported,
            )?;
            Ok(serde_json::to_value(watch)?)
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown automations RPC method 'automations.{other}'"
        ))),
    }
}
