use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use gpui_telegram_agent::{
    evaluate_group_wake_gate, evaluate_outbound_turn_review, is_room_session_key,
    parse_room_session_chat_id, room_session_key, FloorControlQueue, FloorPriority,
    GroupTurnPromptContext, GroupWakeBotCandidate, GroupWakeGateInput, GroupWakeReason,
    GroupWorkspaceBridge, McpToolDefinition, OutboundReviewReason, OutboundTurnReviewInput,
    RoomMemberTurnOutcome, ToolRiskLevel, TurnDeliveryIntake,
};
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_proto::contracts::CreateChatRequest;
use gpui_telegram_proto::sync::{ChatActionEvent, SyncGetDifferenceRequest, UpdateEvent};
use gpui_telegram_server::{
    build_router_with_state,
    config::ServerConfig,
    server::{AppState, ServerGroupWorkspaceBridge},
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tower::ServiceExt;

fn boot_temp_server() -> (TempDir, Arc<AppState>, Router) {
    let dir = TempDir::new().expect("create TempDir");
    let db_path = dir.path().join("grok_multi_agent.db");
    let media_dir = dir.path().join("media_cas");
    std::fs::create_dir_all(&media_dir).expect("create media dir");
    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", db_path.display());
    cfg.media_storage_path = media_dir.to_string_lossy().to_string();
    cfg.host = "127.0.0.1".to_string();
    cfg.port = 0;

    let state = Arc::new(AppState::try_new(cfg).expect("boot AppState"));
    let router = build_router_with_state(Arc::clone(&state));
    (dir, state, router)
}

async fn http_json(
    router: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = if let Some(b) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = router.clone().oneshot(req).await.expect("oneshot router");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let val = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, val)
}

async fn register_user(router: &Router, username: &str, first_name: &str) -> (u64, String) {
    let (status, val) = http_json(
        router,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": username,
            "password": "GrokStrongPassword!2026",
            "first_name": first_name,
            "device_name": "GrokMultiAgentTest"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register {username}: {val}");
    (
        val["user_id"].as_u64().expect("user_id"),
        val["token"].as_str().expect("token").to_string(),
    )
}

#[test]
fn create_chat_request_client_nonce_serde_roundtrip() {
    let without_nonce = CreateChatRequest {
        title: "Alpha".to_string(),
        chat_type: Some(json!(1)),
        member_ids: vec![101, 202],
        ..Default::default()
    };
    let serialized_none = serde_json::to_value(&without_nonce).unwrap();
    assert!(
        serialized_none.get("client_nonce").is_none(),
        "None client_nonce must be omitted from JSON"
    );

    let with_nonce = CreateChatRequest {
        title: String::new(),
        chat_type: Some(json!(1)),
        member_ids: vec![101, 202],
        client_nonce: Some("grp-nonce-42".to_string()),
        ..Default::default()
    };
    let serialized = serde_json::to_value(&with_nonce).unwrap();
    assert_eq!(serialized["client_nonce"], "grp-nonce-42");
    let decoded: CreateChatRequest = serde_json::from_value(serialized).unwrap();
    assert_eq!(decoded.client_nonce.as_deref(), Some("grp-nonce-42"));
}

#[tokio::test]
async fn create_chat_idempotent_client_nonce_and_auto_title_for_mixed_group() {
    let (_dir, state, router) = boot_temp_server();
    let (alice_id, alice_tok) = register_user(&router, "alice_idem", "Alice").await;
    let (bob_id, bob_tok) = register_user(&router, "bob_idem", "Bob").await;

    // Provision a Fabushi AI Bot
    let (st_bot, val_bot) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&alice_tok),
        Some(json!({
            "display_name": "Rust Architect",
            "username": "rust_arch_bot",
            "description": "Designs Rust distributed systems"
        })),
    )
    .await;
    assert_eq!(st_bot, StatusCode::CREATED, "create bot: {val_bot}");
    let bot_id = val_bot["bot"]["bot_user_id"].as_u64().expect("bot_user_id");

    // 1) Empty title with ZERO members MUST still fail with 400
    let (st_empty, _) = http_json(
        &router,
        "POST",
        "/api/v1/chats/create",
        Some(&alice_tok),
        Some(json!({
            "title": "   ",
            "chat_type": 1
        })),
    )
    .await;
    assert_eq!(st_empty, StatusCode::BAD_REQUEST);

    // 2) Empty title + mixed human+bot members + client_nonce -> auto-generated title & created
    let req_body = serde_json::to_value(CreateChatRequest {
        title: String::new(),
        chat_type: Some(json!(1)),
        member_ids: vec![bob_id, bot_id],
        client_nonce: Some("grp-mixed-nonce-001".to_string()),
        ..Default::default()
    })
    .unwrap();

    let (st1, chat1) = http_json(
        &router,
        "POST",
        "/api/v1/chats/create",
        Some(&alice_tok),
        Some(req_body.clone()),
    )
    .await;
    assert_eq!(st1, StatusCode::CREATED, "create mixed group: {chat1}");
    let cid1 = chat1["chat_id"].as_i64().expect("chat_id");
    let title1 = chat1["title"].as_str().expect("auto title");
    assert!(!title1.trim().is_empty(), "auto-generated title must be non-empty");
    assert!(
        title1.chars().count() <= 72,
        "auto-generated title must be <= 72 chars, got: {title1}"
    );
    assert!(
        title1.contains("Bob") && title1.contains("Rust Architect"),
        "auto-generated title should include participant display names, got: {title1}"
    );
    assert_eq!(chat1["member_count"].as_u64(), Some(3));

    // All members (creator, human peer, bot) must be subscribed to gateway_hub
    let subs = state.gateway_hub.get_chat_subscribers(cid1);
    assert!(subs.contains(&UserId(alice_id)));
    assert!(subs.contains(&UserId(bob_id)));
    assert!(subs.contains(&UserId(bot_id)));

    // 3) Retry with identical client_nonce returns the EXACT same chat_id (no duplicate group)
    let (st2, chat2) = http_json(
        &router,
        "POST",
        "/api/v1/chats/create",
        Some(&alice_tok),
        Some(req_body),
    )
    .await;
    assert_eq!(st2, StatusCode::CREATED, "retry create mixed group: {chat2}");
    assert_eq!(
        chat2["chat_id"].as_i64(),
        Some(cid1),
        "duplicate client_nonce must return identical chat_id"
    );
    assert_eq!(chat2["title"].as_str(), Some(title1));

    // 4) Tenant isolation: Bob using the same client_nonce creates his own separate group
    let (st3, chat3) = http_json(
        &router,
        "POST",
        "/api/v1/chats/create",
        Some(&bob_tok),
        Some(json!({
            "title": "",
            "chat_type": 1,
            "member_usernames": ["alice_idem", "rust_arch_bot"],
            "client_nonce": "grp-mixed-nonce-001"
        })),
    )
    .await;
    assert_eq!(st3, StatusCode::CREATED, "bob create with same nonce: {chat3}");
    let cid3 = chat3["chat_id"].as_i64().expect("bob chat_id");
    assert_ne!(
        cid3, cid1,
        "client_nonce idempotency must be scoped per creator_user_id"
    );

    // 5) Very long participant display names truncate cleanly to <= 72 chars with ellipsis
    let (long_u1, _) = register_user(
        &router,
        "long_name_u1",
        "VeryLongParticipantDisplayNameNumberOneAlphaBetaGamma",
    )
    .await;
    let (long_u2, _) = register_user(
        &router,
        "long_name_u2",
        "VeryLongParticipantDisplayNameNumberTwoDeltaEpsilonZeta",
    )
    .await;
    let (st_long, chat_long) = http_json(
        &router,
        "POST",
        "/api/v1/chats/create",
        Some(&alice_tok),
        Some(json!({
            "title": "",
            "chat_type": 1,
            "member_ids": [long_u1, long_u2]
        })),
    )
    .await;
    assert_eq!(st_long, StatusCode::CREATED);
    let long_title = chat_long["title"].as_str().unwrap_or("");
    assert!(
        long_title.chars().count() <= 72 && long_title.ends_with('…'),
        "long auto-title must truncate cleanly <=72 chars with ellipsis: '{long_title}'"
    );
}

#[tokio::test]
async fn members_add_subscribes_gateway_and_broadcasts_user_joined_pts() {
    let (_dir, state, router) = boot_temp_server();
    let (_alice_id, alice_tok) = register_user(&router, "alice_mem", "Alice").await;
    let (bob_id, _bob_tok) = register_user(&router, "bob_mem", "Bob").await;

    let (st_bot, val_bot) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&alice_tok),
        Some(json!({
            "display_name": "QA Bot",
            "username": "qa_member_bot",
            "description": "Runs automated QA checks"
        })),
    )
    .await;
    assert_eq!(st_bot, StatusCode::CREATED);
    let bot_id = val_bot["bot"]["bot_user_id"].as_u64().expect("bot_user_id");

    let (st_grp, val_grp) = http_json(
        &router,
        "POST",
        "/api/v1/chats/create",
        Some(&alice_tok),
        Some(json!({
            "title": "Live Membership Room",
            "chat_type": 1
        })),
    )
    .await;
    assert_eq!(st_grp, StatusCode::CREATED);
    let chat_id = val_grp["chat_id"].as_i64().expect("chat_id");

    // Add both a human member and a bot member via POST /api/v1/chats/{chat_id}/members
    let (st_add, val_add) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{chat_id}/members"),
        Some(&alice_tok),
        Some(json!({ "user_ids": [bob_id, bot_id] })),
    )
    .await;
    assert_eq!(st_add, StatusCode::OK, "add members: {val_add}");
    assert_eq!(val_add["success"], true);

    // Newly added members must be immediately subscribed to gateway_hub
    let subs = state.gateway_hub.get_chat_subscribers(chat_id);
    assert!(subs.contains(&UserId(bob_id)), "Bob must be subscribed to gateway_hub");
    assert!(subs.contains(&UserId(bot_id)), "Bot must be subscribed to gateway_hub");

    // PTS journal must record UserJoined events for both newly added members
    let diff = state
        .pts_engine
        .get_difference(&SyncGetDifferenceRequest::new(ChatId(chat_id), 0, 50))
        .expect("get_difference");
    let joined_users: Vec<UserId> = diff
        .updates
        .iter()
        .filter_map(|u| match &u.event {
            UpdateEvent::ChatAction(ChatActionEvent::UserJoined { user_id }) => Some(*user_id),
            _ => None,
        })
        .collect();
    assert!(
        joined_users.contains(&UserId(bob_id)),
        "PTS journal must contain UserJoined for Bob: {joined_users:?}"
    );
    assert!(
        joined_users.contains(&UserId(bot_id)),
        "PTS journal must contain UserJoined for Bot: {joined_users:?}"
    );

    // GET /api/v1/chats/{chat_id}/members reflects all 3 participants
    let (st_list, val_list) = http_json(
        &router,
        "GET",
        &format!("/api/v1/chats/{chat_id}/members"),
        Some(&alice_tok),
        None,
    )
    .await;
    assert_eq!(st_list, StatusCode::OK);
    let members = val_list["members"].as_array().expect("members array");
    assert_eq!(members.len(), 3);
}

#[tokio::test]
async fn event_driven_floor_nonce_fencing_preemption_and_reply_gate() {
    // 1) Event-driven Notify wakeup (zero busy polling) + winding-down flag + nonce fencing
    let floor = FloorControlQueue::new(30_000);
    let grant_a = floor
        .acquire_floor_wait_with_options("agent-a", FloorPriority::DirectMention, Duration::from_secs(2), false)
        .await
        .expect("agent-a acquires floor immediately");
    assert!(!grant_a.is_winding_down);
    assert_eq!(
        floor.validate_turn_nonce("agent-a", &grant_a.nonce),
        TurnDeliveryIntake::Accepted
    );

    // Spawn waiter for agent-b with is_winding_down = true; release agent-a after 35ms
    let floor_waiter = floor.clone();
    let started = Instant::now();
    let waiter = tokio::spawn(async move {
        floor_waiter
            .acquire_floor_wait_with_options(
                "agent-b",
                FloorPriority::TaskAssignee,
                Duration::from_secs(2),
                true,
            )
            .await
    });

    tokio::time::sleep(Duration::from_millis(35)).await;
    assert!(floor.release_turn("agent-a", &grant_a.nonce));

    let grant_b = waiter
        .await
        .expect("join waiter")
        .expect("agent-b wakes via Notify");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "event-driven wakeup should complete promptly without timeout"
    );
    assert!(grant_b.is_winding_down);
    assert_eq!(
        floor.validate_turn_nonce("agent-b", &grant_b.nonce),
        TurnDeliveryIntake::Accepted
    );

    // 2) Preemption by human interruption immediately invalidates agent-b's fencing nonce
    let preempted = floor.preempt_for_user_interruption();
    assert_eq!(preempted.as_deref(), Some(grant_b.nonce.as_str()));
    assert_eq!(
        floor.validate_turn_nonce("agent-b", &grant_b.nonce),
        TurnDeliveryIntake::UnknownNonce,
        "preempted turn nonce must be rejected so late replies never post"
    );
    assert!(
        !floor.release_turn("agent-b", &grant_b.nonce),
        "stale turn guard release must not disturb floor"
    );

    // 3) Stage 1 shouldReply wake gate selects specialist & stays silent on chatter
    let rust_bot = GroupWakeBotCandidate {
        bot_user_id: 500_001,
        agent_id: "rust-expert".into(),
        name: "Rust Expert".into(),
        username: "rust_expert_bot".into(),
        role: "Rust Systems Engineer".into(),
        description: "Specializes in Rust async, tokio, Cargo, and borrow checker debugging".into(),
        skills: vec!["rust".into(), "tokio".into(), "cargo".into()],
        seconds_since_last_reply: None,
    };
    let design_bot = GroupWakeBotCandidate {
        bot_user_id: 500_002,
        agent_id: "ui-designer".into(),
        name: "Design Lead".into(),
        username: "design_lead_bot".into(),
        role: "Product Designer".into(),
        description: "Focuses on typography, color palettes, and Figma layouts".into(),
        skills: vec!["figma".into(), "typography".into()],
        seconds_since_last_reply: None,
    };

    let wake_rust = evaluate_group_wake_gate(&GroupWakeGateInput {
        candidates: vec![rust_bot.clone(), design_bot.clone()],
        message_text: "Can someone help debug this tokio async borrow checker issue?".into(),
        remaining_loop_budget: 5,
        ..Default::default()
    });
    assert!(wake_rust.should_wake);
    assert_eq!(wake_rust.reason, GroupWakeReason::RelevanceMatch);
    assert_eq!(wake_rust.wake_bot_user_ids, vec![500_001]);

    let low_signal = evaluate_group_wake_gate(&GroupWakeGateInput {
        candidates: vec![rust_bot, design_bot],
        message_text: "ok thanks!".into(),
        remaining_loop_budget: 5,
        ..Default::default()
    });
    assert!(!low_signal.should_wake);
    assert_eq!(low_signal.outcome, RoomMemberTurnOutcome::Pass);
    assert!(low_signal.wake_bot_user_ids.is_empty());

    // 4) Stage 2 outbound review suppresses [PASS] / <PASS/> / NO_REPLY & duplicate echoes
    for sentinel in ["[PASS]", "<PASS/>", "NO_REPLY", "[PASS] teammate answered"] {
        let verdict = evaluate_outbound_turn_review(&OutboundTurnReviewInput {
            agent_id: "ui-designer".into(),
            candidate_text: sentinel.into(),
            recent_room_messages: Vec::new(),
            ..Default::default()
        });
        assert_eq!(verdict.outcome, RoomMemberTurnOutcome::Pass);
        assert!(!verdict.should_send);
        assert_eq!(verdict.reason, OutboundReviewReason::ExplicitPassSentinel);
    }

    let dup_verdict = evaluate_outbound_turn_review(&OutboundTurnReviewInput {
        agent_id: "ui-designer".into(),
        candidate_text: "The Tokio mutex should be dropped before awaiting.".into(),
        recent_room_messages: vec!["The Tokio mutex should be dropped before awaiting.".into()],
        ..Default::default()
    });
    assert_eq!(dup_verdict.outcome, RoomMemberTurnOutcome::Pass);
    assert_eq!(dup_verdict.reason, OutboundReviewReason::DuplicateEcho);

    // 5) Room session key isolation helpers
    let key = room_session_key(8899);
    assert_eq!(key, "group:8899");
    assert!(is_room_session_key(&key));
    assert_eq!(parse_room_session_chat_id(&key), Some(8899));
}

#[tokio::test]
async fn server_group_workspace_bridge_and_unified_permission_dock_roundtrip() {
    let (_dir, state, router) = boot_temp_server();
    let (_alice_id, alice_tok) = register_user(&router, "alice_ws_dock", "Alice").await;

    // Create a group chat and a custom Fabushi bot
    let (st_bot, val_bot) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&alice_tok),
        Some(json!({
            "display_name": "Planner Bot",
            "username": "planner_ws_bot",
            "description": "Coordinates shared group plans"
        })),
    )
    .await;
    assert_eq!(st_bot, StatusCode::CREATED);
    let bot_id = val_bot["bot"]["bot_user_id"].as_i64().expect("bot_user_id");

    let (st_grp, val_grp) = http_json(
        &router,
        "POST",
        "/api/v1/chats/create",
        Some(&alice_tok),
        Some(json!({
            "title": "Workspace & Safety Room",
            "chat_type": 1,
            "member_ids": [bot_id]
        })),
    )
    .await;
    assert_eq!(st_grp, StatusCode::CREATED);
    let chat_id = val_grp["chat_id"].as_i64().expect("chat_id");

    // 1) Verify ServerGroupWorkspaceBridge is wired into BOTH core_engine AND the provisioned bot
    let bot_inst = state
        .fabushi_bots
        .get_by_user_id(bot_id)
        .expect("provisioned bot instance");
    let bot_bridge = bot_inst
        .hermes()
        .group_workspace_bridge()
        .expect("bot HermesEngine must have ServerGroupWorkspaceBridge wired");
    assert!(
        state.fabushi_core().group_workspace_bridge().is_some(),
        "core_engine must have ServerGroupWorkspaceBridge wired"
    );

    // Round-trip write -> read -> list -> optimistic revision conflict via wired bridge
    let (rev1, entry1) = bot_bridge
        .write_key(chat_id, "plan", json!({"phase": 1, "owner": "planner"}), &bot_inst.agent_id, None)
        .await
        .expect("write_key rev1");
    assert_eq!(rev1, 1);
    assert_eq!(entry1.key, "plan");

    let read_back = bot_bridge
        .read_key(chat_id, "plan")
        .await
        .expect("read_key")
        .expect("entry exists");
    assert_eq!(read_back.value, json!({"phase": 1, "owner": "planner"}));

    let conflict = bot_bridge
        .write_key(chat_id, "plan", json!({"phase": 99}), &bot_inst.agent_id, Some(0))
        .await;
    assert!(conflict.is_err(), "stale expected_revision must fail");

    let (rev2, _) = bot_bridge
        .write_key(chat_id, "plan", json!({"phase": 2}), &bot_inst.agent_id, Some(1))
        .await
        .expect("write_key rev2 with matching expected_revision");
    assert_eq!(rev2, 2);

    let keys = bot_bridge.list_keys(chat_id).await.expect("list_keys");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].value, json!({"phase": 2}));

    // Verify canonical AppState.multi_agent_groups room workspace reflects revision 2
    {
        let groups = state.multi_agent_groups.read();
        let room = groups.get(&ChatId(chat_id)).expect("room initialized");
        assert_eq!(room.workspace.revision, 2);
        assert_eq!(
            room.workspace.get("plan").map(|e| e.value.clone()),
            Some(json!({"phase": 2}))
        );
    }

    // Verify standalone ServerGroupWorkspaceBridge constructor also works directly
    let direct_bridge = ServerGroupWorkspaceBridge::new(
        Arc::clone(&state.multi_agent_groups),
        Arc::clone(&state.pts_engine),
        Arc::downgrade(&state.gateway_hub),
    );
    let direct_entry = direct_bridge
        .read_key(chat_id, "plan")
        .await
        .expect("direct read")
        .expect("plan present");
    assert_eq!(direct_entry.value, json!({"phase": 2}));

    // 2) Verify build_turn_system_prompt_with_group_context injects group scope + [PASS] + winding down
    let (sp, _) = state.fabushi_bots.build_turn_system_prompt_with_group_context(
        bot_id,
        chat_id,
        "Base prompt",
        &[],
        &[],
        Some(&GroupTurnPromptContext {
            room_name: "Workspace & Safety Room".into(),
            peers: vec!["Alice (@alice_ws_dock)".into()],
            is_winding_down: true,
            remaining_hops: 1,
        }),
    );
    assert!(sp.contains(&room_session_key(chat_id)));
    assert!(sp.contains("[PASS]"));
    assert!(sp.contains("WINDING DOWN"));

    // 3) Verify unified AppState.permission_dock is shared by pointer identity across server & bots
    let bot_dock = bot_inst
        .hermes()
        .permission_dock()
        .expect("bot HermesEngine must have shared ToolPermissionDock");
    let core_dock = state
        .fabushi_core()
        .permission_dock()
        .expect("core_engine must have shared ToolPermissionDock");
    assert!(Arc::ptr_eq(&state.permission_dock, &bot_dock));
    assert!(Arc::ptr_eq(&state.permission_dock, &core_dock));

    // Register a high-risk MCP tool and verify POST /api/v1/mcp/call -> pending -> approve flow
    state.mcp_registry.write().register(McpToolDefinition::new(
        "deploy_prod_release",
        "Deploys production release",
        "{}",
        ToolRiskLevel::High,
    ));

    let (st_call, val_call) = http_json(
        &router,
        "POST",
        "/api/v1/mcp/call",
        Some(&alice_tok),
        Some(json!({
            "tool_name": "deploy_prod_release",
            "arguments": { "env": "prod" }
        })),
    )
    .await;
    assert_eq!(st_call, StatusCode::ACCEPTED, "high-risk mcp call: {val_call}");
    let req_id = val_call["request_id"].as_str().expect("request_id").to_string();
    assert_eq!(bot_dock.pending_shared_count(), 1);

    let (st_apr, val_apr) = http_json(
        &router,
        "POST",
        "/api/v1/mcp/permission/approve",
        Some(&alice_tok),
        Some(json!({
            "request_id": req_id,
            "allow": true
        })),
    )
    .await;
    assert_eq!(st_apr, StatusCode::OK, "approve mcp request: {val_apr}");
    assert_eq!(val_apr["status"], "approved");
    assert!(
        val_apr["approval_token_id"].as_str().is_some_and(|s| !s.is_empty()),
        "approval must issue a single-use value-bound approval token: {val_apr}"
    );
    assert_eq!(bot_dock.pending_shared_count(), 0);
}

#[tokio::test]
async fn group_chat_bot_reply_routing_unmentioned_mentions_slash_reply_to_and_ws_send() {
    let (_dir, state, router) = boot_temp_server();
    let (alice_id, alice_tok) = register_user(&router, "alice_grp_reply", "Alice").await;

    // Provision two distinct AI bots in the same group
    let (st_a, val_a) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&alice_tok),
        Some(json!({
            "display_name": "Rust Architect",
            "username": "rust_grp_bot",
            "description": "Rust distributed systems specialist"
        })),
    )
    .await;
    assert_eq!(st_a, StatusCode::CREATED, "create bot A: {val_a}");
    let bot_a_id = val_a["bot"]["bot_user_id"].as_u64().expect("bot_a_id");

    let (st_b, val_b) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&alice_tok),
        Some(json!({
            "display_name": "全球法布施",
            "username": "dharma_grp_bot",
            "description": "禅修与经典助手"
        })),
    )
    .await;
    assert_eq!(st_b, StatusCode::CREATED, "create bot B: {val_b}");
    let bot_b_id = val_b["bot"]["bot_user_id"].as_u64().expect("bot_b_id");

    // Create a multi-bot group chat
    let (st_grp, val_grp) = http_json(
        &router,
        "POST",
        "/api/v1/chats/create",
        Some(&alice_tok),
        Some(json!({
            "title": "AI Bots War Room",
            "chat_type": 1,
            "member_ids": [bot_a_id, bot_b_id]
        })),
    )
    .await;
    assert_eq!(st_grp, StatusCode::CREATED, "create group: {val_grp}");
    let chat_id = val_grp["chat_id"].as_i64().expect("chat_id");

    // Helper: poll DB until a message from one of expected_senders appears after `after_msg_id`
    async fn wait_for_bot_reply(
        state: &Arc<AppState>,
        chat_id: i64,
        after_msg_id: i64,
        expected_senders: &[u64],
    ) -> (i64, u64) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let msgs = state
                .db
                .list_messages_with_topic(ChatId(chat_id), None, None, 30)
                .unwrap_or_default();
            if let Some(found) = msgs
                .iter()
                .find(|m| m.message_id.0 > after_msg_id && expected_senders.contains(&m.sender_id.0))
            {
                return (found.message_id.0, found.sender_id.0);
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for bot reply from {:?} after msg {} in chat {}; recent msgs: {:?}",
                expected_senders,
                after_msg_id,
                chat_id,
                msgs.iter()
                    .map(|m| (m.message_id.0, m.sender_id.0))
                    .collect::<Vec<_>>()
            );
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }

    // 1. `/start@dharma_grp_bot` must target ONLY bot B (not bot A!)
    let (st_m1, val_m1) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&alice_tok),
        Some(json!({ "text": "/start@dharma_grp_bot" })),
    )
    .await;
    assert_eq!(st_m1, StatusCode::CREATED, "send /start@dharma_grp_bot: {val_m1}");
    let m1_id = val_m1["message_id"].as_i64().expect("m1_id");
    let (bot_b_welcome_msg_id, responder_1) =
        wait_for_bot_reply(&state, chat_id, m1_id, &[bot_b_id]).await;
    assert_eq!(responder_1, bot_b_id);

    // 2. Replying (`reply_to`) to bot B's message with `/help` must wake bot B
    let (st_m2, val_m2) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&alice_tok),
        Some(json!({
            "text": "/help",
            "reply_to": bot_b_welcome_msg_id
        })),
    )
    .await;
    assert_eq!(st_m2, StatusCode::CREATED, "send reply_to bot B: {val_m2}");
    let m2_id = val_m2["message_id"].as_i64().expect("m2_id");
    let (_, responder_2) = wait_for_bot_reply(&state, chat_id, m2_id, &[bot_b_id]).await;
    assert_eq!(responder_2, bot_b_id);

    // 3. CJK-adjacent display name mention `@全球法布施 /help` wakes bot B
    let (st_m3, val_m3) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&alice_tok),
        Some(json!({ "text": "@全球法布施 /help" })),
    )
    .await;
    assert_eq!(st_m3, StatusCode::CREATED, "send @全球法布施 /help: {val_m3}");
    let m3_id = val_m3["message_id"].as_i64().expect("m3_id");
    let (_, responder_3) = wait_for_bot_reply(&state, chat_id, m3_id, &[bot_b_id]).await;
    assert_eq!(responder_3, bot_b_id);

    // 4. Multi-word display name mention `@Rust Architect /help` wakes bot A
    let (st_m4, val_m4) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&alice_tok),
        Some(json!({ "text": "@Rust Architect /help" })),
    )
    .await;
    assert_eq!(st_m4, StatusCode::CREATED, "send @Rust Architect /help: {val_m4}");
    let m4_id = val_m4["message_id"].as_i64().expect("m4_id");
    let (_, responder_4) = wait_for_bot_reply(&state, chat_id, m4_id, &[bot_a_id]).await;
    assert_eq!(responder_4, bot_a_id);

    // 5. Unmentioned general Chinese message `"你好，请介绍一下自己"` in a 2-bot group
    //    must wake a primary bot and emit a reply (never silently ignored!)
    let (st_m5, val_m5) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{chat_id}/messages"),
        Some(&alice_tok),
        Some(json!({ "text": "你好，请介绍一下自己" })),
    )
    .await;
    assert_eq!(st_m5, StatusCode::CREATED, "send unmentioned greeting: {val_m5}");
    let m5_id = val_m5["message_id"].as_i64().expect("m5_id");
    let (after_m5_id, responder_5) =
        wait_for_bot_reply(&state, chat_id, m5_id, &[bot_a_id, bot_b_id]).await;
    assert!(responder_5 == bot_a_id || responder_5 == bot_b_id);

    // 6. WebSocket `WsClientFrame::SendMessage` in the group must ALSO trigger bot reply!
    let (ws_tx, _ws_rx) = tokio::sync::mpsc::unbounded_channel();
    let ws_req = gpui_telegram_proto::ws::SendMessageRequest::new(
        ChatId(chat_id),
        998_877_665,
        gpui_telegram_proto::message::MessageContent::plain_text("/start@rust_grp_bot"),
    );
    state.gateway_hub.handle_client_frame(
        UserId(alice_id),
        gpui_telegram_proto::ws::WsClientFrame::SendMessage(ws_req),
        &ws_tx,
    );
    let (_, responder_ws) = wait_for_bot_reply(&state, chat_id, after_m5_id, &[bot_a_id]).await;
    assert_eq!(responder_ws, bot_a_id);
}

