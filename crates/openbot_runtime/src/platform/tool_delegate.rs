//! Unified M4 platform tool delegate (`PlatformToolDelegate`) implementing
//! [`ExternalToolDelegate`] for [`OpenBotToolRouter`].
//!
//! Routes all M4 platform tools (`crm_*`, `*_site`, `html_*`, `set_backdrop`,
//! `list_media_models`, `generate_image`, `generate_video`, `*_routine*`,
//! `*_workflow*`, `suggest_marketplace_app` / `marketplace_*`, and `messaging_*`)
//! cleanly into [`PlatformServices`] and `openbot_storage` with full validation.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use openbot_contracts::routines::{
    EventFilter, RoutineLimitPolicy, RoutineMissedPolicy, RoutineRunKind, RoutineRunPlace,
    RoutineRunSource, RoutineSchedule,
};
use openbot_contracts::workflows::{
    WorkflowBranch, WorkflowEdge, WorkflowFieldValue, WorkflowGraph, WorkflowNode,
    WorkflowNodeKind, WorkflowPoint, WorkflowPortType,
};
use openbot_storage::{
    CreateRoutineFlowLinkInput, MemoryStore, MessagingContextEntry, MessagingHistoryFilter,
    MessagingHistoryStore, MessagingPlatform, MessagingStore, OpenBotDatabase, WorkflowRunStore,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{OpenBotRuntimeError, RuntimeResult};
use crate::platform::{
    BridgeEvalRequest, HtmlRenderRequest, MarketplaceSearchRequest, MarketplaceTab,
    PlatformServices, RoutineRunRequest, RoutineSaveRequest, StaticSiteDeleteRequest,
    StaticSitePublishRequest, WorkflowRunRequest, WorkflowSaveRequest,
};
use crate::tool_router::{ExternalToolDelegate, ToolCallContext};

/// Connects M4 [`PlatformServices`] into [`crate::OpenBotToolRouter`] via [`ExternalToolDelegate`].
pub struct PlatformToolDelegate {
    database: OpenBotDatabase,
    platform: Arc<PlatformServices>,
}

impl PlatformToolDelegate {
    pub fn new(database: OpenBotDatabase, platform: Arc<PlatformServices>) -> Arc<Self> {
        Arc::new(Self { database, platform })
    }

    pub fn platform(&self) -> &Arc<PlatformServices> {
        &self.platform
    }
}

impl ExternalToolDelegate for PlatformToolDelegate {
    fn handle_tool(
        &self,
        ctx: &ToolCallContext,
        qualified_tool: &str,
        args: &Value,
    ) -> RuntimeResult<Option<Value>> {
        let short = qualified_tool
            .strip_prefix("openbot.")
            .unwrap_or(qualified_tool);
        match short {
            // --- CRM (OB-F12) ---
            "crm_find" | "crm_customer" | "crm_payments" | "crm_propose" => {
                let out = self
                    .platform
                    .crm
                    .execute_agent_tool(&ctx.agent_id, short, args)?;
                Ok(Some(out))
            }

            // --- Static Sites (OB-F12) ---
            "list_sites" | "publish_site" | "replace_site" | "delete_site" => {
                Ok(Some(self.exec_site_tool(short, args)?))
            }

            // --- HTML Widgets, Backdrops & Credit Media (OB-F12) ---
            "html_preview" | "html_render" | "set_backdrop" | "list_media_models"
            | "generate_image" | "generate_video" => {
                Ok(Some(self.exec_html_media_tool(short, args)?))
            }

            // --- Routines & Routine Flows (OB-F11) ---
            "list_routines"
            | "create_routine"
            | "create_webhook_routine"
            | "update_routine"
            | "update_webhook_routine"
            | "delete_routine"
            | "test_routine"
            | "list_routine_flows"
            | "connect_routine_agents"
            | "disconnect_routine_agents" => Ok(Some(self.exec_routine_tool(ctx, short, args)?)),

            // --- Workflows (OB-F11) ---
            "list_workflows"
            | "workflow_list"
            | "read_workflow"
            | "workflow_get"
            | "edit_workflow"
            | "workflow_save"
            | "delete_workflow"
            | "workflow_delete"
            | "run_workflow"
            | "workflow_run"
            | "read_workflow_runs"
            | "workflow_answer" => Ok(Some(self.exec_workflow_tool(ctx, short, args)?)),

            // --- Marketplace (OB-F12) ---
            "suggest_marketplace_app" | "marketplace_search" | "marketplace_install" => {
                Ok(Some(self.exec_marketplace_tool(short, args)?))
            }

            // --- External Messaging Bridges (OB-F13) ---
            "messaging_remember"
            | "messaging_forget_memory"
            | "messaging_history"
            | "messaging_list_places"
            | "messaging_post" => Ok(Some(self.exec_messaging_tool(ctx, short, args)?)),

            _ => Ok(None),
        }
    }
}

impl PlatformToolDelegate {
    fn exec_site_tool(&self, short: &str, args: &Value) -> RuntimeResult<Value> {
        match short {
            "list_sites" => {
                let sites = self.platform.static_sites.list_sites()?;
                let used = sites.iter().filter(|s| !s.deleted && s.status == "active").count();
                Ok(json!({
                    "sites": sites,
                    "used": used,
                    "count": sites.len(),
                }))
            }
            "publish_site" => {
                let source_path = req_str(args, &["sourcePath", "sourceDir", "source_path"], "sourcePath")?;
                let title = opt_str(args, &["title"]);
                let description = opt_str(args, &["description"]);
                let slug = opt_str(args, &["slug"]).unwrap_or_else(|| {
                    slugify_site_title(title.as_deref().unwrap_or("site"))
                });
                let spa_fallback = args
                    .get("spaFallback")
                    .or_else(|| args.get("spa_fallback"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let allowed_roots = args
                    .get("allowedRoots")
                    .or_else(|| args.get("allowed_roots"))
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(PathBuf::from)
                            .collect::<Vec<_>>()
                    });
                let rec = self.platform.static_sites.publish_site(StaticSitePublishRequest {
                    slug,
                    source_dir: PathBuf::from(source_path),
                    title,
                    description,
                    replace: false,
                    spa_fallback,
                    allowed_roots,
                })?;
                Ok(serde_json::to_value(rec)?)
            }
            "replace_site" => {
                let source_path = req_str(args, &["sourcePath", "sourceDir", "source_path"], "sourcePath")?;
                let site_id = opt_str(args, &["siteId", "site_id"]);
                let existing_slug = if let Some(ref sid) = site_id {
                    self.platform
                        .static_sites
                        .list_sites()?
                        .into_iter()
                        .find(|s| !s.deleted && s.status == "active" && s.site_id == *sid)
                        .map(|s| s.slug)
                } else {
                    None
                };
                let slug = opt_str(args, &["slug"])
                    .or(existing_slug)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::NotFound(
                            "replace_site requires an existing siteId or slug.".to_string(),
                        )
                    })?;
                let title = opt_str(args, &["title"]);
                let description = opt_str(args, &["description"]);
                let spa_fallback = args
                    .get("spaFallback")
                    .or_else(|| args.get("spa_fallback"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let rec = self.platform.static_sites.publish_site(StaticSitePublishRequest {
                    slug,
                    source_dir: PathBuf::from(source_path),
                    title,
                    description,
                    replace: true,
                    spa_fallback,
                    allowed_roots: None,
                })?;
                Ok(serde_json::to_value(rec)?)
            }
            "delete_site" => {
                let outcome = self.platform.static_sites.delete_site(StaticSiteDeleteRequest {
                    slug: opt_str(args, &["slug"]),
                    site_id: opt_str(args, &["siteId", "site_id"]),
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported static site tool '{short}'."
            ))),
        }
    }

    fn exec_html_media_tool(&self, short: &str, args: &Value) -> RuntimeResult<Value> {
        match short {
            "list_media_models" => {
                let kind = opt_str(args, &["kind"]);
                self.platform.html_media.list_media_models(kind.as_deref())
            }
            "html_preview" => {
                let html = req_str(args, &["html"], "html")?;
                let preview_width = args
                    .get("width")
                    .or_else(|| args.get("previewWidth"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32);
                let appearance = opt_str(args, &["appearance"]);
                let out = self.platform.html_media.render_or_generate(HtmlRenderRequest {
                    mode: Some("preview".to_string()),
                    html: Some(html),
                    preview_width,
                    appearance,
                    ..HtmlRenderRequest::default()
                })?;
                Ok(serde_json::to_value(out)?)
            }
            "html_render" => {
                let html = req_str(args, &["html"], "html")?;
                let title = opt_str(args, &["title"]);
                let frame_height = args
                    .get("height")
                    .or_else(|| args.get("frameHeight"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32);
                let appearance = opt_str(args, &["appearance"]);
                let rpc_message = args
                    .get("rpcMessage")
                    .or_else(|| args.get("rpc_message"))
                    .cloned();
                let out = self.platform.html_media.render_or_generate(HtmlRenderRequest {
                    mode: Some("page".to_string()),
                    html: Some(html),
                    title,
                    frame_height,
                    appearance,
                    rpc_message,
                    ..HtmlRenderRequest::default()
                })?;
                Ok(serde_json::to_value(out)?)
            }
            "set_backdrop" => {
                let html = opt_str(args, &["html"]);
                let title = opt_str(args, &["title"]);
                let backdrop = opt_str(args, &["backdrop", "scene"]);
                let bubbles_obj = args.get("bubbles").and_then(Value::as_object);
                let bubbles_yours = opt_str(args, &["bubblesYours", "bubbles_yours"]).or_else(|| {
                    bubbles_obj
                        .and_then(|b| b.get("yours"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
                let bubbles_agents =
                    opt_str(args, &["bubblesAgents", "bubbles_agents"]).or_else(|| {
                        bubbles_obj
                            .and_then(|b| b.get("agents"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    });
                let appearance = opt_str(args, &["appearance"]);
                let out = self.platform.html_media.render_or_generate(HtmlRenderRequest {
                    mode: Some("backdrop".to_string()),
                    html,
                    title,
                    appearance,
                    bubbles_yours,
                    bubbles_agents,
                    backdrop,
                    ..HtmlRenderRequest::default()
                })?;
                Ok(serde_json::to_value(out)?)
            }
            "generate_image" | "generate_video" => {
                let kind = if short == "generate_video" {
                    "video"
                } else {
                    "image"
                };
                let prompt = req_str(args, &["prompt", "mediaPrompt"], "prompt")?;
                let out = self.platform.html_media.render_or_generate(HtmlRenderRequest {
                    mode: Some("page".to_string()),
                    generate_media: true,
                    media_kind: Some(kind.to_string()),
                    media_model: opt_str(args, &["model", "mediaModel"]),
                    media_prompt: Some(prompt),
                    cost_credits: args
                        .get("costCredits")
                        .or_else(|| args.get("cost_credits"))
                        .and_then(Value::as_i64),
                    wallet_balance: args
                        .get("walletBalance")
                        .or_else(|| args.get("wallet_balance"))
                        .and_then(Value::as_i64),
                    approval_threshold: args
                        .get("approvalThreshold")
                        .or_else(|| args.get("approval_threshold"))
                        .and_then(Value::as_i64),
                    agents_credits_allowed: args
                        .get("agentsCreditsAllowed")
                        .or_else(|| args.get("agents_credits_allowed"))
                        .and_then(Value::as_bool),
                    approve_media: args
                        .get("approve")
                        .or_else(|| args.get("approveMedia"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    decline_media: args
                        .get("decline")
                        .or_else(|| args.get("declineMedia"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    ..HtmlRenderRequest::default()
                })?;
                Ok(serde_json::to_value(out)?)
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported HTML/media tool '{short}'."
            ))),
        }
    }

    fn exec_routine_tool(
        &self,
        ctx: &ToolCallContext,
        short: &str,
        args: &Value,
    ) -> RuntimeResult<Value> {
        let owner_id = opt_str(args, &["agentId", "agent_id"]).unwrap_or_else(|| ctx.agent_id.clone());
        match short {
            "list_routines" => {
                let mut routines = self.platform.routines.list_routines(&owner_id, false)?;
                if let Some(rid) = opt_str(args, &["routineId", "routine_id"]) {
                    routines.retain(|r| r.id == rid);
                }
                Ok(json!({ "routines": routines, "count": routines.len() }))
            }
            "create_routine" => {
                let name = req_str(args, &["name"], "name")?;
                let instruction = req_str(args, &["instruction"], "instruction")?;
                let raw_schedule = args.get("schedule").cloned().ok_or_else(|| {
                    OpenBotRuntimeError::Validation("create_routine requires 'schedule'.".to_string())
                })?;
                let schedule: RoutineSchedule = serde_json::from_value(raw_schedule).map_err(|e| {
                    OpenBotRuntimeError::Validation(format!("Invalid routine schedule: {e}"))
                })?;
                let outcome = self.platform.routines.save_routine(RoutineSaveRequest {
                    owner_id,
                    routine_id: opt_str(args, &["routineId", "routine_id"]),
                    is_channel: false,
                    name,
                    instruction,
                    timezone: opt_str(args, &["timezone"]).unwrap_or_else(|| "UTC".to_string()),
                    active: args.get("active").and_then(Value::as_bool).unwrap_or(true),
                    schedule: Some(schedule),
                    webhook_event_type: None,
                    webhook_filters: Vec::new(),
                    webhook_secret: None,
                    limit_policy: parse_limit_policy(args).unwrap_or(RoutineLimitPolicy::Wait),
                    missed_policy: parse_missed_policy(args).unwrap_or(RoutineMissedPolicy::Skip),
                    run_in: parse_run_place(args).unwrap_or(RoutineRunPlace::Chat),
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "create_webhook_routine" => {
                if ctx.origin_routine_id.is_some() {
                    return Err(OpenBotRuntimeError::Forbidden(
                        "Webhook routines can only be created in person-started turns.".to_string(),
                    ));
                }
                let name = req_str(args, &["name"], "name")?;
                let instruction = req_str(args, &["instruction"], "instruction")?;
                let webhook_filters = parse_event_filters(args)?;
                let outcome = self.platform.routines.save_routine(RoutineSaveRequest {
                    owner_id,
                    routine_id: opt_str(args, &["routineId", "routine_id"]),
                    is_channel: false,
                    name,
                    instruction,
                    timezone: opt_str(args, &["timezone"]).unwrap_or_else(|| "UTC".to_string()),
                    active: args.get("active").and_then(Value::as_bool).unwrap_or(true),
                    schedule: None,
                    webhook_event_type: opt_str(args, &["eventType", "event_type"]),
                    webhook_filters,
                    webhook_secret: opt_str(args, &["webhookSecret", "webhook_secret"]),
                    limit_policy: parse_limit_policy(args).unwrap_or(RoutineLimitPolicy::Wait),
                    missed_policy: RoutineMissedPolicy::Skip,
                    run_in: parse_run_place(args).unwrap_or(RoutineRunPlace::Chat),
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "update_routine" | "update_webhook_routine" => {
                if short == "update_webhook_routine" && ctx.origin_routine_id.is_some() {
                    return Err(OpenBotRuntimeError::Forbidden(
                        "Webhook routines can only be updated in person-started turns.".to_string(),
                    ));
                }
                let routine_id = req_str(args, &["routineId", "routine_id"], "routineId")?;
                let existing = self
                    .platform
                    .routines
                    .list_routines(&owner_id, false)?
                    .into_iter()
                    .find(|r| r.id == routine_id)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::NotFound(format!("Routine '{routine_id}' not found."))
                    })?;
                let schedule = if short == "update_webhook_routine" {
                    None
                } else if let Some(raw_sched) = args.get("schedule") {
                    Some(serde_json::from_value::<RoutineSchedule>(raw_sched.clone()).map_err(|e| {
                        OpenBotRuntimeError::Validation(format!("Invalid routine schedule: {e}"))
                    })?)
                } else {
                    Some(existing.trigger.schedule.clone())
                };
                let outcome = self.platform.routines.save_routine(RoutineSaveRequest {
                    owner_id,
                    routine_id: Some(routine_id),
                    is_channel: false,
                    name: opt_str(args, &["name"]).unwrap_or(existing.name),
                    instruction: opt_str(args, &["instruction"]).unwrap_or(existing.instruction),
                    timezone: opt_str(args, &["timezone"]).unwrap_or(existing.timezone),
                    active: args
                        .get("active")
                        .and_then(Value::as_bool)
                        .unwrap_or(existing.active),
                    schedule,
                    webhook_event_type: opt_str(args, &["eventType", "event_type"]),
                    webhook_filters: parse_event_filters(args)?,
                    webhook_secret: None,
                    limit_policy: parse_limit_policy(args)
                        .or(existing.limit_policy)
                        .unwrap_or(RoutineLimitPolicy::Wait),
                    missed_policy: parse_missed_policy(args)
                        .or(existing.missed_policy)
                        .unwrap_or(RoutineMissedPolicy::Skip),
                    run_in: parse_run_place(args)
                        .or(existing.run_in)
                        .unwrap_or(RoutineRunPlace::Chat),
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "delete_routine" => {
                let routine_id = req_str(args, &["routineId", "routine_id"], "routineId")?;
                let deleted = self
                    .platform
                    .routines
                    .delete_routine(&owner_id, &routine_id, false)?;
                Ok(json!({ "deleted": deleted, "routineId": routine_id }))
            }
            "test_routine" => {
                let routine_id = req_str(args, &["routineId", "routine_id"], "routineId")?;
                let outcome = self.platform.routines.run_routine(RoutineRunRequest {
                    owner_id,
                    routine_id,
                    is_channel: false,
                    kind: RoutineRunKind::Manual,
                    source: Some(RoutineRunSource::Test),
                    payload: opt_str(args, &["payload"]),
                    wait_id: opt_str(args, &["waitId", "wait_id"]),
                    quiet_no_update: false,
                    report_text: None,
                    await_events: Vec::new(),
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "list_routine_flows" => {
                let routines = self.platform.routines.list_routines(&owner_id, false)?;
                let ids: Vec<String> = routines.iter().map(|r| r.id.clone()).collect();
                let links = self.platform.routines.flows().links_for_routines(&ids)?;
                Ok(json!({
                    "agentId": owner_id,
                    "routines": routines,
                    "links": links,
                }))
            }
            "connect_routine_agents" => {
                let routine_id = req_str(args, &["routineId", "routine_id"], "routineId")?;
                let from_agent_id =
                    req_str(args, &["fromAgentId", "from_agent_id"], "fromAgentId")?;
                let to_agent_id = req_str(args, &["toAgentId", "to_agent_id"], "toAgentId")?;
                let instruction = opt_str(args, &["instruction"]);
                let routine_owner = self
                    .platform
                    .routines
                    .flows()
                    .routine_owner(&routine_id)?
                    .unwrap_or_else(|| owner_id.clone());
                let link = self.platform.routines.flows().create_link(
                    &routine_owner,
                    &CreateRoutineFlowLinkInput {
                        routine_id,
                        from_agent_id,
                        to_agent_id,
                        instruction,
                    },
                    None,
                )?;
                Ok(serde_json::to_value(link)?)
            }
            "disconnect_routine_agents" => {
                let link_id = req_str(args, &["linkId", "link_id"], "linkId")?;
                let flows = self.platform.routines.flows();
                let removed = flows.delete_link(&link_id, |rid| {
                    flows.routine_owner(rid).ok().flatten()
                })?;
                Ok(json!({ "removed": removed, "count": removed.len() }))
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported routine tool '{short}'."
            ))),
        }
    }

    fn exec_workflow_tool(
        &self,
        ctx: &ToolCallContext,
        short: &str,
        args: &Value,
    ) -> RuntimeResult<Value> {
        match short {
            "list_workflows" | "workflow_list" => {
                let workflows = self.platform.workflows.list_workflows()?;
                Ok(json!({ "workflows": workflows, "count": workflows.len() }))
            }
            "read_workflow" | "workflow_get" => {
                let workflow_id = req_str(args, &["workflowId", "workflow_id", "id"], "workflowId")?;
                let detail = self
                    .platform
                    .workflows
                    .get_workflow(&workflow_id)?
                    .ok_or_else(|| {
                        OpenBotRuntimeError::NotFound(format!(
                            "Workflow '{workflow_id}' not found."
                        ))
                    })?;
                Ok(serde_json::to_value(detail)?)
            }
            "edit_workflow" | "workflow_save" => {
                if ctx.origin_routine_id.is_some() {
                    return Err(OpenBotRuntimeError::Forbidden(
                        "Workflow edits are only allowed in person-started turns.".to_string(),
                    ));
                }
                let workflow_id = opt_str(args, &["workflowId", "workflow_id"]);
                let existing = match workflow_id.as_deref() {
                    Some(id) => self.platform.workflows.get_workflow(id)?,
                    None => None,
                };
                let title = opt_str(args, &["title"])
                    .or_else(|| existing.as_ref().map(|w| w.summary.title.clone()))
                    .unwrap_or_else(|| "Untitled Workflow".to_string());
                let mut graph = if let Some(raw_graph) = args.get("graph") {
                    serde_json::from_value::<WorkflowGraph>(raw_graph.clone()).map_err(|e| {
                        OpenBotRuntimeError::Validation(format!("Invalid workflow graph: {e}"))
                    })?
                } else {
                    existing
                        .as_ref()
                        .map(|w| w.graph.clone())
                        .unwrap_or(WorkflowGraph {
                            nodes: Vec::new(),
                            edges: Vec::new(),
                            strokes: Vec::new(),
                        })
                };
                if let Some(ops) = args.get("ops").and_then(Value::as_array) {
                    apply_workflow_ops(&mut graph, ops)?;
                }
                let allow_loops = args
                    .get("allowLoops")
                    .or_else(|| args.get("allow_loops"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let base_revision = args
                    .get("baseRevision")
                    .or_else(|| args.get("base_revision"))
                    .and_then(Value::as_u64);
                let outcome = self.platform.workflows.save_workflow(WorkflowSaveRequest {
                    workflow_id,
                    title,
                    graph,
                    raw_json_bytes: None,
                    allow_loops,
                    base_revision,
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "delete_workflow" | "workflow_delete" => {
                let workflow_id = req_str(args, &["workflowId", "workflow_id", "id"], "workflowId")?;
                let deleted = self.platform.workflows.delete_workflow(&workflow_id)?;
                Ok(json!({ "deleted": deleted, "workflowId": workflow_id }))
            }
            "run_workflow" | "workflow_run" => {
                if ctx.origin_routine_id.is_some() {
                    return Err(OpenBotRuntimeError::Forbidden(
                        "Workflow execution is only allowed in person-started turns.".to_string(),
                    ));
                }
                let workflow_id = req_str(args, &["workflowId", "workflow_id"], "workflowId")?;
                let condition_mocks: BTreeMap<String, String> = args
                    .get("conditionMocks")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                let outcome = self.platform.workflows.run_workflow(WorkflowRunRequest {
                    workflow_id,
                    run_id: opt_str(args, &["runId", "run_id"]),
                    payload: opt_str(args, &["input", "payload"]),
                    approve_node: opt_str(args, &["approveNode", "approve_node"]),
                    reject_node: opt_str(args, &["rejectNode", "reject_node"]),
                    condition_mocks,
                    idempotency_key: opt_str(args, &["idempotencyKey", "idempotency_key"]),
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "read_workflow_runs" => {
                let workflow_id = req_str(args, &["workflowId", "workflow_id"], "workflowId")?;
                let runs_store = WorkflowRunStore::new(self.database.clone());
                if let Some(run_id) = opt_str(args, &["runId", "run_id"]) {
                    let run = runs_store.run(&run_id)?.ok_or_else(|| {
                        OpenBotRuntimeError::NotFound(format!("Workflow run '{run_id}' not found."))
                    })?;
                    let steps = runs_store.steps(&run_id)?;
                    return Ok(json!({
                        "workflowId": workflow_id,
                        "run": {
                            "id": run.id,
                            "workflowId": run.workflow_id,
                            "status": run.status,
                            "startedAt": run.started_at,
                            "endedAt": run.ended_at,
                            "error": run.error,
                            "steps": steps.into_iter().map(|s| json!({
                                "stepId": s.id,
                                "nodeId": s.node_id,
                                "kind": s.kind,
                                "status": s.status,
                                "branch": s.branch,
                                "output": s.output,
                                "error": s.error,
                            })).collect::<Vec<_>>(),
                        }
                    }));
                }
                let limit = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|v| (v as usize).clamp(1, 5))
                    .unwrap_or(5);
                let page = runs_store.page(&workflow_id, None, limit)?;
                Ok(serde_json::to_value(page)?)
            }
            "workflow_answer" => {
                let answer = args.get("answer").ok_or_else(|| {
                    OpenBotRuntimeError::Validation("workflow_answer requires 'answer'.".to_string())
                })?;
                let encoded = match answer {
                    Value::String(s) if !s.trim().is_empty() => s.trim().to_string(),
                    Value::Object(map) if !map.is_empty() => serde_json::to_string(map)?,
                    _ => {
                        return Err(OpenBotRuntimeError::Validation(
                            "workflow_answer 'answer' must be a non-empty JSON object or string."
                                .to_string(),
                        ));
                    }
                };
                if let Some(step_id) = opt_str(args, &["stepId", "step_id"]) {
                    let delivery_id = opt_str(args, &["deliveryId", "delivery_id"])
                        .or_else(|| ctx.call_id.clone())
                        .unwrap_or_default();
                    let runs_store = WorkflowRunStore::new(self.database.clone());
                    let _ = runs_store.keep_agent_answer(&step_id, &delivery_id, &encoded, None)?;
                }
                Ok(json!({ "accepted": true, "answer": answer }))
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported workflow tool '{short}'."
            ))),
        }
    }

    fn exec_marketplace_tool(&self, short: &str, args: &Value) -> RuntimeResult<Value> {
        match short {
            "suggest_marketplace_app" | "marketplace_search" => {
                let tab = opt_str(args, &["tab"]).unwrap_or_else(|| "apps".to_string());
                let out = self.platform.marketplace.search(MarketplaceSearchRequest {
                    tab,
                    source: opt_str(args, &["source"]),
                    query: opt_str(args, &["query", "q", "name"]),
                    category: opt_str(args, &["category"]),
                    reduced_motion: args
                        .get("reducedMotion")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    alpha_unsupported: args
                        .get("alphaUnsupported")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })?;
                Ok(serde_json::to_value(out)?)
            }
            "marketplace_install" => {
                let tab = match opt_str(args, &["tab"]).as_deref() {
                    Some("agents") => MarketplaceTab::Agents,
                    Some("skills") => MarketplaceTab::Skills,
                    Some("apps") | None => MarketplaceTab::Apps,
                    Some(other) => {
                        return Err(OpenBotRuntimeError::Validation(format!(
                            "Invalid marketplace tab '{other}'; expected apps, agents, or skills."
                        )));
                    }
                };
                let id_or_slug = req_str(args, &["id", "slug", "itemId"], "id")?;
                let item = self.platform.marketplace.install_item(tab, &id_or_slug)?;
                Ok(serde_json::to_value(item)?)
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported marketplace tool '{short}'."
            ))),
        }
    }

    fn exec_messaging_tool(
        &self,
        ctx: &ToolCallContext,
        short: &str,
        args: &Value,
    ) -> RuntimeResult<Value> {
        let store = MessagingStore::new(self.database.clone());
        match short {
            "messaging_list_places" => {
                let filter_conn = opt_str(args, &["connection", "connectionId"]);
                let mut connections = store.connections()?;
                if let Some(ref c) = filter_conn {
                    connections.retain(|item| item.connection_id == *c || item.workspace_id == *c);
                }
                let links = store.links(&ctx.agent_id)?;
                Ok(json!({
                    "connections": connections,
                    "links": links,
                }))
            }
            "messaging_remember" => {
                let text = req_str(args, &["text"], "text")?;
                let connection_id = opt_str(args, &["connection", "connectionId"])
                    .or_else(|| store.connections().ok().and_then(|v| v.first().map(|c| c.connection_id.clone())))
                    .ok_or_else(|| {
                        OpenBotRuntimeError::NotFound(
                            "No connected messaging workspace available for messaging_remember."
                                .to_string(),
                        )
                    })?;
                let memory_id = opt_str(args, &["memoryId", "memory_id"]);
                let turn_id = ctx
                    .turn_id
                    .clone()
                    .unwrap_or_else(|| format!("turn:{}", Uuid::new_v4()));
                let cmd_id = ctx
                    .call_id
                    .clone()
                    .unwrap_or_else(|| format!("msg-mem:{}", Uuid::new_v4()));
                let saved = MemoryStore::messaging(self.database.clone()).save_from_tool(
                    &connection_id,
                    &text,
                    memory_id.as_deref(),
                    &turn_id,
                    &cmd_id,
                )?;
                Ok(serde_json::to_value(saved)?)
            }
            "messaging_forget_memory" => {
                let memory_id = req_str(args, &["memoryId", "memory_id"], "memoryId")?;
                let connection_id = opt_str(args, &["connection", "connectionId"])
                    .or_else(|| store.connections().ok().and_then(|v| v.first().map(|c| c.connection_id.clone())))
                    .ok_or_else(|| {
                        OpenBotRuntimeError::NotFound(
                            "No connected messaging workspace available for messaging_forget_memory."
                                .to_string(),
                        )
                    })?;
                let deleted = MemoryStore::messaging(self.database.clone())
                    .delete(&connection_id, &memory_id)?;
                Ok(json!({ "deleted": deleted, "memoryId": memory_id }))
            }
            "messaging_history" => {
                let connection_id = opt_str(args, &["connection", "connectionId"])
                    .or_else(|| store.connections().ok().and_then(|v| v.first().map(|c| c.connection_id.clone())))
                    .unwrap_or_default();
                let place = opt_str(args, &["place", "channelId"]).unwrap_or_default();
                let limit = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|v| (v as usize).clamp(1, 100))
                    .unwrap_or(20);
                let history = MessagingHistoryStore::new(self.database.clone()).recent(
                    &connection_id,
                    &place,
                    None,
                    limit,
                    &MessagingHistoryFilter {
                        query: opt_str(args, &["query"]),
                        before: opt_str(args, &["before"]),
                    },
                )?;
                Ok(json!({ "messages": history, "count": history.len() }))
            }
            "messaging_post" => {
                let connection = req_str(args, &["connection", "connectionId"], "connection")?;
                let place = req_str(args, &["place", "channelId"], "place")?;
                let text = opt_str(args, &["text"]).unwrap_or_default();
                let conn_record = store
                    .connection(&connection)?
                    .or_else(|| {
                        store
                            .connections()
                            .ok()
                            .and_then(|v| v.into_iter().find(|c| c.workspace_id == connection))
                    });
                let platform = conn_record
                    .as_ref()
                    .map(|c| c.platform)
                    .or_else(|| MessagingPlatform::parse(&connection))
                    .unwrap_or(MessagingPlatform::Telegram);
                let dispatched_telegram = if platform == MessagingPlatform::Telegram {
                    self.platform
                        .bridges
                        .dispatch_telegram_outbound(&place, &text)?
                } else {
                    false
                };
                if let Some(ref rec) = conn_record {
                    let msg_id = format!("out-{}", Uuid::new_v4());
                    let _ = self.platform.bridges.record_history(
                        &rec.connection_id,
                        &place,
                        &place,
                        &ctx.agent_id,
                        &MessagingContextEntry {
                            id: msg_id,
                            author_name: ctx.agent_id.clone(),
                            text: text.clone(),
                            sent_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                            files: None,
                        },
                    )?;
                }
                let eval = self.platform.bridges.eval_bridge(BridgeEvalRequest {
                    platform: platform.as_str().to_string(),
                    workspace_id: Some(connection.clone()),
                    sender: Some(ctx.agent_id.clone()),
                    text: Some(text),
                    channel_id: Some(place.clone()),
                    ..BridgeEvalRequest::default()
                })?;
                Ok(json!({
                    "posted": true,
                    "connection": connection,
                    "place": place,
                    "dispatchedTelegram": dispatched_telegram,
                    "evaluation": eval,
                }))
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported messaging tool '{short}'."
            ))),
        }
    }
}

fn apply_workflow_ops(graph: &mut WorkflowGraph, ops: &[Value]) -> RuntimeResult<()> {
    let mut ref_to_id: BTreeMap<String, String> = BTreeMap::new();
    for op_val in ops {
        let obj = op_val.as_object().ok_or_else(|| {
            OpenBotRuntimeError::Validation("Each workflow op must be a JSON object.".to_string())
        })?;
        let op = obj
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| OpenBotRuntimeError::Validation("Workflow op missing 'op'.".to_string()))?;
        match op {
            "add" => {
                let kind_val = obj.get("kind").cloned().ok_or_else(|| {
                    OpenBotRuntimeError::Validation("Workflow 'add' op requires 'kind'.".to_string())
                })?;
                let kind: WorkflowNodeKind = serde_json::from_value(kind_val).map_err(|e| {
                    OpenBotRuntimeError::Validation(format!("Invalid workflow node kind: {e}"))
                })?;
                let node_id = obj
                    .get("node")
                    .or_else(|| obj.get("id"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("node-{}", Uuid::new_v4().simple()));
                if let Some(r) = obj.get("ref").and_then(Value::as_str) {
                    ref_to_id.insert(r.to_string(), node_id.clone());
                }
                let idx = graph.nodes.len() as f64;
                let x = obj.get("x").and_then(Value::as_f64).unwrap_or(idx * 240.0);
                let y = obj.get("y").and_then(Value::as_f64).unwrap_or(0.0);
                let fields = parse_workflow_fields(obj.get("fields"))?;
                graph.nodes.push(WorkflowNode {
                    id: node_id,
                    kind,
                    at: WorkflowPoint { x, y },
                    size: None,
                    locked: None,
                    fields,
                    schedule: None,
                });
            }
            "update" => {
                let target = resolve_node_ref(obj.get("node"), &ref_to_id)?;
                let node = graph
                    .nodes
                    .iter_mut()
                    .find(|n| n.id == target)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::NotFound(format!("Workflow node '{target}' not found."))
                    })?;
                if let Some(fields_val) = obj.get("fields") {
                    for (k, v) in parse_workflow_fields(Some(fields_val))? {
                        node.fields.insert(k, v);
                    }
                }
            }
            "move" => {
                let target = resolve_node_ref(obj.get("node"), &ref_to_id)?;
                let node = graph
                    .nodes
                    .iter_mut()
                    .find(|n| n.id == target)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::NotFound(format!("Workflow node '{target}' not found."))
                    })?;
                if let Some(x) = obj.get("x").and_then(Value::as_f64) {
                    node.at.x = x;
                }
                if let Some(y) = obj.get("y").and_then(Value::as_f64) {
                    node.at.y = y;
                }
            }
            "remove" => {
                let target = resolve_node_ref(obj.get("node"), &ref_to_id)?;
                graph.nodes.retain(|n| n.id != target);
                graph.edges.retain(|e| e.from != target && e.to != target);
            }
            "connect" => {
                let from = resolve_node_ref(obj.get("from"), &ref_to_id)?;
                let to = resolve_node_ref(obj.get("to"), &ref_to_id)?;
                let branch = match obj.get("branch").and_then(Value::as_str) {
                    Some("yes") => Some(WorkflowBranch::Yes),
                    Some("no") => Some(WorkflowBranch::No),
                    Some(other) => {
                        return Err(OpenBotRuntimeError::Validation(format!(
                            "Invalid workflow branch '{other}'; expected 'yes' or 'no'."
                        )));
                    }
                    None => None,
                };
                let port_type = match obj.get("type").and_then(Value::as_str) {
                    Some("image") => WorkflowPortType::Image,
                    Some("file") => WorkflowPortType::File,
                    _ => WorkflowPortType::Text,
                };
                graph.edges.push(WorkflowEdge {
                    id: format!("edge-{}", Uuid::new_v4().simple()),
                    from,
                    to,
                    port_type,
                    branch,
                });
            }
            "disconnect" => {
                let from = resolve_node_ref(obj.get("from"), &ref_to_id)?;
                let to = resolve_node_ref(obj.get("to"), &ref_to_id)?;
                graph.edges.retain(|e| !(e.from == from && e.to == to));
            }
            other => {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Unsupported workflow op '{other}'."
                )));
            }
        }
    }
    Ok(())
}

fn resolve_node_ref(val: Option<&Value>, refs: &BTreeMap<String, String>) -> RuntimeResult<String> {
    let raw = val
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            OpenBotRuntimeError::Validation("Workflow op requires a node identifier.".to_string())
        })?;
    Ok(refs.get(raw).cloned().unwrap_or_else(|| raw.to_string()))
}

fn parse_workflow_fields(val: Option<&Value>) -> RuntimeResult<BTreeMap<String, WorkflowFieldValue>> {
    let Some(map) = val.and_then(Value::as_object) else {
        return Ok(BTreeMap::new());
    };
    let mut out = BTreeMap::new();
    for (k, v) in map {
        let parsed: WorkflowFieldValue = serde_json::from_value(v.clone()).map_err(|e| {
            OpenBotRuntimeError::Validation(format!("Invalid workflow field '{k}': {e}"))
        })?;
        out.insert(k.clone(), parsed);
    }
    Ok(out)
}

fn parse_limit_policy(args: &Value) -> Option<RoutineLimitPolicy> {
    match opt_str(args, &["limitPolicy", "limit_policy"]).as_deref() {
        Some("skip") => Some(RoutineLimitPolicy::Skip),
        Some("wait") => Some(RoutineLimitPolicy::Wait),
        _ => None,
    }
}

fn parse_missed_policy(args: &Value) -> Option<RoutineMissedPolicy> {
    match opt_str(args, &["missedPolicy", "missed_policy"]).as_deref() {
        Some("run-once") | Some("run_once") => Some(RoutineMissedPolicy::RunOnce),
        Some("skip") => Some(RoutineMissedPolicy::Skip),
        _ => None,
    }
}

fn parse_run_place(args: &Value) -> Option<RoutineRunPlace> {
    let raw = args.get("runIn").or_else(|| args.get("run_in"))?;
    if let Some(s) = raw.as_str() {
        return match s.trim().to_ascii_lowercase().as_str() {
            "thread" => Some(RoutineRunPlace::Thread),
            "chat" => Some(RoutineRunPlace::Chat),
            _ => None,
        };
    }
    if let Some(obj) = raw.as_object() {
        if let Some(kind) = obj.get("kind").or_else(|| obj.get("place")).and_then(Value::as_str) {
            return match kind.trim().to_ascii_lowercase().as_str() {
                "thread" => Some(RoutineRunPlace::Thread),
                "chat" => Some(RoutineRunPlace::Chat),
                _ => None,
            };
        }
    }
    None
}

fn parse_event_filters(args: &Value) -> RuntimeResult<Vec<EventFilter>> {
    let Some(arr) = args.get("filters").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let f: EventFilter = serde_json::from_value(item.clone()).map_err(|e| {
            OpenBotRuntimeError::Validation(format!("Invalid webhook event filter: {e}"))
        })?;
        out.push(f);
    }
    Ok(out)
}

fn slugify_site_title(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_dash = false;
        } else if !out.is_empty() && !last_dash {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 60 {
            break;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "site".to_string()
    } else {
        trimmed
    }
}

fn opt_str(args: &Value, keys: &[&str]) -> Option<String> {
    let obj = args.as_object()?;
    for key in keys {
        if let Some(s) = obj.get(*key).and_then(Value::as_str) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn req_str(args: &Value, keys: &[&str], label: &str) -> RuntimeResult<String> {
    opt_str(args, keys).ok_or_else(|| {
        OpenBotRuntimeError::Validation(format!("Missing required string argument '{label}'."))
    })
}
