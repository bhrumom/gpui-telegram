//! All 50 async `/api/v1/community/*` methods on `CommunityClientFacade`
//! routing online-first with transparent offline / legacy-404 local fallback.

use super::CommunityClientFacade;
use crate::net::api_community::RegisterBotActorResult;
use gpui_telegram_core::error::Result;
use gpui_telegram_proto::community_contracts::{
    ActorProfileResponse, AlgoPreferencesDto, AwardResponse, BookmarkRequest, BookmarkResponse,
    CommentSort, CommentsListResponse, CommentsQueryParams, CommunityActorDto,
    CommunityCommentDto, CommunityDetailResponse, CommunityIdentitiesResponse, CommunityListDto,
    CommunityListsResponse, CommunityMuteBlockDto, CommunityNotificationKind,
    CommunityPollVoteRequest, CommunityPostDto, CommunitySearchRequestDto,
    CommunitySearchResultDto, CommunityWikiRevisionDto, CreateCommentRequest,
    CreateCommentResponse, CreateCommunityListRequest, CreateCommunityRequest, CreatePostRequest,
    CreatePostResponse, DeleteEntityResponse, FeedRequestDto, FeedResponseDto,
    FollowActionResponse, GiveAwardRequest, JoinCommunityResponse, ListCommunitiesResponse,
    MarkNotificationsReadRequest, MarkNotificationsReadResponse, ModActionRequest, ModLogEntryDto,
    ModLogsResponse, ModQueueResponse, ModQueueStatus, MuteBlockListResponse, MuteBlockRequest,
    NotificationsListResponse, PollDto, PostDetailResponse, RecordSignalRequest,
    RecordSignalResponse, RegisterBotActorRequest, RegisterHumanActorRequest, RepostRequest,
    RepostResponse, SubCommunityDto, SummonBotRequest, SummonBotResponse, ToggleActionResponse,
    TrendingResponse, UpdateCommentRequest, UpdateCommunityActorRequest,
    UpdateCommunityListRequest, UpdateCommunityRequest, UpdateCommunityWikiRequest,
    UpdatePostRequest, VoteRequest, VoteResponse,
};

impl CommunityClientFacade {
    // -------------------------------------------------------------------------
    // A. Identities, Actors, Follow & Mutes/Blocks (1–10)
    // -------------------------------------------------------------------------

    pub async fn get_identities(&self) -> Result<CommunityIdentitiesResponse> {
        let (resp, was_online, start_epoch) = self
            .execute_with_fallback_tagged(
                true,
                None,
                |http, tok, act| async move {
                    http.community_get_identities(tok.as_deref(), act.as_deref())
                        .await
                },
                |local, act| local.get_identities(act.as_deref()),
            )
            .await?;
        if was_online {
            let actors = Self::collect_identities_actors(&resp);
            self.mirror_owned_actors_local(start_epoch, actors).await;
        }
        Ok(resp)
    }

    pub async fn register_human_actor(
        &self,
        req: &RegisterHumanActorRequest,
    ) -> Result<CommunityActorDto> {
        let req_remote = req.clone();
        let req_local = req.clone();
        let (actor, was_online, start_epoch) = self
            .execute_with_fallback_tagged(
                false,
                None,
                move |http, tok, act| async move {
                    http.community_register_human_actor(
                        tok.as_deref(),
                        act.as_deref(),
                        &req_remote,
                    )
                    .await
                },
                move |local, _act| local.register_human_actor(&req_local),
            )
            .await?;
        if was_online {
            self.mirror_owned_actors_local(start_epoch, vec![actor.clone()])
                .await;
        }
        Ok(actor)
    }

    pub async fn register_bot_actor(
        &self,
        req: &RegisterBotActorRequest,
    ) -> Result<RegisterBotActorResult> {
        let req_remote = req.clone();
        let req_local = req.clone();
        let (res, was_online, start_epoch) = self
            .execute_with_fallback_tagged(
                false,
                None,
                move |http, tok, act| async move {
                    http.community_register_bot_actor(tok.as_deref(), act.as_deref(), &req_remote)
                        .await
                },
                move |local, _act| local.register_bot_actor(&req_local),
            )
            .await?;
        if was_online {
            self.mirror_owned_actors_local(start_epoch, vec![res.actor.clone()])
                .await;
        }
        Ok(res)
    }

    pub async fn update_actor(
        &self,
        actor_id: &str,
        req: &UpdateCommunityActorRequest,
    ) -> Result<CommunityActorDto> {
        let aid_remote = actor_id.to_string();
        let aid_local = actor_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_update_actor(tok.as_deref(), act.as_deref(), &aid_remote, &req_remote)
                    .await
            },
            move |local, act| local.update_actor(act.as_deref(), &aid_local, &req_local),
        )
        .await
    }

    pub async fn get_actor_profile(&self, handle_or_id: &str) -> Result<ActorProfileResponse> {
        let h_remote = handle_or_id.to_string();
        let h_local = handle_or_id.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_get_actor_profile(tok.as_deref(), act.as_deref(), &h_remote)
                    .await
            },
            move |local, act| local.get_actor_profile(act.as_deref(), &h_local),
        )
        .await
    }

    pub async fn follow_actor(&self, actor_id: &str) -> Result<FollowActionResponse> {
        let aid_remote = actor_id.to_string();
        let aid_local = actor_id.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_follow_actor(tok.as_deref(), act.as_deref(), &aid_remote)
                    .await
            },
            move |local, act| local.follow_actor(act.as_deref(), &aid_local),
        )
        .await
    }

    pub async fn unfollow_actor(&self, actor_id: &str) -> Result<FollowActionResponse> {
        let aid_remote = actor_id.to_string();
        let aid_local = actor_id.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_unfollow_actor(tok.as_deref(), act.as_deref(), &aid_remote)
                    .await
            },
            move |local, act| local.unfollow_actor(act.as_deref(), &aid_local),
        )
        .await
    }

    pub async fn list_mutes_blocks(&self) -> Result<MuteBlockListResponse> {
        self.execute_with_fallback(
            true,
            None,
            |http, tok, act| async move {
                http.community_list_mutes_blocks(tok.as_deref(), act.as_deref())
                    .await
            },
            |local, act| local.list_mutes_blocks(act.as_deref()),
        )
        .await
    }

    pub async fn set_mute_block(&self, req: &MuteBlockRequest) -> Result<CommunityMuteBlockDto> {
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_set_mute_block(tok.as_deref(), act.as_deref(), &req_remote)
                    .await
            },
            move |local, act| local.set_mute_block(act.as_deref(), &req_local),
        )
        .await
    }

    pub async fn remove_mute_block(&self, req: &MuteBlockRequest) -> Result<DeleteEntityResponse> {
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_remove_mute_block(tok.as_deref(), act.as_deref(), &req_remote)
                    .await
            },
            move |local, act| local.remove_mute_block(act.as_deref(), &req_local),
        )
        .await
    }
}

impl CommunityClientFacade {
    // -------------------------------------------------------------------------
    // B. Feed, Algorithm Preferences & Signals (11–14)
    // -------------------------------------------------------------------------

    pub async fn get_feed(&self, req: &FeedRequestDto) -> Result<FeedResponseDto> {
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            true,
            None,
            move |http, tok, act| async move {
                http.community_get_feed(tok.as_deref(), act.as_deref(), &req_remote)
                    .await
            },
            move |local, act| local.get_feed(act.as_deref(), &req_local),
        )
        .await
    }

    pub async fn get_algo_prefs(&self) -> Result<AlgoPreferencesDto> {
        self.execute_with_fallback(
            true,
            None,
            |http, tok, act| async move {
                http.community_get_algo_prefs(tok.as_deref(), act.as_deref())
                    .await
            },
            |local, act| local.get_algo_prefs(act.as_deref()),
        )
        .await
    }

    pub async fn update_algo_prefs(&self, prefs: &AlgoPreferencesDto) -> Result<AlgoPreferencesDto> {
        let p_remote = prefs.clone();
        let p_local = prefs.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_update_algo_prefs(tok.as_deref(), act.as_deref(), &p_remote)
                    .await
            },
            move |local, act| local.update_algo_prefs(act.as_deref(), &p_local),
        )
        .await
    }

    pub async fn record_signal(&self, req: &RecordSignalRequest) -> Result<RecordSignalResponse> {
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_record_signal(tok.as_deref(), act.as_deref(), &req_remote)
                    .await
            },
            move |local, act| local.record_signal(act.as_deref(), &req_local),
        )
        .await
    }

    // -------------------------------------------------------------------------
    // C. Posts & Post Interactions (15–25)
    // -------------------------------------------------------------------------

    async fn create_post_inner(
        &self,
        explicit_act_as: Option<&str>,
        req: &CreatePostRequest,
    ) -> Result<CreatePostResponse> {
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            explicit_act_as,
            move |http, tok, act| async move {
                http.community_create_post(tok.as_deref(), act.as_deref(), &req_remote)
                    .await
            },
            move |local, act| local.create_post(act.as_deref(), &req_local),
        )
        .await
    }

    pub async fn create_post(&self, req: &CreatePostRequest) -> Result<CreatePostResponse> {
        self.create_post_inner(None, req).await
    }

    pub async fn create_post_as(
        &self,
        actor_id: &str,
        req: &CreatePostRequest,
    ) -> Result<CreatePostResponse> {
        let normalized = crate::net::api_community::validate_and_normalize_actor_id(actor_id)
            .map_err(|msg| gpui_telegram_core::error::TelegramError::Validation(msg.to_string()))?;
        self.create_post_inner(Some(normalized), req).await
    }

    pub async fn get_post(
        &self,
        post_id: &str,
        comment_sort: Option<CommentSort>,
    ) -> Result<PostDetailResponse> {
        let pid_remote = post_id.to_string();
        let pid_local = post_id.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_get_post(tok.as_deref(), act.as_deref(), &pid_remote, comment_sort)
                    .await
            },
            move |local, act| local.get_post(act.as_deref(), &pid_local, comment_sort),
        )
        .await
    }

    pub async fn update_post(
        &self,
        post_id: &str,
        req: &UpdatePostRequest,
    ) -> Result<CommunityPostDto> {
        let pid_remote = post_id.to_string();
        let pid_local = post_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_update_post(tok.as_deref(), act.as_deref(), &pid_remote, &req_remote)
                    .await
            },
            move |local, act| local.update_post(act.as_deref(), &pid_local, &req_local),
        )
        .await
    }

    pub async fn delete_post(&self, post_id: &str) -> Result<DeleteEntityResponse> {
        let pid_remote = post_id.to_string();
        let pid_local = post_id.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_delete_post(tok.as_deref(), act.as_deref(), &pid_remote)
                    .await
            },
            move |local, act| local.delete_post(act.as_deref(), &pid_local),
        )
        .await
    }

    pub async fn vote_post(&self, post_id: &str, req: &VoteRequest) -> Result<VoteResponse> {
        let pid_remote = post_id.to_string();
        let pid_local = post_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_vote_post(tok.as_deref(), act.as_deref(), &pid_remote, &req_remote)
                    .await
            },
            move |local, act| local.vote_post(act.as_deref(), &pid_local, &req_local),
        )
        .await
    }

    pub async fn toggle_post_like(&self, post_id: &str) -> Result<ToggleActionResponse> {
        let pid_remote = post_id.to_string();
        let pid_local = post_id.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_toggle_post_like(tok.as_deref(), act.as_deref(), &pid_remote)
                    .await
            },
            move |local, act| local.toggle_post_like(act.as_deref(), &pid_local),
        )
        .await
    }

    pub async fn repost(&self, post_id: &str, req: &RepostRequest) -> Result<RepostResponse> {
        let pid_remote = post_id.to_string();
        let pid_local = post_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_repost(tok.as_deref(), act.as_deref(), &pid_remote, &req_remote)
                    .await
            },
            move |local, act| local.repost(act.as_deref(), &pid_local, &req_local),
        )
        .await
    }

    pub async fn bookmark_post(
        &self,
        post_id: &str,
        req: &BookmarkRequest,
    ) -> Result<BookmarkResponse> {
        let pid_remote = post_id.to_string();
        let pid_local = post_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_bookmark_post(tok.as_deref(), act.as_deref(), &pid_remote, &req_remote)
                    .await
            },
            move |local, act| local.bookmark_post(act.as_deref(), &pid_local, &req_local),
        )
        .await
    }
}

impl CommunityClientFacade {
    pub async fn vote_poll(
        &self,
        post_id: &str,
        req: &CommunityPollVoteRequest,
    ) -> Result<PollDto> {
        let pid_remote = post_id.to_string();
        let pid_local = post_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_vote_poll(tok.as_deref(), act.as_deref(), &pid_remote, &req_remote)
                    .await
            },
            move |local, act| local.vote_poll(act.as_deref(), &pid_local, &req_local),
        )
        .await
    }

    pub async fn give_post_award(
        &self,
        post_id: &str,
        req: &GiveAwardRequest,
    ) -> Result<AwardResponse> {
        let pid_remote = post_id.to_string();
        let pid_local = post_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_give_post_award(
                    tok.as_deref(),
                    act.as_deref(),
                    &pid_remote,
                    &req_remote,
                )
                .await
            },
            move |local, act| local.give_post_award(act.as_deref(), &pid_local, &req_local),
        )
        .await
    }

    pub async fn summon_bot(
        &self,
        post_id: &str,
        req: &SummonBotRequest,
    ) -> Result<SummonBotResponse> {
        let pid_remote = post_id.to_string();
        let pid_local = post_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_summon_bot(tok.as_deref(), act.as_deref(), &pid_remote, &req_remote)
                    .await
            },
            move |local, act| local.summon_bot(act.as_deref(), &pid_local, &req_local),
        )
        .await
    }

    // -------------------------------------------------------------------------
    // D. Comments & Comment Interactions (26–32)
    // -------------------------------------------------------------------------

    pub async fn get_comments(
        &self,
        post_id: &str,
        params: &CommentsQueryParams,
    ) -> Result<CommentsListResponse> {
        let pid_remote = post_id.to_string();
        let pid_local = post_id.to_string();
        let p_remote = params.clone();
        let p_local = params.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_get_comments(tok.as_deref(), act.as_deref(), &pid_remote, &p_remote)
                    .await
            },
            move |local, act| local.get_comments(act.as_deref(), &pid_local, &p_local),
        )
        .await
    }

    pub async fn create_comment(
        &self,
        post_id: &str,
        req: &CreateCommentRequest,
    ) -> Result<CreateCommentResponse> {
        let pid_remote = post_id.to_string();
        let pid_local = post_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_create_comment(
                    tok.as_deref(),
                    act.as_deref(),
                    &pid_remote,
                    &req_remote,
                )
                .await
            },
            move |local, act| local.create_comment(act.as_deref(), &pid_local, &req_local),
        )
        .await
    }

    pub async fn update_comment(
        &self,
        comment_id: &str,
        req: &UpdateCommentRequest,
    ) -> Result<CommunityCommentDto> {
        let cid_remote = comment_id.to_string();
        let cid_local = comment_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_update_comment(
                    tok.as_deref(),
                    act.as_deref(),
                    &cid_remote,
                    &req_remote,
                )
                .await
            },
            move |local, act| local.update_comment(act.as_deref(), &cid_local, &req_local),
        )
        .await
    }

    pub async fn delete_comment(&self, comment_id: &str) -> Result<DeleteEntityResponse> {
        let cid_remote = comment_id.to_string();
        let cid_local = comment_id.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_delete_comment(tok.as_deref(), act.as_deref(), &cid_remote)
                    .await
            },
            move |local, act| local.delete_comment(act.as_deref(), &cid_local),
        )
        .await
    }

    pub async fn vote_comment(
        &self,
        comment_id: &str,
        req: &VoteRequest,
    ) -> Result<VoteResponse> {
        let cid_remote = comment_id.to_string();
        let cid_local = comment_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_vote_comment(tok.as_deref(), act.as_deref(), &cid_remote, &req_remote)
                    .await
            },
            move |local, act| local.vote_comment(act.as_deref(), &cid_local, &req_local),
        )
        .await
    }

    pub async fn toggle_comment_like(&self, comment_id: &str) -> Result<ToggleActionResponse> {
        let cid_remote = comment_id.to_string();
        let cid_local = comment_id.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_toggle_comment_like(tok.as_deref(), act.as_deref(), &cid_remote)
                    .await
            },
            move |local, act| local.toggle_comment_like(act.as_deref(), &cid_local),
        )
        .await
    }

    pub async fn give_comment_award(
        &self,
        comment_id: &str,
        req: &GiveAwardRequest,
    ) -> Result<AwardResponse> {
        let cid_remote = comment_id.to_string();
        let cid_local = comment_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_give_comment_award(
                    tok.as_deref(),
                    act.as_deref(),
                    &cid_remote,
                    &req_remote,
                )
                .await
            },
            move |local, act| local.give_comment_award(act.as_deref(), &cid_local, &req_local),
        )
        .await
    }
}

impl CommunityClientFacade {
    // -------------------------------------------------------------------------
    // E. Communities (c/<slug>), Wiki & Moderation (33–42)
    // -------------------------------------------------------------------------

    pub async fn list_communities(
        &self,
        category: Option<&str>,
        limit: Option<usize>,
    ) -> Result<ListCommunitiesResponse> {
        let cat_remote = category.map(str::to_string);
        let cat_local = category.map(str::to_string);
        self.execute_with_fallback(
            true,
            None,
            move |http, tok, act| async move {
                http.community_list_communities(
                    tok.as_deref(),
                    act.as_deref(),
                    cat_remote.as_deref(),
                    limit,
                )
                .await
            },
            move |local, act| local.list_communities(act.as_deref(), cat_local.as_deref(), limit),
        )
        .await
    }

    pub async fn create_community(&self, req: &CreateCommunityRequest) -> Result<SubCommunityDto> {
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_create_community(tok.as_deref(), act.as_deref(), &req_remote)
                    .await
            },
            move |local, act| local.create_community(act.as_deref(), &req_local),
        )
        .await
    }

    pub async fn get_community(&self, slug: &str) -> Result<CommunityDetailResponse> {
        let s_remote = slug.to_string();
        let s_local = slug.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_get_community(tok.as_deref(), act.as_deref(), &s_remote)
                    .await
            },
            move |local, act| local.get_community(act.as_deref(), &s_local),
        )
        .await
    }

    pub async fn update_community(
        &self,
        slug: &str,
        req: &UpdateCommunityRequest,
    ) -> Result<SubCommunityDto> {
        let s_remote = slug.to_string();
        let s_local = slug.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_update_community(
                    tok.as_deref(),
                    act.as_deref(),
                    &s_remote,
                    &req_remote,
                )
                .await
            },
            move |local, act| local.update_community(act.as_deref(), &s_local, &req_local),
        )
        .await
    }

    pub async fn join_community(&self, slug: &str) -> Result<JoinCommunityResponse> {
        let s_remote = slug.to_string();
        let s_local = slug.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_join_community(tok.as_deref(), act.as_deref(), &s_remote)
                    .await
            },
            move |local, act| local.join_community(act.as_deref(), &s_local),
        )
        .await
    }

    pub async fn leave_community(&self, slug: &str) -> Result<JoinCommunityResponse> {
        let s_remote = slug.to_string();
        let s_local = slug.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_leave_community(tok.as_deref(), act.as_deref(), &s_remote)
                    .await
            },
            move |local, act| local.leave_community(act.as_deref(), &s_local),
        )
        .await
    }

    pub async fn update_wiki(
        &self,
        slug: &str,
        req: &UpdateCommunityWikiRequest,
    ) -> Result<CommunityWikiRevisionDto> {
        let s_remote = slug.to_string();
        let s_local = slug.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_update_wiki(tok.as_deref(), act.as_deref(), &s_remote, &req_remote)
                    .await
            },
            move |local, act| local.update_wiki(act.as_deref(), &s_local, &req_local),
        )
        .await
    }

    pub async fn get_mod_queue(
        &self,
        slug: &str,
        status: Option<ModQueueStatus>,
        limit: Option<usize>,
    ) -> Result<ModQueueResponse> {
        let s_remote = slug.to_string();
        let s_local = slug.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_get_mod_queue(
                    tok.as_deref(),
                    act.as_deref(),
                    &s_remote,
                    status,
                    limit,
                )
                .await
            },
            move |local, act| local.get_mod_queue(act.as_deref(), &s_local, status, limit),
        )
        .await
    }

    pub async fn execute_mod_action(
        &self,
        slug: &str,
        req: &ModActionRequest,
    ) -> Result<ModLogEntryDto> {
        let s_remote = slug.to_string();
        let s_local = slug.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_execute_mod_action(
                    tok.as_deref(),
                    act.as_deref(),
                    &s_remote,
                    &req_remote,
                )
                .await
            },
            move |local, act| local.execute_mod_action(act.as_deref(), &s_local, &req_local),
        )
        .await
    }

    pub async fn get_mod_logs(&self, slug: &str, limit: Option<usize>) -> Result<ModLogsResponse> {
        let s_remote = slug.to_string();
        let s_local = slug.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_get_mod_logs(tok.as_deref(), act.as_deref(), &s_remote, limit)
                    .await
            },
            move |local, act| local.get_mod_logs(act.as_deref(), &s_local, limit),
        )
        .await
    }
}

impl CommunityClientFacade {
    // -------------------------------------------------------------------------
    // F. Lists & Custom Feeds (43–46)
    // -------------------------------------------------------------------------

    pub async fn list_lists(&self) -> Result<CommunityListsResponse> {
        self.execute_with_fallback(
            true,
            None,
            |http, tok, act| async move {
                http.community_list_lists(tok.as_deref(), act.as_deref())
                    .await
            },
            |local, act| local.list_lists(act.as_deref()),
        )
        .await
    }

    pub async fn create_list(&self, req: &CreateCommunityListRequest) -> Result<CommunityListDto> {
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_create_list(tok.as_deref(), act.as_deref(), &req_remote)
                    .await
            },
            move |local, act| local.create_list(act.as_deref(), &req_local),
        )
        .await
    }

    pub async fn update_list(
        &self,
        list_id: &str,
        req: &UpdateCommunityListRequest,
    ) -> Result<CommunityListDto> {
        let lid_remote = list_id.to_string();
        let lid_local = list_id.to_string();
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_update_list(tok.as_deref(), act.as_deref(), &lid_remote, &req_remote)
                    .await
            },
            move |local, act| local.update_list(act.as_deref(), &lid_local, &req_local),
        )
        .await
    }

    pub async fn delete_list(&self, list_id: &str) -> Result<DeleteEntityResponse> {
        let lid_remote = list_id.to_string();
        let lid_local = list_id.to_string();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_delete_list(tok.as_deref(), act.as_deref(), &lid_remote)
                    .await
            },
            move |local, act| local.delete_list(act.as_deref(), &lid_local),
        )
        .await
    }

    // -------------------------------------------------------------------------
    // G. Trending, Search & Notifications (47–50)
    // -------------------------------------------------------------------------

    pub async fn get_trending(&self, limit: Option<usize>) -> Result<TrendingResponse> {
        self.execute_with_fallback(
            true,
            None,
            move |http, tok, act| async move {
                http.community_get_trending(tok.as_deref(), act.as_deref(), limit)
                    .await
            },
            move |local, _act| local.get_trending(limit),
        )
        .await
    }

    pub async fn search(&self, req: &CommunitySearchRequestDto) -> Result<CommunitySearchResultDto> {
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            true,
            None,
            move |http, tok, act| async move {
                http.community_search(tok.as_deref(), act.as_deref(), &req_remote)
                    .await
            },
            move |local, act| local.search(act.as_deref(), &req_local),
        )
        .await
    }

    pub async fn get_notifications(
        &self,
        kind: Option<CommunityNotificationKind>,
        limit: Option<usize>,
        unread_only: Option<bool>,
    ) -> Result<NotificationsListResponse> {
        self.execute_with_fallback(
            true,
            None,
            move |http, tok, act| async move {
                http.community_get_notifications(
                    tok.as_deref(),
                    act.as_deref(),
                    kind,
                    limit,
                    unread_only,
                )
                .await
            },
            move |local, act| local.get_notifications(act.as_deref(), kind, limit, unread_only),
        )
        .await
    }

    pub async fn mark_notifications_read(
        &self,
        req: &MarkNotificationsReadRequest,
    ) -> Result<MarkNotificationsReadResponse> {
        let req_remote = req.clone();
        let req_local = req.clone();
        self.execute_with_fallback(
            false,
            None,
            move |http, tok, act| async move {
                http.community_mark_notifications_read(tok.as_deref(), act.as_deref(), &req_remote)
                    .await
            },
            move |local, act| local.mark_notifications_read(act.as_deref(), &req_local),
        )
        .await
    }
}
