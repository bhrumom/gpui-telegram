//! Canonical dual-case (camelCase + snake_case) wire JSON serializers and flexible
//! request parsers shared across `orca_ide` Unified RPC and `gpui_telegram_server` REST routes.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use orca_core::bot_ecosystem::{
    BotChatTreeEntry, NativeChatBotCardKind, UsageLimitContinue,
};
use orca_core::bot_items::{
    AutomationLinkMeta, BotItem, BotItemLink, BotItemLinkKind, BotItemLinkMeta, BotItemStatus,
    DispatchLinkMeta, DispatchLiveState, PrLinkMeta, PrLinkState, UrlLinkMeta, bot_item_ask_text,
};
use orca_core::bot_memory::TidyUpDraft;
use orca_core::bot_preamble::soul_description;
use orca_core::bot_types::{BotItemsStale, OrcaBotRecord};
use orca_core::BotErrorCode;
use serde_json::{Map, Value, json};

use crate::ecosystem_handover::BotChatTreeView;
use crate::home_store::BotHomeStore;
use crate::{OrcaBotsError, Result};

/// Formats UTC epoch milliseconds as an ISO-8601 / RFC-3339 timestamp ending in `Z`.
pub fn format_ms_rfc3339(ms: i64) -> Option<String> {
    let dt = DateTime::<Utc>::from_timestamp_millis(ms)?;
    if ms.rem_euclid(1000) == 0 {
        Some(dt.to_rfc3339_opts(SecondsFormat::Secs, true))
    } else {
        Some(dt.to_rfc3339_opts(SecondsFormat::Millis, true))
    }
}

/// Parses an integer epoch ms, RFC-3339 string, or naive `YYYY-MM-DDTHH:MM:SS` timestamp into ms.
pub fn parse_rfc3339_or_ms(val: &Value) -> Result<i64> {
    if let Some(n) = val.as_i64() {
        return Ok(n);
    }
    if let Some(s) = val.as_str() {
        return parse_timestamp_str_ms(s);
    }
    Err(OrcaBotsError::bot(
        BotErrorCode::InvalidArgument,
        "Expected integer epoch ms or ISO-8601 timestamp string.",
    ))
}

/// Parses an RFC-3339 or naive ISO-8601 `YYYY-MM-DDTHH:MM:SS` string into epoch ms.
pub fn parse_timestamp_str_ms(raw: &str) -> Result<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(OrcaBotsError::bot(
            BotErrorCode::InvalidArgument,
            "Timestamp string cannot be empty.",
        ));
    }
    if let Ok(n) = trimmed.parse::<i64>() {
        return Ok(n);
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(dt.timestamp_millis());
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S") {
        return Ok(naive.and_utc().timestamp_millis());
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S%.f") {
        return Ok(naive.and_utc().timestamp_millis());
    }
    Err(OrcaBotsError::bot(
        BotErrorCode::InvalidArgument,
        format!("Invalid ISO-8601 timestamp '{trimmed}'."),
    ))
}

/// Serializes an [`OrcaBotRecord`] to canonical wire JSON carrying both camelCase and snake_case
/// fields, explicit defaulted `itemsStale` (`5`) and `hostMcpServers` (`false`), `homePath`,
/// avatar CAS metadata, and `soulDescription`.
pub fn bot_record_to_wire_json(bot: &OrcaBotRecord, home_store: &BotHomeStore) -> Value {
    let mut val = serde_json::to_value(bot).unwrap_or_else(|_| json!({}));
    let home_dir = home_store.bot_dir(&bot.folder);
    let soul_md = home_store
        .read_text_optional(&home_dir.join("SOUL.md"))
        .ok()
        .flatten()
        .unwrap_or_default();
    let soul_desc = soul_description(&soul_md).unwrap_or_default();
    let has_avatar_png = home_dir.join("avatar.png").is_file();
    let has_custom_avatar = has_avatar_png || bot.avatar_spec.generated_over_mascot.is_some();
    let items_stale = bot.items_stale.unwrap_or_default();
    let host_mcp = bot.connectors.uses_host_mcp_servers();

    if let Some(obj) = val.as_object_mut() {
        obj.insert("hostId".into(), json!(bot.host_id));
        obj.insert("homePath".into(), json!(home_dir.to_string_lossy().to_string()));
        obj.insert("hostMcpServers".into(), json!(host_mcp));
        obj.insert("host_mcp_servers".into(), json!(host_mcp));
        obj.insert("itemsStale".into(), json!(items_stale));
        obj.insert("items_stale".into(), json!(items_stale));
        obj.insert("avatarRevision".into(), json!(bot.avatar_spec.avatar_revision));
        obj.insert("avatarRevisionEpoch".into(), json!(bot.avatar_spec.avatar_revision_epoch));
        if let Some(ref c) = bot.avatar_spec.avatar_color {
            obj.insert("avatarColor".into(), json!(c));
        }
        if let Some(ref o) = bot.avatar_spec.avatar_outfit {
            obj.insert("avatarOutfit".into(), json!(o));
        }
        obj.insert("hasCustomAvatar".into(), json!(has_custom_avatar));
        if has_avatar_png {
            obj.insert("avatarFile".into(), json!("avatar.png"));
        }
        if let Some(ref g) = bot.avatar_spec.generated_over_mascot {
            obj.insert("generatedOverMascot".into(), json!(g));
        }
        obj.insert("firstRun".into(), json!(bot.first_run));
        if let Some(pm) = bot.permission_mode {
            obj.insert("permissionMode".into(), json!(pm));
        }
        if let Some(ref sp) = bot.skill_projects {
            obj.insert("skillProjects".into(), json!(sp));
        }
        if let Some(so) = bot.sidebar_order {
            obj.insert("sidebarOrder".into(), json!(so));
        }
        if let Some(ref cs) = bot.chat_session_id {
            obj.insert("chatSessionId".into(), json!(cs));
        }
        if let Some(ref la) = bot.look_around {
            obj.insert("lookAround".into(), json!(la));
        }
        if let Some(ref tu) = bot.tidy_up {
            obj.insert("tidyUp".into(), json!(tu));
        }
        if let Some(ref tid) = bot.template_id {
            obj.insert("templateId".into(), json!(tid));
        }
        if let Some(ref hw) = bot.handed_paired_workspaces {
            obj.insert("handedPairedWorkspaces".into(), json!(hw));
        }
        obj.insert("createdAt".into(), json!(bot.created_at));
        obj.insert("updatedAt".into(), json!(bot.updated_at));
        obj.insert("soulDescription".into(), json!(soul_desc));
        obj.insert("notifications".into(), json!(bot.notifications));
    }
    val
}

/// Parses a `BotItemLink` accepting both nested `meta: {"type": ...}` and flat fields
/// (`chain_id`, `parent_dispatch_id`, `state`, `active`, `title`).
pub fn parse_flexible_item_link(val: &Value) -> Result<BotItemLink> {
    let Some(obj) = val.as_object() else {
        return Err(OrcaBotsError::bot(
            BotErrorCode::InvalidArgument,
            "Item link must be a JSON object.",
        ));
    };
    if obj.contains_key("meta") {
        let link: BotItemLink = serde_json::from_value(val.clone())?;
        link.validate()?;
        return Ok(link);
    }
    let kind_raw = obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| OrcaBotsError::bot(BotErrorCode::InvalidArgument, "Item link missing 'kind'."))?;
    let kind = BotItemLinkKind::parse(kind_raw).ok_or_else(|| {
        OrcaBotsError::bot(
            BotErrorCode::InvalidArgument,
            format!("Unknown item link kind '{kind_raw}'."),
        )
    })?;
    let reference = obj
        .get("ref")
        .or_else(|| obj.get("reference"))
        .and_then(Value::as_str)
        .ok_or_else(|| OrcaBotsError::bot(BotErrorCode::InvalidArgument, "Item link missing 'ref'."))?
        .to_string();
    let added_event = obj
        .get("added_event")
        .or_else(|| obj.get("addedEvent"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let meta = match kind {
        BotItemLinkKind::Dispatch => {
            let state = obj
                .get("state")
                .cloned()
                .map(serde_json::from_value::<DispatchLiveState>)
                .transpose()?;
            BotItemLinkMeta::Dispatch(DispatchLinkMeta {
                chain_id: obj
                    .get("chain_id")
                    .or_else(|| obj.get("chainId"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                parent_dispatch_id: obj
                    .get("parent_dispatch_id")
                    .or_else(|| obj.get("parentDispatchId"))
                    .or_else(|| obj.get("parentDispatch"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                latest_dispatch_id: obj
                    .get("latest_dispatch_id")
                    .or_else(|| obj.get("latestDispatchId"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                state,
                ..DispatchLinkMeta::default()
            })
        }
        BotItemLinkKind::Pr => {
            let state = obj
                .get("state")
                .cloned()
                .map(serde_json::from_value::<PrLinkState>)
                .transpose()?;
            BotItemLinkMeta::Pr(PrLinkMeta {
                state,
                title: obj.get("title").and_then(Value::as_str).map(ToOwned::to_owned),
                ..PrLinkMeta::default()
            })
        }
        BotItemLinkKind::Url => BotItemLinkMeta::Url(UrlLinkMeta::default()),
        BotItemLinkKind::File => BotItemLinkMeta::File,
        BotItemLinkKind::Automation => BotItemLinkMeta::Automation(AutomationLinkMeta {
            active: obj.get("active").and_then(Value::as_bool).unwrap_or(true),
        }),
    };
    let link = BotItemLink {
        kind,
        reference,
        added_event,
        meta,
    };
    link.validate()?;
    Ok(link)
}

/// Serializes a [`BotItemLink`] with flattened metadata fields alongside `"meta"`.
pub fn item_link_to_wire_json(link: &BotItemLink) -> Value {
    let mut val = serde_json::to_value(link).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        match &link.meta {
            BotItemLinkMeta::Dispatch(d) => {
                if let Some(ref c) = d.chain_id {
                    obj.insert("chain_id".into(), json!(c));
                    obj.insert("chainId".into(), json!(c));
                }
                if let Some(ref p) = d.parent_dispatch_id {
                    obj.insert("parent_dispatch_id".into(), json!(p));
                    obj.insert("parentDispatchId".into(), json!(p));
                }
                if let Some(ref l) = d.latest_dispatch_id {
                    obj.insert("latest_dispatch_id".into(), json!(l));
                }
                if let Some(st) = d.state {
                    obj.insert("state".into(), json!(st));
                }
            }
            BotItemLinkMeta::Pr(pr) => {
                if let Some(st) = pr.state {
                    obj.insert("state".into(), json!(st));
                }
                if let Some(ref t) = pr.title {
                    obj.insert("title".into(), json!(t));
                }
            }
            BotItemLinkMeta::Automation(a) => {
                obj.insert("active".into(), json!(a.active));
            }
            BotItemLinkMeta::Url(_) | BotItemLinkMeta::File | BotItemLinkMeta::None => {}
        }
    }
    val
}

/// Serializes a [`BotItem`] with canonical `closed_at`, `snoozed_until`, `plainTextFallback`,
/// `staleAfterDays`, and flattened wire links.
pub fn item_to_wire_json(
    item: &BotItem,
    stale_policy: BotItemsStale,
    deduped_links: Option<&[BotItemLink]>,
) -> Value {
    let mut val = serde_json::to_value(item).unwrap_or_else(|_| json!({}));
    let links_slice = deduped_links.unwrap_or(&item.links);
    let wire_links: Vec<Value> = links_slice.iter().map(item_link_to_wire_json).collect();
    let stale_after_days = stale_policy.days().map(|base| {
        if item.status == BotItemStatus::Planned {
            base.saturating_mul(3)
        } else {
            base
        }
    });
    if let Some(obj) = val.as_object_mut() {
        obj.insert("links".into(), Value::Array(wire_links));
        if let Some(closed_ms) = item.closed_at_ms
            && let Some(iso) = format_ms_rfc3339(closed_ms)
        {
            obj.insert("closed_at".into(), json!(iso));
        }
        if let Some(snooze_ms) = item.snoozed_until_ms
            && let Some(iso) = format_ms_rfc3339(snooze_ms)
        {
            obj.insert("snoozed_until".into(), json!(iso));
        }
        if let Some(ref ask) = item.ask {
            obj.insert("plainTextFallback".into(), json!(bot_item_ask_text(ask)));
        }
        if let Some(days) = stale_after_days {
            obj.insert("staleAfterDays".into(), json!(days));
        }
    }
    val
}

/// Parses either a raw [`TidyUpDraft`] (`{index, topics, replaced, merged}`) or a structured
/// consolidation draft (`{learned, filed: [{topic, lines}], replaced: [{old, new}], merged: [{sources, into}]}`)
/// deterministically against live `before_md` so all 16 core Tidy-Up validators run identically.
pub fn parse_tidy_up_draft_value(val: &Value, before_md: &str) -> Result<(TidyUpDraft, bool)> {
    let Some(obj) = val.as_object() else {
        return Err(OrcaBotsError::bot(
            BotErrorCode::InvalidArgument,
            "Tidy-up draft must be a JSON object.",
        ));
    };
    if obj.contains_key("index") {
        return Ok((serde_json::from_value::<TidyUpDraft>(val.clone())?, false));
    }
    if let Some(next_content) = obj
        .get("nextContent")
        .or_else(|| obj.get("next_content"))
        .and_then(Value::as_str)
    {
        return Ok((
            TidyUpDraft {
                index: next_content.to_string(),
                topics: BTreeMap::new(),
                replaced: Vec::new(),
                merged: Vec::new(),
            },
            true,
        ));
    }

    let mut lines: Vec<String> = if before_md.trim().is_empty() {
        vec!["# Memory".to_string(), String::new()]
    } else {
        before_md.lines().map(ToOwned::to_owned).collect()
    };
    let mut removed_set: HashSet<String> = HashSet::new();
    let mut replaced: Vec<String> = Vec::new();
    let mut merged: Vec<String> = Vec::new();
    let mut topics: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut appended: Vec<String> = Vec::new();

    if let Some(arr) = obj.get("replaced").and_then(Value::as_array) {
        for item in arr {
            if let (Some(old), Some(new)) = (
                item.get("old").and_then(Value::as_str),
                item.get("new").and_then(Value::as_str),
            ) {
                removed_set.insert(old.trim().to_string());
                replaced.push(old.trim().to_string());
                appended.push(new.trim().to_string());
            }
        }
    }
    if let Some(arr) = obj.get("merged").and_then(Value::as_array) {
        for item in arr {
            if let Some(sources) = item.get("sources").and_then(Value::as_array) {
                for src in sources {
                    if let Some(s) = src.as_str() {
                        removed_set.insert(s.trim().to_string());
                        merged.push(s.trim().to_string());
                    }
                }
            }
            if let Some(into) = item.get("into").and_then(Value::as_str) {
                appended.push(into.trim().to_string());
            }
        }
    }
    if let Some(arr) = obj.get("filed").and_then(Value::as_array) {
        for item in arr {
            let Some(topic) = item.get("topic").and_then(Value::as_str) else {
                continue;
            };
            let entry_lines: Vec<String> = item
                .get("lines")
                .and_then(Value::as_array)
                .map(|ls| {
                    ls.iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.trim().to_string())
                        .collect()
                })
                .unwrap_or_default();
            for l in &entry_lines {
                removed_set.insert(l.clone());
            }
            topics
                .entry(topic.trim().to_string())
                .or_default()
                .extend(entry_lines);
        }
    }
    if let Some(arr) = obj.get("learned").and_then(Value::as_array) {
        for item in arr {
            if let Some(s) = item.as_str() {
                appended.push(s.trim().to_string());
            }
        }
    }

    lines.retain(|l| !removed_set.contains(l.trim()));
    for line in appended {
        if !line.is_empty() {
            lines.push(line);
        }
    }
    let mut index = lines.join("\n");
    if !index.ends_with('\n') {
        index.push('\n');
    }
    Ok((
        TidyUpDraft {
            index,
            topics,
            replaced,
            merged,
        },
        true,
    ))
}

fn chat_entry_to_wire(entry: &BotChatTreeEntry) -> Value {
    let children: Vec<Value> = entry.children.iter().map(chat_entry_to_wire).collect();
    let workers: Vec<Value> = entry
        .children
        .iter()
        .filter(|c| c.node.kind == orca_core::bot_ecosystem::BotChatNodeKind::Worker)
        .map(chat_entry_to_wire)
        .collect();
    json!({
        "id": entry.node.session_id,
        "sessionId": entry.node.session_id,
        "session_id": entry.node.session_id,
        "parentSessionId": entry.node.parent_session_id,
        "title": entry.node.title,
        "kind": entry.node.kind,
        "state": entry.node.live_state,
        "live_state": entry.node.live_state,
        "note": entry.node.note,
        "unread": entry.node.unread,
        "needsYou": entry.node.needs_you,
        "updatedAtMs": entry.node.updated_at_ms,
        "children": children,
        "workers": workers,
    })
}

/// Serializes a [`BotChatTreeView`] with canonical `botId`, `chats`, `homeChat`,
/// `supportedCardKinds` (all 10), `usageLimitStatus`, `usageLimitContinue`, and
/// `handedPairedWorkspaces`.
pub fn chat_tree_to_wire_json(
    bot: &OrcaBotRecord,
    view: &BotChatTreeView,
    usage_limit: Option<&UsageLimitContinue>,
) -> Value {
    let mut val = serde_json::to_value(view).unwrap_or_else(|_| json!({}));
    let chats: Vec<Value> = view.tree.roots.iter().map(chat_entry_to_wire).collect();
    let home_chat = chats
        .iter()
        .find(|c| c.get("kind").and_then(Value::as_str) == Some("home"))
        .cloned()
        .or_else(|| chats.first().cloned());
    let supported_cards: Vec<&'static str> = NativeChatBotCardKind::ALL
        .iter()
        .map(|k| k.as_str())
        .collect();
    let ul_json = usage_limit.map(|u| {
        json!({
            "enabled": u.auto_continue_enabled,
            "autoContinue": u.auto_continue_enabled,
            "provider": u.provider,
            "resetsAt": format_ms_rfc3339(u.reset_at_ms),
            "resetAtMs": u.reset_at_ms,
            "pausedAtMs": u.paused_at_ms,
        })
    });
    let ul_status = view.usage_limit_notice.clone().or_else(|| {
        usage_limit.map(|u| {
            let when = format_ms_rfc3339(u.reset_at_ms).unwrap_or_default();
            if u.auto_continue_enabled {
                format!("Continues at {when}")
            } else {
                format!("Hit limit · resets at {when}")
            }
        })
    });
    if let Some(obj) = val.as_object_mut() {
        obj.insert("botId".into(), json!(bot.id));
        obj.insert("chats".into(), Value::Array(chats));
        if let Some(hc) = home_chat {
            obj.insert("homeChat".into(), hc);
        }
        obj.insert("supportedCardKinds".into(), json!(supported_cards));
        if let Some(st) = ul_status {
            obj.insert("usageLimitStatus".into(), json!(st));
        }
        if let Some(ul) = ul_json {
            obj.insert("usageLimitContinue".into(), ul);
        }
        obj.insert(
            "handedPairedWorkspaces".into(),
            json!(bot.handed_paired_workspaces.clone().unwrap_or_default()),
        );
    }
    val
}

/// Parses a `usageLimitContinue` JSON object (`{enabled, resetsAt | resetAtMs, provider?, hitAtMs?}`).
pub fn parse_usage_limit_continue(val: &Value, default_provider: &str, now_ms: i64) -> Result<Option<UsageLimitContinue>> {
    if val.is_null() {
        return Ok(None);
    }
    let Some(obj) = val.as_object() else {
        return Err(OrcaBotsError::bot(
            BotErrorCode::InvalidArgument,
            "usageLimitContinue must be a JSON object or null.",
        ));
    };
    let enabled = obj
        .get("enabled")
        .or_else(|| obj.get("autoContinue"))
        .or_else(|| obj.get("auto_continue"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let provider = obj
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or(default_provider);
    let reset_at_ms = match obj
        .get("resetsAt")
        .or_else(|| obj.get("resets_at"))
        .or_else(|| obj.get("resetAtMs"))
        .or_else(|| obj.get("reset_at_ms"))
    {
        Some(v) => parse_rfc3339_or_ms(v)?,
        None => now_ms.saturating_add(3_600_000),
    };
    let hit_at_ms = obj
        .get("hitAtMs")
        .or_else(|| obj.get("hit_at_ms"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| now_ms.min(reset_at_ms.saturating_sub(1_000)));
    let limit = UsageLimitContinue::new(provider, hit_at_ms, reset_at_ms, enabled)?;
    Ok(Some(limit))
}

/// Helper to build a JSON object merging `base` with `extra` entries.
pub fn merge_json_fields(mut base: Value, extra: Map<String, Value>) -> Value {
    if let Some(obj) = base.as_object_mut() {
        for (k, v) in extra {
            obj.insert(k, v);
        }
    }
    base
}
