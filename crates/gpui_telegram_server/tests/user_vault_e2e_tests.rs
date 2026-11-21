use argon2::{Algorithm, Argon2, Params as Argon2Params, Version};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Key, Nonce,
};
use futures::StreamExt;
use gpui_telegram_agent::fabushi_bot::OFFICIAL_DEFAULT_BOT_USER_ID;
use gpui_telegram_client::{
    ApproveDeviceSyncRequest, Argon2KdfParamsDto, ChangePasswordRequest, ClientConfig,
    DeviceSyncRequestBody, HttpClientError, LoginRequest, RegisterRequest,
    SplitKeyEmailRegisterRequest, SplitKeyLoginRequest, SplitKeyRegisterRequest, TelegramClient,
    UpgradeSplitKeyRequest,
};
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::contracts::SendMessageHttpRequest as RichSendMessageRequest;
use gpui_telegram_proto::message::MessageContent;
use gpui_telegram_proto::miniapp_contracts::{CreateFabushiBotRequest, FabushiBotCreateResponse};
use gpui_telegram_proto::user_vault::{
    ArchiveLocalBotReplyRequest, UpsertUserVaultItemRequest, UserVaultKeyEnvelopeDto,
    DEVICE_SYNC_CODE_DOMAIN_V1, USER_VAULT_CATEGORY_AI_OAUTH, USER_VAULT_CATEGORY_BROWSER_COOKIE,
    USER_VAULT_CATEGORY_PASSWORD, USER_VAULT_KDF_ALG_DEVICE_KEYCHAIN_V2,
    USER_VAULT_KDF_ALG_SPLIT_KEY_V2,
};
use gpui_telegram_proto::ws::WsServerFrame;
use gpui_telegram_server::{build_router_with_state, config::ServerConfig, server::AppState};
use hkdf::Hkdf;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tower::ServiceExt;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret as X25519StaticSecret};

fn boot_in_memory_server() -> (TempDir, Arc<AppState>, Router) {
    let dir = TempDir::new().expect("create TempDir");
    let mut cfg = ServerConfig::default();
    cfg.database_url = ":memory:".to_string();
    cfg.media_storage_path = dir.path().join("cas").to_string_lossy().into_owned();
    cfg.host = "127.0.0.1".to_string();
    cfg.port = 0;
    let state = Arc::new(AppState::try_new(cfg).expect("boot in-memory AppState"));
    let router = build_router_with_state(state.clone());
    (dir, state, router)
}

async fn api(
    router: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let res = router.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let val = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, val)
}

async fn register_user(router: &Router, username: &str) -> (u64, String) {
    let (status, val) = api(
        router,
        Method::POST,
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": username,
            "password": "StrongPassword!123",
            "first_name": username,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register failed: {val}");
    (
        val["user_id"].as_u64().unwrap(),
        val["token"].as_str().unwrap().to_string(),
    )
}

fn sample_envelope(salt_byte: u8, wrap_byte: u8) -> UserVaultKeyEnvelopeDto {
    UserVaultKeyEnvelopeDto {
        key_version: 1,
        kdf_alg: "argon2id".to_string(),
        kdf_salt_b64: B64.encode([salt_byte; 16]),
        kdf_m_cost_kib: 1024,
        kdf_t_cost: 1,
        kdf_p_cost: 1,
        wrapped_root_key_b64: B64.encode([wrap_byte; 60]),
        recovery_wrapped_root_key_b64: Some(B64.encode([wrap_byte.wrapping_add(1); 60])),
        updated_at_ms: 0,
    }
}

#[tokio::test]
async fn test_user_vault_envelope_isolation_and_validation() {
    let (_dir, state, router) = boot_in_memory_server();

    // Unauthenticated access must fail with 401
    let (st_unauth, _) = api(&router, Method::GET, "/api/v1/user-vault/envelope", None, None).await;
    assert_eq!(st_unauth, StatusCode::UNAUTHORIZED);

    let (user_a_id, tok_a) = register_user(&router, "vault_alice").await;
    let (user_b_id, tok_b) = register_user(&router, "vault_bob").await;

    // Initial fetch returns envelope: None
    let (st, v) = api(&router, Method::GET, "/api/v1/user-vault/envelope", Some(&tok_a), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(v["envelope"].is_null());

    // Invalid KDF algorithm rejected with 400
    let mut bad_env = sample_envelope(0x11, 0x22);
    bad_env.kdf_alg = "plaintext".to_string();
    let (st_bad_alg, _) = api(
        &router,
        Method::PUT,
        "/api/v1/user-vault/envelope",
        Some(&tok_a),
        Some(serde_json::to_value(&bad_env).unwrap()),
    )
    .await;
    assert_eq!(st_bad_alg, StatusCode::BAD_REQUEST);

    // Invalid base64 rejected with 400
    let mut bad_b64 = sample_envelope(0x11, 0x22);
    bad_b64.wrapped_root_key_b64 = "not!valid!base64".to_string();
    let (st_bad_b64, _) = api(
        &router,
        Method::PUT,
        "/api/v1/user-vault/envelope",
        Some(&tok_a),
        Some(serde_json::to_value(&bad_b64).unwrap()),
    )
    .await;
    assert_eq!(st_bad_b64, StatusCode::BAD_REQUEST);

    // User A initializes envelope
    let env_a = sample_envelope(0xAA, 0xBB);
    let (st_put_a, v_put_a) = api(
        &router,
        Method::PUT,
        "/api/v1/user-vault/envelope",
        Some(&tok_a),
        Some(serde_json::to_value(&env_a).unwrap()),
    )
    .await;
    assert_eq!(st_put_a, StatusCode::OK);
    assert_eq!(v_put_a["envelope"]["wrapped_root_key_b64"], json!(env_a.wrapped_root_key_b64));
    assert!(v_put_a["envelope"]["updated_at_ms"].as_i64().unwrap() > 0);

    // User B still sees envelope: None
    let (st_get_b, v_get_b) = api(&router, Method::GET, "/api/v1/user-vault/envelope", Some(&tok_b), None).await;
    assert_eq!(st_get_b, StatusCode::OK);
    assert!(v_get_b["envelope"].is_null());

    // User B writes its own envelope -> does not affect User A
    let env_b = sample_envelope(0xCC, 0xDD);
    let (st_put_b, _) = api(
        &router,
        Method::PUT,
        "/api/v1/user-vault/envelope",
        Some(&tok_b),
        Some(serde_json::to_value(&env_b).unwrap()),
    )
    .await;
    assert_eq!(st_put_b, StatusCode::OK);

    let (st_get_a, v_get_a) = api(&router, Method::GET, "/api/v1/user-vault/envelope", Some(&tok_a), None).await;
    assert_eq!(st_get_a, StatusCode::OK);
    assert_eq!(v_get_a["envelope"]["wrapped_root_key_b64"], json!(env_a.wrapped_root_key_b64));

    let stored_a = state.db.get_user_vault_envelope(UserId(user_a_id)).unwrap().unwrap();
    let stored_b = state.db.get_user_vault_envelope(UserId(user_b_id)).unwrap().unwrap();
    assert_ne!(stored_a.wrapped_root_key_b64, stored_b.wrapped_root_key_b64);
}

#[tokio::test]
async fn test_user_vault_items_categories_revisions_conflicts_and_tombstones() {
    let (_dir, _state, router) = boot_in_memory_server();
    let (_user_a_id, tok_a) = register_user(&router, "sync_alice").await;
    let (_user_b_id, tok_b) = register_user(&router, "sync_bob").await;

    // Validation guards
    let valid_req = UpsertUserVaultItemRequest {
        expected_revision: None,
        nonce_b64: B64.encode([1u8; 12]),
        ciphertext_b64: B64.encode(b"opaque-ciphertext-bytes"),
        deleted: false,
    };
    let (st_bad_cat, _) = api(
        &router,
        Method::PUT,
        "/api/v1/user-vault/items/credit_card/card_1",
        Some(&tok_a),
        Some(serde_json::to_value(&valid_req).unwrap()),
    )
    .await;
    assert_eq!(st_bad_cat, StatusCode::BAD_REQUEST);

    let (st_bad_key, _) = api(
        &router,
        Method::PUT,
        "/api/v1/user-vault/items/password/..evil",
        Some(&tok_a),
        Some(serde_json::to_value(&valid_req).unwrap()),
    )
    .await;
    assert_eq!(st_bad_key, StatusCode::BAD_REQUEST);

    // Put items across all 3 categories -> monotonic revisions 1, 2, 3
    let (st1, r1) = api(
        &router,
        Method::PUT,
        &format!("/api/v1/user-vault/items/{USER_VAULT_CATEGORY_PASSWORD}/cred_alpha"),
        Some(&tok_a),
        Some(serde_json::to_value(&UpsertUserVaultItemRequest {
            expected_revision: Some(0),
            nonce_b64: B64.encode([0x10; 12]),
            ciphertext_b64: B64.encode(b"cipher-password-1"),
            deleted: false,
        }).unwrap()),
    )
    .await;
    assert_eq!(st1, StatusCode::OK);
    assert_eq!(r1["revision"].as_i64(), Some(1));

    let (st2, r2) = api(
        &router,
        Method::PUT,
        &format!("/api/v1/user-vault/items/{USER_VAULT_CATEGORY_BROWSER_COOKIE}/profile_main"),
        Some(&tok_a),
        Some(serde_json::to_value(&UpsertUserVaultItemRequest {
            expected_revision: None,
            nonce_b64: B64.encode([0x20; 12]),
            ciphertext_b64: B64.encode(b"cipher-cookie-2"),
            deleted: false,
        }).unwrap()),
    )
    .await;
    assert_eq!(st2, StatusCode::OK);
    assert_eq!(r2["revision"].as_i64(), Some(2));

    let (st3, r3) = api(
        &router,
        Method::PUT,
        &format!("/api/v1/user-vault/items/{USER_VAULT_CATEGORY_AI_OAUTH}/subscriptions_v1"),
        Some(&tok_a),
        Some(serde_json::to_value(&UpsertUserVaultItemRequest {
            expected_revision: Some(0),
            nonce_b64: B64.encode([0x30; 12]),
            ciphertext_b64: B64.encode(b"cipher-oauth-3"),
            deleted: false,
        }).unwrap()),
    )
    .await;
    assert_eq!(st3, StatusCode::OK);
    assert_eq!(r3["revision"].as_i64(), Some(3));

    // Incremental since_revision pull + category filter
    let (st_pull_1, pull_1) = api(
        &router,
        Method::GET,
        "/api/v1/user-vault/items?since_revision=1",
        Some(&tok_a),
        None,
    )
    .await;
    assert_eq!(st_pull_1, StatusCode::OK);
    assert_eq!(pull_1["latest_revision"].as_i64(), Some(3));
    assert_eq!(pull_1["items"].as_array().unwrap().len(), 2);
    assert_eq!(pull_1["items"][0]["revision"].as_i64(), Some(2));
    assert_eq!(pull_1["items"][1]["revision"].as_i64(), Some(3));

    let (st_pull_cat, pull_cat) = api(
        &router,
        Method::GET,
        "/api/v1/user-vault/items?since_revision=0&category=ai_oauth",
        Some(&tok_a),
        None,
    )
    .await;
    assert_eq!(st_pull_cat, StatusCode::OK);
    assert_eq!(pull_cat["items"].as_array().unwrap().len(), 1);
    assert_eq!(pull_cat["items"][0]["item_key"], json!("subscriptions_v1"));

    // Optimistic concurrency 409 Conflict
    let (st_conflict, conflict_body) = api(
        &router,
        Method::PUT,
        &format!("/api/v1/user-vault/items/{USER_VAULT_CATEGORY_PASSWORD}/cred_alpha"),
        Some(&tok_a),
        Some(serde_json::to_value(&UpsertUserVaultItemRequest {
            expected_revision: Some(0),
            nonce_b64: B64.encode([0x11; 12]),
            ciphertext_b64: B64.encode(b"stale-write"),
            deleted: false,
        }).unwrap()),
    )
    .await;
    assert_eq!(st_conflict, StatusCode::CONFLICT);
    assert_eq!(conflict_body["error"], json!("revision_conflict"));

    // User B isolation: sees empty items and cannot overwrite User A
    let (st_b_pull, pull_b) = api(
        &router,
        Method::GET,
        "/api/v1/user-vault/items",
        Some(&tok_b),
        None,
    )
    .await;
    assert_eq!(st_b_pull, StatusCode::OK);
    assert_eq!(pull_b["latest_revision"].as_i64(), Some(0));
    assert!(pull_b["items"].as_array().unwrap().is_empty());

    let (st_b_put, put_b) = api(
        &router,
        Method::PUT,
        &format!("/api/v1/user-vault/items/{USER_VAULT_CATEGORY_PASSWORD}/cred_alpha"),
        Some(&tok_b),
        Some(serde_json::to_value(&UpsertUserVaultItemRequest {
            expected_revision: Some(0),
            nonce_b64: B64.encode([0x40; 12]),
            ciphertext_b64: B64.encode(b"bob-cipher"),
            deleted: false,
        }).unwrap()),
    )
    .await;
    assert_eq!(st_b_put, StatusCode::OK);
    assert_eq!(put_b["revision"].as_i64(), Some(1));

    // DELETE conflict on wrong expected_revision, then valid DELETE tombstone
    let (st_del_conflict, _) = api(
        &router,
        Method::DELETE,
        "/api/v1/user-vault/items/password/cred_alpha?expected_revision=99",
        Some(&tok_a),
        None,
    )
    .await;
    assert_eq!(st_del_conflict, StatusCode::CONFLICT);

    let (st_del, del_body) = api(
        &router,
        Method::DELETE,
        "/api/v1/user-vault/items/password/cred_alpha?expected_revision=1",
        Some(&tok_a),
        None,
    )
    .await;
    assert_eq!(st_del, StatusCode::OK);
    assert_eq!(del_body["revision"].as_i64(), Some(4));

    let (st_pull_tomb, pull_tomb) = api(
        &router,
        Method::GET,
        "/api/v1/user-vault/items?since_revision=3",
        Some(&tok_a),
        None,
    )
    .await;
    assert_eq!(st_pull_tomb, StatusCode::OK);
    assert_eq!(pull_tomb["latest_revision"].as_i64(), Some(4));
    assert_eq!(pull_tomb["items"].as_array().unwrap().len(), 1);
    assert_eq!(pull_tomb["items"][0]["deleted"], json!(true));
    assert_eq!(pull_tomb["items"][0]["item_key"], json!("cred_alpha"));
}

#[tokio::test]
async fn test_skip_server_bot_reply_and_archive_local_bot_reply_e2e() {
    let (_dir, _state, router) = boot_in_memory_server();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let base_url = format!("http://{addr}");

    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url.clone();
    let client_a = TelegramClient::new(cfg.clone());
    let client_b = TelegramClient::new(cfg);

    let auth_a = client_a
        .http_client
        .register(&gpui_telegram_client::RegisterRequest {
            username: "local_bot_alice".to_string(),
            password: "StrongPassword!123".to_string(),
            first_name: "Alice".to_string(),
            device_name: "DesktopA".to_string(),
        })
        .await
        .unwrap();
    client_a.set_auth_session(auth_a.clone());

    let auth_b = client_b
        .http_client
        .register(&gpui_telegram_client::RegisterRequest {
            username: "local_bot_bob".to_string(),
            password: "StrongPassword!123".to_string(),
            first_name: "Bob".to_string(),
            device_name: "DesktopB".to_string(),
        })
        .await
        .unwrap();
    client_b.set_auth_session(auth_b.clone());

    let welcome = client_a
        .http_client
        .provision_welcome_chat(&auth_a.token)
        .await
        .unwrap();
    let welcome_chat_id = welcome.chat_id;

    // Connect WebSocket for Alice to verify broadcast PtsPush on archive_local_bot_reply
    let ws_url = format!("ws://{addr}/api/v1/ws?token={}", auth_a.token);
    let (mut ws_stream, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    let history_before = client_a
        .get_chat_messages(welcome_chat_id as u64, Some(50))
        .await
        .unwrap();
    let count_before = history_before.len();

    // 1. Send message with skip_server_bot_reply = true -> must NOT trigger server bot reply
    let send_req = RichSendMessageRequest {
        chat_id: welcome_chat_id,
        text: Some("/help".to_string()),
        content: Some(MessageContent::plain_text("/help")),
        entities: Vec::new(),
        reply_to: None,
        quote_text: None,
        topic_id: None,
        client_nonce: Some(client_a.next_client_nonce()),
        forward_from_chat_id: None,
        forward_from_msg_id: None,
        silent: false,
        scheduled_at_ms: None,
        inline_keyboard: None,
        send_as_peer_id: None,
        skip_server_bot_reply: true,
    };
    let send_resp = client_a.send_message_durable(send_req).await.unwrap();

    tokio::time::sleep(Duration::from_millis(180)).await;
    let history_after_user_send = client_a
        .get_chat_messages(welcome_chat_id as u64, Some(50))
        .await
        .unwrap();
    assert_eq!(
        history_after_user_send.len(),
        count_before + 1,
        "skip_server_bot_reply=true must prevent automatic server bot reply"
    );
    assert_eq!(history_after_user_send.last().unwrap().text, "/help");

    // 2. Unauthorized user B cannot archive into Alice's chat (403)
    let bob_err = client_b
        .archive_local_bot_reply(
            welcome_chat_id,
            &ArchiveLocalBotReplyRequest {
                bot_user_id: OFFICIAL_DEFAULT_BOT_USER_ID,
                text: "spoofed reply".to_string(),
                reply_to_msg_id: Some(send_resp.message_id),
                client_nonce: Some(900_001),
            },
        )
        .await;
    assert!(bob_err.is_err());

    // 3. Cannot spoof a human user ID as bot_user_id (403)
    let human_spoof_err = client_a
        .archive_local_bot_reply(
            welcome_chat_id,
            &ArchiveLocalBotReplyRequest {
                bot_user_id: auth_b.user_id as i64,
                text: "spoofed human".to_string(),
                reply_to_msg_id: None,
                client_nonce: Some(900_002),
            },
        )
        .await;
    assert!(human_spoof_err.is_err());

    // 4. Alice archives local Fabushi Bot reply -> persisted & broadcast over WebSocket
    let archive_resp = client_a
        .archive_local_bot_reply(
            welcome_chat_id,
            &ArchiveLocalBotReplyRequest {
                bot_user_id: OFFICIAL_DEFAULT_BOT_USER_ID,
                text: "Local Obscura task finished safely without leaking secrets.".to_string(),
                reply_to_msg_id: Some(send_resp.message_id),
                client_nonce: Some(900_003),
            },
        )
        .await
        .unwrap();
    assert!(archive_resp.message_id > send_resp.message_id);
    assert!(archive_resp.pts > send_resp.pts);

    // Wait for WS PtsPush carrying the archived bot reply
    let mut saw_bot_ws_push = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(250), ws_stream.next()).await {
            Ok(Some(Ok(WsMessage::Text(txt)))) => {
                if let Ok(WsServerFrame::PtsPush(pts)) = serde_json::from_str::<WsServerFrame>(&txt) {
                    if let gpui_telegram_proto::sync::UpdateEvent::NewMessage(msg) = pts.event {
                        if msg.id.0 == archive_resp.message_id {
                            assert_eq!(msg.sender_id.0, OFFICIAL_DEFAULT_BOT_USER_ID as u64);
                            saw_bot_ws_push = true;
                            break;
                        }
                    }
                }
            }
            Ok(Some(Ok(_))) => continue,
            _ => break,
        }
    }
    assert!(saw_bot_ws_push, "archive_local_bot_reply must broadcast PtsPush over WebSocket");

    let history_final = client_a
        .get_chat_messages(welcome_chat_id as u64, Some(50))
        .await
        .unwrap();
    assert_eq!(history_final.len(), count_before + 2);
    let last_msg = history_final.last().unwrap();
    assert_eq!(last_msg.sender_id, OFFICIAL_DEFAULT_BOT_USER_ID as u64);
    assert_eq!(
        last_msg.text,
        "Local Obscura task finished safely without leaking secrets."
    );

    // 5. Alice creates her own custom Fabushi Bot and cannot have Bob use it, while Alice can archive with it
    let http = reqwest::Client::new();
    let create_bot_res = http
        .post(format!("{base_url}/api/v1/fabushi-bots"))
        .bearer_auth(&auth_a.token)
        .json(&CreateFabushiBotRequest {
            display_name: "Alice Vault Bot".to_string(),
            username: Some("alice_local_vault_bot".to_string()),
            agent_id: None,
            description: "Local assistant".to_string(),
            title: "Alice Local Vault Bot".to_string(),
            avatar_shape: None,
            avatar_color: None,
            preferred_model: None,
            thinking_level: None,
            approval_mode: None,
            toolset_preset: None,
            coding_agent: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(create_bot_res.status(), StatusCode::CREATED);
    let created_bot: FabushiBotCreateResponse = create_bot_res.json().await.unwrap();

    let custom_arch = client_a
        .archive_local_bot_reply(
            created_bot.companion_chat_id,
            &ArchiveLocalBotReplyRequest {
                bot_user_id: created_bot.bot.bot_user_id,
                text: "Custom Fabushi local reply".to_string(),
                reply_to_msg_id: None,
                client_nonce: Some(900_004),
            },
        )
        .await
        .unwrap();
    assert!(custom_arch.message_id > 0);

    // Bob cannot archive Alice's private bot into Bob's welcome chat
    let bob_welcome = client_b
        .http_client
        .provision_welcome_chat(&auth_b.token)
        .await
        .unwrap();
    let bob_private_bot_err = client_b
        .archive_local_bot_reply(
            bob_welcome.chat_id,
            &ArchiveLocalBotReplyRequest {
                bot_user_id: created_bot.bot.bot_user_id,
                text: "stolen bot reply".to_string(),
                reply_to_msg_id: None,
                client_nonce: Some(900_005),
            },
        )
        .await;
    assert!(bob_private_bot_err.is_err());

    // 6. Test TelegramClient E2EE Vault HTTP methods end-to-end over TCP
    assert!(client_a.get_user_vault_envelope().await.unwrap().envelope.is_none());
    let env_a = sample_envelope(0x55, 0x66);
    let saved_env = client_a.put_user_vault_envelope(&env_a).await.unwrap().envelope.unwrap();
    assert_eq!(saved_env.wrapped_root_key_b64, env_a.wrapped_root_key_b64);

    let p1 = client_a
        .put_user_vault_item(
            USER_VAULT_CATEGORY_PASSWORD,
            "cred_client_1",
            &UpsertUserVaultItemRequest {
                expected_revision: Some(0),
                nonce_b64: B64.encode([0x77; 12]),
                ciphertext_b64: B64.encode(b"client-vault-cipher"),
                deleted: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(p1.revision, 1);

    let pulled = client_a.pull_user_vault_items(Some(0), None).await.unwrap();
    assert_eq!(pulled.latest_revision, 1);
    assert_eq!(pulled.items.len(), 1);

    let del = client_a
        .delete_user_vault_item(USER_VAULT_CATEGORY_PASSWORD, "cred_client_1", Some(1))
        .await
        .unwrap();
    assert_eq!(del.revision, 2);

    client_a.clear_session();
    client_b.clear_session();
    tokio::task::spawn_blocking(move || drop((client_a, client_b)))
        .await
        .unwrap();
    server_handle.abort();
}

fn test_fast_kdf() -> Argon2KdfParamsDto {
    Argon2KdfParamsDto {
        m_cost_kib: 1_024,
        t_cost: 1,
        p_cost: 1,
    }
}

fn test_derive_split_key(
    password: &str,
    salt_b64: &str,
    params: Argon2KdfParamsDto,
) -> (String, [u8; 32]) {
    let salt = B64.decode(salt_b64).unwrap();
    let a2_params = Argon2Params::new(params.m_cost_kib, params.t_cost, params.p_cost, Some(32)).unwrap();
    let a2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, a2_params);
    let mut master = [0u8; 32];
    a2.hash_password_into(password.as_bytes(), &salt, &mut master).unwrap();

    let hk = Hkdf::<Sha256>::new(None, &master);
    let mut auth_verifier = [0u8; 32];
    hk.expand(b"gpui-telegram/auth-verifier/v2", &mut auth_verifier).unwrap();
    let mut vault_kek = [0u8; 32];
    hk.expand(b"gpui-telegram/vault-kek/v2", &mut vault_kek).unwrap();
    (B64.encode(auth_verifier), vault_kek)
}

fn test_wrap_key_with_aad(wrap_key: &[u8; 32], root_key: &[u8; 32], aad: &str, nonce_byte: u8) -> String {
    let nonce = [nonce_byte; 12];
    let cipher = ChaCha20Poly1305::new(Key::from_slice(wrap_key));
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: root_key,
                aad: aad.as_bytes(),
            },
        )
        .unwrap();
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    B64.encode(out)
}

fn test_unwrap_key_with_aad(wrap_key: &[u8; 32], wrapped_b64: &str, aad: &str) -> [u8; 32] {
    let raw = B64.decode(wrapped_b64).unwrap();
    let (nonce, ct) = raw.split_at(12);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(wrap_key));
    let pt = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ct,
                aad: aad.as_bytes(),
            },
        )
        .unwrap();
    let mut out = [0u8; 32];
    out.copy_from_slice(&pt);
    out
}

fn test_seal_item(root_key: &[u8; 32], user_id: u64, cat: &str, item_key: &str, rev: i64, pt: &[u8]) -> (String, String) {
    let hk = Hkdf::<Sha256>::new(Some(b"gpui-user-vault-item-hkdf-v1"), root_key);
    let info = format!("gpui-user-vault:subkey:v1|{user_id}|{cat}|{item_key}");
    let mut subkey = [0u8; 32];
    hk.expand(info.as_bytes(), &mut subkey).unwrap();
    let aad = format!("gpui-user-vault:item:v1|{user_id}|{cat}|{item_key}|{rev}");
    let nonce = [0x42u8; 12];
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&subkey));
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: pt, aad: aad.as_bytes() })
        .unwrap();
    (B64.encode(nonce), B64.encode(ct))
}

fn test_open_item(root_key: &[u8; 32], user_id: u64, cat: &str, item_key: &str, rev: i64, nonce_b64: &str, ct_b64: &str) -> Vec<u8> {
    let hk = Hkdf::<Sha256>::new(Some(b"gpui-user-vault-item-hkdf-v1"), root_key);
    let info = format!("gpui-user-vault:subkey:v1|{user_id}|{cat}|{item_key}");
    let mut subkey = [0u8; 32];
    hk.expand(info.as_bytes(), &mut subkey).unwrap();
    let aad = format!("gpui-user-vault:item:v1|{user_id}|{cat}|{item_key}|{rev}");
    let nonce = B64.decode(nonce_b64).unwrap();
    let ct = B64.decode(ct_b64).unwrap();
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&subkey));
    cipher
        .decrypt(Nonce::from_slice(&nonce), Payload { msg: &ct, aad: aad.as_bytes() })
        .unwrap()
}

#[tokio::test]
async fn test_split_key_v2_register_login_email_and_fake_salt_e2e() {
    let (_dir, state, router) = boot_in_memory_server();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let base_url = format!("http://{addr}");

    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url.clone();
    let client_a = TelegramClient::new(cfg.clone());
    let client_b = TelegramClient::new(cfg.clone());
    let client_email = TelegramClient::new(cfg);

    // 1. Missing-user deterministic fake salt (zero account-enumeration side channel)
    let fake_1 = client_a.get_auth_kdf_params("nonexistent_alice").await.unwrap();
    let fake_2 = client_a.get_auth_kdf_params("  NONEXISTENT_ALICE ").await.unwrap();
    let fake_other = client_a.get_auth_kdf_params("nonexistent_bob").await.unwrap();
    assert_eq!(fake_1.auth_version, 2);
    assert_eq!(fake_1.auth_salt_b64, fake_2.auth_salt_b64);
    assert_eq!(B64.decode(&fake_1.auth_salt_b64).unwrap().len(), 16);
    assert_ne!(fake_1.auth_salt_b64, fake_other.auth_salt_b64);

    // 2. Register v2 account without ever sending raw password
    let raw_pw = "MySecretPassphrase!2026";
    let salt_b64 = B64.encode([0x3Au8; 16]);
    let params = test_fast_kdf();
    let (verifier_b64, vault_kek) = test_derive_split_key(raw_pw, &salt_b64, params);

    let reg_resp = client_a
        .register_split_key(&SplitKeyRegisterRequest {
            username: "zk_alice".to_string(),
            password: None,
            first_name: "ZK Alice".to_string(),
            last_name: None,
            device_name: "Mac1".to_string(),
            auto_welcome: false,
            auth_version: Some(2),
            auth_salt_b64: Some(salt_b64.clone()),
            auth_verifier_b64: Some(verifier_b64.clone()),
            kdf_params: Some(params),
        })
        .await
        .unwrap();
    assert_eq!(reg_resp.auth_version, 2);

    // Server DB stores Argon2id hash of verifier (never raw password or raw verifier)
    let db_rec = state
        .db
        .get_user_split_key_auth_by_identifier("zk_alice")
        .unwrap()
        .unwrap();
    assert_eq!(db_rec.auth_version, 2);
    assert_eq!(db_rec.auth_salt_b64.as_deref(), Some(salt_b64.as_str()));
    assert_eq!(db_rec.kdf_params, params);
    let stored_hash = db_rec.password_hash.unwrap();
    assert!(stored_hash.starts_with("$argon2id$"));
    assert!(!stored_hash.contains(raw_pw));
    assert!(!stored_hash.contains(&verifier_b64));

    // Upload split_key_v2 envelope wrapping a VaultRootKey
    let root_key = [0x99u8; 32];
    let wrapped_b64 = test_wrap_key_with_aad(
        &vault_kek,
        &root_key,
        &format!("gpui-user-vault:wrapped-root-key:v1|{}", reg_resp.user_id),
        0x11,
    );
    let rec_wrapped_b64 = test_wrap_key_with_aad(
        &[0x77u8; 32],
        &root_key,
        &format!("gpui-user-vault:recovery-root-key:v1|{}", reg_resp.user_id),
        0x12,
    );
    client_a
        .put_user_vault_envelope(&UserVaultKeyEnvelopeDto {
            key_version: 2,
            kdf_alg: USER_VAULT_KDF_ALG_SPLIT_KEY_V2.to_string(),
            kdf_salt_b64: salt_b64.clone(),
            kdf_m_cost_kib: params.m_cost_kib,
            kdf_t_cost: params.t_cost,
            kdf_p_cost: params.p_cost,
            wrapped_root_key_b64: wrapped_b64,
            recovery_wrapped_root_key_b64: Some(rec_wrapped_b64),
            updated_at_ms: 0,
        })
        .await
        .unwrap();

    // 3. Reject raw password login on v2 account; accept verifier-only login
    let raw_login_err = client_b
        .login(&LoginRequest {
            username: "zk_alice".to_string(),
            password: raw_pw.to_string(),
            device_name: "RawAttempt".to_string(),
        })
        .await;
    assert!(matches!(raw_login_err, Err(HttpClientError::Unauthorized(_))));

    let bad_verifier_err = client_b
        .login_split_key(&SplitKeyLoginRequest {
            username: "zk_alice".to_string(),
            password: String::new(),
            device_name: "BadVerifier".to_string(),
            auth_verifier_b64: Some(raw_pw.to_string()),
        })
        .await;
    assert!(matches!(bad_verifier_err, Err(HttpClientError::Unauthorized(_))));

    let kdf_fetched = client_b.get_auth_kdf_params("zk_alice").await.unwrap();
    assert_eq!(kdf_fetched.auth_version, 2);
    assert_eq!(kdf_fetched.auth_salt_b64, salt_b64);
    assert_eq!(kdf_fetched.kdf_params, params);

    let (verifier_login, kek_login) =
        test_derive_split_key(raw_pw, &kdf_fetched.auth_salt_b64, kdf_fetched.kdf_params);
    let login_resp = client_b
        .login_split_key(&SplitKeyLoginRequest {
            username: "zk_alice".to_string(),
            password: String::new(),
            device_name: "Mac2".to_string(),
            auth_verifier_b64: Some(verifier_login),
        })
        .await
        .unwrap();
    assert_eq!(login_resp.auth_version, 2);
    let fetched_env = client_b.get_user_vault_envelope().await.unwrap().envelope.unwrap();
    let unwrapped_root = test_unwrap_key_with_aad(
        &kek_login,
        &fetched_env.wrapped_root_key_b64,
        &format!("gpui-user-vault:wrapped-root-key:v1|{}", login_resp.user_id),
    );
    assert_eq!(unwrapped_root, root_key);

    // 4. Email Split-Key V2 registration + email KDF lookup + email login
    let send_code = client_email
        .email_send_code("zk_carol@example.com")
        .await
        .unwrap();
    let code = send_code.debug_code.unwrap();
    let email_salt = B64.encode([0x5Cu8; 16]);
    let (email_verifier, _) = test_derive_split_key("EmailSecret!2026", &email_salt, params);
    let email_reg = client_email
        .email_register_split_key(&SplitKeyEmailRegisterRequest {
            email: "zk_carol@example.com".to_string(),
            code,
            username: "zk_carol".to_string(),
            password: None,
            first_name: "Carol".to_string(),
            last_name: None,
            device_name: "MacEmail".to_string(),
            auth_version: Some(2),
            auth_salt_b64: Some(email_salt.clone()),
            auth_verifier_b64: Some(email_verifier.clone()),
            kdf_params: Some(params),
        })
        .await
        .unwrap();
    assert_eq!(email_reg.auth_version, 2);

    let kdf_by_email = client_email
        .get_auth_kdf_params("ZK_CAROL@EXAMPLE.COM")
        .await
        .unwrap();
    assert_eq!(kdf_by_email.auth_version, 2);
    assert_eq!(kdf_by_email.auth_salt_b64, email_salt);

    let login_by_email = client_email
        .login_split_key(&SplitKeyLoginRequest {
            username: "zk_carol@example.com".to_string(),
            password: String::new(),
            device_name: "MacEmail2".to_string(),
            auth_verifier_b64: Some(email_verifier),
        })
        .await
        .unwrap();
    assert_eq!(login_by_email.user_id, email_reg.user_id);
    assert_eq!(login_by_email.auth_version, 2);

    client_a.clear_session();
    client_b.clear_session();
    client_email.clear_session();
    tokio::task::spawn_blocking(move || drop((client_a, client_b, client_email)))
        .await
        .unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn test_v1_to_v2_silent_upgrade_and_change_password_preserves_vault_e2e() {
    let (_dir, _state, router) = boot_in_memory_server();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let base_url = format!("http://{addr}");

    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client = TelegramClient::new(cfg.clone());
    let client2 = TelegramClient::new(cfg);

    // 1. Legacy v1 registration
    let old_pw = "LegacyPassword!1";
    let reg = client
        .register(&RegisterRequest {
            username: "legacy_dave".to_string(),
            password: old_pw.to_string(),
            first_name: "Dave".to_string(),
            device_name: "MacLegacy".to_string(),
        })
        .await
        .unwrap();

    let kdf_v1 = client.get_auth_kdf_params("legacy_dave").await.unwrap();
    assert_eq!(kdf_v1.auth_version, 1);
    assert_eq!(kdf_v1.auth_salt_b64, "");

    // Store encrypted vault item under a VaultRootKey
    let root_key = [0xABu8; 32];
    let (item_nonce, item_ct) = test_seal_item(
        &root_key,
        reg.user_id,
        USER_VAULT_CATEGORY_PASSWORD,
        "cred_legacy",
        1,
        br#"{"origin":"https://example.com","secret":"hunter2"}"#,
    );
    client
        .put_user_vault_item(
            USER_VAULT_CATEGORY_PASSWORD,
            "cred_legacy",
            &UpsertUserVaultItemRequest {
                expected_revision: Some(0),
                nonce_b64: item_nonce,
                ciphertext_b64: item_ct,
                deleted: false,
            },
        )
        .await
        .unwrap();

    // 2. Silent v1 -> v2 upgrade
    let params = test_fast_kdf();
    let salt_v2 = B64.encode([0x21u8; 16]);
    let (verifier_v2, kek_v2) = test_derive_split_key(old_pw, &salt_v2, params);

    // Wrong old password rejected (401)
    let bad_up = client
        .upgrade_split_key(&UpgradeSplitKeyRequest {
            old_password: "WrongPassword!".to_string(),
            auth_salt_b64: salt_v2.clone(),
            auth_verifier_b64: verifier_v2.clone(),
            kdf_params: params,
        })
        .await;
    assert!(matches!(bad_up, Err(HttpClientError::Unauthorized(_))));

    // Correct old password upgrades atomically
    let up_ok = client
        .upgrade_split_key(&UpgradeSplitKeyRequest {
            old_password: old_pw.to_string(),
            auth_salt_b64: salt_v2.clone(),
            auth_verifier_b64: verifier_v2.clone(),
            kdf_params: params,
        })
        .await
        .unwrap();
    assert!(up_ok.ok);
    assert_eq!(up_ok.auth_version, 2);

    // Double upgrade rejected with 409 Conflict
    let double_up = client
        .upgrade_split_key(&UpgradeSplitKeyRequest {
            old_password: old_pw.to_string(),
            auth_salt_b64: salt_v2.clone(),
            auth_verifier_b64: verifier_v2.clone(),
            kdf_params: params,
        })
        .await;
    assert!(matches!(double_up, Err(HttpClientError::Conflict(_))));

    // Upload split_key_v2 envelope wrapping existing root_key
    let wrapped_v2 = test_wrap_key_with_aad(
        &kek_v2,
        &root_key,
        &format!("gpui-user-vault:wrapped-root-key:v1|{}", reg.user_id),
        0x31,
    );
    let rec_wrapped = test_wrap_key_with_aad(
        &[0x88u8; 32],
        &root_key,
        &format!("gpui-user-vault:recovery-root-key:v1|{}", reg.user_id),
        0x32,
    );
    client
        .put_user_vault_envelope(&UserVaultKeyEnvelopeDto {
            key_version: 2,
            kdf_alg: USER_VAULT_KDF_ALG_SPLIT_KEY_V2.to_string(),
            kdf_salt_b64: salt_v2.clone(),
            kdf_m_cost_kib: params.m_cost_kib,
            kdf_t_cost: params.t_cost,
            kdf_p_cost: params.p_cost,
            wrapped_root_key_b64: wrapped_v2,
            recovery_wrapped_root_key_b64: Some(rec_wrapped.clone()),
            updated_at_ms: 0,
        })
        .await
        .unwrap();

    // 3. Password change preserves vault ciphertext & Emergency Kit without re-encrypting items
    let new_pw = "BrandNewPassphrase!99";
    let salt_v3 = B64.encode([0x64u8; 16]);
    let (verifier_v3, kek_v3) = test_derive_split_key(new_pw, &salt_v3, params);

    // Wrong old_secret rejected (401)
    let bad_chg = client
        .change_password(&ChangePasswordRequest {
            old_secret: "not-the-old-verifier".to_string(),
            auth_version: 2,
            new_auth_salt_b64: salt_v3.clone(),
            new_auth_verifier_b64: verifier_v3.clone(),
            new_kdf_params: params,
        })
        .await;
    assert!(matches!(bad_chg, Err(HttpClientError::Unauthorized(_))));

    // Re-wrap envelope with kek_v3 and change password
    let wrapped_v3 = test_wrap_key_with_aad(
        &kek_v3,
        &root_key,
        &format!("gpui-user-vault:wrapped-root-key:v1|{}", reg.user_id),
        0x33,
    );
    client
        .put_user_vault_envelope(&UserVaultKeyEnvelopeDto {
            key_version: 2,
            kdf_alg: USER_VAULT_KDF_ALG_SPLIT_KEY_V2.to_string(),
            kdf_salt_b64: salt_v3.clone(),
            kdf_m_cost_kib: params.m_cost_kib,
            kdf_t_cost: params.t_cost,
            kdf_p_cost: params.p_cost,
            wrapped_root_key_b64: wrapped_v3,
            recovery_wrapped_root_key_b64: Some(rec_wrapped),
            updated_at_ms: 0,
        })
        .await
        .unwrap();

    let chg_ok = client
        .change_password(&ChangePasswordRequest {
            old_secret: verifier_v2.clone(),
            auth_version: 2,
            new_auth_salt_b64: salt_v3.clone(),
            new_auth_verifier_b64: verifier_v3.clone(),
            new_kdf_params: params,
        })
        .await
        .unwrap();
    assert!(chg_ok.ok);
    assert_eq!(chg_ok.auth_version, 2);

    // Old verifier login rejected, new verifier login succeeds & decrypts original item!
    let old_login_err = client2
        .login_split_key(&SplitKeyLoginRequest {
            username: "legacy_dave".to_string(),
            password: String::new(),
            device_name: "MacNew".to_string(),
            auth_verifier_b64: Some(verifier_v2),
        })
        .await;
    assert!(matches!(old_login_err, Err(HttpClientError::Unauthorized(_))));

    let kdf_v3 = client2.get_auth_kdf_params("legacy_dave").await.unwrap();
    assert_eq!(kdf_v3.auth_version, 2);
    assert_eq!(kdf_v3.auth_salt_b64, salt_v3);
    let (v3_login, kek_v3_login) =
        test_derive_split_key(new_pw, &kdf_v3.auth_salt_b64, kdf_v3.kdf_params);
    assert_eq!(v3_login, verifier_v3);

    let login_v3 = client2
        .login_split_key(&SplitKeyLoginRequest {
            username: "legacy_dave".to_string(),
            password: String::new(),
            device_name: "MacNew".to_string(),
            auth_verifier_b64: Some(v3_login),
        })
        .await
        .unwrap();
    assert_eq!(login_v3.auth_version, 2);

    let env_v3 = client2.get_user_vault_envelope().await.unwrap().envelope.unwrap();
    let unwrapped_root = test_unwrap_key_with_aad(
        &kek_v3_login,
        &env_v3.wrapped_root_key_b64,
        &format!("gpui-user-vault:wrapped-root-key:v1|{}", login_v3.user_id),
    );
    assert_eq!(unwrapped_root, root_key);

    let pulled = client2.pull_user_vault_items(Some(0), None).await.unwrap();
    assert_eq!(pulled.items.len(), 1);
    let pt = test_open_item(
        &unwrapped_root,
        login_v3.user_id,
        &pulled.items[0].category,
        &pulled.items[0].item_key,
        pulled.items[0].revision,
        &pulled.items[0].nonce_b64,
        &pulled.items[0].ciphertext_b64,
    );
    assert_eq!(pt, br#"{"origin":"https://example.com","secret":"hunter2"}"#);

    client.clear_session();
    client2.clear_session();
    tokio::task::spawn_blocking(move || drop((client, client2)))
        .await
        .unwrap();
    server_handle.abort();
}

#[tokio::test]
async fn test_device_keychain_v2_envelope_and_x25519_device_sync_e2e() {
    let (_dir, state, router) = boot_in_memory_server();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let base_url = format!("http://{addr}");

    let mut cfg = ClientConfig::default();
    cfg.server_url = base_url;
    let client_a1 = TelegramClient::new(cfg.clone());
    let client_a2 = TelegramClient::new(cfg.clone());
    let client_bob = TelegramClient::new(cfg);

    let auth_a = client_a1
        .register(&RegisterRequest {
            username: "keychain_alice".to_string(),
            password: "StrongPassword!1".to_string(),
            first_name: "Alice".to_string(),
            device_name: "MacBookPro".to_string(),
        })
        .await
        .unwrap();
    let login_a2 = client_a2
        .login(&LoginRequest {
            username: "keychain_alice".to_string(),
            password: "StrongPassword!1".to_string(),
            device_name: "MacMini".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(auth_a.user_id, login_a2.user_id);

    let _auth_bob = client_bob
        .register(&RegisterRequest {
            username: "evil_bob".to_string(),
            password: "StrongPassword!2".to_string(),
            first_name: "Bob".to_string(),
            device_name: "BobBox".to_string(),
        })
        .await
        .unwrap();

    // 1. Validate device_keychain_v2 envelope rules
    let bad_kc_env = UserVaultKeyEnvelopeDto {
        key_version: 2,
        kdf_alg: USER_VAULT_KDF_ALG_DEVICE_KEYCHAIN_V2.to_string(),
        kdf_salt_b64: String::new(),
        kdf_m_cost_kib: 65_536,
        kdf_t_cost: 3,
        kdf_p_cost: 1,
        wrapped_root_key_b64: String::new(),
        recovery_wrapped_root_key_b64: None,
        updated_at_ms: 0,
    };
    let bad_kc_err = client_a1.put_user_vault_envelope(&bad_kc_env).await;
    assert!(matches!(bad_kc_err, Err(HttpClientError::Api { status: 400, .. })));

    let root_key = [0xDEu8; 32];
    let rec_wrapped = test_wrap_key_with_aad(
        &[0x44u8; 32],
        &root_key,
        &format!("gpui-user-vault:recovery-root-key:v1|{}", auth_a.user_id),
        0x22,
    );
    let valid_kc_env = UserVaultKeyEnvelopeDto {
        key_version: 2,
        kdf_alg: USER_VAULT_KDF_ALG_DEVICE_KEYCHAIN_V2.to_string(),
        kdf_salt_b64: String::new(),
        kdf_m_cost_kib: 65_536,
        kdf_t_cost: 3,
        kdf_p_cost: 1,
        wrapped_root_key_b64: String::new(),
        recovery_wrapped_root_key_b64: Some(rec_wrapped),
        updated_at_ms: 0,
    };
    let saved_kc = client_a1
        .put_user_vault_envelope(&valid_kc_env)
        .await
        .unwrap()
        .envelope
        .unwrap();
    assert_eq!(saved_kc.kdf_alg, USER_VAULT_KDF_ALG_DEVICE_KEYCHAIN_V2);
    assert_eq!(saved_kc.wrapped_root_key_b64, "");
    assert_eq!(saved_kc.kdf_salt_b64, "");

    // 2. Reject all-zero or malformed X25519 public key
    let zero_pk_err = client_a2
        .device_sync_request(&DeviceSyncRequestBody {
            device_name: "MacMini".to_string(),
            device_public_key_b64: B64.encode([0u8; 32]),
        })
        .await;
    assert!(matches!(zero_pk_err, Err(HttpClientError::Api { status: 400, .. })));

    // 3. New device generates X25519 keypair and initiates request
    let req_secret = X25519StaticSecret::from([0x15u8; 32]);
    let req_pub = X25519PublicKey::from(&req_secret);
    let req_pub_b64 = B64.encode(req_pub.as_bytes());
    let mut hasher = Sha256::new();
    hasher.update(DEVICE_SYNC_CODE_DOMAIN_V1);
    hasher.update(req_pub.as_bytes());
    let digest = hasher.finalize();
    let expected_code_num = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    let expected_code = format!("{:03}-{:03}", expected_code_num / 1_000, expected_code_num % 1_000);

    let sync_req = client_a2
        .device_sync_request(&DeviceSyncRequestBody {
            device_name: "MacMini".to_string(),
            device_public_key_b64: req_pub_b64.clone(),
        })
        .await
        .unwrap();
    assert_eq!(sync_req.verification_code, expected_code);
    assert!(sync_req.expires_at > chrono::Utc::now().timestamp());

    // Bob sees empty pending list and gets 403 on all Alice request_id operations; unknown gets 404
    assert!(client_bob.device_sync_pending().await.unwrap().items.is_empty());
    let bob_status = client_bob.device_sync_status(&sync_req.request_id).await;
    assert!(matches!(bob_status, Err(HttpClientError::Api { status: 403, .. })));
    let bob_deny = client_bob.device_sync_deny(&sync_req.request_id).await;
    assert!(matches!(bob_deny, Err(HttpClientError::Api { status: 403, .. })));
    let bob_approve = client_bob
        .device_sync_approve(
            &sync_req.request_id,
            &ApproveDeviceSyncRequest {
                approver_ephemeral_pub_b64: req_pub_b64.clone(),
                encrypted_root_key_b64: B64.encode([0xAAu8; 48]),
                nonce_b64: B64.encode([0x01u8; 12]),
            },
        )
        .await;
    assert!(matches!(bob_approve, Err(HttpClientError::Api { status: 403, .. })));
    let missing_status = client_a2.device_sync_status("dsr_does_not_exist").await;
    assert!(matches!(missing_status, Err(HttpClientError::Api { status: 404, .. })));

    // Alice device 1 polls pending list & sees request
    let pending = client_a1.device_sync_pending().await.unwrap();
    assert_eq!(pending.items.len(), 1);
    assert_eq!(pending.items[0].request_id, sync_req.request_id);
    assert_eq!(pending.items[0].verification_code, expected_code);

    // Status before approval is pending with None ciphertext
    let st_pending = client_a2.device_sync_status(&sync_req.request_id).await.unwrap();
    assert_eq!(st_pending.status, "pending");
    assert!(st_pending.encrypted_root_key_b64.is_none());

    // Alice device 1 seals VaultRootKey with ephemeral X25519 + HKDF-SHA256 + ChaCha20-Poly1305
    let target_pub_bytes: [u8; 32] = B64
        .decode(&pending.items[0].device_public_key_b64)
        .unwrap()
        .try_into()
        .unwrap();
    let target_pub = X25519PublicKey::from(target_pub_bytes);
    let eph_secret = X25519StaticSecret::from([0x29u8; 32]);
    let eph_pub = X25519PublicKey::from(&eph_secret);
    let shared = eph_secret.diffie_hellman(&target_pub);
    let hk = Hkdf::<Sha256>::new(None, shared.as_bytes());
    let mut wrap_key = [0u8; 32];
    hk.expand(
        format!("gpui-telegram/device-key-transfer/v1:{}", sync_req.request_id).as_bytes(),
        &mut wrap_key,
    )
    .unwrap();
    let aad = format!("device-sync:{}:{}", sync_req.request_id, auth_a.user_id);
    let transfer_nonce = [0x61u8; 12];
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&wrap_key));
    let encrypted_root = cipher
        .encrypt(
            Nonce::from_slice(&transfer_nonce),
            Payload {
                msg: &root_key,
                aad: aad.as_bytes(),
            },
        )
        .unwrap();

    let approve_resp = client_a1
        .device_sync_approve(
            &sync_req.request_id,
            &ApproveDeviceSyncRequest {
                approver_ephemeral_pub_b64: B64.encode(eph_pub.as_bytes()),
                encrypted_root_key_b64: B64.encode(&encrypted_root),
                nonce_b64: B64.encode(transfer_nonce),
            },
        )
        .await
        .unwrap();
    assert!(approve_resp.ok);
    assert_eq!(approve_resp.status, "approved");
    assert!(client_a1.device_sync_pending().await.unwrap().items.is_empty());

    // First status poll delivers bundle and atomically wipes SQLite ciphertext (single-use!)
    let st_approved = client_a2.device_sync_status(&sync_req.request_id).await.unwrap();
    assert_eq!(st_approved.status, "approved");
    let eph_pub_recv: [u8; 32] = B64
        .decode(st_approved.approver_ephemeral_pub_b64.as_ref().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let shared_recv = req_secret.diffie_hellman(&X25519PublicKey::from(eph_pub_recv));
    let hk_recv = Hkdf::<Sha256>::new(None, shared_recv.as_bytes());
    let mut unwrap_key = [0u8; 32];
    hk_recv
        .expand(
            format!("gpui-telegram/device-key-transfer/v1:{}", sync_req.request_id).as_bytes(),
            &mut unwrap_key,
        )
        .unwrap();
    let recv_nonce = B64.decode(st_approved.nonce_b64.as_ref().unwrap()).unwrap();
    let recv_ct = B64
        .decode(st_approved.encrypted_root_key_b64.as_ref().unwrap())
        .unwrap();
    let opened_root = ChaCha20Poly1305::new(Key::from_slice(&unwrap_key))
        .decrypt(
            Nonce::from_slice(&recv_nonce),
            Payload {
                msg: &recv_ct,
                aad: aad.as_bytes(),
            },
        )
        .unwrap();
    assert_eq!(opened_root.as_slice(), &root_key);

    // Second status poll sees consumed + None ciphertext, and DB columns are NULL
    let st_consumed = client_a2.device_sync_status(&sync_req.request_id).await.unwrap();
    assert_eq!(st_consumed.status, "consumed");
    assert!(st_consumed.approver_ephemeral_pub_b64.is_none());
    assert!(st_consumed.encrypted_root_key_b64.is_none());
    assert!(st_consumed.nonce_b64.is_none());

    let (db_status, db_eph, db_ct, db_nonce): (String, Option<String>, Option<String>, Option<String>) =
        state
            .db
            .pool()
            .with_reader(|conn| {
                conn.query_row(
                    "SELECT status, approver_ephemeral_pub_b64, encrypted_root_key_b64, nonce_b64
                     FROM user_vault_device_sync_requests WHERE request_id = ?1",
                    rusqlite::params![sync_req.request_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
            })
            .unwrap();
    assert_eq!(db_status, "consumed");
    assert!(db_eph.is_none());
    assert!(db_ct.is_none());
    assert!(db_nonce.is_none());

    // 4. Deny flow: wipes & blocks subsequent approval
    let req2 = client_a2
        .device_sync_request(&DeviceSyncRequestBody {
            device_name: "MacMini2".to_string(),
            device_public_key_b64: req_pub_b64.clone(),
        })
        .await
        .unwrap();
    let deny_resp = client_a1.device_sync_deny(&req2.request_id).await.unwrap();
    assert_eq!(deny_resp.status, "denied");
    let st_denied = client_a2.device_sync_status(&req2.request_id).await.unwrap();
    assert_eq!(st_denied.status, "denied");
    let approve_denied_err = client_a1
        .device_sync_approve(
            &req2.request_id,
            &ApproveDeviceSyncRequest {
                approver_ephemeral_pub_b64: B64.encode(eph_pub.as_bytes()),
                encrypted_root_key_b64: B64.encode(&encrypted_root),
                nonce_b64: B64.encode(transfer_nonce),
            },
        )
        .await;
    assert!(matches!(approve_denied_err, Err(HttpClientError::Conflict(_))));

    // 5. Expired device sync request is omitted from /pending, rejected with 410 on /approve, and marked expired on /status
    let req3 = client_a2
        .device_sync_request(&DeviceSyncRequestBody {
            device_name: "MacMiniExpired".to_string(),
            device_public_key_b64: req_pub_b64,
        })
        .await
        .unwrap();
    state
        .db
        .pool()
        .with_conn(|conn| {
            conn.execute(
                "UPDATE user_vault_device_sync_requests SET expires_at = 1 WHERE request_id = ?1",
                rusqlite::params![req3.request_id],
            )
            .map_err(|e| gpui_telegram_core::error::TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert!(client_a1.device_sync_pending().await.unwrap().items.is_empty());
    let approve_exp_err = client_a1
        .device_sync_approve(
            &req3.request_id,
            &ApproveDeviceSyncRequest {
                approver_ephemeral_pub_b64: B64.encode(eph_pub.as_bytes()),
                encrypted_root_key_b64: B64.encode(&encrypted_root),
                nonce_b64: B64.encode(transfer_nonce),
            },
        )
        .await;
    assert!(matches!(approve_exp_err, Err(HttpClientError::Api { status: 410, .. })));
    let st_exp = client_a2.device_sync_status(&req3.request_id).await.unwrap();
    assert_eq!(st_exp.status, "expired");

    client_a1.clear_session();
    client_a2.clear_session();
    client_bob.clear_session();
    tokio::task::spawn_blocking(move || drop((client_a1, client_a2, client_bob)))
        .await
        .unwrap();
    server_handle.abort();
}
