//! Extended Unified RPC routes for `bots.home.*`, `bots.chat.*`, `bots.memory.*`,
//! `bots.tidyUp.*`, `bots.items.*`, `bots.lookAround.*`, `bots.upNext.*`, and `bots.groups.*`.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use orca_bots::memory_service::{
    TIDY_UP_MAX_CHATS, TIDY_UP_MAX_TOTAL_CHARS, evaluate_tidy_up_due,
};
use orca_bots::wire_json::{
    format_ms_rfc3339, item_link_to_wire_json, item_to_wire_json, parse_flexible_item_link,
    parse_rfc3339_or_ms,
};
use orca_bots::{
    BotPatch, GroupMessageSender, ListBotItemsFilter, OrcaBotsFacade, PostGroupMessageRequest,
    TidyUpRuntimeState, TidyUpSchedule, UpdateBotGroupRequest,
};
use orca_core::bot_groups::{
    BotGroupPeerScope, BotGroupReplyMode, BotGroupSend, MentionCandidate, RemoteBotMember,
};
use orca_core::bot_items::{BotItemActor, BotItemLinkKind, BotItemStatus};
use orca_core::bot_look_around::{
    LookAroundEvery, LookAroundPatch, ensure_look_around_agent, evaluate_look_around,
};
use orca_core::bot_memory::{
    JournalAsker, JournalCloseAttribution, JournalCloseStatus, JournalEntryInput,
    is_protected_owner_line, parse_memory_facts, utc_today_iso_date,
};
use orca_core::bot_preamble::{QuietBotAnswerRow, StreamingQuietMarkerFilter};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{OrcaIdeRuntimeError, Result};

fn opt_str<'a>(params: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(s) = params
            .get(*key)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            return Some(s);
        }
    }
    None
}

fn req_str<'a>(params: &'a Value, keys: &[&str]) -> Result<&'a str> {
    opt_str(params, keys).ok_or_else(|| {
        OrcaIdeRuntimeError::invalid_argument(format!(
            "missing required string '{}'",
            keys.first().copied().unwrap_or("param")
        ))
    })
}

fn collect_home_files(root: &Path, cur: &Path, out: &mut Map<String, Value>) {
    let Ok(entries) = std::fs::read_dir(cur) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if fname.starts_with(".tmp.") || fname == ".claude" || fname == ".agents" {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_home_files(root, &path, out);
        } else if meta.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            let key = rel.to_string_lossy().replace('\\', "/");
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.insert(key, Value::String(text));
            }
        }
    }
}

pub(super) async fn dispatch_bots_home(
    bots: &Arc<OrcaBotsFacade>,
    sub: &str,
    params: &Value,
) -> Result<Value> {
    let bot_id = req_str(params, &["botId", "bot_id", "id"])?;
    let bot = bots.controller().get_bot(bot_id).await?;
    let home = bots.home_store();
    match sub {
        "writeSoul" | "write_soul" => {
            home.assert_bot_owns_home(&bot.folder, &bot.id)?;
            let content = params
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing string 'content'"))?;
            let path = home.resolve_relative_path(&bot.folder, "SOUL.md")?;
            home.replace_bytes(&path, content.as_bytes())?;
            let complete_first_run = params
                .get("completeFirstRun")
                .or_else(|| params.get("complete_first_run"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut first_run = bot.first_run;
            if complete_first_run && bot.first_run {
                let updated = bots
                    .controller()
                    .update_bot(
                        &bot.id,
                        BotPatch {
                            first_run: Some(false),
                            ..BotPatch::default()
                        },
                    )
                    .await?;
                first_run = updated.first_run;
            }
            Ok(json!({ "ok": true, "botId": bot.id, "firstRun": first_run }))
        }
        "writeFile" | "write_file" => {
            home.assert_bot_owns_home(&bot.folder, &bot.id)?;
            let rel = req_str(params, &["path"])?;
            let content = params
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing string 'content'"))?;
            let target = home.resolve_relative_path(&bot.folder, rel)?;
            home.replace_bytes(&target, content.as_bytes())?;
            Ok(json!({ "ok": true, "botId": bot.id, "path": rel }))
        }
        "inspect" => {
            home.assert_bot_owns_home(&bot.folder, &bot.id)?;
            let dir = home.bot_dir(&bot.folder);
            let mut files = Map::new();
            collect_home_files(&dir, &dir, &mut files);
            Ok(json!({
                "botId": bot.id,
                "folder": bot.folder.as_str(),
                "homePath": dir.to_string_lossy().to_string(),
                "files": Value::Object(files),
            }))
        }
        "syncGenerated" | "sync_generated" => {
            home.sync_generated_home_files(&bot)?;
            Ok(json!({ "ok": true, "botId": bot.id }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown bots.home RPC method 'bots.home.{other}'"
        ))),
    }
}

pub(super) async fn dispatch_bots_chat(
    bots: &Arc<OrcaBotsFacade>,
    sub: &str,
    params: &Value,
) -> Result<Value> {
    let bot_id = req_str(params, &["botId", "bot_id", "id"])?;
    let bot = bots.controller().get_bot(bot_id).await?;
    match sub {
        "preamble" => {
            let cli_cmd = opt_str(params, &["cliCmd", "cli_cmd"]);
            let (preamble, rules_key) =
                bots.memory_service().build_session_preamble(&bot, cli_cmd, None)?;
            Ok(json!({
                "botId": bot.id,
                "preamble": preamble,
                "rulesKey": rules_key,
                "firstRun": bot.first_run,
            }))
        }
        "deltaSync" | "delta_sync" => {
            let session_id = req_str(params, &["sessionId", "session_id"])?;
            let cli_cmd = opt_str(params, &["cliCmd", "cli_cmd"]);
            let sync_opt = bots.sync_session_memory_delta(&bot, session_id, cli_cmd)?;
            let mut parts: Vec<String> = Vec::new();
            if let Some(sync) = sync_opt {
                if let Some(s) = sync.delta.soul_update {
                    parts.push(s);
                }
                if let Some(m) = sync.delta.memory_update {
                    parts.push(m);
                }
                if let Some(t) = sync.delta.topics_update {
                    parts.push(t);
                }
                if let Some(r) = sync.rules_update {
                    parts.push(r);
                }
            }
            let delta_text = parts.join("\n\n");
            let has_changes = !delta_text.is_empty();
            Ok(json!({
                "botId": bot.id,
                "sessionId": session_id,
                "hasChanges": has_changes,
                "deltaText": delta_text,
            }))
        }
        "streamQuietFilter" | "stream_quiet_filter" => {
            let chunks: Vec<String> = params
                .get("chunks")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect())
                .unwrap_or_default();
            let tool_actions = params
                .get("toolActionsCount")
                .or_else(|| params.get("tool_actions_count"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let in_automation = params
                .get("inAutomationTurn")
                .or_else(|| params.get("in_automation_turn"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let streaming = params
                .get("streaming")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut filter = StreamingQuietMarkerFilter::new();
            let mut emitted = Vec::with_capacity(chunks.len());
            for ch in &chunks {
                emitted.push(filter.push_chunk(ch));
            }
            let ans = filter.finish(QuietBotAnswerRow {
                bot_answer: true,
                streaming,
                in_automation_turn: in_automation,
                in_acting_turn: tool_actions > 0,
            });
            let suppressed = ans.quiet && ans.words.is_empty();
            Ok(json!({
                "quiet": ans.quiet,
                "suppressed": suppressed,
                "outboundText": ans.words,
                "finalText": ans.words,
                "statusLabel": ans.display_text().unwrap_or(""),
                "nothingToReport": ans.nothing_to_report,
                "emittedChunks": emitted,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown bots.chat RPC method 'bots.chat.{other}'"
        ))),
    }
}

pub(super) async fn dispatch_bots_memory(
    bots: &Arc<OrcaBotsFacade>,
    sub: &str,
    params: &Value,
    now_ms: i64,
) -> Result<Value> {
    let bot_id = req_str(params, &["botId", "bot_id", "id"])?;
    let bot = bots.controller().get_bot(bot_id).await?;
    let home = bots.home_store();
    match sub {
        "applyTurn" | "apply_turn" => {
            home.assert_bot_owns_home(&bot.folder, &bot.id)?;
            let content = params
                .get("content")
                .or_else(|| params.get("markdown"))
                .and_then(Value::as_str)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing string 'content'"))?;
            let source = params
                .get("source")
                .or_else(|| params.get("turnSource"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let owner_turn_text = opt_str(params, &["ownerTurnText", "owner_turn_text"]).unwrap_or("");
            let session_id = opt_str(params, &["sessionId", "session_id"]).unwrap_or("");
            let requested_allow_deletions = params
                .get("allowDeletions")
                .or_else(|| params.get("allow_deletions"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let effective_allow_deletions = requested_allow_deletions && source.is_none();
            let today = opt_str(params, &["today"])
                .map(ToOwned::to_owned)
                .unwrap_or_else(utc_today_iso_date);
            let mem_path = home.bot_dir(&bot.folder).join("MEMORY.md");
            let before_md = home.read_text_optional(&mem_path)?.unwrap_or_default();
            home.replace_bytes(&mem_path, content.as_bytes())?;
            let res = bots.memory_service().apply_turn_end_memory_governance(
                &bot,
                &before_md,
                source,
                owner_turn_text,
                session_id,
                effective_allow_deletions,
                now_ms,
                &today,
            )?;
            Ok(json!({
                "content": res.final_markdown,
                "final_markdown": res.final_markdown,
                "tagged_lines": res.tagged_lines,
                "restored_lines": res.restored_lines,
            }))
        }
        "journal" => {
            let sync = params.get("sync").and_then(Value::as_bool).unwrap_or(false);
            let today = utc_today_iso_date();
            let mut year_month = opt_str(params, &["yearMonth", "year_month"])
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| today[..7].to_string());
            if sync {
                let items = bots.items_store().list_items(ListBotItemsFilter {
                    owner_bot_id: Some(bot.id.clone()),
                    statuses: Some(vec![BotItemStatus::Done, BotItemStatus::Canceled]),
                    include_closed: true,
                    include_snoozed: true,
                    now_ms: Some(now_ms),
                    limit: None,
                    offset: 0,
                })?;
                for item in &items {
                    let attribution = match item.made_by.as_ref() {
                        Some(BotItemActor::Person {
                            source,
                            user_id,
                            name,
                        }) => JournalCloseAttribution::ExternalPeople(vec![JournalAsker {
                            source_name: source.clone(),
                            user_id: user_id.clone(),
                            name: name.clone(),
                        }]),
                        _ => JournalCloseAttribution::Owner,
                    };
                    let closed_ms = item.closed_at_ms.unwrap_or(now_ms);
                    let date_iso = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(closed_ms)
                        .map(|dt| dt.format("%Y-%m-%d").to_string())
                        .unwrap_or_else(utc_today_iso_date);
                    let status = if item.status == BotItemStatus::Canceled {
                        JournalCloseStatus::Canceled
                    } else {
                        JournalCloseStatus::Done
                    };
                    let prs: Vec<String> = item
                        .links
                        .iter()
                        .filter(|l| l.kind == BotItemLinkKind::Pr)
                        .map(|l| l.reference.clone())
                        .collect();
                    let entry = JournalEntryInput {
                        date: date_iso.clone(),
                        item_number: item.number,
                        status,
                        title: item.title.clone(),
                        prs,
                        attribution,
                    };
                    let _ = bots
                        .memory_service()
                        .append_closed_item_journal(&bot.folder, &bot.id, &entry)?;
                    if opt_str(params, &["yearMonth", "year_month"]).is_none()
                        && date_iso.len() >= 7
                    {
                        year_month = date_iso[..7].to_string();
                    }
                }
            }
            let journal_path = home
                .bot_dir(&bot.folder)
                .join("memory")
                .join("journal")
                .join(format!("{year_month}.md"));
            let content = home.read_text_optional(&journal_path)?.unwrap_or_default();
            Ok(json!({
                "botId": bot.id,
                "yearMonth": year_month,
                "content": content,
            }))
        }
        "inspect" => {
            let mem_path = home.bot_dir(&bot.folder).join("MEMORY.md");
            let content = home.read_text_optional(&mem_path)?.unwrap_or_default();
            let facts = parse_memory_facts(&content);
            let entries: Vec<Value> = facts
                .into_iter()
                .map(|f| {
                    let protected = is_protected_owner_line(&f.text);
                    json!({
                        "line": f.line,
                        "key": f.key,
                        "date": f.date,
                        "kind": f.kind.map(|k| k.prefix_word()),
                        "scope": f.scope,
                        "text": f.text,
                        "body": f.body,
                        "source": f.source,
                        "protected": protected,
                    })
                })
                .collect();
            Ok(json!({
                "botId": bot.id,
                "content": content,
                "entries": entries,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown bots.memory RPC method 'bots.memory.{other}'"
        ))),
    }
}

pub(super) async fn dispatch_bots_tidy_up(
    bots: &Arc<OrcaBotsFacade>,
    sub: &str,
    params: &Value,
) -> Result<Value> {
    let bot_id = req_str(params, &["botId", "bot_id", "id"])?;
    let bot = bots.controller().get_bot(bot_id).await?;
    match sub {
        "evaluateSchedule" | "evaluate_schedule" => {
            let sched_raw = opt_str(params, &["schedule"]).unwrap_or("night");
            let schedule = match sched_raw.to_ascii_lowercase().as_str() {
                "night" | "nightly" => TidyUpSchedule::Night,
                "week" | "weekly" => TidyUpSchedule::Week,
                other => {
                    return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                        "unknown tidyUp schedule '{other}'"
                    )));
                }
            };
            let now_val = params
                .get("localNow")
                .or_else(|| params.get("local_now"))
                .or_else(|| params.get("now"))
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'localNow'"))?;
            let now_ms = parse_rfc3339_or_ms(now_val)?;
            let since_ms = match params.get("since") {
                Some(v) if !v.is_null() => Some(parse_rfc3339_or_ms(v)?),
                _ => None,
            };
            let utc_offset = params
                .get("utcOffsetSeconds")
                .or_else(|| params.get("utc_offset_seconds"))
                .and_then(Value::as_i64)
                .unwrap_or(0) as i32;
            let allow_sleep = params
                .get("allowSleepCatchup")
                .or_else(|| params.get("allow_sleep_catchup"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let tmp_state = TidyUpRuntimeState {
                schedule,
                enabled: bot.agent.supports_tidy_up(),
                turned_off: false,
                since_ms,
                ..TidyUpRuntimeState::default()
            };
            let due_now = evaluate_tidy_up_due(&tmp_state, now_ms, utc_offset, allow_sleep)?;
            Ok(json!({ "botId": bot.id, "dueNow": due_now }))
        }
        "collectInputs" | "collect_inputs" => {
            let raw_turns = params
                .get("turns")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut kept: Vec<Value> = Vec::new();
            let mut seen_chats: HashSet<String> = HashSet::new();
            let mut total_chars = 0usize;
            for turn in raw_turns {
                let source = turn.get("source").and_then(Value::as_str).unwrap_or("");
                let author = turn.get("author").and_then(Value::as_str).unwrap_or(source);
                if !source.eq_ignore_ascii_case("owner") || !author.eq_ignore_ascii_case("owner") {
                    continue;
                }
                let chat_id = turn
                    .get("chatId")
                    .or_else(|| turn.get("chat_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("default")
                    .to_string();
                if !seen_chats.contains(&chat_id) && seen_chats.len() >= TIDY_UP_MAX_CHATS {
                    continue;
                }
                let text_len = turn
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|s| s.chars().count())
                    .unwrap_or(0);
                if total_chars.saturating_add(text_len) > TIDY_UP_MAX_TOTAL_CHARS {
                    break;
                }
                seen_chats.insert(chat_id);
                total_chars = total_chars.saturating_add(text_len);
                kept.push(turn);
            }
            Ok(json!({
                "botId": bot.id,
                "turns": kept,
                "chatsReviewed": seen_chats.len(),
                "totalChars": total_chars,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown bots.tidyUp RPC method 'bots.tidyUp.{other}'"
        ))),
    }
}

fn resolve_item_from_params(bots: &Arc<OrcaBotsFacade>, params: &Value) -> Result<orca_core::bot_items::BotItem> {
    if let Some(num) = params
        .get("number")
        .or_else(|| params.get("itemNumber"))
        .or_else(|| params.get("item_number"))
        .and_then(Value::as_u64)
    {
        return Ok(bots.items_store().get_item_by_number(num)?);
    }
    if let Some(s) = opt_str(params, &["item_uid", "itemUid", "uid", "number", "itemNum"]) {
        if let Ok(num) = s.trim().parse::<u64>() {
            return Ok(bots.items_store().get_item_by_number(num)?);
        }
        return Ok(bots.items_store().get_item_by_uid(s)?);
    }
    Err(OrcaIdeRuntimeError::invalid_argument(
        "missing required item identifier ('number' or 'item_uid')",
    ))
}

pub(super) async fn dispatch_bots_items(
    bots: &Arc<OrcaBotsFacade>,
    sub: &str,
    params: &Value,
    now_ms: i64,
) -> Result<Value> {
    match sub {
        "link" => {
            let item = resolve_item_from_params(bots, params)?;
            let link_val = params
                .get("link")
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'link' object"))?;
            let link = parse_flexible_item_link(link_val)?;
            let actor = params
                .get("actor")
                .cloned()
                .map(serde_json::from_value::<BotItemActor>)
                .transpose()?
                .unwrap_or(BotItemActor::Owner);
            let (updated, event) = bots.items_store().add_link(&item.uid, actor, link, now_ms)?;
            let deduped = bots.items_store().links_one_per_chain(&updated.uid)?;
            let bot = bots.controller().get_bot(&updated.owner_bot_id).await.ok();
            let stale_policy = bot.and_then(|b| b.items_stale).unwrap_or_default();
            let wire_links: Vec<Value> = deduped.iter().map(item_link_to_wire_json).collect();
            Ok(json!({
                "item": item_to_wire_json(&updated, stale_policy, Some(&deduped)),
                "event": event,
                "links": wire_links,
            }))
        }
        "show" | "get" => {
            let item = resolve_item_from_params(bots, params)?;
            let events = bots.items_store().list_events(&item.uid, None, None)?;
            let deduped = bots.items_store().links_one_per_chain(&item.uid)?;
            let bot = bots.controller().get_bot(&item.owner_bot_id).await.ok();
            let stale_policy = bot.and_then(|b| b.items_stale).unwrap_or_default();
            let item_json = item_to_wire_json(&item, stale_policy, Some(&deduped));
            let wire_links: Vec<Value> = deduped.iter().map(item_link_to_wire_json).collect();
            let fallback = item_json.get("plainTextFallback").cloned();
            let stale_days = item_json.get("staleAfterDays").cloned();
            Ok(json!({
                "item": item_json,
                "events": events,
                "links": wire_links,
                "links_deduped": wire_links,
                "ask": item.ask,
                "plainTextFallback": fallback,
                "staleAfterDays": stale_days,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown bots.items RPC method 'bots.items.{other}'"
        ))),
    }
}

pub(super) async fn dispatch_bots_look_around(
    bots: &Arc<OrcaBotsFacade>,
    sub: &str,
    params: &Value,
    now_ms: i64,
) -> Result<Value> {
    let bot_id = req_str(params, &["botId", "bot_id", "id"])?;
    let bot = bots.controller().get_bot(bot_id).await?;
    match sub {
        "configure" | "patch" => {
            ensure_look_around_agent(bot.agent)?;
            let every = match opt_str(params, &["every", "schedule"]) {
                Some(raw) => Some(LookAroundEvery::parse(raw).ok_or_else(|| {
                    OrcaIdeRuntimeError::invalid_argument(format!(
                        "unknown look-around schedule preset '{raw}'"
                    ))
                })?),
                None => None,
            };
            let since_ms = match params.get("since").or_else(|| params.get("sinceMs")) {
                Some(v) if !v.is_null() => Some(parse_rfc3339_or_ms(v)?),
                _ => None,
            };
            let on = params
                .get("on")
                .or_else(|| params.get("enabled"))
                .and_then(Value::as_bool)
                .or(Some(true));
            let weekdays_only = params
                .get("weekdaysOnly")
                .or_else(|| params.get("weekdays_only"))
                .and_then(Value::as_bool);
            let focus = params
                .get("focus")
                .map(|v| v.as_str().map(ToOwned::to_owned));
            let next = bots.up_next_store().patch_schedule(
                &bot.id,
                bot.agent,
                LookAroundPatch {
                    on,
                    every,
                    weekdays_only,
                    focus,
                    since_ms,
                },
                now_ms,
            )?;
            Ok(json!({
                "on": next.on,
                "every": next.schedule.every,
                "hours": next.schedule.every.hours(),
                "weekdaysOnly": next.schedule.weekdays_only,
                "focus": next.schedule.focus,
                "since": next.schedule.since_ms.and_then(format_ms_rfc3339),
                "schedule": next,
            }))
        }
        "evaluate" => {
            ensure_look_around_agent(bot.agent)?;
            let eval_now = match params.get("now").or_else(|| params.get("nowMs")) {
                Some(v) if !v.is_null() => parse_rfc3339_or_ms(v)?,
                _ => now_ms,
            };
            let utc_offset = params
                .get("utcOffsetSeconds")
                .or_else(|| params.get("utc_offset_seconds"))
                .and_then(Value::as_i64)
                .unwrap_or(0) as i32;
            let persisted = bots.up_next_store().get_state(&bot.id)?;
            let mut sched = persisted.schedule.schedule.clone();
            if let Some(since_val) = params.get("since").or_else(|| params.get("sinceMs"))
                && !since_val.is_null()
            {
                sched.since_ms = Some(parse_rfc3339_or_ms(since_val)?);
            }
            let eval = evaluate_look_around(&sched, eval_now, persisted.last_run_at_ms, utc_offset)?;
            Ok(json!({
                "dueNow": eval.due_now,
                "latestSlotMs": eval.latest_slot_ms,
                "nextSlotMs": eval.next_slot_ms,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown bots.lookAround RPC method 'bots.lookAround.{other}'"
        ))),
    }
}

pub(super) async fn dispatch_bots_up_next(
    bots: &Arc<OrcaBotsFacade>,
    sub: &str,
    params: &Value,
    now_ms: i64,
) -> Result<Value> {
    let bot_id = req_str(params, &["botId", "bot_id", "id"])?;
    let _bot = bots.controller().get_bot(bot_id).await?;
    let suggestion_id = req_str(params, &["suggestionId", "suggestion_id", "id"])?;
    match sub {
        "start" => {
            let started = bots.up_next_store().start_suggestion(bot_id, suggestion_id)?;
            Ok(json!({
                "suggestion": started.suggestion,
                "ask": started.composer_ask,
                "composerAsk": started.composer_ask,
            }))
        }
        "dismiss" => {
            let dismissed = bots
                .up_next_store()
                .dismiss_suggestion(bot_id, suggestion_id, now_ms)?;
            Ok(json!({ "dismissed": true, "suggestion": dismissed }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown bots.upNext RPC method 'bots.upNext.{other}'"
        ))),
    }
}

fn enrich_group_former_sessions(group: &orca_core::bot_groups::BotGroup) -> Value {
    let mut val = serde_json::to_value(group).unwrap_or_else(|_| json!({}));
    let mut combined_former: BTreeMap<String, Vec<String>> = group.former_session_ids.clone();
    for thr in &group.threads {
        for (bid, sids) in &thr.former_session_ids {
            let entry = combined_former.entry(bid.clone()).or_default();
            for sid in sids {
                if !entry.contains(sid) {
                    entry.push(sid.clone());
                }
            }
        }
    }
    if let Some(obj) = val.as_object_mut() {
        obj.insert("formerSessionIds".into(), json!(combined_former));
        obj.insert("remoteMembers".into(), json!(group.remote_members));
    }
    val
}

pub(super) async fn dispatch_bots_groups(
    bots: &Arc<OrcaBotsFacade>,
    sub: &str,
    params: &Value,
) -> Result<Value> {
    let group_id = req_str(params, &["groupId", "group_id", "id"])?;
    match sub {
        "update" => {
            let name = opt_str(params, &["name"]).map(ToOwned::to_owned);
            let reply_mode = params
                .get("replyMode")
                .or_else(|| params.get("reply_mode"))
                .cloned()
                .map(serde_json::from_value::<BotGroupReplyMode>)
                .transpose()?;
            let member_bot_ids = params
                .get("memberBotIds")
                .or_else(|| params.get("member_bot_ids"))
                .cloned()
                .map(serde_json::from_value::<Vec<String>>)
                .transpose()?;
            let remote_members = params
                .get("remoteMembers")
                .or_else(|| params.get("remote_members"))
                .cloned()
                .map(serde_json::from_value::<Vec<RemoteBotMember>>)
                .transpose()?;
            let updated = bots.group_runtime().update_group(
                group_id,
                UpdateBotGroupRequest {
                    name,
                    reply_mode,
                    member_bot_ids,
                    remote_members,
                },
            )?;
            Ok(enrich_group_former_sessions(&updated))
        }
        "read" | "get" => {
            let group = bots
                .group_runtime()
                .get_group(group_id)?
                .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("group '{group_id}' not found")))?;
            let mut all_msgs = bots.group_runtime().list_messages(group_id, None)?;
            for thr in &group.threads {
                all_msgs.extend(bots.group_runtime().list_messages(group_id, Some(&thr.id))?);
            }
            let pending = bots.group_runtime().list_pending_for_group(group_id)?;
            let ops = bots.group_runtime().list_operations(group_id)?;
            let mut val = enrich_group_former_sessions(&group);
            if let Some(obj) = val.as_object_mut() {
                obj.insert("messages".into(), serde_json::to_value(&all_msgs)?);
                obj.insert("pending".into(), serde_json::to_value(&pending)?);
                obj.insert("heldMail".into(), serde_json::to_value(&pending)?);
                obj.insert("turns".into(), serde_json::to_value(&ops)?);
                obj.insert("deliveries".into(), serde_json::to_value(&ops)?);
            }
            Ok(val)
        }
        "request" => {
            let from_bot_id = req_str(params, &["fromBotId", "from_bot_id"])?;
            let to_bot_id = req_str(params, &["toBotId", "to_bot_id"])?;
            let req_type = opt_str(params, &["type", "kind"]).unwrap_or("question");
            let text = req_str(params, &["text"])?;
            let thread_id = opt_str(params, &["threadId", "thread_id"]).map(ToOwned::to_owned);
            let group = bots
                .group_runtime()
                .get_group(group_id)?
                .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("group '{group_id}' not found")))?;

            let session_map = match thread_id.as_deref() {
                Some(tid) => &group
                    .threads
                    .iter()
                    .find(|t| t.id == tid)
                    .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("thread '{tid}' not found")))?
                    .session_ids,
                None => &group.session_ids,
            };
            let source_session_id = session_map
                .get(from_bot_id)
                .cloned()
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument(format!("bot '{from_bot_id}' is not an active member of group '{group_id}'")))?;
            let target_session_id = session_map
                .get(to_bot_id)
                .cloned()
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument(format!("bot '{to_bot_id}' is not an active member of group '{group_id}'")))?;

            let all_bots = bots.controller().list_bots().await;
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

            let existing_msgs = bots
                .group_runtime()
                .list_messages(group_id, thread_id.as_deref())?;
            let next_seq = existing_msgs.last().map(|m| m.sequence).unwrap_or(0).saturating_add(1);
            let root_request_id = format!("preq-{}", Uuid::new_v4());

            if req_type.eq_ignore_ascii_case("status") {
                let posted = bots.group_runtime().post_message(PostGroupMessageRequest {
                    group_id: group_id.to_string(),
                    thread_id,
                    sender: GroupMessageSender::Bot {
                        bot_id: from_bot_id.to_string(),
                    },
                    send: BotGroupSend {
                        text: text.to_string(),
                        images: Vec::new(),
                    },
                    active_local_candidates,
                    paused_bot_ids,
                    requester: None,
                    peer_scope: None,
                    synthetic_root: None,
                })?;
                return Ok(json!({
                    "message": posted.message,
                    "mailed": true,
                    "fireAndForget": true,
                    "status": "sent",
                }));
            }

            let peer_scope = BotGroupPeerScope {
                root_request_id: root_request_id.clone(),
                root_sequence: next_seq,
                source_session_id: source_session_id.clone(),
                target_session_id: target_session_id.clone(),
            };
            peer_scope.validate()?;
            let posted = bots.group_runtime().post_message(PostGroupMessageRequest {
                group_id: group_id.to_string(),
                thread_id,
                sender: GroupMessageSender::Bot {
                    bot_id: from_bot_id.to_string(),
                },
                send: BotGroupSend {
                    text: text.to_string(),
                    images: Vec::new(),
                },
                active_local_candidates,
                paused_bot_ids,
                requester: None,
                peer_scope: Some(peer_scope.clone()),
                synthetic_root: None,
            })?;
            Ok(json!({
                "message": posted.message,
                "peerScope": peer_scope,
                "rootRequestId": root_request_id,
                "rootSequence": next_seq,
                "sourceSessionId": source_session_id,
                "targetSessionId": target_session_id,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown bots.groups RPC method 'bots.groups.{other}'"
        ))),
    }
}
