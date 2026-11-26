//! `fabushi bot <subcommand>` handlers covering all 35+ Orca Bot OS operations.

use base64::engine::general_purpose::STANDARD as BASE64_STD;
use base64::Engine as _;
use orca_core::bot_items::{BotItemAsk, BotItemAskOption, BotItemAskQuestion};
use serde_json::{json, Map, Value};

use crate::error::CliError;
use crate::orca_commands::{
    build_query_url, load_json_file_or_inline, orca_usage, pct_encode, OrcaArgBag, OrcaCliHttp,
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

fn parse_actor_json(raw: &str) -> Result<Value, CliError> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed)
            .map_err(|e| orca_usage(format!("Invalid actor JSON: {e}")));
    }
    if trimmed.eq_ignore_ascii_case("owner") {
        return Ok(json!({ "kind": "owner" }));
    }
    if trimmed.eq_ignore_ascii_case("system") {
        return Ok(json!({ "kind": "system" }));
    }
    if let Some(bot_id) = trimmed.strip_prefix("bot:") {
        return Ok(json!({ "kind": "bot", "bot_id": bot_id.trim() }));
    }
    if let Some(rest) = trimmed.strip_prefix("person:") {
        let mut parts = rest.splitn(3, ':');
        let source = parts.next().unwrap_or("telegram").trim();
        let user_id = parts.next().unwrap_or("").trim();
        let name = parts.next().map(|s| s.trim().to_string());
        return Ok(json!({
            "kind": "person",
            "source": source,
            "user_id": user_id,
            "name": name,
        }));
    }
    Ok(json!({ "kind": "bot", "bot_id": trimmed }))
}

fn build_ask_from_args(args: &OrcaArgBag) -> Result<Option<BotItemAsk>, CliError> {
    if let Some(raw_json) = args.first(&["--ask-json"]) {
        let val = load_json_file_or_inline(raw_json, "ask")?;
        let ask: BotItemAsk = serde_json::from_value(val)
            .map_err(|e| orca_usage(format!("Invalid --ask-json payload: {e}")))?;
        return Ok(Some(ask));
    }

    let mut questions: Vec<BotItemAskQuestion> = Vec::new();
    let mut rec_labels: Vec<Option<String>> = Vec::new();

    for (flag, val_opt) in &args.ordered_flags {
        match flag.as_str() {
            "--ask" => {
                let prompt = val_opt.clone().unwrap_or_default();
                questions.push(BotItemAskQuestion {
                    prompt,
                    options: Vec::new(),
                    recommended: None,
                    multi: false,
                });
                rec_labels.push(None);
            }
            "--option" => {
                if let Some(q) = questions.last_mut()
                    && let Some(raw) = val_opt
                {
                    let (label, detail) = match raw.split_once('|') {
                        Some((l, d)) => (l.trim().to_string(), Some(d.trim().to_string())),
                        None => (raw.trim().to_string(), None),
                    };
                    q.options.push(BotItemAskOption { label, detail });
                }
            }
            "--recommended" => {
                if let Some(slot) = rec_labels.last_mut() {
                    *slot = val_opt.clone();
                }
            }
            "--multi" => {
                if let Some(q) = questions.last_mut() {
                    q.multi = true;
                }
            }
            _ => {}
        }
    }

    if questions.is_empty() {
        return Ok(None);
    }

    for (q, rec_opt) in questions.iter_mut().zip(rec_labels.into_iter()) {
        if let Some(rec_raw) = rec_opt {
            if let Ok(idx) = rec_raw.trim().parse::<usize>() {
                q.recommended = Some(idx);
            } else if let Some(idx) = q
                .options
                .iter()
                .position(|o| o.label.eq_ignore_ascii_case(rec_raw.trim()))
            {
                q.recommended = Some(idx);
            }
        }
    }

    Ok(Some(BotItemAsk {
        context: args.first(&["--ask-context", "--context"]).map(ToOwned::to_owned),
        questions,
    }))
}

pub(crate) async fn execute_bot(
    http: &OrcaCliHttp,
    sub: &str,
    args: &OrcaArgBag,
) -> Result<Value, CliError> {
    match sub {
        "list" => http.get("/api/v1/orca/bots").await,
        "get" | "show" => {
            let bot_id = req_str(args, Some(0), &["--bot-id", "--id"], "<bot_id>")?;
            http.get(&format!("/api/v1/orca/bots/{}", pct_encode(bot_id)))
                .await
        }
        "create" => {
            let mut body = Map::new();
            if let Some(name) = args.first(&["--name"]).or_else(|| args.pos(0)) {
                body.insert("name".into(), json!(name));
            }
            if let Some(agent) = args.first(&["--agent"]) {
                body.insert("agent".into(), json!(agent));
            }
            if let Some(soul) = args.first(&["--soul", "--custom-soul"]) {
                body.insert("custom_soul".into(), json!(soul));
            }
            if let Some(mascot) = args.first(&["--mascot"]) {
                body.insert("mascot".into(), json!(mascot));
            }
            if let Some(color) = args.first(&["--color", "--avatar-color"]) {
                body.insert("avatar_color".into(), json!(color));
            }
            if let Some(outfit) = args.first(&["--outfit", "--avatar-outfit"]) {
                body.insert("avatar_outfit".into(), json!(outfit));
            }
            if let Some(pm) = args.first(&["--permission-mode"]) {
                body.insert("permission_mode".into(), json!(pm));
            }
            if let Some(ws) = args.first(&["--folder-workspace-id", "--workspace-id"]) {
                body.insert("folder_workspace_id".into(), json!(ws));
            }
            if let Some(tpl) = args.first(&["--template-id"]) {
                body.insert("template_id".into(), json!(tpl));
            }
            if let Some(fr) = args.opt_bool(&["--first-run"])? {
                body.insert("first_run".into(), json!(fr));
            }
            http.post("/api/v1/orca/bots", &Value::Object(body)).await
        }
        "update" => {
            let bot_id = req_str(args, Some(0), &["--bot-id", "--id"], "<bot_id>")?;
            let mut body = Map::new();
            if let Some(name) = args.first(&["--name"]) {
                body.insert("name".into(), json!(name));
            }
            if let Some(agent) = args.first(&["--agent"]) {
                body.insert("agent".into(), json!(agent));
            }
            if let Some(paused) = args.opt_bool(&["--paused"])? {
                body.insert("paused".into(), json!(paused));
            }
            if let Some(fr) = args.opt_bool(&["--first-run"])? {
                body.insert("first_run".into(), json!(fr));
            }
            if let Some(avatar) = args.first(&["--avatar"]) {
                body.insert("avatar".into(), json!(avatar));
            }
            if let Some(color) = args.first(&["--color", "--avatar-color"]) {
                body.insert("avatar_color".into(), json!(color));
            }
            if let Some(outfit) = args.first(&["--outfit", "--avatar-outfit"]) {
                body.insert("avatar_outfit".into(), json!(outfit));
            }
            if let Some(png_path) = args.first(&["--avatar-png"]) {
                let bytes = std::fs::read(png_path).map_err(|e| {
                    orca_usage(format!("Failed reading --avatar-png '{png_path}': {e}"))
                })?;
                body.insert("avatar_png_b64".into(), json!(BASE64_STD.encode(bytes)));
            }
            if let Some(rev) = args.opt_u64(&["--expected-avatar-revision"])? {
                body.insert("expected_avatar_revision".into(), json!(rev));
            }
            if let Some(epoch) = args.first(&["--expected-avatar-revision-epoch"]) {
                body.insert("expected_avatar_revision_epoch".into(), json!(epoch));
            }
            if let Some(hmcp) = args.opt_bool(&["--host-mcp-servers"])? {
                body.insert("host_mcp_servers".into(), json!(hmcp));
            }
            let path = format!("/api/v1/orca/bots/{}", pct_encode(bot_id));
            http.patch(&path, &Value::Object(body)).await
        }
        "delete" => {
            let bot_id = req_str(args, Some(0), &["--bot-id", "--id"], "<bot_id>")?;
            http.delete(&format!("/api/v1/orca/bots/{}", pct_encode(bot_id)))
                .await
        }
        "templates" => {
            let mut q = Vec::new();
            if let Some(cat) = args.first(&["--category"]) {
                q.push(("category", cat.to_string()));
            }
            let path = build_query_url("/api/v1/orca/bots/templates", &q);
            http.get(&path).await
        }
        "from-template" => {
            let template_id = req_str(args, Some(0), &["--template-id", "--id"], "<template_id>")?;
            let mut body = json!({ "template_id": template_id });
            if let Some(name) = args.first(&["--name", "--custom-name"]) {
                body["custom_name"] = json!(name);
            }
            http.post("/api/v1/orca/bots/from-template", &body).await
        }
        _ => execute_bot_extended(http, sub, args).await,
    }
}

async fn execute_bot_extended(
    http: &OrcaCliHttp,
    sub: &str,
    args: &OrcaArgBag,
) -> Result<Value, CliError> {
    match sub {
        "chats" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            http.get(&format!("/api/v1/orca/bots/{}/chats", pct_encode(bot_id)))
                .await
        }
        "new-chat" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let mut body = Map::new();
            if let Some(title) = args.first(&["--title"]).or_else(|| args.pos(1)) {
                body.insert("title".into(), json!(title));
            }
            if let Some(sid) = args.first(&["--session-id"]) {
                body.insert("session_id".into(), json!(sid));
            }
            http.post(
                &format!("/api/v1/orca/bots/{}/chats", pct_encode(bot_id)),
                &Value::Object(body),
            )
            .await
        }
        "reply" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let (chat_id, text) = if args.first(&["--chat-id"]).is_some()
                || args.first(&["--text"]).is_some()
            {
                let cid = args.first(&["--chat-id", "--session-id"]).map(ToOwned::to_owned);
                let txt = req_str(args, Some(1), &["--text", "--message"], "<text>")?.to_string();
                (cid, txt)
            } else if args.positionals.len() >= 3 {
                (Some(args.positionals[1].clone()), args.positionals[2].clone())
            } else {
                (None, req_str(args, Some(1), &["--text"], "<text>")?.to_string())
            };
            let mut body = json!({
                "text": text,
                "leave_thread": args.has_bool(&["--leave-thread"]),
                "is_forwarded": args.has_bool(&["--is-forwarded"]),
                "is_quoted_reply": args.has_bool(&["--is-quoted-reply"]),
            });
            if let Some(cid) = chat_id {
                body["chat_id"] = json!(cid);
            }
            if let Some(addr) = args.first(&["--addressed-bot-name"]) {
                body["addressed_bot_name"] = json!(addr);
            }
            http.post(
                &format!("/api/v1/orca/bots/{}/reply", pct_encode(bot_id)),
                &body,
            )
            .await
        }
        "memory-get" | "memory" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            http.get(&format!("/api/v1/orca/bots/{}/memory", pct_encode(bot_id)))
                .await
        }
        "memory-set" | "memory-add" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let mut body = Map::new();
            if let Some(md) = args.first(&["--markdown"]) {
                body.insert("markdown".into(), json!(md));
            } else if let Some(line) = args
                .first(&["--fact", "--append-line", "--line"])
                .or_else(|| args.pos(1))
            {
                body.insert("append_line".into(), json!(line));
            }
            if let Some(src) = args.first(&["--turn-source"]) {
                body.insert("turn_source".into(), json!(src));
            }
            if let Some(txt) = args.first(&["--owner-turn-text"]) {
                body.insert("owner_turn_text".into(), json!(txt));
            }
            if args.has_bool(&["--allow-deletions"]) {
                body.insert("allow_deletions".into(), json!(true));
            }
            http.post(
                &format!("/api/v1/orca/bots/{}/memory", pct_encode(bot_id)),
                &Value::Object(body),
            )
            .await
        }
        "memory-forget" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let query = req_str(args, Some(1), &["--query", "--fact"], "<query>")?;
            let mut body = json!({ "query": query });
            if let Some(by) = args.first(&["--by"]) {
                body["by"] = json!(by);
            }
            http.post(
                &format!("/api/v1/orca/bots/{}/memory/forget", pct_encode(bot_id)),
                &body,
            )
            .await
        }
        "memory-undo" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let row_id = req_str(args, Some(1), &["--row-id", "--history-id"], "<row_id>")?;
            let body = json!({ "row_id": row_id });
            http.post(
                &format!("/api/v1/orca/bots/{}/memory/undo", pct_encode(bot_id)),
                &body,
            )
            .await
        }
        "tidy-up" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            if args.has_bool(&["--undo"]) {
                let mut body = Map::new();
                if let Some(rid) = args.first(&["--row-id", "--history-id"]) {
                    body.insert("row_id".into(), json!(rid));
                }
                return http
                    .post(
                        &format!("/api/v1/orca/bots/{}/tidy-up/undo", pct_encode(bot_id)),
                        &Value::Object(body),
                    )
                    .await;
            }
            let mut body = Map::new();
            if args.has_bool(&["--enable", "--disable"]) || args.first(&["--schedule"]).is_some() {
                let enabled = if args.has_bool(&["--disable"]) {
                    false
                } else {
                    true
                };
                let schedule = args.first(&["--schedule"]).unwrap_or("night");
                body.insert(
                    "configure".into(),
                    json!({ "enabled": enabled, "schedule": schedule }),
                );
            }
            if args.has_bool(&["--force"]) {
                body.insert("force".into(), json!(true));
                body.insert("run".into(), json!(true));
            } else if args.has_bool(&["--run"]) {
                body.insert("run".into(), json!(true));
            }
            if let Some(now_ms) = args.opt_i64(&["--now-ms"])? {
                body.insert("now_ms".into(), json!(now_ms));
            }
            if let Some(off) = args.opt_i64(&["--utc-offset-seconds"])? {
                body.insert("utc_offset_seconds".into(), json!(off));
            }
            http.post(
                &format!("/api/v1/orca/bots/{}/tidy-up", pct_encode(bot_id)),
                &Value::Object(body),
            )
            .await
        }
        "item-add" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let title = req_str(args, Some(1), &["--title"], "<title>")?;
            let mut body = Map::new();
            body.insert("title".into(), json!(title));
            if let Some(desc) = args.first(&["--description", "--desc"]) {
                body.insert("description".into(), json!(desc));
            }
            if let Some(dm) = args.first(&["--done-means"]) {
                body.insert("done_means".into(), json!(dm));
            }
            if let Some(st) = args.first(&["--status"]) {
                body.insert("status".into(), json!(st));
            }
            if let Some(wn) = args.first(&["--wait-note"]) {
                body.insert("wait_note".into(), json!(wn));
            }
            if let Some(actor_raw) = args.first(&["--actor"]) {
                body.insert("actor".into(), parse_actor_json(actor_raw)?);
            }
            if let Some(ask) = build_ask_from_args(args)? {
                body.insert(
                    "ask".into(),
                    serde_json::to_value(ask)
                        .map_err(|e| orca_usage(format!("Failed serializing ask: {e}")))?,
                );
            }
            http.post(
                &format!("/api/v1/orca/bots/{}/items", pct_encode(bot_id)),
                &Value::Object(body),
            )
            .await
        }
        _ => execute_bot_items_and_groups(http, sub, args).await,
    }
}

async fn execute_bot_items_and_groups(
    http: &OrcaCliHttp,
    sub: &str,
    args: &OrcaArgBag,
) -> Result<Value, CliError> {
    match sub {
        "item-set" => {
            let item_num = req_str(args, Some(0), &["--item", "--item-num"], "<item_num>")?;
            let mut body = Map::new();
            if let Some(title) = args.first(&["--title"]) {
                body.insert("title".into(), json!(title));
            }
            if let Some(desc) = args.first(&["--description", "--desc"]) {
                body.insert("description".into(), json!(desc));
            }
            if let Some(dm) = args.first(&["--done-means"]) {
                body.insert("done_means".into(), json!(dm));
            }
            if let Some(st) = args.first(&["--status"]) {
                body.insert("status".into(), json!(st));
            }
            if let Some(wn) = args.first(&["--wait-note"]) {
                body.insert("wait_note".into(), json!(wn));
            }
            if let Some(note) = args.first(&["--note", "--close-note"]) {
                body.insert("note".into(), json!(note));
            }
            if let Some(ev) = args.opt_u64(&["--expected-version"])? {
                body.insert("expected_version".into(), json!(ev));
            }
            if let Some(actor_raw) = args.first(&["--actor"]) {
                body.insert("actor".into(), parse_actor_json(actor_raw)?);
            }
            if let Some(ask) = build_ask_from_args(args)? {
                body.insert(
                    "ask".into(),
                    serde_json::to_value(ask)
                        .map_err(|e| orca_usage(format!("Failed serializing ask: {e}")))?,
                );
            }
            http.patch(
                &format!("/api/v1/orca/items/{}", pct_encode(item_num)),
                &Value::Object(body),
            )
            .await
        }
        "item-comment" => {
            let item_num = req_str(args, Some(0), &["--item", "--item-num"], "<item_num>")?;
            let mut body = Map::new();
            if let Some(text) = args.first(&["--text", "--message"]).or_else(|| args.pos(1)) {
                body.insert("text".into(), json!(text));
            }
            if let Some(reply_text) = args.first(&["--reply-text"]) {
                body.insert("reply_text".into(), json!(reply_text));
            }
            if let Some(qe) = args.opt_u64(&["--question-event"])? {
                body.insert("question_event".into(), json!(qe));
            }
            if let Some(actor_raw) = args.first(&["--actor"]) {
                body.insert("actor".into(), parse_actor_json(actor_raw)?);
            }
            let files = args.all(&["--file"]);
            if !files.is_empty() {
                body.insert("files".into(), json!(files));
            }
            if let Some(raw_sel) = args.first(&["--selections-json"]) {
                let parsed = load_json_file_or_inline(raw_sel, "selections")?;
                body.insert("selections".into(), parsed);
            } else {
                let selects = args.all(&["--select"]);
                if !selects.is_empty() {
                    let mut sels = Vec::new();
                    for raw in selects {
                        if let Some((q_part, opt_part)) = raw.split_once(':') {
                            let q_idx = q_part.trim().parse::<usize>().unwrap_or(0);
                            let mut indices = Vec::new();
                            for part in opt_part.split(',') {
                                if let Ok(oi) = part.trim().parse::<usize>() {
                                    indices.push(oi);
                                }
                            }
                            sels.push(json!({ "question_index": q_idx, "option_indices": indices }));
                        } else if let Ok(oi) = raw.trim().parse::<usize>() {
                            sels.push(json!({ "question_index": 0, "option_indices": [oi] }));
                        } else {
                            body.insert("reply_text".into(), json!(raw));
                        }
                    }
                    if !sels.is_empty() {
                        body.insert("selections".into(), Value::Array(sels));
                    }
                }
            }
            http.post(
                &format!("/api/v1/orca/items/{}/comment", pct_encode(item_num)),
                &Value::Object(body),
            )
            .await
        }
        "item-done" | "item-cancel" => {
            let item_num = req_str(args, Some(0), &["--item", "--item-num"], "<item_num>")?;
            let status = if sub == "item-done" { "done" } else { "canceled" };
            let mut body = json!({ "status": status });
            if let Some(note) = args.first(&["--note", "--close-note"]) {
                body["close_note"] = json!(note);
            }
            if let Some(ev) = args.opt_u64(&["--expected-version"])? {
                body["expected_version"] = json!(ev);
            }
            if let Some(actor_raw) = args.first(&["--actor"]) {
                body["actor"] = parse_actor_json(actor_raw)?;
            }
            http.patch(
                &format!("/api/v1/orca/items/{}", pct_encode(item_num)),
                &body,
            )
            .await
        }
        "item-snooze" => {
            let item_num = req_str(args, Some(0), &["--item", "--item-num"], "<item_num>")?;
            let mut body = Map::new();
            if let Some(until) = args.opt_i64(&["--until-ms", "--until"])? {
                body.insert("until_ms".into(), json!(until));
            }
            if args.has_bool(&["--unsnooze"]) {
                body.insert("unsnooze".into(), json!(true));
            }
            if let Some(actor_raw) = args.first(&["--actor"]) {
                body.insert("actor".into(), parse_actor_json(actor_raw)?);
            }
            http.post(
                &format!("/api/v1/orca/items/{}/snooze", pct_encode(item_num)),
                &Value::Object(body),
            )
            .await
        }
        "item-link" => {
            let item_num = req_str(args, Some(0), &["--item", "--item-num"], "<item_num>")?;
            let kind = req_str(args, None, &["--kind"], "--kind")?;
            let reference = req_str(args, None, &["--ref", "--reference"], "--ref")?;
            let meta = match kind.trim().to_ascii_lowercase().as_str() {
                "dispatch" => {
                    let mut m = Map::new();
                    m.insert("type".into(), json!("dispatch"));
                    if let Some(c) = args.first(&["--chain-id"]) {
                        m.insert("chain_id".into(), json!(c));
                    }
                    if let Some(p) = args.first(&["--parent-dispatch"]) {
                        m.insert("parent_dispatch_id".into(), json!(p));
                    }
                    Value::Object(m)
                }
                "pr" => {
                    let mut m = Map::new();
                    m.insert("type".into(), json!("pr"));
                    if let Some(t) = args.first(&["--title"]) {
                        m.insert("title".into(), json!(t));
                    }
                    Value::Object(m)
                }
                "file" => json!({ "type": "file" }),
                "url" => json!({ "type": "url" }),
                "automation" => json!({ "type": "automation", "active": true }),
                _ => json!({ "type": "none" }),
            };
            let body = json!({
                "link": {
                    "kind": kind,
                    "ref": reference,
                    "meta": meta,
                }
            });
            http.patch(
                &format!("/api/v1/orca/items/{}", pct_encode(item_num)),
                &body,
            )
            .await
        }
        "item-list" => {
            let bot_id = args.first(&["--bot-id"]).or_else(|| args.pos(0));
            if let Some(bid) = bot_id {
                let mut q = Vec::new();
                if args.has_bool(&["--include-closed"]) {
                    q.push(("include_closed", "true".to_string()));
                }
                if args.has_bool(&["--include-snoozed"]) {
                    q.push(("include_snoozed", "true".to_string()));
                }
                if let Some(st) = args.first(&["--status"]) {
                    q.push(("status", st.to_string()));
                }
                if let Some(lim) = args.opt_usize(&["--limit"])? {
                    q.push(("limit", lim.to_string()));
                }
                let path = build_query_url(
                    &format!("/api/v1/orca/bots/{}/items", pct_encode(bid)),
                    &q,
                );
                http.get(&path).await
            } else {
                let rpc = json!({
                    "method": "bots.items_list",
                    "params": {
                        "include_closed": args.has_bool(&["--include-closed"]),
                        "include_snoozed": args.has_bool(&["--include-snoozed"]),
                        "limit": args.opt_usize(&["--limit"])?,
                    },
                    "protocol_version": 3,
                    "caller_scope": "cli",
                });
                let resp = http.post("/api/v1/orca/rpc", &rpc).await?;
                Ok(resp.get("result").cloned().unwrap_or(resp))
            }
        }
        "item-show" => {
            let item_num = req_str(args, Some(0), &["--item", "--item-num"], "<item_num>")?;
            http.get(&format!("/api/v1/orca/items/{}", pct_encode(item_num)))
                .await
        }
        "needs-you" => {
            let mut q = Vec::new();
            if let Some(bid) = args.first(&["--bot-id"]).or_else(|| args.pos(0)) {
                q.push(("bot_id", bid.to_string()));
            }
            if let Some(foc) = args.first(&["--focused", "--focused-uid"]) {
                q.push(("focused_uid", foc.to_string()));
            }
            if let Some(cap) = args.opt_usize(&["--cap"])? {
                q.push(("cap", cap.to_string()));
            }
            http.get(&build_query_url("/api/v1/orca/needs-you", &q))
                .await
        }
        _ => execute_bot_ecosystem(http, sub, args).await,
    }
}

async fn execute_bot_ecosystem(
    http: &OrcaCliHttp,
    sub: &str,
    args: &OrcaArgBag,
) -> Result<Value, CliError> {
    match sub {
        "look-around" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let path = format!("/api/v1/orca/bots/{}/look-around", pct_encode(bot_id));
            let has_write = args.has_bool(&["--enable", "--disable", "--weekdays-only", "--run", "--force"])
                || args.first(&["--schedule", "--every", "--focus", "--suggestions-json"]).is_some();
            if !has_write {
                return http.get(&path).await;
            }
            let mut body = Map::new();
            let mut patch = Map::new();
            if args.has_bool(&["--enable"]) {
                patch.insert("on".into(), json!(true));
            } else if args.has_bool(&["--disable"]) {
                patch.insert("on".into(), json!(false));
            }
            if let Some(sched) = args.first(&["--schedule", "--every"]) {
                patch.insert("every".into(), json!(sched));
            }
            if args.has_bool(&["--weekdays-only"]) {
                patch.insert("weekdays_only".into(), json!(true));
            }
            if let Some(focus) = args.first(&["--focus"]) {
                patch.insert("focus".into(), json!(focus));
            }
            if !patch.is_empty() {
                body.insert("patch".into(), Value::Object(patch));
            }
            if args.has_bool(&["--run", "--force"]) {
                body.insert("run".into(), json!(true));
            }
            if args.has_bool(&["--force"]) {
                body.insert("force".into(), json!(true));
            }
            if let Some(raw_sug) = args.first(&["--suggestions-json"]) {
                body.insert(
                    "suggestions".into(),
                    load_json_file_or_inline(raw_sug, "suggestions")?,
                );
                body.insert("run".into(), json!(true));
            }
            http.post(&path, &Value::Object(body)).await
        }
        "suggest" | "up-next" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let path = format!("/api/v1/orca/bots/{}/up-next", pct_encode(bot_id));
            if let Some(sid) = args.first(&["--start"]) {
                http.post(&path, &json!({ "action": "start", "suggestion_id": sid }))
                    .await
            } else if let Some(sid) = args.first(&["--dismiss"]) {
                http.post(&path, &json!({ "action": "dismiss", "suggestion_id": sid }))
                    .await
            } else {
                http.get(&path).await
            }
        }
        "group-list" => http.get("/api/v1/orca/groups").await,
        "group-create" => {
            let name = req_str(args, Some(0), &["--name"], "<name>")?;
            let mut body = json!({
                "name": name,
                "member_bot_ids": args.all(&["--member", "--member-bot-id"]),
            });
            if let Some(rm) = args.first(&["--reply-mode"]) {
                body["reply_mode"] = json!(rm);
            }
            if let Some(folder) = args.first(&["--folder"]) {
                body["folder"] = json!(folder);
            }
            http.post("/api/v1/orca/groups", &body).await
        }
        "group-send" => {
            let gid = req_str(args, Some(0), &["--group-id", "--group"], "<group_id>")?;
            let text = req_str(args, Some(1), &["--text", "--message"], "<text>")?;
            let mut body = json!({ "text": text });
            if let Some(tid) = args.first(&["--thread-id", "--thread"]) {
                body["thread_id"] = json!(tid);
            }
            if let Some(sb) = args.first(&["--sender-bot"]) {
                body["sender"] = json!(format!("bot:{sb}"));
            } else if let Some(s) = args.first(&["--sender"]) {
                body["sender"] = json!(s);
            }
            if let Some(req_raw) = args.first(&["--requester"]) {
                body["requester"] = json!(req_raw);
            }
            http.post(
                &format!("/api/v1/orca/groups/{}/send", pct_encode(gid)),
                &body,
            )
            .await
        }
        "group-thread" => {
            let gid = req_str(args, Some(0), &["--group-id", "--group"], "<group_id>")?;
            let first_post = req_str(args, Some(1), &["--title", "--first-post"], "<title>")?;
            http.post(
                &format!("/api/v1/orca/groups/{}/threads", pct_encode(gid)),
                &json!({ "first_post": first_post }),
            )
            .await
        }
        "group-messages" => {
            let gid = req_str(args, Some(0), &["--group-id", "--group"], "<group_id>")?;
            let mut q = Vec::new();
            if let Some(tid) = args.first(&["--thread-id", "--thread"]) {
                q.push(("thread_id", tid.to_string()));
            }
            let path = build_query_url(
                &format!("/api/v1/orca/groups/{}/messages", pct_encode(gid)),
                &q,
            );
            http.get(&path).await
        }
        "access-list" => {
            let bot_id = args.first(&["--bot-id"]).or_else(|| args.pos(0));
            let mut q = Vec::new();
            if let Some(st) = args.first(&["--status"]) {
                q.push(("status", st.to_string()));
            }
            let base = match bot_id {
                Some(bid) => format!("/api/v1/orca/bots/{}/access", pct_encode(bid)),
                None => "/api/v1/orca/access".to_string(),
            };
            http.get(&build_query_url(&base, &q)).await
        }
        "access-request" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let member = if let Some(raw_m) = args.first(&["--member-json"]) {
                load_json_file_or_inline(raw_m, "member")?
            } else {
                let source = args.first(&["--source"]).unwrap_or("slack");
                let id = req_str(args, Some(1), &["--member-id", "--id"], "<member_id>")?;
                let name = args.first(&["--member-name", "--name"]).unwrap_or(id);
                json!({ "source": source, "id": id, "name": name })
            };
            let mut body = json!({ "member": member });
            if let Some(ws) = args.first(&["--workspace-id", "--connector-workspace-id"]) {
                body["connector_workspace_id"] = json!(ws);
            }
            if let Some(th) = args.first(&["--target-host-id"]) {
                body["target_host_id"] = json!(th);
            }
            if let Some(ttl) = args.opt_i64(&["--ttl-ms"])? {
                body["ttl_ms"] = json!(ttl);
            }
            http.post(
                &format!("/api/v1/orca/bots/{}/access", pct_encode(bot_id)),
                &body,
            )
            .await
        }
        "access-decide" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let mut body = Map::new();
            if let Some(tok) = args.first(&["--redeem-token"]) {
                body.insert("redeem_token".into(), json!(tok));
            } else {
                let req_id = req_str(args, Some(1), &["--request-id"], "--request-id")?;
                let approve = if args.has_bool(&["--decline", "--reject", "--deny"]) {
                    false
                } else {
                    args.opt_bool(&["--approve"])?.unwrap_or(true)
                };
                body.insert("request_id".into(), json!(req_id));
                body.insert("approve".into(), json!(approve));
            }
            http.post(
                &format!("/api/v1/orca/bots/{}/access/decide", pct_encode(bot_id)),
                &Value::Object(body),
            )
            .await
        }
        _ => execute_bot_sharing(http, sub, args).await,
    }
}

async fn execute_bot_sharing(
    http: &OrcaCliHttp,
    sub: &str,
    args: &OrcaArgBag,
) -> Result<Value, CliError> {
    match sub {
        "hand-over" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let ws_id = req_str(args, Some(1), &["--workspace-id", "--workspace"], "--workspace-id")?;
            let mut body = json!({ "workspace_id": ws_id });
            if args.has_bool(&["--create-new"]) {
                body["mode"] = json!("create_new");
            } else if let Some(mode) = args.first(&["--mode"]) {
                body["mode"] = json!(mode);
            }
            if let Some(p) = args.first(&["--purpose"]) {
                body["purpose"] = json!(p);
            }
            if let Some(wn) = args.first(&["--workspace-name"]) {
                body["workspace_name"] = json!(wn);
            }
            if let Some(rs) = args.first(&["--repo-slug"]) {
                body["repo_slug"] = json!(rs);
            }
            if let Some(rp) = args.first(&["--recent-prompt"]) {
                body["recent_prompt"] = json!(rp);
            }
            if let Some(ag) = args.first(&["--agent"]) {
                body["agent"] = json!(ag);
            }
            if let Some(cn) = args.first(&["--custom-name", "--name"]) {
                body["custom_name"] = json!(cn);
            }
            http.post(
                &format!("/api/v1/orca/bots/{}/hand-over", pct_encode(bot_id)),
                &body,
            )
            .await
        }
        "package-export" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let mut body = Map::new();
            if args.has_bool(&["--include-memory"]) {
                body.insert("include_memory".into(), json!(true));
            }
            if args.has_bool(&["--include-memory-index"]) {
                body.insert("include_memory_index".into(), json!(true));
            }
            if args.has_bool(&["--include-memory-topics"]) {
                body.insert("include_memory_topics".into(), json!(true));
            }
            if args.has_bool(&["--include-skills"]) {
                body.insert("include_skills".into(), json!(true));
            }
            if args.has_bool(&["--include-avatar"]) {
                body.insert("include_avatar_png".into(), json!(true));
            }
            let hints = args.all(&["--connector-hint"]);
            if !hints.is_empty() {
                body.insert("connectors_hint".into(), json!(hints));
            }
            let mut res = http
                .post(
                    &format!("/api/v1/orca/bots/{}/package/export", pct_encode(bot_id)),
                    &Value::Object(body),
                )
                .await?;
            if let Some(out_path) = args.first(&["--out", "--output-file"])
                && let Some(b64) = res.get("zip_b64").and_then(Value::as_str)
            {
                let bytes = BASE64_STD
                    .decode(b64)
                    .map_err(|e| orca_usage(format!("Failed decoding zip_b64: {e}")))?;
                std::fs::write(out_path, &bytes).map_err(|e| {
                    CliError::OrcaError {
                        code: "io_error".to_string(),
                        message: format!("Failed writing package to '{out_path}': {e}"),
                        exit_code: 1,
                    }
                })?;
                if let Some(obj) = res.as_object_mut() {
                    obj.insert("written_path".into(), json!(out_path));
                }
            }
            Ok(res)
        }
        "package-import" => {
            let zip_file = req_str(args, Some(0), &["--file", "--zip"], "<zip_file>")?;
            let raw_bytes = std::fs::read(zip_file).map_err(|e| {
                orca_usage(format!("Failed reading package file '{zip_file}': {e}"))
            })?;
            let mut body = json!({ "zip_b64": BASE64_STD.encode(raw_bytes) });
            if let Some(name) = args.first(&["--name", "--custom-name"]) {
                body["custom_name"] = json!(name);
            }
            http.post("/api/v1/orca/bots/package/import", &body).await
        }
        "share-identity" => http.get("/api/v1/orca/bots/share/identity").await,
        "share-seal" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            let mut body = Map::new();
            if let Some(mode) = args.first(&["--mode"]) {
                body.insert("mode".into(), json!(mode));
            }
            if let Some(rec) = args.first(&["--recipient-json", "--recipient"]) {
                body.insert(
                    "recipient_identity".into(),
                    load_json_file_or_inline(rec, "recipient_identity")?,
                );
            }
            if let Some(payload) = args.first(&["--payload-json", "--payload"]) {
                body.insert(
                    "payload".into(),
                    load_json_file_or_inline(payload, "payload")?,
                );
            }
            if let Some(oid) = args.first(&["--offer-id"]) {
                body.insert("offer_id".into(), json!(oid));
            }
            if let Some(sid) = args.first(&["--share-id"]) {
                body.insert("share_id".into(), json!(sid));
            }
            if let Some(exp) = args.opt_i64(&["--expires-at-ms"])? {
                body.insert("expires_at_ms".into(), json!(exp));
            }
            if let Some(text) = args.first(&["--text"]) {
                body.insert("text".into(), json!(text));
            }
            http.post(
                &format!("/api/v1/orca/bots/{}/share/seal", pct_encode(bot_id)),
                &Value::Object(body),
            )
            .await
        }
        "share-open" => {
            let mut body = Map::new();
            if let Some(env_raw) = args.first(&["--envelope-json", "--envelope"]).or_else(|| args.pos(0)) {
                let parsed = load_json_file_or_inline(env_raw, "envelope")?;
                if parsed.get("offer").is_some() || parsed.get("held_message").is_some() {
                    if let Some(obj) = parsed.as_object() {
                        for (k, v) in obj {
                            body.insert(k.clone(), v.clone());
                        }
                    }
                } else if args.first(&["--mode"]).is_some_and(|m| m == "held_message") {
                    body.insert("mode".into(), json!("held_message"));
                    body.insert("held_message".into(), parsed);
                } else {
                    body.insert("offer".into(), parsed);
                }
            }
            if let Some(mode) = args.first(&["--mode"]) {
                body.insert("mode".into(), json!(mode));
            }
            if let Some(offer) = args.first(&["--offer-json"]) {
                body.insert("offer".into(), load_json_file_or_inline(offer, "offer")?);
            }
            if let Some(held) = args.first(&["--held-json"]) {
                body.insert(
                    "held_message".into(),
                    load_json_file_or_inline(held, "held_message")?,
                );
            }
            if let Some(ed) = args.first(&["--sender-ed25519", "--expected-sender-ed25519"]) {
                body.insert("expected_sender_ed25519_b64".into(), json!(ed));
            }
            if let Some(sid) = args.first(&["--expected-share-id"]) {
                body.insert("expected_share_id".into(), json!(sid));
            }
            if let Some(bid) = args.first(&["--expected-bot-id"]) {
                body.insert("expected_bot_id".into(), json!(bid));
            }
            http.post("/api/v1/orca/bots/share/open", &Value::Object(body))
                .await
        }
        "import-discover" => {
            let openclaw_dirs = args.all(&["--openclaw-dir"]);
            let hermes_dirs = args.all(&["--hermes-dir"]);
            let openbot_dirs = args.all(&["--openbot-dir"]);
            let export_text = args.first(&["--export-text"]);
            if openclaw_dirs.is_empty()
                && hermes_dirs.is_empty()
                && openbot_dirs.is_empty()
                && export_text.is_none()
            {
                return http.get("/api/v1/orca/bots/import/discover").await;
            }
            let mut body = Map::new();
            if !openclaw_dirs.is_empty() || !hermes_dirs.is_empty() || !openbot_dirs.is_empty() {
                body.insert(
                    "roots".into(),
                    json!({
                        "openclaw_dirs": openclaw_dirs,
                        "hermes_dirs": hermes_dirs,
                        "openbot_dirs": openbot_dirs,
                    }),
                );
            }
            if let Some(et) = export_text {
                body.insert("export_text".into(), json!(et));
            }
            if let Some(name) = args.first(&["--name", "--custom-name"]) {
                body.insert("custom_name".into(), json!(name));
            }
            http.post("/api/v1/orca/bots/import/discover", &Value::Object(body))
                .await
        }
        "chat-tree" => {
            let bot_id = req_str(args, Some(0), &["--bot-id"], "<bot_id>")?;
            http.get(&format!(
                "/api/v1/orca/bots/{}/chat-tree",
                pct_encode(bot_id)
            ))
            .await
        }
        other => Err(CliError::UnknownCommand(format!(
            "Unknown 'bot' subcommand '{other}'"
        ))),
    }
}
