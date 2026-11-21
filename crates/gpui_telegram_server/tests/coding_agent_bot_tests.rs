use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use futures::{SinkExt, StreamExt};
use gpui_telegram_proto::coding_agent_contracts::{
    CodingAgentBotConfig, CodingApprovalDecision, CodingControlAction, CodingDiffStat,
    CodingHostCommand, CodingHostEvent, CodingPermissionMode, CodingProviderKind, CodingTokenUsage,
    DetectedCodingProvider,
};
use gpui_telegram_proto::miniapp_contracts::{
    CreateFabushiBotRequest, FabushiBotCreateResponse, FabushiBotSummaryDto,
    PatchFabushiBotRequest,
};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::{WsClientFrame, WsServerFrame};
use gpui_telegram_server::{
    build_router_with_state, config::ServerConfig, server::AppState,
};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};
use tower::ServiceExt;
use uuid::Uuid;

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("{}_{}", prefix, Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self { path: dir }
    }

    fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn boot_temp_server(prefix: &str) -> (TempDirGuard, Arc<AppState>, Router) {
    let dir = TempDirGuard::new(prefix);
    let db_path = dir.path().join("coding_e2e.db");
    let media_dir = dir.path().join("media_cas");
    std::fs::create_dir_all(&media_dir).expect("create media dir");
    let mut cfg = ServerConfig::default();
    cfg.database_url = format!("sqlite://{}", db_path.display());
    cfg.media_storage_path = media_dir.to_string_lossy().to_string();
    cfg.host = "127.0.0.1".to_string();
    cfg.port = 0;

    let state = Arc::new(AppState::try_new(cfg).expect("boot AppState"));
    let router = build_router_with_state(state.clone());
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

async fn register_user(router: &Router, username: &str) -> (u64, String) {
    let (status, val) = http_json(
        router,
        "POST",
        "/api/v1/auth/register",
        None,
        Some(json!({
            "username": username,
            "password": "CodingStrongPassword!2026",
            "first_name": username,
            "last_name": "Tester",
            "device_name": "DeviceOne"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register {username}: {val}");
    (
        val["user_id"].as_u64().expect("user_id"),
        val["token"].as_str().expect("token").to_string(),
    )
}

async fn login_second_device(router: &Router, username: &str, device_name: &str) -> String {
    let (status, val) = http_json(
        router,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({
            "username": username,
            "password": "CodingStrongPassword!2026",
            "device_name": device_name
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login {device_name}: {val}");
    val["token"].as_str().expect("token").to_string()
}

async fn list_chat_messages(router: &Router, token: &str, chat_id: i64) -> Vec<Value> {
    let (status, val) = http_json(
        router,
        "GET",
        &format!("/api/v1/chats/{chat_id}/messages?limit=100"),
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "list messages for {chat_id}: {val}");
    val["messages"].as_array().cloned().unwrap_or_default()
}

struct MockWsClient {
    out_tx: mpsc::UnboundedSender<WsClientFrame>,
    in_rx: mpsc::UnboundedReceiver<WsServerFrame>,
    _task: tokio::task::JoinHandle<()>,
}

impl MockWsClient {
    async fn connect(addr: std::net::SocketAddr, token: &str) -> Self {
        let ws_url = format!("ws://{}/api/v1/ws?token={}", addr, token);
        let (ws_stream, _) = connect_async(&ws_url).await.expect("connect ws");
        let (mut sink, mut stream) = ws_stream.split();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<WsClientFrame>();
        let (in_tx, in_rx) = mpsc::unbounded_channel::<WsServerFrame>();

        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    maybe_out = out_rx.recv() => {
                        let Some(frame) = maybe_out else { break; };
                        let txt = serde_json::to_string(&frame).unwrap();
                        if sink.send(WsMessage::Text(txt.into())).await.is_err() {
                            break;
                        }
                    }
                    maybe_in = stream.next() => {
                        match maybe_in {
                            Some(Ok(WsMessage::Text(txt))) => {
                                if let Ok(sf) = serde_json::from_str::<WsServerFrame>(&txt) {
                                    let _ = in_tx.send(sf);
                                }
                            }
                            Some(Ok(WsMessage::Close(_))) | None => break,
                            _ => {}
                        }
                    }
                }
            }
        });

        Self {
            out_tx,
            in_rx,
            _task: task,
        }
    }

    fn send_host_event(&self, ev: CodingHostEvent) {
        self.out_tx
            .send(WsClientFrame::CodingHostEvent(ev))
            .expect("send CodingHostEvent");
    }

    async fn recv_coding_command(&mut self, timeout: Duration) -> Option<CodingHostCommand> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return None;
            }
            match tokio::time::timeout(deadline - now, self.in_rx.recv()).await {
                Ok(Some(WsServerFrame::Update(UpdateEvent::CodingHostCommand(cmd)))) => {
                    return Some(cmd);
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => return None,
            }
        }
    }

    async fn recv_server_frame_matching<F>(
        &mut self,
        timeout: Duration,
        mut pred: F,
    ) -> Option<WsServerFrame>
    where
        F: FnMut(&WsServerFrame) -> bool,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return None;
            }
            match tokio::time::timeout(deadline - now, self.in_rx.recv()).await {
                Ok(Some(frame)) => {
                    if pred(&frame) {
                        return Some(frame);
                    }
                }
                Ok(None) | Err(_) => return None,
            }
        }
    }
}

fn sample_provider(kind: CodingProviderKind) -> DetectedCodingProvider {
    DetectedCodingProvider {
        kind,
        display_name: kind.display_name().to_string(),
        binary_path: Some(format!("/usr/local/bin/{}", kind.default_binary().unwrap_or("acp"))),
        version: Some("1.2.3".to_string()),
        installed: true,
        recommended_username: kind.recommended_username().to_string(),
        supports_acp: kind.supports_acp(),
        login_hint: "Ready".to_string(),
    }
}

async fn start_tcp_server(router: Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind tcp");
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (addr, handle)
}

#[tokio::test]
async fn coding_agent_bot_creation_commands_patch_and_probe() {
    let (_dir, state, router) = boot_temp_server("coding_create_probe");
    let (alice_uid, alice_token) = register_user(&router, "coding_alice_1").await;
    let (addr, server_handle) = start_tcp_server(router.clone()).await;

    let mut ws = MockWsClient::connect(addr, &alice_token).await;
    ws.send_host_event(CodingHostEvent::HostRegister {
        device_id: "mac-alice-1".to_string(),
        display_name: "Alice MacBook".to_string(),
        os_name: "macos".to_string(),
        providers: vec![sample_provider(CodingProviderKind::ClaudeCode)],
    });

    // Wait briefly for HostRegister to be processed on the WS task.
    for _ in 0..40 {
        if !state.coding_agent_broker.hosts_for_user(alice_uid).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    // Spawn live probe HTTP call while responding over WS
    let router_clone = router.clone();
    let token_clone = alice_token.clone();
    let probe_fut = tokio::spawn(async move {
        http_json(
            &router_clone,
            "POST",
            "/api/v1/coding-agents/probe",
            Some(&token_clone),
            Some(json!({ "device_id": "mac-alice-1" })),
        )
        .await
    });

    let cmd = ws
        .recv_coding_command(Duration::from_secs(2))
        .await
        .expect("should receive ProbeProviders");
    match cmd {
        CodingHostCommand::ProbeProviders {
            req_id,
            owner_user_id,
            target_device_id,
        } => {
            assert_eq!(owner_user_id, alice_uid);
            assert_eq!(target_device_id.as_deref(), Some("mac-alice-1"));
            ws.send_host_event(CodingHostEvent::ProbeResult {
                req_id,
                device_id: "mac-alice-1".to_string(),
                providers: vec![
                    sample_provider(CodingProviderKind::ClaudeCode),
                    sample_provider(CodingProviderKind::Codex),
                ],
            });
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let (probe_st, probe_val) = probe_fut.await.unwrap();
    assert_eq!(probe_st, StatusCode::OK, "probe response: {probe_val}");
    assert_eq!(probe_val["ok"], true);
    assert_eq!(probe_val["device_id"], "mac-alice-1");
    assert_eq!(probe_val["providers"].as_array().unwrap().len(), 2);

    // Create Coding Agent Bot
    let coding_cfg = CodingAgentBotConfig {
        provider_kind: CodingProviderKind::ClaudeCode,
        workspace_path: "/Users/alice/my-repo".to_string(),
        use_worktree: false,
        permission_mode: CodingPermissionMode::Ask,
        bound_device_id: Some("mac-alice-1".to_string()),
        allow_group_members: false,
        ..Default::default()
    };
    let req = CreateFabushiBotRequest {
        display_name: "Alice Claude Coder".to_string(),
        username: Some("alice_claude_dev_bot".to_string()),
        agent_id: None,
        description: "Local Claude coding agent".to_string(),
        title: "Claude Coder".to_string(),
        avatar_shape: Some("hex".to_string()),
        avatar_color: Some("#3B82F6".to_string()),
        preferred_model: None,
        thinking_level: None,
        approval_mode: None,
        toolset_preset: None,
        coding_agent: Some(coding_cfg.clone()),
    };
    let (st_c, val_c) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&alice_token),
        Some(serde_json::to_value(&req).unwrap()),
    )
    .await;
    assert_eq!(st_c, StatusCode::CREATED, "create coding bot: {val_c}");
    let created: FabushiBotCreateResponse = serde_json::from_value(val_c).unwrap();
    assert_eq!(created.bot.coding_agent.as_ref(), Some(&coding_cfg));
    assert_eq!(created.registry.coding_agent.as_ref(), Some(&coding_cfg));
    assert!(created.companion_chat_id > 0);

    // Verify all 13 coding slash commands are registered
    let cmd_names: Vec<&str> = created
        .registry
        .commands
        .iter()
        .map(|c| c.command.as_str())
        .collect();
    for expected in [
        "start", "workspace", "worktree", "model", "mode", "diff", "rollback", "handoff", "stop",
        "new", "status", "login", "help",
    ] {
        assert!(
            cmd_names.contains(&expected),
            "Missing command /{expected} in {cmd_names:?}"
        );
    }

    // Companion DM welcome card reflects Coding Agent config (not Hermes generic)
    let welcome_msgs = list_chat_messages(&router, &alice_token, created.companion_chat_id).await;
    assert!(!welcome_msgs.is_empty());
    assert!(
        welcome_msgs[0]["text"]
            .as_str()
            .unwrap()
            .contains("Claude Code"),
        "Welcome card should mention Coding Agent provider: {}",
        welcome_msgs[0]
    );

    // Patch coding_agent config
    let mut patched_cfg = coding_cfg.clone();
    patched_cfg.permission_mode = CodingPermissionMode::AutoEdit;
    patched_cfg.use_worktree = true;
    patched_cfg.model_override = Some("claude-sonnet-4".to_string());
    let patch_req = PatchFabushiBotRequest {
        coding_agent: Some(patched_cfg.clone()),
        ..Default::default()
    };
    let (st_p, val_p) = http_json(
        &router,
        "PATCH",
        &format!("/api/v1/fabushi-bots/{}", created.bot.bot_user_id),
        Some(&alice_token),
        Some(serde_json::to_value(&patch_req).unwrap()),
    )
    .await;
    assert_eq!(st_p, StatusCode::OK, "patch coding bot: {val_p}");
    let patched_dto: FabushiBotSummaryDto = serde_json::from_value(val_p).unwrap();
    assert_eq!(patched_dto.coding_agent.as_ref(), Some(&patched_cfg));

    // Verify DB persistence via GET
    let (st_g, val_g) = http_json(
        &router,
        "GET",
        &format!("/api/v1/fabushi-bots/{}", created.bot.bot_user_id),
        Some(&alice_token),
        None,
    )
    .await;
    assert_eq!(st_g, StatusCode::OK);
    let fetched_dto: FabushiBotSummaryDto = serde_json::from_value(val_g).unwrap();
    assert_eq!(fetched_dto.coding_agent.as_ref(), Some(&patched_cfg));

    server_handle.abort();
}

#[tokio::test]
async fn single_device_targeting_and_offline_error() {
    let (_dir, state, router) = boot_temp_server("coding_single_device");
    let (alice_uid, tok_dev1) = register_user(&router, "coding_multi_dev").await;
    let tok_dev2 = login_second_device(&router, "coding_multi_dev", "LinuxWorkstation").await;
    let (addr, server_handle) = start_tcp_server(router.clone()).await;

    let mut ws1 = MockWsClient::connect(addr, &tok_dev1).await;
    let mut ws2 = MockWsClient::connect(addr, &tok_dev2).await;

    ws1.send_host_event(CodingHostEvent::HostRegister {
        device_id: "dev-mac-1".to_string(),
        display_name: "Mac Studio".to_string(),
        os_name: "macos".to_string(),
        providers: vec![sample_provider(CodingProviderKind::Codex)],
    });
    ws2.send_host_event(CodingHostEvent::HostRegister {
        device_id: "dev-linux-2".to_string(),
        display_name: "Linux Box".to_string(),
        os_name: "linux".to_string(),
        providers: vec![sample_provider(CodingProviderKind::Codex)],
    });

    for _ in 0..40 {
        if state.coding_agent_broker.hosts_for_user(alice_uid).len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    // Create bot strictly bound to `dev-mac-1`
    let (st_c, val_c) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&tok_dev1),
        Some(
            serde_json::to_value(CreateFabushiBotRequest {
                display_name: "Pinned Mac Codex".to_string(),
                username: Some("pinned_mac_codex_bot".to_string()),
                coding_agent: Some(CodingAgentBotConfig {
                    provider_kind: CodingProviderKind::Codex,
                    workspace_path: "/repo/pinned".to_string(),
                    bound_device_id: Some("dev-mac-1".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(st_c, StatusCode::CREATED, "{val_c}");
    let created: FabushiBotCreateResponse = serde_json::from_value(val_c).unwrap();
    let cid = created.companion_chat_id;

    // Send prompt -> MUST reach ONLY ws1 (`dev-mac-1`) and NEVER ws2 (`dev-linux-2`)
    let (st_m1, _) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{cid}/messages"),
        Some(&tok_dev1),
        Some(json!({ "text": "run unit tests" })),
    )
    .await;
    assert!(st_m1.is_success());

    let cmd1 = ws1
        .recv_coding_command(Duration::from_secs(2))
        .await
        .expect("ws1 must receive StartTurn");
    let tid1 = match cmd1 {
        CodingHostCommand::StartTurn {
            turn_id,
            target_device_id,
            prompt,
            ..
        } => {
            assert_eq!(target_device_id.as_deref(), Some("dev-mac-1"));
            assert_eq!(prompt, "run unit tests");
            turn_id
        }
        other => panic!("expected StartTurn on ws1, got {other:?}"),
    };
    assert!(
        ws2.recv_coding_command(Duration::from_millis(200)).await.is_none(),
        "ws2 MUST NOT receive coding command targeted to dev-mac-1"
    );

    // Finish turn 1 cleanly
    ws1.send_host_event(CodingHostEvent::TurnCompleted {
        turn_id: tid1,
        ok: true,
        final_text: "Done on Mac".to_string(),
        error: None,
        turn_idx: 1,
        checkpoint_ref: None,
        diff_stat: None,
        token_usage: None,
    });

    // Disconnect ws1 while ws2 remains connected -> prompt MUST fail with offline message naming `dev-mac-1` (NOT fall back to ws2!)
    drop(ws1);
    for _ in 0..40 {
        let hosts = state.coding_agent_broker.hosts_for_user(alice_uid);
        if hosts
            .iter()
            .find(|h| h.device_id == "dev-mac-1")
            .is_some_and(|h| !h.online)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let (st_m2, val_m2) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{cid}/messages"),
        Some(&tok_dev2),
        Some(json!({ "text": "second prompt while mac offline" })),
    )
    .await;
    assert!(st_m2.is_success());
    let m2_id = val_m2["message_id"].as_i64().unwrap();

    assert!(
        ws2.recv_coding_command(Duration::from_millis(200)).await.is_none(),
        "ws2 MUST NOT receive command when pinned dev-mac-1 is offline"
    );

    let mut saw_offline_notice = false;
    for _ in 0..40 {
        let msgs = list_chat_messages(&router, &tok_dev2, cid).await;
        if let Some(reply) = msgs.iter().find(|m| {
            m["id"].as_i64().unwrap_or(0) > m2_id
                && m["sender_id"].as_i64() == Some(created.bot.bot_user_id)
        }) {
            let txt = reply["text"].as_str().unwrap_or("");
            if txt.contains("dev-mac-1") && txt.contains("离线") {
                saw_offline_notice = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        saw_offline_notice,
        "Bot must post informative offline message naming dev-mac-1"
    );

    // Unpinned bot (`bound_device_id: None`) with 2 active connections MUST deliver to ONLY ONE connection
    let tok_dev3 = login_second_device(&router, "coding_multi_dev", "ThirdBox").await;
    let mut ws3 = MockWsClient::connect(addr, &tok_dev3).await;
    ws3.send_host_event(CodingHostEvent::HostRegister {
        device_id: "dev-3".to_string(),
        display_name: "Third Box".to_string(),
        os_name: "macos".to_string(),
        providers: vec![sample_provider(CodingProviderKind::Codex)],
    });
    tokio::time::sleep(Duration::from_millis(40)).await;

    let (_, val_unpinned) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&tok_dev2),
        Some(
            serde_json::to_value(CreateFabushiBotRequest {
                display_name: "Unpinned Bot".to_string(),
                username: Some("unpinned_codex_bot".to_string()),
                coding_agent: Some(CodingAgentBotConfig {
                    provider_kind: CodingProviderKind::Codex,
                    workspace_path: "/repo/unpinned".to_string(),
                    bound_device_id: None,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap(),
        ),
    )
    .await;
    let unpinned: FabushiBotCreateResponse = serde_json::from_value(val_unpinned).unwrap();
    let _ = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{}/messages", unpinned.companion_chat_id),
        Some(&tok_dev2),
        Some(json!({ "text": "hello single target" })),
    )
    .await;

    let got_ws2 = ws2.recv_coding_command(Duration::from_millis(300)).await;
    let got_ws3 = ws3.recv_coding_command(Duration::from_millis(300)).await;
    assert_eq!(
        got_ws2.is_some() as u8 + got_ws3.is_some() as u8,
        1,
        "Exactly ONE device should receive StartTurn (never broadcast): ws2={got_ws2:?}, ws3={got_ws3:?}"
    );

    server_handle.abort();
}

#[tokio::test]
async fn streamed_turn_approval_diff_rollback_and_double_click_guard() {
    let (_dir, state, router) = boot_temp_server("coding_stream_approval");
    let (alice_uid, alice_token) = register_user(&router, "coding_stream_user").await;
    let (addr, server_handle) = start_tcp_server(router.clone()).await;

    let mut ws = MockWsClient::connect(addr, &alice_token).await;
    ws.send_host_event(CodingHostEvent::HostRegister {
        device_id: "mac-stream".to_string(),
        display_name: "Mac Stream".to_string(),
        os_name: "macos".to_string(),
        providers: vec![sample_provider(CodingProviderKind::ClaudeCode)],
    });
    for _ in 0..40 {
        if !state.coding_agent_broker.hosts_for_user(alice_uid).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    let (_, val_c) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&alice_token),
        Some(
            serde_json::to_value(CreateFabushiBotRequest {
                display_name: "Stream Bot".to_string(),
                username: Some("stream_coding_bot".to_string()),
                coding_agent: Some(CodingAgentBotConfig {
                    provider_kind: CodingProviderKind::ClaudeCode,
                    workspace_path: "/repo/stream".to_string(),
                    bound_device_id: Some("mac-stream".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap(),
        ),
    )
    .await;
    let created: FabushiBotCreateResponse = serde_json::from_value(val_c).unwrap();
    let bot_id = created.bot.bot_user_id;
    let cid = created.companion_chat_id;

    let (st_m, val_m) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{cid}/messages"),
        Some(&alice_token),
        Some(json!({ "text": "fix failing parser tests" })),
    )
    .await;
    assert!(st_m.is_success());
    let prompt_msg_id = val_m["message_id"].as_i64().unwrap();

    let cmd = ws
        .recv_coding_command(Duration::from_secs(2))
        .await
        .expect("StartTurn");
    let turn_id = match cmd {
        CodingHostCommand::StartTurn { turn_id, .. } => turn_id,
        other => panic!("expected StartTurn, got {other:?}"),
    };

    // Emit streaming events
    ws.send_host_event(CodingHostEvent::TurnStarted {
        turn_id: turn_id.clone(),
        provider_session_id: Some("sess_1".to_string()),
        workspace_path: "/repo/stream".to_string(),
        worktree_path: None,
        git_branch: Some("main".to_string()),
        turn_idx: 1,
    });
    ws.send_host_event(CodingHostEvent::TurnActivity {
        turn_id: turn_id.clone(),
        state: "thinking".to_string(),
        detail: Some("Inspecting parser.rs".to_string()),
    });
    ws.send_host_event(CodingHostEvent::TurnThinkingDelta {
        turn_id: turn_id.clone(),
        delta: "Checking edge cases...".to_string(),
    });
    ws.send_host_event(CodingHostEvent::TurnToolStep {
        turn_id: turn_id.clone(),
        call_id: "call_1".to_string(),
        tool_name: "bash".to_string(),
        title: "cargo test".to_string(),
        status: "running".to_string(),
        output_preview: None,
    });

    // Wait for initial streamed message + verify WS NewMessage and [⏹️ 停止] button
    let new_msg_frame = ws
        .recv_server_frame_matching(Duration::from_secs(2), |f| {
            matches!(f, WsServerFrame::PtsPush(p) if matches!(&p.event, UpdateEvent::NewMessage(m) if m.sender_id.0 as i64 == bot_id && m.id.0 > prompt_msg_id))
        })
        .await
        .expect("should receive initial streamed bot NewMessage");
    let stream_msg_id = match new_msg_frame {
        WsServerFrame::PtsPush(p) => match p.event {
            UpdateEvent::NewMessage(m) => {
                let kb_str = serde_json::to_string(&m.inline_keyboard).unwrap();
                assert!(
                    kb_str.contains(&format!("coding:stop:{turn_id}")),
                    "Running turn must have stop button: {kb_str}"
                );
                m.id.0
            }
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };

    // Wait past 120ms throttle, send tool completion + text delta -> expect EditMessage PTS push
    tokio::time::sleep(Duration::from_millis(140)).await;
    ws.send_host_event(CodingHostEvent::TurnToolStep {
        turn_id: turn_id.clone(),
        call_id: "call_1".to_string(),
        tool_name: "bash".to_string(),
        title: "cargo test".to_string(),
        status: "completed".to_string(),
        output_preview: Some("1 failed".to_string()),
    });
    ws.send_host_event(CodingHostEvent::TurnTextDelta {
        turn_id: turn_id.clone(),
        delta: "Found root cause in parser.rs".to_string(),
    });

    let edit_frame = ws
        .recv_server_frame_matching(Duration::from_secs(2), |f| {
            matches!(f, WsServerFrame::PtsPush(p) if matches!(&p.event, UpdateEvent::EditMessage(m) if m.id.0 == stream_msg_id && serde_json::to_string(&m.content).is_ok_and(|s| s.contains("Found root cause in parser.rs"))))
        })
        .await
        .expect("should receive EditMessage with streamed text delta");
    if let WsServerFrame::PtsPush(p) = edit_frame {
        if let UpdateEvent::EditMessage(m) = p.event {
            let txt = serde_json::to_string(&m.content).unwrap();
            assert!(txt.contains("Found root cause in parser.rs"), "{txt}");
        }
    }

    // Emit TurnApprovalRequested
    ws.send_host_event(CodingHostEvent::TurnApprovalRequested {
        turn_id: turn_id.clone(),
        request_id: "apr_1".to_string(),
        kind: "bash".to_string(),
        title: "Run cargo fix".to_string(),
        detail: "cargo fix --allow-dirty".to_string(),
        rule_key: "bash:cargo_fix".to_string(),
    });

    let mut approval_msg_id = 0_i64;
    for _ in 0..40 {
        let msgs = list_chat_messages(&router, &alice_token, cid).await;
        if let Some(card) = msgs.iter().find(|m| {
            m["inline_keyboard"]
                .to_string()
                .contains(&format!("coding:approve:{turn_id}:apr_1:session"))
        }) {
            approval_msg_id = card["id"].as_i64().unwrap();
            let kb_s = card["inline_keyboard"].to_string();
            assert!(kb_s.contains(&format!("coding:approve:{turn_id}:apr_1:once")));
            assert!(kb_s.contains(&format!("coding:approve:{turn_id}:apr_1:session")));
            assert!(kb_s.contains(&format!("coding:approve:{turn_id}:apr_1:deny")));
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(approval_msg_id > 0, "Approval card message must be posted");

    // Click AllowSession
    let (st_apr, val_apr) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{cid}/callback"),
        Some(&alice_token),
        Some(json!({
            "message_id": approval_msg_id,
            "callback_data": format!("coding:approve:{turn_id}:apr_1:session")
        })),
    )
    .await;
    assert_eq!(st_apr, StatusCode::OK, "approve callback: {val_apr}");
    assert_eq!(val_apr["inline_keyboard"], json!([]));

    match ws.recv_coding_command(Duration::from_secs(2)).await {
        Some(CodingHostCommand::ResolveApproval {
            turn_id: tid,
            request_id,
            decision,
            ..
        }) => {
            assert_eq!(tid, turn_id);
            assert_eq!(request_id, "apr_1");
            assert_eq!(decision, CodingApprovalDecision::AllowSession);
        }
        other => panic!("expected ResolveApproval, got {other:?}"),
    }

    // Duplicate click on approval button MUST NOT dispatch another ResolveApproval
    let (st_dup, val_dup) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{cid}/callback"),
        Some(&alice_token),
        Some(json!({
            "message_id": approval_msg_id,
            "callback_data": format!("coding:approve:{turn_id}:apr_1:deny")
        })),
    )
    .await;
    assert_eq!(st_dup, StatusCode::OK);
    assert!(
        ws.recv_coding_command(Duration::from_millis(150)).await.is_none(),
        "Double-click on already resolved approval must not re-dispatch"
    );
    let _ = val_dup;

    // Complete the turn with diff_stat + token_usage
    ws.send_host_event(CodingHostEvent::TurnCompleted {
        turn_id: turn_id.clone(),
        ok: true,
        final_text: "Fixed parser and all tests pass.".to_string(),
        error: None,
        turn_idx: 1,
        checkpoint_ref: Some("refs/fabushi-coding/checkpoints/t/1".to_string()),
        diff_stat: Some(CodingDiffStat {
            files_changed: 2,
            insertions: 15,
            deletions: 4,
            changed_files: vec!["src/parser.rs".to_string(), "tests/parser_test.rs".to_string()],
        }),
        token_usage: Some(CodingTokenUsage {
            input_tokens: 1500,
            output_tokens: 420,
            cached_tokens: 800,
            cost_usd: Some(0.0123),
        }),
    });

    // Wait for finalized message in DB with Diff/Rollback/PR buttons and [⏹️ 停止] cleared
    let mut final_msg = Value::Null;
    for _ in 0..40 {
        let msgs = list_chat_messages(&router, &alice_token, cid).await;
        if let Some(m) = msgs.iter().find(|m| m["id"].as_i64() == Some(stream_msg_id)) {
            let kb_s = m["inline_keyboard"].to_string();
            if kb_s.contains(&format!("coding:diff:{bot_id}:1")) {
                final_msg = m.clone();
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!final_msg.is_null(), "Finalized message must update buttons");
    let final_txt = final_msg["text"].as_str().unwrap();
    assert!(final_txt.contains("Fixed parser and all tests pass."));
    assert!(final_txt.contains("1500"));
    assert!(final_txt.contains("420"));
    let final_kb = final_msg["inline_keyboard"].to_string();
    assert!(!final_kb.contains("coding:stop:"), "Stop button must be cleared");
    assert!(final_kb.contains(&format!("coding:diff:{bot_id}:1")));
    assert!(final_kb.contains(&format!("coding:rollback:{bot_id}:1")));
    assert!(final_kb.contains(&format!("coding:pr:{bot_id}:1")));

    // Trigger coding:diff callback
    let router_diff = router.clone();
    let tok_diff = alice_token.clone();
    let diff_fut = tokio::spawn(async move {
        http_json(
            &router_diff,
            "POST",
            &format!("/api/v1/chats/{cid}/callback"),
            Some(&tok_diff),
            Some(json!({
                "message_id": stream_msg_id,
                "callback_data": format!("coding:diff:{bot_id}:1")
            })),
        )
        .await
    });
    match ws.recv_coding_command(Duration::from_secs(2)).await {
        Some(CodingHostCommand::Control {
            req_id,
            action: CodingControlAction::GetDiff { turn_idx },
            ..
        }) => {
            assert_eq!(turn_idx, Some(1));
            ws.send_host_event(CodingHostEvent::ControlResult {
                req_id,
                ok: true,
                markdown_reply: "```diff\n-old\n+new\n```".to_string(),
                updated_workspace: None,
                updated_use_worktree: None,
                updated_model: None,
                updated_permission_mode: None,
                handoff_summary: None,
            });
        }
        other => panic!("expected GetDiff control, got {other:?}"),
    }
    let (st_diff, val_diff) = diff_fut.await.unwrap();
    assert_eq!(st_diff, StatusCode::OK, "{val_diff}");
    assert!(val_diff["system_message"].as_str().unwrap().contains("+new"));

    // Trigger coding:rollback -> MUST post confirmation prompt first without calling RollbackCheckpoint
    let (st_rb, val_rb) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{cid}/callback"),
        Some(&alice_token),
        Some(json!({
            "message_id": stream_msg_id,
            "callback_data": format!("coding:rollback:{bot_id}:1")
        })),
    )
    .await;
    assert_eq!(st_rb, StatusCode::OK, "{val_rb}");
    assert!(
        ws.recv_coding_command(Duration::from_millis(120)).await.is_none(),
        "coding:rollback must require confirmation before executing RollbackCheckpoint"
    );
    let rb_confirm_mid = val_rb["reply_message_id"].as_i64().unwrap();
    assert!(val_rb["inline_keyboard"]
        .to_string()
        .contains(&format!("coding:rollback_confirm:{bot_id}:1")));

    // Confirm rollback
    let router_rb = router.clone();
    let tok_rb = alice_token.clone();
    let rb_fut = tokio::spawn(async move {
        http_json(
            &router_rb,
            "POST",
            &format!("/api/v1/chats/{cid}/callback"),
            Some(&tok_rb),
            Some(json!({
                "message_id": rb_confirm_mid,
                "callback_data": format!("coding:rollback_confirm:{bot_id}:1")
            })),
        )
        .await
    });
    match ws.recv_coding_command(Duration::from_secs(2)).await {
        Some(CodingHostCommand::Control {
            req_id,
            action: CodingControlAction::RollbackCheckpoint { turn_idx },
            ..
        }) => {
            assert_eq!(turn_idx, Some(1));
            ws.send_host_event(CodingHostEvent::ControlResult {
                req_id,
                ok: true,
                markdown_reply: "↩️ 已成功回滚至 Turn #1 检查点。".to_string(),
                updated_workspace: None,
                updated_use_worktree: None,
                updated_model: None,
                updated_permission_mode: None,
                handoff_summary: None,
            });
        }
        other => panic!("expected RollbackCheckpoint control, got {other:?}"),
    }
    let (st_rb_ok, val_rb_ok) = rb_fut.await.unwrap();
    assert_eq!(st_rb_ok, StatusCode::OK, "{val_rb_ok}");
    assert_eq!(val_rb_ok["inline_keyboard"], json!([]));

    // Second click on rollback_confirm MUST be blocked
    let (st_rb2, _) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{cid}/callback"),
        Some(&alice_token),
        Some(json!({
            "message_id": rb_confirm_mid,
            "callback_data": format!("coding:rollback_confirm:{bot_id}:1")
        })),
    )
    .await;
    assert_eq!(st_rb2, StatusCode::OK);
    assert!(
        ws.recv_coding_command(Duration::from_millis(120)).await.is_none(),
        "Duplicate click on rollback_confirm must not re-run RollbackCheckpoint"
    );

    server_handle.abort();
}

#[tokio::test]
async fn owner_only_security_enforcement_in_group_chat_and_callbacks() {
    let (_dir, state, router) = boot_temp_server("coding_owner_only");
    let (alice_uid, alice_token) = register_user(&router, "owner_alice_sec").await;
    let (bob_uid, bob_token) = register_user(&router, "member_bob_sec").await;
    let (addr, server_handle) = start_tcp_server(router.clone()).await;

    let mut ws = MockWsClient::connect(addr, &alice_token).await;
    ws.send_host_event(CodingHostEvent::HostRegister {
        device_id: "alice-sec-mac".to_string(),
        display_name: "Alice Sec Mac".to_string(),
        os_name: "macos".to_string(),
        providers: vec![sample_provider(CodingProviderKind::ClaudeCode)],
    });
    for _ in 0..40 {
        if !state.coding_agent_broker.hosts_for_user(alice_uid).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    // Alice creates an owner-only Coding Agent Bot (allow_group_members = false)
    let (_, val_c) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&alice_token),
        Some(
            serde_json::to_value(CreateFabushiBotRequest {
                display_name: "Alice Private Coder".to_string(),
                username: Some("alice_private_coder_bot".to_string()),
                coding_agent: Some(CodingAgentBotConfig {
                    provider_kind: CodingProviderKind::ClaudeCode,
                    workspace_path: "/Users/alice/secret-repo".to_string(),
                    bound_device_id: Some("alice-sec-mac".to_string()),
                    allow_group_members: false,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap(),
        ),
    )
    .await;
    let created: FabushiBotCreateResponse = serde_json::from_value(val_c).unwrap();
    let bot_id = created.bot.bot_user_id;

    // Create group chat containing Alice, Bob, and Alice's Coding Bot
    let (st_chat, val_chat) = http_json(
        &router,
        "POST",
        "/api/v1/chats",
        Some(&alice_token),
        Some(json!({
            "title": "Team Dev Room",
            "chat_type": 1,
            "member_ids": [bob_uid, bot_id as u64]
        })),
    )
    .await;
    assert_eq!(st_chat, StatusCode::CREATED, "{val_chat}");
    let group_cid = val_chat["chat_id"].as_i64().unwrap();

    // 1) Bob tries to prompt the bot in the group -> MUST reject in chat & NEVER dispatch to Alice's host!
    let (st_bob_msg, val_bob_msg) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{group_cid}/messages"),
        Some(&bob_token),
        Some(json!({ "text": "@alice_private_coder_bot cat ~/.ssh/id_rsa" })),
    )
    .await;
    assert!(st_bob_msg.is_success());
    let bob_mid = val_bob_msg["message_id"].as_i64().unwrap();

    assert!(
        ws.recv_coding_command(Duration::from_millis(200)).await.is_none(),
        "SECURITY VIOLATION: Non-owner prompt MUST NEVER reach owner's local coding host"
    );

    let mut saw_lock_reply = false;
    for _ in 0..40 {
        let msgs = list_chat_messages(&router, &bob_token, group_cid).await;
        if let Some(r) = msgs.iter().find(|m| {
            m["id"].as_i64().unwrap_or(0) > bob_mid && m["sender_id"].as_i64() == Some(bot_id)
        }) {
            if r["text"]
                .as_str()
                .unwrap_or("")
                .contains("该编程智能体仅限所有者操作")
            {
                saw_lock_reply = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        saw_lock_reply,
        "Bot must post Owner-Only refusal message to Bob"
    );

    // 2) Alice starts a turn in the group, host requests approval -> Bob tries to approve/stop/diff -> 403 FORBIDDEN!
    let _ = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{group_cid}/messages"),
        Some(&alice_token),
        Some(json!({ "text": "@alice_private_coder_bot run cargo check" })),
    )
    .await;
    let turn_id = match ws.recv_coding_command(Duration::from_secs(2)).await {
        Some(CodingHostCommand::StartTurn { turn_id, .. }) => turn_id,
        other => panic!("expected StartTurn for Alice, got {other:?}"),
    };

    ws.send_host_event(CodingHostEvent::TurnApprovalRequested {
        turn_id: turn_id.clone(),
        request_id: "apr_grp_1".to_string(),
        kind: "bash".to_string(),
        title: "Run cargo check".to_string(),
        detail: "cargo check".to_string(),
        rule_key: "bash:cargo_check".to_string(),
    });
    let mut grp_apr_mid = 0_i64;
    for _ in 0..40 {
        let msgs = list_chat_messages(&router, &alice_token, group_cid).await;
        if let Some(m) = msgs.iter().find(|m| {
            m["inline_keyboard"]
                .to_string()
                .contains(&format!("coding:approve:{turn_id}:apr_grp_1:once"))
        }) {
            grp_apr_mid = m["id"].as_i64().unwrap();
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(grp_apr_mid > 0);
    assert!(!state
        .coding_agent_broker
        .is_turn_caller_authorized(&turn_id, gpui_telegram_core::types::UserId(bob_uid)));
    assert!(state
        .coding_agent_broker
        .is_turn_caller_authorized(&turn_id, gpui_telegram_core::types::UserId(alice_uid)));

    for cb_data in [
        format!("coding:approve:{turn_id}:apr_grp_1:once"),
        format!("coding:stop:{turn_id}"),
        format!("coding:diff:{bot_id}:1"),
        format!("coding:rollback_confirm:{bot_id}:1"),
    ] {
        let (st_cb, val_cb) = http_json(
            &router,
            "POST",
            &format!("/api/v1/chats/{group_cid}/callback"),
            Some(&bob_token),
            Some(json!({
                "message_id": grp_apr_mid,
                "callback_data": cb_data
            })),
        )
        .await;
        assert_eq!(
            st_cb,
            StatusCode::FORBIDDEN,
            "Non-owner Bob callback `{cb_data}` must return 403 FORBIDDEN: {val_cb}"
        );
    }
    assert!(
        ws.recv_coding_command(Duration::from_millis(150)).await.is_none(),
        "Bob's rejected callbacks MUST NOT dispatch any command to Alice's host"
    );

    // Alice CAN approve
    let (st_alice_apr, _) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{group_cid}/callback"),
        Some(&alice_token),
        Some(json!({
            "message_id": grp_apr_mid,
            "callback_data": format!("coding:approve:{turn_id}:apr_grp_1:once")
        })),
    )
    .await;
    assert_eq!(st_alice_apr, StatusCode::OK);
    assert!(matches!(
        ws.recv_coding_command(Duration::from_secs(2)).await,
        Some(CodingHostCommand::ResolveApproval {
            decision: CodingApprovalDecision::AllowOnce,
            ..
        })
    ));

    // Also verify POST /api/v1/chats/{chat_id}/agent-runs blocks Bob and never falls into Hermes
    let (st_ar, _) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{group_cid}/agent-runs"),
        Some(&bob_token),
        Some(json!({ "bot_id": bot_id, "prompt": "hack" })),
    )
    .await;
    assert_eq!(st_ar, StatusCode::FORBIDDEN);

    server_handle.abort();
}

#[tokio::test]
async fn slash_commands_persist_config_and_group_handoff_respects_loop_budget() {
    let (_dir, state, router) = boot_temp_server("coding_slash_handoff");
    let (alice_uid, alice_token) = register_user(&router, "handoff_alice").await;
    let (addr, server_handle) = start_tcp_server(router.clone()).await;

    let mut ws = MockWsClient::connect(addr, &alice_token).await;
    ws.send_host_event(CodingHostEvent::HostRegister {
        device_id: "mac-handoff".to_string(),
        display_name: "Mac Handoff".to_string(),
        os_name: "macos".to_string(),
        providers: vec![
            sample_provider(CodingProviderKind::ClaudeCode),
            sample_provider(CodingProviderKind::Codex),
        ],
    });
    for _ in 0..40 {
        if !state.coding_agent_broker.hosts_for_user(alice_uid).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    // Create Bot A (ClaudeCode) and Bot B (Codex)
    let (_, val_a) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&alice_token),
        Some(
            serde_json::to_value(CreateFabushiBotRequest {
                display_name: "Claude Builder".to_string(),
                username: Some("handoff_claude_bot".to_string()),
                coding_agent: Some(CodingAgentBotConfig {
                    provider_kind: CodingProviderKind::ClaudeCode,
                    workspace_path: "/repo/old".to_string(),
                    bound_device_id: Some("mac-handoff".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap(),
        ),
    )
    .await;
    let bot_a: FabushiBotCreateResponse = serde_json::from_value(val_a).unwrap();

    let (_, val_b) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&alice_token),
        Some(
            serde_json::to_value(CreateFabushiBotRequest {
                display_name: "Codex Reviewer".to_string(),
                username: Some("handoff_codex_bot".to_string()),
                coding_agent: Some(CodingAgentBotConfig {
                    provider_kind: CodingProviderKind::Codex,
                    workspace_path: "/repo/new".to_string(),
                    bound_device_id: Some("mac-handoff".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap(),
        ),
    )
    .await;
    let bot_b: FabushiBotCreateResponse = serde_json::from_value(val_b).unwrap();

    // Auto-responder task on mock WS for Control & StartTurn
    let out_tx = ws.out_tx.clone();
    let (handoff_turn_tx, mut handoff_turn_rx) = mpsc::unbounded_channel::<(i64, String)>();
    let responder_task = tokio::spawn(async move {
        while let Some(frame) = ws.in_rx.recv().await {
            if let WsServerFrame::Update(UpdateEvent::CodingHostCommand(cmd)) = frame {
                match cmd {
                    CodingHostCommand::Control { req_id, action, .. } => {
                        let ev = match action {
                            CodingControlAction::SetWorkspace { path } => {
                                CodingHostEvent::ControlResult {
                                    req_id,
                                    ok: true,
                                    markdown_reply: format!("📂 工作区已切换至 `{path}`"),
                                    updated_workspace: Some(path),
                                    updated_use_worktree: None,
                                    updated_model: None,
                                    updated_permission_mode: None,
                                    handoff_summary: None,
                                }
                            }
                            CodingControlAction::WorktreeOp { op, .. } => {
                                CodingHostEvent::ControlResult {
                                    req_id,
                                    ok: true,
                                    markdown_reply: format!("🌿 Worktree `{op}` 已更新"),
                                    updated_workspace: None,
                                    updated_use_worktree: Some(op == "on"),
                                    updated_model: None,
                                    updated_permission_mode: None,
                                    handoff_summary: None,
                                }
                            }
                            CodingControlAction::SetModel { model } => {
                                CodingHostEvent::ControlResult {
                                    req_id,
                                    ok: true,
                                    markdown_reply: format!("🧠 模型已切换为 `{model}`"),
                                    updated_workspace: None,
                                    updated_use_worktree: None,
                                    updated_model: Some(model),
                                    updated_permission_mode: None,
                                    handoff_summary: None,
                                }
                            }
                            CodingControlAction::SetPermissionMode { mode } => {
                                CodingHostEvent::ControlResult {
                                    req_id,
                                    ok: true,
                                    markdown_reply: format!("🛡️ 模式已切换为 `{}`", mode.as_str()),
                                    updated_workspace: None,
                                    updated_use_worktree: None,
                                    updated_model: None,
                                    updated_permission_mode: Some(mode),
                                    handoff_summary: None,
                                }
                            }
                            CodingControlAction::BuildHandoffSummary {
                                target_bot_username,
                            } => CodingHostEvent::ControlResult {
                                req_id,
                                ok: true,
                                markdown_reply: "Handoff ready".to_string(),
                                updated_workspace: None,
                                updated_use_worktree: None,
                                updated_model: None,
                                updated_permission_mode: None,
                                handoff_summary: Some(format!(
                                    "Implemented parser fix for @{target_bot_username} review."
                                )),
                            },
                            _ => CodingHostEvent::ControlResult {
                                req_id,
                                ok: true,
                                markdown_reply: "ok".to_string(),
                                updated_workspace: None,
                                updated_use_worktree: None,
                                updated_model: None,
                                updated_permission_mode: None,
                                handoff_summary: None,
                            },
                        };
                        let _ = out_tx.send(WsClientFrame::CodingHostEvent(ev));
                    }
                    CodingHostCommand::StartTurn {
                        turn_id,
                        bot_user_id,
                        prompt,
                        ..
                    } => {
                        let _ = handoff_turn_tx.send((bot_user_id, prompt));
                        let _ = out_tx.send(WsClientFrame::CodingHostEvent(
                            CodingHostEvent::TurnCompleted {
                                turn_id,
                                ok: true,
                                final_text: "Reviewed handoff context.".to_string(),
                                error: None,
                                turn_idx: 1,
                                checkpoint_ref: None,
                                diff_stat: None,
                                token_usage: None,
                            },
                        ));
                    }
                    _ => {}
                }
            }
        }
    });

    // Test slash command persistence in Bot A's companion DM
    let dm_a = bot_a.companion_chat_id;
    for cmd_text in [
        "/workspace /repo/new",
        "/worktree on",
        "/model claude-opus-4",
        "/mode auto_edit",
    ] {
        let (st, _) = http_json(
            &router,
            "POST",
            &format!("/api/v1/chats/{dm_a}/messages"),
            Some(&alice_token),
            Some(json!({ "text": cmd_text })),
        )
        .await;
        assert!(st.is_success());
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    let mut persisted_ok = false;
    for _ in 0..40 {
        let (_, val_get) = http_json(
            &router,
            "GET",
            &format!("/api/v1/fabushi-bots/{}", bot_a.bot.bot_user_id),
            Some(&alice_token),
            None,
        )
        .await;
        let summary: FabushiBotSummaryDto = serde_json::from_value(val_get).unwrap();
        if let Some(cfg) = summary.coding_agent {
            if cfg.workspace_path == "/repo/new"
                && cfg.use_worktree
                && cfg.model_override.as_deref() == Some("claude-opus-4")
                && cfg.permission_mode == CodingPermissionMode::AutoEdit
            {
                persisted_ok = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        persisted_ok,
        "Slash commands (/workspace, /worktree on, /model, /mode) must persist coding_agent_json"
    );

    // Create shared group chat with both Bot A and Bot B, test `/handoff`
    let (st_g, val_g) = http_json(
        &router,
        "POST",
        "/api/v1/chats",
        Some(&alice_token),
        Some(json!({
            "title": "Coding Handoff Room",
            "chat_type": 1,
            "member_ids": [bot_a.bot.bot_user_id as u64, bot_b.bot.bot_user_id as u64]
        })),
    )
    .await;
    assert_eq!(st_g, StatusCode::CREATED);
    let group_cid = val_g["chat_id"].as_i64().unwrap();

    let (st_h, _) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{group_cid}/messages"),
        Some(&alice_token),
        Some(json!({
            "text": "@handoff_claude_bot /handoff @handoff_codex_bot check diff please"
        })),
    )
    .await;
    assert!(st_h.is_success());

    // Bot B (`handoff_codex_bot`) should receive StartTurn with the handoff summary from Bot A!
    let (target_bot_uid, received_prompt) =
        tokio::time::timeout(Duration::from_secs(3), handoff_turn_rx.recv())
            .await
            .expect("Bot B should be triggered via handoff")
            .expect("handoff turn received");
    assert_eq!(target_bot_uid, bot_b.bot.bot_user_id);
    assert!(
        received_prompt.contains("@handoff_codex_bot")
            && received_prompt.contains("Implemented parser fix"),
        "Handoff prompt to target bot: {received_prompt}"
    );

    // Verify GroupMultiAgentRoom loop budget recorded hops
    let hops = {
        let groups = state.multi_agent_groups.read();
        groups
            .get(&gpui_telegram_core::types::ChatId(group_cid))
            .map(|r| r.loop_budget.current_hop())
            .unwrap_or(0)
    };
    assert!(hops >= 2, "GroupMultiAgentRoom loop_budget must track hops: {hops}");

    responder_task.abort();
    server_handle.abort();
}

#[tokio::test]
async fn stale_approval_callback_after_turn_completed_fails_cleanly() {
    let (_dir, state, router) = boot_temp_server("coding_stale_approval");
    let (alice_uid, alice_token) = register_user(&router, "stale_apr_alice").await;
    let (addr, server_handle) = start_tcp_server(router.clone()).await;

    let mut ws = MockWsClient::connect(addr, &alice_token).await;
    ws.send_host_event(CodingHostEvent::HostRegister {
        device_id: "mac-stale-apr".to_string(),
        display_name: "Mac Stale".to_string(),
        os_name: "macos".to_string(),
        providers: vec![sample_provider(CodingProviderKind::ClaudeCode)],
    });
    for _ in 0..40 {
        if !state.coding_agent_broker.hosts_for_user(alice_uid).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    let (_, val_c) = http_json(
        &router,
        "POST",
        "/api/v1/fabushi-bots",
        Some(&alice_token),
        Some(
            serde_json::to_value(CreateFabushiBotRequest {
                display_name: "Stale Approval Bot".to_string(),
                username: Some("stale_apr_coder_bot".to_string()),
                coding_agent: Some(CodingAgentBotConfig {
                    provider_kind: CodingProviderKind::ClaudeCode,
                    workspace_path: "/repo/stale".to_string(),
                    bound_device_id: Some("mac-stale-apr".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap(),
        ),
    )
    .await;
    let created: FabushiBotCreateResponse = serde_json::from_value(val_c).unwrap();
    let cid = created.companion_chat_id;

    let _ = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{cid}/messages"),
        Some(&alice_token),
        Some(json!({ "text": "run risky command" })),
    )
    .await;

    let turn_id = match ws.recv_coding_command(Duration::from_secs(2)).await {
        Some(CodingHostCommand::StartTurn { turn_id, .. }) => turn_id,
        other => panic!("expected StartTurn, got {other:?}"),
    };

    // Host requests approval, then turn completes/cancels WITHOUT resolving the approval first
    ws.send_host_event(CodingHostEvent::TurnApprovalRequested {
        turn_id: turn_id.clone(),
        request_id: "apr_stale_1".to_string(),
        kind: "bash".to_string(),
        title: "Run deploy".to_string(),
        detail: "deploy.sh".to_string(),
        rule_key: "bash:deploy".to_string(),
    });

    let mut approval_msg_id = 0_i64;
    for _ in 0..40 {
        let msgs = list_chat_messages(&router, &alice_token, cid).await;
        if let Some(card) = msgs.iter().find(|m| {
            m["inline_keyboard"]
                .to_string()
                .contains(&format!("coding:approve:{turn_id}:apr_stale_1:once"))
        }) {
            approval_msg_id = card["id"].as_i64().unwrap();
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(approval_msg_id > 0, "Approval card must be posted");

    ws.send_host_event(CodingHostEvent::TurnCompleted {
        turn_id: turn_id.clone(),
        ok: false,
        final_text: String::new(),
        error: Some("Cancelled by user".to_string()),
        turn_idx: 1,
        checkpoint_ref: None,
        diff_stat: None,
        token_usage: None,
    });

    // Wait until TurnCompleted clears the stale approval card keyboard
    let mut cleared = false;
    for _ in 0..40 {
        let msgs = list_chat_messages(&router, &alice_token, cid).await;
        if let Some(m) = msgs.iter().find(|m| m["id"].as_i64() == Some(approval_msg_id)) {
            if m["inline_keyboard"] == json!([]) {
                cleared = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(cleared, "Approval card inline keyboard must be cleared on TurnCompleted");

    // Clicking the stale approval callback after TurnCompleted MUST fail cleanly and emit NO ResolveApproval
    let (st_stale, val_stale) = http_json(
        &router,
        "POST",
        &format!("/api/v1/chats/{cid}/callback"),
        Some(&alice_token),
        Some(json!({
            "message_id": approval_msg_id,
            "callback_data": format!("coding:approve:{turn_id}:apr_stale_1:once")
        })),
    )
    .await;
    assert_eq!(
        st_stale,
        StatusCode::BAD_REQUEST,
        "Stale approval callback after TurnCompleted must fail cleanly: {val_stale}"
    );
    assert!(
        ws.recv_coding_command(Duration::from_millis(150)).await.is_none(),
        "Stale approval callback must NEVER emit ResolveApproval to host"
    );

    server_handle.abort();
}
