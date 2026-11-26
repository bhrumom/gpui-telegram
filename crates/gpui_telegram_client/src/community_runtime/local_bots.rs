//! Local reactive `@mention` bot triggering, `summon_bot`, cooldowns, and 5-hop anti-loop guard.

use super::{now_millis, LocalCommunityRuntime};
use gpui_telegram_community::{
    extract_entities, BotLoopGuard, BotWakeSelector, DeterministicBotResponder,
    DEFAULT_BOT_COOLDOWN_MS, DEFAULT_BOT_DUPLICATE_THRESHOLD, DEFAULT_BOT_MAX_LOOP_HOPS,
};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_proto::community_contracts::{
    ActorKind, BotAutonomyMode, CommentSort, CommunityActorDto, CommunityCommentDto,
    CommunityPostDto, CreateCommentRequest, PostStatus, SummonBotRequest, SummonBotResponse,
};
use rusqlite::{params, OptionalExtension};
use std::collections::HashSet;

pub(crate) fn flatten_comment_tree(nodes: &[CommunityCommentDto]) -> Vec<CommunityCommentDto> {
    let mut flat = Vec::new();
    let mut stack: Vec<&CommunityCommentDto> = nodes.iter().rev().collect();
    while let Some(node) = stack.pop() {
        flat.push(node.clone());
        for child in node.children.iter().rev() {
            stack.push(child);
        }
    }
    flat
}

impl LocalCommunityRuntime {
    fn trailing_post_bot_hops(&self, post_id: &str) -> Result<u32> {
        self.store.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT a.actor_kind
                     FROM community_comments c
                     JOIN community_actors a ON a.actor_id = c.author_actor_id
                     WHERE c.post_id = ?1
                       AND c.is_removed = 0
                       AND c.is_deleted = 0
                     ORDER BY c.created_at_ms DESC, c.rowid DESC
                     LIMIT 16",
                )
                .map_err(|e| TelegramError::Database(format!("prepare trailing bot hops: {e}")))?;
            let rows = stmt
                .query_map(params![post_id], |r| r.get::<_, String>(0))
                .map_err(|e| TelegramError::Database(format!("query trailing bot hops: {e}")))?;
            let mut consecutive: u32 = 0;
            for row in rows {
                let kind = row
                    .map_err(|e| TelegramError::Database(format!("read trailing actor_kind: {e}")))?;
                if kind == "human" {
                    break;
                }
                consecutive = consecutive.saturating_add(1);
            }
            Ok(consecutive)
        })
    }

    fn persisted_comment_bot_hops(
        &self,
        post_id: &str,
        parent_comment_id: Option<&str>,
    ) -> Result<u32> {
        if let Some(cid) = parent_comment_id.map(str::trim).filter(|s| !s.is_empty()) {
            let cid_hops = self
                .store
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT bot_hop_count FROM community_comments WHERE comment_id = ?1",
                        params![cid],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()
                    .map_err(|e| TelegramError::Database(format!("query bot_hop_count: {e}")))
                })?
                .unwrap_or(0)
                .max(0) as u32;
            if cid_hops > 0 {
                let trailing = self.trailing_post_bot_hops(post_id)?;
                return Ok(cid_hops.max(trailing));
            }
            return Ok(0);
        }

        self.trailing_post_bot_hops(post_id)
    }

    /// Triggers local reactive replies ONLY for explicitly `@mention`ed non-Manual bots.
    /// Plain posts/comments without `@mentions` produce zero unsolicited replies.
    pub(crate) fn maybe_trigger_local_mention_bots(
        &self,
        post: &CommunityPostDto,
        parent_comment_id: Option<&str>,
        trigger_actor_id: &str,
        trigger_text: &str,
        now_ms: i64,
    ) -> Result<bool> {
        if post.status != PostStatus::Published || post.is_removed || post.is_locked {
            return Ok(false);
        }

        let Some(trigger_actor) = self.store.get_actor(trigger_actor_id, None)? else {
            return Ok(false);
        };

        if trigger_actor.actor_kind == ActorKind::Human {
            self.bot_loop_guard
                .read()
                .reset_on_human_turn(&post.post_id);
        } else if self.persisted_comment_bot_hops(&post.post_id, parent_comment_id)?
            >= DEFAULT_BOT_MAX_LOOP_HOPS
        {
            return Ok(false);
        }

        let entities = extract_entities(trigger_text);
        if entities.mentions.is_empty() {
            return Ok(false);
        }
        let mention_set: HashSet<String> = entities
            .mentions
            .into_iter()
            .map(|m| m.to_ascii_lowercase())
            .collect();

        if let Some(cid) = post.community_id.as_deref() {
            if let Some(comm) = self.store.get_community(cid, None)? {
                if !comm.allow_bots {
                    return Ok(false);
                }
            }
        }

        let all_actors = self.store.list_actors(None, 200)?;
        let mentioned_bots: Vec<CommunityActorDto> = all_actors
            .into_iter()
            .filter(|a| {
                a.actor_kind.is_bot_like()
                    && a.actor_id != trigger_actor.actor_id
                    && a.bot_autonomy_mode != BotAutonomyMode::Manual
                    && mention_set.contains(&a.handle.to_ascii_lowercase())
            })
            .collect();

        if mentioned_bots.is_empty() {
            return Ok(false);
        }

        let tree = self
            .store
            .list_comment_tree(&post.post_id, None, CommentSort::Old, None, 50)?;
        let flat_ctx = flatten_comment_tree(&tree);

        for bot in mentioned_bots {
            // Skip community-banned/muted bots before consuming cooldown or loop-guard state.
            if let Some(cid) = post.community_id.as_deref() {
                let blocked_in_comm = self
                    .store
                    .with_conn(|conn| {
                        conn.query_row(
                            "SELECT role FROM community_memberships
                             WHERE community_id = ?1 AND actor_id = ?2",
                            params![cid, bot.actor_id],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()
                        .map_err(|e| {
                            TelegramError::Database(format!("check bot membership role: {e}"))
                        })
                    })
                    .ok()
                    .flatten()
                    .is_some_and(|role| matches!(role.as_str(), "banned" | "muted"));
                if blocked_in_comm {
                    continue;
                }
            }

            // Generate reply & check duplicate echo BEFORE consuming cooldown/loop-guard state.
            let Some(reply) =
                DeterministicBotResponder::generate_reply(&bot, post, &flat_ctx, trigger_text)
            else {
                continue;
            };
            let recent_bot_bodies: Vec<&str> = flat_ctx
                .iter()
                .filter(|c| c.author_actor_id == bot.actor_id)
                .map(|c| c.body.as_str())
                .collect();
            if BotLoopGuard::check_not_duplicate_echo(
                &reply,
                &recent_bot_bodies,
                DEFAULT_BOT_DUPLICATE_THRESHOLD,
            )
            .is_err()
            {
                continue;
            }

            let acquired = self.bot_loop_guard.read().try_acquire_turn(
                &post.post_id,
                &bot.actor_id,
                &trigger_actor.actor_id,
                trigger_actor.actor_kind,
                false,
                now_ms,
            );
            if acquired.is_err() {
                continue;
            }
            let scope_key = format!("post:{}", post.post_id);
            let within_cooldown = match self.store.check_and_record_bot_cooldown(
                &bot.actor_id,
                &scope_key,
                DEFAULT_BOT_COOLDOWN_MS,
                30,
                now_ms,
            ) {
                Ok(ok) => ok,
                Err(err) => {
                    tracing::debug!(
                        bot_actor_id = %bot.actor_id,
                        post_id = %post.post_id,
                        error = %err,
                        "Skipping local @mention bot due to cooldown check error"
                    );
                    continue;
                }
            };
            if !within_cooldown {
                continue;
            }

            let req = CreateCommentRequest {
                parent_comment_id: parent_comment_id.map(str::to_string),
                body: reply,
                media: Vec::new(),
                is_mod_distinguished: false,
            };
            match self.store.create_comment(
                &bot.actor_id,
                &post.post_id,
                &req,
                now_ms.saturating_add(1),
            ) {
                Ok(_) => return Ok(true),
                Err(err) => {
                    tracing::debug!(
                        bot_actor_id = %bot.actor_id,
                        post_id = %post.post_id,
                        error = %err,
                        "Local @mention bot reply skipped after store rejection"
                    );
                    continue;
                }
            }
        }

        Ok(false)
    }

    pub fn summon_bot(
        &self,
        act_as: Option<&str>,
        post_id: &str,
        req: &SummonBotRequest,
    ) -> Result<SummonBotResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        self.run_lazy_maintenance(now_ms);
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let post = self
            .store
            .get_post(post_id, Some(&caller_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("post '{post_id}' not found")))?;

        if post.status != PostStatus::Published || post.is_locked || post.is_removed {
            return Err(TelegramError::PermissionDenied(
                "cannot summon bot on locked, unpublished, or removed post".to_string(),
            ));
        }

        let community = match post.community_id.as_deref() {
            Some(cid) => self.store.get_community(cid, None)?,
            None => None,
        };
        if let Some(ref comm) = community {
            if !comm.allow_bots {
                return Err(TelegramError::PermissionDenied(format!(
                    "bots are disabled in c/{}",
                    comm.slug
                )));
            }
        }

        let caller_actor = self.store.get_actor(&caller_id, None)?;
        let caller_kind = caller_actor
            .as_ref()
            .map(|a| a.actor_kind)
            .unwrap_or(ActorKind::Human);

        let bot = if let Some(id) = req
            .bot_actor_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.store
                .get_actor(id, None)?
                .ok_or_else(|| TelegramError::NotFound(format!("bot actor '{id}' not found")))?
        } else if let Some(handle) = req
            .bot_handle
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            self.store
                .get_actor(handle, None)?
                .ok_or_else(|| TelegramError::NotFound(format!("bot '@{handle}' not found")))?
        } else {
            let all = self.store.list_actors(None, 200)?;
            let bots: Vec<CommunityActorDto> = all
                .into_iter()
                .filter(|a| a.actor_kind.is_bot_like())
                .collect();
            let trigger_text = req
                .prompt
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(&post.body);
            let picked = caller_actor
                .as_ref()
                .and_then(|ca| {
                    BotWakeSelector::select_bots_for_event(
                        &bots,
                        &post,
                        trigger_text,
                        ca,
                        community.as_ref(),
                        1,
                    )
                    .into_iter()
                    .next()
                })
                .or_else(|| {
                    bots.into_iter().find(|b| {
                        b.bot_autonomy_mode != BotAutonomyMode::Manual && b.actor_id != caller_id
                    })
                });
            picked.ok_or_else(|| {
                TelegramError::NotFound("no eligible community bot available to summon".to_string())
            })?
        };

        if !bot.actor_kind.is_bot_like() {
            return Err(TelegramError::Validation(format!(
                "actor '{}' is not a bot citizen",
                bot.actor_id
            )));
        }
        if bot.bot_autonomy_mode == BotAutonomyMode::Manual {
            return Err(TelegramError::PermissionDenied(format!(
                "bot '@{}' is in manual-only mode",
                bot.handle
            )));
        }

        if caller_kind.is_bot_like()
            && self.persisted_comment_bot_hops(post_id, req.parent_comment_id.as_deref())?
                >= DEFAULT_BOT_MAX_LOOP_HOPS
        {
            return Ok(SummonBotResponse {
                summoned_bot_actor_id: bot.actor_id,
                triggered: false,
                reason: "LoopBudgetExceeded".to_string(),
                created_comment: None,
            });
        }

        let explicit_human = caller_kind == ActorKind::Human;
        if let Err(rej) = self.bot_loop_guard.read().try_acquire_turn(
            post_id,
            &bot.actor_id,
            &caller_id,
            caller_kind,
            explicit_human,
            now_ms,
        ) {
            return Ok(SummonBotResponse {
                summoned_bot_actor_id: bot.actor_id,
                triggered: false,
                reason: format!("{rej:?}"),
                created_comment: None,
            });
        }

        let tree = self
            .store
            .list_comment_tree(post_id, Some(&caller_id), CommentSort::Old, None, 50)?;
        let flat_ctx = flatten_comment_tree(&tree);
        let trigger_text = req
            .prompt
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&post.body);
        let Some(reply) =
            DeterministicBotResponder::generate_reply(&bot, &post, &flat_ctx, trigger_text)
        else {
            return Ok(SummonBotResponse {
                summoned_bot_actor_id: bot.actor_id,
                triggered: false,
                reason: "bot declined to reply".to_string(),
                created_comment: None,
            });
        };

        let comment_req = CreateCommentRequest {
            parent_comment_id: req.parent_comment_id.clone(),
            body: reply,
            media: Vec::new(),
            is_mod_distinguished: false,
        };
        let created = self
            .store
            .create_comment(&bot.actor_id, post_id, &comment_req, now_ms)?;
        self.mark_guest_dirty_if_unbound();

        Ok(SummonBotResponse {
            summoned_bot_actor_id: bot.actor_id,
            triggered: true,
            reason: "bot replied".to_string(),
            created_comment: Some(created.comment),
        })
    }
}
