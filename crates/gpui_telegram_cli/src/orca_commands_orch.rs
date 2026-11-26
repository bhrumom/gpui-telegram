//! `fabushi orchestration`, `fabushi webhooks`, and `fabushi automations` CLI handlers.

use orca_core::triggers_webhooks::{
    AutomationConnectorEventTrigger, AutomationEventKind, AutomationEventSource,
    AutomationEventTrigger, AutomationGoogleTrigger, AutomationWebhookTrigger, WebhookFilters,
    WebhookWhereClause, WebhookWhereOp,
};
use serde_json::{json, Map, Value};

use crate::error::CliError;
use crate::orca_commands::{
    build_query_url, load_json_file_or_inline, orca_usage, pct_encode, sanitize_webhook_value,
    OrcaArgBag, OrcaCliHttp,
};

fn req_str<'a>(
    args: &'a OrcaArgBag,
    pos_idx: Option<usize>,
    flags: &[&str],
    label: &str,
) -> Result<&'a str, CliError> {
    if let Some(v) = args.first(flags).filter(|s| !s.trim().is_empty()) {
        return Ok(v);
    }
    if let Some(i) = pos_idx
        && let Some(v) = args.pos(i).filter(|s| !s.trim().is_empty())
    {
        return Ok(v);
    }
    Err(orca_usage(format!("Missing required argument {label}")))
}

pub(crate) async fn execute_orchestration(
    http: &OrcaCliHttp,
    sub: &str,
    args: &OrcaArgBag,
) -> Result<Value, CliError> {
    match sub {
        "run-create" => {
            let title = req_str(args, Some(0), &["--title", "--name"], "<title>")?;
            let mut body = json!({ "title": title });
            if let Some(rid) = args.first(&["--run-id", "--id"]) {
                body["run_id"] = json!(rid);
            }
            if let Some(csid) = args.first(&["--coordinator-session-id", "--session-id"]) {
                body["coordinator_orca_session_id"] = json!(csid);
            }
            if let Some(local) = args.opt_bool(&["--mailbox-local-only"])? {
                body["mailbox_is_local_only"] = json!(local);
            }
            if let Some(ms) = args.opt_i64(&["--now-ms"])? {
                body["now_ms"] = json!(ms);
            }
            http.post("/api/v1/orca/orchestration/runs", &body).await
        }
        "run-list" | "runs" => {
            let mut q = Vec::new();
            if let Some(st) = args.first(&["--status"]) {
                q.push(("status", st.to_string()));
            }
            http.get(&build_query_url("/api/v1/orca/orchestration/runs", &q))
                .await
        }
        "run-get" => {
            let run_id = req_str(args, Some(0), &["--run-id", "--id"], "<run_id>")?;
            http.get(&format!(
                "/api/v1/orca/orchestration/runs/{}",
                pct_encode(run_id)
            ))
            .await
        }
        "task-create" => {
            let run_id = req_str(args, Some(0), &["--run-id"], "--run-id")?;
            let title = req_str(
                args,
                if args.first(&["--run-id"]).is_some() {
                    Some(0)
                } else {
                    Some(1)
                },
                &["--title", "--name"],
                "--title",
            )?;
            let mut body = json!({
                "run_id": run_id,
                "title": title,
                "depends_on": args.all(&["--depends-on", "--dep"]),
            });
            if let Some(tid) = args.first(&["--task-id", "--id"]) {
                body["task_id"] = json!(tid);
            }
            if let Some(pid) = args.first(&["--parent-id", "--parent"]) {
                body["parent_id"] = json!(pid);
            }
            if let Some(depth) = args.opt_u32(&["--nesting-depth", "--depth"])? {
                body["nesting_depth"] = json!(depth);
            }
            http.post("/api/v1/orca/orchestration/tasks", &body).await
        }
        "task-set" | "task-transition" => {
            let task_id = req_str(args, Some(0), &["--task-id", "--id"], "<task_id>")?;
            let run_id = req_str(args, None, &["--run-id"], "--run-id")?;
            let status = req_str(args, Some(1), &["--status", "--next"], "--status")?;
            let mut body = json!({ "run_id": run_id, "status": status });
            if let Some(ms) = args.opt_i64(&["--now-ms"])? {
                body["now_ms"] = json!(ms);
            }
            http.patch(
                &format!("/api/v1/orca/orchestration/tasks/{}", pct_encode(task_id)),
                &body,
            )
            .await
        }
        "worker-start" => {
            let run_id = req_str(args, None, &["--run-id"], "--run-id")?;
            let task_id = req_str(args, None, &["--task-id"], "--task-id")?;
            let agent_id = args.first(&["--agent", "--agent-id"]).unwrap_or("claude");
            let mut body = json!({
                "run_id": run_id,
                "task_id": task_id,
                "agent_id": agent_id,
                "keep_open": args.has_bool(&["--keep-open"]),
            });
            if let Some(did) = args.first(&["--dispatch-id"]) {
                body["dispatch_id"] = json!(did);
            }
            if let Some(host) = args.first(&["--host-id", "--on", "--host"]) {
                body["host_id"] = json!(host);
            }
            if let Some(wt) = args.first(&["--worktree-id", "--worktree"]) {
                body["worktree_id"] = json!(wt);
            }
            if let Some(depth) = args.opt_u32(&["--depth"])? {
                body["depth"] = json!(depth);
            }
            if let Some(tab) = args.first(&["--tab-id"]) {
                body["tab_id"] = json!(tab);
            }
            if let Some(fu) = args.first(&["--follow-up-of"]) {
                body["follow_up_of_dispatch_id"] = json!(fu);
            }
            http.post("/api/v1/orca/orchestration/workers/start", &body)
                .await
        }
        "worker-done" => {
            let dispatch_id = req_str(args, Some(0), &["--dispatch-id", "--id"], "<dispatch_id>")?;
            let expected_gen = args
                .opt_u64(&["--expected-generation", "--generation", "--gen"])?
                .unwrap_or(1);
            let succeeded = if args.has_bool(&["--failed"]) {
                false
            } else {
                args.opt_bool(&["--succeeded"])?.unwrap_or(true)
            };
            let mut body = json!({
                "dispatch_id": dispatch_id,
                "expected_generation": expected_gen,
                "succeeded": succeeded,
                "keep_open": args.has_bool(&["--keep-open"]),
                "setup_failed": args.has_bool(&["--setup-failed"]),
            });
            if let Some(ec) = args.opt_i64(&["--exit-code"])? {
                body["exit_code"] = json!(ec);
            }
            if let Some(tx) = args.first(&["--transcript", "--transcript-text"]) {
                body["transcript_text"] = json!(tx);
            }
            http.post("/api/v1/orca/orchestration/workers/done", &body)
                .await
        }
        "worker-read" => {
            let dispatch_id = req_str(args, Some(0), &["--dispatch-id", "--id"], "<dispatch_id>")?;
            http.get(&format!(
                "/api/v1/orca/orchestration/workers/{}/archive",
                pct_encode(dispatch_id)
            ))
            .await
        }
        "worker-release" => {
            let dispatch_id = req_str(args, Some(0), &["--dispatch-id", "--id"], "<dispatch_id>")?;
            let mut body = Map::new();
            if let Some(ms) = args.opt_i64(&["--now-ms"])? {
                body.insert("now_ms".into(), json!(ms));
            }
            http.post(
                &format!(
                    "/api/v1/orca/orchestration/workers/{}/release",
                    pct_encode(dispatch_id)
                ),
                &Value::Object(body),
            )
            .await
        }
        _ => execute_orchestration_mail_and_gates(http, sub, args).await,
    }
}

async fn execute_orchestration_mail_and_gates(
    http: &OrcaCliHttp,
    sub: &str,
    args: &OrcaArgBag,
) -> Result<Value, CliError> {
    match sub {
        "send" => {
            let run_id = req_str(args, None, &["--run-id"], "--run-id")?;
            let from_id = args.first(&["--from", "--from-id"]).unwrap_or("coordinator");
            let to_raw = req_str(args, Some(0), &["--to", "--target"], "--to")?;
            let target_val = if to_raw.trim().starts_with('{') {
                load_json_file_or_inline(to_raw, "target")?
            } else {
                json!(to_raw)
            };
            let msg_type = args
                .first(&["--type", "--message-type"])
                .unwrap_or("status");
            let body_text = req_str(
                args,
                if args.first(&["--to", "--target"]).is_some() {
                    Some(0)
                } else {
                    Some(1)
                },
                &["--body", "--message", "--text"],
                "--body",
            )?;
            let mut payload = json!({
                "run_id": run_id,
                "from_id": from_id,
                "target": target_val,
                "message_type": msg_type,
                "body": body_text,
            });
            if let Some(pri) = args.first(&["--priority"]) {
                payload["priority"] = json!(pri);
            }
            if let Some(server_host) = args.first(&["--route-server", "--on", "--server-host"]) {
                payload["route"] = json!({
                    "kind": "server_relay",
                    "server_host_id": server_host,
                });
            }
            if let Some(gen_val) = args.opt_u64(&["--dispatch-generation", "--generation"])? {
                payload["dispatch_generation"] = json!(gen_val);
            }
            http.post("/api/v1/orca/orchestration/messages", &payload)
                .await
        }
        "check" => {
            let recipient = req_str(args, Some(0), &["--recipient-id", "--recipient", "--id"], "<recipient_id>")?;
            let mut q = vec![("recipient_id", recipient.to_string())];
            if let Some(did) = args.first(&["--dispatch-id"]) {
                q.push(("dispatch_id", did.to_string()));
            }
            if let Some(g) = args.opt_u64(&["--expected-generation", "--generation", "--gen"])? {
                q.push(("expected_generation", g.to_string()));
            }
            if let Some(ms) = args.opt_i64(&["--now-ms"])? {
                q.push(("now_ms", ms.to_string()));
            }
            http.get(&build_query_url("/api/v1/orca/orchestration/mailbox", &q))
                .await
        }
        "ask" => {
            let run_id = req_str(args, None, &["--run-id"], "--run-id")?;
            let dispatch_id = req_str(args, None, &["--dispatch-id"], "--dispatch-id")?;
            let prompt = req_str(args, Some(0), &["--prompt", "--question", "--text"], "<prompt>")?;
            let mut body = json!({
                "run_id": run_id,
                "dispatch_id": dispatch_id,
                "prompt": prompt,
            });
            if let Some(ms) = args.opt_i64(&["--now-ms"])? {
                body["now_ms"] = json!(ms);
            }
            http.post("/api/v1/orca/orchestration/questions", &body)
                .await
        }
        "gate-create" => {
            let run_id = req_str(args, None, &["--run-id"], "--run-id")?;
            let task_id = req_str(args, None, &["--task-id"], "--task-id")?;
            let gate_id = args
                .first(&["--gate-id", "--id"])
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("gate-{}", uuid::Uuid::new_v4()));
            let payload = if let Some(raw_p) = args.first(&["--payload-json"]) {
                load_json_file_or_inline(raw_p, "gate payload")?
            } else {
                let kind = args.first(&["--kind"]).unwrap_or("decision");
                match kind.trim().to_ascii_lowercase().as_str() {
                    "review" => json!({
                        "kind": "review",
                        "gate_id": gate_id,
                        "task_id": task_id,
                        "required_approvals": args.opt_u32(&["--required-approvals"])?.unwrap_or(1),
                        "approvals": [],
                        "rejections": [],
                        "status": "pending",
                        "deadline_ms": args.opt_i64(&["--deadline-ms"])?,
                    }),
                    "merge" => json!({
                        "kind": "merge",
                        "gate_id": gate_id,
                        "task_id": task_id,
                        "ci_passed": args.has_bool(&["--ci-passed"]),
                        "conflicts_resolved": args.has_bool(&["--conflicts-resolved"]),
                        "review_gates": [],
                        "require_human_approval": args.has_bool(&["--require-human-approval"]),
                        "human_approved": args.has_bool(&["--human-approved"]),
                        "status": "pending",
                        "deadline_ms": args.opt_i64(&["--deadline-ms"])?,
                    }),
                    _ => json!({
                        "kind": "decision",
                        "prompt": args.first(&["--prompt"]).or_else(|| args.pos(0)).unwrap_or("Approve gate?"),
                    }),
                }
            };
            let mut body = json!({
                "run_id": run_id,
                "task_id": task_id,
                "gate_id": gate_id,
                "payload": payload,
            });
            if let Some(dl) = args.opt_i64(&["--deadline-ms"])? {
                body["deadline_ms"] = json!(dl);
            }
            if let Some(ms) = args.opt_i64(&["--now-ms"])? {
                body["now_ms"] = json!(ms);
            }
            http.post("/api/v1/orca/orchestration/gates", &body).await
        }
        "gate-resolve" => {
            let gate_id = req_str(args, Some(0), &["--gate-id", "--id"], "<gate_id>")?;
            let actor = args.first(&["--actor"]).unwrap_or("owner");
            let mut body = if args.has_bool(&["--reject", "--decline", "--deny"]) {
                json!({
                    "action": "reject",
                    "actor": actor,
                    "reason": args.first(&["--reason", "--note"]),
                })
            } else if args.first(&["--action"]).is_some_and(|a| a == "evaluate_only" || a == "evaluate") {
                json!({ "action": "evaluate_only" })
            } else if args.has_bool(&["--ci-passed", "--conflicts-resolved", "--human-approved"]) {
                json!({
                    "action": "update_merge_signals",
                    "ci_passed": args.opt_bool(&["--ci-passed"])?,
                    "conflicts_resolved": args.opt_bool(&["--conflicts-resolved"])?,
                    "human_approved": args.opt_bool(&["--human-approved"])?,
                })
            } else {
                json!({
                    "action": "approve",
                    "actor": actor,
                    "note": args.first(&["--note"]),
                })
            };
            if let Some(ms) = args.opt_i64(&["--now-ms"])? {
                body["now_ms"] = json!(ms);
            }
            http.post(
                &format!("/api/v1/orca/orchestration/gates/{}/resolve", pct_encode(gate_id)),
                &body,
            )
            .await
        }
        "host-list" => http.get("/api/v1/orca/orchestration/hosts").await,
        "host-register" => {
            let raw = req_str(args, Some(0), &["--host-json", "--json-input"], "<host_json>")?;
            let body = load_json_file_or_inline(raw, "host")?;
            http.post("/api/v1/orca/orchestration/hosts", &body).await
        }
        "host-select" => {
            let raw = req_str(args, Some(0), &["--criteria-json", "--json-input"], "<criteria_json>")?;
            let body = load_json_file_or_inline(raw, "criteria")?;
            http.post("/api/v1/orca/orchestration/hosts/select", &body)
                .await
        }
        other => Err(CliError::UnknownCommand(format!(
            "Unknown 'orchestration' subcommand '{other}'"
        ))),
    }
}

fn parse_where_clause(raw: &str) -> Result<WebhookWhereClause, CliError> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed)
            .map_err(|e| orca_usage(format!("Invalid --where JSON: {e}")));
    }
    if let Some((path, val)) = trimmed.split_once("!=") {
        return Ok(WebhookWhereClause {
            path: path.trim().to_string(),
            op: WebhookWhereOp::NotEq,
            values: vec![val.trim().to_string()],
        });
    }
    if let Some((path, val)) = trimmed.split_once('=') {
        return Ok(WebhookWhereClause {
            path: path.trim().to_string(),
            op: WebhookWhereOp::Eq,
            values: vec![val.trim().to_string()],
        });
    }
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() >= 2 {
        let path = parts[0].to_string();
        let op = match parts[1] {
            "=" | "eq" => WebhookWhereOp::Eq,
            "!=" | "ne" | "not_eq" => WebhookWhereOp::NotEq,
            "in" => WebhookWhereOp::In,
            "contains" => WebhookWhereOp::Contains,
            "exists" => WebhookWhereOp::Exists,
            "!exists" | "not_exists" => WebhookWhereOp::NotExists,
            other => {
                return Err(orca_usage(format!(
                    "Unknown --where operator '{other}' (expected =, !=, in, contains, exists, !exists)"
                )));
            }
        };
        let values = if parts.len() > 2 {
            parts[2..].join(" ").split(',').map(|s| s.trim().to_string()).collect()
        } else {
            Vec::new()
        };
        return Ok(WebhookWhereClause { path, op, values });
    }
    Err(orca_usage(format!(
        "Invalid --where clause '{raw}' (expected 'path=value' or 'path op value')"
    )))
}

fn build_webhook_filters(args: &OrcaArgBag) -> Result<Option<WebhookFilters>, CliError> {
    let wheres = args.all(&["--where"]);
    let picks = args.all(&["--pick"]);
    let text_contains = args.first(&["--text-contains"]).map(ToOwned::to_owned);
    let subject_from = args.first(&["--subject-from"]).map(ToOwned::to_owned);
    let subject = args.first(&["--subject"]).map(ToOwned::to_owned);

    if wheres.is_empty()
        && picks.is_empty()
        && text_contains.is_none()
        && subject_from.is_none()
        && subject.is_none()
    {
        return Ok(None);
    }

    let mut parsed_wheres = Vec::with_capacity(wheres.len());
    for w in &wheres {
        parsed_wheres.push(parse_where_clause(w)?);
    }
    Ok(Some(WebhookFilters {
        where_clauses: parsed_wheres,
        text_contains,
        subject_from,
        subject,
        pick: picks,
    }))
}

pub(crate) async fn execute_webhooks(
    http: &OrcaCliHttp,
    sub: &str,
    args: &OrcaArgBag,
) -> Result<Value, CliError> {
    let mut val = match sub {
        "list" => {
            let mut q = Vec::new();
            if let Some(bid) = args.first(&["--bot-id"]).or_else(|| args.pos(0)) {
                q.push(("bot_id", bid.to_string()));
            }
            http.get(&build_query_url("/api/v1/orca/webhooks", &q))
                .await?
        }
        "create" => {
            let bot_id = req_str(args, None, &["--bot-id"], "--bot-id")?;
            let automation_id = req_str(args, None, &["--automation-id"], "--automation-id")?;
            let mut body = json!({
                "bot_id": bot_id,
                "automation_id": automation_id,
                "include_copy_card": false,
            });
            if let Some(adapter) = args.first(&["--adapter"]) {
                body["adapter"] = json!(adapter);
            }
            if let Some(filters) = build_webhook_filters(args)? {
                body["filters"] = serde_json::to_value(filters)
                    .map_err(|e| orca_usage(format!("Failed serializing webhook filters: {e}")))?;
            }
            if let Some(cap) = args.opt_u32(&["--hourly-cap"])? {
                body["hourly_cap"] = json!(cap);
            }
            if let Some(gm) = args.opt_u32(&["--grace-minutes"])? {
                body["grace_minutes"] = json!(gm);
            }
            if args.has_bool(&["--require-signature"]) {
                body["require_signature"] = json!(true);
            }
            http.post("/api/v1/orca/webhooks", &body).await?
        }
        "show" | "get" => {
            let hook_id = req_str(args, Some(0), &["--hook-id", "--id"], "<hook_id>")?;
            http.get(&format!("/api/v1/orca/webhooks/{}", pct_encode(hook_id)))
                .await?
        }
        "patch" | "edit" | "update" => {
            let hook_id = req_str(args, Some(0), &["--hook-id", "--id"], "<hook_id>")?;
            let mut body = Map::new();
            if let Some(paused) = args.opt_bool(&["--paused"])? {
                body.insert("paused".into(), json!(paused));
            }
            if let Some(filters) = build_webhook_filters(args)? {
                body.insert(
                    "filters".into(),
                    serde_json::to_value(filters)
                        .map_err(|e| orca_usage(format!("Failed serializing webhook filters: {e}")))?,
                );
            }
            http.patch(
                &format!("/api/v1/orca/webhooks/{}", pct_encode(hook_id)),
                &Value::Object(body),
            )
            .await?
        }
        "calls" => {
            let hook_id = req_str(args, Some(0), &["--hook-id", "--id"], "<hook_id>")?;
            let mut q = Vec::new();
            if let Some(cid) = args.first(&["--call-id"]) {
                q.push(("call_id", cid.to_string()));
            }
            let path = build_query_url(
                &format!("/api/v1/orca/webhooks/{}/calls", pct_encode(hook_id)),
                &q,
            );
            http.get(&path).await?
        }
        "check" => {
            let hook_id = req_str(args, Some(0), &["--hook-id", "--id"], "<hook_id>")?;
            let hook = http
                .get(&format!("/api/v1/orca/webhooks/{}", pct_encode(hook_id)))
                .await?;
            let calls = http
                .get(&format!(
                    "/api/v1/orca/webhooks/{}/calls",
                    pct_encode(hook_id)
                ))
                .await?;
            let call_count = calls.as_array().map(Vec::len).unwrap_or(0);
            let latest_call = calls.as_array().and_then(|arr| arr.last()).cloned();
            json!({
                "ok": true,
                "webhook": hook,
                "call_count": call_count,
                "latest_call": latest_call,
            })
        }
        other => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown 'webhooks' subcommand '{other}'"
            )));
        }
    };
    sanitize_webhook_value(&mut val);
    Ok(val)
}

fn parse_trigger_arg(raw: &str, idx: usize, args: &OrcaArgBag) -> Result<AutomationEventTrigger, CliError> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed)
            .map_err(|e| orca_usage(format!("Invalid --trigger JSON: {e}")));
    }
    let (src_str, kind_str) = trimmed
        .split_once(':')
        .or_else(|| trimmed.split_once('.'))
        .ok_or_else(|| {
            orca_usage(format!(
                "Invalid --trigger '{trimmed}' (expected '<source>:<event_kind>', e.g. github:pr_opened)"
            ))
        })?;
    let source = AutomationEventSource::parse(src_str.trim()).ok_or_else(|| {
        orca_usage(format!("Unknown trigger source '{}'", src_str.trim()))
    })?;
    let trigger_id = format!("trg-{}-{}", source.as_str(), idx + 1);
    match source {
        AutomationEventSource::Slack
        | AutomationEventSource::Github
        | AutomationEventSource::Linear => {
            let kind = AutomationEventKind::parse(src_str.trim(), kind_str.trim()).ok_or_else(
                || {
                    orca_usage(format!(
                        "Unknown event kind '{}' for source '{}'",
                        kind_str.trim(),
                        src_str.trim()
                    ))
                },
            )?;
            Ok(AutomationEventTrigger::Connector(
                AutomationConnectorEventTrigger {
                    id: trigger_id,
                    kind,
                    in_places: args.all(&["--in"]),
                    from: args.all(&["--from"]),
                    text_contains: args.first(&["--text-contains"]).map(ToOwned::to_owned),
                    subject: args.first(&["--subject"]).map(ToOwned::to_owned),
                    label: args.first(&["--label"]).map(ToOwned::to_owned),
                    run: args.first(&["--run-name"]).map(ToOwned::to_owned),
                    on_owner_message: args.opt_bool(&["--on-owner-message"])?,
                    from_owner: args.opt_bool(&["--from-owner"])?,
                    names: Default::default(),
                },
            ))
        }
        AutomationEventSource::Gmail => Ok(AutomationEventTrigger::Google(
            AutomationGoogleTrigger::Gmail {
                id: trigger_id,
                account: args.first(&["--account"]).map(ToOwned::to_owned),
                from: args.all(&["--from"]),
                to: args.all(&["--to"]),
                subject_contains: args.first(&["--subject-contains"]).map(ToOwned::to_owned),
                label: args.first(&["--label"]).map(ToOwned::to_owned),
            },
        )),
        AutomationEventSource::Calendar => Ok(AutomationEventTrigger::Google(
            AutomationGoogleTrigger::Calendar {
                id: trigger_id,
                account: args.first(&["--account"]).map(ToOwned::to_owned),
                calendar: args.first(&["--calendar"]).map(ToOwned::to_owned),
                minutes_before: args.opt_u32(&["--minutes-before"])?.unwrap_or(15),
            },
        )),
        AutomationEventSource::Webhook => {
            let hook_rule = if kind_str.starts_with("wh-") {
                kind_str.to_string()
            } else {
                format!("wh-{kind_str}")
            };
            Ok(AutomationEventTrigger::Webhook(AutomationWebhookTrigger {
                id: hook_rule,
                rev: 1,
                filters: WebhookFilters::default(),
            }))
        }
    }
}

fn parse_deadline_ms(raw: &str) -> Result<u64, CliError> {
    let trimmed = raw.trim();
    if let Ok(ms) = trimmed.parse::<u64>() {
        return Ok(ms);
    }
    chrono::DateTime::parse_from_rfc3339(trimmed)
        .map(|dt| dt.timestamp_millis().max(0) as u64)
        .map_err(|e| orca_usage(format!("Invalid --by timestamp '{trimmed}' (RFC3339 or epoch ms): {e}")))
}

pub(crate) async fn execute_automations(
    http: &OrcaCliHttp,
    sub: &str,
    args: &OrcaArgBag,
) -> Result<Value, CliError> {
    match sub {
        "list" => {
            let mut q = Vec::new();
            if let Some(bid) = args.first(&["--bot-id"]).or_else(|| args.pos(0)) {
                q.push(("bot_id", bid.to_string()));
            }
            if args.has_bool(&["--local-only"]) {
                q.push(("local_only", "true".to_string()));
            }
            http.get(&build_query_url("/api/v1/orca/automations", &q))
                .await
        }
        "create" => {
            let name = req_str(args, None, &["--name"], "--name")?;
            let prompt = req_str(args, None, &["--prompt"], "--prompt")?;
            let bot_id = req_str(args, None, &["--bot-id"], "--bot-id")?;
            let raw_triggers = args.all(&["--trigger"]);
            let mut triggers = Vec::with_capacity(raw_triggers.len());
            for (i, raw_t) in raw_triggers.iter().enumerate() {
                triggers.push(parse_trigger_arg(raw_t, i, args)?);
            }
            let mut body = json!({
                "name": name,
                "prompt": prompt,
                "bot_id": bot_id,
                "triggers": triggers,
            });
            if let Some(en) = args.opt_bool(&["--enabled"])? {
                body["enabled"] = json!(en);
            } else if args.has_bool(&["--disable"]) {
                body["enabled"] = json!(false);
            }
            if let Some(rrule) = args.first(&["--rrule"]) {
                body["rrule"] = json!(rrule);
            }
            if let Some(dt) = args.opt_i64(&["--dtstart-ms"])? {
                body["dtstart_ms"] = json!(dt);
            }
            if args.has_bool(&["--once"]) || args.first(&["--by"]).is_some() {
                let by = match args.first(&["--by"]) {
                    Some(raw_by) => Some(parse_deadline_ms(raw_by)?),
                    None => None,
                };
                body["once"] = json!({ "by": by });
            }
            if let Some(sid) = args.first(&["--chat-session-id", "--session-id"]) {
                body["chat_session_id"] = json!(sid);
            }
            http.post("/api/v1/orca/automations", &body).await
        }
        "edit" | "update" | "patch" => {
            let id = req_str(args, Some(0), &["--id", "--automation-id"], "<id>")?;
            let mut body = Map::new();
            if args.has_bool(&["--enable"]) {
                body.insert("enabled".into(), json!(true));
            } else if args.has_bool(&["--disable"]) {
                body.insert("enabled".into(), json!(false));
            } else if let Some(en) = args.opt_bool(&["--enabled"])? {
                body.insert("enabled".into(), json!(en));
            }
            if let Some(name) = args.first(&["--name"]) {
                body.insert("name".into(), json!(name));
            }
            if let Some(prompt) = args.first(&["--prompt"]) {
                body.insert("prompt".into(), json!(prompt));
            }
            if let Some(rrule) = args.first(&["--rrule"]) {
                body.insert("rrule".into(), json!(rrule));
            }
            http.patch(
                &format!("/api/v1/orca/automations/{}", pct_encode(id)),
                &Value::Object(body),
            )
            .await
        }
        "remove" | "delete" => {
            let id = req_str(args, Some(0), &["--id", "--automation-id"], "<id>")?;
            http.delete(&format!("/api/v1/orca/automations/{}", pct_encode(id)))
                .await
        }
        "watch-pr" => {
            let bot_id = req_str(args, None, &["--bot-id"], "--bot-id")?;
            let pr = req_str(args, Some(0), &["--pr"], "<owner/repo#n>")?;
            let mut body = json!({
                "bot_id": bot_id,
                "pr": pr,
            });
            if let Some(dur) = args.first(&["--duration"]) {
                body["duration"] = json!(dur);
            }
            if let Some(sid) = args.first(&["--session-id"]) {
                body["session_id"] = json!(sid);
            }
            if args.has_bool(&["--ci-finished-supported"]) {
                body["ci_finished_supported"] = json!(true);
            }
            http.post("/api/v1/orca/automations/watch-pr", &body).await
        }
        "watch-pr-wake" => {
            let watch_id = req_str(args, Some(0), &["--watch-id", "--id"], "<watch_id>")?;
            let outcome = req_str(args, Some(1), &["--outcome"], "--outcome")?;
            let mut body = json!({ "outcome": outcome });
            if let Some(ms) = args.opt_u64(&["--now-ms"])? {
                body["now_ms"] = json!(ms);
            }
            http.post(
                &format!(
                    "/api/v1/orca/automations/watch-pr/{}/wake",
                    pct_encode(watch_id)
                ),
                &body,
            )
            .await
        }
        other => Err(CliError::UnknownCommand(format!(
            "Unknown 'automations' subcommand '{other}'"
        ))),
    }
}
