use axum::Router;
use base64::Engine as _;
use fabushi_remote_engine::ipc::{BoxFuture, LocalIpcHandler, LocalIpcServer};
use gpui_telegram_core::action_bus::{
    register_full_app_actions, ActionRegistry, CliExporter, CliSchemaExport, HermesToolDefinition,
    HermesToolExporter, McpExporter, McpToolsListResponse, OpenApiExporter,
};
use gpui_telegram_proto::remote_control_contracts::{LocalIpcRequest, LocalIpcResponse};
use gpui_telegram_server::{build_router_with_state, server::AppState, ServerConfig};
use std::sync::{Arc, Mutex};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::{Command, Output};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const CLI_BIN: &str = env!("CARGO_BIN_EXE_fabushi");

fn base_cmd() -> Command {
    let mut cmd = Command::new(CLI_BIN);
    cmd.env_remove("GPUI_TELEGRAM_SERVER_URL")
        .env_remove("FABUSHI_SERVER_URL")
        .env_remove("GPUI_TELEGRAM_TOKEN")
        .env_remove("GPUI_TG_TOKEN")
        .env_remove("FABUSHI_TOKEN")
        .env_remove("FABUSHI_IPC_SOCKET")
        .env_remove("FABUSHI_CLI_SESSION_PATH")
        .env_remove("FABUSHI_IN_MUX");
    cmd
}

fn run_cli_sync(args: &[&str]) -> Output {
    base_cmd()
        .args(args)
        .output()
        .expect("spawn fabushi binary")
}

fn run_cli_with_env(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = base_cmd();
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn fabushi binary")
}

async fn run_cli_async(args: Vec<String>) -> Output {
    tokio::task::spawn_blocking(move || {
        base_cmd()
            .args(&args)
            .output()
            .expect("spawn fabushi binary")
    })
    .await
    .expect("spawn_blocking join")
}

async fn run_cli_async_with_env(args: Vec<String>, envs: Vec<(String, String)>) -> Output {
    tokio::task::spawn_blocking(move || {
        let mut cmd = base_cmd();
        cmd.args(&args);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.output().expect("spawn fabushi binary")
    })
    .await
    .expect("spawn_blocking join")
}

fn stdout_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

struct EphemeralServer {
    _dir: TempDir,
    base_url: String,
    state: Option<Arc<AppState>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl EphemeralServer {
    async fn start() -> Self {
        let dir = TempDir::new().expect("create temp dir");
        let cfg = ServerConfig {
            database_url: dir.path().join("cli_test.db").to_string_lossy().into_owned(),
            media_storage_path: dir.path().join("cas").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let state = Arc::new(AppState::try_new(cfg).expect("init AppState"));
        let router: Router = build_router_with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral TcpListener");
        let addr = listener.local_addr().expect("local_addr");
        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });

        Self {
            _dir: dir,
            base_url: format!("http://{addr}"),
            state: Some(state),
            shutdown_tx: Some(tx),
            task: Some(task),
        }
    }

    async fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        if let Some(state) = self.state.take() {
            let weak_state = Arc::downgrade(&state);
            let weak_hub = Arc::downgrade(&state.gateway_hub);
            let weak_idx = Arc::downgrade(&state.search_indexer);
            drop(state);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while (weak_state.strong_count() > 0
                || weak_hub.strong_count() > 0
                || weak_idx.strong_count() > 0)
                && std::time::Instant::now() < deadline
            {
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            }
        }
    }
}

async fn register_user(http: &Client, base_url: &str, username: &str) -> (u64, String) {
    let resp = http
        .post(format!("{base_url}/api/v1/auth/register"))
        .json(&json!({
            "username": username,
            "password": "CliTestPassword#2026",
            "first_name": username,
            "device_name": "CliIntegrationTest"
        }))
        .send()
        .await
        .expect("register request");
    assert!(resp.status().is_success(), "register failed: {}", resp.status());
    let body: Value = resp.json().await.expect("parse register json");
    let uid = body["user_id"].as_u64().expect("user_id");
    let tok = body["token"].as_str().expect("token").to_string();
    (uid, tok)
}

async fn create_group_chat(
    http: &Client,
    base_url: &str,
    token: &str,
    title: &str,
    member_ids: Vec<u64>,
) -> i64 {
    let resp = http
        .post(format!("{base_url}/api/v1/chats"))
        .bearer_auth(token)
        .json(&json!({
            "title": title,
            "chat_type": "group",
            "member_ids": member_ids
        }))
        .send()
        .await
        .expect("create chat request");
    assert!(resp.status().is_success(), "create chat failed: {}", resp.status());
    let body: Value = resp.json().await.expect("parse chat json");
    body["chat_id"].as_i64().expect("chat_id")
}

struct MockCliIpcHandler {
    sessions: Mutex<Vec<String>>,
    clipboard: Mutex<String>,
    files: Mutex<HashMap<String, String>>,
}

impl MockCliIpcHandler {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(vec!["main".to_string()]),
            clipboard: Mutex::new("initial-clip".to_string()),
            files: Mutex::new(HashMap::new()),
        }
    }
}

impl LocalIpcHandler for MockCliIpcHandler {
    fn handle(&self, req: LocalIpcRequest) -> BoxFuture<'_, LocalIpcResponse> {
        Box::pin(async move {
            let devices = vec![
                json!({
                    "node_id": "local:1:studio",
                    "display_name": "My Mac Studio",
                    "device_id": "mac-studio",
                    "os_name": "macOS",
                    "online": true,
                    "is_current_device": true
                }),
                json!({
                    "node_id": "cloud:1:ubuntu",
                    "display_name": "Build Ubuntu Node",
                    "host": "10.0.0.42",
                    "os_name": "Linux",
                    "online": true,
                    "is_current_device": false,
                    "kind": "cloud_server"
                }),
            ];

            match req.method.as_str() {
                "status" => LocalIpcResponse::ok(
                    req.id,
                    json!({ "service": "mock-desktop", "healthy": true }),
                ),
                "echo" => LocalIpcResponse::ok(req.id, req.params),
                "device.list" => LocalIpcResponse::ok(req.id, json!({ "computers": devices })),
                "device.info" => {
                    let nid = req.params["node_id"].as_str().unwrap_or("");
                    if let Some(d) = devices.into_iter().find(|d| d["node_id"] == nid) {
                        LocalIpcResponse::ok(req.id, d)
                    } else {
                        LocalIpcResponse::err(req.id, "not_found", "device not found")
                    }
                }
                "screen.capture" => {
                    let png_bytes = b"\x89PNG\r\n\x1a\nMOCK_PNG";
                    let b64 = base64::engine::general_purpose::STANDARD.encode(png_bytes);
                    LocalIpcResponse::ok(
                        req.id,
                        json!({
                            "node_id": req.params["node_id"],
                            "width": 1920,
                            "height": 1080,
                            "image_b64": b64,
                        }),
                    )
                }
                "input.click" | "input.mouse" => LocalIpcResponse::ok(
                    req.id,
                    json!({ "ok": true, "clicked": req.params }),
                ),
                "input.type" | "input.hotkey" | "input.keyboard" => LocalIpcResponse::ok(
                    req.id,
                    json!({ "ok": true, "keyboard": req.params }),
                ),
                "clipboard.set" => {
                    let text = req.params["text"].as_str().unwrap_or("").to_string();
                    *self.clipboard.lock().unwrap() = text.clone();
                    LocalIpcResponse::ok(req.id, json!({ "ok": true, "text": text }))
                }
                "clipboard.get" => {
                    let text = self.clipboard.lock().unwrap().clone();
                    LocalIpcResponse::ok(req.id, json!({ "mime_type": "text/plain", "text": text }))
                }
                "file.push" => {
                    let remote = req.params["remote_path"].as_str().unwrap_or("").to_string();
                    let b64 = req.params["data_b64"].as_str().unwrap_or("");
                    let decoded = base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .unwrap_or_default();
                    let text = String::from_utf8_lossy(&decoded).into_owned();
                    self.files.lock().unwrap().insert(remote.clone(), text);
                    LocalIpcResponse::ok(req.id, json!({ "pushed": true, "remote_path": remote }))
                }
                "file.pull" => {
                    let remote = req.params["remote_path"].as_str().unwrap_or("");
                    if let Some(content) = self.files.lock().unwrap().get(remote).cloned() {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(content.as_bytes());
                        LocalIpcResponse::ok(req.id, json!({ "data_b64": b64, "remote_path": remote }))
                    } else {
                        LocalIpcResponse::err(req.id, "not_found", "remote file not found")
                    }
                }
                "lterm.ls" => {
                    let list = self.sessions.lock().unwrap().clone();
                    LocalIpcResponse::ok(req.id, json!({ "sessions": list }))
                }
                "lterm.has" => {
                    let name = req.params["name"].as_str().unwrap_or("");
                    let exists = self.sessions.lock().unwrap().iter().any(|s| s == name);
                    LocalIpcResponse::ok(req.id, json!({ "name": name, "exists": exists }))
                }
                "lterm.new" => {
                    let name = req.params["name"]
                        .as_str()
                        .unwrap_or("term-1")
                        .to_string();
                    let mut guard = self.sessions.lock().unwrap();
                    if !guard.contains(&name) {
                        guard.push(name.clone());
                    }
                    LocalIpcResponse::ok(req.id, json!({ "name": name, "alive": true }))
                }
                "lterm.rename" => {
                    let old_n = req.params["old_name"].as_str().unwrap_or("");
                    let new_n = req.params["new_name"].as_str().unwrap_or("");
                    let mut guard = self.sessions.lock().unwrap();
                    if let Some(item) = guard.iter_mut().find(|s| s.as_str() == old_n) {
                        *item = new_n.to_string();
                        LocalIpcResponse::ok(req.id, json!({ "renamed": true, "name": new_n }))
                    } else {
                        LocalIpcResponse::err(req.id, "not_found", "session missing")
                    }
                }
                "lterm.kill" => {
                    let name = req.params["name"].as_str().unwrap_or("");
                    let mut guard = self.sessions.lock().unwrap();
                    let before = guard.len();
                    guard.retain(|s| s != name);
                    LocalIpcResponse::ok(req.id, json!({ "killed": guard.len() < before }))
                }
                other => LocalIpcResponse::err(
                    req.id,
                    "method_not_found",
                    format!("unknown mock IPC method '{other}'"),
                ),
            }
        })
    }
}

#[test]
fn test_cli_help_and_version() {
    for flag in ["--help", "-h", "help"] {
        let out = run_cli_sync(&[flag]);
        assert!(
            out.status.success(),
            "'{flag}' should exit 0, stderr: {}",
            stderr_str(&out)
        );
        let text = stdout_str(&out);
        for needle in [
            "fabushi",
            "export-schema",
            "list",
            "schema",
            "approve",
            "send-message",
            "get-chat-history",
            "list-dialogs",
            "delete-message",
            "device",
            "cloudpc",
            "assist",
            "lterm",
            "screen",
            "mcp",
        ] {
            assert!(
                text.contains(needle),
                "Help output for '{flag}' missing '{needle}':\n{text}"
            );
        }
    }

    for ver_flag in ["--version", "-V"] {
        let ver = run_cli_sync(&[ver_flag]);
        assert!(ver.status.success(), "'{ver_flag}' failed");
        let text = stdout_str(&ver);
        assert!(text.contains("fabushi"));
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
    }

    for sub_help in [
        vec!["send-message", "--help"],
        vec!["messaging", "send-message", "--help"],
        vec!["help", "send-message"],
    ] {
        let out = run_cli_sync(&sub_help);
        assert!(out.status.success(), "{sub_help:?} failed");
        let text = stdout_str(&out);
        for flag in [
            "--chat-id",
            "--text",
            "--reply-to-id",
            "--topic-id",
            "--silent",
            "--client-nonce",
        ] {
            assert!(
                text.contains(flag),
                "Action help {sub_help:?} missing flag '{flag}':\n{text}"
            );
        }
    }
}

#[test]
fn test_cli_export_schema_matches_cli_exporter() {
    let reg = ActionRegistry::new();
    register_full_app_actions(&reg).unwrap();

    let out = run_cli_sync(&["export-schema"]);
    assert!(out.status.success(), "export-schema failed: {}", stderr_str(&out));
    let parsed: CliSchemaExport =
        serde_json::from_str(&stdout_str(&out)).expect("parse export-schema cli JSON");
    let expected = CliExporter::export_cli_schema(&reg, "fabushi", env!("CARGO_PKG_VERSION"));
    assert_eq!(parsed, expected);

    let openapi_out = run_cli_sync(&["export-schema", "--format", "openapi"]);
    assert!(openapi_out.status.success());
    let openapi_val: Value = serde_json::from_str(&stdout_str(&openapi_out)).unwrap();
    let expected_openapi = OpenApiExporter::export_spec(
        &reg,
        "Fabushi Platform Action Bus",
        env!("CARGO_PKG_VERSION"),
        "http://127.0.0.1:18080",
    );
    assert_eq!(openapi_val, expected_openapi);

    let mcp_out = run_cli_sync(&["export-schema", "--format", "mcp"]);
    assert!(mcp_out.status.success());
    let mcp_val: McpToolsListResponse = serde_json::from_str(&stdout_str(&mcp_out)).unwrap();
    assert_eq!(mcp_val, McpExporter::export_tools_list(&reg));

    let hermes_out = run_cli_sync(&["export-schema", "--format", "hermes"]);
    assert!(hermes_out.status.success());
    let hermes_val: Vec<HermesToolDefinition> =
        serde_json::from_str(&stdout_str(&hermes_out)).unwrap();
    assert_eq!(hermes_val, HermesToolExporter::export_tools(&reg));
}

#[test]
fn test_cli_local_list_and_schema_subcommands() {
    let reg = ActionRegistry::new();
    register_full_app_actions(&reg).unwrap();

    let list_json_out = run_cli_sync(&["list", "--local", "--json"]);
    assert!(
        list_json_out.status.success(),
        "list --local --json failed: {}",
        stderr_str(&list_json_out)
    );
    let list_val: Value = serde_json::from_str(&stdout_str(&list_json_out)).unwrap();
    let actions = list_val["actions"].as_array().expect("actions array");
    assert_eq!(actions.len(), reg.count());

    let del = actions.iter().find(|a| a["name"] == "delete_message").unwrap();
    assert_eq!(del["requires_approval"], true);
    let send = actions.iter().find(|a| a["name"] == "send_message").unwrap();
    assert_eq!(send["requires_approval"], false);

    let list_text_out = run_cli_sync(&["list", "--local"]);
    assert!(list_text_out.status.success());
    let table = stdout_str(&list_text_out);
    assert!(table.contains("send-message"));
    assert!(table.contains("delete-message"));
    assert!(table.contains("device-list"));

    let dialogs_out = run_cli_sync(&["list", "--local", "--category", "dialogs", "--json"]);
    assert!(dialogs_out.status.success());
    let dialogs_val: Value = serde_json::from_str(&stdout_str(&dialogs_out)).unwrap();
    let dialog_actions = dialogs_val["actions"].as_array().unwrap();
    assert_eq!(dialog_actions.len(), 1);
    assert_eq!(dialog_actions[0]["name"], "list_dialogs");

    let schema_send = run_cli_sync(&["schema", "send-message"]);
    assert!(schema_send.status.success());
    let schema_send_val: Value = serde_json::from_str(&stdout_str(&schema_send)).unwrap();
    assert_eq!(schema_send_val["name"], "send_message");
    assert_eq!(schema_send_val["requires_approval"], false);
    assert!(schema_send_val["input_schema"].is_object());
    assert!(schema_send_val["output_schema"].is_object());

    let schema_del = run_cli_sync(&["schema", "delete_message"]);
    assert!(schema_del.status.success());
    let schema_del_val: Value = serde_json::from_str(&stdout_str(&schema_del)).unwrap();
    assert_eq!(schema_del_val["requires_approval"], true);

    let bad_schema = run_cli_sync(&["schema", "unknown_action"]);
    assert!(!bad_schema.status.success());
}

#[test]
fn test_cli_local_action_bus_execution_and_validation() {
    let send_out = run_cli_sync(&[
        "send-message",
        "--local",
        "--user-id",
        "42",
        "--chat-id",
        "1001",
        "--text",
        "Hello CLI",
    ]);
    assert!(
        send_out.status.success(),
        "send-message --local failed: {}",
        stderr_str(&send_out)
    );
    let send_val: Value = serde_json::from_str(&stdout_str(&send_out)).unwrap();
    assert_eq!(send_val["chat_id"], 1001);
    assert!(send_val["message_id"].as_i64().unwrap() >= 1);
    assert!(send_val["pts"].as_u64().unwrap() >= 1);

    let hist_out = run_cli_sync(&[
        "get-chat-history",
        "--local",
        "--user-id",
        "42",
        "--chat-id",
        "1001",
        "--limit",
        "10",
    ]);
    assert!(hist_out.status.success());
    let hist_val: Value = serde_json::from_str(&stdout_str(&hist_out)).unwrap();
    assert_eq!(hist_val["chat_id"], 1001);
    assert!(hist_val["messages"].is_array());

    let dialogs_out = run_cli_sync(&[
        "dialogs",
        "list-dialogs",
        "--local",
        "--user-id",
        "42",
        "--limit",
        "5",
    ]);
    assert!(dialogs_out.status.success());
    let dialogs_val: Value = serde_json::from_str(&stdout_str(&dialogs_out)).unwrap();
    assert!(dialogs_val["dialogs"].is_array());

    let del_no_approve = run_cli_sync(&[
        "delete-message",
        "--local",
        "--user-id",
        "42",
        "--chat-id",
        "1001",
        "--message-ids",
        "1,2",
    ]);
    assert!(!del_no_approve.status.success());
    assert_eq!(del_no_approve.status.code(), Some(4));

    let del_approved = run_cli_sync(&[
        "delete-message",
        "--local",
        "--user-id",
        "42",
        "--chat-id",
        "1001",
        "--message-ids",
        "1,2",
        "--approve",
    ]);
    assert!(
        del_approved.status.success(),
        "delete-message --approve failed: {}",
        stderr_str(&del_approved)
    );
    let del_val: Value = serde_json::from_str(&stdout_str(&del_approved)).unwrap();
    assert_eq!(del_val["deleted_count"], 2);

    let empty_text = run_cli_sync(&[
        "send-message",
        "--local",
        "--chat-id",
        "1001",
        "--text",
        "   ",
    ]);
    assert!(!empty_text.status.success());
    assert_eq!(empty_text.status.code(), Some(2));

    let missing_chat = run_cli_sync(&["send-message", "--local", "--text", "hi"]);
    assert!(!missing_chat.status.success());
    assert_eq!(missing_chat.status.code(), Some(2));

    let unknown_flag = run_cli_sync(&[
        "send-message",
        "--local",
        "--chat-id",
        "1001",
        "--text",
        "hi",
        "--bogus",
        "1",
    ]);
    assert!(!unknown_flag.status.success());
    assert_eq!(unknown_flag.status.code(), Some(2));

    let unknown_json_field = run_cli_sync(&[
        "send-message",
        "--local",
        "--json",
        r#"{"chat_id": 1001, "text": "hi", "unknown_field": true}"#,
    ]);
    assert!(!unknown_json_field.status.success());
    assert_eq!(unknown_json_field.status.code(), Some(2));
}

#[test]
fn test_cli_unified_envelope_mcp_config_and_nested_mux_refusal() {
    // 1. Uniform JSON envelope on `version` and `echo`
    let ver_env = run_cli_sync(&["version"]);
    assert!(ver_env.status.success(), "stderr: {}", stderr_str(&ver_env));
    let ver_json: Value = serde_json::from_str(&stdout_str(&ver_env)).unwrap();
    assert_eq!(ver_json["success"], true);
    assert_eq!(ver_json["data"]["program"], "fabushi");
    assert!(ver_json["error"].is_null());
    assert!(ver_json["timestamp"].as_str().is_some_and(|s| !s.is_empty()));

    let echo_out = run_cli_sync(&["echo", "hello", "fabushi"]);
    assert!(echo_out.status.success());
    let echo_json: Value = serde_json::from_str(&stdout_str(&echo_out)).unwrap();
    assert_eq!(echo_json["success"], true);
    assert_eq!(echo_json["data"]["echo"]["message"], "hello fabushi");
    assert!(echo_json["error"].is_null());

    // 2. Uniform JSON error envelope
    let err_out = run_cli_sync(&["definitely-not-a-command"]);
    assert!(!err_out.status.success());
    assert_eq!(err_out.status.code(), Some(2));
    let err_json: Value = serde_json::from_str(&stderr_str(&err_out)).unwrap();
    assert_eq!(err_json["success"], false);
    assert!(err_json["data"].is_null());
    assert_eq!(err_json["error"]["code"], "unknown_command");
    assert!(err_json["timestamp"].as_str().is_some_and(|s| !s.is_empty()));

    // 3. MCP config generation across all 4 clients
    for client in ["antigravity", "claude", "cursor", "vscode"] {
        let cfg_out = run_cli_sync(&[
            "mcp",
            "config",
            "--client",
            client,
            "--url",
            "https://mcp.ombhrum.com",
        ]);
        assert!(
            cfg_out.status.success(),
            "mcp config {client} failed: {}",
            stderr_str(&cfg_out)
        );
        let cfg_json: Value = serde_json::from_str(&stdout_str(&cfg_out)).unwrap();
        assert_eq!(cfg_json["success"], true);
        assert_eq!(cfg_json["data"]["client"], client);
    }

    // 4. FABUSHI_IN_MUX=1 recursive attach/new refusal
    for sub in ["attach", "new"] {
        let mux_out = run_cli_with_env(&["lterm", sub, "nested"], &[("FABUSHI_IN_MUX", "1")]);
        assert!(
            !mux_out.status.success(),
            "lterm {sub} must refuse when FABUSHI_IN_MUX=1"
        );
        let err_text = stderr_str(&mux_out);
        let err_val: Value = serde_json::from_str(&err_text)
            .unwrap_or_else(|e| panic!("Expected JSON error, got '{err_text}': {e}"));
        assert_eq!(err_val["success"], false);
        assert!(err_text.contains("FABUSHI_IN_MUX=1"));
    }

    // 5. lterm has when IPC is offline -> exit code 2 (error)
    let dir = TempDir::new().unwrap();
    let dead_sock = dir.path().join("missing.sock");
    let has_err = run_cli_with_env(
        &["lterm", "has", "main"],
        &[("FABUSHI_IPC_SOCKET", dead_sock.to_str().unwrap())],
    );
    assert_eq!(has_err.status.code(), Some(2));
}

#[tokio::test]
async fn test_cli_ipc_roundtrip_fuzzy_match_lterm_and_screen_file_ops() {
    let dir = TempDir::new().expect("temp dir");
    let sock = dir.path().join("fabushi-ipc.sock");
    let handler = Arc::new(MockCliIpcHandler::new());
    let server = LocalIpcServer::bind(sock.clone(), handler)
        .await
        .expect("bind mock LocalIpcServer");

    let sock_str = sock.to_string_lossy().into_owned();

    // 1. status over IPC
    let status_out = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "status".into(),
    ])
    .await;
    assert!(status_out.status.success(), "stderr: {}", stderr_str(&status_out));
    let status_val: Value = serde_json::from_str(&stdout_str(&status_out)).unwrap();
    assert_eq!(status_val["success"], true);
    assert_eq!(status_val["data"]["ipc"]["connected"], true);

    // 2. device info --m fuzzy matching ("studio" -> "My Mac Studio")
    let info_out = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "device".into(),
        "info".into(),
        "--m".into(),
        "studio".into(),
    ])
    .await;
    assert!(info_out.status.success(), "stderr: {}", stderr_str(&info_out));
    let info_val: Value = serde_json::from_str(&stdout_str(&info_out)).unwrap();
    assert_eq!(info_val["success"], true);
    assert_eq!(info_val["data"]["node_id"], "local:1:studio");

    // 3. screen capture --m studio --out <path>
    let shot_path = dir.path().join("capture.png");
    let shot_out = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "screen".into(),
        "capture".into(),
        "--m".into(),
        "studio".into(),
        "--out".into(),
        shot_path.to_string_lossy().into_owned(),
    ])
    .await;
    assert!(shot_out.status.success(), "stderr: {}", stderr_str(&shot_out));
    assert!(shot_path.is_file());
    assert_eq!(std::fs::read(&shot_path).unwrap(), b"\x89PNG\r\n\x1a\nMOCK_PNG");

    // 4. clipboard set & get via --m
    let clip_set = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "clipboard".into(),
        "set".into(),
        "--m".into(),
        "studio".into(),
        "hello-from-cli".into(),
    ])
    .await;
    assert!(clip_set.status.success(), "stderr: {}", stderr_str(&clip_set));
    let clip_get = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "clipboard".into(),
        "get".into(),
        "--m".into(),
        "studio".into(),
    ])
    .await;
    assert!(clip_get.status.success());
    let clip_val: Value = serde_json::from_str(&stdout_str(&clip_get)).unwrap();
    assert_eq!(clip_val["data"]["text"], "hello-from-cli");

    // 5. file push & pull via --m
    let local_src = dir.path().join("upload.txt");
    let local_dst = dir.path().join("downloaded.txt");
    std::fs::write(&local_src, "fabushi-file-sync-ok").unwrap();
    let push_out = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "file".into(),
        "push".into(),
        "--m".into(),
        "studio".into(),
        local_src.to_string_lossy().into_owned(),
        "/remote/upload.txt".into(),
    ])
    .await;
    assert!(push_out.status.success(), "stderr: {}", stderr_str(&push_out));
    let pull_out = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "file".into(),
        "pull".into(),
        "--m".into(),
        "studio".into(),
        "/remote/upload.txt".into(),
        local_dst.to_string_lossy().into_owned(),
    ])
    .await;
    assert!(pull_out.status.success(), "stderr: {}", stderr_str(&pull_out));
    assert_eq!(std::fs::read_to_string(&local_dst).unwrap(), "fabushi-file-sync-ok");

    // 6. lterm has exact exit codes (0 exists, 1 missing) + new/rename/kill
    let has_ok = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "lterm".into(),
        "has".into(),
        "main".into(),
    ])
    .await;
    assert_eq!(has_ok.status.code(), Some(0));
    let has_ok_val: Value = serde_json::from_str(&stdout_str(&has_ok)).unwrap();
    assert_eq!(has_ok_val["data"]["exists"], true);

    let has_missing = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "lterm".into(),
        "has".into(),
        "nope".into(),
    ])
    .await;
    assert_eq!(has_missing.status.code(), Some(1));
    let has_miss_val: Value = serde_json::from_str(&stdout_str(&has_missing)).unwrap();
    assert_eq!(has_miss_val["data"]["exists"], false);

    let new_out = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "lterm".into(),
        "new".into(),
        "work".into(),
        "--detach".into(),
    ])
    .await;
    assert!(new_out.status.success(), "stderr: {}", stderr_str(&new_out));

    let rename_out = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "lterm".into(),
        "rename".into(),
        "work".into(),
        "prod".into(),
    ])
    .await;
    assert!(rename_out.status.success());

    let has_prod = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "lterm".into(),
        "has".into(),
        "prod".into(),
    ])
    .await;
    assert_eq!(has_prod.status.code(), Some(0));

    let kill_out = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "lterm".into(),
        "kill".into(),
        "prod".into(),
    ])
    .await;
    assert!(kill_out.status.success());

    // 7. Input actions via --m fuzzy match + --output text verification
    let click_out = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "input".into(),
        "click".into(),
        "--m".into(),
        "studio".into(),
        "--x".into(),
        "100".into(),
        "--y".into(),
        "200".into(),
    ])
    .await;
    assert!(click_out.status.success(), "stderr: {}", stderr_str(&click_out));

    let type_out = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "input".into(),
        "type".into(),
        "--m".into(),
        "studio".into(),
        "hello".into(),
    ])
    .await;
    assert!(type_out.status.success());

    let hotkey_out = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "input".into(),
        "hotkey".into(),
        "--m".into(),
        "studio".into(),
        "Cmd+C".into(),
    ])
    .await;
    assert!(hotkey_out.status.success());

    let text_list = run_cli_async(vec![
        "--socket".into(),
        sock_str.clone(),
        "device".into(),
        "list".into(),
        "--output".into(),
        "text".into(),
    ])
    .await;
    assert!(text_list.status.success());
    let list_text = stdout_str(&text_list);
    assert!(list_text.contains("My Mac Studio"));
    assert!(list_text.contains("local:1:studio"));

    let has_text_ok = run_cli_async(vec![
        "--socket".into(),
        sock_str,
        "lterm".into(),
        "has".into(),
        "main".into(),
        "--output".into(),
        "text".into(),
    ])
    .await;
    assert_eq!(has_text_ok.status.code(), Some(0));
    assert_eq!(stdout_str(&has_text_ok).trim(), "exists");

    server.shutdown();
    server.wait_stopped().await;
}

#[tokio::test]
async fn test_cli_http_live_server_list_schema_and_actions_roundtrip() {
    let server = EphemeralServer::start().await;
    let http = Client::builder().no_proxy().build().unwrap();
    let (_alice_id, alice_tok) = register_user(&http, &server.base_url, "cli_alice_1").await;
    let chat_id =
        create_group_chat(&http, &server.base_url, &alice_tok, "CLI Live Chat", vec![]).await;

    let list_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "list".into(),
        "--json".into(),
    ])
    .await;
    assert!(list_out.status.success(), "list failed: {}", stderr_str(&list_out));
    let list_val: Value = serde_json::from_str(&stdout_str(&list_out)).unwrap();
    let actions = list_val["actions"].as_array().unwrap();
    assert!(actions.len() >= 4);
    assert!(actions.iter().any(|a| a["name"] == "send_message"));

    let schema_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "schema".into(),
        "send_message".into(),
    ])
    .await;
    assert!(schema_out.status.success(), "schema failed: {}", stderr_str(&schema_out));
    let schema_val: Value = serde_json::from_str(&stdout_str(&schema_out)).unwrap();
    assert_eq!(schema_val["name"], "send_message");
    assert!(schema_val["input_schema"].is_object());

    let send_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "send-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--text".into(),
        "Live HTTP CLI msg".into(),
    ])
    .await;
    assert!(send_out.status.success(), "send-message failed: {}", stderr_str(&send_out));
    let send_val: Value = serde_json::from_str(&stdout_str(&send_out)).unwrap();
    assert_eq!(send_val["success"], true);
    assert!(send_val["result"]["message_id"].as_i64().unwrap() > 0);

    let hist_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "get-chat-history".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--limit".into(),
        "10".into(),
    ])
    .await;
    assert!(hist_out.status.success(), "get-chat-history failed: {}", stderr_str(&hist_out));
    let hist_val: Value = serde_json::from_str(&stdout_str(&hist_out)).unwrap();
    assert_eq!(hist_val["success"], true);
    let msgs = hist_val["result"]["messages"].as_array().unwrap();
    assert!(msgs.iter().any(|m| m["text"] == "Live HTTP CLI msg"));

    let dlg_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "list-dialogs".into(),
        "--limit".into(),
        "10".into(),
    ])
    .await;
    assert!(dlg_out.status.success(), "list-dialogs failed: {}", stderr_str(&dlg_out));
    let dlg_val: Value = serde_json::from_str(&stdout_str(&dlg_out)).unwrap();
    assert_eq!(dlg_val["success"], true);
    let dialogs = dlg_val["result"]["dialogs"].as_array().unwrap();
    assert!(dialogs.iter().any(|d| d["chat_id"] == chat_id));

    let unauth_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "send-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--text".into(),
        "Should fail 401".into(),
    ])
    .await;
    assert!(!unauth_out.status.success());
    assert_eq!(unauth_out.status.code(), Some(3));

    server.stop().await;
}

#[tokio::test]
async fn test_cli_saved_session_and_full_app_http_roundtrip() {
    let server = EphemeralServer::start().await;
    let http = Client::builder().no_proxy().build().unwrap();
    let (_alice_id, _alice_tok) = register_user(&http, &server.base_url, "cli_sess_alice").await;

    let session_dir = TempDir::new().unwrap();
    let session_path = session_dir.path().join("cli_session.json");
    let session_env = vec![(
        "FABUSHI_CLI_SESSION_PATH".to_string(),
        session_path.to_string_lossy().into_owned(),
    )];

    // 1. Login and persist ~/.fabushi/cli_session.json (0600)
    let login_out = run_cli_async_with_env(
        vec![
            "--server-url".into(),
            server.base_url.clone(),
            "auth".into(),
            "login".into(),
            "--username".into(),
            "cli_sess_alice".into(),
            "--password".into(),
            "CliTestPassword#2026".into(),
        ],
        session_env.clone(),
    )
    .await;
    assert!(
        login_out.status.success(),
        "auth login failed: {}",
        stderr_str(&login_out)
    );
    assert!(session_path.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&session_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    // 2. Subsequent commands use saved session automatically (no --token flag)
    let dead_sock = session_dir.path().join("unused.sock").to_string_lossy().into_owned();
    let mut env_with_dead_ipc = session_env.clone();
    env_with_dead_ipc.push(("FABUSHI_IPC_SOCKET".into(), dead_sock));

    let whoami_out = run_cli_async_with_env(
        vec!["auth".into(), "whoami".into()],
        env_with_dead_ipc.clone(),
    )
    .await;
    assert!(
        whoami_out.status.success(),
        "whoami failed: {}",
        stderr_str(&whoami_out)
    );
    let whoami_val: Value = serde_json::from_str(&stdout_str(&whoami_out)).unwrap();
    assert_eq!(whoami_val["success"], true);
    assert_eq!(whoami_val["data"]["username"], "cli_sess_alice");

    let chat_create_out = run_cli_async_with_env(
        vec!["chat".into(), "create".into(), "Unified CLI Chat".into()],
        env_with_dead_ipc.clone(),
    )
    .await;
    assert!(
        chat_create_out.status.success(),
        "chat create failed: {}",
        stderr_str(&chat_create_out)
    );
    let chat_val: Value = serde_json::from_str(&stdout_str(&chat_create_out)).unwrap();
    let chat_id = chat_val["data"]["chat_id"].as_i64().unwrap();

    let msg_send_out = run_cli_async_with_env(
        vec![
            "msg".into(),
            "send".into(),
            chat_id.to_string(),
            "Hello unified CLI".into(),
        ],
        env_with_dead_ipc.clone(),
    )
    .await;
    assert!(
        msg_send_out.status.success(),
        "msg send failed: {}",
        stderr_str(&msg_send_out)
    );

    let wallet_out = run_cli_async_with_env(
        vec!["user".into(), "wallet".into()],
        env_with_dead_ipc.clone(),
    )
    .await;
    assert!(wallet_out.status.success(), "user wallet failed: {}", stderr_str(&wallet_out));

    let task_create_out = run_cli_async_with_env(
        vec!["task".into(), "create".into(), "CLI Task".into()],
        env_with_dead_ipc.clone(),
    )
    .await;
    assert!(
        task_create_out.status.success(),
        "task create failed: {}",
        stderr_str(&task_create_out)
    );

    // 3. Remote Assist & Official MCP HTTP endpoints via saved session
    let assist_id_out = run_cli_async_with_env(
        vec!["assist".into(), "id".into()],
        env_with_dead_ipc.clone(),
    )
    .await;
    assert!(
        assist_id_out.status.success(),
        "assist id failed: {}",
        stderr_str(&assist_id_out)
    );
    let assist_val: Value = serde_json::from_str(&stdout_str(&assist_id_out)).unwrap();
    assert_eq!(assist_val["data"]["assist_id"].as_str().map(str::len), Some(9));

    let assist_allow_out = run_cli_async_with_env(
        vec!["assist".into(), "allow".into(), "on".into()],
        env_with_dead_ipc.clone(),
    )
    .await;
    assert!(assist_allow_out.status.success(), "assist allow failed: {}", stderr_str(&assist_allow_out));

    let assist_code_out = run_cli_async_with_env(
        vec!["assist".into(), "set-code".into(), "Abcd1234".into()],
        env_with_dead_ipc.clone(),
    )
    .await;
    assert!(assist_code_out.status.success(), "assist set-code failed: {}", stderr_str(&assist_code_out));

    let mcp_status_out = run_cli_async_with_env(
        vec!["mcp".into(), "status".into(), "--url".into(), server.base_url.clone()],
        env_with_dead_ipc.clone(),
    )
    .await;
    assert!(mcp_status_out.status.success(), "mcp status failed: {}", stderr_str(&mcp_status_out));
    let mcp_status_val: Value = serde_json::from_str(&stdout_str(&mcp_status_out)).unwrap();
    assert_eq!(mcp_status_val["success"], true);
    assert!(mcp_status_val["data"]["protected_resource"]["scopes_supported"].is_array());

    let mcp_grants_out = run_cli_async_with_env(
        vec!["mcp".into(), "grants".into()],
        env_with_dead_ipc.clone(),
    )
    .await;
    assert!(mcp_grants_out.status.success(), "mcp grants failed: {}", stderr_str(&mcp_grants_out));

    // 4. mcp serve-stdio relays stdin JSON-RPC line cleanly (verifies 401 -> JSON-RPC error response)
    let stdio_out = tokio::task::spawn_blocking({
        let url = server.base_url.clone();
        move || {
            use std::io::Write;
            let mut child = base_cmd()
                .args(["--server-url", &url, "mcp", "serve-stdio"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("spawn serve-stdio");
            {
                let mut stdin = child.stdin.take().unwrap();
                writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#)
                    .unwrap();
            }
            child.wait_with_output().unwrap()
        }
    })
    .await
    .unwrap();
    assert!(stdio_out.status.success(), "serve-stdio failed: {}", stderr_str(&stdio_out));
    let rpc_resp: Value = serde_json::from_str(stdout_str(&stdio_out).trim()).unwrap();
    assert_eq!(rpc_resp["jsonrpc"], "2.0");
    assert_eq!(rpc_resp["id"], 1);
    assert!(rpc_resp["error"]["message"]
        .as_str()
        .is_some_and(|m| m.contains("401")));

    let logout_out = run_cli_async_with_env(
        vec!["auth".into(), "logout".into()],
        env_with_dead_ipc,
    )
    .await;
    assert!(logout_out.status.success());
    assert!(!session_path.exists());

    server.stop().await;
}

#[tokio::test]
async fn test_cli_http_approval_gate_single_use_and_deny_roundtrip() {
    let server = EphemeralServer::start().await;
    let http = Client::builder().no_proxy().build().unwrap();
    let (_alice_id, alice_tok) = register_user(&http, &server.base_url, "cli_alice_gate").await;
    let (bob_id, bob_tok) = register_user(&http, &server.base_url, "cli_bob_gate").await;
    let chat_id = create_group_chat(
        &http,
        &server.base_url,
        &alice_tok,
        "CLI Gate Chat",
        vec![bob_id],
    )
    .await;

    let mut mids = Vec::new();
    for idx in 1..=2 {
        let send_out = run_cli_async(vec![
            "--server-url".into(),
            server.base_url.clone(),
            "--token".into(),
            alice_tok.clone(),
            "send-message".into(),
            "--chat-id".into(),
            chat_id.to_string(),
            "--text".into(),
            format!("Delete target #{idx}"),
        ])
        .await;
        assert!(send_out.status.success(), "seed msg {idx} failed: {}", stderr_str(&send_out));
        let send_val: Value = serde_json::from_str(&stdout_str(&send_out)).unwrap();
        mids.push(send_val["result"]["message_id"].as_i64().unwrap());
    }
    let mid_1 = mids[0];
    let mid_2 = mids[1];

    let del_gate_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "delete-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--message-ids".into(),
        mid_1.to_string(),
        "--for-everyone".into(),
    ])
    .await;
    assert!(!del_gate_out.status.success());
    assert_eq!(del_gate_out.status.code(), Some(4));
    let gate_err_text = stderr_str(&del_gate_out);
    let gate_err_val: Value = serde_json::from_str(&gate_err_text)
        .unwrap_or_else(|e| panic!("Expected JSON error on stderr, got '{gate_err_text}': {e}"));
    assert_eq!(gate_err_val["error"], "Approval required");
    let gate_id = gate_err_val["gate_id"]
        .as_str()
        .expect("gate_id present")
        .to_string();
    assert!(gate_id.starts_with("actgate_"));

    let bob_approve_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        bob_tok.clone(),
        "approve".into(),
        gate_id.clone(),
        "--decision".into(),
        "approve_once".into(),
    ])
    .await;
    assert!(!bob_approve_out.status.success());
    assert_eq!(bob_approve_out.status.code(), Some(4));

    let alice_approve_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "approve".into(),
        gate_id.clone(),
        "--decision".into(),
        "approve_once".into(),
    ])
    .await;
    assert!(
        alice_approve_out.status.success(),
        "alice approve failed: {}",
        stderr_str(&alice_approve_out)
    );
    let approve_val: Value = serde_json::from_str(&stdout_str(&alice_approve_out)).unwrap();
    assert_eq!(approve_val["approved"], true);
    let approval_token = approve_val["approval_token"]
        .as_str()
        .expect("approval_token")
        .to_string();

    let exec_approved = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "--approval-token".into(),
        approval_token.clone(),
        "delete-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--message-ids".into(),
        mid_1.to_string(),
        "--for-everyone".into(),
    ])
    .await;
    assert!(
        exec_approved.status.success(),
        "delete with approval_token failed: {}",
        stderr_str(&exec_approved)
    );
    let exec_val: Value = serde_json::from_str(&stdout_str(&exec_approved)).unwrap();
    assert_eq!(exec_val["success"], true);
    assert_eq!(exec_val["result"]["deleted_count"], 1);

    let replay_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "--approval-token".into(),
        approval_token,
        "delete-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--message-ids".into(),
        mid_1.to_string(),
        "--for-everyone".into(),
    ])
    .await;
    assert!(!replay_out.status.success());
    assert_eq!(replay_out.status.code(), Some(4));
    assert!(stderr_str(&replay_out).contains("approval_consumed"));

    let del_gate_2 = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "delete-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--message-ids".into(),
        mid_2.to_string(),
        "--for-everyone".into(),
    ])
    .await;
    assert!(!del_gate_2.status.success());
    let gate_2_val: Value = serde_json::from_str(&stderr_str(&del_gate_2)).unwrap();
    let gate_id_2 = gate_2_val["gate_id"].as_str().unwrap().to_string();

    let deny_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "approve".into(),
        gate_id_2.clone(),
        "--decision".into(),
        "deny".into(),
        "--reason".into(),
        "rejected".into(),
    ])
    .await;
    assert!(deny_out.status.success(), "deny failed: {}", stderr_str(&deny_out));
    let deny_val: Value = serde_json::from_str(&stdout_str(&deny_out)).unwrap();
    assert_eq!(deny_val["approved"], false);
    assert_eq!(deny_val["denied"], true);

    let redecide_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok.clone(),
        "approve".into(),
        gate_id_2,
        "--decision".into(),
        "approve_once".into(),
    ])
    .await;
    assert!(!redecide_out.status.success());
    assert!(stderr_str(&redecide_out).contains("409") || stderr_str(&redecide_out).contains("already_decided"));

    let auto_approve_out = run_cli_async(vec![
        "--server-url".into(),
        server.base_url.clone(),
        "--token".into(),
        alice_tok,
        "delete-message".into(),
        "--chat-id".into(),
        chat_id.to_string(),
        "--message-ids".into(),
        mid_2.to_string(),
        "--for-everyone".into(),
        "--approve".into(),
    ])
    .await;
    assert!(
        auto_approve_out.status.success(),
        "auto-approve failed: {}",
        stderr_str(&auto_approve_out)
    );
    let auto_val: Value = serde_json::from_str(&stdout_str(&auto_approve_out)).unwrap();
    assert_eq!(auto_val["success"], true);
    assert_eq!(auto_val["result"]["deleted_count"], 1);

    server.stop().await;
}
