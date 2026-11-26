//! End-to-end headless GPUI integration tests for Telegram-style instant loading & hydration:
//! 1. Cold startup with saved session + SQLite cache hydrates `is_authenticated`, `sidebar.dialogs`,
//!    `stage.active_chat_id`, and `stage.messages` synchronously on Frame 0 (zero login/empty flash).
//! 2. Cold startup with transient network/server outage keeps cached workspace open offline
//!    instead of wiping credentials or cached history.
//! 3. Cold startup with explicit `401 Unauthorized` from `/api/v1/auth/me` logs out and wipes cache.
//! 4. Brand-new login (cold cache) shows dialog skeletons + syncing stage placeholder while initial
//!    `refresh_chats` is in flight, then populates dialogs and proactively prefetches top chats.
//! 5. Opening an uncached chat sets `stage.is_loading_history = true` synchronously until server
//!    confirms history (even when empty), while opening a cached/prefetched chat renders instantly.
//! 6. `WindowConnectingWidget` enforces the 1200ms Telegram grace delay for transient handshakes
//!    and shows immediately for `WaitingForNetwork`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Application, Entity};
use gpui_telegram_client::net::hermes_client::HermesRichChatDto;
use gpui_telegram_client::{
    AuthResponse, ChatSummary, ClientConfig, HistoricalMessage, LocalStore, StoredSession,
    TelegramClient,
};
use gpui_telegram_desktop::views::app_shell::window_connecting_widget::{
    ConnectionStatus, WindowConnectingWidget, CONNECTING_BANNER_GRACE_MS,
};
use gpui_telegram_desktop::views::AppShell;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CHILD_ENV: &str = "GPUI_INSTANT_LOAD_TEST_CHILD";
const TARGET_ENV: &str = "GPUI_INSTANT_LOAD_TEST_TARGET";

fn sample_chat(chat_id: u64, title: &str, last_message: &str) -> ChatSummary {
    ChatSummary {
        chat_id,
        title: title.to_string(),
        chat_type: 0,
        last_message: Some(last_message.to_string()),
        unread_count: 0,
        is_group: Some(false),
        is_bot: Some(false),
        avatar_color: Some(0x2AABEE),
        updated_at: Some(1_730_000_000_000),
    }
}

fn sample_msg(id: u64, sender_id: u64, text: &str, is_outgoing: bool) -> HistoricalMessage {
    HistoricalMessage {
        message_id: id,
        sender_id,
        text: text.to_string(),
        content: None,
        date: Some(1_730_000_000_000 + (id as i64) * 1000),
        reply_to: None,
        is_outgoing: Some(is_outgoing),
        reply_to_info: None,
        reactions: Vec::new(),
        ttl_seconds: None,
        expires_at_ms: None,
        send_as_peer_id: None,
        reactions_authoritative: false,
    }
}

fn spawn_mock_server(
    rt: &tokio::runtime::Handle,
    handler: impl Fn(&str) -> (u16, String, u64) + Send + Sync + 'static,
) -> String {
    let handler = Arc::new(handler);
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock listener");
    std_listener.set_nonblocking(true).expect("set_nonblocking");
    let addr = std_listener.local_addr().expect("local_addr");
    let rt_handle = rt.clone();
    rt_handle.spawn(async move {
        let listener = TcpListener::from_std(std_listener).expect("tokio TcpListener");
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let h = Arc::clone(&handler);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let (status, body, delay_ms) = h(path);
                if delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
                let reason = match status {
                    200 => "OK",
                    401 => "Unauthorized",
                    404 => "Not Found",
                    503 => "Service Unavailable",
                    _ => "OK",
                };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

fn seed_cached_profile(dir: &std::path::Path, server_url: &str, user_id: u64, token: &str) {
    let store = LocalStore::open_with_dir(dir).expect("seed local store");
    store.bind_account_scope(server_url, user_id);
    store.replace_cached_dialog_summaries(&[
        sample_chat(101, "Alice Chen", "Cached hello from Alice"),
        sample_chat(202, "Design Team", "Cached update"),
    ]);
    let rich_101: HermesRichChatDto = serde_json::from_value(serde_json::json!({
        "chat_id": 101,
        "title": "Alice Chen",
        "is_pinned": true,
        "is_muted": false,
        "peer_user_id": 888,
        "peer_display_name": "Alice Chen"
    }))
    .unwrap();
    store.replace_cached_rich_chats(&[rich_101]);
    store.replace_cached_chat_history(
        101,
        &[
            sample_msg(1, 888, "Cached message 1", false),
            sample_msg(2, user_id, "Cached message 2", true),
        ],
        &[],
    );
    store.set_last_active_chat_id(Some(101));
    store.set_cached_user_display_name(Some("Alice Owner".to_string()));

    ClientConfig::save_session_in_dir(
        dir,
        &StoredSession {
            server_url: server_url.to_string(),
            user_id,
            username: "owner_u".to_string(),
            token: token.to_string(),
            session_id: "sid_cached".to_string(),
            created_at_ms: 1_730_000_000_000,
        },
    )
    .expect("seed session.json");
}

fn run_headless_target(target_key: &str) -> ! {
    let key = target_key.to_string();
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        Application::headless().run(move |cx: &mut App| {
            let key_inner = key.clone();
            cx.spawn(async move |app: &mut gpui::AsyncApp| {
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // Run synchronous or async test body via a blocking channel? No — run directly!
                }));
                let _ = res;
                let ok: Result<(), String> = match key_inner.as_str() {
                    "frame0_hydration" => body_cold_startup_frame0_hydration(app).await,
                    "offline_preservation" => body_cold_startup_offline_preserves_cache(app).await,
                    "unauthorized_401" => body_cold_startup_401_logs_out(app).await,
                    "cold_login_skeleton_prefetch" => {
                        body_cold_login_skeleton_and_prefetch(app).await
                    }
                    "uncached_chat_skeleton" => body_uncached_chat_skeleton(app).await,
                    other => Err(format!("unknown target: {other}")),
                };
                match ok {
                    Ok(()) => std::process::exit(0),
                    Err(msg) => {
                        eprintln!("instant loading test '{key_inner}' failed: {msg}");
                        std::process::exit(101);
                    }
                }
            })
            .detach();
        });
    }));
    match outer {
        Ok(()) => std::process::exit(0),
        Err(_) => std::process::exit(103),
    }
}

fn spawn_headless_test(test_name: &str, target_key: &str) {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        let tgt = std::env::var(TARGET_ENV).unwrap_or_else(|_| target_key.to_string());
        run_headless_target(&tgt);
    }
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let exe = std::env::current_exe().expect("current_exe");
    let out = std::process::Command::new(exe)
        .env(CHILD_ENV, "1")
        .env(TARGET_ENV, target_key)
        .env("GPUI_TELEGRAM_CONFIG_DIR", tmp.path())
        .env("GPUI_OFFLINE_TEST", "1")
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .output()
        .expect("spawn headless child");
    assert!(
        out.status.success(),
        "headless test '{target_key}' failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

async fn body_cold_startup_frame0_hydration(app: &mut gpui::AsyncApp) -> Result<(), String> {
    let dir = tempfile::TempDir::new().map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let server_url = spawn_mock_server(rt.handle(), |path| {
        if path.starts_with("/api/v1/auth/me") {
            let body = serde_json::json!({
                "user_id": 42,
                "username": "owner_u",
                "first_name": "Alice Verified",
                "session_id": "sid_cached"
            })
            .to_string();
            (200, body, 80)
        } else if path == "/api/v1/chats" {
            let body = serde_json::json!([
                { "chat_id": 101, "title": "Alice Chen", "chat_type": 0, "last_message": "Fresh from server", "unread_count": 0, "is_pinned": true },
                { "chat_id": 202, "title": "Design Team", "chat_type": 1, "last_message": "Fresh team msg", "unread_count": 1 }
            ])
            .to_string();
            (200, body, 80)
        } else if path.starts_with("/api/v1/chats/101/messages") {
            let body = serde_json::json!([
                { "message_id": 1, "sender_id": 888, "text": "Cached message 1", "date": 1730000001000i64, "is_outgoing": false },
                { "message_id": 2, "sender_id": 42, "text": "Cached message 2", "date": 1730000002000i64, "is_outgoing": true },
                { "message_id": 3, "sender_id": 888, "text": "Fresh message 3", "date": 1730000003000i64, "is_outgoing": false }
            ])
            .to_string();
            (200, body, 80)
        } else {
            (200, "[]".to_string(), 0)
        }
    });

    seed_cached_profile(dir.path(), &server_url, 42, "tok_valid");

    let mut cfg = ClientConfig::default();
    cfg.server_url = server_url.clone();
    let client = Arc::new(
        TelegramClient::open_with_dir(cfg, dir.path()).map_err(|e| e.to_string())?,
    );

    // Frame 0: synchronously construct AppShell and immediately verify FULL hydration before any network completes!
    let shell: Entity<AppShell> = app
        .update(|cx| {
            let s = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
            let sh = s.read(cx);
            if !sh.is_authenticated {
                return Err("Frame 0 must be authenticated immediately without login flash".into());
            }
            let sb = sh.sidebar.read(cx);
            if sb.dialogs.len() != 2 || sb.is_loading_dialogs {
                return Err(format!(
                    "Frame 0 sidebar must hydrate 2 cached dialogs without loading skeleton (got len={}, loading={})",
                    sb.dialogs.len(),
                    sb.is_loading_dialogs
                ));
            }
            if !sb.dialogs.iter().any(|d| d.id == 101 && d.is_pinned) {
                return Err("Frame 0 sidebar must restore pinned state from cached rich chats".into());
            }
            let st = sh.stage.read(cx);
            if st.active_chat_id != Some(101) {
                return Err(format!(
                    "Frame 0 stage must restore last_active_chat_id=101 (got {:?})",
                    st.active_chat_id
                ));
            }
            if st.messages.len() != 2 || st.is_loading_history {
                return Err(format!(
                    "Frame 0 stage must hydrate 2 cached messages without history skeleton (got len={}, loading={})",
                    st.messages.len(),
                    st.is_loading_history
                ));
            }
            if sh.window_runtime.connecting_widget.should_show_banner() {
                return Err("Frame 0 connecting pill must stay hidden during 1200ms grace period".into());
            }
            Ok(s)
        })
        .map_err(|e| e.to_string())??;

    // Wait for background get_me + refresh_chats + load_historical_messages to reconcile
    let mut reconciled = false;
    for _ in 0..80 {
        app.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let done = shell
            .read_with(app, |sh, cx| {
                sh.is_authenticated
                    && sh.stage.read(cx).active_chat_id == Some(101)
                    && sh.stage.read(cx).messages.len() == 3
                    && sh
                        .client
                        .local_store
                        .get_cached_user_display_name()
                        .as_deref()
                        == Some("Alice Verified")
            })
            .map_err(|e| e.to_string())?;
        if done {
            reconciled = true;
            break;
        }
    }
    if !reconciled {
        return Err("Background refresh did not seamlessly reconcile fresh messages & profile".into());
    }
    Ok(())
}

async fn body_cold_startup_offline_preserves_cache(app: &mut gpui::AsyncApp) -> Result<(), String> {
    let dir = tempfile::TempDir::new().map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    // Mock server returning 503 (transient backend outage)
    let server_url = spawn_mock_server(rt.handle(), |_| {
        (503, r#"{"error":"temporarily unavailable"}"#.to_string(), 10)
    });

    seed_cached_profile(dir.path(), &server_url, 42, "tok_offline");

    let mut cfg = ClientConfig::default();
    cfg.server_url = server_url.clone();
    let client = Arc::new(
        TelegramClient::open_with_dir(cfg, dir.path()).map_err(|e| e.to_string())?,
    );

    let shell: Entity<AppShell> = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .map_err(|e| e.to_string())?;

    // Wait for background get_me 503 response to be processed
    let mut saw_waiting = false;
    for _ in 0..60 {
        app.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let check = shell
            .read_with(app, |sh, cx| {
                let status = sh.window_runtime.connecting_widget.status;
                let banner_shown = sh.window_runtime.connecting_widget.should_show_banner();
                let auth = sh.is_authenticated;
                let dlgs = sh.sidebar.read(cx).dialogs.len();
                let msgs = sh.stage.read(cx).messages.len();
                (status, banner_shown, auth, dlgs, msgs)
            })
            .map_err(|e| e.to_string())?;
        if check.0 == ConnectionStatus::WaitingForNetwork {
            if !check.1 || !check.2 || check.3 != 2 || check.4 != 2 {
                return Err(format!(
                    "Transient outage must keep workspace open offline with WaitingForNetwork pill: {check:?}"
                ));
            }
            saw_waiting = true;
            break;
        }
    }
    if !saw_waiting {
        return Err("Expected WaitingForNetwork status after transient 503 outage".into());
    }
    if ClientConfig::load_session_in_dir(dir.path()).is_none() {
        return Err("Transient outage must NOT delete saved session.json".into());
    }
    Ok(())
}

async fn body_cold_startup_401_logs_out(app: &mut gpui::AsyncApp) -> Result<(), String> {
    let dir = tempfile::TempDir::new().map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let server_url = spawn_mock_server(rt.handle(), |path| {
        if path.starts_with("/api/v1/auth/me") {
            (401, r#"{"error":"token expired"}"#.to_string(), 20)
        } else {
            (401, r#"{"error":"unauthorized"}"#.to_string(), 0)
        }
    });

    seed_cached_profile(dir.path(), &server_url, 42, "tok_expired");

    let mut cfg = ClientConfig::default();
    cfg.server_url = server_url.clone();
    let client = Arc::new(
        TelegramClient::open_with_dir(cfg, dir.path()).map_err(|e| e.to_string())?,
    );

    let shell: Entity<AppShell> = app
        .update(|cx| cx.new(|cx| AppShell::new_with_client(client.clone(), cx)))
        .map_err(|e| e.to_string())?;

    let mut logged_out = false;
    for _ in 0..60 {
        app.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let state = shell
            .read_with(app, |sh, cx| {
                (
                    sh.is_authenticated,
                    sh.sidebar.read(cx).dialogs.is_empty(),
                    sh.stage.read(cx).messages.is_empty(),
                    sh.client.local_store.get_cached_dialog_summaries().is_empty(),
                )
            })
            .map_err(|e| e.to_string())?;
        if !state.0 && state.1 && state.2 && state.3 {
            logged_out = true;
            break;
        }
    }
    if !logged_out {
        return Err("401 Unauthorized from /api/v1/auth/me must log out and clear cache".into());
    }
    if ClientConfig::load_session_in_dir(dir.path()).is_some() {
        return Err("401 Unauthorized must delete saved session.json".into());
    }
    Ok(())
}

async fn body_cold_login_skeleton_and_prefetch(app: &mut gpui::AsyncApp) -> Result<(), String> {
    let dir = tempfile::TempDir::new().map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let server_url = spawn_mock_server(rt.handle(), |path| {
        if path == "/api/v1/chats" {
            let body = serde_json::json!([
                { "chat_id": 301, "title": "Alpha", "chat_type": 0, "last_message": "Hi Alpha", "unread_count": 0 },
                { "chat_id": 302, "title": "Beta", "chat_type": 0, "last_message": "Hi Beta", "unread_count": 0 }
            ])
            .to_string();
            (200, body, 120)
        } else if path.starts_with("/api/v1/chats/301/messages") {
            let body = serde_json::json!([
                { "message_id": 11, "sender_id": 501, "text": "Prefetched Alpha msg", "date": 1730000010000i64, "is_outgoing": false }
            ])
            .to_string();
            (200, body, 20)
        } else if path.starts_with("/api/v1/chats/302/messages") {
            let body = serde_json::json!([
                { "message_id": 22, "sender_id": 502, "text": "Prefetched Beta msg", "date": 1730000020000i64, "is_outgoing": false }
            ])
            .to_string();
            (200, body, 20)
        } else {
            (200, "[]".to_string(), 0)
        }
    });

    let mut cfg = ClientConfig::default();
    cfg.server_url = server_url;
    let client = Arc::new(
        TelegramClient::open_with_dir(cfg, dir.path()).map_err(|e| e.to_string())?,
    );

    let shell: Entity<AppShell> = app
        .update(|cx| {
            let s = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
            s.update(cx, |sh, cx| {
                sh.apply_authenticated_session(
                    AuthResponse {
                        user_id: 77,
                        username: "fresh_user".to_string(),
                        session_id: "sid_fresh".to_string(),
                        token: "tok_fresh".to_string(),
                        first_name: Some("Fresh".to_string()),
                        created_at: None,
                        expires_at: None,
                    },
                    cx,
                );
            });
            let sh = s.read(cx);
            let sb = sh.sidebar.read(cx);
            let st = sh.stage.read(cx);
            if !sb.is_loading_dialogs || sb.has_synced_dialogs {
                return Err(format!(
                    "Cold login must immediately show sidebar loading skeletons (loading={}, synced={})",
                    sb.is_loading_dialogs, sb.has_synced_dialogs
                ));
            }
            if !st.is_loading_workspace {
                return Err("Cold login must immediately show syncing workspace placeholder".into());
            }
            Ok(s)
        })
        .map_err(|e| e.to_string())??;

    // Wait for refresh_chats + background prefetch of chat 301 & 302
    let mut prefetched = false;
    for _ in 0..80 {
        app.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let ready = shell
            .read_with(app, |sh, cx| {
                let sb = sh.sidebar.read(cx);
                let st = sh.stage.read(cx);
                !sb.is_loading_dialogs
                    && sb.has_synced_dialogs
                    && sb.dialogs.len() == 2
                    && !st.is_loading_workspace
                    && sh.client.local_store.get_cached_chat_history(301).is_some()
                    && sh.client.local_store.get_cached_chat_history(302).is_some()
            })
            .map_err(|e| e.to_string())?;
        if ready {
            prefetched = true;
            break;
        }
    }
    if !prefetched {
        return Err("Expected refresh_chats + background top-chat prefetch to populate LocalStore".into());
    }

    // Clicking prefetched chat 302 MUST display messages synchronously on Frame 0 with no skeleton!
    shell
        .update(app, |sh, cx| {
            sh.sync_selected_chat_inspector_and_stage(302, cx);
            let st = sh.stage.read(cx);
            if st.active_chat_id != Some(302) || st.messages.len() != 1 || st.is_loading_history {
                return Err(format!(
                    "Prefetched chat 302 must open instantly with 1 message and no skeleton (active={:?}, msgs={}, loading={})",
                    st.active_chat_id,
                    st.messages.len(),
                    st.is_loading_history
                ));
            }
            Ok(())
        })
        .map_err(|e| e.to_string())??;

    Ok(())
}

async fn body_uncached_chat_skeleton(app: &mut gpui::AsyncApp) -> Result<(), String> {
    let dir = tempfile::TempDir::new().map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let server_url = spawn_mock_server(rt.handle(), |path| {
        if path == "/api/v1/chats" {
            let body = serde_json::json!([
                { "chat_id": 404, "title": "Empty Uncached Chat", "chat_type": 0, "unread_count": 0 }
            ])
            .to_string();
            (200, body, 10)
        } else if path.starts_with("/api/v1/chats/404/messages") {
            // Deliberately slow response confirming 0 messages
            (200, "[]".to_string(), 160)
        } else {
            (200, "[]".to_string(), 0)
        }
    });

    let mut cfg = ClientConfig::default();
    cfg.server_url = server_url;
    let client = Arc::new(
        TelegramClient::open_with_dir(cfg, dir.path()).map_err(|e| e.to_string())?,
    );

    let shell: Entity<AppShell> = app
        .update(|cx| {
            let s = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
            s.update(cx, |sh, cx| {
                sh.session_sync
                    .begin_session("tok_404".into(), "sid_404".into(), 91, "u91".into());
                sh.is_authenticated = true;
                sh.client.set_auth_session(AuthResponse {
                    user_id: 91,
                    username: "u91".into(),
                    session_id: "sid_404".into(),
                    token: "tok_404".into(),
                    first_name: None,
                    created_at: None,
                    expires_at: None,
                });
                sh.sidebar.update(cx, |sb, _| {
                    sb.dialogs.push(gpui_telegram_desktop::views::chat_sidebar::DialogItemModel {
                        id: 404,
                        peer_name: "Empty Uncached Chat".to_string(),
                        avatar_initials: "EU".to_string(),
                        avatar_color: 0x2AABEE,
                        last_message: String::new(),
                        timestamp: String::new(),
                        unread_count: 0,
                        is_muted: false,
                        is_pinned: false,
                        is_online: false,
                        typing_indicator: None,
                        delivery_status: gpui_telegram_desktop::views::chat_sidebar::DialogDeliveryStatus::Read,
                        folder: "All".to_string(),
                    });
                });
                sh.sync_selected_chat_inspector_and_stage(404, cx);
            });
            let st = s.read(cx).stage.read(cx);
            if st.active_chat_id != Some(404) || !st.is_loading_history {
                return Err(format!(
                    "Uncached chat 404 must immediately enter is_loading_history=true skeleton state (active={:?}, loading={})",
                    st.active_chat_id, st.is_loading_history
                ));
            }
            Ok(s)
        })
        .map_err(|e| e.to_string())??;

    // Wait for delayed server response confirming 0 messages
    let mut confirmed_empty = false;
    for _ in 0..80 {
        app.background_executor()
            .timer(Duration::from_millis(25))
            .await;
        let done = shell
            .read_with(app, |sh, cx| {
                let st = sh.stage.read(cx);
                !st.is_loading_history && st.loaded_history_chats.contains(&404) && st.messages.is_empty()
            })
            .map_err(|e| e.to_string())?;
        if done {
            confirmed_empty = true;
            break;
        }
    }
    if !confirmed_empty {
        return Err("Stage did not clear is_loading_history after server confirmed empty chat".into());
    }
    Ok(())
}

#[test]
fn connecting_widget_grace_delay_and_localization() {
    let mut w = WindowConnectingWidget::new();
    assert!(!w.should_show_banner());

    // Connecting/Updating/ProxyConnecting wait for the 1200ms Telegram grace window
    w.set_status(ConnectionStatus::Connecting);
    assert!(w.is_visible);
    assert!(!w.should_show_banner(), "must not flash during initial grace delay");
    assert_eq!(w.localized_status_text("zh-CN"), "连接中…");
    assert_eq!(w.localized_status_text("en"), "Connecting...");

    // Backdate status_changed_at past grace window -> banner now shows
    w.status_changed_at = Instant::now() - Duration::from_millis(CONNECTING_BANNER_GRACE_MS + 50);
    assert!(w.should_show_banner());

    // WaitingForNetwork shows immediately with zero grace delay
    w.set_status(ConnectionStatus::WaitingForNetwork);
    assert!(w.should_show_banner());
    assert_eq!(w.localized_status_text("zh-CN"), "等待网络…");

    // Connected hides immediately
    w.set_status(ConnectionStatus::Connected);
    assert!(!w.is_visible);
    assert!(!w.should_show_banner());
}

#[test]
fn cold_startup_hydrates_session_dialogs_active_chat_and_history_on_frame_0() {
    spawn_headless_test(
        "cold_startup_hydrates_session_dialogs_active_chat_and_history_on_frame_0",
        "frame0_hydration",
    );
}

#[test]
fn cold_startup_transient_network_error_preserves_cached_workspace_offline() {
    spawn_headless_test(
        "cold_startup_transient_network_error_preserves_cached_workspace_offline",
        "offline_preservation",
    );
}

#[test]
fn cold_startup_401_unauthorized_logs_out_and_clears_cache() {
    spawn_headless_test(
        "cold_startup_401_unauthorized_logs_out_and_clears_cache",
        "unauthorized_401",
    );
}

#[test]
fn fresh_login_cold_cache_shows_skeletons_then_hydrates_and_prefetches_top_chats() {
    spawn_headless_test(
        "fresh_login_cold_cache_shows_skeletons_then_hydrates_and_prefetches_top_chats",
        "cold_login_skeleton_prefetch",
    );
}

#[test]
fn opening_uncached_chat_sets_loading_history_skeleton_until_server_confirms() {
    spawn_headless_test(
        "opening_uncached_chat_sets_loading_history_skeleton_until_server_confirms",
        "uncached_chat_skeleton",
    );
}
