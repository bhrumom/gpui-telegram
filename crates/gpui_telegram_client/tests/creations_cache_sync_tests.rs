use futures::{SinkExt, StreamExt};
use gpui_telegram_client::net::{
    AppStoreCatalogResponse, AuthResponse, BotRecommendationItem, CatalogQueryResponse,
    CatalogRecommendResponse, CatalogSortMode, CoverArtwork, CreateCreationRequest,
    CreateCreationReviewRequest, CreationAuditListResponse, CreationKind, CreationListResponse,
    CreationPayload, CreationReviewAuditDto, CreationReviewEntryDto, CreationReviewStatus,
    CreationReviewsResponse, CreationSyncResponse, CreationVisibility, GameLeaderboardEntryDto,
    GameLeaderboardResponse, HttpClientError, PublicCatalogItemDto, ReviewDecision,
    ReviewDecisionRequest, SubmitGameScoreRequest, SubmitReviewRequest, SubmitReviewResponse,
    UpdateCreationRequest, UpsertCreationStateRequest, UserCreationDto, UserCreationStateDto,
    UserCreationStatesResponse,
};
use gpui_telegram_client::{ClientConfig, ClientDomainEvent, LocalStore, TelegramClient};
use gpui_telegram_proto::creations::{BookChapter, BookPage, BookPayload, BookRichBlock};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use parking_lot::Mutex;
use rusqlite::Connection;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as WsMessage;

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> (String, String, Vec<u8>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut header_end = None;
    while header_end.is_none() {
        let n = stream.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = Some(pos + 4);
        }
    }
    let hend = header_end.unwrap_or(buf.len());
    let headers_str = String::from_utf8_lossy(&buf[..hend]).to_string();
    let content_len = headers_str
        .lines()
        .find_map(|l| {
            let lower = l.to_ascii_lowercase();
            lower
                .strip_prefix("content-length:")
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while buf.len() < hend + content_len {
        let n = stream.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let first_line = headers_str.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    (method, path, buf[hend..].to_vec())
}

async fn write_json_response(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    reason: &str,
    body: &str,
) {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
}

fn sample_book_payload() -> CreationPayload {
    CreationPayload::Book(BookPayload {
        author: "Alice Author".to_string(),
        series_name: None,
        series_index: None,
        blurb: "A rich space exploration handbook.".to_string(),
        estimated_read_mins: 12,
        word_count: 1500,
        chapters: vec![BookChapter {
            chapter_id: "ch1".to_string(),
            chapter_number: 1,
            title: "Chapter 1".to_string(),
            subtitle: None,
            pages: vec![BookPage {
                page_number: 1,
                blocks: vec![BookRichBlock::Paragraph {
                    text: "This chapter covers orbital mechanics, star charts, and navigation protocols across deep space.".to_string(),
                }],
            }],
        }],
    })
}

fn sample_creation(
    id: &str,
    owner: i64,
    kind: CreationKind,
    status: CreationReviewStatus,
    rev: i64,
    updated_at_ms: i64,
    deleted_at_ms: Option<i64>,
) -> UserCreationDto {
    UserCreationDto {
        creation_id: id.to_string(),
        owner_user_id: owner,
        creator_bot_user_id: Some(777_020),
        creator_chat_id: Some(500),
        kind,
        title: format!("Title {id}"),
        subtitle: "Subtitle".to_string(),
        summary: "Detailed summary for sample creation".to_string(),
        author_display_name: "Alice Author".to_string(),
        language: "en".to_string(),
        genres: vec!["sci-fi".to_string()],
        tags: vec!["space".to_string()],
        cover: CoverArtwork::default(),
        age_rating: "4+".to_string(),
        payload: sample_book_payload(),
        version: 1,
        cloud_revision: rev,
        review_status: status,
        visibility: CreationVisibility::Public,
        has_live_published_version: status == CreationReviewStatus::Published,
        published_version: (status == CreationReviewStatus::Published).then_some(1),
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: None,
        reviewed_at_ms: None,
        published_at_ms: (status == CreationReviewStatus::Published).then_some(updated_at_ms),
        created_at_ms: 1_000,
        updated_at_ms,
        deleted_at_ms,
        rating_avg: 4.8,
        rating_count: 12,
        engagement_count: 100,
        save_or_install_count: 42,
    }
}

fn sample_catalog_item(
    id: &str,
    kind: CreationKind,
    featured_rank: Option<u32>,
    published_at_ms: i64,
) -> PublicCatalogItemDto {
    PublicCatalogItemDto {
        creation_id: id.to_string(),
        owner_user_id: 1001,
        kind,
        title: format!("Catalog {id}"),
        subtitle: "Featured".to_string(),
        summary: "Public catalog item summary".to_string(),
        author_display_name: "Alice Author".to_string(),
        language: "en".to_string(),
        genres: vec!["sci-fi".to_string()],
        tags: vec!["featured".to_string()],
        cover: CoverArtwork::default(),
        age_rating: "4+".to_string(),
        published_version: 1,
        published_at_ms,
        updated_at_ms: published_at_ms,
        rating_avg: 4.9,
        rating_count: 25,
        engagement_count: 300,
        save_or_install_count: 90,
        featured_rank,
        deep_link: kind.deep_link_uri(id),
        payload: None,
    }
}

fn sample_state(id: &str, user_id: i64, kind: CreationKind, updated_at_ms: i64) -> UserCreationStateDto {
    UserCreationStateDto {
        creation_id: id.to_string(),
        user_id,
        kind,
        in_library: true,
        favorite: true,
        progress_percent: 65.5,
        state_json: serde_json::json!({"current_chapter": 2, "bookmark_page": 7}),
        updated_at_ms,
    }
}

fn auth_session(user_id: u64, username: &str, token: &str) -> AuthResponse {
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

#[test]
fn all_25_rest_methods_and_409_422_and_inline_query_shapes() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let seen_routes: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let seen_srv = seen_routes.clone();

    let addr = rt.block_on(async move {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let (method, path, body_bytes) = read_http_request(&mut stream).await;
                let body_str = String::from_utf8_lossy(&body_bytes).to_string();
                let base_path = path.split('?').next().unwrap_or(&path).to_string();
                let query = path.split('?').nth(1).unwrap_or("");
                seen_srv.lock().insert(format!("{method} {base_path}"));

                let (status, reason, resp_body) = match (method.as_str(), base_path.as_str()) {
                    ("POST", "/api/v1/creations") => (
                        201,
                        "Created",
                        serde_json::to_string(&sample_creation(
                            "cr_book_1",
                            1001,
                            CreationKind::Book,
                            CreationReviewStatus::Draft,
                            1,
                            2_000,
                            None,
                        ))
                        .unwrap(),
                    ),
                    ("GET", "/api/v1/creations/my") => {
                        assert!(query.contains("kind=book"), "query={query}");
                        assert!(query.contains("status=draft"), "query={query}");
                        assert!(query.contains("q=star%20map"), "query={query}");
                        let resp = CreationListResponse {
                            items: vec![sample_creation(
                                "cr_book_1",
                                1001,
                                CreationKind::Book,
                                CreationReviewStatus::Draft,
                                1,
                                2_000,
                                None,
                            )],
                            latest_revision: 1,
                        };
                        (200, "OK", serde_json::to_string(&resp).unwrap())
                    }
                    ("GET", "/api/v1/creations/sync") => {
                        if query == "since_revision=-1" {
                            (
                                400,
                                "Bad Request",
                                r#"{"error":"invalid_since_revision","message":"since_revision must be >= 0"}"#.to_string(),
                            )
                        } else {
                            let resp = CreationSyncResponse {
                                latest_revision: 2,
                                items: vec![sample_creation(
                                    "cr_book_1",
                                    1001,
                                    CreationKind::Book,
                                    CreationReviewStatus::Draft,
                                    2,
                                    2_100,
                                    None,
                                )],
                            };
                            (200, "OK", serde_json::to_string(&resp).unwrap())
                        }
                    }
                    ("GET", "/api/v1/creations/review-queue") => {
                        let resp = CreationListResponse {
                            items: vec![sample_creation(
                                "cr_book_1",
                                1001,
                                CreationKind::Book,
                                CreationReviewStatus::PendingReview,
                                3,
                                2_200,
                                None,
                            )],
                            latest_revision: 3,
                        };
                        (200, "OK", serde_json::to_string(&resp).unwrap())
                    }
                    ("GET", "/api/v1/creations/cr_book_1") => (
                        200,
                        "OK",
                        serde_json::to_string(&sample_creation(
                            "cr_book_1",
                            1001,
                            CreationKind::Book,
                            CreationReviewStatus::Draft,
                            2,
                            2_100,
                            None,
                        ))
                        .unwrap(),
                    ),
                    ("PUT", "/api/v1/creations/cr_book_1") => {
                        if body_str.contains("\"expected_revision\":1") {
                            (
                                409,
                                "Conflict",
                                r#"{"error":"revision_conflict","message":"expected revision 1 does not match current revision 3","expected_revision":1,"current_revision":3}"#.to_string(),
                            )
                        } else {
                            (
                                200,
                                "OK",
                                serde_json::to_string(&sample_creation(
                                    "cr_book_1",
                                    1001,
                                    CreationKind::Book,
                                    CreationReviewStatus::Draft,
                                    3,
                                    2_200,
                                    None,
                                ))
                                .unwrap(),
                            )
                        }
                    }
                    ("DELETE", "/api/v1/creations/cr_book_1") => (
                        200,
                        "OK",
                        serde_json::to_string(&sample_creation(
                            "cr_book_1",
                            1001,
                            CreationKind::Book,
                            CreationReviewStatus::Unpublished,
                            6,
                            2_600,
                            Some(2_600),
                        ))
                        .unwrap(),
                    ),
                    ("POST", "/api/v1/creations/cr_book_1/submit-review") => {
                        if body_str.contains("trigger_422") {
                            (
                                422,
                                "Unprocessable Entity",
                                r#"{"error":"validation_failed","message":"Creation failed review validation","validation_errors":["summary is too short","missing chapter text"]}"#.to_string(),
                            )
                        } else {
                            let resp = SubmitReviewResponse {
                                creation: sample_creation(
                                    "cr_book_1",
                                    1001,
                                    CreationKind::Book,
                                    CreationReviewStatus::PendingReview,
                                    4,
                                    2_300,
                                    None,
                                ),
                                validation_errors: vec![],
                            };
                            (200, "OK", serde_json::to_string(&resp).unwrap())
                        }
                    }
                    ("POST", "/api/v1/creations/cr_book_1/withdraw-review") => {
                        assert_eq!(body_str.trim(), "{}");
                        (
                            200,
                            "OK",
                            serde_json::to_string(&sample_creation(
                                "cr_book_1",
                                1001,
                                CreationKind::Book,
                                CreationReviewStatus::Draft,
                                5,
                                2_400,
                                None,
                            ))
                            .unwrap(),
                        )
                    }
                    ("POST", "/api/v1/creations/cr_book_1/unpublish") => {
                        assert_eq!(body_str.trim(), "{}");
                        (
                            200,
                            "OK",
                            serde_json::to_string(&sample_creation(
                                "cr_book_1",
                                1001,
                                CreationKind::Book,
                                CreationReviewStatus::Unpublished,
                                5,
                                2_500,
                                None,
                            ))
                            .unwrap(),
                        )
                    }
                    ("GET", "/api/v1/creations/cr_book_1/audit") => {
                        let resp = CreationAuditListResponse {
                            items: vec![CreationReviewAuditDto {
                                audit_id: 1,
                                creation_id: "cr_book_1".to_string(),
                                actor_user_id: 1001,
                                from_status: CreationReviewStatus::Draft,
                                to_status: CreationReviewStatus::PendingReview,
                                action: "submit_review".to_string(),
                                reason: None,
                                notes: Some("ready".to_string()),
                                validation_errors: vec![],
                                created_at_ms: 2_300,
                            }],
                        };
                        (200, "OK", serde_json::to_string(&resp).unwrap())
                    }
                    ("POST", "/api/v1/creations/cr_book_1/review") => (
                        200,
                        "OK",
                        serde_json::to_string(&sample_creation(
                            "cr_book_1",
                            1001,
                            CreationKind::Book,
                            CreationReviewStatus::Published,
                            5,
                            2_450,
                            None,
                        ))
                        .unwrap(),
                    ),
                    _ => handle_catalog_and_inline_mock(&method, &base_path, query),
                };
                write_json_response(&mut stream, status, reason, &resp_body).await;
            }
        });
        addr
    });

    let dir = tempfile::tempdir().unwrap();
    let cfg = ClientConfig {
        server_url: format!("http://{addr}"),
        ..Default::default()
    };
    let client = TelegramClient::open_with_dir(cfg, dir.path()).unwrap();
    client.set_auth_session(auth_session(1001, "alice", "tok_test"));

    rt.block_on(async {
        let create_req = CreateCreationRequest {
            kind: CreationKind::Book,
            title: "Star Atlas".to_string(),
            subtitle: "Vol 1".to_string(),
            summary: "Comprehensive celestial atlas".to_string(),
            author_display_name: "Alice Author".to_string(),
            language: "en".to_string(),
            genres: vec!["sci-fi".to_string()],
            tags: vec!["space".to_string()],
            cover: CoverArtwork::default(),
            age_rating: "4+".to_string(),
            visibility: CreationVisibility::Public,
            creator_bot_user_id: None,
            creator_chat_id: None,
            payload: sample_book_payload(),
        };
        let created = client.create_creation(&create_req).await.unwrap();
        assert_eq!(created.creation_id, "cr_book_1");
        assert!(client.local_store.get_user_creation("cr_book_1").is_some());

        let my_list = client
            .list_my_creations(
                Some(CreationKind::Book),
                Some(CreationReviewStatus::Draft),
                Some("star map"),
            )
            .await
            .unwrap();
        assert_eq!(my_list.items.len(), 1);

        let sync_ok = client.sync_creations(Some(0)).await.unwrap();
        assert_eq!(sync_ok.latest_revision, 2);
        let sync_neg = client.sync_creations(Some(-1)).await;
        assert!(matches!(sync_neg, Err(HttpClientError::Api { status: 400, .. })));

        let fetched = client.get_creation("cr_book_1").await.unwrap();
        assert_eq!(fetched.cloud_revision, 2);

        let bad_id = client.get_creation("   ").await;
        assert!(matches!(bad_id, Err(HttpClientError::Api { status: 400, .. })));
        let dot_id = client.get_creation("..").await;
        assert!(matches!(dot_id, Err(HttpClientError::Api { status: 400, .. })));
        let single_dot_id = client.get_creation(".").await;
        assert!(matches!(single_dot_id, Err(HttpClientError::Api { status: 400, .. })));

        let conflict = client
            .update_creation(
                "cr_book_1",
                &UpdateCreationRequest {
                    expected_revision: Some(1),
                    title: Some("Stale edit".to_string()),
                    ..Default::default()
                },
            )
            .await;
        match conflict {
            Err(HttpClientError::Conflict(msg)) => {
                assert!(msg.contains("revision_conflict"), "msg={msg}");
                assert!(msg.contains('1') && msg.contains('3'), "msg={msg}");
            }
            other => panic!("expected 409 Conflict, got {other:?}"),
        }

        let updated = client
            .update_creation(
                "cr_book_1",
                &UpdateCreationRequest {
                    expected_revision: Some(2),
                    title: Some("Updated Atlas".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.cloud_revision, 3);

        let err_422 = client
            .submit_creation_for_review(
                "cr_book_1",
                Some(&SubmitReviewRequest {
                    visibility: Some(CreationVisibility::Public),
                    notes: Some("trigger_422".to_string()),
                }),
            )
            .await;
        match err_422 {
            Err(HttpClientError::Api { status: 422, message }) => {
                assert!(message.contains("summary is too short"), "msg={message}");
                assert!(message.contains("missing chapter text"), "msg={message}");
            }
            other => panic!("expected 422 Api error, got {other:?}"),
        }

        let submitted = client
            .submit_creation_for_review("cr_book_1", None)
            .await
            .unwrap();
        assert_eq!(submitted.creation.review_status, CreationReviewStatus::PendingReview);

        let queue = client
            .list_review_queue(Some(CreationReviewStatus::PendingReview), Some(CreationKind::Book))
            .await
            .unwrap();
        assert_eq!(queue.items.len(), 1);

        let audit = client.get_creation_audit("cr_book_1").await.unwrap();
        assert_eq!(audit.items.len(), 1);

        let withdrawn = client.withdraw_creation_review("cr_book_1").await.unwrap();
        assert_eq!(withdrawn.review_status, CreationReviewStatus::Draft);

        let approved = client
            .submit_review_decision(
                "cr_book_1",
                &ReviewDecisionRequest {
                    decision: ReviewDecision::Approve,
                    visibility: Some(CreationVisibility::Public),
                    reason: None,
                    notes: Some("looks great".to_string()),
                },
            )
            .await
            .unwrap();
        assert_eq!(approved.review_status, CreationReviewStatus::Published);

        let unpub = client.unpublish_creation("cr_book_1").await.unwrap();
        assert_eq!(unpub.review_status, CreationReviewStatus::Unpublished);

        let deleted = client.delete_creation("cr_book_1").await.unwrap();
        assert!(deleted.deleted_at_ms.is_some());
        assert!(client.local_store.get_user_creation("cr_book_1").is_none());

        let cat_zero = client
            .search_public_catalog(
                Some(CreationKind::Book),
                Some("atlas"),
                Some("sci-fi"),
                Some(CatalogSortMode::TopRated),
                Some(0),
                Some(0),
            )
            .await
            .unwrap();
        assert_eq!(cat_zero.items.len(), 0);
        assert_eq!(cat_zero.total, 7);

        assert_eq!(
            client.list_catalog_books(None, None, None, Some(10), None).await.unwrap().items.len(),
            1
        );
        assert_eq!(
            client.list_catalog_music(None, None, None, Some(10), None).await.unwrap().items.len(),
            1
        );
        assert_eq!(
            client.list_catalog_games(None, None, None, Some(10), None).await.unwrap().items.len(),
            1
        );
        let appstore = client.list_catalog_appstore(None, None, None, None, None).await.unwrap();
        assert_eq!(appstore.apps.len(), 1);

        let recs = client
            .recommend_public_catalog(Some("space"), Some(CreationKind::Book), None, Some(5))
            .await
            .unwrap();
        assert_eq!(recs.items.len(), 1);

        let saved_state = client
            .upsert_creation_state(
                "cr_cat_book",
                &UpsertCreationStateRequest {
                    in_library: Some(true),
                    favorite: Some(true),
                    progress_percent: Some(65.5),
                    state_json: Some(serde_json::json!({"page": 7})),
                },
            )
            .await
            .unwrap();
        assert_eq!(saved_state.creation_id, "cr_cat_book");

        let states = client.list_my_creation_states(Some(CreationKind::Book)).await.unwrap();
        assert_eq!(states.items.len(), 1);

        let rev_entry = client
            .submit_creation_review(
                "cr_cat_book",
                &CreateCreationReviewRequest {
                    rating: 5,
                    title: Some("Awesome".to_string()),
                    body: "Great read!".to_string(),
                },
            )
            .await
            .unwrap();
        assert_eq!(rev_entry.rating, 5);

        let revs = client
            .list_creation_reviews("cr_cat_book", Some(10), Some(0))
            .await
            .unwrap();
        assert_eq!(revs.rating_count, 1);

        let score = client
            .submit_game_score(
                "cr_cat_game",
                &SubmitGameScoreRequest {
                    score: 1200,
                    metadata: Some(serde_json::json!({"level": 3})),
                },
            )
            .await
            .unwrap();
        assert_eq!(score.score, 1200);

        let board = client.get_game_leaderboard("cr_cat_game", Some(10)).await.unwrap();
        assert_eq!(board.items.len(), 1);

        // Inline query: M3 live envelope + items + dual + bare + empty query + '@' stripping
        let iq_results = client
            .query_bot_inline("@CreatorStudioBot", "", Some(CreationKind::Book), Some(5))
            .await
            .unwrap();
        assert_eq!(iq_results.query_id.as_deref(), Some("iq_live_m3_0001"));
        assert_eq!(iq_results.bot_username, "CreatorStudioBot");
        assert_eq!(iq_results.total, 1);
        assert_eq!(iq_results.items.len(), 1);
        assert_eq!(iq_results.items[0].creation_id, "cr_iq_results");
        assert_eq!(iq_results.items[0].result_type.as_deref(), Some("article"));
        assert!(iq_results.items[0].input_message_content.is_some());
        assert!(iq_results.items[0].reply_markup.is_some());

        let iq_items = client
            .query_bot_inline("CreatorStudioBot", "shape_items", None, None)
            .await
            .unwrap();
        assert_eq!(iq_items.total, 1);
        assert_eq!(iq_items.items[0].creation_id, "cr_iq_items");

        let iq_dual = client
            .query_bot_inline("CreatorStudioBot", "shape_dual", None, None)
            .await
            .unwrap();
        assert_eq!(iq_dual.items[0].creation_id, "cr_iq_dual");

        let iq_bare = client
            .query_bot_inline("CreatorStudioBot", "shape_bare", None, None)
            .await
            .unwrap();
        assert_eq!(iq_bare.total, 1);
        assert_eq!(iq_bare.items[0].creation_id, "cr_iq_bare");
    });

    assert!(seen_routes.lock().len() >= 25, "routes={:?}", *seen_routes.lock());
}

fn handle_catalog_and_inline_mock(method: &str, base_path: &str, query: &str) -> (u16, &'static str, String) {
    match (method, base_path) {
        ("GET", "/api/v1/catalog") => {
            assert!(query.contains("limit=0"), "query={query}");
            let resp = CatalogQueryResponse { items: vec![], total: 7 };
            (200, "OK", serde_json::to_string(&resp).unwrap())
        }
        ("GET", "/api/v1/catalog/books") => {
            let resp = CatalogQueryResponse {
                items: vec![sample_catalog_item("cr_cat_book", CreationKind::Book, Some(1), 3_000)],
                total: 1,
            };
            (200, "OK", serde_json::to_string(&resp).unwrap())
        }
        ("GET", "/api/v1/catalog/music") => {
            let resp = CatalogQueryResponse {
                items: vec![sample_catalog_item("cr_cat_music", CreationKind::Music, Some(2), 3_100)],
                total: 1,
            };
            (200, "OK", serde_json::to_string(&resp).unwrap())
        }
        ("GET", "/api/v1/catalog/games") => {
            let resp = CatalogQueryResponse {
                items: vec![sample_catalog_item("cr_cat_game", CreationKind::Game, Some(1), 3_200)],
                total: 1,
            };
            (200, "OK", serde_json::to_string(&resp).unwrap())
        }
        ("GET", "/api/v1/catalog/appstore") => {
            let resp = AppStoreCatalogResponse {
                featured: vec![sample_catalog_item("cr_cat_book", CreationKind::Book, Some(1), 3_000)],
                apps: vec![sample_catalog_item("cr_cat_app", CreationKind::App, Some(1), 3_300)],
                mini_apps: vec![],
                cross_vertical: vec![sample_catalog_item("cr_cat_game", CreationKind::Game, Some(2), 3_200)],
            };
            (200, "OK", serde_json::to_string(&resp).unwrap())
        }
        ("GET", "/api/v1/catalog/recommend") => {
            let resp = CatalogRecommendResponse {
                items: vec![BotRecommendationItem {
                    creation_id: "cr_cat_book".to_string(),
                    kind: CreationKind::Book,
                    title: "Catalog cr_cat_book".to_string(),
                    subtitle: "Featured".to_string(),
                    summary: "Recommended".to_string(),
                    author_display_name: "Alice Author".to_string(),
                    genres: vec!["sci-fi".to_string()],
                    cover: CoverArtwork::default(),
                    rating_avg: 4.9,
                    rating_count: 25,
                    score: 98.5,
                    reason: "Top rated match".to_string(),
                    deep_link: "tg://apple/books/cr_cat_book".to_string(),
                }],
            };
            (200, "OK", serde_json::to_string(&resp).unwrap())
        }
        ("PUT", "/api/v1/catalog/cr_cat_book/state") => (
            200,
            "OK",
            serde_json::to_string(&sample_state("cr_cat_book", 1001, CreationKind::Book, 3_400))
                .unwrap(),
        ),
        ("GET", "/api/v1/catalog/my-states") => {
            let resp = UserCreationStatesResponse {
                items: vec![sample_state("cr_cat_book", 1001, CreationKind::Book, 3_400)],
            };
            (200, "OK", serde_json::to_string(&resp).unwrap())
        }
        ("POST", "/api/v1/catalog/cr_cat_book/reviews") => {
            let resp = CreationReviewEntryDto {
                review_id: 1,
                creation_id: "cr_cat_book".to_string(),
                user_id: 1001,
                author_display_name: "Alice".to_string(),
                rating: 5,
                title: Some("Awesome".to_string()),
                body: "Great read!".to_string(),
                created_at_ms: 3_500,
                updated_at_ms: 3_500,
            };
            (201, "Created", serde_json::to_string(&resp).unwrap())
        }
        ("GET", "/api/v1/catalog/cr_cat_book/reviews") => {
            let resp = CreationReviewsResponse {
                items: vec![CreationReviewEntryDto {
                    review_id: 1,
                    creation_id: "cr_cat_book".to_string(),
                    user_id: 1001,
                    author_display_name: "Alice".to_string(),
                    rating: 5,
                    title: Some("Awesome".to_string()),
                    body: "Great read!".to_string(),
                    created_at_ms: 3_500,
                    updated_at_ms: 3_500,
                }],
                rating_avg: 5.0,
                rating_count: 1,
            };
            (200, "OK", serde_json::to_string(&resp).unwrap())
        }
        ("POST", "/api/v1/catalog/cr_cat_game/scores") => {
            let resp = GameLeaderboardEntryDto {
                rank: 1,
                creation_id: "cr_cat_game".to_string(),
                user_id: 1001,
                display_name: "Alice".to_string(),
                score: 1200,
                metadata: Some(serde_json::json!({"level": 3})),
                achieved_at_ms: 3_600,
            };
            (200, "OK", serde_json::to_string(&resp).unwrap())
        }
        ("GET", "/api/v1/catalog/cr_cat_game/leaderboard") => {
            let resp = GameLeaderboardResponse {
                items: vec![GameLeaderboardEntryDto {
                    rank: 1,
                    creation_id: "cr_cat_game".to_string(),
                    user_id: 1001,
                    display_name: "Alice".to_string(),
                    score: 1200,
                    metadata: Some(serde_json::json!({"level": 3})),
                    achieved_at_ms: 3_600,
                }],
            };
            (200, "OK", serde_json::to_string(&resp).unwrap())
        }
        ("GET", "/api/v1/bots/inline-query") => {
            assert!(query.contains("bot_username=CreatorStudioBot"), "query={query}");
            assert!(query.contains("query="), "query must always be present: {query}");
            if query.contains("query=shape_items") {
                (200, "OK", r#"{"items":[{"id":"cr_iq_items","kind":"music","title":"Jazz","summary":"Smooth","deep_link":"tg://apple/music/cr_iq_items"}]}"#.to_string())
            } else if query.contains("query=shape_dual") {
                (200, "OK", r#"{"results":[{"creation_id":"cr_iq_dual","kind":"game","title":"Dodge","description":"Fast","deep_link":"tg://apple/games/cr_iq_dual"}],"items":[{"creation_id":"cr_old","kind":"game","title":"Old"}]}"#.to_string())
            } else if query.contains("query=shape_bare") {
                (200, "OK", r#"[{"id":"cr_iq_bare","kind":"app","title":"Notes","description":"Clean","deep_link":"tg://apple/appstore/cr_iq_bare"}]"#.to_string())
            } else {
                // Exact M3 live wire envelope (no raw query echo)
                (
                    200,
                    "OK",
                    r#"{"query_id":"iq_live_m3_0001","bot_username":"CreatorStudioBot","total":1,"results":[{"id":"cr_iq_results","type":"article","creation_id":"cr_iq_results","kind":"book","title":"Atlas","subtitle":"Vol 1","description":"Stars","summary":"Stars","author_display_name":"Alice","genres":["sci-fi"],"rating_avg":4.9,"rating_count":10,"score":95.0,"reason":"Featured","deep_link":"tg://apple/books/cr_iq_results","input_message_content":{"message_text":"Atlas"},"reply_markup":{"inline_keyboard":[[{"text":"Open","url":"tg://apple/books/cr_iq_results"}]]}}],"items":[{"id":"cr_iq_results","type":"article","creation_id":"cr_iq_results","kind":"book","title":"Atlas","description":"Stars","summary":"Stars","deep_link":"tg://apple/books/cr_iq_results"}]}"#.to_string(),
                )
            }
        }
        _ => (404, "Not Found", format!(r#"{{"error":"not_found","message":"{method} {base_path}"}}"#)),
    }
}

#[test]
fn pre_m4_sqlite_upgrade_disk_restart_tombstones_and_monotonic_revision() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("client_cache.db");

    // Seed a pre-M4 SQLite DB containing only legacy tables + data
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE cache_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE chats (chat_id INTEGER PRIMARY KEY, payload_json TEXT NOT NULL);
             INSERT INTO cache_meta (key, value) VALUES ('legacy_marker', 'preserved');",
        )
        .unwrap();
    }

    // Upgrade via LocalStore::open_with_dir
    let store = LocalStore::open_with_dir(dir.path()).unwrap();
    assert_eq!(
        store.get_cache_meta("legacy_marker").as_deref(),
        Some("preserved")
    );
    assert_eq!(store.creations_last_revision(), 0);

    let c1_v1 = sample_creation(
        "cr_1",
        1001,
        CreationKind::Book,
        CreationReviewStatus::Draft,
        2,
        1_000,
        None,
    );
    let c2_v1 = sample_creation(
        "cr_2",
        1001,
        CreationKind::Music,
        CreationReviewStatus::Published,
        3,
        2_000,
        None,
    );
    let rev1 = store.apply_creations_sync_delta(3, &[c1_v1.clone(), c2_v1.clone()]);
    assert_eq!(rev1, 3);
    assert_eq!(store.load_user_creations(None, None).len(), 2);

    // Tombstone cr_1 at revision 5
    let c1_deleted = sample_creation(
        "cr_1",
        1001,
        CreationKind::Book,
        CreationReviewStatus::Unpublished,
        5,
        3_000,
        Some(3_000),
    );
    let rev2 = store.apply_creations_sync_delta(5, &[c1_deleted]);
    assert_eq!(rev2, 5);
    assert!(store.get_user_creation("cr_1").is_none());
    assert_eq!(store.load_user_creations(None, None).len(), 1);

    // Stale revision delta (rev=2, latest_revision=1) MUST NOT resurrect cr_1 or regress cursor
    let rev3 = store.apply_creations_sync_delta(1, &[c1_v1.clone()]);
    assert_eq!(rev3, 5, "creations_last_revision must be monotonic");
    assert!(
        store.get_user_creation("cr_1").is_none(),
        "stale revision must never resurrect soft-deleted creation"
    );

    // Persist state + public catalog and verify survival across disk restart
    store.upsert_user_creation_state(sample_state("cr_2", 1001, CreationKind::Music, 4_000));
    store.upsert_public_catalog_items(&[sample_catalog_item(
        "cr_pub_1",
        CreationKind::Book,
        Some(1),
        5_000,
    )]);
    drop(store);

    let reopened = LocalStore::open_with_dir(dir.path()).unwrap();
    assert_eq!(reopened.creations_last_revision(), 5);
    assert!(reopened.get_user_creation("cr_1").is_none());
    assert_eq!(reopened.load_user_creations(None, None).len(), 1);
    assert_eq!(reopened.load_user_creation_states().len(), 1);
    assert_eq!(reopened.load_public_catalog_items(Some(CreationKind::Book)).len(), 1);

    // Even after restart, stale revision replay MUST NOT resurrect tombstoned cr_1
    reopened.upsert_user_creations(&[c1_v1]);
    assert!(reopened.get_user_creation("cr_1").is_none());
}

#[test]
fn offline_network_fallback_preserves_401_and_403_errors() {
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Step 1: Seed cache using a closed dead local port (unreachable => HttpClientError::Network)
    let dead_port = {
        let sock = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        sock.local_addr().unwrap().port()
    };
    let offline_cfg = ClientConfig {
        server_url: format!("http://127.0.0.1:{dead_port}"),
        ..Default::default()
    };
    let offline_client = TelegramClient::open_with_dir(offline_cfg, dir.path()).unwrap();
    offline_client.set_auth_session(auth_session(1001, "alice", "tok_alice"));
    offline_client.local_store.apply_creations_sync_delta(
        4,
        &[sample_creation(
            "cr_offline_1",
            1001,
            CreationKind::Book,
            CreationReviewStatus::Draft,
            4,
            10_000,
            None,
        )],
    );
    offline_client.local_store.upsert_user_creation_state(sample_state(
        "cr_offline_1",
        1001,
        CreationKind::Book,
        10_100,
    ));
    let mut cat_jazz = sample_catalog_item("cr_offline_jazz", CreationKind::Music, Some(2), 10_250);
    cat_jazz.title = "Midnight Jazz Session".to_string();
    cat_jazz.genres = vec!["jazz".to_string()];
    cat_jazz.rating_avg = 5.0;
    cat_jazz.engagement_count = 900;

    let mut cat_ambient = sample_catalog_item("cr_offline_pub", CreationKind::Music, Some(1), 10_200);
    cat_ambient.title = "Ambient Drift".to_string();
    cat_ambient.genres = vec!["ambient".to_string()];
    cat_ambient.rating_avg = 4.2;
    cat_ambient.engagement_count = 150;

    offline_client
        .local_store
        .upsert_public_catalog_items(&[cat_ambient, cat_jazz]);

    rt.block_on(async {
        let cached_creations = offline_client.sync_my_creations_with_cache().await.unwrap();
        assert_eq!(cached_creations.len(), 1);
        assert_eq!(cached_creations[0].creation_id, "cr_offline_1");

        let cached_states = offline_client
            .sync_my_creation_states_with_cache(Some(CreationKind::Book))
            .await
            .unwrap();
        assert_eq!(cached_states.len(), 1);
        assert_eq!(cached_states[0].creation_id, "cr_offline_1");

        let cached_catalog = offline_client
            .sync_public_catalog_with_cache(
                Some(CreationKind::Music),
                None,
                None,
                Some(CatalogSortMode::TopRated),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(cached_catalog.len(), 2);
        assert_eq!(cached_catalog[0].creation_id, "cr_offline_jazz");

        // Offline q + genre + limit/offset filtering
        let filtered_jazz = offline_client
            .sync_public_catalog_with_cache(
                Some(CreationKind::Music),
                Some("midnight"),
                Some("jazz"),
                Some(CatalogSortMode::Popular),
                Some(5),
                Some(0),
            )
            .await
            .unwrap();
        assert_eq!(filtered_jazz.len(), 1);
        assert_eq!(filtered_jazz[0].creation_id, "cr_offline_jazz");

        let limit_zero = offline_client
            .sync_public_catalog_with_cache(Some(CreationKind::Music), None, None, None, Some(0), None)
            .await
            .unwrap();
        assert!(limit_zero.is_empty());
    });
    drop(offline_client);

    // Step 2: Live mock returning 401 and 403 MUST NOT be swallowed as offline fallback
    let auth_err_addr = rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let (_method, path, _body) = read_http_request(&mut stream).await;
                if path.starts_with("/api/v1/creations/sync") {
                    write_json_response(
                        &mut stream,
                        401,
                        "Unauthorized",
                        r#"{"error":"unauthorized","message":"token expired"}"#,
                    )
                    .await;
                } else {
                    write_json_response(
                        &mut stream,
                        403,
                        "Forbidden",
                        r#"{"error":"forbidden","message":"account suspended"}"#,
                    )
                    .await;
                }
            }
        });
        addr
    });

    let err_cfg = ClientConfig {
        server_url: format!("http://{auth_err_addr}"),
        ..Default::default()
    };
    let err_client = TelegramClient::new(err_cfg);
    err_client.set_auth_session(auth_session(1001, "alice", "tok_expired"));

    rt.block_on(async {
        let res_401 = err_client.sync_my_creations_with_cache().await;
        assert!(matches!(res_401, Err(HttpClientError::Unauthorized(_))));

        let res_states_403 = err_client.sync_my_creation_states_with_cache(None).await;
        assert!(matches!(
            res_states_403,
            Err(HttpClientError::Api { status: 403, .. })
        ));

        let res_cat_403 = err_client
            .sync_public_catalog_with_cache(None, None, None, None, None, None)
            .await;
        assert!(matches!(
            res_cat_403,
            Err(HttpClientError::Api { status: 403, .. })
        ));
    });
}

fn sqlite_table_count(db_path: &std::path::Path, table: &str) -> i64 {
    let conn = Connection::open(db_path).unwrap();
    let sql = format!("SELECT COUNT(*) FROM {table}");
    conn.query_row(&sql, [], |r| r.get::<_, i64>(0)).unwrap()
}

#[test]
fn account_switch_and_server_switch_atomically_wipe_creation_caches_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("client_cache.db");
    let cfg_srv_a = ClientConfig {
        server_url: "http://127.0.0.1:19001".to_string(),
        ..Default::default()
    };
    let cfg_srv_b = ClientConfig {
        server_url: "http://127.0.0.1:19002".to_string(),
        ..Default::default()
    };

    // 1. User A on Server A populates all 3 tables + creations_last_revision = 9
    {
        let client_a = TelegramClient::open_with_dir(cfg_srv_a.clone(), dir.path()).unwrap();
        client_a.set_auth_session(auth_session(1001, "alice", "tok_a"));
        client_a.local_store.apply_creations_sync_delta(
            9,
            &[sample_creation(
                "cr_alice_secret",
                1001,
                CreationKind::Book,
                CreationReviewStatus::Draft,
                9,
                5_000,
                None,
            )],
        );
        client_a.local_store.upsert_user_creation_state(sample_state(
            "cr_alice_secret",
            1001,
            CreationKind::Book,
            5_100,
        ));
        client_a.local_store.upsert_public_catalog_items(&[sample_catalog_item(
            "cr_pub_a",
            CreationKind::Game,
            Some(1),
            5_200,
        )]);
    }

    // 2. Reopen as SAME User A on SAME Server A => all data preserved
    {
        let client_a2 = TelegramClient::open_with_dir(cfg_srv_a.clone(), dir.path()).unwrap();
        client_a2.set_auth_session(auth_session(1001, "alice", "tok_a2"));
        assert_eq!(client_a2.local_store.creations_last_revision(), 9);
        assert_eq!(client_a2.load_cached_my_creations(None, None).len(), 1);
        assert_eq!(client_a2.load_cached_my_creation_states().len(), 1);
        assert_eq!(client_a2.load_cached_public_catalog(None).len(), 1);
    }

    // 3. Switch to User B (2002) on Server A WITHOUT explicit logout => atomic wipe
    {
        let client_b = TelegramClient::open_with_dir(cfg_srv_a.clone(), dir.path()).unwrap();
        client_b.set_auth_session(auth_session(2002, "bob", "tok_b"));
        assert_eq!(client_b.local_store.creations_last_revision(), 0);
        assert!(client_b.load_cached_my_creations(None, None).is_empty());
        assert!(client_b.load_cached_my_creation_states().is_empty());
        assert!(client_b.load_cached_public_catalog(None).is_empty());
    }
    assert_eq!(sqlite_table_count(&db_path, "user_creations_cache"), 0);
    assert_eq!(sqlite_table_count(&db_path, "user_creation_states_cache"), 0);
    assert_eq!(sqlite_table_count(&db_path, "public_catalog_cache"), 0);

    // 4. Populate for User B on Server A, then switch SAME User B to Server B => atomic wipe
    {
        let client_b_srv_a = TelegramClient::open_with_dir(cfg_srv_a, dir.path()).unwrap();
        client_b_srv_a.set_auth_session(auth_session(2002, "bob", "tok_b"));
        client_b_srv_a.local_store.apply_creations_sync_delta(
            12,
            &[sample_creation(
                "cr_bob_srv_a",
                2002,
                CreationKind::Music,
                CreationReviewStatus::Draft,
                12,
                6_000,
                None,
            )],
        );
        client_b_srv_a.local_store.upsert_user_creation_state(sample_state(
            "cr_bob_srv_a",
            2002,
            CreationKind::Music,
            6_100,
        ));
        client_b_srv_a.local_store.upsert_public_catalog_items(&[sample_catalog_item(
            "cr_pub_b",
            CreationKind::Music,
            None,
            6_200,
        )]);
    }
    {
        let client_b_srv_b = TelegramClient::open_with_dir(cfg_srv_b, dir.path()).unwrap();
        client_b_srv_b.set_auth_session(auth_session(2002, "bob", "tok_b_srv_b"));
        assert_eq!(client_b_srv_b.local_store.creations_last_revision(), 0);
        assert!(client_b_srv_b.load_cached_my_creations(None, None).is_empty());
        assert!(client_b_srv_b.load_cached_my_creation_states().is_empty());
        assert!(client_b_srv_b.load_cached_public_catalog(None).is_empty());
    }
    assert_eq!(sqlite_table_count(&db_path, "user_creations_cache"), 0);
    assert_eq!(sqlite_table_count(&db_path, "user_creation_states_cache"), 0);
    assert_eq!(sqlite_table_count(&db_path, "public_catalog_cache"), 0);
}

#[test]
fn live_tcp_ws_dispatches_user_creation_changed_and_public_catalog_changed_with_eviction() {
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let addr = rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut peek_buf = [0u8; 2048];
                    let n = stream.peek(&mut peek_buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&peek_buf[..n]);
                    if head.to_ascii_lowercase().contains("upgrade: websocket") {
                        if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                            let frames = vec![
                                WsServerFrame::Update(UpdateEvent::UserCreationChanged {
                                    creation_id: "cr_ws_book".to_string(),
                                    owner_user_id: 1001,
                                    kind: CreationKind::Book,
                                    review_status: CreationReviewStatus::Published,
                                    cloud_revision: 11,
                                }),
                                WsServerFrame::Update(UpdateEvent::PublicCatalogChanged {
                                    creation_id: "cr_ws_book".to_string(),
                                    kind: CreationKind::Book,
                                    action: "published".to_string(),
                                }),
                                WsServerFrame::Update(UpdateEvent::PublicCatalogChanged {
                                    creation_id: "cr_evict_me".to_string(),
                                    kind: CreationKind::Game,
                                    action: "removed".to_string(),
                                }),
                            ];
                            for frame in frames {
                                let text = serde_json::to_string(&frame).unwrap();
                                let _ = ws.send(WsMessage::Text(text.into())).await;
                            }
                            while let Some(Ok(_)) = ws.next().await {}
                        }
                        return;
                    }
                    let (_m, _path, _body) = read_http_request(&mut stream).await;
                    write_json_response(&mut stream, 200, "OK", r#"{"ok":true}"#).await;
                });
            }
        });
        addr
    });

    let cfg = ClientConfig {
        server_url: format!("http://{addr}"),
        ..Default::default()
    };
    let client = TelegramClient::open_with_dir(cfg, dir.path()).unwrap();
    client.set_auth_session(auth_session(1001, "alice", "tok_ws"));
    client.local_store.save_creations_last_revision(4);
    client.local_store.upsert_public_catalog_items(&[
        sample_catalog_item("cr_evict_me", CreationKind::Game, Some(1), 8_000),
        sample_catalog_item("cr_keep_me", CreationKind::Music, Some(2), 8_100),
    ]);
    assert_eq!(client.load_cached_public_catalog(None).len(), 2);

    client.connect_ws();

    let mut received = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while received.len() < 3 && std::time::Instant::now() < deadline {
        received.extend(client.poll_events());
        if received.len() < 3 {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    client.disconnect_ws();

    assert!(
        received.iter().any(|ev| matches!(
            ev,
            ClientDomainEvent::UserCreationChanged {
                creation_id,
                owner_user_id: 1001,
                kind: CreationKind::Book,
                review_status: CreationReviewStatus::Published,
                cloud_revision: 11,
            } if creation_id == "cr_ws_book"
        )),
        "expected UserCreationChanged in {received:?}"
    );
    assert!(
        received.iter().any(|ev| matches!(
            ev,
            ClientDomainEvent::PublicCatalogChanged {
                creation_id,
                kind: CreationKind::Book,
                action,
            } if creation_id == "cr_ws_book" && action == "published"
        )),
        "expected PublicCatalogChanged(published) in {received:?}"
    );
    assert!(
        received.iter().any(|ev| matches!(
            ev,
            ClientDomainEvent::PublicCatalogChanged {
                creation_id,
                kind: CreationKind::Game,
                action,
            } if creation_id == "cr_evict_me" && action == "removed"
        )),
        "expected PublicCatalogChanged(removed) in {received:?}"
    );

    // UserCreationChanged metadata push must NOT advance creations_last_revision prematurely
    assert_eq!(client.local_store.creations_last_revision(), 4);

    // PublicCatalogChanged(removed) must evict cr_evict_me while keeping cr_keep_me
    let remaining = client.load_cached_public_catalog(None);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].creation_id, "cr_keep_me");
}

#[test]
fn state_stale_timestamp_guard_and_in_memory_store_parity() {
    // 1. Pure in-memory LocalStore parity for cache_meta, monotonic revision, and state timestamps
    let mem = LocalStore::new();
    mem.set_cache_meta("k1", "v1");
    assert_eq!(mem.get_cache_meta("k1").as_deref(), Some("v1"));
    mem.delete_cache_meta("k1");
    assert!(mem.get_cache_meta("k1").is_none());

    mem.save_creations_last_revision(10);
    mem.save_creations_last_revision(4);
    assert_eq!(mem.creations_last_revision(), 10);

    let newer = sample_state("cr_state_1", 1001, CreationKind::Book, 5_000);
    let mut older = sample_state("cr_state_1", 1001, CreationKind::Book, 4_000);
    older.progress_percent = 5.0;
    mem.upsert_user_creation_state(newer.clone());
    mem.upsert_user_creation_state(older.clone());
    assert_eq!(
        mem.get_user_creation_state("cr_state_1").unwrap().progress_percent,
        65.5
    );

    // 2. Disk-backed SQLite parity across restart
    let dir = tempfile::tempdir().unwrap();
    {
        let disk = LocalStore::open_with_dir(dir.path()).unwrap();
        disk.save_creations_last_revision(15);
        disk.save_creations_last_revision(3);
        assert_eq!(disk.creations_last_revision(), 15);
        disk.upsert_user_creation_state(newer);
        disk.upsert_user_creation_state(older);
        assert_eq!(
            disk.get_user_creation_state("cr_state_1").unwrap().progress_percent,
            65.5
        );
    }
    let reopened = LocalStore::open_with_dir(dir.path()).unwrap();
    assert_eq!(reopened.creations_last_revision(), 15);
    assert_eq!(
        reopened.get_user_creation_state("cr_state_1").unwrap().progress_percent,
        65.5
    );
}

#[test]
fn f9_local_store_memory_vs_sqlite_meta_and_revision_parity() {
    let dir = tempfile::tempdir().unwrap();
    let mem = LocalStore::new();
    let disk = LocalStore::open_with_dir(dir.path()).unwrap();

    let assert_parity = |label: &str, expected_rev: i64, expected_meta: Option<&str>| {
        assert_eq!(mem.creations_last_revision(), expected_rev, "mem rev {label}");
        assert_eq!(disk.creations_last_revision(), expected_rev, "disk rev {label}");
        assert_eq!(
            mem.get_cache_meta("creations_last_revision").as_deref(),
            expected_meta,
            "mem meta {label}"
        );
        assert_eq!(
            disk.get_cache_meta("creations_last_revision").as_deref(),
            expected_meta,
            "disk meta {label}"
        );
    };

    assert_parity("initial", 0, None);

    mem.save_creations_last_revision(15);
    disk.save_creations_last_revision(15);
    assert_parity("save 15", 15, Some("15"));

    mem.save_creations_last_revision(7);
    disk.save_creations_last_revision(7);
    assert_parity("monotonic ignore 7", 15, Some("15"));

    mem.save_creations_last_revision(-10);
    disk.save_creations_last_revision(-10);
    assert_parity("monotonic ignore -10", 15, Some("15"));

    mem.set_cache_meta("creations_last_revision", "42");
    disk.set_cache_meta("creations_last_revision", "42");
    assert_parity("set_cache_meta 42", 42, Some("42"));

    mem.set_cache_meta("creations_last_revision", " 55 ");
    disk.set_cache_meta("creations_last_revision", " 55 ");
    assert_parity("set_cache_meta padded 55", 55, Some(" 55 "));

    mem.set_cache_meta("creations_last_revision", "-9");
    disk.set_cache_meta("creations_last_revision", "-9");
    assert_parity("set_cache_meta negative", 0, Some("-9"));

    mem.set_cache_meta("creations_last_revision", "not_a_number");
    disk.set_cache_meta("creations_last_revision", "not_a_number");
    assert_parity("set_cache_meta invalid", 0, Some("not_a_number"));

    mem.delete_cache_meta("creations_last_revision");
    disk.delete_cache_meta("creations_last_revision");
    assert_parity("delete_cache_meta", 0, None);

    let item_rev_24 = sample_creation(
        "cr_delta_24",
        1001,
        CreationKind::Book,
        CreationReviewStatus::Draft,
        24,
        9_000,
        None,
    );
    let r_mem = mem.apply_creations_sync_delta(20, &[item_rev_24.clone()]);
    let r_disk = disk.apply_creations_sync_delta(20, &[item_rev_24]);
    assert_eq!(r_mem, 24);
    assert_eq!(r_disk, 24);
    assert_parity("apply_creations_sync_delta 20/item 24", 24, Some("24"));

    mem.set_cache_meta("creations_last_revision", "10");
    disk.set_cache_meta("creations_last_revision", "10");
    mem.save_creations_last_revision(12);
    disk.save_creations_last_revision(12);
    assert_parity("lower via meta then save 12", 12, Some("12"));

    mem.clear_all();
    disk.clear_all();
    assert_parity("clear_all", 0, None);

    let r_disk2 = disk.apply_creations_sync_delta(33, &[]);
    assert_eq!(r_disk2, 33);
    drop(disk);
    let reopened = LocalStore::open_with_dir(dir.path()).unwrap();
    assert_eq!(reopened.creations_last_revision(), 33);
    assert_eq!(
        reopened.get_cache_meta("creations_last_revision").as_deref(),
        Some("33")
    );
}

#[test]
fn f9_admin_unpublish_unlisted_decision_and_sync_delta_evict_catalog_without_foreign_pollution() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let addr = rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let (method, path, body_bytes) = read_http_request(&mut stream).await;
                let base_path = path.split('?').next().unwrap_or(&path).to_string();
                let body_str = String::from_utf8_lossy(&body_bytes).to_string();

                let (status, reason, resp_body) = match (method.as_str(), base_path.as_str()) {
                    ("POST", "/api/v1/creations/cr_alice_pub/unpublish") => {
                        let mut dto = sample_creation(
                            "cr_alice_pub",
                            1001,
                            CreationKind::Book,
                            CreationReviewStatus::Unpublished,
                            7,
                            10_000,
                            None,
                        );
                        dto.visibility = CreationVisibility::Private;
                        dto.has_live_published_version = false;
                        (200, "OK", serde_json::to_string(&dto).unwrap())
                    }
                    ("POST", "/api/v1/creations/cr_alice_unlisted/review") => {
                        let mut dto = sample_creation(
                            "cr_alice_unlisted",
                            1001,
                            CreationKind::Book,
                            CreationReviewStatus::Published,
                            8,
                            10_100,
                            None,
                        );
                        dto.visibility = CreationVisibility::Unlisted;
                        dto.has_live_published_version = true;
                        (200, "OK", serde_json::to_string(&dto).unwrap())
                    }
                    ("POST", "/api/v1/creations/cr_alice_reject_unlive/review") => {
                        let mut dto = sample_creation(
                            "cr_alice_reject_unlive",
                            1001,
                            CreationKind::Book,
                            CreationReviewStatus::Rejected,
                            9,
                            10_200,
                            None,
                        );
                        dto.visibility = CreationVisibility::Private;
                        dto.has_live_published_version = false;
                        (200, "OK", serde_json::to_string(&dto).unwrap())
                    }
                    ("POST", "/api/v1/creations/cr_alice_reject_live_v1/review") => {
                        let mut dto = sample_creation(
                            "cr_alice_reject_live_v1",
                            1001,
                            CreationKind::Book,
                            CreationReviewStatus::Rejected,
                            10,
                            10_300,
                            None,
                        );
                        dto.visibility = CreationVisibility::Public;
                        dto.has_live_published_version = true;
                        (200, "OK", serde_json::to_string(&dto).unwrap())
                    }
                    ("GET", "/api/v1/creations/sync") => {
                        if body_str.is_empty() && path.contains("since_revision=0") {
                            let resp = CreationSyncResponse {
                                latest_revision: 0,
                                items: vec![],
                            };
                            (200, "OK", serde_json::to_string(&resp).unwrap())
                        } else {
                            let mut cr_del = sample_creation(
                                "cr_sync_del",
                                1001,
                                CreationKind::Book,
                                CreationReviewStatus::Unpublished,
                                11,
                                11_000,
                                Some(11_000),
                            );
                            cr_del.has_live_published_version = false;
                            cr_del.visibility = CreationVisibility::Private;

                            let mut cr_unpub = sample_creation(
                                "cr_sync_unpub",
                                1001,
                                CreationKind::Book,
                                CreationReviewStatus::Unpublished,
                                12,
                                11_100,
                                None,
                            );
                            cr_unpub.has_live_published_version = false;
                            cr_unpub.visibility = CreationVisibility::Private;

                            let mut cr_unlisted = sample_creation(
                                "cr_sync_unlisted",
                                1001,
                                CreationKind::Book,
                                CreationReviewStatus::Published,
                                13,
                                11_200,
                                None,
                            );
                            cr_unlisted.has_live_published_version = true;
                            cr_unlisted.visibility = CreationVisibility::Unlisted;

                            let cr_live = sample_creation(
                                "cr_sync_live",
                                1001,
                                CreationKind::Book,
                                CreationReviewStatus::Published,
                                14,
                                11_300,
                                None,
                            );

                            let resp = CreationSyncResponse {
                                latest_revision: 14,
                                items: vec![cr_del, cr_unpub, cr_unlisted, cr_live],
                            };
                            (200, "OK", serde_json::to_string(&resp).unwrap())
                        }
                    }
                    _ => (404, "Not Found", r#"{"error":"not_found"}"#.to_string()),
                };
                write_json_response(&mut stream, status, reason, &resp_body).await;
            }
        });
        addr
    });

    // Part 1 (F9-1 & F9-2): Admin unpublish + Approve(Unlisted) + Reject(unlive vs live-v1)
    let admin_dir = tempfile::tempdir().unwrap();
    let cfg = ClientConfig {
        server_url: format!("http://{addr}"),
        ..Default::default()
    };
    {
        let admin = TelegramClient::open_with_dir(cfg.clone(), admin_dir.path()).unwrap();
        admin.set_auth_session(auth_session(9001, "admin", "tok_admin"));
        admin.local_store.upsert_public_catalog_items(&[
            sample_catalog_item("cr_alice_pub", CreationKind::Book, Some(1), 9_000),
            sample_catalog_item("cr_alice_unlisted", CreationKind::Book, Some(2), 9_100),
            sample_catalog_item("cr_alice_reject_unlive", CreationKind::Book, Some(3), 9_200),
            sample_catalog_item("cr_alice_reject_live_v1", CreationKind::Book, Some(4), 9_300),
        ]);

        rt.block_on(async {
            // F9-1: Admin unpublishes Alice's creation -> evicts catalog, NEVER pollutes admin cache
            let unpub = admin.unpublish_creation("cr_alice_pub").await.unwrap();
            assert_eq!(unpub.owner_user_id, 1001);
            assert!(
                admin.load_cached_my_creations(None, None).is_empty(),
                "admin unpublish must never insert foreign creation into user_creations_cache"
            );
            assert!(
                admin.sync_my_creations_with_cache().await.unwrap().is_empty(),
                "admin sync_my_creations_with_cache must remain free of foreign creations"
            );
            assert!(
                !admin
                    .load_cached_public_catalog(Some(CreationKind::Book))
                    .iter()
                    .any(|c| c.creation_id == "cr_alice_pub"),
                "unpublish must evict public_catalog_cache"
            );

            // F9-2: Approve(Unlisted) evicts public_catalog_cache immediately without WS
            admin
                .submit_review_decision(
                    "cr_alice_unlisted",
                    &ReviewDecisionRequest {
                        decision: ReviewDecision::Approve,
                        visibility: Some(CreationVisibility::Unlisted),
                        reason: None,
                        notes: None,
                    },
                )
                .await
                .unwrap();
            assert!(
                !admin
                    .load_cached_public_catalog(Some(CreationKind::Book))
                    .iter()
                    .any(|c| c.creation_id == "cr_alice_unlisted"),
                "Approve(Unlisted) must evict item from public_catalog_cache"
            );
            assert!(admin.load_cached_my_creations(None, None).is_empty());

            // Reject without live published version evicts stale catalog item
            admin
                .submit_review_decision(
                    "cr_alice_reject_unlive",
                    &ReviewDecisionRequest {
                        decision: ReviewDecision::Reject,
                        visibility: None,
                        reason: Some("needs edits".to_string()),
                        notes: None,
                    },
                )
                .await
                .unwrap();
            assert!(
                !admin
                    .load_cached_public_catalog(Some(CreationKind::Book))
                    .iter()
                    .any(|c| c.creation_id == "cr_alice_reject_unlive")
            );

            // Reject of v2 when live Public v1 remains active keeps live v1 in catalog cache
            admin
                .submit_review_decision(
                    "cr_alice_reject_live_v1",
                    &ReviewDecisionRequest {
                        decision: ReviewDecision::Reject,
                        visibility: None,
                        reason: Some("v2 needs edits".to_string()),
                        notes: None,
                    },
                )
                .await
                .unwrap();
            assert!(
                admin
                    .load_cached_public_catalog(Some(CreationKind::Book))
                    .iter()
                    .any(|c| c.creation_id == "cr_alice_reject_live_v1")
            );
        });
    }

    // Reopen admin store from disk and verify zero pollution persisted
    let admin_reopened = TelegramClient::open_with_dir(cfg.clone(), admin_dir.path()).unwrap();
    admin_reopened.set_auth_session(auth_session(9001, "admin", "tok_admin"));
    assert!(admin_reopened.load_cached_my_creations(None, None).is_empty());
    let remaining_cat: Vec<String> = admin_reopened
        .load_cached_public_catalog(Some(CreationKind::Book))
        .into_iter()
        .map(|c| c.creation_id)
        .collect();
    assert_eq!(remaining_cat, vec!["cr_alice_reject_live_v1".to_string()]);

    // Part 2 (F9-4): Sync delta evicts deleted, unpublished, and non-public items from public_catalog_cache
    let alice_dir = tempfile::tempdir().unwrap();
    let alice = TelegramClient::open_with_dir(cfg, alice_dir.path()).unwrap();
    alice.set_auth_session(auth_session(1001, "alice", "tok_alice"));
    alice.local_store.save_creations_last_revision(1);
    alice.local_store.upsert_public_catalog_items(&[
        sample_catalog_item("cr_sync_del", CreationKind::Book, Some(1), 10_000),
        sample_catalog_item("cr_sync_unpub", CreationKind::Book, Some(2), 10_100),
        sample_catalog_item("cr_sync_unlisted", CreationKind::Book, Some(3), 10_200),
        sample_catalog_item("cr_sync_live", CreationKind::Book, Some(4), 10_300),
    ]);

    rt.block_on(async {
        let synced = alice.sync_my_creations_with_cache().await.unwrap();
        assert_eq!(synced.len(), 3, "cr_sync_del tombstoned; 3 non-deleted remain");
    });
    let surviving_cat: Vec<String> = alice
        .load_cached_public_catalog(Some(CreationKind::Book))
        .into_iter()
        .map(|c| c.creation_id)
        .collect();
    assert_eq!(surviving_cat, vec!["cr_sync_live".to_string()]);
}


