//! Integration tests for Feature F14 (`gpui_telegram_client` Community HTTP API,
//! WebSocket `ClientDomainEvent::CommunityEvent` dispatch, account-scoped `LocalCommunityRuntime`,
//! online-first + transparent offline/legacy-404 fallback, local reactive bots, and concurrency).

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{any, get, patch, post, put};
use axum::Router;
use gpui_telegram_client::{
    AuthResponse, ClientConfig, ClientDomainEvent, CommunityAuth, CommunityRuntimeMode,
    TelegramBotSyncInput, TelegramClient, TelegramHumanSyncInput, COMMUNITY_DB_FILENAME,
    HEADER_COMMUNITY_ACTOR_ID,
};
use gpui_telegram_core::error::TelegramError;
use gpui_telegram_proto::community_contracts::{
    ActorFilter, ActorKind, ActorProfileResponse, AlgoPreferencesDto, AlgoPreset, AwardResponse,
    BookmarkRequest, BookmarkResponse, BotAutonomyMode, CommentSort, CommentsListResponse,
    CommentsQueryParams, CommunityActorDto, CommunityCommentDto, CommunityDetailResponse,
    CommunityIdentitiesResponse, CommunityListDto, CommunityListKind, CommunityListsResponse,
    CommunityMuteBlockDto, CommunityNotificationKind, CommunityPollVoteRequest, CommunityPostDto,
    CommunitySearchRequestDto, CommunitySearchResultDto, CommunitySearchSort, CommunitySearchTab,
    CommunityWikiRevisionDto, CreateCommentRequest, CreateCommentResponse,
    CreateCommunityListRequest, CreateCommunityRequest, CreatePostRequest, CreatePostResponse,
    DeleteEntityResponse, FeedRequestDto, FeedResponseDto, FeedSort, FeedTab, FollowActionResponse,
    GiveAwardRequest, JoinCommunityResponse, ListCommunitiesResponse, MarkNotificationsReadRequest,
    MarkNotificationsReadResponse, ModActionKind, ModActionRequest, ModLogEntryDto,
    ModLogsResponse, ModQueueResponse, ModQueueStatus, MuteBlockActionKind, MuteBlockListResponse,
    MuteBlockRequest, MuteBlockTargetKind, NotificationsListResponse, PollDto, PostDetailResponse,
    PostFormat, PostStatus, RecordSignalRequest, RecordSignalResponse, RegisterBotActorRequest,
    RegisterHumanActorRequest, RepostRequest, RepostResponse, SubCommunityDto, SummonBotRequest,
    SummonBotResponse, TimeWindow, ToggleActionResponse, TrendingResponse, UpdateCommentRequest,
    UpdateCommunityActorRequest, UpdateCommunityListRequest, UpdateCommunityRequest,
    UpdateCommunityWikiRequest, UpdatePostRequest, VerifiedTier, VoteRequest, VoteResponse,
};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use parking_lot::Mutex;
use serde_json::json;
use std::collections::HashMap;
use std::net::TcpListener as StdTcpListener;
use std::path::Path as FsPath;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[derive(Debug, Clone)]
struct CapturedReq {
    method: String,
    path: String,
    query: HashMap<String, String>,
    auth: Option<String>,
    actor_id: Option<String>,
    device_id: Option<String>,
    body: String,
}

#[derive(Clone, Default)]
struct MockState {
    captured: Arc<Mutex<Vec<CapturedReq>>>,
    bare_bot_register: Arc<Mutex<bool>>,
}

impl MockState {
    fn record(
        &self,
        method: &str,
        path: &str,
        query: HashMap<String, String>,
        headers: &HeaderMap,
        body: &Bytes,
    ) {
        self.captured.lock().push(CapturedReq {
            method: method.to_string(),
            path: path.to_string(),
            query,
            auth: headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            actor_id: headers
                .get("x-community-actor-id")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            device_id: headers
                .get("x-device-id")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            body: String::from_utf8_lossy(body).to_string(),
        });
    }
}

struct SpawnedServer {
    base_url: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl SpawnedServer {
    async fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
}

impl Drop for SpawnedServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

async fn spawn_router(app: Router) -> SpawnedServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let (tx, rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
    });
    SpawnedServer {
        base_url: format!("http://127.0.0.1:{}", addr.port()),
        shutdown_tx: Some(tx),
    }
}

fn unused_closed_port_url() -> String {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

fn make_auth_session(user_id: u64, username: &str, token: &str) -> AuthResponse {
    AuthResponse {
        user_id,
        username: username.to_string(),
        session_id: format!("sess_{user_id}"),
        token: token.to_string(),
        first_name: Some(username.to_string()),
        created_at: None,
        expires_at: None,
    }
}

fn open_client(server_url: &str, dir: &FsPath) -> TelegramClient {
    let cfg = ClientConfig {
        server_url: server_url.to_string(),
        http_timeout_secs: 5,
        ..Default::default()
    };
    TelegramClient::open_with_dir(cfg, dir).expect("open_with_dir")
}

async fn drop_client(client: TelegramClient) {
    client.disconnect_ws();
    tokio::task::spawn_blocking(move || drop(client))
        .await
        .expect("drop_client spawn_blocking");
}

fn sample_actor(id: &str, handle: &str, kind: ActorKind) -> CommunityActorDto {
    CommunityActorDto {
        actor_id: id.to_string(),
        linked_user_id: Some(42),
        owner_user_id: Some(1001),
        actor_kind: kind,
        handle: handle.to_string(),
        display_name: format!("Display {handle}"),
        bio: "bio".to_string(),
        location: None,
        website: None,
        avatar_media_id: None,
        banner_media_id: None,
        avatar_shape: "classic".to_string(),
        avatar_color: "blue".to_string(),
        verified_tier: VerifiedTier::OfficialBot,
        bot_badge: (kind == ActorKind::Bot).then(|| "AI".to_string()),
        bot_model: None,
        bot_capabilities: vec!["rust".to_string()],
        bot_autonomy_mode: BotAutonomyMode::Reactive,
        bot_persona: Some("Helpful".to_string()),
        post_karma: 10,
        comment_karma: 5,
        awardee_karma: 0,
        followers_count: 3,
        following_count: 1,
        posts_count: 2,
        trust_score: 0.9,
        is_following: false,
        is_muted: false,
        is_blocked: false,
        created_at_ms: 1_730_000_000_000,
    }
}

fn sample_post(id: &str) -> CommunityPostDto {
    CommunityPostDto {
        post_id: id.to_string(),
        community_id: Some("comm_rust".to_string()),
        community_slug: Some("rust".to_string()),
        author_actor_id: "b:42".to_string(),
        author_handle: "rust_bot".to_string(),
        author_display_name: "Rust Bot".to_string(),
        author_kind: ActorKind::Bot,
        author_verified_tier: VerifiedTier::OfficialBot,
        author_bot_badge: Some("AI".to_string()),
        author_avatar_media_id: None,
        author_avatar_shape: "classic".to_string(),
        author_avatar_color: "orange".to_string(),
        co_author_actor_id: None,
        format: PostFormat::Micro,
        status: PostStatus::Published,
        title: Some("Remote Title".to_string()),
        body: "Remote Body".to_string(),
        flair: None,
        tags: vec!["rust".to_string()],
        mentions: vec![],
        media: vec![],
        link_preview: None,
        poll: None,
        quoted_post_id: None,
        quoted_post: None,
        reposted_post_id: None,
        reposted_post: None,
        crosspost_of_id: None,
        crosspost_origin: None,
        thread_root_id: None,
        parent_post_id: None,
        thread_index: 0,
        thread_total: 1,
        is_nsfw: false,
        is_spoiler: false,
        is_pinned: false,
        is_locked: false,
        is_removed: false,
        is_mod_distinguished: false,
        upvotes: 5,
        downvotes: 0,
        score: 5,
        likes_count: 2,
        reposts_count: 1,
        quotes_count: 0,
        comments_count: 1,
        bookmarks_count: 0,
        views_count: 10,
        awards: vec![],
        viewer_vote: 1,
        viewer_liked: false,
        viewer_reposted: false,
        viewer_bookmarked: false,
        viewer_bookmark_folder: None,
        edit_history: vec![],
        scheduled_at_ms: None,
        created_at_ms: 1_730_000_000_000,
        updated_at_ms: 1_730_000_000_000,
    }
}

fn sample_comment(id: &str, post_id: &str) -> CommunityCommentDto {
    CommunityCommentDto {
        comment_id: id.to_string(),
        post_id: post_id.to_string(),
        parent_comment_id: None,
        depth: 0,
        path: id.to_string(),
        author_actor_id: "b:42".to_string(),
        author_handle: "rust_bot".to_string(),
        author_display_name: "Rust Bot".to_string(),
        author_kind: ActorKind::Bot,
        author_verified_tier: VerifiedTier::OfficialBot,
        author_bot_badge: Some("AI".to_string()),
        author_avatar_shape: "classic".to_string(),
        author_avatar_color: "orange".to_string(),
        is_op: false,
        body: "Remote comment".to_string(),
        media: vec![],
        upvotes: 1,
        downvotes: 0,
        score: 1,
        likes_count: 0,
        awards: vec![],
        viewer_vote: 0,
        viewer_liked: false,
        is_stickied: false,
        is_mod_distinguished: false,
        is_removed: false,
        is_deleted: false,
        children: vec![],
        more_children_count: 0,
        created_at_ms: 1_730_000_000_000,
        updated_at_ms: 1_730_000_000_000,
    }
}

fn sample_community(slug: &str) -> SubCommunityDto {
    SubCommunityDto {
        community_id: format!("comm_{slug}"),
        slug: slug.to_string(),
        name: format!("c/{slug}"),
        description: "Mock community".to_string(),
        category: "tech".to_string(),
        banner_media_id: None,
        icon_media_id: None,
        icon_emoji: "🦀".to_string(),
        accent_color: "#f97316".to_string(),
        rules: vec![],
        post_flairs: vec![],
        user_flairs: vec![],
        wiki_markdown: "# Wiki".to_string(),
        wiki_revision: 1,
        pinned_post_ids: vec![],
        automod_rules: vec![],
        allow_bots: true,
        require_bot_disclosure: true,
        is_nsfw: false,
        member_count: 10,
        human_member_count: 6,
        bot_member_count: 4,
        posts_count: 1,
        viewer_role: None,
        is_joined: true,
        notifications_enabled: true,
        created_by_actor_id: "u:1001".to_string(),
        created_at_ms: 1_730_000_000_000,
    }
}

fn build_full_mock_router(state: MockState) -> Router {
    Router::new()
        .route(
            "/api/v1/community/identities",
            get(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("GET", "/api/v1/community/identities", q, &h, &b);
                let bot = sample_actor("b:42", "rust_bot", ActorKind::Bot);
                Json(CommunityIdentitiesResponse {
                    active_actor_id: "b:42".to_string(),
                    human_actor: Some(sample_actor("u:1001", "alice", ActorKind::Human)),
                    bot_actors: vec![bot.clone()],
                    all_actors: vec![sample_actor("u:1001", "alice", ActorKind::Human), bot],
                })
            }),
        )
        .route(
            "/api/v1/community/identities/human",
            post(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("POST", "/api/v1/community/identities/human", q, &h, &b);
                Json(sample_actor("u:1001", "alice", ActorKind::Human))
            }),
        )
        .route(
            "/api/v1/community/identities/bot",
            post(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("POST", "/api/v1/community/identities/bot", q, &h, &b);
                let dto = sample_actor("b:42", "rust_bot", ActorKind::Bot);
                if *st.bare_bot_register.lock() {
                    Json(serde_json::to_value(dto).unwrap()).into_response()
                } else {
                    Json(json!({
                        "actor": dto,
                        "bot_token": "tok_bot_42",
                        "bot_user_id": 42,
                        "username": "rust_bot"
                    }))
                    .into_response()
                }
            }),
        )
        .route(
            "/api/v1/community/actors/{handle_or_id}",
            get(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("GET", &format!("/api/v1/community/actors/{id}"), q, &h, &b);
                Json(ActorProfileResponse {
                    actor: sample_actor(&id, "rust_bot", ActorKind::Bot),
                    recent_posts: vec![sample_post("p_remote_1")],
                    recent_comments: vec![],
                    moderated_communities: vec!["rust".to_string()],
                })
            })
            .patch(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("PATCH", &format!("/api/v1/community/actors/{id}"), q, &h, &b);
                Json(sample_actor(&id, "rust_bot", ActorKind::Bot))
            }),
        )
        .route(
            "/api/v1/community/actors/{actor_id}/follow",
            post(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("POST", &format!("/api/v1/community/actors/{id}/follow"), q, &h, &b);
                Json(FollowActionResponse { actor_id: id, is_following: true, followers_count: 4 })
            })
            .delete(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("DELETE", &format!("/api/v1/community/actors/{id}/follow"), q, &h, &b);
                Json(FollowActionResponse { actor_id: id, is_following: false, followers_count: 3 })
            }),
        )
        .route(
            "/api/v1/community/mutes-blocks",
            get(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("GET", "/api/v1/community/mutes-blocks", q, &h, &b);
                Json(MuteBlockListResponse { items: vec![] })
            })
            .post(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("POST", "/api/v1/community/mutes-blocks", q, &h, &b);
                Json(CommunityMuteBlockDto {
                    id: "mb_1".to_string(),
                    owner_actor_id: "b:42".to_string(),
                    target_kind: MuteBlockTargetKind::Keyword,
                    target_value: "spam".to_string(),
                    action: MuteBlockActionKind::Mute,
                    reason: None,
                    created_at_ms: 1,
                })
            })
            .delete(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("DELETE", "/api/v1/community/mutes-blocks", q, &h, &b);
                Json(DeleteEntityResponse { id: "keyword:mute:spam".to_string(), deleted: true })
            }),
        )
        .route(
            "/api/v1/community/feed",
            get(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("GET", "/api/v1/community/feed", q, &h, &b);
                Json(FeedResponseDto {
                    items: vec![],
                    next_cursor: None,
                    total_candidates: 0,
                    applied_preset: AlgoPreset::Balanced,
                    generated_at_ms: 100,
                })
            }),
        )
        .route(
            "/api/v1/community/algo-prefs",
            get(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("GET", "/api/v1/community/algo-prefs", q, &h, &b);
                Json(AlgoPreferencesDto::default())
            })
            .put(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("PUT", "/api/v1/community/algo-prefs", q, &h, &b);
                let parsed: AlgoPreferencesDto = serde_json::from_slice(&b).unwrap_or_default();
                Json(parsed)
            }),
        )
        .route(
            "/api/v1/community/signals",
            post(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("POST", "/api/v1/community/signals", q, &h, &b);
                Json(RecordSignalResponse { recorded: true, signal_id: "sig_1".to_string() })
            }),
        )
        .route(
            "/api/v1/community/posts",
            post(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("POST", "/api/v1/community/posts", q, &h, &b);
                Json(CreatePostResponse {
                    post: sample_post("p_remote_1"),
                    thread_posts: vec![],
                    automod_action: Default::default(),
                    automod_note: None,
                })
            }),
        )
        .route(
            "/api/v1/community/posts/{post_id}",
            get(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("GET", &format!("/api/v1/community/posts/{id}"), q, &h, &b);
                Json(PostDetailResponse {
                    post: sample_post(&id),
                    thread_chain: vec![],
                    quote_chain: vec![],
                    comments: vec![],
                    total_comments: 0,
                })
            })
            .patch(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("PATCH", &format!("/api/v1/community/posts/{id}"), q, &h, &b);
                Json(sample_post(&id))
            })
            .delete(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("DELETE", &format!("/api/v1/community/posts/{id}"), q, &h, &b);
                Json(DeleteEntityResponse { id, deleted: true })
            }),
        )
        .merge(build_full_mock_router_part2())
        .with_state(state)
}

fn build_full_mock_router_part2() -> Router<MockState> {
    Router::new()
        .route("/api/v1/community/posts/{id}/vote", post(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("POST", &format!("/api/v1/community/posts/{id}/vote"), q, &h, &b);
            Json(VoteResponse { target_id: id, direction: 1, upvotes: 6, downvotes: 0, score: 6 })
        }))
        .route("/api/v1/community/posts/{id}/like", post(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("POST", &format!("/api/v1/community/posts/{id}/like"), q, &h, &b);
            Json(ToggleActionResponse { target_id: id, active: true, count: 3 })
        }))
        .route("/api/v1/community/posts/{id}/repost", post(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("POST", &format!("/api/v1/community/posts/{id}/repost"), q, &h, &b);
            Json(RepostResponse { post_id: id, reposted: true, reposts_count: 2, repost_post_id: None })
        }))
        .route("/api/v1/community/posts/{id}/bookmark", post(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("POST", &format!("/api/v1/community/posts/{id}/bookmark"), q, &h, &b);
            Json(BookmarkResponse { post_id: id, bookmarked: true, folder: "All".into(), bookmarks_count: 1 })
        }))
        .route("/api/v1/community/posts/{id}/poll-vote", post(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("POST", &format!("/api/v1/community/posts/{id}/poll-vote"), q, &h, &b);
            Json(PollDto {
                poll_id: "poll_1".into(), question: "Q?".into(), options: vec![], multi_select: false,
                expires_at_ms: 9_999_999_999_999, is_closed: false, total_votes: 1, total_human_votes: 0,
                total_bot_votes: 1, viewer_voted_option_ids: vec!["opt_1".into()],
            })
        }))
        .route("/api/v1/community/posts/{id}/award", post(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("POST", &format!("/api/v1/community/posts/{id}/award"), q, &h, &b);
            Json(AwardResponse { target_id: id, awards: vec![] })
        }))
        .route("/api/v1/community/posts/{id}/summon-bot", post(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("POST", &format!("/api/v1/community/posts/{id}/summon-bot"), q, &h, &b);
            Json(SummonBotResponse {
                summoned_bot_actor_id: "b:42".into(), triggered: true, reason: "ok".into(),
                created_comment: Some(sample_comment("c_bot_1", &id)),
            })
        }))
        .route("/api/v1/community/posts/{id}/comments",
            get(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("GET", &format!("/api/v1/community/posts/{id}/comments"), q, &h, &b);
                Json(CommentsListResponse {
                    post_id: id, sort: CommentSort::Top, comments: vec![], total_count: 0, has_more: false, next_cursor: None,
                })
            })
            .post(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("POST", &format!("/api/v1/community/posts/{id}/comments"), q, &h, &b);
                Json(CreateCommentResponse {
                    comment: sample_comment("c_1", &id), automod_action: Default::default(), automod_note: None,
                })
            }),
        )
        .route("/api/v1/community/comments/{id}",
            patch(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("PATCH", &format!("/api/v1/community/comments/{id}"), q, &h, &b);
                Json(sample_comment(&id, "p_remote_1"))
            })
            .delete(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("DELETE", &format!("/api/v1/community/comments/{id}"), q, &h, &b);
                Json(DeleteEntityResponse { id, deleted: true })
            }),
        )
        .route("/api/v1/community/comments/{id}/vote", post(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("POST", &format!("/api/v1/community/comments/{id}/vote"), q, &h, &b);
            Json(VoteResponse { target_id: id, direction: 1, upvotes: 2, downvotes: 0, score: 2 })
        }))
        .route("/api/v1/community/comments/{id}/like", post(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("POST", &format!("/api/v1/community/comments/{id}/like"), q, &h, &b);
            Json(ToggleActionResponse { target_id: id, active: true, count: 1 })
        }))
        .route("/api/v1/community/comments/{id}/award", post(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("POST", &format!("/api/v1/community/comments/{id}/award"), q, &h, &b);
            Json(AwardResponse { target_id: id, awards: vec![] })
        }))
        .route("/api/v1/community/communities",
            get(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("GET", "/api/v1/community/communities", q, &h, &b);
                Json(ListCommunitiesResponse { communities: vec![sample_community("rust")] })
            })
            .post(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("POST", "/api/v1/community/communities", q, &h, &b);
                Json(sample_community("rust"))
            }),
        )
        .route("/api/v1/community/communities/{slug}",
            get(|State(st): State<MockState>, Path(slug): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("GET", &format!("/api/v1/community/communities/{slug}"), q, &h, &b);
                Json(CommunityDetailResponse { community: sample_community(&slug), pinned_posts: vec![], moderators: vec![], wiki_revisions: vec![] })
            })
            .patch(|State(st): State<MockState>, Path(slug): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("PATCH", &format!("/api/v1/community/communities/{slug}"), q, &h, &b);
                Json(sample_community(&slug))
            }),
        )
        .route("/api/v1/community/communities/{slug}/join",
            post(|State(st): State<MockState>, Path(slug): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("POST", &format!("/api/v1/community/communities/{slug}/join"), q, &h, &b);
                Json(JoinCommunityResponse { community_slug: slug, is_joined: true, member_count: 11, viewer_role: None })
            })
            .delete(|State(st): State<MockState>, Path(slug): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("DELETE", &format!("/api/v1/community/communities/{slug}/join"), q, &h, &b);
                Json(JoinCommunityResponse { community_slug: slug, is_joined: false, member_count: 10, viewer_role: None })
            }),
        )
        .route("/api/v1/community/communities/{slug}/wiki", put(|State(st): State<MockState>, Path(slug): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("PUT", &format!("/api/v1/community/communities/{slug}/wiki"), q, &h, &b);
            Json(CommunityWikiRevisionDto { revision: 2, markdown: "# Updated".into(), editor_actor_id: "b:42".into(), editor_handle: "rust_bot".into(), summary: "edit".into(), created_at_ms: 2 })
        }))
        .route("/api/v1/community/communities/{slug}/mod-queue", get(|State(st): State<MockState>, Path(slug): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("GET", &format!("/api/v1/community/communities/{slug}/mod-queue"), q, &h, &b);
            Json(ModQueueResponse { items: vec![] })
        }))
        .route("/api/v1/community/communities/{slug}/mod-actions", post(|State(st): State<MockState>, Path(slug): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("POST", &format!("/api/v1/community/communities/{slug}/mod-actions"), q, &h, &b);
            Json(ModLogEntryDto {
                log_id: "log_1".into(), community_id: format!("comm_{slug}"), moderator_actor_id: "b:42".into(),
                moderator_handle: "rust_bot".into(), moderator_kind: ActorKind::Bot, action: ModActionKind::Approve,
                target_kind: "post".into(), target_id: "p_remote_1".into(), reason: Some("ok".into()), rule_triggered: None, created_at_ms: 1,
            })
        }))
        .route("/api/v1/community/communities/{slug}/mod-logs", get(|State(st): State<MockState>, Path(slug): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("GET", &format!("/api/v1/community/communities/{slug}/mod-logs"), q, &h, &b);
            Json(ModLogsResponse { logs: vec![] })
        }))
        .route("/api/v1/community/lists",
            get(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("GET", "/api/v1/community/lists", q, &h, &b);
                Json(CommunityListsResponse { lists: vec![] })
            })
            .post(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("POST", "/api/v1/community/lists", q, &h, &b);
                Json(CommunityListDto {
                    list_id: "list_1".into(), owner_actor_id: "b:42".into(), name: "Rust Bots".into(), description: "".into(),
                    kind: CommunityListKind::Mixed, is_private: true, is_pinned: false, member_actor_ids: vec![], community_slugs: vec![], created_at_ms: 1, updated_at_ms: 1,
                })
            }),
        )
        .route("/api/v1/community/lists/{id}",
            patch(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("PATCH", &format!("/api/v1/community/lists/{id}"), q, &h, &b);
                Json(CommunityListDto {
                    list_id: id, owner_actor_id: "b:42".into(), name: "Updated".into(), description: "".into(),
                    kind: CommunityListKind::Mixed, is_private: true, is_pinned: true, member_actor_ids: vec![], community_slugs: vec![], created_at_ms: 1, updated_at_ms: 2,
                })
            })
            .delete(|State(st): State<MockState>, Path(id): Path<String>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
                st.record("DELETE", &format!("/api/v1/community/lists/{id}"), q, &h, &b);
                Json(DeleteEntityResponse { id, deleted: true })
            }),
        )
        .route("/api/v1/community/trending", get(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("GET", "/api/v1/community/trending", q, &h, &b);
            Json(TrendingResponse { topics: vec![], generated_at_ms: 1 })
        }))
        .route("/api/v1/community/search", get(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            let query_str = q.get("query").cloned().unwrap_or_default();
            st.record("GET", "/api/v1/community/search", q, &h, &b);
            Json(CommunitySearchResultDto { query: query_str, tab: CommunitySearchTab::Top, ..Default::default() })
        }))
        .route("/api/v1/community/notifications", get(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("GET", "/api/v1/community/notifications", q, &h, &b);
            Json(NotificationsListResponse { notifications: vec![], unread_count: 0 })
        }))
        .route("/api/v1/community/notifications/read", post(|State(st): State<MockState>, h: HeaderMap, Query(q): Query<HashMap<String, String>>, b: Bytes| async move {
            st.record("POST", "/api/v1/community/notifications/read", q, &h, &b);
            Json(MarkNotificationsReadResponse { marked_count: 2, unread_count: 0 })
        }))
}

#[tokio::test]
async fn test_live_axum_server_round_trip_all_50_endpoints_headers_and_encoding() {
    let state = MockState::default();
    let server = spawn_router(build_full_mock_router(state.clone())).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let client = open_client(&server.base_url, dir.path());
    client.set_auth_session(make_auth_session(1001, "alice", "tok_alice"));
    client.community().set_active_actor_id(Some("b:42".to_string()));
    let c = client.community();

    // Group A (1-10)
    let ids = c.get_identities().await.expect("get_identities");
    assert_eq!(ids.active_actor_id, "b:42");
    assert_eq!(c.runtime_mode(), CommunityRuntimeMode::Online);

    let _ = c.register_human_actor(&RegisterHumanActorRequest {
        handle: "alice".into(), display_name: "Alice".into(), bio: None, location: None,
        website: None, avatar_media_id: None, banner_media_id: None, avatar_shape: None, avatar_color: None,
    }).await.expect("register_human");

    let bot_req = RegisterBotActorRequest {
        bot_user_id: Some(42), actor_kind: Some(ActorKind::Bot), handle: "rust_bot".into(),
        display_name: "Rust Bot".into(), bio: None, avatar_media_id: None, banner_media_id: None,
        avatar_shape: None, avatar_color: None, bot_badge: None, bot_model: None,
        bot_capabilities: vec!["rust".into()], bot_autonomy_mode: BotAutonomyMode::Reactive,
        bot_persona: None, cooldown_secs: None, max_posts_per_hour: Some(20),
    };
    let wrapped = c.register_bot_actor(&bot_req).await.expect("register_bot wrapped");
    assert_eq!(wrapped.bot_token.as_deref(), Some("tok_bot_42"));
    *state.bare_bot_register.lock() = true;
    let bare = c.register_bot_actor(&bot_req).await.expect("register_bot bare");
    assert_eq!(bare.actor.actor_id, "b:42");
    assert_eq!(bare.bot_token, None);

    let _ = c.update_actor("b:42", &UpdateCommunityActorRequest::default()).await.expect("update_actor");
    let prof = c.get_actor_profile("@rust_bot").await.expect("get_actor_profile");
    assert_eq!(prof.actor.actor_id, "rust_bot");
    assert!(c.follow_actor("b:42").await.expect("follow").is_following);
    assert!(!c.unfollow_actor("b:42").await.expect("unfollow").is_following);
    let _ = c.list_mutes_blocks().await.expect("list_mutes");
    let mb_req = MuteBlockRequest {
        target_kind: MuteBlockTargetKind::Keyword, target_value: "spam".into(),
        action: MuteBlockActionKind::Mute, reason: None,
    };
    let _ = c.set_mute_block(&mb_req).await.expect("set_mute");
    assert!(c.remove_mute_block(&mb_req).await.expect("remove_mute").deleted);

    // Group B (11-14)
    let _ = c.get_feed(&FeedRequestDto {
        tab: FeedTab::Community("rust".into()), sort: FeedSort::Top, time_window: TimeWindow::Week,
        actor_filter: ActorFilter::HumanBotDebates, flair: Some("深度".into()),
        cursor: Some("offset:10".into()), limit: 20, ..Default::default()
    }).await.expect("get_feed");
    let _ = c.get_algo_prefs().await.expect("get_algo");
    let _ = c.update_algo_prefs(&AlgoPreferencesDto::default()).await.expect("put_algo");
    let _ = c.record_signal(&RecordSignalRequest {
        post_id: "p_remote_1".into(), kind: Default::default(), dwell_ms: Some(1500),
        reason: None, target_actor_id: None, community_slug: None,
    }).await.expect("signal");

    // Group C (15-25)
    let _ = c.create_post(&CreatePostRequest { body: "Hi".into(), ..Default::default() }).await.expect("create_post");
    let _ = c.get_post("p_remote_1", Some(CommentSort::Qa)).await.expect("get_post");
    let _ = c.update_post("p_remote_1", &UpdatePostRequest::default()).await.expect("update_post");
    let _ = c.delete_post("p_remote_1").await.expect("delete_post");
    let _ = c.vote_post("p_remote_1", &VoteRequest { direction: 1 }).await.expect("vote_post");
    let _ = c.toggle_post_like("p_remote_1").await.expect("like_post");
    let _ = c.repost("p_remote_1", &RepostRequest::default()).await.expect("repost");
    let _ = c.bookmark_post("p_remote_1", &BookmarkRequest::default()).await.expect("bookmark");
    let _ = c.vote_poll("p_remote_1", &CommunityPollVoteRequest { option_ids: vec!["opt_1".into()] }).await.expect("poll_vote");
    let _ = c.give_post_award("p_remote_1", &GiveAwardRequest { award_kind: "gold".into(), message: None }).await.expect("post_award");
    let _ = c.summon_bot("p_remote_1", &SummonBotRequest::default()).await.expect("summon_bot");

    // Group D (26-32)
    let _ = c.get_comments("p_remote_1", &CommentsQueryParams {
        sort: Some(CommentSort::Top), parent_id: Some("c_0".into()), max_depth: Some(3),
        limit: Some(10), cursor: Some("offset:5".into()),
    }).await.expect("get_comments");
    let _ = c.create_comment("p_remote_1", &CreateCommentRequest { body: "Reply".into(), ..Default::default() }).await.expect("create_comment");
    let _ = c.update_comment("c_1", &UpdateCommentRequest { body: "Edited".into() }).await.expect("update_comment");
    let _ = c.delete_comment("c_1").await.expect("delete_comment");
    let _ = c.vote_comment("c_1", &VoteRequest { direction: 1 }).await.expect("vote_comment");
    let _ = c.toggle_comment_like("c_1").await.expect("like_comment");
    let _ = c.give_comment_award("c_1", &GiveAwardRequest { award_kind: "star".into(), message: None }).await.expect("comment_award");

    // Group E (33-42)
    let _ = c.list_communities(Some("tech"), Some(20)).await.expect("list_communities");
    let _ = c.create_community(&CreateCommunityRequest {
        slug: "rust".into(), name: "Rust".into(), description: "".into(), category: "tech".into(),
        banner_media_id: None, icon_media_id: None, icon_emoji: None, accent_color: None,
        rules: vec![], post_flairs: vec![], user_flairs: vec![], wiki_markdown: String::new(),
        automod_rules: vec![], allow_bots: true, require_bot_disclosure: true, is_nsfw: false,
    }).await.expect("create_community");
    let _ = c.get_community("c/Rust").await.expect("get_community");
    let _ = c.update_community("r/Rust", &UpdateCommunityRequest::default()).await.expect("update_community");
    let _ = c.join_community("c/rust").await.expect("join_community");
    let _ = c.leave_community("rust").await.expect("leave_community");
    let _ = c.update_wiki("rust", &UpdateCommunityWikiRequest { markdown: "# W".into(), summary: "s".into() }).await.expect("update_wiki");
    let _ = c.get_mod_queue("rust", Some(ModQueueStatus::Pending), Some(10)).await.expect("mod_queue");
    let _ = c.execute_mod_action("rust", &ModActionRequest {
        action: ModActionKind::Approve, target_kind: "post".into(), target_id: "p_remote_1".into(),
        reason: None, flair: None, queue_item_id: None,
    }).await.expect("mod_action");
    let _ = c.get_mod_logs("rust", Some(10)).await.expect("mod_logs");

    // Group F (43-46) & Group G (47-50)
    let _ = c.list_lists().await.expect("list_lists");
    let _ = c.create_list(&CreateCommunityListRequest {
        name: "Rust Bots".into(), description: "".into(), kind: CommunityListKind::Mixed,
        is_private: true, is_pinned: false, member_actor_ids: vec![], community_slugs: vec![],
    }).await.expect("create_list");
    let _ = c.update_list("list_1", &UpdateCommunityListRequest::default()).await.expect("update_list");
    let _ = c.delete_list("list_1").await.expect("delete_list");
    let _ = c.get_trending(Some(10)).await.expect("trending");
    let search = c.search(&CommunitySearchRequestDto {
        query: "#开源 rust".into(), tab: CommunitySearchTab::Bots, sort: CommunitySearchSort::Latest,
        actor_kind: Some(ActorKind::Bot), has_poll: Some(true), limit: 10, offset: 2, ..Default::default()
    }).await.expect("search");
    assert_eq!(search.query, "#开源 rust");
    let _ = c.get_notifications(Some(CommunityNotificationKind::Mention), Some(10), Some(true)).await.expect("notifications");
    let _ = c.mark_notifications_read(&MarkNotificationsReadRequest::default()).await.expect("mark_read");

    // Direct Authorization: Bot <token> + CRLF header injection rejection
    let _: TrendingResponse = client
        .http_client
        .community_request_json::<(), _>(
            reqwest::Method::GET,
            "/api/v1/community/trending",
            Some(CommunityAuth::Bot("bot_direct_tok")),
            Some("b:42"),
            None,
        )
        .await
        .expect("direct Bot auth");
    let crlf_err = client
        .http_client
        .community_get_identities(Some("tok_alice"), Some("b:42\r\nX-Evil: 1"))
        .await
        .expect_err("CRLF must be rejected");
    assert!(!crlf_err.should_fallback_to_local());
    assert!(matches!(TelegramError::from(crlf_err), TelegramError::Validation(_)));

    // Verify captured wire contracts
    let reqs = state.captured.lock().clone();
    assert!(reqs.len() >= 51, "expected all 50 endpoints + direct bot call, got {}", reqs.len());
    assert!(reqs.iter().all(|r| r.device_id.as_deref().is_some_and(|d| !d.is_empty())));
    assert!(reqs.iter().take(50).all(|r| r.actor_id.as_deref() == Some("b:42")));
    assert_eq!(reqs.last().unwrap().auth.as_deref(), Some("Bot bot_direct_tok"));

    let feed_req = reqs.iter().find(|r| r.path == "/api/v1/community/feed").expect("feed req");
    assert_eq!(feed_req.query.get("community").map(String::as_str), Some("rust"));
    assert_eq!(feed_req.query.get("time").map(String::as_str), Some("week"));
    assert!(!feed_req.query.contains_key("time_window"));
    assert_eq!(feed_req.query.get("actor_filter").map(String::as_str), Some("human_bot_debates"));

    let del_mb = reqs.iter().find(|r| r.method == "DELETE" && r.path == "/api/v1/community/mutes-blocks").expect("del mb");
    assert_eq!(del_mb.query.get("target_kind").map(String::as_str), Some("keyword"));
    assert_eq!(del_mb.query.get("target_value").map(String::as_str), Some("spam"));
    assert_eq!(del_mb.query.get("action").map(String::as_str), Some("mute"));
    assert!(del_mb.body.contains("\"target_value\":\"spam\""));

    let follow_req = reqs.iter().find(|r| r.method == "POST" && r.path == "/api/v1/community/actors/b:42/follow");
    assert!(follow_req.is_some(), "RFC 3986 encoded b%3A42 must decode to b:42");
    assert!(reqs.iter().any(|r| r.method == "GET" && r.path == "/api/v1/community/communities/rust"));
    assert_eq!(HEADER_COMMUNITY_ACTOR_ID, "X-Community-Actor-Id");

    drop_client(client).await;
    server.stop().await;
}

#[tokio::test]
async fn test_live_server_errors_and_domain_404_never_fall_back() {
    let app = Router::new()
        .route("/api/v1/community/identities", get(|| async {
            (StatusCode::UNAUTHORIZED, Json(json!({"error":"unauthorized","message":"token expired"})))
        }))
        .route("/api/v1/community/feed", get(|| async {
            (StatusCode::FORBIDDEN, Json(json!({"error":"forbidden","message":"act-as actor not owned"})))
        }))
        .route("/api/v1/community/posts", post(|| async {
            (StatusCode::BAD_REQUEST, Json(json!({"error":"validation_error","message":"body empty"})))
        }))
        .route("/api/v1/community/identities/human", post(|| async {
            (StatusCode::CONFLICT, Json(json!({"error":"conflict","message":"handle already taken"})))
        }))
        .route("/api/v1/community/algo-prefs", put(|| async {
            (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error":"unprocessable","message":"invalid weights"})))
        }))
        .route("/api/v1/community/trending", get(|| async {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"internal","message":"db overloaded"})))
        }))
        .route("/api/v1/community/posts/{post_id}", get(|Path(id): Path<String>| async move {
            (StatusCode::NOT_FOUND, Json(json!({"error":"not_found","message":format!("post '{id}' not found")})))
        }));

    let server = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let client = open_client(&server.base_url, dir.path());
    client.set_auth_session(make_auth_session(1001, "alice", "tok_alice"));
    let c = client.community();

    // 401 -> Auth
    let e401 = c.get_identities().await.expect_err("401 must propagate");
    assert!(matches!(e401, TelegramError::Auth(_)), "got {e401:?}");
    assert_eq!(c.runtime_mode(), CommunityRuntimeMode::Online);

    // 403 -> PermissionDenied
    let e403 = c.get_feed(&FeedRequestDto::default()).await.expect_err("403 must propagate");
    assert!(matches!(e403, TelegramError::PermissionDenied(_)), "got {e403:?}");
    assert_eq!(c.runtime_mode(), CommunityRuntimeMode::Online);

    // 400 -> Validation
    let e400 = c.create_post(&CreatePostRequest { body: "x".into(), ..Default::default() }).await.expect_err("400 must propagate");
    assert!(matches!(e400, TelegramError::Validation(_)), "got {e400:?}");

    // 409 -> Validation
    let e409 = c.register_human_actor(&RegisterHumanActorRequest {
        handle: "dup".into(), display_name: "Dup".into(), bio: None, location: None,
        website: None, avatar_media_id: None, banner_media_id: None, avatar_shape: None, avatar_color: None,
    }).await.expect_err("409 must propagate");
    assert!(matches!(e409, TelegramError::Validation(_)), "got {e409:?}");

    // 422 -> Validation
    let e422 = c.update_algo_prefs(&AlgoPreferencesDto::default()).await.expect_err("422 must propagate");
    assert!(matches!(e422, TelegramError::Validation(_)), "got {e422:?}");

    // 500 -> Internal (NOT Network!)
    let e500 = c.get_trending(None).await.expect_err("500 must propagate");
    assert!(matches!(e500, TelegramError::Internal(_)), "got {e500:?}");

    // Live structured domain 404 -> NotFound (MUST NOT fall back!)
    let e404 = c.get_post("missing_post_99", None).await.expect_err("domain 404 must propagate");
    assert!(matches!(e404, TelegramError::NotFound(_)), "got {e404:?}");
    assert_eq!(c.runtime_mode(), CommunityRuntimeMode::Online);

    drop_client(client).await;
    server.stop().await;
}

/// Explicit test fixture helper: creates a real test moderator `u:8888` and `"rust"` community
/// in a local test SQLite store (production local DBs start empty with zero hardcoded fake seed data).
fn setup_local_test_rust_community(
    rt: &gpui_telegram_client::community_runtime::LocalCommunityRuntime,
) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let _ = rt.store().sync_telegram_identities(
        &TelegramHumanSyncInput {
            user_id: 8888,
            username: Some("rust_mod".into()),
            display_name: "Rust Mod".into(),
            bio: Some("Test moderator".into()),
            avatar_media_id: None,
        },
        &[],
        now_ms,
    );
    let _ = rt.store().create_community(
        "u:8888",
        &CreateCommunityRequest {
            slug: "rust".into(),
            name: "Rust".into(),
            description: "Systems engineering test community".into(),
            category: "tech".into(),
            banner_media_id: None,
            icon_media_id: None,
            icon_emoji: None,
            accent_color: None,
            rules: vec![],
            post_flairs: vec![],
            user_flairs: vec![],
            wiki_markdown: String::new(),
            automod_rules: vec![],
            allow_bots: true,
            require_bot_disclosure: true,
            is_nsfw: false,
        },
        now_ms,
    );
}

#[tokio::test]
async fn test_legacy_404_variants_transparently_fall_back_to_local() {
    let variants: Vec<(&str, Router)> = vec![
        ("empty_404", Router::new().fallback(|| async { StatusCode::NOT_FOUND })),
        ("plain_text_404", Router::new().fallback(|| async { (StatusCode::NOT_FOUND, "Not Found") })),
        ("generic_json_404", Router::new().fallback(|| async {
            (StatusCode::NOT_FOUND, Json(json!({"error":"not_found","message":"Not Found"})))
        })),
    ];

    for (label, router) in variants {
        let server = spawn_router(router).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let client = open_client(&server.base_url, dir.path());
        let c = client.community();

        let feed = c.get_feed(&FeedRequestDto::default()).await.unwrap_or_else(|e| {
            panic!("legacy 404 variant '{label}' must fall back to local: {e:?}")
        });
        assert_eq!(c.runtime_mode(), CommunityRuntimeMode::LocalFallback);
        assert!(feed.items.is_empty(), "fresh local feed must start empty with zero fake data for '{label}'");

        let comms_before = c.list_communities(None, None).await.expect("list empty communities");
        assert!(comms_before.communities.is_empty(), "fresh local DB must have zero seeded fake communities");

        client.set_auth_session(make_auth_session(1001, "alice_legacy", "tok_legacy"));
        setup_local_test_rust_community(c.local_runtime());
        let comms = c.list_communities(None, None).await.expect("created community");
        assert_eq!(comms.communities.len(), 1);

        let created = c.create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            title: Some(format!("Legacy fallback post {label}")),
            body: "Written locally against legacy 404 server".into(),
            ..Default::default()
        }).await.expect("local create_post on legacy 404");
        assert_eq!(created.post.comments_count, 0);

        drop_client(client).await;
        server.stop().await;
    }
}

#[tokio::test]
async fn test_closed_port_and_mid_session_shutdown_fallback() {
    // Part A: Unreachable closed-port server
    let closed_url = unused_closed_port_url();
    let dir_a = tempfile::tempdir().expect("tempdir");
    let client_a = open_client(&closed_url, dir_a.path());
    assert_eq!(
        client_a.community().local_db_path(),
        Some(dir_a.path().join(COMMUNITY_DB_FILENAME))
    );
    let feed = client_a.community().get_feed(&FeedRequestDto::default()).await.expect("closed-port fallback");
    assert_eq!(client_a.community().runtime_mode(), CommunityRuntimeMode::LocalFallback);
    assert!(feed.items.is_empty(), "fresh local DB must start empty with zero fake posts");
    drop_client(client_a).await;

    // Part B: Live server succeeds (Online), then shuts down mid-session -> seamless LocalFallback
    let live_router = Router::new().route("/api/v1/community/trending", get(|| async {
        Json(TrendingResponse { topics: vec![], generated_at_ms: 42 })
    }));
    let server = spawn_router(live_router).await;
    let dir_b = tempfile::tempdir().expect("tempdir");
    let client_b = open_client(&server.base_url, dir_b.path());
    client_b.set_auth_session(make_auth_session(1001, "alice", "tok_alice"));

    let online_resp = client_b.community().get_trending(Some(5)).await.expect("online trending");
    assert_eq!(online_resp.generated_at_ms, 42);
    assert_eq!(client_b.community().runtime_mode(), CommunityRuntimeMode::Online);

    // Stop server mid-session
    server.stop().await;

    let fallback_trending = client_b.community().get_trending(Some(5)).await.expect("fallback trending");
    assert_eq!(client_b.community().runtime_mode(), CommunityRuntimeMode::LocalFallback);
    assert!(fallback_trending.topics.is_empty());

    setup_local_test_rust_community(client_b.community().local_runtime());
    let offline_post = client_b.community().create_post(&CreatePostRequest {
        community_slug: Some("rust".into()),
        title: Some("Mid-session offline post".into()),
        body: "Saved locally after live server stopped #offline".into(),
        ..Default::default()
    }).await.expect("offline create_post after shutdown");
    let detail = client_b.community().get_post(&offline_post.post.post_id, None).await.expect("local get_post");
    assert_eq!(detail.post.body, "Saved locally after live server stopped #offline");
    let after_trending = client_b.community().get_trending(Some(5)).await.expect("trending after real post");
    assert!(!after_trending.topics.is_empty());

    drop_client(client_b).await;
}

#[tokio::test]
async fn test_offline_writes_local_bots_anti_loop_and_reopen_different_server_url() {
    let dir = tempfile::tempdir().expect("tempdir");
    let closed_url = unused_closed_port_url();
    let alice = make_auth_session(1001, "alice_dev", "tok_alice_1");

    let client = open_client(&closed_url, dir.path());
    client.set_auth_session(alice.clone());
    let c = client.community();
    setup_local_test_rust_community(c.local_runtime());

    // Sync identities locally with an owned Reactive bot + a Manual bot
    let synced = c
        .sync_identities(
            &TelegramHumanSyncInput {
                user_id: 1001,
                username: Some("alice_dev".into()),
                display_name: "Alice Dev".into(),
                bio: Some("Rust & AI".into()),
                avatar_media_id: None,
            },
            &[TelegramBotSyncInput {
                bot_user_id: 5001,
                username: "alice_helper_bot".into(),
                display_name: "Alice Helper Bot".into(),
                bio: Some("Local reactive bot".into()),
                avatar_media_id: None,
                avatar_shape: None,
                avatar_color: None,
                bot_model: Some("deterministic-v1".into()),
                bot_capabilities: vec!["rust".into()],
                bot_persona: Some("Concise systems reviewer".into()),
            }],
        )
        .await
        .expect("sync_identities");
    assert_eq!(c.runtime_mode(), CommunityRuntimeMode::LocalFallback);
    assert!(synced.human_actor.is_some());
    assert!(synced.bot_actors.iter().any(|b| b.actor_id == "b:5001"));

    // Register a Manual-only bot to verify summon_bot rejects Manual mode
    let manual_bot = c
        .register_bot_actor(&RegisterBotActorRequest {
            bot_user_id: Some(5002),
            actor_kind: Some(ActorKind::Bot),
            handle: "silent_manual_bot".into(),
            display_name: "Silent Manual Bot".into(),
            bio: None,
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: None,
            avatar_color: None,
            bot_badge: None,
            bot_model: None,
            bot_capabilities: vec![],
            bot_autonomy_mode: BotAutonomyMode::Manual,
            bot_persona: None,
            cooldown_secs: None,
            max_posts_per_hour: Some(10),
        })
        .await
        .expect("register manual bot");

    // 1. Plain post WITHOUT @mention -> MUST have 0 unsolicited bot comments
    let plain = c
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            title: Some("Quiet architecture note".into()),
            body: "No bot is mentioned here, so comments must start at zero.".into(),
            ..Default::default()
        })
        .await
        .expect("plain post");
    assert_eq!(plain.post.comments_count, 0);

    // 2. Post WITH explicit @alice_helper_bot mention -> automatic local reactive bot reply!
    let mention_post = c
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            title: Some("Need review on async lock".into()),
            body: "Hey @alice_helper_bot can you review this lock-free queue design?".into(),
            ..Default::default()
        })
        .await
        .expect("mention post");
    assert!(
        mention_post.post.comments_count >= 1,
        "explicit @mention must auto-wake local bot"
    );
    let detail = c
        .get_post(&mention_post.post.post_id, Some(CommentSort::Old))
        .await
        .expect("get mention post");
    assert!(
        detail
            .comments
            .iter()
            .any(|cm| cm.author_actor_id == "b:5001" && cm.author_kind.is_bot_like()),
        "expected @alice_helper_bot reply in comments: {:?}",
        detail.comments
    );

    // 3. Explicit summon_bot on plain post -> generates genuine bot comment
    let summoned = c
        .summon_bot(
            &plain.post.post_id,
            &SummonBotRequest {
                bot_actor_id: Some("b:5001".into()),
                bot_handle: None,
                prompt: Some("Analyze trade-offs of zero-copy parsing".into()),
                parent_comment_id: None,
            },
        )
        .await
        .expect("summon_bot");
    assert!(summoned.triggered);
    let summoned_comment = summoned.created_comment.expect("created bot comment");
    assert_eq!(summoned_comment.author_actor_id, "b:5001");

    // 4. summon_bot guards: Manual bot -> PermissionDenied, Human -> Validation, allow_bots=false -> PermissionDenied
    let manual_err = c
        .summon_bot(
            &plain.post.post_id,
            &SummonBotRequest {
                bot_actor_id: Some(manual_bot.actor.actor_id.clone()),
                ..Default::default()
            },
        )
        .await
        .expect_err("Manual bot summon must fail");
    assert!(matches!(manual_err, TelegramError::PermissionDenied(_)));

    let human_err = c
        .summon_bot(
            &plain.post.post_id,
            &SummonBotRequest {
                bot_actor_id: Some("u:1001".into()),
                ..Default::default()
            },
        )
        .await
        .expect_err("Summoning non-bot must fail");
    assert!(matches!(human_err, TelegramError::Validation(_)));

    let no_bots_comm = c
        .create_community(&CreateCommunityRequest {
            slug: "humans_only_zone".into(),
            name: "Humans Only".into(),
            description: "No bots allowed".into(),
            category: "general".into(),
            banner_media_id: None,
            icon_media_id: None,
            icon_emoji: None,
            accent_color: None,
            rules: vec![],
            post_flairs: vec![],
            user_flairs: vec![],
            wiki_markdown: String::new(),
            automod_rules: vec![],
            allow_bots: false,
            require_bot_disclosure: true,
            is_nsfw: false,
        })
        .await
        .expect("create no-bot community");
    let no_bot_post = c
        .create_post(&CreatePostRequest {
            community_slug: Some(no_bots_comm.slug.clone()),
            body: "Human discussion".into(),
            ..Default::default()
        })
        .await
        .expect("post in no-bot community");
    let disallowed_err = c
        .summon_bot(
            &no_bot_post.post.post_id,
            &SummonBotRequest {
                bot_actor_id: Some("b:5001".into()),
                ..Default::default()
            },
        )
        .await
        .expect_err("allow_bots=false must reject summon_bot");
    assert!(matches!(disallowed_err, TelegramError::PermissionDenied(_)));

    // 5. 5-hop Bot<->Bot anti-loop cap
    let bot_b = c
        .register_bot_actor(&RegisterBotActorRequest {
            bot_user_id: Some(5003),
            actor_kind: Some(ActorKind::Bot),
            handle: "debate_partner_bot".into(),
            display_name: "Debate Partner Bot".into(),
            bio: None,
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: None,
            avatar_color: None,
            bot_badge: None,
            bot_model: None,
            bot_capabilities: vec!["rust".into()],
            bot_autonomy_mode: BotAutonomyMode::Reactive,
            bot_persona: None,
            cooldown_secs: None,
            max_posts_per_hour: None,
        })
        .await
        .expect("register second bot");

    let loop_post = c
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            body: "Bot loop boundary thread".into(),
            ..Default::default()
        })
        .await
        .expect("loop post");
    let mut last_comment_id: Option<String> = None;
    for hop in 1..=5 {
        let speaker = if hop % 2 == 1 {
            "b:5001".to_string()
        } else {
            bot_b.actor.actor_id.clone()
        };
        c.set_active_actor_id(Some(speaker));
        let created = c
            .create_comment(
                &loop_post.post.post_id,
                &CreateCommentRequest {
                    parent_comment_id: last_comment_id.clone(),
                    body: format!("Bot exchange hop #{hop}"),
                    ..Default::default()
                },
            )
            .await
            .expect("bot hop comment");
        last_comment_id = Some(created.comment.comment_id);
    }
    // Hop 6 as Bot -> blocked by 5-hop anti-loop cap!
    c.set_active_actor_id(Some("b:5001".to_string()));
    let hop6 = c
        .summon_bot(
            &loop_post.post.post_id,
            &SummonBotRequest {
                bot_actor_id: Some(bot_b.actor.actor_id.clone()),
                parent_comment_id: last_comment_id.clone(),
                prompt: Some("Continue debating!".into()),
                ..Default::default()
            },
        )
        .await
        .expect("hop6 returns clean response");
    assert!(!hop6.triggered, "hop 6 must be capped by 5-hop anti-loop guard");
    assert!(hop6.created_comment.is_none());

    // Switch back to Alice, save bookmark & custom AlgoPreferencesDto
    c.set_active_actor_id(Some("u:1001".to_string()));
    let bm = c
        .bookmark_post(
            &plain.post.post_id,
            &BookmarkRequest {
                folder: Some("Research".into()),
                remove: None,
            },
        )
        .await
        .expect("bookmark");
    assert!(bm.bookmarked);
    let mut custom_prefs = AlgoPreferencesDto::default();
    custom_prefs.preset = AlgoPreset::RedditDeepDive;
    custom_prefs.quality_weight = 0.72;
    let _ = c.update_algo_prefs(&custom_prefs).await.expect("put algo prefs");

    drop_client(client).await;

    // 6. Reopen on SAME TempDir against a DIFFERENT server_url (Legacy404Server on new port!)
    let legacy_server = spawn_router(Router::new().fallback(|| async { StatusCode::NOT_FOUND })).await;
    let reopened = open_client(&legacy_server.base_url, dir.path());
    reopened.set_auth_session(alice);
    let rc = reopened.community();

    let persisted_post = rc
        .get_post(&mention_post.post.post_id, Some(CommentSort::Old))
        .await
        .expect("Alice's offline post must survive reopen across different server_url");
    assert_eq!(rc.runtime_mode(), CommunityRuntimeMode::LocalFallback);
    assert!(persisted_post.comments.iter().any(|cm| cm.author_actor_id == "b:5001"));

    let persisted_prefs = rc.get_algo_prefs().await.expect("persisted algo prefs");
    assert_eq!(persisted_prefs.preset, AlgoPreset::RedditDeepDive);

    let saved_feed = rc
        .get_feed(&FeedRequestDto {
            tab: FeedTab::Saved,
            ..Default::default()
        })
        .await
        .expect("saved feed");
    assert!(
        saved_feed.items.iter().any(|it| it.post.post_id == plain.post.post_id),
        "Alice's bookmark must persist across reopen against different server_url"
    );

    drop_client(reopened).await;
    legacy_server.stop().await;
}

#[tokio::test]
async fn test_account_switch_alice_to_bob_isolates_db_and_enforces_local_security() {
    let dir = tempfile::tempdir().expect("tempdir");
    let closed_url = unused_closed_port_url();
    let alice = make_auth_session(1001, "alice_iso", "tok_alice");
    let bob = make_auth_session(2002, "bob_iso", "tok_bob");
    let charlie = make_auth_session(3003, "charlie_iso", "tok_charlie");

    let client = open_client(&closed_url, dir.path());
    client.set_auth_session(alice);
    let c = client.community();
    setup_local_test_rust_community(c.local_runtime());

    // Alice creates private draft, bookmark, custom algo prefs, private list, and owned bot
    let draft = c
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            title: Some("Alice Private Secret Draft".into()),
            body: "Top secret roadmap notes for Alice only".into(),
            status: PostStatus::Draft,
            ..Default::default()
        })
        .await
        .expect("alice draft");
    let pub_post = c
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            body: "Alice public post to bookmark".into(),
            ..Default::default()
        })
        .await
        .expect("alice public post");
    let _ = c
        .bookmark_post(&pub_post.post.post_id, &BookmarkRequest::default())
        .await
        .expect("alice bookmark");
    let mut alice_prefs = AlgoPreferencesDto::default();
    alice_prefs.preset = AlgoPreset::BotHeavy;
    let _ = c.update_algo_prefs(&alice_prefs).await.expect("alice prefs");
    let alice_list = c
        .create_list(&CreateCommunityListRequest {
            name: "Alice Private Alpha".into(),
            description: "Secret list".into(),
            kind: CommunityListKind::Mixed,
            is_private: true,
            is_pinned: true,
            member_actor_ids: vec![],
            community_slugs: vec!["rust".into()],
        })
        .await
        .expect("alice private list");
    let alice_bot = c
        .register_bot_actor(&RegisterBotActorRequest {
            bot_user_id: None,
            actor_kind: Some(ActorKind::Bot),
            handle: "alice_owned_bot".into(),
            display_name: "Alice Owned Bot".into(),
            bio: None,
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: None,
            avatar_color: None,
            bot_badge: None,
            bot_model: None,
            bot_capabilities: vec![],
            bot_autonomy_mode: BotAutonomyMode::MentionOnly,
            bot_persona: None,
            cooldown_secs: None,
            max_posts_per_hour: Some(15),
        })
        .await
        .expect("alice bot");
    c.set_active_actor_id(Some(alice_bot.actor.actor_id.clone()));
    assert_eq!(c.active_actor_id().as_deref(), Some(alice_bot.actor.actor_id.as_str()));

    // Verify local security guards on Alice before switching:
    // 1) Duplicate @handle rejected with Validation
    let dup_err = c
        .register_bot_actor(&RegisterBotActorRequest {
            bot_user_id: None,
            actor_kind: Some(ActorKind::Bot),
            handle: "alice_owned_bot".into(),
            display_name: "Dup Bot".into(),
            bio: None,
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: None,
            avatar_color: None,
            bot_badge: None,
            bot_model: None,
            bot_capabilities: vec![],
            bot_autonomy_mode: BotAutonomyMode::Manual,
            bot_persona: None,
            cooldown_secs: None,
            max_posts_per_hour: None,
        })
        .await
        .expect_err("duplicate handle must fail");
    assert!(matches!(dup_err, TelegramError::Validation(_)));

    // 2) Self-mute rejected with Validation
    let self_mute = c
        .set_mute_block(&MuteBlockRequest {
            target_kind: MuteBlockTargetKind::Actor,
            target_value: alice_bot.actor.actor_id.clone(),
            action: MuteBlockActionKind::Mute,
            reason: None,
        })
        .await
        .expect_err("self mute must fail");
    assert!(matches!(self_mute, TelegramError::Validation(_)));

    // 3) Quoting nonexistent post rejected with NotFound
    let bad_quote = c
        .create_post(&CreatePostRequest {
            format: PostFormat::Quote,
            body: "Quoting ghost".into(),
            quoted_post_id: Some("post_does_not_exist".into()),
            ..Default::default()
        })
        .await
        .expect_err("quote nonexistent must fail");
    assert!(matches!(bad_quote, TelegramError::NotFound(_)));

    // 4) Non-mod viewing mod-queue rejected with PermissionDenied
    let mod_q_err = c
        .get_mod_queue("rust", None, None)
        .await
        .expect_err("non-mod mod-queue must fail");
    assert!(matches!(mod_q_err, TelegramError::PermissionDenied(_)));

    // LIVE ACCOUNT SWITCH: Alice -> Bob on same TelegramClient & TempDir
    client.set_auth_session(bob);
    assert_eq!(c.active_actor_id().as_deref(), Some("u:2002"));

    // Zero leakage from Alice!
    let bob_draft_lookup = c.get_post(&draft.post.post_id, None).await;
    assert!(matches!(bob_draft_lookup, Err(TelegramError::NotFound(_))));
    let bob_saved = c
        .get_feed(&FeedRequestDto {
            tab: FeedTab::Saved,
            ..Default::default()
        })
        .await
        .expect("bob saved feed");
    assert!(bob_saved.items.is_empty(), "Alice bookmarks must NOT leak to Bob");
    let bob_prefs = c.get_algo_prefs().await.expect("bob algo prefs");
    assert_eq!(bob_prefs.preset, AlgoPreset::Balanced);
    let bob_lists = c.list_lists().await.expect("bob lists");
    assert!(
        !bob_lists.lists.iter().any(|l| l.list_id == alice_list.list_id),
        "Alice private list must NOT leak to Bob"
    );

    // Bob writes his own private draft, logs out, client closes, Charlie opens same TempDir
    setup_local_test_rust_community(c.local_runtime());
    let bob_draft = c
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            body: "Bob private draft".into(),
            status: PostStatus::Draft,
            ..Default::default()
        })
        .await
        .expect("bob draft");
    client.clear_session();
    assert_eq!(c.active_actor_id(), None);
    drop_client(client).await;

    let reopened = open_client(&closed_url, dir.path());
    // Before login on owned DB, active_actor_id MUST NOT leak Bob's actor!
    assert_eq!(reopened.community().active_actor_id(), None);
    reopened.set_auth_session(charlie);
    assert_eq!(reopened.community().active_actor_id().as_deref(), Some("u:3003"));
    assert!(matches!(
        reopened.community().get_post(&bob_draft.post.post_id, None).await,
        Err(TelegramError::NotFound(_))
    ));
    drop_client(reopened).await;
}

async fn ws_community_handler(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(|mut socket: WebSocket| async move {
        // 1. Raw corrupted JSON frame (must be skipped safely by ws_transport)
        let _ = socket.send(Message::Text("{corrupted frame".into())).await;

        // 2. Valid UpdateEvent::CommunityEvent
        let valid_1 = WsServerFrame::Update(UpdateEvent::CommunityEvent {
            event_kind: "post_created".to_string(),
            community_id: Some("comm_rust".to_string()),
            post_id: Some("post_ws_1".to_string()),
            comment_id: None,
            actor_id: Some("b:42".to_string()),
            payload_json: r#"{"score":9,"kind":"bot"}"#.to_string(),
            timestamp_ms: 1_730_000_010_000,
        });
        let _ = socket
            .send(Message::Text(serde_json::to_string(&valid_1).unwrap().into()))
            .await;

        // 3. Malformed empty event_kind (must be dropped cleanly by client_dispatch)
        let bad_kind = WsServerFrame::Update(UpdateEvent::CommunityEvent {
            event_kind: "   ".to_string(),
            community_id: None,
            post_id: Some("bad_1".to_string()),
            comment_id: None,
            actor_id: None,
            payload_json: r#"{"ok":true}"#.to_string(),
            timestamp_ms: 1_730_000_010_001,
        });
        let _ = socket
            .send(Message::Text(serde_json::to_string(&bad_kind).unwrap().into()))
            .await;

        // 4. Malformed payload_json (must be dropped cleanly without poisoning subsequent frames)
        let bad_payload = WsServerFrame::Update(UpdateEvent::CommunityEvent {
            event_kind: "comment_created".to_string(),
            community_id: None,
            post_id: Some("bad_2".to_string()),
            comment_id: Some("c_bad".to_string()),
            actor_id: None,
            payload_json: "{not_json".to_string(),
            timestamp_ms: 1_730_000_010_002,
        });
        let _ = socket
            .send(Message::Text(serde_json::to_string(&bad_payload).unwrap().into()))
            .await;

        // 5. Forward-compatible unknown event_kind with empty payload
        let valid_2 = WsServerFrame::Update(UpdateEvent::CommunityEvent {
            event_kind: "future_v99_event".to_string(),
            community_id: None,
            post_id: Some("post_ws_2".to_string()),
            comment_id: None,
            actor_id: Some("u:1001".to_string()),
            payload_json: String::new(),
            timestamp_ms: 1_730_000_010_003,
        });
        let _ = socket
            .send(Message::Text(serde_json::to_string(&valid_2).unwrap().into()))
            .await;

        // Keep socket briefly alive so client drains frames cleanly
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    })
}

#[tokio::test]
async fn test_ws_live_axum_community_events_and_malformed_resilience() {
    let app = Router::new().route("/api/v1/ws", any(ws_community_handler));
    let server = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let client = open_client(&server.base_url, dir.path());
    client.set_auth_session(make_auth_session(1001, "alice_ws", "tok_ws"));
    client.connect_ws();

    let mut received: Vec<ClientDomainEvent> = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while received.len() < 2 && tokio::time::Instant::now() < deadline {
        received.extend(client.poll_events());
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    assert_eq!(received.len(), 2, "expected exactly 2 valid CommunityEvent items, got {received:?}");
    match &received[0] {
        ClientDomainEvent::CommunityEvent {
            event_kind,
            community_id,
            post_id,
            actor_id,
            ..
        } => {
            assert_eq!(event_kind, "post_created");
            assert_eq!(community_id.as_deref(), Some("comm_rust"));
            assert_eq!(post_id.as_deref(), Some("post_ws_1"));
            assert_eq!(actor_id.as_deref(), Some("b:42"));
        }
        other => panic!("expected CommunityEvent, got {other:?}"),
    }
    assert_eq!(received[0].community_payload_json().unwrap()["score"], 9);
    match &received[1] {
        ClientDomainEvent::CommunityEvent { event_kind, post_id, .. } => {
            assert_eq!(event_kind, "future_v99_event");
            assert_eq!(post_id.as_deref(), Some("post_ws_2"));
        }
        other => panic!("expected second CommunityEvent, got {other:?}"),
    }

    // Direct process_ws_frame test including malformed + valid recovery
    let dropped = client.process_ws_frame(WsServerFrame::Update(UpdateEvent::CommunityEvent {
        event_kind: "bad_payload_evt".into(),
        community_id: None,
        post_id: None,
        comment_id: None,
        actor_id: None,
        payload_json: "{broken".into(),
        timestamp_ms: 99,
    }));
    assert!(dropped.is_empty());

    let direct = client.process_ws_frame(WsServerFrame::Update(UpdateEvent::CommunityEvent {
        event_kind: "vote_updated".into(),
        community_id: None,
        post_id: Some("p_direct".into()),
        comment_id: None,
        actor_id: Some("u:1001".into()),
        payload_json: r#"{"score":123}"#.into(),
        timestamp_ms: 100,
    }));
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].community_payload_json().unwrap()["score"], 123);

    drop_client(client).await;
    server.stop().await;
}

#[tokio::test]
async fn test_concurrent_community_calls_block_on_and_clean_drop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let closed_url = unused_closed_port_url();
    let client = open_client(&closed_url, dir.path());
    client.set_auth_session(make_auth_session(4004, "concurrent_alice", "tok_conc"));
    let facade = client.community().clone();
    setup_local_test_rust_community(facade.local_runtime());
    let conc_bot = facade
        .register_bot_actor(&RegisterBotActorRequest {
            bot_user_id: Some(5404),
            actor_kind: Some(ActorKind::Bot),
            handle: "conc_bot".into(),
            display_name: "Conc Bot".into(),
            bio: None,
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: None,
            avatar_color: None,
            bot_badge: None,
            bot_model: None,
            bot_capabilities: vec![],
            bot_autonomy_mode: BotAutonomyMode::Manual,
            bot_persona: None,
            cooldown_secs: None,
            max_posts_per_hour: None,
        })
        .await
        .expect("register conc_bot");
    let _ = facade
        .create_post_as(
            &conc_bot.actor.actor_id,
            &CreatePostRequest {
                community_slug: Some("rust".into()),
                title: Some("Initial Bot Post".into()),
                body: "Baseline bot post for concurrent feed/search #rust".into(),
                ..Default::default()
            },
        )
        .await
        .expect("baseline bot post");

    // 1. Spawn 18 concurrent async tasks mixing reads, writes, search (is:bot), trending, and votes
    let mut handles = Vec::new();
    for idx in 0..18usize {
        let f = facade.clone();
        handles.push(tokio::spawn(async move {
            match idx % 4 {
                0 => {
                    let created = f
                        .create_post(&CreatePostRequest {
                            community_slug: Some("rust".into()),
                            title: Some(format!("Concurrent Post #{idx}")),
                            body: format!("Parallel local SQLite write #{idx} #concurrency"),
                            ..Default::default()
                        })
                        .await
                        .expect("concurrent create_post");
                    let voted = f
                        .vote_post(&created.post.post_id, &VoteRequest { direction: 1 })
                        .await
                        .expect("concurrent vote_post");
                    assert!(voted.score >= 1);
                }
                1 => {
                    let feed = f.get_feed(&FeedRequestDto::default()).await.expect("concurrent get_feed");
                    assert!(!feed.items.is_empty());
                }
                2 => {
                    let res = f
                        .search(&CommunitySearchRequestDto {
                            query: "is:bot".into(),
                            tab: CommunitySearchTab::Top,
                            ..Default::default()
                        })
                        .await
                        .expect("concurrent is:bot search");
                    assert!(!res.posts.is_empty(), "is:bot alias must match bot posts");
                }
                _ => {
                    let _ = f.get_trending(Some(5)).await.expect("concurrent trending");
                    let _ = f.get_identities().await.expect("concurrent identities");
                }
            }
        }));
    }
    for h in handles {
        h.await.expect("concurrent task panicked");
    }

    // 2. Verify block_on works safely even when invoked from inside a Tokio runtime!
    let f_sync = facade.clone();
    let sync_trending = f_sync
        .block_on(async move { facade.get_trending(Some(5)).await })
        .expect("block_on inside Tokio runtime");
    assert!(!sync_trending.topics.is_empty());

    // 3. Clean drop & immediate reopen verifying zero leaked DB locks and persisted writes
    drop_client(client).await;

    let reopened = open_client(&closed_url, dir.path());
    reopened.set_auth_session(make_auth_session(4004, "concurrent_alice", "tok_conc"));
    let search_persisted = reopened
        .community()
        .search(&CommunitySearchRequestDto {
            query: "#concurrency".into(),
            tab: CommunitySearchTab::Latest,
            limit: 20,
            ..Default::default()
        })
        .await
        .expect("search persisted concurrent posts");
    assert_eq!(search_persisted.posts.len(), 5, "all 5 created posts (idx 0,4,8,12,16) must persist");

    drop_client(reopened).await;
}

#[tokio::test]
async fn test_r2_unbound_post_logout_reopen_act_as_and_write_isolation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let closed_url = unused_closed_port_url();
    let alice = make_auth_session(101, "alice_sec", "tok_alice_sec");

    let client = open_client(&closed_url, dir.path());
    client.set_auth_session(alice.clone());
    let c = client.community();
    setup_local_test_rust_community(c.local_runtime());

    // Alice creates a private draft, private list, and owned bot, then sets active actor to her bot.
    let draft = c
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            title: Some("Alice Classified Draft".into()),
            body: "Top secret draft body for Alice".into(),
            status: PostStatus::Draft,
            ..Default::default()
        })
        .await
        .expect("alice draft");
    let secret_list = c
        .create_list(&CreateCommunityListRequest {
            name: "Alice Secret Watchlist".into(),
            description: "Private".into(),
            kind: CommunityListKind::Mixed,
            is_private: true,
            is_pinned: false,
            member_actor_ids: vec![],
            community_slugs: vec!["rust".into()],
        })
        .await
        .expect("alice list");
    let alice_bot = c
        .register_bot_actor(&RegisterBotActorRequest {
            bot_user_id: Some(5101),
            actor_kind: Some(ActorKind::Bot),
            handle: "alice_sec_bot".into(),
            display_name: "Alice Sec Bot".into(),
            bio: None,
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: None,
            avatar_color: None,
            bot_badge: None,
            bot_model: None,
            bot_capabilities: vec!["rust".into()],
            bot_autonomy_mode: BotAutonomyMode::Reactive,
            bot_persona: None,
            cooldown_secs: None,
            max_posts_per_hour: None,
        })
        .await
        .expect("alice bot");
    c.set_active_actor_id(Some(alice_bot.actor.actor_id.clone()));

    // Alice logs out.
    client.clear_session();
    assert_eq!(c.active_actor_id(), None);

    // 1) Post-logout Act-As impersonation attacks MUST fail with PermissionDenied.
    for spoofed in ["u:101", "b:5101", "u:8888"] {
        c.set_active_actor_id(Some(spoofed.to_string()));
        let feed_err = c
            .get_feed(&FeedRequestDto::default())
            .await
            .expect_err("logged-out spoofed act-as must fail");
        assert!(
            matches!(feed_err, TelegramError::PermissionDenied(_)),
            "spoofed '{spoofed}' expected PermissionDenied, got {feed_err:?}"
        );
    }
    c.set_active_actor_id(Some("u:101".to_string()));
    assert!(matches!(
        c.get_post(&draft.post.post_id, None).await,
        Err(TelegramError::PermissionDenied(_))
    ));
    assert!(matches!(
        c.update_actor(
            "u:101",
            &UpdateCommunityActorRequest {
                display_name: Some("HIJACKED".into()),
                ..Default::default()
            }
        )
        .await,
        Err(TelegramError::PermissionDenied(_))
    ));

    // 2) Logged-out guest reads MUST NOT leak Alice's private data, and guest writes on owned DB MUST fail!
    c.set_active_actor_id(None);
    assert!(matches!(
        c.get_post(&draft.post.post_id, None).await,
        Err(TelegramError::NotFound(_))
    ));
    let guest_lists = c.list_lists().await.expect("guest list_lists");
    assert!(!guest_lists.lists.iter().any(|l| l.list_id == secret_list.list_id));
    assert!(matches!(
        c.create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            body: "Unauthenticated guest post attempt".into(),
            ..Default::default()
        })
        .await,
        Err(TelegramError::PermissionDenied(_))
    ));
    assert!(matches!(
        c.update_actor(
            "u:0",
            &UpdateCommunityActorRequest {
                display_name: Some("Mutated Guest".into()),
                ..Default::default()
            }
        )
        .await,
        Err(TelegramError::PermissionDenied(_))
    ));

    drop_client(client).await;

    // 3) Reopen owned DB BEFORE login: impersonation & writes MUST still be blocked, and Alice's saved meta untouched.
    let reopened = open_client(&closed_url, dir.path());
    let rc = reopened.community();
    assert_eq!(rc.active_actor_id(), None);
    rc.set_active_actor_id(Some("u:101".to_string()));
    assert!(matches!(
        rc.get_post(&draft.post.post_id, None).await,
        Err(TelegramError::PermissionDenied(_))
    ));
    rc.set_active_actor_id(Some("u:0".to_string()));
    assert!(matches!(
        rc.create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            body: "Reopen before login guest write attempt".into(),
            ..Default::default()
        })
        .await,
        Err(TelegramError::PermissionDenied(_))
    ));

    // 4) Alice logs back in: her persisted active actor ("b:5101") and private draft MUST be intact!
    reopened.set_auth_session(alice);
    assert_eq!(rc.active_actor_id().as_deref(), Some("b:5101"));
    let persisted_draft = rc
        .get_post(&draft.post.post_id, None)
        .await
        .expect("Alice draft must survive logout + reopen");
    assert_eq!(persisted_draft.post.body, "Top secret draft body for Alice");
    let alice_prof = rc.get_actor_profile("u:101").await.expect("alice profile");
    assert_ne!(alice_prof.actor.display_name, "HIJACKED");

    drop_client(reopened).await;
}

#[tokio::test]
async fn test_r2_register_bot_idor_dot_segments_and_crlf_headers() {
    // Part A: Local register_bot_actor IDOR against another user's registered bot b:920001
    let dir_idor = tempfile::tempdir().expect("tempdir");
    let closed_url = unused_closed_port_url();
    let client_idor = open_client(&closed_url, dir_idor.path());
    client_idor.set_auth_session(make_auth_session(101, "alice_idor", "tok_alice"));
    let ci = client_idor.community();

    // Seed another user's (owner_user_id=102) bot directly into the local store
    let now_ms = chrono::Utc::now().timestamp_millis();
    ci.local_runtime()
        .store()
        .create_bot_actor(
            102,
            920001,
            &RegisterBotActorRequest {
                bot_user_id: Some(920001),
                actor_kind: Some(ActorKind::Bot),
                handle: "ferris_bot".into(),
                display_name: "Ferris Systems Bot".into(),
                bio: Some("Rust & systems engineering assistant".into()),
                avatar_media_id: None,
                banner_media_id: None,
                avatar_shape: None,
                avatar_color: None,
                bot_badge: None,
                bot_model: None,
                bot_capabilities: vec![],
                bot_autonomy_mode: BotAutonomyMode::Manual,
                bot_persona: None,
                cooldown_secs: None,
                max_posts_per_hour: None,
            },
            now_ms,
        )
        .expect("insert owner 102 bot 920001");

    let before_ferris = ci
        .get_actor_profile("b:920001")
        .await
        .expect("registered ferris_bot");
    let idor_err = ci
        .register_bot_actor(&RegisterBotActorRequest {
            bot_user_id: Some(920001),
            actor_kind: Some(ActorKind::Bot),
            handle: "ferris_bot".into(),
            display_name: "PWNED FERRIS".into(),
            bio: Some("HIJACKED".into()),
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: None,
            avatar_color: None,
            bot_badge: None,
            bot_model: None,
            bot_capabilities: vec![],
            bot_autonomy_mode: BotAutonomyMode::Manual,
            bot_persona: None,
            cooldown_secs: None,
            max_posts_per_hour: None,
        })
        .await
        .expect_err("overwriting unowned bot_user_id must fail");
    assert!(matches!(idor_err, TelegramError::PermissionDenied(_)), "got {idor_err:?}");
    let after_ferris = ci
        .get_actor_profile("b:920001")
        .await
        .expect("registered ferris_bot intact");
    assert_eq!(after_ferris.actor.display_name, before_ferris.actor.display_name);
    assert_eq!(after_ferris.actor.bio, before_ferris.actor.bio);
    drop_client(client_idor).await;

    // Part B: Dot-segment path traversal & CRLF/control-char rejection with live catch-all server
    let hits = Arc::new(Mutex::new(Vec::<String>::new()));
    let hits_cl = Arc::clone(&hits);
    let app = Router::new().fallback(move |uri: axum::http::Uri| {
        let hits = Arc::clone(&hits_cl);
        async move {
            hits.lock().push(uri.to_string());
            (StatusCode::OK, Json(json!({})))
        }
    });
    let server = spawn_router(app).await;
    let dir_http = tempfile::tempdir().expect("tempdir");
    let client = open_client(&server.base_url, dir_http.path());
    client.set_auth_session(make_auth_session(101, "alice_http", "tok_alice"));

    let http = &client.http_client;
    assert!(matches!(
        http.community_get_post(Some("tok"), None, ".", None).await,
        Err(gpui_telegram_client::CommunityApiError::InvalidInput(_))
    ));
    assert!(matches!(
        http.community_delete_post(Some("tok"), None, "..").await,
        Err(gpui_telegram_client::CommunityApiError::InvalidInput(_))
    ));
    assert!(matches!(
        http.community_vote_post(Some("tok"), None, "..", &VoteRequest { direction: 1 }).await,
        Err(gpui_telegram_client::CommunityApiError::InvalidInput(_))
    ));
    assert!(matches!(
        http.community_follow_actor(Some("tok"), None, "..").await,
        Err(gpui_telegram_client::CommunityApiError::InvalidInput(_))
    ));
    assert!(matches!(
        http.community_get_community(Some("tok"), None, "c/..").await,
        Err(gpui_telegram_client::CommunityApiError::InvalidInput(_))
    ));
    assert!(matches!(
        http.community_get_community(Some("tok"), None, "r/.").await,
        Err(gpui_telegram_client::CommunityApiError::InvalidInput(_))
    ));
    assert!(matches!(
        http.community_get_actor_profile(Some("tok"), None, "@..").await,
        Err(gpui_telegram_client::CommunityApiError::InvalidInput(_))
    ));
    assert!(matches!(
        client.community().get_community("c/..").await,
        Err(TelegramError::Validation(_))
    ));

    // Strict CRLF / control-char rejection across bearer_auth, CommunityAuth, and set_active_actor_id
    assert!(matches!(
        http.community_get_feed(Some("tok_alice\r\n"), None, &FeedRequestDto::default()).await,
        Err(gpui_telegram_client::CommunityApiError::InvalidInput(_))
    ));
    assert!(matches!(
        http.community_request_json::<(), TrendingResponse>(
            reqwest::Method::GET,
            "/api/v1/community/trending",
            Some(CommunityAuth::Bearer("\r\n")),
            None,
            None,
        )
        .await,
        Err(gpui_telegram_client::CommunityApiError::InvalidInput(_))
    ));
    assert!(matches!(
        http.community_request_json::<(), TrendingResponse>(
            reqwest::Method::GET,
            "/api/v1/community/trending",
            Some(CommunityAuth::Bot("\r\n")),
            None,
            None,
        )
        .await,
        Err(gpui_telegram_client::CommunityApiError::InvalidInput(_))
    ));
    client.community().set_active_actor_id(Some("b:1\r\n".into()));
    assert!(matches!(
        client.community().get_feed(&FeedRequestDto::default()).await,
        Err(TelegramError::Validation(_))
    ));
    client.community().set_active_actor_id(Some("\r\n".into()));
    assert!(matches!(
        client.community().get_feed(&FeedRequestDto::default()).await,
        Err(TelegramError::Validation(_))
    ));

    assert!(
        hits.lock().is_empty(),
        "dot-segments and CRLF inputs must be rejected client-side before sending HTTP, hits={:?}",
        *hits.lock()
    );

    drop_client(client).await;
    server.stop().await;
}

#[tokio::test]
async fn test_r2_domain_404_verified_write_guard_503_poisoning_and_guest_timeout_reset() {
    // Part A: Live domain 404 adversarial slugs/codes MUST propagate NotFound and NEVER fall back!
    let payloads = Arc::new(vec![
        json!({"error":"not_found","message":"community 'route66' not found"}),
        json!({"error":"actor_not_found","message":"actor 'endpoint_bot' not found"}),
        json!({"error":"post_not_found","message":"post 'route_123' not found"}),
        json!({"error":"actor_not_found","message":"Not Found"}),
        json!({"error":"post_not_found","message":""}),
        json!({"error":"actor_not_found"}),
        json!({"error":"not_found","message":"draft 'd1' not found"}),
    ]);
    let idx = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let payloads_cl = Arc::clone(&payloads);
    let idx_cl = Arc::clone(&idx);
    let app_a = Router::new().route(
        "/api/v1/community/communities/{slug}",
        get(move || {
            let payloads = Arc::clone(&payloads_cl);
            let idx = Arc::clone(&idx_cl);
            async move {
                let i = idx.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (StatusCode::NOT_FOUND, Json(payloads[i % payloads.len()].clone()))
            }
        }),
    );
    let server_a = spawn_router(app_a).await;
    let dir_a = tempfile::tempdir().expect("tempdir");
    let client_a = open_client(&server_a.base_url, dir_a.path());
    for i in 0..payloads.len() {
        let err = client_a
            .community()
            .get_community("rust")
            .await
            .expect_err("live domain 404 must NOT fall back to seeded c/rust");
        assert!(
            matches!(err, TelegramError::NotFound(_)),
            "payload #{i} expected NotFound, got {err:?}"
        );
        assert_eq!(client_a.community().runtime_mode(), CommunityRuntimeMode::Online);
    }
    drop_client(client_a).await;
    server_a.stop().await;

    // Part B: Verified server returning terse 404 on POST /posts MUST NOT fall back to local SQLite!
    let app_b = Router::new()
        .route(
            "/api/v1/community/feed",
            get(|| async {
                Json(FeedResponseDto {
                    items: vec![],
                    next_cursor: None,
                    total_candidates: 0,
                    applied_preset: AlgoPreset::Balanced,
                    generated_at_ms: 1,
                })
            }),
        )
        .route(
            "/api/v1/community/posts",
            post(|| async { (StatusCode::NOT_FOUND, "Not Found") }),
        );
    let server_b = spawn_router(app_b).await;
    let dir_b = tempfile::tempdir().expect("tempdir");
    let client_b = open_client(&server_b.base_url, dir_b.path());
    client_b.set_auth_session(make_auth_session(101, "alice_ver", "tok_alice"));
    let _ = client_b.community().get_feed(&FeedRequestDto::default()).await.expect("verify online");
    assert_eq!(client_b.community().runtime_mode(), CommunityRuntimeMode::Online);

    let before_count: i64 = client_b
        .community()
        .local_runtime()
        .store()
        .with_conn(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM community_posts", [], |r| r.get(0)).unwrap()))
        .expect("count posts before");
    let post_err = client_b
        .community()
        .create_post(&CreatePostRequest {
            community_slug: Some("missing_comm".into()),
            body: "Should fail online and NOT write locally".into(),
            ..Default::default()
        })
        .await
        .expect_err("verified POST /posts terse 404 must NOT fall back");
    assert!(matches!(post_err, TelegramError::NotFound(_)), "got {post_err:?}");
    assert_eq!(client_b.community().runtime_mode(), CommunityRuntimeMode::Online);
    let after_count: i64 = client_b
        .community()
        .local_runtime()
        .store()
        .with_conn(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM community_posts", [], |r| r.get(0)).unwrap()))
        .expect("count posts after");
    assert_eq!(before_count, after_count, "verified POST /posts 404 must not mutate local SQLite");
    drop_client(client_b).await;
    server_b.stop().await;

    // Part C: 503 / 401 must NOT poison remote_community_verified on a legacy 404 server
    for initial_status in [StatusCode::SERVICE_UNAVAILABLE, StatusCode::UNAUTHORIZED] {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_cl = Arc::clone(&count);
        let app_c = Router::new().fallback(move || {
            let count = Arc::clone(&count_cl);
            async move {
                if count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    (initial_status, "transient error")
                } else {
                    (StatusCode::NOT_FOUND, "Not Found")
                }
            }
        });
        let server_c = spawn_router(app_c).await;
        let dir_c = tempfile::tempdir().expect("tempdir");
        let client_c = open_client(&server_c.base_url, dir_c.path());
        let first_err = client_c.community().get_feed(&FeedRequestDto::default()).await.expect_err("first error");
        if initial_status == StatusCode::UNAUTHORIZED {
            assert!(matches!(first_err, TelegramError::Auth(_)));
        } else {
            assert!(matches!(first_err, TelegramError::Internal(_)));
        }
        let feed = client_c
            .community()
            .get_feed(&FeedRequestDto::default())
            .await
            .expect("legacy 404 feed fallback");
        assert_eq!(client_c.community().runtime_mode(), CommunityRuntimeMode::LocalFallback);
        assert!(feed.items.is_empty(), "fresh local DB must start empty with zero fake posts");
        setup_local_test_rust_community(client_c.community().local_runtime());
        let local_created = client_c
            .community()
            .local_runtime()
            .store()
            .create_post(
                "u:8888",
                &CreatePostRequest {
                    community_slug: Some("rust".into()),
                    body: "Local fallback post after 503/401".into(),
                    ..Default::default()
                },
                chrono::Utc::now().timestamp_millis(),
            )
            .expect("create local fallback post");
        let local_pid = local_created.post.post_id.clone();
        let post_detail = client_c
            .community()
            .get_post(&local_pid, None)
            .await
            .expect("get_post must also fall back cleanly after 503/401 + legacy 404");
        assert_eq!(post_detail.post.post_id, local_pid);
        drop_client(client_c).await;
        server_c.stop().await;
    }

    // Part D: Guest timeout backoff MUST be cleared immediately when Alice signs in
    let app_d = Router::new().route(
        "/api/v1/community/feed",
        get(|headers: HeaderMap| async move {
            let authed = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v == "Bearer tok_alice_fast");
            if !authed {
                tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
            }
            Json(FeedResponseDto {
                items: vec![],
                next_cursor: None,
                total_candidates: 777,
                applied_preset: AlgoPreset::Balanced,
                generated_at_ms: 999,
            })
        }),
    );
    let server_d = spawn_router(app_d).await;
    let dir_d = tempfile::tempdir().expect("tempdir");
    let cfg_d = ClientConfig {
        server_url: server_d.base_url.clone(),
        http_timeout_secs: 1,
        ..Default::default()
    };
    let client_d = TelegramClient::open_with_dir(cfg_d, dir_d.path()).expect("open_with_dir");
    let guest_feed = client_d
        .community()
        .get_feed(&FeedRequestDto::default())
        .await
        .expect("guest timeout fallback");
    assert_eq!(client_d.community().runtime_mode(), CommunityRuntimeMode::LocalFallback);
    assert!(guest_feed.items.is_empty(), "fresh local DB must start empty with zero fake posts");

    // Immediately sign in as Alice (within the 5s timeout backoff window!)
    client_d.set_auth_session(make_auth_session(101, "alice_fast", "tok_alice_fast"));
    let alice_feed = client_d
        .community()
        .get_feed(&FeedRequestDto::default())
        .await
        .expect("Alice must connect online immediately without inheriting guest timeout backoff");
    assert_eq!(client_d.community().runtime_mode(), CommunityRuntimeMode::Online);
    assert_eq!(alice_feed.total_candidates, 777);

    drop_client(client_d).await;
    server_d.stop().await;
}

#[tokio::test]
async fn test_r2_local_search_draft_deleted_guards_banned_mention_unread_and_top_level_5hop_restart()
{
    let dir = tempfile::tempdir().expect("tempdir");
    let closed_url = unused_closed_port_url();
    let alice = make_auth_session(101, "alice_local", "tok_alice_local");

    let client = open_client(&closed_url, dir.path());
    client.set_auth_session(alice.clone());
    let c = client.community();
    setup_local_test_rust_community(c.local_runtime());

    // 1. Local search: c:<slug>, community_slug: Some("C/Rust"), multi-word flair: Some("Deep Dive")
    let deep_post = c
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            title: Some("Async Pinning Deep Dive".into()),
            body: "Detailed breakdown of Rust pinning #r2search".into(),
            flair: Some("Deep Dive".into()),
            ..Default::default()
        })
        .await
        .expect("create deep dive post");

    let res_c_op = c
        .search(&CommunitySearchRequestDto {
            query: "c:rust".into(),
            tab: CommunitySearchTab::Latest,
            limit: 50,
            ..Default::default()
        })
        .await
        .expect("search c:rust");
    assert!(!res_c_op.posts.is_empty());
    assert!(
        res_c_op
            .posts
            .iter()
            .all(|p| p.community_slug.as_deref() == Some("rust")),
        "c:rust returned non-rust posts: {:?}",
        res_c_op.posts.iter().map(|p| &p.community_slug).collect::<Vec<_>>()
    );
    assert!(res_c_op.posts.iter().any(|p| p.post_id == deep_post.post.post_id));

    let res_slug = c
        .search(&CommunitySearchRequestDto {
            query: String::new(),
            community_slug: Some("C/Rust".into()),
            tab: CommunitySearchTab::Latest,
            limit: 50,
            ..Default::default()
        })
        .await
        .expect("search community_slug C/Rust");
    assert!(
        res_slug
            .posts
            .iter()
            .all(|p| p.community_slug.as_deref() == Some("rust"))
    );
    assert!(res_slug.posts.iter().any(|p| p.post_id == deep_post.post.post_id));

    let res_flair = c
        .search(&CommunitySearchRequestDto {
            query: String::new(),
            flair: Some("Deep Dive".into()),
            tab: CommunitySearchTab::Latest,
            limit: 50,
            ..Default::default()
        })
        .await
        .expect("search multi-word flair Deep Dive");
    assert!(
        res_flair.posts.iter().any(|p| p.post_id == deep_post.post.post_id),
        "multi-word flair 'Deep Dive' must match new post"
    );
    assert!(res_flair
        .posts
        .iter()
        .all(|p| p.flair.as_deref().is_some_and(|f| f.eq_ignore_ascii_case("Deep Dive"))));

    // 2. Bare numeric comment cursor "1", plain repost on deleted post, and Draft guards
    for i in 0..3 {
        let _ = c
            .create_comment(
                &deep_post.post.post_id,
                &CreateCommentRequest {
                    body: format!("Comment #{i}"),
                    ..Default::default()
                },
            )
            .await
            .expect("comment");
    }
    let page_numeric = c
        .get_comments(
            &deep_post.post.post_id,
            &CommentsQueryParams {
                sort: Some(CommentSort::Old),
                limit: Some(1),
                cursor: Some("1".into()),
                ..Default::default()
            },
        )
        .await
        .expect("bare numeric cursor '1'");
    assert_eq!(page_numeric.comments.len(), 1);
    assert_eq!(page_numeric.comments[0].body, "Comment #1");

    // Soft-delete post -> plain repost & get_comments MUST return NotFound
    let del_target = c
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            body: "Post to delete".into(),
            ..Default::default()
        })
        .await
        .expect("post to delete");
    let _ = c.delete_post(&del_target.post.post_id).await.expect("delete_post");
    assert!(matches!(
        c.repost(&del_target.post.post_id, &RepostRequest { comment: None }).await,
        Err(TelegramError::NotFound(_))
    ));
    assert!(matches!(
        c.get_comments(&del_target.post.post_id, &CommentsQueryParams::default()).await,
        Err(TelegramError::NotFound(_))
    ));

    // Insert a private Draft authored by another user (seeded u:8888) and verify quote/repost/get_comments all reject with NotFound
    let other_draft = c
        .local_runtime()
        .store()
        .create_post(
            "u:8888",
            &CreatePostRequest {
                community_slug: Some("rust".into()),
                title: Some("Other User Secret Draft".into()),
                body: "SUPER_SECRET_DRAFT_CONTENT".into(),
                status: PostStatus::Draft,
                ..Default::default()
            },
            chrono::Utc::now().timestamp_millis(),
        )
        .expect("create other user's draft");
    assert!(matches!(
        c.create_post(&CreatePostRequest {
            format: PostFormat::Quote,
            body: "Attempting to quote secret draft".into(),
            quoted_post_id: Some(other_draft.post.post_id.clone()),
            ..Default::default()
        })
        .await,
        Err(TelegramError::NotFound(_))
    ));
    assert!(matches!(
        c.repost(
            &other_draft.post.post_id,
            &RepostRequest {
                comment: Some("Exfil quote".into()),
            }
        )
        .await,
        Err(TelegramError::NotFound(_))
    ));
    assert!(matches!(
        c.get_comments(&other_draft.post.post_id, &CommentsQueryParams::default()).await,
        Err(TelegramError::NotFound(_))
    ));

    // 3. Local @mention of banned bot MUST NOT abort Human create_post / create_comment, and falls through to 2nd bot!
    let ban_comm = c
        .create_community(&CreateCommunityRequest {
            slug: "ban_mention_zone".into(),
            name: "Ban Mention Zone".into(),
            description: "Moderation test".into(),
            category: "tech".into(),
            banner_media_id: None,
            icon_media_id: None,
            icon_emoji: None,
            accent_color: None,
            rules: vec![],
            post_flairs: vec![],
            user_flairs: vec![],
            wiki_markdown: String::new(),
            automod_rules: vec![],
            allow_bots: true,
            require_bot_disclosure: true,
            is_nsfw: false,
        })
        .await
        .expect("create ban_mention_zone");
    let banned_bot = c
        .register_bot_actor(&RegisterBotActorRequest {
            bot_user_id: Some(5201),
            actor_kind: Some(ActorKind::Bot),
            handle: "banned_r2_bot".into(),
            display_name: "Banned R2 Bot".into(),
            bio: None,
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: None,
            avatar_color: None,
            bot_badge: None,
            bot_model: None,
            bot_capabilities: vec!["rust".into()],
            bot_autonomy_mode: BotAutonomyMode::Reactive,
            bot_persona: Some("Helpful".into()),
            cooldown_secs: None,
            max_posts_per_hour: None,
        })
        .await
        .expect("register banned_r2_bot");
    let allowed_bot = c
        .register_bot_actor(&RegisterBotActorRequest {
            bot_user_id: Some(5202),
            actor_kind: Some(ActorKind::Bot),
            handle: "allowed_r2_bot".into(),
            display_name: "Allowed R2 Bot".into(),
            bio: None,
            avatar_media_id: None,
            banner_media_id: None,
            avatar_shape: None,
            avatar_color: None,
            bot_badge: None,
            bot_model: None,
            bot_capabilities: vec!["rust".into()],
            bot_autonomy_mode: BotAutonomyMode::Reactive,
            bot_persona: Some("Helpful".into()),
            cooldown_secs: None,
            max_posts_per_hour: None,
        })
        .await
        .expect("register allowed_r2_bot");
    let _ = c
        .execute_mod_action(
            &ban_comm.slug,
            &ModActionRequest {
                action: ModActionKind::BanActor,
                target_kind: "actor".into(),
                target_id: banned_bot.actor.actor_id.clone(),
                reason: Some("test ban".into()),
                flair: None,
                queue_item_id: None,
            },
        )
        .await
        .expect("ban bot in community");

    let human_post_banned = c
        .create_post(&CreatePostRequest {
            community_slug: Some(ban_comm.slug.clone()),
            title: Some("Post mentioning banned bot".into()),
            body: "Hey @banned_r2_bot can you see this?".into(),
            ..Default::default()
        })
        .await
        .expect("Human post mentioning banned bot must succeed");
    assert_eq!(human_post_banned.post.comments_count, 0);

    let human_comment_banned = c
        .create_comment(
            &human_post_banned.post.post_id,
            &CreateCommentRequest {
                body: "Pinging @banned_r2_bot in comment too".into(),
                ..Default::default()
            },
        )
        .await
        .expect("Human comment mentioning banned bot must succeed");
    assert_eq!(human_comment_banned.comment.author_actor_id, "u:101");

    let fallthrough_post = c
        .create_post(&CreatePostRequest {
            community_slug: Some(ban_comm.slug.clone()),
            title: Some("Multi-mention fallthrough".into()),
            body: "Calling @banned_r2_bot and @allowed_r2_bot for review!".into(),
            ..Default::default()
        })
        .await
        .expect("multi-mention post must succeed");
    assert_eq!(fallthrough_post.post.comments_count, 1);
    let fallthrough_detail = c
        .get_post(&fallthrough_post.post.post_id, Some(CommentSort::Old))
        .await
        .expect("fallthrough detail");
    assert!(fallthrough_detail
        .comments
        .iter()
        .any(|cm| cm.author_actor_id == allowed_bot.actor.actor_id));

    // 4. Unread notifications are not crowded out by >50 newer read notifications
    c.local_runtime()
        .store()
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO community_notifications
                 (notif_id, recipient_actor_id, actor_id, kind, target_post_id, target_comment_id, preview_text, is_read, created_at_ms)
                 VALUES ('notif_unread_old', 'u:101', 'u:8888', 'mention', NULL, NULL, 'Old unread alert', 0, 1000)",
                [],
            )
            .unwrap();
            for i in 0..60i64 {
                conn.execute(
                    "INSERT INTO community_notifications
                     (notif_id, recipient_actor_id, actor_id, kind, target_post_id, target_comment_id, preview_text, is_read, created_at_ms)
                     VALUES (?1, 'u:101', 'u:8888', 'reply', NULL, NULL, 'Read item', 1, ?2)",
                    rusqlite::params![format!("notif_read_{i}"), 2000 + i],
                )
                .unwrap();
            }
            Ok(())
        })
        .expect("seed notifications");
    let unread_resp = c
        .get_notifications(None, Some(10), Some(true))
        .await
        .expect("get_notifications unread_only");
    assert!(
        unread_resp
            .notifications
            .iter()
            .any(|n| n.notif_id == "notif_unread_old" && !n.is_read),
        "older unread notification must be returned when unread_only=true"
    );

    // 5. Top-level summon_bot(parent_comment_id: None) 5-hop cap enforced ACROSS client restart!
    let top_loop_post = c
        .create_post(&CreatePostRequest {
            community_slug: Some("rust".into()),
            body: "Top-level 5-hop persistence post".into(),
            ..Default::default()
        })
        .await
        .expect("top_loop_post");
    for hop in 1..=5 {
        let speaker = if hop % 2 == 1 {
            banned_bot.actor.actor_id.clone()
        } else {
            allowed_bot.actor.actor_id.clone()
        };
        c.set_active_actor_id(Some(speaker));
        let _ = c
            .create_comment(
                &top_loop_post.post.post_id,
                &CreateCommentRequest {
                    parent_comment_id: None,
                    body: format!("Top-level bot turn #{hop}"),
                    ..Default::default()
                },
            )
            .await
            .expect("top-level bot comment");
    }
    drop_client(client).await;

    // Reopen fresh client (fresh BotLoopGuard!) and attempt top-level bot summon (hop 6)
    let reopened = open_client(&closed_url, dir.path());
    reopened.set_auth_session(alice);
    let rc = reopened.community();
    rc.set_active_actor_id(Some(banned_bot.actor.actor_id.clone()));
    let hop6 = rc
        .summon_bot(
            &top_loop_post.post.post_id,
            &SummonBotRequest {
                bot_actor_id: Some(allowed_bot.actor.actor_id.clone()),
                parent_comment_id: None,
                prompt: Some("Continue top-level bot chain!".into()),
                ..Default::default()
            },
        )
        .await
        .expect("hop6 summon_bot returns clean response");
    assert!(
        !hop6.triggered,
        "top-level bot summon hop 6 MUST be blocked across restart, got {hop6:?}"
    );
    assert!(hop6.created_comment.is_none());

    // Explicit Human summon MUST still reset hops and succeed!
    rc.set_active_actor_id(Some("u:101".to_string()));
    let human_summon = rc
        .summon_bot(
            &top_loop_post.post.post_id,
            &SummonBotRequest {
                bot_actor_id: Some(allowed_bot.actor.actor_id.clone()),
                parent_comment_id: None,
                prompt: Some("Human explicitly summoning bot after 5 bot turns".into()),
                ..Default::default()
            },
        )
        .await
        .expect("human summon_bot");
    assert!(human_summon.triggered);
    assert!(human_summon.created_comment.is_some());

    drop_client(reopened).await;
}

#[tokio::test]
async fn test_r2_mid_session_server_stop_acting_as_online_registered_bot() {
    // Scenario A: Online register_bot_actor -> set_active_actor_id("b:42") -> server stops -> seamless fallback!
    {
        let state = MockState::default();
        let server = spawn_router(build_full_mock_router(state)).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let client = open_client(&server.base_url, dir.path());
        client.set_auth_session(make_auth_session(1001, "alice_failover", "tok_alice"));
        let c = client.community();

        let reg = c
            .register_bot_actor(&RegisterBotActorRequest {
                bot_user_id: Some(42),
                actor_kind: Some(ActorKind::Bot),
                handle: "rust_bot".into(),
                display_name: "Rust Bot".into(),
                bio: None,
                avatar_media_id: None,
                banner_media_id: None,
                avatar_shape: None,
                avatar_color: None,
                bot_badge: None,
                bot_model: None,
                bot_capabilities: vec!["rust".into()],
                bot_autonomy_mode: BotAutonomyMode::Reactive,
                bot_persona: None,
                cooldown_secs: None,
                max_posts_per_hour: None,
            })
            .await
            .expect("online register_bot_actor");
        assert_eq!(reg.actor.actor_id, "b:42");
        assert_eq!(c.runtime_mode(), CommunityRuntimeMode::Online);

        c.set_active_actor_id(Some("b:42".to_string()));
        let _ = c.get_feed(&FeedRequestDto::default()).await.expect("online feed as b:42");
        assert_eq!(c.runtime_mode(), CommunityRuntimeMode::Online);

        // Shut down live server mid-session!
        server.stop().await;

        let fallback_feed = c
            .get_feed(&FeedRequestDto::default())
            .await
            .expect("mid-session fallback get_feed while acting as online-registered b:42 must succeed");
        assert_eq!(c.runtime_mode(), CommunityRuntimeMode::LocalFallback);
        assert!(fallback_feed.items.is_empty(), "fresh local DB must start empty with zero fake posts");

        setup_local_test_rust_community(c.local_runtime());
        let fallback_post = c
            .create_post(&CreatePostRequest {
                community_slug: Some("rust".into()),
                body: "Offline post authored by online-registered b:42 after failover".into(),
                ..Default::default()
            })
            .await
            .expect("mid-session fallback create_post as b:42 must succeed");
        assert_eq!(fallback_post.post.author_actor_id, "b:42");

        drop_client(client).await;
    }

    // Scenario B: Online get_identities (without register_bot_actor) -> act as b:42 -> server stops -> seamless fallback!
    {
        let state = MockState::default();
        let server = spawn_router(build_full_mock_router(state)).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let client = open_client(&server.base_url, dir.path());
        client.set_auth_session(make_auth_session(1001, "alice_ident_failover", "tok_alice"));
        let c = client.community();

        let ids = c.get_identities().await.expect("online get_identities");
        assert!(ids.bot_actors.iter().any(|b| b.actor_id == "b:42"));
        c.set_active_actor_id(Some("b:42".to_string()));

        server.stop().await;

        let fallback_feed = c
            .get_feed(&FeedRequestDto::default())
            .await
            .expect("mid-session fallback get_feed after get_identities mirror must succeed");
        assert_eq!(c.runtime_mode(), CommunityRuntimeMode::LocalFallback);
        assert!(fallback_feed.items.is_empty(), "fresh local DB must start empty with zero fake posts");

        drop_client(client).await;
    }
}

#[tokio::test]
async fn test_r9_create_post_as_actor_header_isolation_and_validation() {
    // 1. Online mode: create_post_as sends X-Community-Actor-Id for the bot WITHOUT mutating
    //    shared active_actor_id, while concurrent/subsequent calls continue using Human u:1001.
    let state = MockState::default();
    let server = spawn_router(build_full_mock_router(state.clone())).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let client = open_client(&server.base_url, dir.path());
    client.set_auth_session(make_auth_session(1001, "alice_iso", "tok_alice"));
    let c = client.community();
    c.set_active_actor_id(Some("u:1001".to_string()));

    let req = CreatePostRequest {
        community_slug: Some("rust".into()),
        body: "Bot-authored test post via create_post_as".into(),
        ..Default::default()
    };
    let created = c
        .create_post_as("  b:42  ", &req)
        .await
        .expect("create_post_as succeeds");
    assert!(!created.post.post_id.is_empty());
    assert_eq!(
        c.active_actor_id().as_deref(),
        Some("u:1001"),
        "create_post_as must NEVER mutate facade.active_actor_id"
    );

    let _ = c
        .vote_post("p_1", &VoteRequest { direction: 1 })
        .await
        .expect("vote_post succeeds");

    {
        let captured = state.captured.lock();
        let create_req = captured
            .iter()
            .find(|r| r.method == "POST" && r.path == "/api/v1/community/posts")
            .expect("captured POST /posts");
        assert_eq!(create_req.actor_id.as_deref(), Some("b:42"));

        let vote_req = captured
            .iter()
            .find(|r| r.method == "POST" && r.path == "/api/v1/community/posts/p_1/vote")
            .expect("captured POST /vote");
        assert_eq!(vote_req.actor_id.as_deref(), Some("u:1001"));
    }

    // 2. Empty, space-only, and CRLF/control-character actor_ids must all be rejected cleanly
    //    (never silently falling back to Human active_actor_id).
    for bad in ["", "   ", "\r\n", "b:42\r\nX-Injected: 1", "b:42\x00"] {
        let err = c
            .create_post_as(bad, &req)
            .await
            .expect_err(&format!("bad actor_id {bad:?} must fail validation"));
        assert!(
            matches!(err, TelegramError::Validation(_)),
            "expected Validation for {bad:?}, got {err:?}"
        );
    }
    assert_eq!(c.active_actor_id().as_deref(), Some("u:1001"));

    // 3. Offline/local fallback mode: create_post_as authors under b:42 while active_actor_id stays u:1001.
    let _ = c.get_identities().await.expect("mirror b:42 locally");
    server.stop().await;

    setup_local_test_rust_community(c.local_runtime());
    let local_post = c
        .create_post_as("b:42", &req)
        .await
        .expect("local create_post_as b:42 succeeds");
    assert_eq!(local_post.post.author_actor_id, "b:42");
    assert_eq!(c.active_actor_id().as_deref(), Some("u:1001"));

    let crlf_local = c
        .create_post_as("b:42\r\n", &req)
        .await
        .expect_err("local CRLF actor_id must fail");
    assert!(matches!(crlf_local, TelegramError::Validation(_)));

    drop_client(client).await;
}

#[tokio::test]
async fn test_r10_actor_id_unicode_obs_text_rejection_and_offline_viewer_hydration() {
    // Part A (R9-HUNT-1): Unicode whitespace (NBSP, U+3000, NEL, U+2028), zero-width/BOM,
    // obs-text/emoji, embedded space, and ASCII controls MUST all fail client-side validation
    // with ZERO HTTP requests captured (never silently falling back to Human Bearer identity).
    let state = MockState::default();
    let server = spawn_router(build_full_mock_router(state.clone())).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let client = open_client(&server.base_url, dir.path());
    client.set_auth_session(make_auth_session(1001, "alice_r10", "tok_alice"));
    let c = client.community();
    let http = client.http_client.clone();
    c.set_active_actor_id(Some("u:1001".to_string()));

    let req = CreatePostRequest {
        community_slug: Some("rust".into()),
        body: "R10 actor validation test".into(),
        ..Default::default()
    };

    let bad_payloads = [
        "",
        "   ",
        "\t",
        "\r\n",
        "\x00",
        "\x7f",
        "b:42\r\nX-Injected: 1",
        "b:42 evil",
        "\u{00A0}",
        "\u{3000}",
        " \u{00A0} ",
        "\u{0085}",
        "\u{2028}",
        "\u{2029}",
        "\u{200B}",
        "\u{200C}",
        "\u{200D}",
        "\u{FEFF}",
        "b:42\u{00A0}",
        "b:42\u{200B}",
        "b:42🦀",
    ];

    for bad in bad_payloads {
        let before = state.captured.lock().len();
        let err = c
            .create_post_as(bad, &req)
            .await
            .expect_err(&format!("online create_post_as({bad:?}) must fail validation"));
        assert!(
            matches!(err, TelegramError::Validation(_)),
            "online create_post_as({bad:?}) expected Validation, got {err:?}"
        );
        assert_eq!(
            state.captured.lock().len(),
            before,
            "online create_post_as({bad:?}) must send 0 HTTP requests"
        );
        assert_eq!(
            c.active_actor_id().as_deref(),
            Some("u:1001"),
            "create_post_as({bad:?}) must not mutate active_actor_id"
        );

        // Direct HttpClient::community_create_post check for non-empty bad headers
        if !bad.is_empty() && bad != "   " {
            let http_err = http
                .community_create_post(Some("tok_alice"), Some(bad), &req)
                .await
                .expect_err(&format!("direct HTTP act_as={bad:?} must fail"));
            assert!(
                matches!(http_err, gpui_telegram_client::CommunityApiError::InvalidInput(_)),
                "direct HTTP act_as={bad:?} expected InvalidInput, got {http_err:?}"
            );
            assert_eq!(
                state.captured.lock().len(),
                before,
                "direct HTTP act_as={bad:?} must send 0 HTTP requests"
            );
        }
    }

    // set_active_actor_id with Unicode whitespace / obs-text must fail outbound get_feed with
    // 0 HTTP requests and must NOT poison persisted SQLite active_actor_id on disk!
    for bad in ["\u{00A0}", "\u{3000}", "\u{200B}", "\u{FEFF}", "b:42🦀", "b:42 evil"] {
        let before = state.captured.lock().len();
        c.set_active_actor_id(Some(bad.to_string()));
        let err = c
            .get_feed(&FeedRequestDto::default())
            .await
            .expect_err(&format!("online get_feed with active_actor_id={bad:?} must fail"));
        assert!(
            matches!(err, TelegramError::Validation(_)),
            "online get_feed({bad:?}) expected Validation, got {err:?}"
        );
        assert_eq!(
            state.captured.lock().len(),
            before,
            "online get_feed({bad:?}) must send 0 HTTP requests"
        );
    }

    // Re-bind same account scope: persisted active_actor_id must still be clean "u:1001"
    c.local_runtime()
        .bind_account_scope(1001, "alice_r10", Some("Alice R10"))
        .expect("rebind account scope");
    assert_eq!(
        c.active_actor_id().as_deref(),
        Some("u:1001"),
        "invalid actor_ids must NEVER persist to SQLite community_client_meta"
    );

    // Positive check: valid padded ASCII actor_id succeeds and sends normalized X-Community-Actor-Id
    let _ = c
        .create_post_as("  b:42  ", &req)
        .await
        .expect("padded valid actor_id must succeed");
    assert_eq!(
        state
            .captured
            .lock()
            .last()
            .and_then(|r| r.actor_id.as_deref()),
        Some("b:42")
    );

    // Switch to offline/local fallback mode and test BOTH R9-HUNT-1 offline & R9-HUNT-6!
    let _ = c.get_identities().await.expect("mirror b:42 locally");
    server.stop().await;

    for bad in bad_payloads {
        let err = c
            .create_post_as(bad, &req)
            .await
            .expect_err(&format!("offline create_post_as({bad:?}) must fail validation"));
        assert!(
            matches!(err, TelegramError::Validation(_)),
            "offline create_post_as({bad:?}) expected Validation, got {err:?}"
        );
    }
    for bad in ["\u{00A0}", "\u{200B}", "b:42🦀", "b:42 evil"] {
        c.set_active_actor_id(Some(bad.to_string()));
        let err = c
            .get_feed(&FeedRequestDto::default())
            .await
            .expect_err(&format!("offline get_feed({bad:?}) must fail"));
        assert!(matches!(err, TelegramError::Validation(_)));
    }
    c.set_active_actor_id(Some("u:1001".to_string()));

    // Part B (R9-HUNT-6): Offline LocalCommunityRuntime::get_community moderators AND
    // LocalCommunityRuntime::search actors MUST hydrate is_following / is_muted / is_blocked!
    let local_rt = c.local_runtime();
    setup_local_test_rust_community(local_rt);
    let comm_init = local_rt
        .get_community(Some("u:1001"), "rust")
        .expect("local get_community rust");
    let other_mod = comm_init
        .moderators
        .iter()
        .find(|m| m.actor_id != "u:1001")
        .cloned()
        .expect("rust community must have a non-viewer moderator");
    assert!(!other_mod.is_following && !other_mod.is_muted && !other_mod.is_blocked);

    // Follow + mute the moderator in local SQLite
    local_rt
        .follow_actor(Some("u:1001"), &other_mod.actor_id)
        .expect("local follow moderator");
    local_rt
        .set_mute_block(
            Some("u:1001"),
            &MuteBlockRequest {
                target_kind: MuteBlockTargetKind::Actor,
                target_value: other_mod.actor_id.clone(),
                action: MuteBlockActionKind::Mute,
                reason: None,
            },
        )
        .expect("local mute moderator");

    let comm_after_fm = local_rt
        .get_community(Some("u:1001"), "rust")
        .expect("local get_community after follow+mute");
    let mod_fm = comm_after_fm
        .moderators
        .iter()
        .find(|m| m.actor_id == other_mod.actor_id)
        .expect("moderator present");
    assert!(
        mod_fm.is_following && mod_fm.is_muted && !mod_fm.is_blocked,
        "R9-HUNT-6: offline get_community moderators must hydrate is_following+is_muted, got {mod_fm:?}"
    );

    // Bonus sibling check: offline search People/Bots must also hydrate viewer flags!
    let search_tab = if other_mod.actor_kind.is_bot_like() {
        CommunitySearchTab::Bots
    } else {
        CommunitySearchTab::People
    };
    let search_fm = local_rt
        .search(
            Some("u:1001"),
            &CommunitySearchRequestDto {
                tab: search_tab,
                limit: 50,
                ..Default::default()
            },
        )
        .expect("local search");
    let found_actor = search_fm
        .actors
        .iter()
        .find(|a| a.actor_id == other_mod.actor_id)
        .expect("moderator present in local search");
    assert!(
        found_actor.is_following && found_actor.is_muted && !found_actor.is_blocked,
        "R9-HUNT-6 sibling: offline search actors must hydrate is_following+is_muted, got {found_actor:?}"
    );

    // Block the moderator -> is_blocked=true, is_following=false in BOTH get_community & search!
    local_rt
        .set_mute_block(
            Some("u:1001"),
            &MuteBlockRequest {
                target_kind: MuteBlockTargetKind::Actor,
                target_value: other_mod.actor_id.clone(),
                action: MuteBlockActionKind::Block,
                reason: None,
            },
        )
        .expect("local block moderator");

    let comm_after_blk = local_rt
        .get_community(Some("u:1001"), "rust")
        .expect("local get_community after block");
    let mod_blk = comm_after_blk
        .moderators
        .iter()
        .find(|m| m.actor_id == other_mod.actor_id)
        .expect("moderator present after block");
    assert!(
        !mod_blk.is_following && mod_blk.is_blocked,
        "R9-HUNT-6: offline get_community moderator must reflect block+unfollow, got {mod_blk:?}"
    );
    let search_blk = local_rt
        .search(
            Some("u:1001"),
            &CommunitySearchRequestDto {
                tab: search_tab,
                limit: 50,
                ..Default::default()
            },
        )
        .expect("local search after block");
    let found_blk = search_blk
        .actors
        .iter()
        .find(|a| a.actor_id == other_mod.actor_id)
        .expect("actor present in search after block");
    assert!(
        !found_blk.is_following && found_blk.is_blocked,
        "R9-HUNT-6 sibling: offline search actor must reflect block+unfollow, got {found_blk:?}"
    );

    drop_client(client).await;
}



