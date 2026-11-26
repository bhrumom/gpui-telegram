//! Community Bot reactive turns, webhook/update dispatch, 5-hop anti-loop guard, and 5s background scheduler.

use crate::routes::community::{broadcast_community_event, sync_post_in_search};
use crate::server::AppState;
use chrono::Utc;
use gpui_telegram_agent::{evaluate_outbound_turn_review, OutboundTurnReviewInput};
use gpui_telegram_community::bot_runtime::{
    BotLoopGuard, BotWakeSelector, DeterministicBotResponder, DEFAULT_BOT_DUPLICATE_THRESHOLD,
    DEFAULT_BOT_MAX_LOOP_HOPS,
};
use gpui_telegram_community::model::extract_entities;
use gpui_telegram_community::store::ops;
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_proto::community_contracts::{
    ActorKind, BotAutonomyMode, CommentSort, CommunityActorDto, CommunityCommentDto,
    CommunityPostDto, CreateCommentRequest, PostStatus, SubCommunityDto,
};
use rusqlite::{params, OptionalExtension};
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;

fn check_bot_comment_loop_guard_conn(
    conn: &rusqlite::Connection,
    bot_actor: &CommunityActorDto,
    post_id: &str,
    parent_comment_id: Option<&str>,
    explicit_human_summon: bool,
) -> Result<(), TelegramError> {
    if !bot_actor.actor_kind.is_bot_like() || explicit_human_summon {
        return Ok(());
    }

    let max_hops = DEFAULT_BOT_MAX_LOOP_HOPS as i64;
    let parent_hops: i64 = if let Some(pid) =
        parent_comment_id.map(str::trim).filter(|s| !s.is_empty())
    {
        conn.query_row(
            "SELECT bot_hop_count FROM community_comments WHERE comment_id = ?1 AND post_id = ?2",
            params![pid, post_id],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| TelegramError::Database(e.to_string()))?
        .unwrap_or(0)
    } else {
        0
    };

    // Also count consecutive bot comments at the tail of this post (blocks flat bot-to-bot ping-pong)
    let mut stmt = conn
        .prepare(
            "SELECT a.actor_kind
             FROM community_comments c
             JOIN community_actors a ON a.actor_id = c.author_actor_id
             WHERE c.post_id = ?1 AND c.is_removed = 0 AND c.is_deleted = 0
             ORDER BY c.created_at_ms DESC, c.rowid DESC
             LIMIT 8",
        )
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    let rows = stmt
        .query_map(params![post_id], |r| r.get::<_, String>(0))
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    let mut consecutive_bots = 0usize;
    for r in rows {
        let kind_str = r.map_err(|e| TelegramError::Database(e.to_string()))?;
        if ActorKind::parse(&kind_str).is_some_and(ActorKind::is_bot_like) {
            consecutive_bots += 1;
        } else {
            break;
        }
    }

    if parent_hops >= max_hops || consecutive_bots >= DEFAULT_BOT_MAX_LOOP_HOPS as usize {
        return Err(TelegramError::Validation(format!(
            "bot_loop_budget_exceeded: maximum {} consecutive bot hops reached on post '{}'",
            DEFAULT_BOT_MAX_LOOP_HOPS, post_id
        )));
    }
    Ok(())
}

/// Authoritative write-transaction 5-hop anti-loop check (zero TOCTOU under `BEGIN IMMEDIATE`).
pub(crate) fn check_bot_comment_loop_guard_tx(
    tx: &rusqlite::Transaction<'_>,
    bot_actor: &CommunityActorDto,
    post_id: &str,
    parent_comment_id: Option<&str>,
    explicit_human_summon: bool,
) -> Result<(), TelegramError> {
    check_bot_comment_loop_guard_conn(tx, bot_actor, post_id, parent_comment_id, explicit_human_summon)
}

/// Read-only fast-fail pre-check for the 5-hop anti-loop ceiling before async LLM/reply work.
/// Authoritative enforcement MUST ALSO run inside the write transaction via `check_bot_comment_loop_guard_tx`.
pub fn check_bot_comment_loop_guard(
    state: &AppState,
    bot_actor: &CommunityActorDto,
    post_id: &str,
    parent_comment_id: Option<&str>,
    explicit_human_summon: bool,
) -> Result<(), TelegramError> {
    if !bot_actor.actor_kind.is_bot_like() || explicit_human_summon {
        return Ok(());
    }
    state.db.pool().with_reader(|conn| {
        check_bot_comment_loop_guard_conn(
            conn,
            bot_actor,
            post_id,
            parent_comment_id,
            explicit_human_summon,
        )
    })
}

/// Combines post title and body for `@mention` scanning and wake context.
pub(crate) fn post_mention_text(post: &CommunityPostDto) -> String {
    match post.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => format!("{t}\n{}", post.body),
        None => post.body.clone(),
    }
}

/// Idempotently backfills `community_notifications` (`kind = 'mention'`) for `@mentioned` actors
/// when a `Draft` or `Scheduled` post transitions to `Published` (matching `create_post_tx`).
pub(crate) fn backfill_post_mention_notifications_tx(
    tx: &rusqlite::Transaction<'_>,
    post: &CommunityPostDto,
    now_ms: i64,
) -> Result<(), TelegramError> {
    if post.status != PostStatus::Published || post.is_removed {
        return Ok(());
    }
    let full_text = post_mention_text(post);
    let entities = extract_entities(&full_text);
    if entities.mentions.is_empty() {
        return Ok(());
    }
    let preview = format!("@{} mentioned you in a post.", post.author_handle);
    let mut seen_recipients: HashSet<String> = HashSet::new();
    for mention_handle in entities.mentions {
        let Some(mentioned) = ops::get_actor_conn(tx, &mention_handle, None)? else {
            continue;
        };
        if mentioned.actor_id == post.author_actor_id
            || mentioned.actor_id.is_empty()
            || !seen_recipients.insert(mentioned.actor_id.clone())
        {
            continue;
        }
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM community_notifications
                    WHERE recipient_actor_id = ?1
                      AND actor_id = ?2
                      AND target_post_id = ?3
                      AND target_comment_id IS NULL
                      AND kind = 'mention'
                )",
                params![&mentioned.actor_id, &post.author_actor_id, &post.post_id],
                |r| r.get(0),
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        if exists {
            continue;
        }
        let notif_id = format!("notif_{}", uuid::Uuid::new_v4().simple());
        tx.execute(
            "INSERT INTO community_notifications (
                notif_id, recipient_actor_id, actor_id, kind,
                target_post_id, target_comment_id, preview_text, is_read, created_at_ms
             ) VALUES (?1, ?2, ?3, 'mention', ?4, NULL, ?5, 0, ?6)",
            params![
                notif_id,
                &mentioned.actor_id,
                &post.author_actor_id,
                &post.post_id,
                &preview,
                now_ms,
            ],
        )
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    }
    Ok(())
}

/// Recursively flattens a nested comment tree, skipping removed/deleted tombstones while still
/// descending into their active children.
pub(crate) fn flatten_comment_tree(
    nodes: &[CommunityCommentDto],
    out: &mut Vec<CommunityCommentDto>,
) {
    for node in nodes {
        if !node.is_removed && !node.is_deleted {
            let mut flat = node.clone();
            flat.children.clear();
            out.push(flat);
        }
        if !node.children.is_empty() {
            flatten_comment_tree(&node.children, out);
        }
    }
}

/// Filters out suspended bots and bots banned/muted in `community` (plus short-circuits if `!allow_bots`)
/// BEFORE `BotWakeSelector` runs so ineligible bots never starve eligible bots.
pub(crate) fn filter_eligible_bots_for_community_conn(
    conn: &rusqlite::Connection,
    bots: Vec<CommunityActorDto>,
    community: Option<&SubCommunityDto>,
) -> Result<Vec<CommunityActorDto>, TelegramError> {
    if community.is_some_and(|c| !c.allow_bots) {
        return Ok(Vec::new());
    }
    let mut excluded: HashSet<String> = HashSet::new();
    {
        let mut stmt = conn
            .prepare("SELECT actor_id FROM community_actors WHERE is_suspended != 0")
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        for r in rows {
            excluded.insert(r.map_err(|e| TelegramError::Database(e.to_string()))?);
        }
    }
    if let Some(comm) = community {
        let mut stmt = conn
            .prepare(
                "SELECT actor_id FROM community_memberships
                 WHERE community_id = ?1 AND role IN ('banned', 'muted')",
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![comm.community_id], |r| r.get::<_, String>(0))
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        for r in rows {
            excluded.insert(r.map_err(|e| TelegramError::Database(e.to_string()))?);
        }
    }
    Ok(bots
        .into_iter()
        .filter(|b| !excluded.contains(&b.actor_id))
        .collect())
}

/// Read-only cooldown pre-flight matching `ops::check_and_record_bot_cooldown_tx` with zero writes.
fn is_bot_cooldown_available_conn(
    conn: &rusqlite::Connection,
    bot_actor_id: &str,
    scope_key: &str,
    min_cooldown_ms: i64,
    max_per_hour: i64,
    now_ms: i64,
) -> Result<bool, TelegramError> {
    if max_per_hour <= 0 {
        return Ok(false);
    }
    let min_cooldown_ms = min_cooldown_ms.max(0);
    let existing: Option<(i64, i64, i64)> = conn
        .query_row(
            "SELECT last_replied_at_ms, hourly_window_start_ms, hourly_count
             FROM community_bot_cooldowns WHERE bot_actor_id = ?1 AND scope_key = ?2",
            params![bot_actor_id, scope_key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    if let Some((last_ts, win_start, count)) = existing {
        if min_cooldown_ms > 0
            && (now_ms < last_ts || now_ms.saturating_sub(last_ts) < min_cooldown_ms)
        {
            return Ok(false);
        }
        if now_ms >= win_start
            && now_ms.saturating_sub(win_start) < 3_600_000
            && count >= max_per_hour
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Enqueues `community_mention` updates for any `@mentioned` Bot actors in a post or comment.
pub fn enqueue_bot_mention_updates(
    state: &Arc<AppState>,
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
            if let Ok(Some(target_actor)) = state
                .db
                .pool()
                .with_reader(|conn| ops::get_actor_conn(conn, reply_actor_id, None))
            {
                if target_actor.actor_kind.is_bot_like() {
                    if let Some(bid) = target_actor.linked_user_id.filter(|&id| id > 0) {
                        if seen_bot_ids.insert(bid) {
                            targets.push((bid, "community_comment"));
                        }
                    }
                }
            }
        }
    }

    for mention_handle in entities.mentions {
        if let Ok(Some(mentioned)) = state
            .db
            .pool()
            .with_reader(|conn| ops::get_actor_conn(conn, &mention_handle, None))
        {
            if mentioned.actor_id != from_actor.actor_id && mentioned.actor_kind.is_bot_like() {
                if let Some(bid) = mentioned.linked_user_id.filter(|&id| id > 0) {
                    if seen_bot_ids.insert(bid) {
                        targets.push((bid, "community_mention"));
                    }
                }
            }
        }
    }

    for (bot_user_id, kind) in targets {
        let payload = json!({
            "post_id": post_id,
            "comment_id": comment_id,
            "community_slug": community_slug,
            "from_actor_id": from_actor.actor_id,
            "from_handle": from_actor.handle,
            "from_kind": from_actor.actor_kind.as_str(),
            "text": text,
            "timestamp_ms": now_ms,
        });
        crate::routes::telegram_bot_api::enqueue_community_bot_update(
            state,
            bot_user_id,
            kind,
            payload,
        );
    }
}

/// Generates a persona-aware reply for a bot, using a configured external LLM if active or
/// falling back cleanly to `DeterministicBotResponder::generate_reply`, then passing Stage-2 review.
pub async fn generate_bot_reply_text(
    state: &AppState,
    bot: &CommunityActorDto,
    post: &CommunityPostDto,
    thread_context: &[CommunityCommentDto],
    trigger_text: &str,
) -> Option<String> {
    let mut candidate: Option<String> = None;

    // Attempt real LLM generation ONLY when an external (non-local_hermes) provider is configured.
    let provider_snap = state.fabushi_bots.core_engine().providers().snapshot();
    let has_remote_llm = provider_snap
        .providers
        .get(&provider_snap.active_provider_id)
        .is_some_and(|p| p.api_mode.as_str() != "local_hermes" && !p.credentials.entries.is_empty());

    if has_remote_llm {
        if let Some(bid) = bot.linked_user_id {
            if let Some(inst) = state.fabushi_bots.get_by_user_id(bid) {
                let prompt = format!(
                    "You are @{} ({}) replying in a community thread.\nPost title: {}\nPost body: {}\nTrigger: {}\nRespond concisely in-character (2-4 sentences). Output [PASS] if you have nothing substantive to add.",
                    bot.handle,
                    bot.display_name,
                    post.title.as_deref().unwrap_or("(no title)"),
                    post.body.chars().take(600).collect::<String>(),
                    trigger_text.chars().take(400).collect::<String>()
                );
                let session_id = format!("community_post_{}_{}", post.post_id, bot.actor_id);
                let req = gpui_telegram_agent::HermesTurnRequest {
                    session_id: Some(session_id),
                    chat_id: None,
                    bot_id: Some(bid.to_string()),
                    bot_name: Some(bot.display_name.clone()),
                    user_id: None,
                    source: "community".to_string(),
                    prompt,
                    model: bot.bot_model.clone(),
                    system_prompt: bot.bot_persona.clone(),
                    preset: Some(gpui_telegram_agent::ToolsetPreset::Minimal),
                    max_iterations: Some(2),
                    skip_slash_commands: true,
                    ..Default::default()
                };
                if let Ok(Ok(turn)) = tokio::time::timeout(
                    std::time::Duration::from_secs(6),
                    inst.hermes().run_turn(req),
                )
                .await
                {
                    let txt = turn.response_text.trim().to_string();
                    if !txt.is_empty() {
                        candidate = Some(txt);
                    }
                }
            }
        }
    }

    let mut flat_comments = Vec::new();
    flatten_comment_tree(thread_context, &mut flat_comments);
    flat_comments.sort_by(|a, b| {
        a.created_at_ms
            .cmp(&b.created_at_ms)
            .then_with(|| a.comment_id.cmp(&b.comment_id))
    });

    let draft = match candidate {
        Some(text) => text,
        None if state.config.is_in_memory() || state.config.allow_deterministic_bot_fallback => {
            DeterministicBotResponder::generate_reply(bot, post, &flat_comments, trigger_text)?
        }
        None => return None,
    };

    let recent_msgs: Vec<String> = flat_comments
        .iter()
        .rev()
        .take(10)
        .map(|c| c.body.clone())
        .collect();
    let review = evaluate_outbound_turn_review(&OutboundTurnReviewInput {
        agent_id: bot.actor_id.clone(),
        candidate_text: draft.clone(),
        recent_room_messages: recent_msgs.clone(),
        duplicate_similarity_threshold: DEFAULT_BOT_DUPLICATE_THRESHOLD,
    });
    if !review.should_send {
        return None;
    }
    let recent_refs: Vec<&str> = recent_msgs.iter().map(String::as_str).collect();
    if BotLoopGuard::check_not_duplicate_echo(
        &review.sanitized_text,
        &recent_refs,
        DEFAULT_BOT_DUPLICATE_THRESHOLD,
    )
    .is_err()
    {
        return None;
    }
    Some(review.sanitized_text)
}

fn load_bot_rate_limit_conn(conn: &rusqlite::Connection, bot_actor_id: &str) -> i64 {
    conn.query_row(
        "SELECT bot_rate_limit_per_hour FROM community_actors WHERE actor_id = ?1",
        params![bot_actor_id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .ok()
    .flatten()
    .unwrap_or(20)
    .clamp(1, 500)
}

async fn run_reactive_bots_for_event(
    state: Arc<AppState>,
    post: CommunityPostDto,
    trigger_actor: CommunityActorDto,
    trigger_text: String,
    parent_comment_id: Option<String>,
    max_wakes: usize,
) {
    if state.gateway_hub.is_shutdown()
        || post.status != PostStatus::Published
        || post.is_removed
        || post.is_locked
    {
        return;
    }

    let now_ms = Utc::now().timestamp_millis();
    let prep = state.db.pool().with_reader(|conn| {
        let community: Option<SubCommunityDto> = if let Some(cid) = post.community_id.as_deref() {
            ops::get_community_conn(conn, cid, None)?
        } else {
            None
        };
        if community.as_ref().is_some_and(|c| !c.allow_bots) {
            return Ok((Vec::new(), community, Vec::new()));
        }
        let mut bots = ops::list_actors_conn(conn, Some(ActorKind::Bot), 100)?;
        let hybrids = ops::list_actors_conn(conn, Some(ActorKind::HybridAgent), 50)?;
        bots.extend(hybrids);
        let eligible_bots =
            filter_eligible_bots_for_community_conn(conn, bots, community.as_ref())?;
        let comments =
            ops::list_comment_tree_conn(conn, &post.post_id, None, CommentSort::New, None, 100)?;
        Ok((eligible_bots, community, comments))
    });

    let Ok((eligible_bots, community, thread_context)) = prep else {
        return;
    };
    if eligible_bots.is_empty() {
        return;
    }

    let selected = BotWakeSelector::select_bots_for_event(
        &eligible_bots,
        &post,
        &trigger_text,
        &trigger_actor,
        community.as_ref(),
        max_wakes,
    );

    let scope_key = format!("post:{}", post.post_id);
    for bot in selected {
        if state.gateway_hub.is_shutdown() {
            break;
        }
        if bot.actor_id == trigger_actor.actor_id {
            continue;
        }
        if trigger_actor.actor_kind.is_bot_like()
            && state.community_bot_guard.current_hops(&post.post_id)
                >= state.community_bot_guard.max_hops()
        {
            continue;
        }

        // Read-only pre-flight (ZERO DB or BotLoopGuard mutations before reply passes review!)
        let preflight_ok = state
            .db
            .pool()
            .with_reader(|conn| {
                if check_bot_comment_loop_guard_conn(
                    conn,
                    &bot,
                    &post.post_id,
                    parent_comment_id.as_deref(),
                    false,
                )
                .is_err()
                {
                    return Ok(false);
                }
                let max_hr = load_bot_rate_limit_conn(conn, &bot.actor_id);
                is_bot_cooldown_available_conn(
                    conn,
                    &bot.actor_id,
                    &scope_key,
                    gpui_telegram_community::DEFAULT_BOT_COOLDOWN_MS,
                    max_hr,
                    now_ms,
                )
            })
            .unwrap_or(false);
        if !preflight_ok {
            continue;
        }

        let Some(reply_body) =
            generate_bot_reply_text(&state, &bot, &post, &thread_context, &trigger_text).await
        else {
            continue;
        };

        let comment_req = CreateCommentRequest {
            parent_comment_id: parent_comment_id.clone(),
            body: reply_body,
            media: Vec::new(),
            is_mod_distinguished: false,
        };
        let created = state.db.pool().with_transaction(|tx| {
            let tx_now_ms = Utc::now().timestamp_millis();
            let fresh_post = ops::get_post_conn(tx, &post.post_id, None)?
                .ok_or_else(|| TelegramError::NotFound(format!("Post '{}' not found", post.post_id)))?;
            if fresh_post.status != PostStatus::Published
                || fresh_post.is_removed
                || fresh_post.is_locked
            {
                return Err(TelegramError::PermissionDenied(
                    "Post is no longer open for reactive bot comments".into(),
                ));
            }
            check_bot_comment_loop_guard_tx(
                tx,
                &bot,
                &post.post_id,
                parent_comment_id.as_deref(),
                false,
            )?;
            let max_hr = load_bot_rate_limit_conn(tx, &bot.actor_id);
            let cd_ok = ops::check_and_record_bot_cooldown_tx(
                tx,
                &bot.actor_id,
                &scope_key,
                gpui_telegram_community::DEFAULT_BOT_COOLDOWN_MS,
                max_hr,
                tx_now_ms,
            )?;
            if !cd_ok {
                return Err(TelegramError::Validation("bot_cooldown_active".into()));
            }
            let resp = ops::create_comment_tx(tx, &bot.actor_id, &post.post_id, &comment_req, tx_now_ms)?;
            state
                .community_bot_guard
                .try_acquire_turn(
                    &post.post_id,
                    &bot.actor_id,
                    &trigger_actor.actor_id,
                    trigger_actor.actor_kind,
                    false,
                    tx_now_ms,
                )
                .map_err(|rej| TelegramError::Validation(format!("bot_guard_declined: {rej:?}")))?;
            Ok((resp, tx_now_ms))
        });

        if let Ok((resp, tx_now_ms)) = created {
            broadcast_community_event(
                &state,
                "comment_created",
                post.community_id.clone(),
                Some(post.post_id.clone()),
                Some(resp.comment.comment_id.clone()),
                Some(bot.actor_id.clone()),
                &json!({
                    "comment_id": resp.comment.comment_id,
                    "post_id": post.post_id,
                    "author_actor_id": bot.actor_id,
                    "author_kind": bot.actor_kind.as_str(),
                    "reactive_bot": true,
                }),
            );
            enqueue_bot_mention_updates(
                &state,
                &resp.comment.body,
                &bot,
                &post.post_id,
                Some(&resp.comment.comment_id),
                post.community_slug.as_deref(),
                Some(&trigger_actor.actor_id),
                tx_now_ms,
            );
        }
    }
}

/// Triggers Bot API mention updates/webhooks synchronously and spawns bounded reactive bot replies
/// using `Weak<AppState>` so server shutdown drains cleanly.
pub fn dispatch_post_bot_effects(
    state: &Arc<AppState>,
    post: &CommunityPostDto,
    author: &CommunityActorDto,
    now_ms: i64,
) {
    if post.status != PostStatus::Published || post.is_removed {
        return;
    }
    let full_text = post_mention_text(post);
    enqueue_bot_mention_updates(
        state,
        &full_text,
        author,
        &post.post_id,
        None,
        post.community_slug.as_deref(),
        None,
        now_ms,
    );

    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let weak = Arc::downgrade(state);
        let post_cl = post.clone();
        let author_cl = author.clone();
        rt.spawn(async move {
            let Some(st) = weak.upgrade() else { return };
            run_reactive_bots_for_event(st, post_cl, author_cl, full_text, None, 1).await;
        });
    }
}

/// Triggers Bot API mention/reply updates/webhooks synchronously and spawns bounded reactive bot replies.
pub fn dispatch_comment_bot_effects(
    state: &Arc<AppState>,
    post: &CommunityPostDto,
    comment: &CommunityCommentDto,
    author: &CommunityActorDto,
    reply_target_actor_id: Option<&str>,
    now_ms: i64,
) {
    if comment.is_removed || comment.is_deleted {
        return;
    }
    enqueue_bot_mention_updates(
        state,
        &comment.body,
        author,
        &post.post_id,
        Some(&comment.comment_id),
        post.community_slug.as_deref(),
        reply_target_actor_id,
        now_ms,
    );

    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let weak = Arc::downgrade(state);
        let post_cl = post.clone();
        let author_cl = author.clone();
        let text = comment.body.clone();
        let parent_id = Some(comment.comment_id.clone());
        rt.spawn(async move {
            let Some(st) = weak.upgrade() else { return };
            run_reactive_bots_for_event(st, post_cl, author_cl, text, parent_id, 1).await;
        });
    }
}

/// Executes one 5-second background Community scheduler tick:
/// 1. Publishes due `Scheduled` posts -> Tantivy index + mention notifications + WS fan-out + reactive bot wake & mention webhooks.
/// 2. Closes expired polls -> WS fan-out.
/// 3. Runs at most 1 bounded autonomous bot turn guarded by `BotLoopGuard` + DB cooldown + `reply_gate`.
pub async fn run_community_background_tick(state: &Arc<AppState>) {
    if state.gateway_hub.is_shutdown() {
        return;
    }
    let now_ms = Utc::now().timestamp_millis();

    // 1. Publish due scheduled posts
    if let Ok(published_posts) = state.db.pool().with_transaction(|tx| {
        let posts = ops::publish_due_scheduled_posts_tx(tx, now_ms)?;
        for post in &posts {
            if post.status == PostStatus::Published && !post.is_removed {
                backfill_post_mention_notifications_tx(tx, post, now_ms)?;
            }
        }
        Ok(posts)
    }) {
        for post in published_posts {
            sync_post_in_search(state, &post);
            if post.status == PostStatus::Published && !post.is_removed {
                broadcast_community_event(
                    state,
                    "scheduled_post_published",
                    post.community_id.clone(),
                    Some(post.post_id.clone()),
                    None,
                    Some(post.author_actor_id.clone()),
                    &json!({
                        "post_id": post.post_id,
                        "community_slug": post.community_slug,
                        "author_actor_id": post.author_actor_id,
                    }),
                );
                if let Ok(Some(author)) = state
                    .db
                    .pool()
                    .with_reader(|conn| ops::get_actor_conn(conn, &post.author_actor_id, None))
                {
                    if !author.actor_kind.is_bot_like() {
                        state.community_bot_guard.reset_on_human_turn(&post.post_id);
                    }
                    dispatch_post_bot_effects(state, &post, &author, now_ms);
                }
            }
        }
    }

    // 2. Close expired polls
    if let Ok(closed_post_ids) = state
        .db
        .pool()
        .with_transaction(|tx| ops::close_expired_polls_tx(tx, now_ms))
    {
        for pid in closed_post_ids {
            broadcast_community_event(
                state,
                "poll_closed",
                None,
                Some(pid.clone()),
                None,
                None,
                &json!({ "post_id": pid }),
            );
        }
    }

    // 3. Rate-limited autonomous bot turn (max 1 turn per tick)
    let auto_candidates = state.db.pool().with_reader(|conn| {
        let mut bots: Vec<CommunityActorDto> = ops::list_actors_conn(conn, Some(ActorKind::Bot), 50)?
            .into_iter()
            .filter(|b| b.bot_autonomy_mode == BotAutonomyMode::Autonomous)
            .collect();
        bots.extend(
            ops::list_actors_conn(conn, Some(ActorKind::HybridAgent), 50)?
                .into_iter()
                .filter(|b| b.bot_autonomy_mode == BotAutonomyMode::Autonomous),
        );
        if bots.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let cutoff = now_ms.saturating_sub(24 * 3_600_000);
        let mut stmt = conn
            .prepare(
                "SELECT post_id FROM community_posts
                 WHERE status = 'published' AND is_removed = 0 AND is_locked = 0
                   AND created_at_ms >= ?1
                 ORDER BY created_at_ms DESC LIMIT 5",
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let pids: Vec<String> = stmt
            .query_map(params![cutoff], |r| r.get::<_, String>(0))
            .map_err(|e| TelegramError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let mut posts = Vec::new();
        for pid in pids {
            if let Some(p) = ops::get_post_conn(conn, &pid, None)? {
                posts.push(p);
            }
        }
        Ok((bots, posts))
    });

    let Ok((auto_bots, recent_posts)) = auto_candidates else {
        return;
    };
    if auto_bots.is_empty() || recent_posts.is_empty() {
        return;
    }

    for post in recent_posts {
        let Ok((Some(author), community, eligible_bots)) = state.db.pool().with_reader(|conn| {
            let author = ops::get_actor_conn(conn, &post.author_actor_id, None)?;
            let community = match post.community_id.as_deref() {
                Some(cid) => ops::get_community_conn(conn, cid, None)?,
                None => None,
            };
            let eligible = filter_eligible_bots_for_community_conn(
                conn,
                auto_bots.clone(),
                community.as_ref(),
            )?;
            Ok((author, community, eligible))
        }) else {
            continue;
        };
        if eligible_bots.is_empty() {
            continue;
        }
        let trigger_text = post_mention_text(&post);
        let picked = BotWakeSelector::select_bots_for_event(
            &eligible_bots,
            &post,
            &trigger_text,
            &author,
            community.as_ref(),
            1,
        );
        let Some(bot) = picked.into_iter().next() else {
            continue;
        };
        if bot.actor_id == author.actor_id {
            continue;
        }
        if author.actor_kind.is_bot_like()
            && state.community_bot_guard.current_hops(&post.post_id)
                >= state.community_bot_guard.max_hops()
        {
            continue;
        }
        let scope_key = format!("auto:{}", post.post_id);
        let Ok(Some(thread_ctx)) = state.db.pool().with_reader(|conn| {
            if check_bot_comment_loop_guard_conn(conn, &bot, &post.post_id, None, false).is_err() {
                return Ok(None);
            }
            let max_hr = load_bot_rate_limit_conn(conn, &bot.actor_id);
            if !is_bot_cooldown_available_conn(
                conn,
                &bot.actor_id,
                &scope_key,
                60_000,
                max_hr,
                now_ms,
            )? {
                return Ok(None);
            }
            let comments =
                ops::list_comment_tree_conn(conn, &post.post_id, None, CommentSort::New, None, 100)?;
            Ok(Some(comments))
        }) else {
            continue;
        };
        if let Some(reply_body) =
            generate_bot_reply_text(state, &bot, &post, &thread_ctx, &trigger_text).await
        {
            let req = CreateCommentRequest {
                parent_comment_id: None,
                body: reply_body,
                media: Vec::new(),
                is_mod_distinguished: false,
            };
            if let Ok(resp) = state.db.pool().with_transaction(|tx| {
                let tx_now_ms = Utc::now().timestamp_millis();
                let fresh_post = ops::get_post_conn(tx, &post.post_id, None)?.ok_or_else(|| {
                    TelegramError::NotFound(format!("Post '{}' not found", post.post_id))
                })?;
                if fresh_post.status != PostStatus::Published
                    || fresh_post.is_removed
                    || fresh_post.is_locked
                {
                    return Err(TelegramError::PermissionDenied(
                        "Post is no longer open for autonomous bot comments".into(),
                    ));
                }
                check_bot_comment_loop_guard_tx(tx, &bot, &post.post_id, None, false)?;
                let max_hr = load_bot_rate_limit_conn(tx, &bot.actor_id);
                let cd_ok = ops::check_and_record_bot_cooldown_tx(
                    tx,
                    &bot.actor_id,
                    &scope_key,
                    60_000,
                    max_hr,
                    tx_now_ms,
                )?;
                if !cd_ok {
                    return Err(TelegramError::Validation("bot_cooldown_active".into()));
                }
                let created =
                    ops::create_comment_tx(tx, &bot.actor_id, &post.post_id, &req, tx_now_ms)?;
                state
                    .community_bot_guard
                    .try_acquire_turn(
                        &post.post_id,
                        &bot.actor_id,
                        &author.actor_id,
                        author.actor_kind,
                        false,
                        tx_now_ms,
                    )
                    .map_err(|rej| {
                        TelegramError::Validation(format!("bot_guard_declined: {rej:?}"))
                    })?;
                Ok(created)
            }) {
                broadcast_community_event(
                    state,
                    "comment_created",
                    post.community_id.clone(),
                    Some(post.post_id.clone()),
                    Some(resp.comment.comment_id.clone()),
                    Some(bot.actor_id.clone()),
                    &json!({
                        "comment_id": resp.comment.comment_id,
                        "post_id": post.post_id,
                        "author_actor_id": bot.actor_id,
                        "autonomous_bot": true,
                    }),
                );
                break; // at most 1 autonomous bot turn per 5s tick
            }
        }
    }
}
