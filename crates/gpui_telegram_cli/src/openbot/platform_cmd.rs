//! `openbot platform <action>` handler covering all 13 platform actions (`OB-F11`, `OB-F12`, `OB-F13`, `OB-F18`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use openbot_contracts::channels::SIGNED_OUT_CHANNEL_MEMBER_ID;
use openbot_contracts::limits::INPUT_LIMITS;
use openbot_contracts::routines::{
    EventFilter, RoutineIntervalUnit, RoutineLimitPolicy, RoutineMissedPolicy, RoutineRunKind,
    RoutineRunPlace, RoutineRunSource, RoutineSchedule,
};
use openbot_contracts::voice::{
    CODEX_LIVE_VOICES, SPEECH_MODEL_IDS, VOICE_MODEL_IDS, VOICE_TURN_PAUSES, VoiceServerRoutes,
};
use openbot_contracts::workflows::WorkflowGraph;
use openbot_runtime::{
    BridgeEvalRequest, CrmProposeRequest, CrmSyncRequest, HtmlRenderRequest,
    MarketplaceSearchRequest, OpenBotHostRuntime, RoutineRunRequest, RoutineSaveRequest,
    StaticSiteDeleteRequest, StaticSitePublishRequest, StaticSiteService, VoiceEvalRequest,
    WebhookPostRequest, WorkflowRunRequest, WorkflowSaveRequest,
};
use openbot_storage::{
    MessagingContextEntry, MessagingLinkInput, MessagingPlatform, WebhookRouteOwner,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    FlagSpec, ParsedFlags, map_runtime_error, map_storage_error, open_host_runtime, parse_csv,
    parse_f64_domain, parse_i64_domain, parse_json_value, parse_u32_domain, parse_u64_domain,
    parse_usize_domain, read_inline_or_file,
};
use crate::error::CliError;

pub fn flag_spec(action: &str) -> Result<FlagSpec, CliError> {
    match action {
        "workflow-save" => Ok(FlagSpec {
            value_flags: &["--spec", "--spec-file", "--workflow-id", "--base-revision"],
            bool_flags: &["--allow-loops"],
            opt_bool_flags: &[],
            required_flags: &[],
        }),
        "workflow-run" => Ok(FlagSpec {
            value_flags: &[
                "--workflow-id",
                "--run-id",
                "--trigger-payload",
                "--condition-mock",
                "--approve-node",
                "--reject-node",
                "--idempotency-key",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--workflow-id"],
        }),
        "routine-save" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--channel-id",
                "--routine-id",
                "--name",
                "--instruction",
                "--schedule",
                "--timezone",
                "--quiet-policy",
                "--webhook-secret",
                "--event-type",
                "--filters",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--name", "--schedule"],
        }),
        "routine-run" => Ok(FlagSpec {
            value_flags: &[
                "--routine-id",
                "--agent-id",
                "--channel-id",
                "--source",
                "--report-text",
                "--await-events",
                "--payload",
                "--wait-id",
            ],
            bool_flags: &["--quiet-no-update"],
            opt_bool_flags: &[],
            required_flags: &["--routine-id"],
        }),
        "webhook-post" => Ok(FlagSpec {
            value_flags: &[
                "--routine-id",
                "--payload",
                "--signature",
                "--timestamp",
                "--delivery-id",
                "--event-type",
                "--secret",
                "--agent-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--routine-id", "--payload", "--signature"],
        }),
        "crm-sync" => Ok(FlagSpec {
            value_flags: &[
                "--source",
                "--api-key",
                "--fixture",
                "--private-fields",
                "--for-agent-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--source"],
        }),
        "crm-propose" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--operation",
                "--payload",
                "--reason",
                "--proposal-id",
                "--direct-change",
            ],
            bool_flags: &["--approve", "--reject", "--commit-unapproved"],
            opt_bool_flags: &[],
            required_flags: &[],
        }),
        "site-publish" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--slug",
                "--site-id",
                "--title",
                "--description",
                "--source-dir",
                "--files",
            ],
            bool_flags: &["--replace", "--spa-fallback"],
            opt_bool_flags: &[],
            required_flags: &["--slug"],
        }),
        "site-delete" => Ok(FlagSpec {
            value_flags: &["--slug", "--site-id", "--agent-id"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &[],
        }),
        "marketplace-search" => Ok(FlagSpec {
            value_flags: &["--tab", "--query", "--source", "--category"],
            bool_flags: &["--reduced-motion", "--alpha-unsupported"],
            opt_bool_flags: &[],
            required_flags: &[],
        }),
        "html-render" => Ok(FlagSpec {
            value_flags: &[
                "--mode",
                "--html",
                "--html-file",
                "--title",
                "--width",
                "--height",
                "--appearance",
                "--backdrop",
                "--bubbles",
                "--rpc-message",
                "--generate-media",
                "--media-model",
                "--media-prompt",
                "--media-cost",
                "--wallet-balance",
                "--approval-threshold",
            ],
            bool_flags: &["--approve-media", "--decline-media"],
            opt_bool_flags: &["--agents-credits-allowed"],
            required_flags: &[],
        }),
        "voice-eval" => Ok(FlagSpec {
            value_flags: &[
                "--mode",
                "--stt-provider",
                "--tts-provider",
                "--tts-voice",
                "--tts-text",
                "--tts-speed",
                "--locale",
                "--audio-rate-hz",
                "--audio-seconds",
                "--audio-bytes",
                "--live-voice",
                "--turn-pause-ms",
                "--sdp",
                "--agent-id",
                "--channel-id",
                "--voice-server-json",
            ],
            bool_flags: &["--cycle-speed"],
            opt_bool_flags: &[],
            required_flags: &["--mode"],
        }),
        "bridge-sync" => Ok(FlagSpec {
            value_flags: &[
                "--platform",
                "--direction",
                "--agent-id",
                "--workspace-id",
                "--payload",
                "--allowed-roles",
                "--allowed-senders",
            ],
            bool_flags: &[],
            opt_bool_flags: &["--own-number"],
            required_flags: &["--platform", "--direction", "--payload"],
        }),
        other => Err(CliError::UnknownCommand(format!(
            "Unknown action '{other}' for 'openbot platform'"
        ))),
    }
}

pub fn run_platform(action: &str, flags: &ParsedFlags, data_dir: &Path) -> Result<Value, CliError> {
    let runtime = open_host_runtime(data_dir)?;
    match action {
        "workflow-save" => run_workflow_save(flags, &runtime),
        "workflow-run" => run_workflow_run(flags, &runtime),
        "routine-save" => run_routine_save(flags, &runtime),
        "routine-run" => run_routine_run(flags, &runtime),
        "webhook-post" => run_webhook_post(flags, &runtime),
        "crm-sync" => run_crm_sync(flags, &runtime),
        "crm-propose" => run_crm_propose(flags, &runtime),
        "site-publish" => run_site_publish(flags, &runtime, data_dir),
        "site-delete" => run_site_delete(flags, &runtime),
        "marketplace-search" => run_marketplace_search(flags, &runtime),
        "html-render" => run_html_render(flags, &runtime),
        "voice-eval" => run_voice_eval(flags, &runtime),
        "bridge-sync" => run_bridge_sync(flags, &runtime),
        other => Err(CliError::UnknownCommand(format!(
            "Unknown action '{other}' for 'openbot platform'"
        ))),
    }
}

fn run_workflow_save(flags: &ParsedFlags, runtime: &OpenBotHostRuntime) -> Result<Value, CliError> {
    let Some(raw) = read_inline_or_file(
        flags.get("--spec"),
        flags.get("--spec-file"),
        "workflow spec",
    )?
    else {
        return Err(CliError::Usage(
            "Missing required argument '--spec' or '--spec-file'".into(),
        ));
    };
    let raw_bytes = raw.len();
    if raw_bytes > INPUT_LIMITS.workflow_graph_bytes {
        return Err(CliError::Validation(format!(
            "Workflow graph JSON size {raw_bytes} bytes exceeds 1 MiB ({}) byte limit",
            INPUT_LIMITS.workflow_graph_bytes
        )));
    }
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|e| CliError::Validation(format!("malformed workflow spec JSON: {e}")))?;
    let obj = parsed.as_object().ok_or_else(|| {
        CliError::Validation("workflow spec must be a JSON object".to_string())
    })?;
    let title = obj
        .get("title")
        .or_else(|| obj.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("Untitled Workflow")
        .to_string();
    let mut graph_val = if let Some(inner) = obj.get("graph") {
        inner.clone()
    } else {
        json!({
            "nodes": obj.get("nodes").cloned().unwrap_or_else(|| json!([])),
            "edges": obj.get("edges").cloned().unwrap_or_else(|| json!([])),
            "strokes": obj.get("strokes").cloned().unwrap_or_else(|| json!([])),
        })
    };
    if let Some(gobj) = graph_val.as_object_mut() {
        gobj.entry("nodes".to_string()).or_insert_with(|| json!([]));
        gobj.entry("edges".to_string()).or_insert_with(|| json!([]));
        gobj.entry("strokes".to_string()).or_insert_with(|| json!([]));
    }
    let graph: WorkflowGraph = serde_json::from_value(graph_val)
        .map_err(|e| CliError::Validation(format!("invalid workflow graph specification: {e}")))?;
    let base_revision = match flags.get("--base-revision") {
        Some(r) => Some(parse_u64_domain(r, "base-revision")?),
        None => None,
    };

    let req = WorkflowSaveRequest {
        workflow_id: flags.get("--workflow-id").map(str::to_string),
        title,
        graph,
        raw_json_bytes: Some(raw_bytes),
        allow_loops: flags.has("--allow-loops"),
        base_revision,
    };
    let out = runtime
        .workflows()
        .save_workflow(req)
        .map_err(map_runtime_error)?;
    Ok(json!({
        "workflow": {
            "id": out.workflow_id,
            "workflow_id": out.workflow_id,
            "title": out.title,
            "revision": out.revision,
            "node_count": out.node_count,
            "step_count": out.step_count,
            "edge_count": out.edge_count,
            "loop_edges": out.loop_edges,
        }
    }))
}

fn run_workflow_run(flags: &ParsedFlags, runtime: &OpenBotHostRuntime) -> Result<Value, CliError> {
    let workflow_id = flags.require("--workflow-id")?.to_string();
    let approve_node = flags.get("--approve-node").map(str::to_string);
    let reject_node = flags.get("--reject-node").map(str::to_string);
    let mut run_id = flags.get("--run-id").map(str::to_string);

    if run_id.is_none() && (approve_node.is_some() || reject_node.is_some()) {
        let open_runs = runtime
            .database()
            .workflow_run_store()
            .open_runs()
            .map_err(map_storage_error)?;
        run_id = open_runs
            .into_iter()
            .rev()
            .find(|r| r.workflow_id == workflow_id)
            .map(|r| r.id);
    }

    let condition_mocks: BTreeMap<String, String> = match flags.get("--condition-mock") {
        Some(raw) => {
            let val = parse_json_value(raw, "condition-mock")?;
            serde_json::from_value(val).map_err(|e| {
                CliError::Validation(format!("invalid condition-mock JSON map: {e}"))
            })?
        }
        None => BTreeMap::new(),
    };
    let payload = read_inline_or_file(flags.get("--trigger-payload"), None, "trigger-payload")?;
    let req = WorkflowRunRequest {
        workflow_id,
        run_id,
        payload,
        approve_node,
        reject_node,
        condition_mocks,
        idempotency_key: flags.get("--idempotency-key").map(str::to_string),
    };
    let out = runtime
        .workflows()
        .run_workflow(req)
        .map_err(map_runtime_error)?;
    Ok(json!({ "workflow": out }))
}

fn parse_routine_schedule(raw: &str) -> Result<Option<RoutineSchedule>, CliError> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("webhook") {
        return Ok(None);
    }
    let now_iso = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    if trimmed.starts_with('{') {
        let mut val = parse_json_value(trimmed, "routine schedule")?;
        if let Some(obj) = val.as_object_mut()
            && obj.get("kind").and_then(Value::as_str) == Some("interval")
            && !obj.contains_key("anchorAt")
        {
            obj.insert("anchorAt".to_string(), Value::String(now_iso));
        }
        let sched: RoutineSchedule = serde_json::from_value(val).map_err(|e| {
            CliError::Validation(format!("invalid routine schedule specification: {e}"))
        })?;
        return Ok(Some(sched));
    }

    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("every ") {
        let rest = rest.trim();
        let (num_str, unit_str) = if let Some(n) = rest.strip_suffix("seconds") {
            (n.trim(), "s")
        } else if let Some(n) = rest.strip_suffix("second") {
            (n.trim(), "s")
        } else if let Some(n) = rest.strip_suffix('s') {
            (n.trim(), "s")
        } else if let Some(n) = rest.strip_suffix("minutes") {
            (n.trim(), "m")
        } else if let Some(n) = rest.strip_suffix("minute") {
            (n.trim(), "m")
        } else if let Some(n) = rest.strip_suffix('m') {
            (n.trim(), "m")
        } else if let Some(n) = rest.strip_suffix("hours") {
            (n.trim(), "h")
        } else if let Some(n) = rest.strip_suffix("hour") {
            (n.trim(), "h")
        } else if let Some(n) = rest.strip_suffix('h') {
            (n.trim(), "h")
        } else if let Some(n) = rest.strip_suffix("days") {
            (n.trim(), "d")
        } else if let Some(n) = rest.strip_suffix("day") {
            (n.trim(), "d")
        } else if let Some(n) = rest.strip_suffix('d') {
            (n.trim(), "d")
        } else {
            return Err(CliError::Validation(format!(
                "invalid routine interval expression '{trimmed}'"
            )));
        };
        let amount = parse_u32_domain(num_str, "schedule interval")?;
        return match unit_str {
            "s" => {
                if amount < 180 {
                    return Err(CliError::Validation(format!(
                        "routine interval {amount}s is below the minimum 180s (3 minutes) cadence"
                    )));
                }
                Ok(Some(RoutineSchedule::Interval {
                    amount: amount.div_ceil(60),
                    unit: RoutineIntervalUnit::Minutes,
                    anchor_at: now_iso,
                }))
            }
            "m" => Ok(Some(RoutineSchedule::Interval {
                amount,
                unit: RoutineIntervalUnit::Minutes,
                anchor_at: now_iso,
            })),
            "h" => Ok(Some(RoutineSchedule::Interval {
                amount,
                unit: RoutineIntervalUnit::Hours,
                anchor_at: now_iso,
            })),
            _ => Ok(Some(RoutineSchedule::Interval {
                amount,
                unit: RoutineIntervalUnit::Days,
                anchor_at: now_iso,
            })),
        };
    }

    Ok(Some(RoutineSchedule::Custom {
        expression: trimmed.to_string(),
    }))
}

fn run_routine_save(flags: &ParsedFlags, runtime: &OpenBotHostRuntime) -> Result<Value, CliError> {
    let (owner_id, is_channel) = if let Some(cid) = flags.get("--channel-id") {
        (cid.to_string(), true)
    } else if let Some(aid) = flags.get("--agent-id") {
        (aid.to_string(), false)
    } else {
        return Err(CliError::Usage(
            "Missing required argument '--agent-id' or '--channel-id'".into(),
        ));
    };
    let schedule = parse_routine_schedule(flags.require("--schedule")?)?;
    let filters: Vec<EventFilter> = match flags.get("--filters") {
        Some(raw) => {
            let val = parse_json_value(raw, "webhook filters")?;
            serde_json::from_value(val)
                .map_err(|e| CliError::Validation(format!("invalid webhook filters: {e}")))?
        }
        None => Vec::new(),
    };
    if let Some(qp) = flags.get("--quiet-policy") {
        let low = qp.trim().to_ascii_lowercase();
        if low != "no_update" && low != "always" {
            return Err(CliError::Validation(format!(
                "unsupported quiet-policy '{qp}': expected no_update or always"
            )));
        }
    }
    let instruction = flags
        .get("--instruction")
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Run scheduled routine check.")
        .to_string();

    let req = RoutineSaveRequest {
        owner_id,
        routine_id: flags.get("--routine-id").map(str::to_string),
        is_channel,
        name: flags.require("--name")?.to_string(),
        instruction,
        timezone: flags.get("--timezone").unwrap_or("UTC").to_string(),
        active: true,
        schedule,
        webhook_event_type: flags.get("--event-type").map(str::to_string),
        webhook_filters: filters,
        webhook_secret: flags.get("--webhook-secret").map(str::to_string),
        limit_policy: RoutineLimitPolicy::Wait,
        missed_policy: RoutineMissedPolicy::Skip,
        run_in: RoutineRunPlace::Chat,
    };
    let out = runtime
        .routines()
        .save_routine(req)
        .map_err(map_runtime_error)?;
    Ok(json!({
        "routine": {
            "id": out.routine_id,
            "routine_id": out.routine_id,
            "owner_id": out.owner_id,
            "name": out.name,
            "active": out.active,
            "interval_seconds": out.interval_seconds,
            "next_run_at": out.next_run_at,
            "webhook_route_id": out.webhook_route_id,
        }
    }))
}

fn lookup_routine_owner(
    runtime: &OpenBotHostRuntime,
    routine_id: &str,
) -> Result<(String, bool), CliError> {
    let agents = runtime
        .database()
        .agent_roster()
        .list_agents()
        .map_err(map_storage_error)?;
    let agent_store = runtime.database().agent_routine_store();
    for ag in agents {
        if agent_store
            .get(&ag.id, routine_id)
            .map_err(map_storage_error)?
            .is_some()
        {
            return Ok((ag.id, false));
        }
    }
    let channels = runtime
        .database()
        .channel_store()
        .list(SIGNED_OUT_CHANNEL_MEMBER_ID, false)
        .map_err(map_storage_error)?;
    let ch_store = runtime.database().channel_routine_store();
    for ch in channels {
        if ch_store
            .get(&ch.channel.id, routine_id)
            .map_err(map_storage_error)?
            .is_some()
        {
            return Ok((ch.channel.id, true));
        }
    }
    Err(CliError::Validation(format!(
        "routine '{routine_id}' not found"
    )))
}

fn run_routine_run(flags: &ParsedFlags, runtime: &OpenBotHostRuntime) -> Result<Value, CliError> {
    let routine_id = flags.require("--routine-id")?.to_string();
    let (owner_id, is_channel) = if let Some(cid) = flags.get("--channel-id") {
        (cid.to_string(), true)
    } else if let Some(aid) = flags.get("--agent-id") {
        (aid.to_string(), false)
    } else {
        lookup_routine_owner(runtime, &routine_id)?
    };

    let (kind, source) = match flags
        .get("--source")
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        None | Some("scheduled") => (RoutineRunKind::Scheduled, None),
        Some("test") => (RoutineRunKind::Manual, Some(RoutineRunSource::Test)),
        Some("script") => (RoutineRunKind::Manual, Some(RoutineRunSource::Script)),
        Some("webhook") => (RoutineRunKind::Manual, Some(RoutineRunSource::Webhook)),
        Some(other) => {
            return Err(CliError::Validation(format!(
                "unsupported routine run source '{other}': expected scheduled|script|webhook|test"
            )));
        }
    };

    let await_events = match flags.get("--await-events") {
        Some(raw) => {
            let val = parse_json_value(raw, "await-events")?;
            let arr = val.as_array().ok_or_else(|| {
                CliError::Validation("await-events must be a JSON array".to_string())
            })?;
            arr.iter()
                .map(|item| match item {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect()
        }
        None => Vec::new(),
    };

    let req = RoutineRunRequest {
        owner_id,
        routine_id,
        is_channel,
        kind,
        source,
        payload: flags.get("--payload").map(str::to_string),
        wait_id: flags.get("--wait-id").map(str::to_string),
        quiet_no_update: flags.has("--quiet-no-update"),
        report_text: flags.get("--report-text").map(str::to_string),
        await_events,
    };
    let out = runtime
        .routines()
        .run_routine(req)
        .map_err(map_runtime_error)?;
    Ok(json!({ "routine": out }))
}

fn run_webhook_post(flags: &ParsedFlags, runtime: &OpenBotHostRuntime) -> Result<Value, CliError> {
    let routine_id = flags.require("--routine-id")?.to_string();
    let raw_payload =
        read_inline_or_file(flags.get("--payload"), None, "webhook payload")?.unwrap_or_default();
    let signature = flags.require("--signature")?.to_string();

    let routes = runtime
        .database()
        .webhook_route_store()
        .list()
        .map_err(map_storage_error)?;
    let matched = routes.into_iter().find(|r| r.routine_id == routine_id);

    let owner_id = flags
        .get("--agent-id")
        .map(str::to_string)
        .or_else(|| {
            matched.as_ref().map(|r| match &r.owner {
                WebhookRouteOwner::Agent { id }
                | WebhookRouteOwner::Channel { id }
                | WebhookRouteOwner::Workflow { id } => id.clone(),
            })
        })
        .ok_or_else(|| {
            CliError::Validation(format!(
                "webhook route for routine '{routine_id}' not found"
            ))
        })?;
    let secret = match flags.get("--secret") {
        Some(supplied) => {
            if matched.is_some() && supplied.is_empty() {
                return Err(CliError::Unauthorized(
                    "invalid_signature: webhook HMAC signature verification failed".to_string(),
                ));
            }
            supplied.to_string()
        }
        None => {
            if matched.is_none() {
                return Err(CliError::Validation(format!(
                    "webhook secret for routine '{routine_id}' not found"
                )));
            }
            String::new()
        }
    };

    let standard_webhooks = signature.trim().starts_with("v1,");
    let req = WebhookPostRequest {
        owner_id,
        routine_id,
        secret,
        body: raw_payload.into_bytes(),
        signature,
        timestamp: flags.get("--timestamp").map(str::to_string),
        delivery_id: flags.get("--delivery-id").map(str::to_string),
        event_type: flags.get("--event-type").map(str::to_string),
        standard_webhooks,
        now_ms: None,
    };
    let out = runtime
        .routines()
        .post_webhook(req)
        .map_err(map_runtime_error)?;
    Ok(json!({ "routine": out }))
}

fn run_crm_sync(flags: &ParsedFlags, runtime: &OpenBotHostRuntime) -> Result<Value, CliError> {
    let fixture_json = match flags.get("--fixture") {
        Some(raw) => Some(parse_json_value(raw, "crm fixture")?),
        None => None,
    };
    let private_fields = flags.get("--private-fields").map(parse_csv);
    let req = CrmSyncRequest {
        source: flags.require("--source")?.to_string(),
        api_key: flags.get("--api-key").map(str::to_string),
        fixture_json,
        private_fields,
        for_agent_id: flags.get("--for-agent-id").map(str::to_string),
    };
    let out = runtime
        .crm()
        .sync_stripe(req)
        .map_err(map_runtime_error)?;
    Ok(json!({ "crm": out }))
}

fn run_crm_propose(flags: &ParsedFlags, runtime: &OpenBotHostRuntime) -> Result<Value, CliError> {
    let payload = match flags.get("--payload") {
        Some(raw) => Some(parse_json_value(raw, "crm payload")?),
        None => None,
    };
    let direct_change = match flags.get("--direct-change") {
        Some(raw) => Some(parse_json_value(raw, "crm direct-change")?),
        None => None,
    };
    let req = CrmProposeRequest {
        agent_id: flags.get("--agent-id").map(str::to_string),
        operation: flags.get("--operation").map(str::to_string),
        payload,
        reason: flags.get("--reason").map(str::to_string),
        proposal_id: flags.get("--proposal-id").map(str::to_string),
        approve: flags.has("--approve"),
        reject: flags.has("--reject"),
        commit_unapproved: flags.has("--commit-unapproved"),
        direct_change,
    };
    let out = runtime
        .crm()
        .propose_or_apply(req)
        .map_err(map_runtime_error)?;
    Ok(json!({ "crm": out }))
}

struct ScopedStageDir {
    path: PathBuf,
}

impl ScopedStageDir {
    fn create(parent: &Path) -> Result<Self, CliError> {
        let path = parent.join(format!(".site-publish-stage-{}", Uuid::new_v4()));
        fs::create_dir_all(&path)
            .map_err(|e| CliError::Execution(format!("failed to create site stage dir: {e}")))?;
        Ok(Self { path })
    }
}

impl Drop for ScopedStageDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run_site_publish(
    flags: &ParsedFlags,
    runtime: &OpenBotHostRuntime,
    data_dir: &Path,
) -> Result<Value, CliError> {
    let slug = flags.require("--slug")?.to_string();
    let mut _stage_guard: Option<ScopedStageDir> = None;

    let source_dir = if let Some(dir) = flags.get("--source-dir") {
        PathBuf::from(dir)
    } else if let Some(raw_files) = flags.get("--files") {
        let val = parse_json_value(raw_files, "site files")?;
        let arr = val
            .as_array()
            .ok_or_else(|| CliError::Validation("site files must be a JSON array".to_string()))?;
        let stage = ScopedStageDir::create(data_dir)?;
        for entry in arr {
            let obj = entry.as_object().ok_or_else(|| {
                CliError::Validation("each site file entry must be a JSON object".to_string())
            })?;
            let rel_raw = obj
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    CliError::Validation("site file entry missing string 'path'".to_string())
                })?;
            let validated_rel =
                StaticSiteService::validate_relative_path(rel_raw).map_err(map_runtime_error)?;
            let content = obj.get("content").and_then(Value::as_str).unwrap_or("");
            let dst = stage.path.join(&validated_rel);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    CliError::Execution(format!("failed to create staged file parent: {e}"))
                })?;
            }
            fs::write(&dst, content.as_bytes()).map_err(|e| {
                CliError::Execution(format!("failed to write staged site file: {e}"))
            })?;
        }
        let path = stage.path.clone();
        _stage_guard = Some(stage);
        path
    } else {
        return Err(CliError::Usage(
            "Missing required argument '--source-dir' or '--files'".into(),
        ));
    };

    let req = StaticSitePublishRequest {
        slug,
        source_dir,
        title: flags.get("--title").map(str::to_string),
        description: flags.get("--description").map(str::to_string),
        replace: flags.has("--replace"),
        spa_fallback: flags.has("--spa-fallback"),
        allowed_roots: None,
    };
    let record = runtime
        .static_sites()
        .publish_site(req)
        .map_err(map_runtime_error)?;
    Ok(json!({ "site": record }))
}

fn run_site_delete(flags: &ParsedFlags, runtime: &OpenBotHostRuntime) -> Result<Value, CliError> {
    let slug = flags.get("--slug").map(str::to_string);
    let site_id = flags.get("--site-id").map(str::to_string);
    if slug.is_none() && site_id.is_none() {
        return Err(CliError::Usage(
            "Missing required argument '--slug' or '--site-id'".into(),
        ));
    }
    let req = StaticSiteDeleteRequest { slug, site_id };
    let out = runtime
        .static_sites()
        .delete_site(req)
        .map_err(map_runtime_error)?;
    Ok(json!({ "site": out }))
}

fn run_marketplace_search(
    flags: &ParsedFlags,
    runtime: &OpenBotHostRuntime,
) -> Result<Value, CliError> {
    let req = MarketplaceSearchRequest {
        tab: flags.get("--tab").unwrap_or("apps").to_string(),
        source: flags.get("--source").map(str::to_string),
        query: flags.get("--query").map(str::to_string),
        category: flags.get("--category").map(str::to_string),
        reduced_motion: flags.has("--reduced-motion"),
        alpha_unsupported: flags.has("--alpha-unsupported"),
    };
    let out = runtime
        .marketplace()
        .search(req)
        .map_err(map_runtime_error)?;
    Ok(json!({ "marketplace": out }))
}

fn run_html_render(flags: &ParsedFlags, runtime: &OpenBotHostRuntime) -> Result<Value, CliError> {
    let html = read_inline_or_file(flags.get("--html"), flags.get("--html-file"), "html markup")?;
    let frame_height = match flags.get("--height") {
        Some(raw) => Some(parse_u32_domain(raw, "height")?),
        None => None,
    };
    let preview_width = match flags.get("--width") {
        Some(raw) => Some(parse_u32_domain(raw, "width")?),
        None => None,
    };
    let (bubbles_yours, bubbles_agents) = match flags.get("--bubbles") {
        Some(raw) => {
            let val = parse_json_value(raw, "bubbles")?;
            let yours = val
                .get("yours")
                .and_then(Value::as_str)
                .map(str::to_string);
            let agents = val
                .get("agents")
                .and_then(Value::as_str)
                .map(str::to_string);
            (yours, agents)
        }
        None => (None, None),
    };
    let rpc_message = match flags.get("--rpc-message") {
        Some(raw) => Some(parse_json_value(raw, "rpc-message")?),
        None => None,
    };
    let media_kind = flags.get("--generate-media").map(str::to_string);
    let cost_credits = match flags.get("--media-cost") {
        Some(raw) => Some(parse_i64_domain(raw, "media-cost")?),
        None => None,
    };
    let wallet_balance = match flags.get("--wallet-balance") {
        Some(raw) => Some(parse_i64_domain(raw, "wallet-balance")?),
        None => None,
    };
    let approval_threshold = match flags.get("--approval-threshold") {
        Some(raw) => Some(parse_i64_domain(raw, "approval-threshold")?),
        None => None,
    };
    let req = HtmlRenderRequest {
        mode: flags.get("--mode").map(str::to_string),
        html,
        title: flags.get("--title").map(str::to_string),
        frame_height,
        preview_width,
        appearance: flags.get("--appearance").map(str::to_string),
        bubbles_yours,
        bubbles_agents,
        backdrop: flags.get("--backdrop").map(str::to_string),
        rpc_message,
        generate_media: media_kind.is_some(),
        media_kind,
        media_model: flags.get("--media-model").map(str::to_string),
        media_prompt: flags.get("--media-prompt").map(str::to_string),
        cost_credits,
        wallet_balance,
        approval_threshold,
        agents_credits_allowed: flags
            .get_opt_bool("--agents-credits-allowed", "agents-credits-allowed")?,
        approve_media: flags.has("--approve-media"),
        decline_media: flags.has("--decline-media"),
    };
    let out = runtime
        .html_media()
        .render_or_generate(req)
        .map_err(map_runtime_error)?;
    Ok(json!({ "html_media": out }))
}

fn run_voice_eval(flags: &ParsedFlags, runtime: &OpenBotHostRuntime) -> Result<Value, CliError> {
    let mode_raw = flags.require("--mode")?.trim().to_ascii_lowercase();
    if let Some(rate_raw) = flags.get("--audio-rate-hz") {
        let rate = parse_u32_domain(rate_raw, "audio-rate-hz")?;
        if rate != 16_000 {
            return Err(CliError::Validation(format!(
                "unsupported STT audio sample rate {rate} Hz: expected 16000 Hz (16kHz) mono"
            )));
        }
    }
    if let Some(pause_raw) = flags.get("--turn-pause-ms") {
        let pause_ms = parse_u32_domain(pause_raw, "turn-pause-ms")?;
        if !VOICE_TURN_PAUSES.contains(&pause_ms) {
            return Err(CliError::Validation(format!(
                "unsupported turn-pause-ms {pause_ms}: allowed values are 1500, 2000, 3000, 5000"
            )));
        }
    }

    if mode_raw == "server-config" || mode_raw == "server" {
        if let Some(raw_srv) = flags.get("--voice-server-json") {
            let val = parse_json_value(raw_srv, "voice-server-json")?;
            let obj = val.as_object().ok_or_else(|| {
                CliError::Validation("voice-server-json must be a JSON object".to_string())
            })?;
            if obj.contains_key("apiKey") || obj.contains_key("api_key") {
                return Err(CliError::Validation(
                    "VoiceServerSettings must omit secret apiKey field; apiKey is not allowed"
                        .to_string(),
                ));
            }
            let address = obj
                .get("address")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    CliError::Validation("voice-server-json missing non-empty 'address'".to_string())
                })?
                .to_string();
            let has_api_key = obj
                .get("hasApiKey")
                .or_else(|| obj.get("has_api_key"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let routes: VoiceServerRoutes = match obj.get("routes").and_then(Value::as_object) {
                Some(r) => VoiceServerRoutes {
                    dictation: r.get("dictation").and_then(Value::as_bool).unwrap_or(false),
                    speech: r.get("speech").and_then(Value::as_bool).unwrap_or(false),
                    transcription_model: r
                        .get("transcriptionModel")
                        .or_else(|| r.get("transcription_model"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    speech_model: r
                        .get("speechModel")
                        .or_else(|| r.get("speech_model"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    voice: r.get("voice").and_then(Value::as_str).map(str::to_string),
                },
                None => VoiceServerRoutes {
                    dictation: false,
                    speech: false,
                    transcription_model: None,
                    speech_model: None,
                    voice: None,
                },
            };
            let saved = runtime
                .voice()
                .save_voice_server_settings(address.clone(), None, routes.clone())
                .map_err(map_runtime_error)?;
            return Ok(json!({
                "voice": {
                    "mode": "server-config",
                    "voice_server": {
                        "address": saved.address,
                        "hasApiKey": has_api_key || saved.has_api_key,
                        "routes": saved.routes,
                    }
                }
            }));
        }
        let existing = runtime
            .voice()
            .get_voice_server_settings()
            .map_err(map_runtime_error)?;
        return Ok(json!({
            "voice": {
                "mode": "server-config",
                "voice_server": existing,
            }
        }));
    }

    let audio_seconds = match flags.get("--audio-seconds") {
        Some(raw) => Some(parse_u32_domain(raw, "audio-seconds")?),
        None => None,
    };
    let wav_bytes = match flags.get("--audio-bytes") {
        Some(raw) => Some(parse_usize_domain(raw, "audio-bytes")?),
        None => None,
    };
    let tts_speed = match flags.get("--tts-speed") {
        Some(raw) => Some(parse_f64_domain(raw, "tts-speed")?),
        None => None,
    };
    let sdp = flags.get("--sdp").map(str::to_string);
    let req = VoiceEvalRequest {
        mode: mode_raw.clone(),
        locale: flags.get("--locale").map(str::to_string),
        stt_model: flags.get("--stt-provider").map(str::to_string),
        audio_seconds,
        wav_bytes,
        tts_provider: flags.get("--tts-provider").map(str::to_string),
        tts_text: flags.get("--tts-text").map(str::to_string),
        tts_speed,
        tts_speed_steps: Some(if flags.has("--cycle-speed") { 1 } else { 0 }),
        agent_id: flags.get("--agent-id").map(str::to_string),
        channel_id: flags.get("--channel-id").map(str::to_string),
        live_voice: flags.get("--live-voice").map(str::to_string),
        sdp_length: sdp.as_ref().map(|s| s.len()),
        sdp,
        voice_server_url: None,
        voice_server_api_key: None,
        voice_server_routes: None,
    };
    let out = runtime.voice().eval(req).map_err(map_runtime_error)?;

    let body = match mode_raw.as_str() {
        "stt" => json!({
            "mode": "stt",
            "stt_providers": VOICE_MODEL_IDS.len(),
            "stt_models": VOICE_MODEL_IDS,
            "selected_stt_model": out.model_id,
            "retired": out.retired.unwrap_or(false),
            "live_preview": true,
            "language": out.language,
            "provider": out.provider,
        }),
        "tts" => json!({
            "mode": "tts",
            "tts_providers": SPEECH_MODEL_IDS.len(),
            "tts_models": SPEECH_MODEL_IDS,
            "selected_tts_model": out.model_id,
            "speed": out.speed,
            "next_speed": out.next_speed,
            "speed_cycle": out.speed_cycle,
            "voice_agent_id": out.voice_agent_id,
            "char_count": out.char_count,
            "tts_voice": flags.get("--tts-voice"),
        }),
        "live" => {
            let sid = out
                .session_id
                .clone()
                .unwrap_or_else(|| format!("live-{}", Uuid::new_v4()));
            let answer_sdp = out
                .session_id
                .as_deref()
                .and_then(|id| runtime.voice().get_live_session(id))
                .map(|rec| rec.answer_sdp)
                .unwrap_or_else(|| {
                    "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=OpenBotLiveAnswer\r\nt=0 0\r\n"
                        .to_string()
                });
            json!({
                "mode": "live",
                "live_voices": CODEX_LIVE_VOICES.len(),
                "selected_live_voice": out.live_voice,
                "session_id": sid,
                "answer_sdp": answer_sdp,
                "webrtc_ready": true,
                "max_sdp_length": out.max_sdp_length,
            })
        }
        other => {
            return Err(CliError::Validation(format!(
                "unsupported voice-eval mode '{other}': expected stt|tts|live|server-config"
            )));
        }
    };
    Ok(json!({ "voice": body }))
}

fn run_bridge_sync(flags: &ParsedFlags, runtime: &OpenBotHostRuntime) -> Result<Value, CliError> {
    let platform_raw = flags.require("--platform")?.trim().to_ascii_lowercase();
    let platform = MessagingPlatform::parse(&platform_raw).ok_or_else(|| {
        CliError::Validation(format!(
            "unsupported bridge platform '{platform_raw}': expected slack|discord|telegram|whatsapp"
        ))
    })?;
    let direction = flags.require("--direction")?.trim().to_ascii_lowercase();
    if direction != "inbound" && direction != "outbound" {
        return Err(CliError::Validation(format!(
            "unsupported bridge direction '{direction}': expected inbound or outbound"
        )));
    }
    let payload_val = parse_json_value(flags.require("--payload")?, "bridge payload")?;
    let obj = payload_val.as_object().ok_or_else(|| {
        CliError::Validation("bridge payload must be a JSON object".to_string())
    })?;

    let text = obj
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let str_or_int = |v: &Value| -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    };
    let ws_id = flags
        .get("--workspace-id")
        .map(str::to_string)
        .or_else(|| {
            obj.get("workspaceId")
                .or_else(|| obj.get("workspace_id"))
                .or_else(|| obj.get("guildId"))
                .or_else(|| obj.get("guild_id"))
                .and_then(str_or_int)
        })
        .unwrap_or_else(|| format!("{platform_raw}-default"));

    if direction == "outbound" {
        let mapped_thread_id = obj
            .get("mappedThreadId")
            .or_else(|| obj.get("mapped_thread_id"))
            .or_else(|| obj.get("threadId"))
            .or_else(|| obj.get("chat_id"))
            .and_then(str_or_int)
            .unwrap_or_else(|| format!("{platform_raw}-thread-{}", Uuid::new_v4()));
        if platform == MessagingPlatform::Telegram {
            let _ = runtime
                .bridges()
                .dispatch_telegram_outbound(&mapped_thread_id, &text)
                .map_err(map_runtime_error)?;
        }
        let conn = runtime
            .bridges()
            .ensure_connection(platform, &ws_id, &ws_id)
            .map_err(map_runtime_error)?;
        let entry = MessagingContextEntry {
            id: format!("out-{}", Uuid::new_v4()),
            author_name: flags.get("--agent-id").unwrap_or("openbot").to_string(),
            text,
            sent_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            files: None,
        };
        let _ = runtime
            .bridges()
            .record_history(
                &conn.connection_id,
                &mapped_thread_id,
                &mapped_thread_id,
                flags.get("--agent-id").unwrap_or("openbot"),
                &entry,
            )
            .map_err(map_runtime_error)?;
        return Ok(json!({
            "bridge": {
                "platform": platform_raw,
                "direction": "outbound",
                "decision": "allow",
                "mapped_thread_id": mapped_thread_id,
                "delivered": true,
                "replied": true,
                "saved_history_only": false,
                "pairing_code": Value::Null,
            }
        }));
    }

    let sender = obj
        .get("sender")
        .or_else(|| obj.get("from"))
        .and_then(str_or_int);
    let is_group = obj
        .get("isGroup")
        .or_else(|| obj.get("is_group"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| sender.as_deref().is_some_and(|s| s.ends_with("@g.us")));
    let raw_channel = obj
        .get("channel")
        .or_else(|| obj.get("channelId"))
        .or_else(|| obj.get("channel_id"))
        .or_else(|| obj.get("chat_id"))
        .or_else(|| obj.get("chatId"))
        .and_then(str_or_int);
    let author_roles = obj.get("roles").and_then(Value::as_array).map(|arr| {
        arr.iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>()
    });
    let allowed_roles = match flags.get("--allowed-roles") {
        Some(raw) => {
            let val = parse_json_value(raw, "allowed-roles")?;
            let arr = val.as_array().cloned().ok_or_else(|| {
                CliError::Validation("allowed-roles must be a JSON array".to_string())
            })?;
            Some(arr)
        }
        None => None,
    };
    let allowed_senders = match flags.get("--allowed-senders") {
        Some(raw) => {
            let val = parse_json_value(raw, "allowed-senders")?;
            let arr = val.as_array().cloned().ok_or_else(|| {
                CliError::Validation("allowed-senders must be a JSON array".to_string())
            })?;
            Some(arr)
        }
        None => None,
    };
    let own_number = flags.get_opt_bool("--own-number", "own-number")?;

    let eval_req = BridgeEvalRequest {
        platform: platform_raw.clone(),
        workspace_id: Some(ws_id.clone()),
        sender: sender.clone(),
        text: Some(text.clone()),
        is_group,
        is_self_chat: obj
            .get("isSelfChat")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        from_me: obj.get("fromMe").and_then(Value::as_bool).unwrap_or(false),
        own_number,
        allowed_senders,
        pending_requests_count: None,
        allowed_roles,
        author_roles,
        channel_id: if platform == MessagingPlatform::Whatsapp && !is_group {
            None
        } else {
            raw_channel.clone()
        },
    };
    let eval_out = runtime
        .bridges()
        .eval_bridge(eval_req)
        .map_err(map_runtime_error)?;

    let mut mapped_thread_id: Option<String> = None;
    if eval_out.decision == "allow" || eval_out.decision == "save" {
        let conn = runtime
            .bridges()
            .ensure_connection(platform, &ws_id, &ws_id)
            .map_err(map_runtime_error)?;
        let plat_chan = raw_channel
            .or_else(|| sender.clone())
            .unwrap_or_else(|| format!("{platform_raw}-chan"));
        let agent_id = flags
            .get("--agent-id")
            .map(str::to_string)
            .or_else(|| {
                runtime
                    .agents()
                    .list_agents()
                    .first()
                    .map(|a| a.id.clone())
            })
            .unwrap_or_else(|| "agent-bridge".to_string());
        let link = runtime
            .bridges()
            .ensure_link(&MessagingLinkInput {
                connection_id: conn.connection_id.clone(),
                agent_id,
                platform_channel_id: plat_chan.clone(),
                thread_key: plat_chan.clone(),
                is_direct: !is_group,
                title: format!("{platform_raw}:{plat_chan}"),
            })
            .map_err(map_runtime_error)?;
        let entry = MessagingContextEntry {
            id: format!("in-{}", Uuid::new_v4()),
            author_name: sender.clone().unwrap_or_else(|| "external".to_string()),
            text,
            sent_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            files: None,
        };
        let _ = runtime
            .bridges()
            .record_history(
                &conn.connection_id,
                &plat_chan,
                &plat_chan,
                sender.as_deref().unwrap_or("external"),
                &entry,
            )
            .map_err(map_runtime_error)?;
        mapped_thread_id = Some(link.thread_id);
    }

    let delivered = eval_out.decision == "allow" || eval_out.decision == "save";
    let replied = eval_out.decision == "allow";
    let saved_history_only = eval_out.decision == "save";
    Ok(json!({
        "bridge": {
            "platform": platform_raw,
            "direction": "inbound",
            "decision": eval_out.decision,
            "mapped_thread_id": mapped_thread_id,
            "delivered": delivered,
            "replied": replied,
            "saved_history_only": saved_history_only,
            "pairing_code": eval_out.pairing_code,
            "normalized_forms": eval_out.normalized_forms,
            "dedup_key": eval_out.dedup_key,
        }
    }))
}
