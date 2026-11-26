//! Community tool execution backend for `ServerPlatformImBridge` (Hermes `PlatformIm` tools).

use crate::db::ServerDatabase;
use crate::routes::community::auth::{ensure_bot_actor_synced, sync_telegram_identities_for_user};
use crate::routes::community::bot_dispatch::{
    check_bot_comment_loop_guard_tx, dispatch_comment_bot_effects, dispatch_post_bot_effects,
    post_mention_text,
};
use crate::routes::community::lists_search::{
    execute_community_search, parse_search_sort, parse_search_tab,
};
use crate::routes::community::posts::{
    ensure_comment_publicly_interactable_conn, ensure_post_mod_target_allowed_conn,
    ensure_post_publicly_interactable_conn, is_post_publicly_interactable, load_post_detail_conn,
    sanitize_create_post_response_conn, sanitize_post_embedded_refs_conn,
    validate_create_post_references_conn,
};
use crate::routes::community::{
    authorized_media_user_ids_for_actor, broadcast_community_event_hub, collect_post_media_ids,
    sync_post_in_indexer, validate_community_media_access_db,
};
use crate::server::AppState;
use crate::ws::GatewayHub;
use chrono::Utc;
use gpui_telegram_agent::hermes::toolsets::PlatformImCaller;
use gpui_telegram_community::bot_runtime::BotLoopGuard;
use gpui_telegram_community::identity::{bot_actor_id, human_actor_id, validate_act_as};
use gpui_telegram_community::model::extract_entities;
use gpui_telegram_community::store::ops;
use gpui_telegram_community::CommunitySearchIndexer;
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::community_contracts::{
    ActorKind, CommentSort, CommunityActorDto, CommunitySearchRequestDto, CreateCommentRequest,
    CreatePostRequest, FeedQueryParams, FollowActionResponse, ModActionKind, ModActionRequest,
    PostFormat, PostStatus,
};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, OnceLock, Weak};

pub struct CommunityBridgeContext<'a> {
    pub db: &'a Arc<ServerDatabase>,
    pub gateway_hub: &'a Weak<GatewayHub>,
    pub community_search: &'a Weak<CommunitySearchIndexer>,
    pub community_bot_guard: &'a Weak<BotLoopGuard>,
    pub state_weak: &'a Arc<OnceLock<Weak<AppState>>>,
}

fn s_arg<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a str> {
    let obj = args.as_object()?;
    for k in keys {
        if let Some(s) = obj
            .get(*k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(s);
        }
    }
    None
}

fn i_arg(args: &Value, keys: &[&str]) -> Option<i64> {
    let obj = args.as_object()?;
    for k in keys {
        if let Some(v) = obj.get(*k) {
            if let Some(n) = v.as_i64() {
                return Some(n);
            }
            if let Some(s) = v.as_str().and_then(|s| s.trim().parse::<i64>().ok()) {
                return Some(s);
            }
        }
    }
    None
}

fn b_arg(args: &Value, keys: &[&str]) -> Option<bool> {
    let obj = args.as_object()?;
    for k in keys {
        if let Some(v) = obj.get(*k) {
            if let Some(b) = v.as_bool() {
                return Some(b);
            }
        }
    }
    None
}

/// Resolves the first-class `CommunityActorDto` for a `PlatformImCaller`:
/// - `user_id = Some(uid)` -> `"u:{uid}"` (or `"b:{uid}"` if `uid` is a bot user; optional owner-verified `act_as_actor_id`)
/// - `user_id = None, bot_id = Some(bid)` -> `"b:{bid}"` (auto-synced from `bots_registry`)
/// - `bound_agent_id = Some(aid)` -> matched bot `"b:{bot_user_id}"`
pub fn resolve_community_caller_actor(
    db: &ServerDatabase,
    caller: &PlatformImCaller,
    args: &Value,
    now_ms: i64,
) -> Result<CommunityActorDto, String> {
    if let Some(uid) = caller.user_id.filter(|&u| u > 0) {
        if let Ok(Some(bot)) = db.get_bot_by_user_id(uid as i64) {
            return ensure_bot_actor_synced(db, &bot, now_ms).map_err(|e| e.to_string());
        }
        let _ = sync_telegram_identities_for_user(db, UserId(uid), now_ms);
        if let Some(act_as) = s_arg(args, &["act_as_actor_id"]) {
            let target = db
                .pool()
                .with_reader(|conn| ops::get_actor_conn(conn, act_as, None))
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Requested act_as_actor_id '{act_as}' not found"))?;
            validate_act_as(uid as i64, &target).map_err(|e| e.to_string())?;
            if let Some(bid) = target
                .linked_user_id
                .filter(|_| target.actor_kind.is_bot_like() || target.actor_id.starts_with("b:"))
            {
                if let Ok(Some(bot)) = db.get_bot_by_user_id(bid) {
                    if bot.owner_user_id != uid as i64 {
                        return Err("Caller does not own requested bot actor".into());
                    }
                }
            }
            return Ok(target);
        }
        let hid = human_actor_id(uid as i64);
        return db
            .pool()
            .with_reader(|conn| ops::get_actor_conn(conn, &hid, None))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Human community actor '{hid}' was not synced"));
    }

    if let Some(bid) = caller.bot_id.filter(|&b| b > 0) {
        let cid = bot_actor_id(bid);
        if let Some(existing) = db
            .pool()
            .with_reader(|conn| ops::get_actor_conn(conn, &cid, None))
            .map_err(|e| e.to_string())?
        {
            return Ok(existing);
        }
        if let Some(bot) = db.get_bot_by_user_id(bid).map_err(|e| e.to_string())? {
            return ensure_bot_actor_synced(db, &bot, now_ms).map_err(|e| e.to_string());
        }
        return Err(format!(
            "Bot caller bot_id={bid} not found in bots_registry or community_actors"
        ));
    }

    if let Some(ref agent_id) = caller.bound_agent_id {
        if let Ok(bots) = db.list_all_bots() {
            if let Some(bot) = bots.into_iter().find(|b| b.agent_id == *agent_id) {
                return ensure_bot_actor_synced(db, &bot, now_ms).map_err(|e| e.to_string());
            }
        }
        if let Ok(Some(bot)) = db.get_bot_by_username(agent_id) {
            return ensure_bot_actor_synced(db, &bot, now_ms).map_err(|e| e.to_string());
        }
    }

    Err("Authenticated caller user_id or bot_id is required for Community operations".into())
}

fn enqueue_mention_updates_fallback(
    db: &ServerDatabase,
    text: &str,
    from_actor: &CommunityActorDto,
    post_id: &str,
    comment_id: Option<&str>,
    community_slug: Option<&str>,
    extra_reply_target_actor_id: Option<&str>,
    now_ms: i64,
) {
    let entities = extract_entities(text);
    let mut targets: Vec<(i64, &'static str)> = Vec::new();
    let mut seen_bot_ids: HashSet<i64> = HashSet::new();

    if let Some(reply_actor_id) = extra_reply_target_actor_id {
        if reply_actor_id != from_actor.actor_id {
            if let Ok(Some(target)) = db
                .pool()
                .with_reader(|conn| ops::get_actor_conn(conn, reply_actor_id, None))
            {
                if target.actor_kind.is_bot_like() {
                    if let Some(bid) = target.linked_user_id.filter(|&id| id > 0) {
                        if seen_bot_ids.insert(bid) {
                            targets.push((bid, "community_comment"));
                        }
                    }
                }
            }
        }
    }

    for mention in entities.mentions {
        if let Ok(Some(target)) = db
            .pool()
            .with_reader(|conn| ops::get_actor_conn(conn, &mention, None))
        {
            if target.actor_id != from_actor.actor_id && target.actor_kind.is_bot_like() {
                if let Some(bid) = target.linked_user_id.filter(|&id| id > 0) {
                    if seen_bot_ids.insert(bid) {
                        targets.push((bid, "community_mention"));
                    }
                }
            }
        }
    }

    for (bid, kind) in targets {
        let payload = json!({
            kind: {
                "post_id": post_id,
                "comment_id": comment_id,
                "community_slug": community_slug,
                "from_actor_id": from_actor.actor_id,
                "from_handle": from_actor.handle,
                "from_kind": from_actor.actor_kind.as_str(),
                "text": text,
                "timestamp_ms": now_ms,
            }
        });
        if let Ok(s) = serde_json::to_string(&payload) {
            let _ = db.enqueue_bot_api_update(bid, &s);
        }
    }
}

fn build_post_request_from_tool_args(
    args: &Value,
    force_quote: bool,
) -> Result<CreatePostRequest, String> {
    let mut obj = args
        .as_object()
        .cloned()
        .ok_or_else(|| "Tool arguments must be a JSON object".to_string())?;
    if !obj.contains_key("community_slug") {
        if let Some(c) = obj.get("community").cloned() {
            obj.insert("community_slug".to_string(), c);
        }
    }
    if force_quote {
        if !obj.contains_key("quoted_post_id") {
            if let Some(pid) = obj.get("post_id").cloned() {
                obj.insert("quoted_post_id".to_string(), pid);
            }
        }
        obj.insert("format".to_string(), json!("quote"));
    }
    let mut req: CreatePostRequest =
        serde_json::from_value(Value::Object(obj)).map_err(|e| format!("Invalid post args: {e}"))?;
    if force_quote {
        req.format = PostFormat::Quote;
        if req
            .quoted_post_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none()
        {
            return Err("quoted_post_id (or post_id) is required".into());
        }
    }
    Ok(req)
}

/// Dispatches all 9 first-class Community Hermes `PlatformIm` tools.
pub fn execute_community_im_tool(
    ctx: &CommunityBridgeContext<'_>,
    tool_name: &str,
    args: &Value,
    caller: &PlatformImCaller,
    now_ms: i64,
) -> Result<Value, String> {
    let actor = resolve_community_caller_actor(ctx.db, caller, args, now_ms)?;

    match tool_name {
        "community_browse_feed" => {
            let q = FeedQueryParams {
                tab: s_arg(args, &["tab"]).map(str::to_string),
                sort: s_arg(args, &["sort"]).map(str::to_string),
                time: s_arg(args, &["time"]).map(str::to_string),
                actor_filter: s_arg(args, &["actor_filter"]).map(str::to_string),
                community: s_arg(args, &["community", "community_slug"]).map(str::to_string),
                list_id: s_arg(args, &["list_id"]).map(str::to_string),
                actor: s_arg(args, &["actor"]).map(str::to_string),
                flair: s_arg(args, &["flair"]).map(str::to_string),
                cursor: s_arg(args, &["cursor"]).map(str::to_string),
                limit: i_arg(args, &["limit"]).map(|n| n.clamp(1, 50) as usize),
            };
            let req = q.into_feed_request();
            let resp = ctx
                .db
                .pool()
                .with_reader(|conn| {
                    let mut out = ops::get_feed_conn(conn, &actor.actor_id, &req, now_ms)?;
                    for item in &mut out.items {
                        sanitize_post_embedded_refs_conn(conn, &mut item.post);
                    }
                    Ok(out)
                })
                .map_err(|e| e.to_string())?;
            serde_json::to_value(resp).map_err(|e| e.to_string())
        }
        "community_search" => {
            let req = CommunitySearchRequestDto {
                query: s_arg(args, &["query", "q"]).unwrap_or("").to_string(),
                tab: s_arg(args, &["tab"])
                    .map(parse_search_tab)
                    .unwrap_or_default(),
                sort: s_arg(args, &["sort"])
                    .map(parse_search_sort)
                    .unwrap_or_default(),
                community_slug: s_arg(args, &["community_slug", "community"]).map(str::to_string),
                author_handle: s_arg(args, &["author_handle", "author"]).map(str::to_string),
                flair: s_arg(args, &["flair"]).map(str::to_string),
                actor_kind: s_arg(args, &["actor_kind"]).and_then(ActorKind::parse),
                has_media: b_arg(args, &["has_media"]),
                has_poll: b_arg(args, &["has_poll"]),
                limit: i_arg(args, &["limit"]).unwrap_or(25).clamp(1, 50) as usize,
                offset: i_arg(args, &["offset"]).unwrap_or(0).max(0) as usize,
            };
            let idx = ctx.community_search.upgrade();
            let resp = execute_community_search(
                ctx.db,
                idx.as_deref(),
                Some(&actor.actor_id),
                &req,
            )
            .map_err(|e| e.to_string())?;
            serde_json::to_value(resp).map_err(|e| e.to_string())
        }
        "community_get_post" => {
            let post_id = s_arg(args, &["post_id"])
                .ok_or_else(|| "post_id is required".to_string())?;
            let sort = s_arg(args, &["comment_sort", "sort"])
                .and_then(CommentSort::parse)
                .unwrap_or(CommentSort::Best);
            let max_depth = i_arg(args, &["max_depth"]).map(|d| d.clamp(1, 32) as u32);
            let limit = i_arg(args, &["comments_limit", "limit"])
                .unwrap_or(100)
                .clamp(1, 200) as usize;
            let resp = ctx
                .db
                .pool()
                .with_reader(|conn| {
                    load_post_detail_conn(conn, post_id, Some(&actor.actor_id), sort, max_depth, limit)
                })
                .map_err(|e| e.to_string())?;
            serde_json::to_value(resp).map_err(|e| e.to_string())
        }
        "community_create_post" | "community_quote_post" => {
            let force_quote = tool_name == "community_quote_post";
            let req = build_post_request_from_tool_args(args, force_quote)?;
            let media_ids = collect_post_media_ids(&req);
            let session_uid = caller.user_id.filter(|&u| u > 0).map(UserId);
            let allowed_uids = authorized_media_user_ids_for_actor(ctx.db, &actor, session_uid);
            if let Err(e) = validate_community_media_access_db(ctx.db, &allowed_uids, &media_ids) {
                return Err(format!("Forbidden: {e}"));
            }
            let created = ctx
                .db
                .pool()
                .with_transaction(|tx| {
                    validate_create_post_references_conn(tx, &req)?;
                    let mut created = ops::create_post_tx(tx, &actor.actor_id, &req, now_ms)?;
                    sanitize_create_post_response_conn(tx, &mut created);
                    Ok(created)
                })
                .map_err(|e| e.to_string())?;

            if created.post.status == PostStatus::Published && !created.post.is_removed {
                if let Some(idx) = ctx.community_search.upgrade() {
                    let _ = idx.index_post(&created.post);
                    for tp in &created.thread_posts {
                        if tp.status == PostStatus::Published && !tp.is_removed {
                            let _ = idx.index_post(tp);
                        }
                    }
                    let _ = idx.commit();
                }
                if !actor.actor_kind.is_bot_like() {
                    if let Some(guard) = ctx.community_bot_guard.upgrade() {
                        guard.reset_on_human_turn(&created.post.post_id);
                    }
                }
                if let Some(hub) = ctx.gateway_hub.upgrade() {
                    broadcast_community_event_hub(
                        &hub,
                        "post_created",
                        created.post.community_id.clone(),
                        Some(created.post.post_id.clone()),
                        None,
                        Some(actor.actor_id.clone()),
                        &json!({
                            "post_id": created.post.post_id,
                            "community_slug": created.post.community_slug,
                            "author_actor_id": actor.actor_id,
                            "via": "hermes_bridge",
                        }),
                    );
                }
                if let Some(state) = ctx.state_weak.get().and_then(|w| w.upgrade()) {
                    dispatch_post_bot_effects(&state, &created.post, &actor, now_ms);
                } else {
                    let mention_text = post_mention_text(&created.post);
                    enqueue_mention_updates_fallback(
                        ctx.db,
                        &mention_text,
                        &actor,
                        &created.post.post_id,
                        None,
                        created.post.community_slug.as_deref(),
                        None,
                        now_ms,
                    );
                }
            }
            serde_json::to_value(created).map_err(|e| e.to_string())
        }
        "community_comment" => {
            let post_id = s_arg(args, &["post_id"])
                .ok_or_else(|| "post_id is required".to_string())?;
            let body = s_arg(args, &["body"]).ok_or_else(|| "body is required".to_string())?;
            let parent_comment_id = s_arg(args, &["parent_comment_id"]).map(str::to_string);
            let is_mod_distinguished = b_arg(args, &["is_mod_distinguished"]).unwrap_or(false);

            let req = CreateCommentRequest {
                parent_comment_id: parent_comment_id.clone(),
                body: body.to_string(),
                media: Vec::new(),
                is_mod_distinguished,
            };
            let (post, created, reply_target_actor_id, tx_now_ms) = ctx
                .db
                .pool()
                .with_transaction(|tx| {
                    let tx_now_ms = Utc::now().timestamp_millis().max(now_ms);
                    let post = ops::get_post_conn(tx, post_id, Some(&actor.actor_id))?
                        .ok_or_else(|| TelegramError::NotFound(format!("Post '{post_id}' not found")))?;
                    if !is_post_publicly_interactable(&post) {
                        return Err(TelegramError::NotFound(format!("Post '{post_id}' not found")));
                    }
                    check_bot_comment_loop_guard_tx(
                        tx,
                        &actor,
                        post_id,
                        parent_comment_id.as_deref(),
                        false,
                    )?;
                    let reply_target = if let Some(pid) = parent_comment_id.as_deref() {
                        tx.query_row(
                            "SELECT author_actor_id FROM community_comments WHERE comment_id = ?1",
                            params![pid],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()
                        .map_err(|e| TelegramError::Database(e.to_string()))?
                    } else {
                        Some(post.author_actor_id.clone())
                    };
                    let created =
                        ops::create_comment_tx(tx, &actor.actor_id, post_id, &req, tx_now_ms)?;
                    Ok((post, created, reply_target, tx_now_ms))
                })
                .map_err(|e| e.to_string())?;

            if !actor.actor_kind.is_bot_like() {
                if let Some(guard) = ctx.community_bot_guard.upgrade() {
                    guard.reset_on_human_turn(&post.post_id);
                }
            }
            if let Some(hub) = ctx.gateway_hub.upgrade() {
                broadcast_community_event_hub(
                    &hub,
                    "comment_created",
                    post.community_id.clone(),
                    Some(post.post_id.clone()),
                    Some(created.comment.comment_id.clone()),
                    Some(actor.actor_id.clone()),
                    &json!({
                        "comment_id": created.comment.comment_id,
                        "post_id": post.post_id,
                        "author_actor_id": actor.actor_id,
                        "via": "hermes_bridge",
                    }),
                );
            }
            if let Some(state) = ctx.state_weak.get().and_then(|w| w.upgrade()) {
                dispatch_comment_bot_effects(
                    &state,
                    &post,
                    &created.comment,
                    &actor,
                    reply_target_actor_id.as_deref(),
                    tx_now_ms,
                );
            } else {
                enqueue_mention_updates_fallback(
                    ctx.db,
                    &created.comment.body,
                    &actor,
                    &post.post_id,
                    Some(&created.comment.comment_id),
                    post.community_slug.as_deref(),
                    reply_target_actor_id.as_deref(),
                    tx_now_ms,
                );
            }
            serde_json::to_value(created).map_err(|e| e.to_string())
        }
        "community_vote" => {
            let dir = i_arg(args, &["direction"])
                .filter(|&d| (-1..=1).contains(&d))
                .ok_or_else(|| "direction (-1, 0, or 1) is required".to_string())?;
            let (is_comment, target_id) = if let Some(cid) = s_arg(args, &["comment_id"]) {
                (true, cid)
            } else if let Some(pid) = s_arg(args, &["post_id"]) {
                (false, pid)
            } else if let Some(tid) = s_arg(args, &["target_id"]) {
                let is_c = s_arg(args, &["target_kind", "target_type"])
                    .is_some_and(|k| k.trim().eq_ignore_ascii_case("comment"));
                (is_c, tid)
            } else {
                return Err("post_id, comment_id, or target_id is required".into());
            };
            let resp = ctx
                .db
                .pool()
                .with_transaction(|tx| {
                    if is_comment {
                        let _ = ensure_comment_publicly_interactable_conn(tx, target_id)?;
                        ops::vote_comment_tx(tx, &actor.actor_id, target_id, dir as i8, now_ms)
                    } else {
                        let _ = ensure_post_publicly_interactable_conn(tx, target_id)?;
                        ops::vote_post_tx(tx, &actor.actor_id, target_id, dir as i8, now_ms)
                    }
                })
                .map_err(|e| e.to_string())?;
            if !is_comment {
                if let (Some(idx), Ok(Some(refreshed))) = (
                    ctx.community_search.upgrade(),
                    ctx.db
                        .pool()
                        .with_reader(|conn| ops::get_post_conn(conn, target_id, None)),
                ) {
                    sync_post_in_indexer(&idx, &refreshed);
                }
            }
            if let Some(hub) = ctx.gateway_hub.upgrade() {
                broadcast_community_event_hub(
                    &hub,
                    "vote_updated",
                    None,
                    (!is_comment).then(|| target_id.to_string()),
                    is_comment.then(|| target_id.to_string()),
                    Some(actor.actor_id),
                    &json!({ "target_id": resp.target_id, "score": resp.score }),
                );
            }
            serde_json::to_value(resp).map_err(|e| e.to_string())
        }
        "community_mod_action" => {
            let slug = s_arg(args, &["community_slug", "community"])
                .ok_or_else(|| "community_slug is required".to_string())?;
            let action = s_arg(args, &["action"])
                .and_then(ModActionKind::parse)
                .ok_or_else(|| "Valid moderation action is required".to_string())?;
            let target_kind = s_arg(args, &["target_kind"])
                .ok_or_else(|| "target_kind is required".to_string())?;
            let target_id = s_arg(args, &["target_id"])
                .ok_or_else(|| "target_id is required".to_string())?;
            let target_kind = target_kind.trim().to_ascii_lowercase();
            let req = ModActionRequest {
                action,
                target_kind: target_kind.clone(),
                target_id: target_id.to_string(),
                reason: s_arg(args, &["reason"]).map(str::to_string),
                flair: s_arg(args, &["flair"]).map(str::to_string),
                queue_item_id: s_arg(args, &["queue_item_id"]).map(str::to_string),
            };
            let entry = ctx
                .db
                .pool()
                .with_transaction(|tx| {
                    if target_kind == "post" {
                        ensure_post_mod_target_allowed_conn(tx, &req.target_id)?;
                    }
                    ops::apply_mod_action_tx(tx, slug, &actor.actor_id, &req, now_ms)
                })
                .map_err(|e| e.to_string())?;
            if target_kind == "post" {
                if let Some(idx) = ctx.community_search.upgrade() {
                    match req.action {
                        ModActionKind::Remove => {
                            let _ = idx.delete_post(&req.target_id);
                            let _ = idx.commit();
                        }
                        ModActionKind::Approve | ModActionKind::SetPostFlair => {
                            if let Ok(Some(post)) = ctx
                                .db
                                .pool()
                                .with_reader(|conn| ops::get_post_conn(conn, &req.target_id, None))
                            {
                                sync_post_in_indexer(&idx, &post);
                            }
                        }
                        _ => {}
                    }
                }
            }
            if let Some(hub) = ctx.gateway_hub.upgrade() {
                broadcast_community_event_hub(
                    &hub,
                    "mod_action",
                    Some(entry.community_id.clone()),
                    (target_kind == "post").then(|| req.target_id.clone()),
                    (target_kind == "comment").then(|| req.target_id.clone()),
                    Some(actor.actor_id),
                    &json!({
                        "log_id": entry.log_id,
                        "action": entry.action.as_str(),
                        "target_kind": entry.target_kind,
                        "target_id": entry.target_id,
                    }),
                );
            }
            serde_json::to_value(entry).map_err(|e| e.to_string())
        }
        "community_follow" => {
            let raw_target = s_arg(args, &["actor_id", "target_actor_id", "handle"])
                .ok_or_else(|| "actor_id, target_actor_id, or handle is required".to_string())?;
            let should_follow = match s_arg(args, &["action"]) {
                Some(a) if a.eq_ignore_ascii_case("unfollow") => false,
                Some(a) if a.eq_ignore_ascii_case("follow") => true,
                _ => b_arg(args, &["follow"]).unwrap_or(true),
            };
            let resp = ctx
                .db
                .pool()
                .with_transaction(|tx| {
                    let target = ops::get_actor_conn(tx, raw_target, None)?.ok_or_else(|| {
                        TelegramError::NotFound(format!("Actor '{raw_target}' not found"))
                    })?;
                    if should_follow {
                        ops::follow_actor_tx(tx, &actor.actor_id, &target.actor_id, now_ms)?;
                    } else {
                        ops::unfollow_actor_tx(tx, &actor.actor_id, &target.actor_id)?;
                    }
                    let refreshed =
                        ops::get_actor_conn(tx, &target.actor_id, Some(&actor.actor_id))?
                            .unwrap_or(target);
                    Ok(FollowActionResponse {
                        actor_id: refreshed.actor_id,
                        is_following: should_follow,
                        followers_count: refreshed.followers_count,
                    })
                })
                .map_err(|e| e.to_string())?;
            if let Some(hub) = ctx.gateway_hub.upgrade() {
                broadcast_community_event_hub(
                    &hub,
                    "follow_updated",
                    None,
                    None,
                    None,
                    Some(actor.actor_id),
                    &json!({
                        "target_actor_id": resp.actor_id,
                        "is_following": resp.is_following,
                        "followers_count": resp.followers_count,
                    }),
                );
            }
            serde_json::to_value(resp).map_err(|e| e.to_string())
        }
        other => Err(format!("Unsupported Community tool '{other}'")),
    }
}
