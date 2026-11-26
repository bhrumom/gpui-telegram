//! Complete 50-endpoint `LocalCommunityRuntime` operations backed by `CommunityStore`.

use super::{now_millis, LocalCommunityRuntime, LOCAL_GUEST_ACTOR_ID, LOCAL_GUEST_USER_ID};
use crate::net::api_community::RegisterBotActorResult;
use gpui_telegram_community::identity::{
    bot_actor_id, can_user_act_as, human_actor_id, normalize_handle, validate_act_as,
};
use gpui_telegram_community::{
    parse_search_operators, ModPermissionValidator, TelegramBotSyncInput, TelegramHumanSyncInput,
    ThreadAssembler,
};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_proto::community_contracts::{
    ActorKind, ActorProfileResponse, AlgoPreferencesDto, AwardResponse, BookmarkRequest,
    BookmarkResponse, CommentSort, CommentsListResponse, CommentsQueryParams, CommunityActorDto,
    CommunityCommentDto, CommunityDetailResponse, CommunityIdentitiesResponse, CommunityListDto,
    CommunityListsResponse, CommunityMuteBlockDto, CommunityNotificationKind,
    CommunityPollVoteRequest, CommunityPostDto, CommunitySearchRequestDto,
    CommunitySearchResultDto, CommunitySearchSort, CommunitySearchTab, CommunityWikiRevisionDto,
    CreateCommentRequest, CreateCommentResponse, CreateCommunityListRequest,
    CreateCommunityRequest, CreatePostRequest, CreatePostResponse, DeleteEntityResponse,
    FeedRequestDto, FeedResponseDto, FeedSort, FeedTab, FollowActionResponse, GiveAwardRequest,
    JoinCommunityResponse, ListCommunitiesResponse, MarkNotificationsReadRequest,
    MarkNotificationsReadResponse, ModActionRequest, ModLogEntryDto, ModLogsResponse,
    ModQueueResponse, ModQueueStatus, MuteBlockListResponse, MuteBlockRequest,
    MuteBlockTargetKind, NotificationsListResponse, PollDto, PostDetailResponse, PostFormat,
    PostStatus, RecordSignalRequest, RecordSignalResponse, RegisterBotActorRequest,
    RegisterHumanActorRequest, RepostRequest, RepostResponse, SubCommunityDto,
    ToggleActionResponse, TrendingResponse, UpdateCommentRequest, UpdateCommunityActorRequest,
    UpdateCommunityListRequest, UpdateCommunityRequest, UpdateCommunityWikiRequest,
    UpdatePostRequest, VoteRequest, VoteResponse,
};
use rusqlite::params;
use std::sync::atomic::Ordering;

impl LocalCommunityRuntime {
    pub fn sync_identities_local(
        &self,
        human: &TelegramHumanSyncInput,
        owned_bots: &[TelegramBotSyncInput],
    ) -> Result<CommunityIdentitiesResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let all_actors = self
            .store
            .sync_telegram_identities(human, owned_bots, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        let human_id = human_actor_id(human.user_id);
        let human_actor = all_actors
            .iter()
            .find(|a| a.actor_id == human_id)
            .cloned()
            .or_else(|| {
                all_actors
                    .iter()
                    .find(|a| a.actor_kind == ActorKind::Human)
                    .cloned()
            });
        let bot_actors: Vec<CommunityActorDto> = all_actors
            .iter()
            .filter(|a| a.actor_kind.is_bot_like())
            .cloned()
            .collect();

        let current_active = self.active_actor_id();
        let active_actor_id = current_active
            .filter(|id| all_actors.iter().any(|a| a.actor_id == *id))
            .or_else(|| human_actor.as_ref().map(|a| a.actor_id.clone()))
            .or_else(|| all_actors.first().map(|a| a.actor_id.clone()))
            .unwrap_or_else(|| LOCAL_GUEST_ACTOR_ID.to_string());

        if self.active_actor_id().is_none() {
            self.set_active_actor_id(Some(active_actor_id.clone()));
        }
        Ok(CommunityIdentitiesResponse {
            active_actor_id,
            human_actor,
            bot_actors,
            all_actors,
        })
    }

    pub fn get_identities(&self, act_as: Option<&str>) -> Result<CommunityIdentitiesResponse> {
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let bound_uid = self.bound_user_id();
        let mut all_actors = if let Some(uid) = bound_uid {
            self.store.list_identities_for_user(uid as i64)?
        } else {
            let mut list = self.store.list_identities_for_user(LOCAL_GUEST_USER_ID)?;
            if !list.iter().any(|a| a.actor_id == LOCAL_GUEST_ACTOR_ID) {
                if let Some(g) = self.store.get_actor(LOCAL_GUEST_ACTOR_ID, None)? {
                    list.insert(0, g);
                }
            }
            list
        };
        if all_actors.is_empty() {
            if let Some(caller) = self.store.get_actor(&caller_id, None)? {
                all_actors.push(caller);
            }
        }
        let human_actor = all_actors
            .iter()
            .find(|a| a.actor_kind == ActorKind::Human)
            .cloned();
        let bot_actors: Vec<CommunityActorDto> = all_actors
            .iter()
            .filter(|a| a.actor_kind.is_bot_like())
            .cloned()
            .collect();
        let active_actor_id = if all_actors.iter().any(|a| a.actor_id == caller_id) {
            caller_id
        } else {
            human_actor
                .as_ref()
                .map(|a| a.actor_id.clone())
                .unwrap_or(caller_id)
        };
        Ok(CommunityIdentitiesResponse {
            active_actor_id,
            human_actor,
            bot_actors,
            all_actors,
        })
    }

    pub fn register_human_actor(&self, req: &RegisterHumanActorRequest) -> Result<CommunityActorDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let uid = self.bound_user_id().map(|u| u as i64).unwrap_or(LOCAL_GUEST_USER_ID);
        let expected_id = human_actor_id(uid);
        let norm_handle = normalize_handle(&req.handle)?;
        if let Some(existing_by_id) = self.store.get_actor(&expected_id, None)? {
            validate_act_as(uid, &existing_by_id)?;
        }
        if let Some(existing) = self.store.get_actor(&norm_handle, None)? {
            if existing.actor_id != expected_id {
                return Err(TelegramError::Validation(format!(
                    "handle '@{norm_handle}' is already taken"
                )));
            }
        }
        let actor = self.store.create_human_actor(uid, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(actor)
    }

    pub fn register_bot_actor(&self, req: &RegisterBotActorRequest) -> Result<RegisterBotActorResult> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let owner_uid = self
            .bound_user_id()
            .map(|u| u as i64)
            .unwrap_or(LOCAL_GUEST_USER_ID);
        let norm_handle = normalize_handle(&req.handle)?;
        let bot_uid = match req.bot_user_id {
            Some(id) if id > 0 => id,
            _ => loop {
                let candidate = self.next_local_bot_id.fetch_add(1, Ordering::Relaxed);
                let aid = bot_actor_id(candidate);
                if self.store.get_actor(&aid, None)?.is_none() {
                    break candidate;
                }
            },
        };
        let expected_id = bot_actor_id(bot_uid);
        if let Some(existing_by_id) = self.store.get_actor(&expected_id, None)? {
            validate_act_as(owner_uid, &existing_by_id)?;
        }
        if let Some(existing) = self.store.get_actor(&norm_handle, None)? {
            if existing.actor_id != expected_id {
                return Err(TelegramError::Validation(format!(
                    "handle '@{norm_handle}' is already taken"
                )));
            }
        }
        let actor = self.store.create_bot_actor(owner_uid, bot_uid, req, now_ms)?;
        if let Some(max_hr) = req.max_posts_per_hour {
            self.store.with_conn(|conn| {
                conn.execute(
                    "UPDATE community_actors SET bot_rate_limit_per_hour = ?2 WHERE actor_id = ?1",
                    params![actor.actor_id, max_hr.clamp(1, 500) as i64],
                )
                .map_err(|e| TelegramError::Database(format!("update bot_rate_limit_per_hour: {e}")))?;
                Ok(())
            })?;
        }
        self.mark_guest_dirty_if_unbound();
        Ok(RegisterBotActorResult {
            bot_user_id: Some(bot_uid),
            username: Some(actor.handle.clone()),
            actor,
            bot_token: None,
        })
    }
}

impl LocalCommunityRuntime {
    pub fn update_actor(
        &self,
        act_as: Option<&str>,
        actor_id: &str,
        req: &UpdateCommunityActorRequest,
    ) -> Result<CommunityActorDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let _caller_id = self.resolve_caller_actor_id(act_as)?;
        let target = self
            .store
            .get_actor(actor_id, None)?
            .ok_or_else(|| TelegramError::NotFound(format!("actor '{actor_id}' not found")))?;
        let effective_uid = self
            .bound_user_id()
            .map(|u| u as i64)
            .unwrap_or(LOCAL_GUEST_USER_ID);
        validate_act_as(effective_uid, &target)?;
        let updated = self.store.update_actor(&target.actor_id, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(updated)
    }

    pub fn get_actor_profile(
        &self,
        act_as: Option<&str>,
        handle_or_id: &str,
    ) -> Result<ActorProfileResponse> {
        let now_ms = now_millis();
        self.run_lazy_maintenance(now_ms);
        let viewer_id = self.resolve_caller_actor_id(act_as)?;
        let actor = self
            .store
            .get_actor(handle_or_id, Some(&viewer_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("actor '{handle_or_id}' not found")))?;

        let recent_posts = self
            .store
            .get_feed(
                &viewer_id,
                &FeedRequestDto {
                    tab: FeedTab::ActorPosts(actor.actor_id.clone()),
                    sort: FeedSort::New,
                    limit: 20,
                    ..Default::default()
                },
                now_ms,
            )?
            .items
            .into_iter()
            .map(|item| item.post)
            .collect();

        let actor_clone = actor.clone();
        let (recent_comments, moderated_communities) = self.store.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT c.comment_id, c.post_id, c.parent_comment_id, c.depth, c.path,
                            p.author_actor_id, c.body_markdown, c.media_json,
                            c.upvotes, c.downvotes, c.score, c.likes_count,
                            c.is_stickied, c.is_distinguished_mod, c.is_removed, c.is_deleted,
                            c.created_at_ms, c.updated_at_ms
                     FROM community_comments c
                     JOIN community_posts p ON p.post_id = c.post_id
                     WHERE c.author_actor_id = ?1
                       AND c.is_removed = 0 AND c.is_deleted = 0
                       AND p.status = 'published' AND p.is_removed = 0
                     ORDER BY c.created_at_ms DESC LIMIT 20",
                )
                .map_err(|e| TelegramError::Database(format!("prepare recent comments: {e}")))?;
            let rows = stmt
                .query_map(params![actor_clone.actor_id], |r| {
                    let post_author_id: String = r.get(5)?;
                    let media_json: String = r.get(7)?;
                    let media = serde_json::from_str(&media_json).unwrap_or_default();
                    Ok(CommunityCommentDto {
                        comment_id: r.get(0)?,
                        post_id: r.get(1)?,
                        parent_comment_id: r.get(2)?,
                        depth: r.get::<_, i64>(3)?.max(0) as u32,
                        path: r.get(4)?,
                        author_actor_id: actor_clone.actor_id.clone(),
                        author_handle: actor_clone.handle.clone(),
                        author_display_name: actor_clone.display_name.clone(),
                        author_kind: actor_clone.actor_kind,
                        author_verified_tier: actor_clone.verified_tier,
                        author_bot_badge: actor_clone.bot_badge.clone(),
                        author_avatar_shape: actor_clone.avatar_shape.clone(),
                        author_avatar_color: actor_clone.avatar_color.clone(),
                        is_op: actor_clone.actor_id == post_author_id,
                        body: r.get(6)?,
                        media,
                        upvotes: r.get(8)?,
                        downvotes: r.get(9)?,
                        score: r.get(10)?,
                        likes_count: r.get::<_, i64>(11)?.max(0) as u64,
                        awards: Vec::new(),
                        viewer_vote: 0,
                        viewer_liked: false,
                        is_stickied: r.get::<_, i64>(12)? != 0,
                        is_mod_distinguished: r.get::<_, i64>(13)? != 0,
                        is_removed: r.get::<_, i64>(14)? != 0,
                        is_deleted: r.get::<_, i64>(15)? != 0,
                        children: Vec::new(),
                        more_children_count: 0,
                        created_at_ms: r.get(16)?,
                        updated_at_ms: r.get(17)?,
                    })
                })
                .map_err(|e| TelegramError::Database(format!("query recent comments: {e}")))?;
            let mut comments = Vec::new();
            for row in rows {
                comments.push(
                    row.map_err(|e| TelegramError::Database(format!("read recent comment: {e}")))?,
                );
            }

            let mut mod_stmt = conn
                .prepare(
                    "SELECT c.slug FROM community_memberships m
                     JOIN communities c ON c.community_id = m.community_id
                     WHERE m.actor_id = ?1 AND m.role IN ('owner', 'mod', 'bot_mod')
                     ORDER BY c.slug ASC",
                )
                .map_err(|e| TelegramError::Database(format!("prepare moderated comms: {e}")))?;
            let mod_rows = mod_stmt
                .query_map(params![actor_clone.actor_id], |r| r.get::<_, String>(0))
                .map_err(|e| TelegramError::Database(format!("query moderated comms: {e}")))?;
            let mut mods = Vec::new();
            for row in mod_rows {
                mods.push(
                    row.map_err(|e| TelegramError::Database(format!("read moderated comm: {e}")))?,
                );
            }
            Ok((comments, mods))
        })?;

        Ok(ActorProfileResponse {
            actor,
            recent_posts,
            recent_comments,
            moderated_communities,
        })
    }

    pub fn follow_actor(&self, act_as: Option<&str>, actor_id: &str) -> Result<FollowActionResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let target = self
            .store
            .get_actor(actor_id, None)?
            .ok_or_else(|| TelegramError::NotFound(format!("actor '{actor_id}' not found")))?;
        self.store.follow_actor(&caller_id, &target.actor_id, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        let refreshed = self
            .store
            .get_actor(&target.actor_id, Some(&caller_id))?
            .unwrap_or(target);
        Ok(FollowActionResponse {
            actor_id: refreshed.actor_id,
            is_following: true,
            followers_count: refreshed.followers_count,
        })
    }

    pub fn unfollow_actor(
        &self,
        act_as: Option<&str>,
        actor_id: &str,
    ) -> Result<FollowActionResponse> {
        self.ensure_local_write_allowed()?;
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let target = self
            .store
            .get_actor(actor_id, None)?
            .ok_or_else(|| TelegramError::NotFound(format!("actor '{actor_id}' not found")))?;
        self.store.unfollow_actor(&caller_id, &target.actor_id)?;
        self.mark_guest_dirty_if_unbound();
        let refreshed = self
            .store
            .get_actor(&target.actor_id, Some(&caller_id))?
            .unwrap_or(target);
        Ok(FollowActionResponse {
            actor_id: refreshed.actor_id,
            is_following: false,
            followers_count: refreshed.followers_count,
        })
    }

    pub fn list_mutes_blocks(&self, act_as: Option<&str>) -> Result<MuteBlockListResponse> {
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let items = self.store.list_mutes_blocks(&caller_id)?;
        Ok(MuteBlockListResponse { items })
    }

    pub fn set_mute_block(
        &self,
        act_as: Option<&str>,
        req: &MuteBlockRequest,
    ) -> Result<CommunityMuteBlockDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let mut normalized = req.clone();
        if normalized.target_kind == MuteBlockTargetKind::Actor {
            if let Some(target_actor) = self.store.get_actor(&normalized.target_value, None)? {
                normalized.target_value = target_actor.actor_id;
            }
            if normalized.target_value.trim() == caller_id {
                return Err(TelegramError::Validation(
                    "cannot mute or block your own active actor".to_string(),
                ));
            }
        }
        let saved = self.store.set_mute_block(&caller_id, &normalized, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(saved)
    }

    pub fn remove_mute_block(
        &self,
        act_as: Option<&str>,
        req: &MuteBlockRequest,
    ) -> Result<DeleteEntityResponse> {
        self.ensure_local_write_allowed()?;
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let mut target_val = req.target_value.trim().to_string();
        if req.target_kind == MuteBlockTargetKind::Actor {
            if let Some(target_actor) = self.store.get_actor(&target_val, None)? {
                target_val = target_actor.actor_id;
            }
        }
        let deleted =
            self.store
                .remove_mute_block(&caller_id, req.target_kind, &target_val, req.action)?;
        self.mark_guest_dirty_if_unbound();
        Ok(DeleteEntityResponse {
            id: format!(
                "{}:{}:{}",
                req.target_kind.as_str(),
                req.action.as_str(),
                target_val
            ),
            deleted,
        })
    }
}

impl LocalCommunityRuntime {
    pub fn get_feed(&self, act_as: Option<&str>, req: &FeedRequestDto) -> Result<FeedResponseDto> {
        let now_ms = now_millis();
        self.run_lazy_maintenance(now_ms);
        let viewer_id = self.resolve_caller_actor_id(act_as)?;
        self.store.get_feed(&viewer_id, req, now_ms)
    }

    pub fn get_algo_prefs(&self, act_as: Option<&str>) -> Result<AlgoPreferencesDto> {
        let viewer_id = self.resolve_caller_actor_id(act_as)?;
        self.store.get_algo_prefs(&viewer_id)
    }

    pub fn update_algo_prefs(
        &self,
        act_as: Option<&str>,
        prefs: &AlgoPreferencesDto,
    ) -> Result<AlgoPreferencesDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let viewer_id = self.resolve_caller_actor_id(act_as)?;
        let saved = self.store.put_algo_prefs(&viewer_id, prefs, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(saved)
    }

    pub fn record_signal(
        &self,
        act_as: Option<&str>,
        req: &RecordSignalRequest,
    ) -> Result<RecordSignalResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let viewer_id = self.resolve_caller_actor_id(act_as)?;
        let resp = self.store.record_signal(&viewer_id, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(resp)
    }

    fn verify_referenced_post_alive(&self, ref_id: Option<&str>) -> Result<()> {
        let Some(pid) = ref_id.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(());
        };
        let target = self
            .store
            .get_post(pid, None)?
            .ok_or_else(|| TelegramError::NotFound(format!("referenced post '{pid}' not found")))?;
        if target.status != PostStatus::Published || target.is_removed {
            return Err(TelegramError::NotFound(format!(
                "referenced post '{pid}' not found"
            )));
        }
        Ok(())
    }

    pub fn create_post(
        &self,
        act_as: Option<&str>,
        req: &CreatePostRequest,
    ) -> Result<CreatePostResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        self.verify_referenced_post_alive(req.quoted_post_id.as_deref())?;
        self.verify_referenced_post_alive(req.reposted_post_id.as_deref())?;
        self.verify_referenced_post_alive(req.crosspost_of_id.as_deref())?;

        let mut resp = self.store.create_post(&caller_id, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();

        let trigger_text = match resp.post.title.as_deref() {
            Some(t) if !t.trim().is_empty() => format!("{t}\n{}", resp.post.body),
            _ => resp.post.body.clone(),
        };
        match self.maybe_trigger_local_mention_bots(
            &resp.post,
            None,
            &caller_id,
            &trigger_text,
            now_ms,
        ) {
            Ok(true) => {
                if let Some(refreshed) = self.store.get_post(&resp.post.post_id, Some(&caller_id))? {
                    resp.post = refreshed;
                }
            }
            Ok(false) => {}
            Err(err) => {
                tracing::debug!(
                    post_id = %resp.post.post_id,
                    error = %err,
                    "Local @mention bot trigger failed after post commit"
                );
            }
        }
        Ok(resp)
    }

    fn can_view_unpublished_post(&self, post: &CommunityPostDto, viewer_id: &str) -> Result<bool> {
        if post.author_actor_id == viewer_id {
            return Ok(true);
        }
        if let Some(uid) = self.bound_user_id() {
            if let Some(author) = self.store.get_actor(&post.author_actor_id, None)? {
                return Ok(can_user_act_as(uid as i64, &author));
            }
        }
        Ok(false)
    }

    pub fn get_post(
        &self,
        act_as: Option<&str>,
        post_id: &str,
        comment_sort: Option<CommentSort>,
    ) -> Result<PostDetailResponse> {
        let now_ms = now_millis();
        self.run_lazy_maintenance(now_ms);
        let viewer_id = self.resolve_caller_actor_id(act_as)?;
        let post = self
            .store
            .get_post(post_id, Some(&viewer_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("post '{post_id}' not found")))?;
        if post.status == PostStatus::Deleted {
            return Err(TelegramError::NotFound(format!("post '{post_id}' not found")));
        }
        if matches!(post.status, PostStatus::Draft | PostStatus::Scheduled)
            && !self.can_view_unpublished_post(&post, &viewer_id)?
        {
            return Err(TelegramError::NotFound(format!("post '{post_id}' not found")));
        }

        let pool = self.store.list_thread_and_quote_pool(&post, Some(&viewer_id))?;
        let thread_chain = ThreadAssembler::build_linear_thread(&post, &pool);
        let quote_chain = ThreadAssembler::build_quote_chain(&post, &pool, 8);
        let comments = self.store.list_comment_tree(
            post_id,
            Some(&viewer_id),
            comment_sort.unwrap_or(CommentSort::Best),
            None,
            200,
        )?;
        Ok(PostDetailResponse {
            total_comments: post.comments_count,
            post,
            thread_chain,
            quote_chain,
            comments,
        })
    }

    pub fn update_post(
        &self,
        act_as: Option<&str>,
        post_id: &str,
        req: &UpdatePostRequest,
    ) -> Result<CommunityPostDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let updated = self.store.update_post(post_id, &caller_id, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(updated)
    }

    pub fn delete_post(&self, act_as: Option<&str>, post_id: &str) -> Result<DeleteEntityResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let deleted = self.store.delete_post(post_id, &caller_id, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(DeleteEntityResponse {
            id: post_id.to_string(),
            deleted,
        })
    }

    pub fn vote_post(
        &self,
        act_as: Option<&str>,
        post_id: &str,
        req: &VoteRequest,
    ) -> Result<VoteResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        self.verify_referenced_post_alive(Some(post_id))?;
        let resp = self
            .store
            .vote_post(&caller_id, post_id, req.direction, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(resp)
    }

    pub fn toggle_post_like(
        &self,
        act_as: Option<&str>,
        post_id: &str,
    ) -> Result<ToggleActionResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        self.verify_referenced_post_alive(Some(post_id))?;
        let resp = self.store.toggle_like_post(&caller_id, post_id, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(resp)
    }

    pub fn repost(
        &self,
        act_as: Option<&str>,
        post_id: &str,
        req: &RepostRequest,
    ) -> Result<RepostResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        self.verify_referenced_post_alive(Some(post_id))?;
        if let Some(comment) = req
            .comment
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let quote_req = CreatePostRequest {
                format: PostFormat::Quote,
                body: comment.to_string(),
                quoted_post_id: Some(post_id.to_string()),
                ..Default::default()
            };
            let created = self.store.create_post(&caller_id, &quote_req, now_ms)?;
            self.mark_guest_dirty_if_unbound();
            let reposts_count = self
                .store
                .get_post(post_id, Some(&caller_id))?
                .map(|p| p.reposts_count)
                .unwrap_or(0);
            return Ok(RepostResponse {
                post_id: post_id.to_string(),
                reposted: true,
                reposts_count,
                repost_post_id: Some(created.post.post_id),
            });
        }
        let resp = self.store.toggle_repost(&caller_id, post_id, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(resp)
    }

    pub fn bookmark_post(
        &self,
        act_as: Option<&str>,
        post_id: &str,
        req: &BookmarkRequest,
    ) -> Result<BookmarkResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        if req.remove != Some(true) {
            self.verify_referenced_post_alive(Some(post_id))?;
        }
        let resp = self.store.toggle_bookmark(
            &caller_id,
            post_id,
            req.folder.as_deref(),
            req.remove,
            now_ms,
        )?;
        self.mark_guest_dirty_if_unbound();
        Ok(resp)
    }

    pub fn vote_poll(
        &self,
        act_as: Option<&str>,
        post_id: &str,
        req: &CommunityPollVoteRequest,
    ) -> Result<PollDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        self.run_lazy_maintenance(now_ms);
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        self.verify_referenced_post_alive(Some(post_id))?;
        let poll = self
            .store
            .vote_poll(&caller_id, post_id, &req.option_ids, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(poll)
    }

    pub fn give_post_award(
        &self,
        act_as: Option<&str>,
        post_id: &str,
        req: &GiveAwardRequest,
    ) -> Result<AwardResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        self.verify_referenced_post_alive(Some(post_id))?;
        let awards = self
            .store
            .give_award(&caller_id, "post", post_id, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(AwardResponse {
            target_id: post_id.to_string(),
            awards,
        })
    }
}

fn find_subtree_children(
    nodes: &[CommunityCommentDto],
    target_id: &str,
) -> Option<Vec<CommunityCommentDto>> {
    let mut stack: Vec<&CommunityCommentDto> = nodes.iter().collect();
    while let Some(node) = stack.pop() {
        if node.comment_id == target_id {
            return Some(node.children.clone());
        }
        for child in &node.children {
            stack.push(child);
        }
    }
    None
}

fn trim_comment_depth(nodes: &mut Vec<CommunityCommentDto>, remaining_depth: u32) {
    for node in nodes.iter_mut() {
        if remaining_depth == 0 {
            let dropped = node.children.len() as u32;
            node.more_children_count = node.more_children_count.saturating_add(dropped);
            node.children.clear();
        } else {
            trim_comment_depth(&mut node.children, remaining_depth - 1);
        }
    }
}

fn parse_offset_cursor(cursor: Option<&str>) -> usize {
    cursor
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|c| c.strip_prefix("offset:").unwrap_or(c).parse::<usize>().ok())
        .unwrap_or(0)
}

impl LocalCommunityRuntime {
    pub fn get_comments(
        &self,
        act_as: Option<&str>,
        post_id: &str,
        params: &CommentsQueryParams,
    ) -> Result<CommentsListResponse> {
        let now_ms = now_millis();
        self.run_lazy_maintenance(now_ms);
        let viewer_id = self.resolve_caller_actor_id(act_as)?;
        let post = self
            .store
            .get_post(post_id, Some(&viewer_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("post '{post_id}' not found")))?;
        if post.status == PostStatus::Deleted
            || post.is_removed
            || (matches!(post.status, PostStatus::Draft | PostStatus::Scheduled)
                && !self.can_view_unpublished_post(&post, &viewer_id)?)
        {
            return Err(TelegramError::NotFound(format!("post '{post_id}' not found")));
        }
        let sort = params.sort.unwrap_or(CommentSort::Best);
        let mut roots = if let Some(pid) = params
            .parent_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let full_tree =
                self.store
                    .list_comment_tree(post_id, Some(&viewer_id), sort, None, 2000)?;
            let mut sub = find_subtree_children(&full_tree, pid).ok_or_else(|| {
                TelegramError::NotFound(format!("parent comment '{pid}' not found"))
            })?;
            if let Some(md) = params.max_depth {
                trim_comment_depth(&mut sub, md);
            }
            sub
        } else {
            self.store
                .list_comment_tree(post_id, Some(&viewer_id), sort, params.max_depth, 2000)?
        };

        let offset = parse_offset_cursor(params.cursor.as_deref());
        let limit = params.limit.unwrap_or(50).clamp(1, 200);
        let total_roots = roots.len();
        let page: Vec<CommunityCommentDto> = roots.drain(..).skip(offset).take(limit).collect();
        let end = offset.saturating_add(page.len());
        let has_more = end < total_roots;
        let next_cursor = has_more.then(|| format!("offset:{end}"));

        Ok(CommentsListResponse {
            post_id: post_id.to_string(),
            sort,
            comments: page,
            total_count: post.comments_count,
            has_more,
            next_cursor,
        })
    }

    pub fn create_comment(
        &self,
        act_as: Option<&str>,
        post_id: &str,
        req: &CreateCommentRequest,
    ) -> Result<CreateCommentResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        self.verify_referenced_post_alive(Some(post_id))?;
        let resp = self
            .store
            .create_comment(&caller_id, post_id, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();

        if !resp.comment.is_removed {
            if let Some(post) = self.store.get_post(post_id, Some(&caller_id))? {
                if let Err(err) = self.maybe_trigger_local_mention_bots(
                    &post,
                    Some(&resp.comment.comment_id),
                    &caller_id,
                    &req.body,
                    now_ms,
                ) {
                    tracing::debug!(
                        post_id = %post_id,
                        comment_id = %resp.comment.comment_id,
                        error = %err,
                        "Local @mention bot trigger failed after comment commit"
                    );
                }
            }
        }
        Ok(resp)
    }

    pub fn update_comment(
        &self,
        act_as: Option<&str>,
        comment_id: &str,
        req: &UpdateCommentRequest,
    ) -> Result<CommunityCommentDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let updated = self
            .store
            .update_comment(comment_id, &caller_id, &req.body, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(updated)
    }

    pub fn delete_comment(
        &self,
        act_as: Option<&str>,
        comment_id: &str,
    ) -> Result<DeleteEntityResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let deleted = self.store.delete_comment(comment_id, &caller_id, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(DeleteEntityResponse {
            id: comment_id.to_string(),
            deleted,
        })
    }

    pub fn vote_comment(
        &self,
        act_as: Option<&str>,
        comment_id: &str,
        req: &VoteRequest,
    ) -> Result<VoteResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let resp = self
            .store
            .vote_comment(&caller_id, comment_id, req.direction, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(resp)
    }

    pub fn toggle_comment_like(
        &self,
        act_as: Option<&str>,
        comment_id: &str,
    ) -> Result<ToggleActionResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let resp = self
            .store
            .toggle_like_comment(&caller_id, comment_id, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(resp)
    }

    pub fn give_comment_award(
        &self,
        act_as: Option<&str>,
        comment_id: &str,
        req: &GiveAwardRequest,
    ) -> Result<AwardResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let awards = self
            .store
            .give_award(&caller_id, "comment", comment_id, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(AwardResponse {
            target_id: comment_id.to_string(),
            awards,
        })
    }

    pub fn list_communities(
        &self,
        act_as: Option<&str>,
        category: Option<&str>,
        limit: Option<usize>,
    ) -> Result<ListCommunitiesResponse> {
        let viewer_id = self.resolve_caller_actor_id(act_as)?;
        let communities =
            self.store
                .list_communities(Some(&viewer_id), category, limit.unwrap_or(50))?;
        Ok(ListCommunitiesResponse { communities })
    }

    pub fn create_community(
        &self,
        act_as: Option<&str>,
        req: &CreateCommunityRequest,
    ) -> Result<SubCommunityDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let comm = self.store.create_community(&caller_id, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(comm)
    }
}

impl LocalCommunityRuntime {
    pub fn get_community(
        &self,
        act_as: Option<&str>,
        slug: &str,
    ) -> Result<CommunityDetailResponse> {
        let viewer_id = self.resolve_caller_actor_id(act_as)?;
        let community = self
            .store
            .get_community(slug, Some(&viewer_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("community '{slug}' not found")))?;
        let mut pinned_posts = Vec::with_capacity(community.pinned_post_ids.len());
        for pid in &community.pinned_post_ids {
            if let Some(p) = self.store.get_post(pid, Some(&viewer_id))? {
                if p.status == PostStatus::Published && !p.is_removed {
                    pinned_posts.push(p);
                }
            }
        }
        let moderators = self
            .store
            .list_moderators_for_viewer(&community.community_id, Some(&viewer_id))?;
        let wiki_revisions = self.store.list_wiki_revisions(&community.community_id, 25)?;
        Ok(CommunityDetailResponse {
            community,
            pinned_posts,
            moderators,
            wiki_revisions,
        })
    }

    pub fn update_community(
        &self,
        act_as: Option<&str>,
        slug: &str,
        req: &UpdateCommunityRequest,
    ) -> Result<SubCommunityDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let updated = self.store.update_community(slug, &caller_id, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(updated)
    }

    pub fn join_community(
        &self,
        act_as: Option<&str>,
        slug: &str,
    ) -> Result<JoinCommunityResponse> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let comm = self.store.join_community(slug, &caller_id, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(JoinCommunityResponse {
            community_slug: comm.slug,
            is_joined: comm.is_joined,
            member_count: comm.member_count,
            viewer_role: comm.viewer_role,
        })
    }

    pub fn leave_community(
        &self,
        act_as: Option<&str>,
        slug: &str,
    ) -> Result<JoinCommunityResponse> {
        self.ensure_local_write_allowed()?;
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let comm = self.store.leave_community(slug, &caller_id)?;
        self.mark_guest_dirty_if_unbound();
        Ok(JoinCommunityResponse {
            community_slug: comm.slug,
            is_joined: comm.is_joined,
            member_count: comm.member_count,
            viewer_role: comm.viewer_role,
        })
    }

    pub fn update_wiki(
        &self,
        act_as: Option<&str>,
        slug: &str,
        req: &UpdateCommunityWikiRequest,
    ) -> Result<CommunityWikiRevisionDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let rev = self
            .store
            .update_wiki(slug, &caller_id, &req.markdown, &req.summary, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(rev)
    }

    pub fn get_mod_queue(
        &self,
        act_as: Option<&str>,
        slug: &str,
        status: Option<ModQueueStatus>,
        limit: Option<usize>,
    ) -> Result<ModQueueResponse> {
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let comm = self
            .store
            .get_community(slug, Some(&caller_id))?
            .ok_or_else(|| TelegramError::NotFound(format!("community '{slug}' not found")))?;
        if !ModPermissionValidator::can_moderate(comm.viewer_role) {
            return Err(TelegramError::PermissionDenied(format!(
                "only moderators of c/{} may view the moderation queue",
                comm.slug
            )));
        }
        let items =
            self.store
                .list_mod_queue(&comm.community_id, status, limit.unwrap_or(50))?;
        Ok(ModQueueResponse { items })
    }

    pub fn execute_mod_action(
        &self,
        act_as: Option<&str>,
        slug: &str,
        req: &ModActionRequest,
    ) -> Result<ModLogEntryDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let entry = self.store.apply_mod_action(slug, &caller_id, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(entry)
    }

    pub fn get_mod_logs(
        &self,
        act_as: Option<&str>,
        slug: &str,
        limit: Option<usize>,
    ) -> Result<ModLogsResponse> {
        let _caller_id = self.resolve_caller_actor_id(act_as)?;
        let logs = self.store.list_mod_logs(slug, limit.unwrap_or(50))?;
        Ok(ModLogsResponse { logs })
    }

    pub fn list_lists(&self, act_as: Option<&str>) -> Result<CommunityListsResponse> {
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let lists = self.store.list_lists(&caller_id)?;
        Ok(CommunityListsResponse { lists })
    }

    pub fn create_list(
        &self,
        act_as: Option<&str>,
        req: &CreateCommunityListRequest,
    ) -> Result<CommunityListDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let list = self.store.create_list(&caller_id, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(list)
    }

    pub fn update_list(
        &self,
        act_as: Option<&str>,
        list_id: &str,
        req: &UpdateCommunityListRequest,
    ) -> Result<CommunityListDto> {
        self.ensure_local_write_allowed()?;
        let now_ms = now_millis();
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let list = self.store.update_list(list_id, &caller_id, req, now_ms)?;
        self.mark_guest_dirty_if_unbound();
        Ok(list)
    }

    pub fn delete_list(&self, act_as: Option<&str>, list_id: &str) -> Result<DeleteEntityResponse> {
        self.ensure_local_write_allowed()?;
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let deleted = self.store.delete_list(list_id, &caller_id)?;
        self.mark_guest_dirty_if_unbound();
        Ok(DeleteEntityResponse {
            id: list_id.to_string(),
            deleted,
        })
    }
}

fn normalize_search_query_aliases(raw: &str) -> String {
    raw.split_whitespace()
        .map(|tok| {
            let lower = tok.to_ascii_lowercase();
            match lower.as_str() {
                "is:bot" | "is:bots" => "kind:bot".to_string(),
                "is:human" | "is:humans" => "kind:human".to_string(),
                "is:hybrid" | "is:hybrid_agent" => "kind:hybrid_agent".to_string(),
                _ => tok.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl LocalCommunityRuntime {
    pub fn get_trending(&self, limit: Option<usize>) -> Result<TrendingResponse> {
        let now_ms = now_millis();
        self.run_lazy_maintenance(now_ms);
        let topics = self.store.get_trending(now_ms, limit.unwrap_or(15))?;
        Ok(TrendingResponse {
            topics,
            generated_at_ms: now_ms,
        })
    }

    pub fn search(
        &self,
        act_as: Option<&str>,
        req: &CommunitySearchRequestDto,
    ) -> Result<CommunitySearchResultDto> {
        let now_ms = now_millis();
        self.run_lazy_maintenance(now_ms);
        let viewer_id = self.resolve_caller_actor_id(act_as)?;
        let normalized_raw = normalize_search_query_aliases(&req.query);
        let parsed = parse_search_operators(&normalized_raw);

        let mut tokens: Vec<String> = Vec::new();
        if !parsed.free_text.trim().is_empty() {
            tokens.push(parsed.free_text.trim().to_string());
        }
        let comm_slug = req.community_slug.as_deref().or(parsed.community.as_deref());
        if let Some(c) = comm_slug.map(str::trim).filter(|s| !s.is_empty()) {
            let lower = c.to_ascii_lowercase();
            let slug = lower
                .strip_prefix("c/")
                .or_else(|| lower.strip_prefix("r/"))
                .unwrap_or(&lower);
            if !slug.is_empty() {
                tokens.push(format!("c:{slug}"));
            }
        }
        let author = req.author_handle.as_deref().or(parsed.author.as_deref());
        if let Some(a) = author.map(str::trim).filter(|s| !s.is_empty()) {
            tokens.push(format!("author:{}", a.trim_start_matches('@')));
        }
        let flair = req.flair.as_deref().or(parsed.flair.as_deref());
        if let Some(f) = flair.map(str::trim).filter(|s| !s.is_empty()) {
            tokens.push(format!("flair:\"{}\"", f.replace('"', "")));
        }
        let actor_kind = req.actor_kind.or(parsed.actor_kind).or(match req.tab {
            CommunitySearchTab::Bots => Some(ActorKind::Bot),
            _ => None,
        });
        if let Some(kind) = actor_kind {
            tokens.push(format!("kind:{}", kind.as_str()));
        }
        if req.has_media == Some(true)
            || parsed.has_media == Some(true)
            || req.tab == CommunitySearchTab::Media
        {
            tokens.push("has:media".to_string());
        }
        if req.has_poll == Some(true)
            || parsed.has_poll == Some(true)
            || req.tab == CommunitySearchTab::Polls
        {
            tokens.push("has:poll".to_string());
        }
        if parsed.has_link == Some(true) {
            tokens.push("has:link".to_string());
        }

        let free_lower = parsed.free_text.to_lowercase();
        let terms: Vec<&str> = free_lower.split_whitespace().collect();
        let limit = req.limit.clamp(1, 100);
        let offset = req.offset;

        let mut posts: Vec<CommunityPostDto> = if matches!(
            req.tab,
            CommunitySearchTab::Top
                | CommunitySearchTab::Latest
                | CommunitySearchTab::Media
                | CommunitySearchTab::Polls
        ) {
            let combined_query = tokens.join(" ");
            let mut hits =
                self.store
                    .search_posts_sql(&combined_query, Some(&viewer_id), 100)?;
            match (req.tab, req.sort) {
                (CommunitySearchTab::Latest, _) | (_, CommunitySearchSort::Latest) => {
                    hits.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
                }
                (_, CommunitySearchSort::Top) => {
                    hits.sort_by(|a, b| {
                        b.score
                            .cmp(&a.score)
                            .then_with(|| b.created_at_ms.cmp(&a.created_at_ms))
                    });
                }
                (_, CommunitySearchSort::Relevance) => {}
            }
            hits
        } else {
            Vec::new()
        };

        let mut actors: Vec<CommunityActorDto> = if matches!(
            req.tab,
            CommunitySearchTab::Top | CommunitySearchTab::People | CommunitySearchTab::Bots
        ) {
            self.store
                .list_actors_for_viewer(None, Some(&viewer_id), 200)?
                .into_iter()
                .filter(|a| {
                    let kind_ok = match req.tab {
                        CommunitySearchTab::Bots => a.actor_kind.is_bot_like(),
                        CommunitySearchTab::People => {
                            if let Some(k) = actor_kind {
                                if k == ActorKind::Bot {
                                    a.actor_kind.is_bot_like()
                                } else {
                                    a.actor_kind == k
                                }
                            } else {
                                true
                            }
                        }
                        _ => actor_kind.map_or(true, |k| {
                            if k == ActorKind::Bot {
                                a.actor_kind.is_bot_like()
                            } else {
                                a.actor_kind == k
                            }
                        }),
                    };
                    if !kind_ok {
                        return false;
                    }
                    if terms.is_empty() {
                        return !matches!(req.tab, CommunitySearchTab::Top);
                    }
                    let hay = format!(
                        "{} {} {} {} {}",
                        a.handle,
                        a.display_name,
                        a.bio,
                        a.bot_capabilities.join(" "),
                        a.bot_persona.as_deref().unwrap_or("")
                    )
                    .to_lowercase();
                    terms.iter().all(|t| hay.contains(t))
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut communities: Vec<SubCommunityDto> =
            if matches!(req.tab, CommunitySearchTab::Top | CommunitySearchTab::Communities) {
                self.store
                    .list_communities(Some(&viewer_id), None, 200)?
                    .into_iter()
                    .filter(|c| {
                        if terms.is_empty() {
                            return req.tab == CommunitySearchTab::Communities;
                        }
                        let hay = format!("{} {} {} {}", c.slug, c.name, c.description, c.category)
                            .to_lowercase();
                        terms.iter().all(|t| hay.contains(t))
                    })
                    .collect()
            } else {
                Vec::new()
            };

        let (total_hits, page_len) = match req.tab {
            CommunitySearchTab::People | CommunitySearchTab::Bots => {
                let total = actors.len();
                actors = actors.into_iter().skip(offset).take(limit).collect();
                (total, actors.len())
            }
            CommunitySearchTab::Communities => {
                let total = communities.len();
                communities = communities.into_iter().skip(offset).take(limit).collect();
                (total, communities.len())
            }
            _ => {
                let total = posts.len();
                posts = posts.into_iter().skip(offset).take(limit).collect();
                if req.tab == CommunitySearchTab::Top {
                    actors.truncate(6);
                    communities.truncate(6);
                }
                (total, posts.len())
            }
        };
        let end = offset.saturating_add(page_len);
        let next_offset = (end < total_hits).then_some(end);

        Ok(CommunitySearchResultDto {
            query: req.query.clone(),
            tab: req.tab,
            posts,
            actors,
            communities,
            total_hits,
            next_offset,
        })
    }

    pub fn get_notifications(
        &self,
        act_as: Option<&str>,
        kind: Option<CommunityNotificationKind>,
        limit: Option<usize>,
        unread_only: Option<bool>,
    ) -> Result<NotificationsListResponse> {
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let requested_limit = limit.unwrap_or(50);
        let fetch_limit = if unread_only == Some(true) {
            requested_limit.max(200)
        } else {
            requested_limit
        };
        let mut resp = self
            .store
            .list_notifications(&caller_id, kind, fetch_limit)?;
        if unread_only == Some(true) {
            resp.notifications.retain(|n| !n.is_read);
            resp.notifications.truncate(requested_limit);
        }
        Ok(resp)
    }

    pub fn mark_notifications_read(
        &self,
        act_as: Option<&str>,
        req: &MarkNotificationsReadRequest,
    ) -> Result<MarkNotificationsReadResponse> {
        self.ensure_local_write_allowed()?;
        let caller_id = self.resolve_caller_actor_id(act_as)?;
        let marked_count = self
            .store
            .mark_notifications_read(&caller_id, req.notif_ids.as_deref())?;
        self.mark_guest_dirty_if_unbound();
        let unread_count = self.store.list_notifications(&caller_id, None, 1)?.unread_count;
        Ok(MarkNotificationsReadResponse {
            marked_count,
            unread_count,
        })
    }
}
