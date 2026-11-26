use crate::cli_parser::{GlobalOptions, OutputFormat};
use crate::envelope::{format_json, fuzzy_match_node, ok_envelope, CliOutcome};
use crate::error::CliError;
use crate::ipc_client::{map_ipc_response, BackendSource, FabushiBackendClient};
use crate::mcp_config::{build_client_config, normalize_mcp_url};
use crate::session_store::{clear_cli_session, save_cli_session, CliSession};
use crate::unified_parser::{
    AppOp, AssistOp, AuthOp, BotOp, ChatOp, ClipboardOp, CloudPcOp, DaemonOp, DeviceOp, FileOp,
    InputOp, LtermOp, McpOp, MiniAppOp, MsgOp, TargetSelector, TaskOp, TermOp, UnifiedCommand,
    UserOp,
};
use base64::Engine as _;
use fabushi_remote_engine::terminal::client_tty::{run_local_tty_attach, TtyAttachOptions};
use fabushi_remote_engine::terminal::is_inside_mux;
use gpui_telegram_proto::computer_contracts::{RemoteComputerCommandRequest, RemoteComputerOp};
use gpui_telegram_proto::remote_control_contracts::{
    AssistAllowRequest, AssistSetCodeRequest, AssistStartRequest, PowerActionKind,
    RemoteControlMode,
};
use reqwest::Method;
use serde_json::{json, Value};
use std::io::{BufRead, IsTerminal, Write};

fn render_output(
    opts: &GlobalOptions,
    data: Value,
    text_fn: impl FnOnce(&Value) -> String,
) -> Result<CliOutcome, CliError> {
    if opts.output_format == Some(OutputFormat::Text) {
        Ok(CliOutcome::ok(text_fn(&data)))
    } else {
        let json_str = format_json(&ok_envelope(data), opts.pretty)?;
        Ok(CliOutcome::ok(json_str))
    }
}

fn render_default_text(val: &Value) -> String {
    serde_json::to_string_pretty(val).unwrap_or_else(|_| val.to_string())
}

fn extract_computers_vec(val: &Value) -> Vec<Value> {
    if let Some(arr) = val.as_array() {
        return arr.clone();
    }
    if let Some(arr) = val.get("computers").and_then(|v| v.as_array()) {
        return arr.clone();
    }
    if let Some(arr) = val.get("devices").and_then(|v| v.as_array()) {
        return arr.clone();
    }
    Vec::new()
}

async fn fetch_computers_list(
    backend: &FabushiBackendClient,
    cloud_only: bool,
) -> Result<(BackendSource, Vec<Value>), CliError> {
    let method = if cloud_only {
        "cloudpc.list"
    } else {
        "device.list"
    };
    let (src, val) = backend
        .call_ipc_or_http(method, json!({}), |http, tok| async move {
            let token = tok.ok_or_else(|| {
                CliError::Unauthorized(
                    "Bearer token required to query computers (start Fabushi Desktop/Daemon or pass --token)".into(),
                )
            })?;
            http.get_json("/api/v1/computers", Some(&token), &[]).await
        })
        .await?;

    let mut nodes = extract_computers_vec(&val);
    if cloud_only {
        nodes.retain(|n| {
            matches!(
                n.get("kind").and_then(|k| k.as_str()),
                Some("cloud_server" | "cloud_sandbox")
            )
        });
    }
    Ok((src, nodes))
}

async fn resolve_target_node_id(
    backend: &FabushiBackendClient,
    target: &TargetSelector,
    prefer_cloud: bool,
) -> Result<String, CliError> {
    match target {
        TargetSelector::Id(id) => Ok(id.clone()),
        TargetSelector::Match(pattern) => {
            let (_src, mut nodes) = fetch_computers_list(backend, prefer_cloud).await?;
            if nodes.is_empty() && prefer_cloud {
                let (_, all) = fetch_computers_list(backend, false).await?;
                nodes = all;
            }
            let matched = fuzzy_match_node(&nodes, pattern)?;
            matched
                .get("node_id")
                .and_then(|v| v.as_str())
                .map(String::from)
                .ok_or_else(|| CliError::Execution("Matched device is missing node_id".into()))
        }
    }
}

async fn exec_remote_computer_op_http(
    backend: &FabushiBackendClient,
    node_id: &str,
    operation: RemoteComputerOp,
) -> Result<Value, CliError> {
    let token = backend.require_token()?;
    let http = backend.http_client()?;
    let req = RemoteComputerCommandRequest {
        node_id: node_id.to_string(),
        caller_device_id: backend
            .saved_session()
            .and_then(|s| s.device_id.clone()),
        caller_bot_id: None,
        caller_label: "fabushi-cli".to_string(),
        operation,
    };
    let payload = serde_json::to_value(req)
        .map_err(|e| CliError::Execution(format!("Failed to serialize computer op: {e}")))?;
    http.post_json("/api/v1/computers/exec", Some(token), &payload)
        .await
}

pub async fn execute_unified(
    opts: &GlobalOptions,
    cmd: UnifiedCommand,
) -> Result<CliOutcome, CliError> {
    let backend = FabushiBackendClient::from_options(opts);

    match cmd {
        UnifiedCommand::Status => {
            let ipc = backend.probe_ipc_status().await;
            let session_info = backend.saved_session().map(|s| {
                json!({
                    "logged_in": true,
                    "server_url": s.server_url,
                    "username": s.username,
                    "user_id": s.user_id,
                    "session_id": s.session_id,
                })
            });
            let data = json!({
                "program": crate::registry::PROGRAM_NAME,
                "version": env!("CARGO_PKG_VERSION"),
                "ipc": ipc,
                "server_url": backend.server_url(),
                "has_token": backend.token().is_some(),
                "session": session_info,
            });
            render_output(opts, data, |v| {
                let connected = v["ipc"]["connected"].as_bool().unwrap_or(false);
                format!(
                    "Fabushi {} | IPC: {} ({}) | Server: {}",
                    env!("CARGO_PKG_VERSION"),
                    if connected { "connected" } else { "offline" },
                    v["ipc"]["endpoint"].as_str().unwrap_or("-"),
                    v["server_url"].as_str().unwrap_or("-"),
                )
            })
        }

        UnifiedCommand::Echo { message } => {
            let (source, val) = match backend.try_ipc("echo", json!({ "message": message })).await? {
                Some(resp) => (BackendSource::LocalIpc, map_ipc_response(resp)?),
                None => (
                    BackendSource::LocalIpc,
                    json!({ "message": message, "offline_echo": true }),
                ),
            };
            let data = json!({
                "source": match source { BackendSource::LocalIpc => "local_ipc", BackendSource::HttpServer => "http" },
                "echo": val,
            });
            render_output(opts, data, |v| {
                v["echo"]["message"]
                    .as_str()
                    .unwrap_or(&message)
                    .to_string()
            })
        }

        UnifiedCommand::Daemon { op } => handle_daemon(opts, &backend, op).await,
        UnifiedCommand::App { op } => handle_app(opts, &backend, op).await,
        UnifiedCommand::Auth { op } => handle_auth(opts, &backend, op).await,
        UnifiedCommand::User { op } => handle_user(opts, &backend, op).await,

        UnifiedCommand::VaultStatus => {
            let (_src, val) = backend
                .call_ipc_or_http("vault.status", json!({}), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'vault status'".into())
                    })?;
                    http.get_json("/api/v1/user-vault/envelope", Some(&token), &[])
                        .await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }

        UnifiedCommand::Device { op } => handle_device(opts, &backend, op).await,
        UnifiedCommand::CloudPc { op } => handle_cloudpc(opts, &backend, op).await,
        UnifiedCommand::Assist { op } => handle_assist(opts, &backend, op).await,
        UnifiedCommand::Term { op } => handle_term(opts, &backend, op).await,
        UnifiedCommand::Lterm { op } => handle_lterm(opts, &backend, op).await,

        UnifiedCommand::ScreenCapture {
            target,
            out_path,
            display_index,
        } => {
            let node_id = resolve_target_node_id(&backend, &target, false).await?;
            let (_src, mut val) = backend
                .call_ipc_or_http(
                    "screen.capture",
                    json!({ "node_id": node_id, "display_index": display_index }),
                    |_, _| async {
                        exec_remote_computer_op_http(
                            &backend,
                            &node_id,
                            RemoteComputerOp::CaptureScreen,
                        )
                        .await
                    },
                )
                .await?;

            if let Some(ref path) = out_path {
                let raw_b64 = val
                    .get("image_b64")
                    .or_else(|| val.get("frame_b64"))
                    .or_else(|| val.get("frame_data_url"))
                    .or_else(|| val.get("data_url"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        CliError::Execution(
                            "Screen capture response did not include image payload to write --out"
                                .into(),
                        )
                    })?;
                let b64_clean = raw_b64
                    .split_once("base64,")
                    .map(|(_, rest)| rest)
                    .unwrap_or(raw_b64);
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64_clean.trim())
                    .map_err(|e| {
                        CliError::Execution(format!("Failed to decode captured image base64: {e}"))
                    })?;
                if let Some(parent) = std::path::Path::new(path).parent() {
                    if !parent.as_os_str().is_empty() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                }
                std::fs::write(path, &bytes).map_err(|e| {
                    CliError::Execution(format!("Failed to write screenshot to '{path}': {e}"))
                })?;
                if let Some(obj) = val.as_object_mut() {
                    obj.insert("saved_path".into(), Value::String(path.clone()));
                    obj.insert("bytes_written".into(), json!(bytes.len()));
                }
            }
            render_output(opts, val, |v| {
                if let Some(p) = v.get("saved_path").and_then(|s| s.as_str()) {
                    format!("Captured screen to {p}")
                } else {
                    render_default_text(v)
                }
            })
        }

        UnifiedCommand::Input { op } => handle_input(opts, &backend, op).await,
        UnifiedCommand::Clipboard { op } => handle_clipboard(opts, &backend, op).await,
        UnifiedCommand::File { op } => handle_file(opts, &backend, op).await,
        UnifiedCommand::Chat { op } => handle_chat(opts, &backend, op).await,
        UnifiedCommand::Msg { op } => handle_msg(opts, &backend, op).await,

        UnifiedCommand::Search {
            query,
            chat_id,
            limit,
        } => {
            let (_src, val) = backend
                .call_ipc_or_http(
                    "search",
                    json!({ "query": query, "chat_id": chat_id, "limit": limit }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'search'".into())
                        })?;
                        let mut q = vec![("q", query.clone()), ("limit", limit.to_string())];
                        if let Some(cid) = chat_id {
                            q.push(("chat_id", cid.to_string()));
                        }
                        http.get_json("/api/v1/search", Some(&token), &q).await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }

        UnifiedCommand::CallStatus { call_id } => {
            let (_src, val) = backend
                .call_ipc_or_http(
                    "call.status",
                    json!({ "call_id": call_id }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'call status'".into())
                        })?;
                        http.get_json("/api/v1/calls", Some(&token), &[]).await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }

        UnifiedCommand::Bot { op } => handle_bot(opts, &backend, op).await,
        UnifiedCommand::MiniApp { op } => handle_miniapp(opts, &backend, op).await,
        UnifiedCommand::Task { op } => handle_task(opts, &backend, op).await,
        UnifiedCommand::Mcp { op } => handle_mcp(opts, &backend, op).await,
        UnifiedCommand::Orca(cmd) => crate::orca_commands::execute_orca(opts, &backend, cmd).await,
    }
}

async fn handle_daemon(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: DaemonOp,
) -> Result<CliOutcome, CliError> {
    match op {
        DaemonOp::Status => {
            let ipc = backend.probe_ipc_status().await;
            render_output(opts, ipc, |v| {
                let connected = v["connected"].as_bool().unwrap_or(false);
                format!(
                    "daemon: {} ({})",
                    if connected { "running" } else { "stopped" },
                    v["endpoint"].as_str().unwrap_or("-")
                )
            })
        }
        DaemonOp::Start { foreground } => {
            if !foreground {
                let probe = backend.probe_ipc_status().await;
                if probe["connected"].as_bool() == Some(true) {
                    return render_output(
                        opts,
                        json!({ "started": true, "already_running": true, "ipc": probe }),
                        |_| "fabushi-remote-daemon is already running".to_string(),
                    );
                }
            }
            let daemon_bin = locate_daemon_binary().ok_or_else(|| {
                CliError::Execution(
                    "Could not locate 'fabushi-remote-daemon' binary next to fabushi or in PATH"
                        .into(),
                )
            })?;
            let endpoint = backend.ipc_endpoint().to_string_lossy().into_owned();
            if foreground {
                let status = std::process::Command::new(&daemon_bin)
                    .arg("--agent")
                    .arg("--socket")
                    .arg(&endpoint)
                    .status()
                    .map_err(|e| CliError::Execution(format!("Failed to launch daemon: {e}")))?;
                return render_output(
                    opts,
                    json!({ "exited": true, "code": status.code().unwrap_or(1) }),
                    |_| "fabushi-remote-daemon exited".to_string(),
                );
            }
            let child = std::process::Command::new(&daemon_bin)
                .arg("--agent")
                .arg("--socket")
                .arg(&endpoint)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| CliError::Execution(format!("Failed to spawn daemon: {e}")))?;
            let pid = child.id();
            for _ in 0..20 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                if backend.ipc_client().ping_status().await {
                    break;
                }
            }
            render_output(
                opts,
                json!({
                    "started": true,
                    "pid": pid,
                    "binary": daemon_bin.display().to_string(),
                    "endpoint": endpoint,
                }),
                |v| format!("Started fabushi-remote-daemon (pid {})", v["pid"]),
            )
        }
        DaemonOp::Stop => {
            let stopped_ipc = backend.try_ipc("daemon.stop", json!({})).await?.is_some();
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("launchctl")
                .args([
                    "bootout",
                    &format!("gui/{}/com.ombhrum.fabushi.agent", unsafe { libc_uid() }),
                ])
                .output();
            render_output(
                opts,
                json!({ "stopped": true, "ipc_ack": stopped_ipc }),
                |_| "Requested fabushi-remote-daemon stop".to_string(),
            )
        }
        DaemonOp::InstallService => {
            let daemon_bin = locate_daemon_binary()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| {
                    "/Applications/Fabushi.app/Contents/Helpers/fabushi-remote-daemon".to_string()
                });
            #[cfg(target_os = "macos")]
            {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                let agents_dir = std::path::PathBuf::from(home).join("Library/LaunchAgents");
                std::fs::create_dir_all(&agents_dir).map_err(|e| {
                    CliError::Execution(format!("Failed creating LaunchAgents dir: {e}"))
                })?;
                let plist_path = agents_dir.join("com.ombhrum.fabushi.agent.plist");
                let bundled_plist = std::path::Path::new(
                    "/Applications/Fabushi.app/Contents/Resources/com.ombhrum.fabushi.agent.plist",
                );
                if bundled_plist.is_file() {
                    std::fs::copy(bundled_plist, &plist_path).map_err(|e| {
                        CliError::Execution(format!("Failed copying agent plist: {e}"))
                    })?;
                } else {
                    let plist_xml = format!(
                        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.ombhrum.fabushi.agent</string>
    <key>ProgramArguments</key>
    <array>
        <string>{daemon_bin}</string>
        <string>--agent</string>
    </array>
    <key>LimitLoadToSessionType</key>
    <array>
        <string>LoginWindow</string>
        <string>Aqua</string>
    </array>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>
"#
                    );
                    std::fs::write(&plist_path, plist_xml).map_err(|e| {
                        CliError::Execution(format!("Failed writing agent plist: {e}"))
                    })?;
                }
                let uid = unsafe { libc_uid() };
                let _ = std::process::Command::new("launchctl")
                    .args(["bootout", &format!("gui/{uid}/com.ombhrum.fabushi.agent")])
                    .output();
                let boot = std::process::Command::new("launchctl")
                    .args([
                        "bootstrap",
                        &format!("gui/{uid}"),
                        &plist_path.display().to_string(),
                    ])
                    .output();
                let bootstrapped = boot.map(|o| o.status.success()).unwrap_or(false);
                return render_output(
                    opts,
                    json!({
                        "installed": true,
                        "platform": "macos",
                        "plist": plist_path.display().to_string(),
                        "bootstrapped": bootstrapped,
                    }),
                    |v| format!("Installed LaunchAgent at {}", v["plist"]),
                );
            }
            #[cfg(not(target_os = "macos"))]
            {
                render_output(
                    opts,
                    json!({
                        "installed": true,
                        "platform": std::env::consts::OS,
                        "binary": daemon_bin,
                    }),
                    render_default_text,
                )
            }
        }
    }
}

#[cfg(target_os = "macos")]
unsafe fn libc_uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

fn locate_daemon_binary() -> Option<std::path::PathBuf> {
    let bin_name = if cfg!(windows) {
        "fabushi-remote-daemon.exe"
    } else {
        "fabushi-remote-daemon"
    };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(bin_name);
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    let app_helper = std::path::PathBuf::from(
        "/Applications/Fabushi.app/Contents/Helpers/fabushi-remote-daemon",
    );
    if app_helper.is_file() {
        return Some(app_helper);
    }
    if let Ok(path_env) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_env) {
            let cand = dir.join(bin_name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

async fn handle_app(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: AppOp,
) -> Result<CliOutcome, CliError> {
    match op {
        AppOp::Open => {
            #[cfg(target_os = "macos")]
            {
                let out = std::process::Command::new("/usr/bin/open")
                    .args(["-a", "Fabushi"])
                    .output()
                    .map_err(|e| CliError::Execution(format!("Failed launching Fabushi.app: {e}")))?;
                if !out.status.success() {
                    return Err(CliError::Execution(
                        String::from_utf8_lossy(&out.stderr).trim().to_string(),
                    ));
                }
            }
            #[cfg(target_os = "linux")]
            {
                let _ = std::process::Command::new("gpui_telegram_desktop").spawn();
            }
            #[cfg(target_os = "windows")]
            {
                let _ = std::process::Command::new("Fabushi.exe").spawn();
            }
            render_output(opts, json!({ "opened": true, "app": "Fabushi" }), |_| {
                "Launched Fabushi Desktop".to_string()
            })
        }
        AppOp::Permissions => {
            let ipc_resp = backend.try_ipc("app.permissions", json!({})).await?;
            let data = if let Some(resp) = ipc_resp {
                map_ipc_response(resp)?
            } else {
                json!({
                    "platform": std::env::consts::OS,
                    "screen_recording": if cfg!(target_os = "macos") { "check_system_settings" } else { "available" },
                    "accessibility": if cfg!(target_os = "macos") { "check_system_settings" } else { "available" },
                    "ipc_connected": backend.probe_ipc_status().await["connected"],
                })
            };
            render_output(opts, data, render_default_text)
        }
    }
}

async fn handle_auth(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: AuthOp,
) -> Result<CliOutcome, CliError> {
    match op {
        AuthOp::Login {
            username,
            password,
            device_name,
        } => {
            let uname = username.ok_or_else(|| {
                CliError::Usage("'auth login' requires --username <USERNAME>".into())
            })?;
            let pass = match password {
                Some(p) => p,
                None if !opts.non_interactive && std::io::stdin().is_terminal() => {
                    eprint!("Password for {uname}: ");
                    let _ = std::io::stderr().flush();
                    let mut buf = String::new();
                    std::io::stdin()
                        .read_line(&mut buf)
                        .map_err(|e| CliError::Validation(format!("Failed reading password: {e}")))?;
                    buf.trim_end_matches(&['\r', '\n'][..]).to_string()
                }
                None => {
                    return Err(CliError::Usage(
                        "'auth login' requires --password <PASSWORD> in non-interactive mode".into(),
                    ))
                }
            };
            let dev = device_name.unwrap_or_else(|| "fabushi-cli".to_string());
            let http = backend.http_client()?;
            let resp = http
                .post_json(
                    "/api/v1/auth/login",
                    None,
                    &json!({
                        "username": uname,
                        "password": pass,
                        "device_name": dev,
                    }),
                )
                .await?;
            let token = resp
                .get("token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    CliError::Execution("Login response missing session token".into())
                })?
                .to_string();
            let sess = CliSession {
                server_url: backend.server_url().to_string(),
                token,
                user_id: resp.get("user_id").and_then(|v| v.as_u64()),
                username: resp
                    .get("username")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(|| Some(uname.clone())),
                session_id: resp
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                device_id: Some(dev),
                saved_at: chrono::Utc::now().to_rfc3339(),
            };
            let saved_path = save_cli_session(&sess)?;
            let data = json!({
                "logged_in": true,
                "user_id": sess.user_id,
                "username": sess.username,
                "session_id": sess.session_id,
                "server_url": sess.server_url,
                "session_file": saved_path.display().to_string(),
            });
            render_output(opts, data, |v| {
                format!(
                    "Logged in as {} (session saved to {})",
                    v["username"].as_str().unwrap_or("-"),
                    v["session_file"].as_str().unwrap_or("-")
                )
            })
        }
        AuthOp::Logout => {
            let removed = clear_cli_session()?;
            render_output(
                opts,
                json!({ "logged_out": true, "removed_session_file": removed }),
                |_| "Logged out of Fabushi CLI session".to_string(),
            )
        }
        AuthOp::Whoami => {
            let (_src, val) = backend
                .call_ipc_or_http("user.info", json!({}), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Not logged in (run 'fabushi auth login' or pass --token)".into())
                    })?;
                    http.get_json("/api/v1/auth/me", Some(&token), &[]).await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        AuthOp::Sessions => {
            let val = backend
                .http_authenticated_json(Method::GET, "/api/v1/auth/sessions", None, &[])
                .await?;
            render_output(opts, val, render_default_text)
        }
        AuthOp::Revoke { session_id, others } => {
            let val = if others {
                backend
                    .http_authenticated_json(
                        Method::DELETE,
                        "/api/v1/auth/sessions/others",
                        None,
                        &[],
                    )
                    .await?
            } else {
                let sid = session_id.ok_or_else(|| {
                    CliError::Usage("'auth revoke' requires <session_id> or 'others'".into())
                })?;
                backend
                    .http_authenticated_json(
                        Method::DELETE,
                        &format!("/api/v1/auth/sessions/{sid}"),
                        None,
                        &[],
                    )
                    .await?
            };
            render_output(opts, val, render_default_text)
        }
    }
}

async fn handle_user(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: UserOp,
) -> Result<CliOutcome, CliError> {
    match op {
        UserOp::Info { user_id } => {
            let (_src, val) = backend
                .call_ipc_or_http(
                    "user.info",
                    json!({ "user_id": user_id }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'user info'".into())
                        })?;
                        let path = match user_id {
                            Some(uid) => format!("/api/v1/users/{uid}"),
                            None => "/api/v1/users/me".to_string(),
                        };
                        http.get_json(&path, Some(&token), &[]).await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        UserOp::Wallet => {
            let (_src, val) = backend
                .call_ipc_or_http("user.wallet", json!({}), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'user wallet'".into())
                    })?;
                    let balance = http
                        .get_json("/api/v1/wallet/balance", Some(&token), &[])
                        .await?;
                    let txs = http
                        .get_json("/api/v1/wallet/transactions", Some(&token), &[])
                        .await
                        .unwrap_or_else(|_| json!({ "transactions": [] }));
                    Ok(json!({
                        "balance": balance,
                        "transactions": txs.get("transactions").cloned().unwrap_or_else(|| json!([])),
                    }))
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        UserOp::Search { query, limit } => {
            let (_src, val) = backend
                .call_ipc_or_http(
                    "user.search",
                    json!({ "query": query, "limit": limit }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'user search'".into())
                        })?;
                        http.get_json(
                            "/api/v1/peers/resolve",
                            Some(&token),
                            &[("q", query.clone())],
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
    }
}

async fn handle_device(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: DeviceOp,
) -> Result<CliOutcome, CliError> {
    match op {
        DeviceOp::List { online_only } => {
            let (_src, mut nodes) = fetch_computers_list(backend, false).await?;
            if online_only {
                nodes.retain(|n| n.get("online").and_then(|v| v.as_bool()).unwrap_or(false));
            }
            let count = nodes.len();
            render_output(
                opts,
                json!({ "devices": nodes, "count": count }),
                |v| {
                    let mut out = String::new();
                    if let Some(arr) = v["devices"].as_array() {
                        for d in arr {
                            out.push_str(&format!(
                                "{:<24} {:<20} {:<10} {}\n",
                                d["node_id"].as_str().unwrap_or("-"),
                                d["display_name"].as_str().unwrap_or("-"),
                                if d["online"].as_bool().unwrap_or(false) {
                                    "online"
                                } else {
                                    "offline"
                                },
                                d["os_name"].as_str().unwrap_or("-"),
                            ));
                        }
                    }
                    out
                },
            )
        }
        DeviceOp::Info { target } => {
            let (_src, nodes) = fetch_computers_list(backend, false).await?;
            let matched = match target {
                TargetSelector::Id(ref id) => nodes
                    .iter()
                    .find(|n| n.get("node_id").and_then(|v| v.as_str()) == Some(id.as_str()))
                    .cloned()
                    .ok_or_else(|| CliError::UnknownCommand(format!("Device '{id}' not found")))?,
                TargetSelector::Match(ref pat) => fuzzy_match_node(&nodes, pat)?,
            };
            render_output(opts, matched, render_default_text)
        }
        DeviceOp::Connect { target, mode } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let (_src, val) = backend
                .call_ipc_or_http(
                    "device.connect",
                    json!({ "node_id": node_id, "mode": mode }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'device connect'".into())
                        })?;
                        http.post_json(
                            &format!("/api/v1/computers/{node_id}/test"),
                            Some(&token),
                            &json!({ "mode": mode }),
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        DeviceOp::Disconnect { target, session_id } => {
            let node_id = match target {
                Some(ref t) => Some(resolve_target_node_id(backend, t, false).await?),
                None => None,
            };
            let params = json!({ "node_id": node_id, "session_id": session_id });
            let (_src, val) = if let Some(resp) = backend.try_ipc("device.disconnect", params).await? {
                (BackendSource::LocalIpc, map_ipc_response(resp)?)
            } else if let Some(resp) = if let Some(ref sid) = session_id {
                backend
                    .try_ipc("sessions.disconnect", json!({ "session_id": sid }))
                    .await?
            } else {
                backend.try_ipc("sessions.disconnect_all", json!({})).await?
            } {
                (BackendSource::LocalIpc, map_ipc_response(resp)?)
            } else {
                let token = backend.require_token()?;
                let http = backend.http_client()?;
                let v = if let Some(sid) = session_id {
                    http.post_json(
                        "/api/v1/remote/sessions/disconnect",
                        Some(token),
                        &json!({ "session_id": sid }),
                    )
                    .await?
                } else {
                    http.post_json(
                        "/api/v1/remote/sessions/disconnect-all",
                        Some(token),
                        &json!({}),
                    )
                    .await?
                };
                (BackendSource::HttpServer, v)
            };
            render_output(opts, val, render_default_text)
        }
        DeviceOp::Status => {
            let (_src, val) = if let Some(resp) = backend.try_ipc("device.status", json!({})).await? {
                (BackendSource::LocalIpc, map_ipc_response(resp)?)
            } else if let Some(resp) = backend.try_ipc("sessions.list", json!({})).await? {
                (BackendSource::LocalIpc, map_ipc_response(resp)?)
            } else {
                let token = backend.require_token()?;
                let http = backend.http_client()?;
                let v = http
                    .get_json("/api/v1/remote/sessions", Some(token), &[])
                    .await?;
                (BackendSource::HttpServer, v)
            };
            render_output(opts, val, render_default_text)
        }
        DeviceOp::Power {
            action,
            target,
            mac_address,
            broadcast_ip,
            delay_minutes,
        } => {
            let node_id = match target {
                Some(ref t) => resolve_target_node_id(backend, t, false).await?,
                None => "local".to_string(),
            };
            let power_kind = match action.as_str() {
                "lock" => PowerActionKind::Lock,
                "sleep" => PowerActionKind::Sleep,
                "reboot" => PowerActionKind::Reboot,
                "shutdown" => match delay_minutes {
                    Some(delay) if delay > 0 => {
                        PowerActionKind::ScheduleShutdown { delay_minutes: delay }
                    }
                    _ => PowerActionKind::Shutdown,
                },
                "wol" => PowerActionKind::WakeOnLan {
                    mac_address: mac_address.clone().ok_or_else(|| {
                        CliError::Usage("'device power wol' requires --mac <MAC_ADDRESS>".into())
                    })?,
                    broadcast_ip: broadcast_ip.clone(),
                },
                other => {
                    return Err(CliError::Validation(format!(
                        "Unsupported power action '{other}'"
                    )))
                }
            };
            let power_val = serde_json::to_value(&power_kind).unwrap_or(Value::Null);
            let ipc_params = json!({
                "node_id": node_id,
                "action": power_val,
            });
            let (_src, val) = if let Some(resp) = backend.try_ipc("device.power", ipc_params).await? {
                (BackendSource::LocalIpc, map_ipc_response(resp)?)
            } else if let Some(resp) = backend.try_ipc("power.action", power_val).await? {
                (BackendSource::LocalIpc, map_ipc_response(resp)?)
            } else if let PowerActionKind::WakeOnLan {
                ref mac_address,
                ref broadcast_ip,
            } = power_kind
            {
                if backend.token().is_none() || opts.local {
                    fabushi_remote_engine::power::send_wake_on_lan(
                        mac_address,
                        broadcast_ip.as_deref(),
                    )
                    .map_err(|e| CliError::Execution(e.to_string()))?;
                    (
                        BackendSource::LocalIpc,
                        json!({
                            "dispatched": true,
                            "action": "wake_on_lan",
                            "mac_address": mac_address,
                            "broadcast_ip": broadcast_ip,
                        }),
                    )
                } else {
                    let v = exec_remote_computer_op_http(
                        backend,
                        &node_id,
                        RemoteComputerOp::PowerAction {
                            action: power_kind.clone(),
                        },
                    )
                    .await?;
                    (BackendSource::HttpServer, v)
                }
            } else {
                let v = exec_remote_computer_op_http(
                    backend,
                    &node_id,
                    RemoteComputerOp::PowerAction {
                        action: power_kind.clone(),
                    },
                )
                .await?;
                (BackendSource::HttpServer, v)
            };
            render_output(opts, val, render_default_text)
        }
    }
}

async fn handle_cloudpc(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: CloudPcOp,
) -> Result<CliOutcome, CliError> {
    match op {
        CloudPcOp::List => {
            let (_src, nodes) = fetch_computers_list(backend, true).await?;
            let count = nodes.len();
            render_output(
                opts,
                json!({ "cloud_pcs": nodes, "count": count }),
                render_default_text,
            )
        }
        CloudPcOp::Launch { target } => {
            let node_id = resolve_target_node_id(backend, &target, true).await?;
            let (_src, val) = backend
                .call_ipc_or_http(
                    "cloudpc.launch",
                    json!({ "node_id": node_id }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'cloudpc launch'".into())
                        })?;
                        http.post_json(
                            &format!("/api/v1/computers/{node_id}/test"),
                            Some(&token),
                            &json!({}),
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        CloudPcOp::Shutdown { target } => {
            let node_id = resolve_target_node_id(backend, &target, true).await?;
            let (_src, val) = backend
                .call_ipc_or_http(
                    "cloudpc.shutdown",
                    json!({ "node_id": node_id }),
                    |_, _| async {
                        exec_remote_computer_op_http(
                            backend,
                            &node_id,
                            RemoteComputerOp::PowerAction {
                                action: PowerActionKind::Shutdown,
                            },
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        CloudPcOp::Connect { target } => {
            let node_id = resolve_target_node_id(backend, &target, true).await?;
            let (_src, val) = backend
                .call_ipc_or_http(
                    "cloudpc.connect",
                    json!({ "node_id": node_id }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'cloudpc connect'".into())
                        })?;
                        http.post_json(
                            &format!("/api/v1/computers/{node_id}/test"),
                            Some(&token),
                            &json!({ "mode": "desktop" }),
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        CloudPcOp::Disconnect { target } => {
            let node_id = match target {
                Some(ref t) => Some(resolve_target_node_id(backend, t, true).await?),
                None => None,
            };
            let (_src, val) = backend
                .call_ipc_or_http(
                    "cloudpc.disconnect",
                    json!({ "node_id": node_id }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'cloudpc disconnect'".into())
                        })?;
                        http.post_json(
                            "/api/v1/remote/sessions/disconnect-all",
                            Some(&token),
                            &json!({}),
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        CloudPcOp::Status { target } => {
            let node_id = match target {
                Some(ref t) => Some(resolve_target_node_id(backend, t, true).await?),
                None => None,
            };
            let (_src, val) = backend
                .call_ipc_or_http(
                    "cloudpc.status",
                    json!({ "node_id": node_id }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'cloudpc status'".into())
                        })?;
                        http.get_json("/api/v1/remote/sessions", Some(&token), &[])
                            .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
    }
}

async fn handle_assist(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: AssistOp,
) -> Result<CliOutcome, CliError> {
    match op {
        AssistOp::Id => {
            let (_src, val) = backend
                .call_ipc_or_http("assist.id", json!({}), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'assist id'".into())
                    })?;
                    http.get_json("/api/v1/remote/assist/info", Some(&token), &[])
                        .await
                })
                .await?;
            render_output(opts, val, |v| {
                format!(
                    "Assist ID: {} | Allow: {}",
                    v["assist_id"].as_str().unwrap_or("-"),
                    v["allow_assist"].as_bool().unwrap_or(false)
                )
            })
        }
        AssistOp::ResetCode => {
            let (_src, val) = backend
                .call_ipc_or_http("assist.reset_code", json!({}), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'assist reset-code'".into())
                    })?;
                    http.post_json("/api/v1/remote/assist/reset-code", Some(&token), &json!({}))
                        .await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        AssistOp::SetCode { code } => {
            let body = serde_json::to_value(AssistSetCodeRequest { code: code.clone() })
                .unwrap_or_else(|_| json!({ "code": code }));
            let (_src, val) = backend
                .call_ipc_or_http("assist.set_code", body.clone(), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'assist set-code'".into())
                    })?;
                    http.post_json("/api/v1/remote/assist/set-code", Some(&token), &body)
                        .await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        AssistOp::Allow { enabled } => {
            let body = serde_json::to_value(AssistAllowRequest {
                allow_assist: enabled,
            })
            .unwrap_or_else(|_| json!({ "allow_assist": enabled }));
            let (_src, val) = backend
                .call_ipc_or_http("assist.allow", body.clone(), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'assist allow'".into())
                    })?;
                    http.post_json("/api/v1/remote/assist/allow", Some(&token), &body)
                        .await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        AssistOp::Start { assist_id, code } => {
            let access_code = code.ok_or_else(|| {
                CliError::Usage("'assist start' requires <ASSIST_ID> <CODE>".into())
            })?;
            let req = AssistStartRequest {
                assist_id,
                access_code,
                requester_device_id: backend
                    .saved_session()
                    .and_then(|s| s.device_id.clone()),
                requester_name: Some("Fabushi CLI".to_string()),
                requester_platform: Some(std::env::consts::OS.to_string()),
                mode: RemoteControlMode::Desktop,
            };
            let body = serde_json::to_value(req)
                .map_err(|e| CliError::Execution(format!("Serialize AssistStartRequest: {e}")))?;
            let (_src, val) = backend
                .call_ipc_or_http("assist.start", body.clone(), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'assist start'".into())
                    })?;
                    http.post_json("/api/v1/remote/assist/start", Some(&token), &body)
                        .await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
    }
}

async fn handle_term(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: TermOp,
) -> Result<CliOutcome, CliError> {
    match op {
        TermOp::List { target } => {
            let node_id = match target {
                Some(ref t) => Some(resolve_target_node_id(backend, t, false).await?),
                None => None,
            };
            let (_src, val) = backend
                .call_ipc_or_http(
                    "term.ls",
                    json!({ "node_id": node_id }),
                    |_, _| async {
                        let nid = node_id.clone().ok_or_else(|| {
                            CliError::Usage("'term ls' requires <ID|--m> when IPC is offline".into())
                        })?;
                        exec_remote_computer_op_http(
                            backend,
                            &nid,
                            RemoteComputerOp::TerminalOp {
                                op: "ls".into(),
                                session_name: None,
                                shell: None,
                                input: None,
                                command: None,
                                cols: None,
                                rows: None,
                                timeout_ms: None,
                                new_name: None,
                            },
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        TermOp::Open {
            target,
            session_name,
            shell,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let resolved_name = session_name.clone().unwrap_or_else(|| format!("term-{node_id}"));
            let params = json!({
                "node_id": node_id,
                "name": resolved_name,
                "session_name": session_name,
                "shell": shell,
            });
            let (_src, val) = if let Some(resp) = backend.try_ipc("term.open", params.clone()).await? {
                (BackendSource::LocalIpc, map_ipc_response(resp)?)
            } else if let Some(resp) = backend.try_ipc("term.new", params).await? {
                (BackendSource::LocalIpc, map_ipc_response(resp)?)
            } else {
                let v = exec_remote_computer_op_http(
                    backend,
                    &node_id,
                    RemoteComputerOp::TerminalOp {
                        op: "new".into(),
                        session_name: session_name.clone(),
                        shell: shell.clone(),
                        input: None,
                        command: None,
                        cols: None,
                        rows: None,
                        timeout_ms: None,
                        new_name: None,
                    },
                )
                .await?;
                (BackendSource::HttpServer, v)
            };
            render_output(opts, val, render_default_text)
        }
        TermOp::Exit {
            target,
            session_name,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let resolved_name = session_name.clone().unwrap_or_else(|| format!("term-{node_id}"));
            let params = json!({
                "node_id": node_id,
                "name": resolved_name,
                "session_name": session_name,
            });
            let (_src, val) = if let Some(resp) = backend.try_ipc("term.exit", params.clone()).await? {
                (BackendSource::LocalIpc, map_ipc_response(resp)?)
            } else if let Some(resp) = backend.try_ipc("term.kill", params).await? {
                (BackendSource::LocalIpc, map_ipc_response(resp)?)
            } else {
                let v = exec_remote_computer_op_http(
                    backend,
                    &node_id,
                    RemoteComputerOp::TerminalOp {
                        op: "kill".into(),
                        session_name: session_name.clone(),
                        shell: None,
                        input: None,
                        command: None,
                        cols: None,
                        rows: None,
                        timeout_ms: None,
                        new_name: None,
                    },
                )
                .await?;
                (BackendSource::HttpServer, v)
            };
            render_output(opts, val, render_default_text)
        }
        TermOp::Exec {
            target,
            session_name,
            timeout_ms,
            command,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let resolved_name = session_name.clone().unwrap_or_else(|| format!("term-{node_id}"));
            let (_src, val) = backend
                .call_ipc_or_http(
                    "term.exec",
                    json!({
                        "node_id": node_id,
                        "name": resolved_name,
                        "session_name": session_name,
                        "timeout_ms": timeout_ms,
                        "command": command,
                    }),
                    |_, _| async {
                        exec_remote_computer_op_http(
                            backend,
                            &node_id,
                            RemoteComputerOp::TerminalOp {
                                op: "exec".into(),
                                session_name: session_name.clone(),
                                shell: None,
                                input: None,
                                command: Some(command.clone()),
                                cols: None,
                                rows: None,
                                timeout_ms,
                                new_name: None,
                            },
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, |v| {
                if let Some(output) = v.get("output").or_else(|| v.get("stdout")).and_then(|s| s.as_str()) {
                    output.to_string()
                } else {
                    render_default_text(v)
                }
            })
        }
    }
}

async fn handle_lterm(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: LtermOp,
) -> Result<CliOutcome, CliError> {
    match op {
        LtermOp::Ls => {
            let val = backend.call_ipc_required("lterm.ls", json!({})).await?;
            render_output(opts, val, render_default_text)
        }
        LtermOp::New {
            name,
            shell,
            cols,
            rows,
            detach,
        } => {
            if is_inside_mux() {
                return Err(CliError::Forbidden(
                    "Refusing recursive 'lterm new' inside an active Fabushi mux session (FABUSHI_IN_MUX=1); detach first with Ctrl+] d.".into(),
                ));
            }
            let val = backend
                .call_ipc_required(
                    "lterm.new",
                    json!({
                        "name": name,
                        "shell": shell,
                        "cols": cols.unwrap_or(120),
                        "rows": rows.unwrap_or(36),
                    }),
                )
                .await?;

            if !detach
                && opts.output_format.is_none()
                && !opts.non_interactive
                && std::io::stdin().is_terminal()
                && std::io::stdout().is_terminal()
            {
                let created_name = val
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or(name);
                let _code = run_local_tty_attach(
                    &backend.ipc_client(),
                    TtyAttachOptions {
                        session_name: created_name,
                        shell,
                        create_if_missing: false,
                        ignore_size: true,
                    },
                )
                .await
                .map_err(|e| CliError::Execution(e.to_string()))?;
                return Ok(CliOutcome::ok(String::new()));
            }
            render_output(opts, val, render_default_text)
        }
        LtermOp::Attach { name } => {
            if is_inside_mux() {
                return Err(CliError::Forbidden(
                    "Refusing recursive 'lterm attach' inside an active Fabushi mux session (FABUSHI_IN_MUX=1); detach first with Ctrl+] d.".into(),
                ));
            }
            let code = run_local_tty_attach(
                &backend.ipc_client(),
                TtyAttachOptions {
                    session_name: name,
                    shell: None,
                    create_if_missing: false,
                    ignore_size: true,
                },
            )
            .await
            .map_err(|e| CliError::Execution(e.to_string()))?;
            Ok(CliOutcome::with_exit_code(String::new(), code))
        }
        LtermOp::Has { name } => {
            let client = backend.ipc_client();
            let resp = client
                .call("lterm.has", json!({ "name": name }))
                .await
                .map_err(|e| {
                    CliError::CheckError(format!(
                        "Failed to query local terminal session '{name}' via IPC: {e}"
                    ))
                })?;

            let exists = if resp.success {
                resp.data
                    .as_ref()
                    .and_then(|d| d.get("exists"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            } else if resp
                .error
                .as_ref()
                .map(|e| e.code.as_str())
                == Some("not_found")
            {
                false
            } else {
                let msg = resp
                    .error
                    .map(|e| e.message)
                    .unwrap_or_else(|| "lterm.has IPC check failed".into());
                return Err(CliError::CheckError(msg));
            };

            let data = json!({ "name": name, "exists": exists });
            let out_str = if opts.output_format == Some(OutputFormat::Text) {
                if exists { "exists".to_string() } else { "missing".to_string() }
            } else {
                format_json(&ok_envelope(data), opts.pretty)?
            };
            Ok(CliOutcome::with_exit_code(
                out_str,
                if exists { 0 } else { 1 },
            ))
        }
        LtermOp::Kill { name } => {
            let val = backend
                .call_ipc_required("lterm.kill", json!({ "name": name }))
                .await?;
            render_output(opts, val, render_default_text)
        }
        LtermOp::Rename { old_name, new_name } => {
            let val = backend
                .call_ipc_required(
                    "lterm.rename",
                    json!({ "old_name": old_name, "new_name": new_name }),
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
    }
}

async fn handle_input(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: InputOp,
) -> Result<CliOutcome, CliError> {
    let (ipc_method, daemon_method, ipc_params, daemon_params, node_id, remote_op) = match op {
        InputOp::Click {
            target,
            x,
            y,
            button,
            clicks,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let dp = json!({ "action": "click", "x": x, "y": y, "button": button, "clicks": clicks, "relative": false });
            (
                "input.click",
                "input.mouse",
                json!({ "node_id": node_id, "x": x, "y": y, "button": button, "clicks": clicks }),
                dp,
                node_id,
                RemoteComputerOp::MouseAction {
                    action: "click".into(),
                    x,
                    y,
                    dx: None,
                    dy: None,
                    button: Some(button),
                    clicks: Some(clicks),
                    scroll_x: None,
                    scroll_y: None,
                    relative: false,
                },
            )
        }
        InputOp::Move {
            target,
            x,
            y,
            dx,
            dy,
            relative,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let dp = json!({ "action": "move", "x": x, "y": y, "dx": dx, "dy": dy, "relative": relative });
            (
                "input.move",
                "input.mouse",
                json!({ "node_id": node_id, "x": x, "y": y, "dx": dx, "dy": dy, "relative": relative }),
                dp,
                node_id,
                RemoteComputerOp::MouseAction {
                    action: "move".into(),
                    x,
                    y,
                    dx,
                    dy,
                    button: None,
                    clicks: None,
                    scroll_x: None,
                    scroll_y: None,
                    relative,
                },
            )
        }
        InputOp::Drag {
            target,
            x,
            y,
            dx,
            dy,
            button,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let dp = json!({ "action": "drag", "x": x, "y": y, "dx": dx, "dy": dy, "button": button, "relative": false });
            (
                "input.drag",
                "input.mouse",
                json!({ "node_id": node_id, "x": x, "y": y, "dx": dx, "dy": dy, "button": button }),
                dp,
                node_id,
                RemoteComputerOp::MouseAction {
                    action: "drag".into(),
                    x,
                    y,
                    dx,
                    dy,
                    button: Some(button),
                    clicks: None,
                    scroll_x: None,
                    scroll_y: None,
                    relative: false,
                },
            )
        }
        InputOp::Scroll {
            target,
            x,
            y,
            scroll_x,
            scroll_y,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let dp = json!({ "action": "scroll", "x": x, "y": y, "scroll_x": scroll_x, "scroll_y": scroll_y, "relative": false });
            (
                "input.scroll",
                "input.mouse",
                json!({ "node_id": node_id, "x": x, "y": y, "scroll_x": scroll_x, "scroll_y": scroll_y }),
                dp,
                node_id,
                RemoteComputerOp::MouseAction {
                    action: "scroll".into(),
                    x,
                    y,
                    dx: None,
                    dy: None,
                    button: None,
                    clicks: None,
                    scroll_x: Some(scroll_x),
                    scroll_y: Some(scroll_y),
                    relative: false,
                },
            )
        }
        InputOp::Type { target, text } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let dp = json!({ "action": "type", "text": text, "modifiers": [] });
            (
                "input.type",
                "input.keyboard",
                json!({ "node_id": node_id, "text": text }),
                dp,
                node_id,
                RemoteComputerOp::KeyboardAction {
                    action: "type".into(),
                    text: Some(text),
                    key: None,
                    modifiers: Vec::new(),
                },
            )
        }
        InputOp::Hotkey {
            target,
            key,
            modifiers,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let dp = json!({ "action": "hotkey", "key": key, "modifiers": modifiers });
            (
                "input.hotkey",
                "input.keyboard",
                json!({ "node_id": node_id, "key": key, "modifiers": modifiers }),
                dp,
                node_id,
                RemoteComputerOp::KeyboardAction {
                    action: "hotkey".into(),
                    text: None,
                    key: Some(key),
                    modifiers,
                },
            )
        }
    };

    let (_src, val) = if let Some(resp) = backend.try_ipc(ipc_method, ipc_params).await? {
        (BackendSource::LocalIpc, map_ipc_response(resp)?)
    } else if let Some(resp) = backend.try_ipc(daemon_method, daemon_params).await? {
        (BackendSource::LocalIpc, map_ipc_response(resp)?)
    } else {
        let v = exec_remote_computer_op_http(backend, &node_id, remote_op).await?;
        (BackendSource::HttpServer, v)
    };
    render_output(opts, val, render_default_text)
}

async fn handle_clipboard(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: ClipboardOp,
) -> Result<CliOutcome, CliError> {
    match op {
        ClipboardOp::Get { target } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let (_src, val) = backend
                .call_ipc_or_http(
                    "clipboard.get",
                    json!({ "node_id": node_id }),
                    |_, _| async {
                        exec_remote_computer_op_http(
                            backend,
                            &node_id,
                            RemoteComputerOp::ClipboardGet,
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, |v| {
                v.get("text")
                    .and_then(|s| s.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| render_default_text(v))
            })
        }
        ClipboardOp::Set {
            target,
            text,
            image_b64,
            mime_type,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let effective_mime = mime_type.clone().unwrap_or_else(|| {
                if image_b64.is_some() {
                    "image/png".to_string()
                } else {
                    "text/plain".to_string()
                }
            });
            let (_src, val) = backend
                .call_ipc_or_http(
                    "clipboard.set",
                    json!({
                        "node_id": node_id,
                        "text": text,
                        "image_b64": image_b64,
                        "mime_type": effective_mime,
                    }),
                    |_, _| async {
                        exec_remote_computer_op_http(
                            backend,
                            &node_id,
                            RemoteComputerOp::ClipboardSet {
                                text: text.clone(),
                                image_b64: image_b64.clone(),
                                mime_type: mime_type.clone(),
                            },
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
    }
}

async fn handle_file(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: FileOp,
) -> Result<CliOutcome, CliError> {
    match op {
        FileOp::Ls { target, path } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let (_src, val) = backend
                .call_ipc_or_http(
                    "file.ls",
                    json!({ "node_id": node_id, "path": path }),
                    |_, _| async {
                        exec_remote_computer_op_http(
                            backend,
                            &node_id,
                            RemoteComputerOp::ListDir { path: path.clone() },
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        FileOp::Read {
            target,
            path,
            max_bytes,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let (_src, val) = backend
                .call_ipc_or_http(
                    "file.read",
                    json!({ "node_id": node_id, "path": path, "max_bytes": max_bytes }),
                    |_, _| async {
                        exec_remote_computer_op_http(
                            backend,
                            &node_id,
                            RemoteComputerOp::ReadFile {
                                path: path.clone(),
                                max_bytes,
                            },
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        FileOp::Write {
            target,
            path,
            content,
            from_local,
            create_dirs,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let body = match content {
                Some(c) => c,
                None => {
                    let local_file = from_local.ok_or_else(|| {
                        CliError::Usage("'file write' requires content or --from".into())
                    })?;
                    std::fs::read_to_string(&local_file).map_err(|e| {
                        CliError::Execution(format!("Failed reading '{local_file}': {e}"))
                    })?
                }
            };
            let (_src, val) = backend
                .call_ipc_or_http(
                    "file.write",
                    json!({
                        "node_id": node_id,
                        "path": path,
                        "content": body,
                        "create_dirs": create_dirs,
                    }),
                    |_, _| async {
                        exec_remote_computer_op_http(
                            backend,
                            &node_id,
                            RemoteComputerOp::WriteFile {
                                path: path.clone(),
                                content: body.clone(),
                                create_dirs,
                            },
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        FileOp::Push {
            target,
            local_path,
            remote_path,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let bytes = std::fs::read(&local_path).map_err(|e| {
                CliError::Execution(format!("Failed reading local file '{local_path}': {e}"))
            })?;
            let content_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let text_opt = String::from_utf8(bytes.clone()).ok();
            let (_src, val) = backend
                .call_ipc_or_http(
                    "file.push",
                    json!({
                        "node_id": node_id,
                        "local_path": local_path,
                        "remote_path": remote_path,
                        "data_b64": content_b64,
                        "size": bytes.len(),
                    }),
                    |_, _| async {
                        let write_text = text_opt.clone().unwrap_or_else(|| content_b64.clone());
                        exec_remote_computer_op_http(
                            backend,
                            &node_id,
                            RemoteComputerOp::WriteFile {
                                path: remote_path.clone(),
                                content: write_text,
                                create_dirs: true,
                            },
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        FileOp::Pull {
            target,
            remote_path,
            local_path,
        } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let (_src, mut val) = backend
                .call_ipc_or_http(
                    "file.pull",
                    json!({
                        "node_id": node_id,
                        "remote_path": remote_path,
                        "local_path": local_path,
                    }),
                    |_, _| async {
                        exec_remote_computer_op_http(
                            backend,
                            &node_id,
                            RemoteComputerOp::ReadFile {
                                path: remote_path.clone(),
                                max_bytes: None,
                            },
                        )
                        .await
                    },
                )
                .await?;
            let written_bytes = if let Some(b64) = val.get("data_b64").and_then(|v| v.as_str()) {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(b64.trim())
                    .map_err(|e| {
                        CliError::Execution(format!("Invalid base64 in pulled file: {e}"))
                    })?;
                std::fs::write(&local_path, &decoded).map_err(|e| {
                    CliError::Execution(format!("Failed writing '{local_path}': {e}"))
                })?;
                decoded.len()
            } else if let Some(text) = val
                .get("content")
                .or_else(|| val.get("stdout"))
                .and_then(|v| v.as_str())
            {
                std::fs::write(&local_path, text.as_bytes()).map_err(|e| {
                    CliError::Execution(format!("Failed writing '{local_path}': {e}"))
                })?;
                text.len()
            } else {
                return Err(CliError::Execution(
                    "Remote file pull response did not contain content or data_b64".into(),
                ));
            };
            if let Some(obj) = val.as_object_mut() {
                obj.insert("saved_path".into(), Value::String(local_path.clone()));
                obj.insert("bytes_written".into(), json!(written_bytes));
            }
            render_output(opts, val, |v| {
                format!(
                    "Pulled {} bytes to {}",
                    v["bytes_written"].as_u64().unwrap_or(0),
                    v["saved_path"].as_str().unwrap_or("-")
                )
            })
        }
    }
}

async fn handle_chat(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: ChatOp,
) -> Result<CliOutcome, CliError> {
    match op {
        ChatOp::List { limit } => {
            let (_src, val) = backend
                .call_ipc_or_http(
                    "chat.list",
                    json!({ "limit": limit }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'chat list'".into())
                        })?;
                        http.get_json("/api/v1/chats", Some(&token), &[]).await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        ChatOp::Info { chat_id } => {
            let (_src, val) = backend
                .call_ipc_or_http(
                    "chat.info",
                    json!({ "chat_id": chat_id }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'chat info'".into())
                        })?;
                        http.get_json(&format!("/api/v1/chats/{chat_id}"), Some(&token), &[])
                            .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        ChatOp::Create {
            title,
            chat_type,
            member_ids,
        } => {
            let payload = json!({
                "title": title,
                "chat_type": chat_type,
                "member_ids": member_ids,
            });
            let (_src, val) = backend
                .call_ipc_or_http("chat.create", payload.clone(), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'chat create'".into())
                    })?;
                    http.post_json("/api/v1/chats", Some(&token), &payload).await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
    }
}

async fn handle_msg(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: MsgOp,
) -> Result<CliOutcome, CliError> {
    match op {
        MsgOp::Send {
            chat_id,
            text,
            reply_to_id,
        } => {
            let mut payload = json!({
                "chat_id": chat_id,
                "text": text,
            });
            if let Some(rid) = reply_to_id {
                payload["reply_to_id"] = json!(rid);
            }
            let (_src, val) = backend
                .call_ipc_or_http("msg.send", payload.clone(), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'msg send'".into())
                    })?;
                    http.post_json(
                        &format!("/api/v1/chats/{chat_id}/messages"),
                        Some(&token),
                        &payload,
                    )
                    .await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        MsgOp::History {
            chat_id,
            limit,
            offset_id,
        } => {
            let (_src, val) = backend
                .call_ipc_or_http(
                    "msg.history",
                    json!({ "chat_id": chat_id, "limit": limit, "offset_id": offset_id }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'msg history'".into())
                        })?;
                        let mut q = vec![("limit", limit.to_string())];
                        if let Some(oid) = offset_id {
                            q.push(("offset_id", oid.to_string()));
                        }
                        http.get_json(
                            &format!("/api/v1/chats/{chat_id}/messages"),
                            Some(&token),
                            &q,
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        MsgOp::Search {
            query,
            chat_id,
            limit,
        } => {
            let (_src, val) = backend
                .call_ipc_or_http(
                    "msg.search",
                    json!({ "query": query, "chat_id": chat_id, "limit": limit }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'msg search'".into())
                        })?;
                        let mut q = vec![("q", query.clone()), ("limit", limit.to_string())];
                        if let Some(cid) = chat_id {
                            q.push(("chat_id", cid.to_string()));
                        }
                        http.get_json("/api/v1/search/messages", Some(&token), &q)
                            .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        MsgOp::Delete {
            chat_id,
            message_ids,
            for_everyone,
        } => {
            let payload = json!({
                "chat_id": chat_id,
                "message_ids": message_ids,
                "revoke": for_everyone,
            });
            let (_src, val) = backend
                .call_ipc_or_http("msg.delete", payload.clone(), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'msg delete'".into())
                    })?;
                    http.post_json("/api/v1/messages/delete", Some(&token), &payload)
                        .await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        MsgOp::React {
            chat_id,
            message_id,
            emoji,
        } => {
            let payload = json!({
                "emoji": emoji,
                "action": "toggle",
            });
            let (_src, val) = backend
                .call_ipc_or_http(
                    "msg.react",
                    json!({ "chat_id": chat_id, "message_id": message_id, "emoji": emoji }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'msg react'".into())
                        })?;
                        http.post_json(
                            &format!("/api/v1/chats/{chat_id}/messages/{message_id}/reactions"),
                            Some(&token),
                            &payload,
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
    }
}

async fn handle_bot(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: BotOp,
) -> Result<CliOutcome, CliError> {
    match op {
        BotOp::List => {
            let (_src, val) = backend
                .call_ipc_or_http("bot.list", json!({}), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'bot list'".into())
                    })?;
                    http.get_json("/api/v1/fabushi-bots", Some(&token), &[])
                        .await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        BotOp::BindComputer { bot_id, target } => {
            let node_id = resolve_target_node_id(backend, &target, false).await?;
            let payload = json!({
                "bound_node_id": node_id,
                "clear_bound_node": false,
            });
            let (_src, val) = backend
                .call_ipc_or_http(
                    "bot.bind_computer",
                    json!({ "bot_id": bot_id, "node_id": node_id }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized(
                                "Bearer token required for 'bot bind-computer'".into(),
                            )
                        })?;
                        http.put_json(
                            &format!("/api/v1/bots/{bot_id}/computer"),
                            Some(&token),
                            &payload,
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
    }
}

async fn handle_miniapp(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: MiniAppOp,
) -> Result<CliOutcome, CliError> {
    match op {
        MiniAppOp::List => {
            let (_src, val) = backend
                .call_ipc_or_http("miniapp.list", json!({}), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'miniapp list'".into())
                    })?;
                    http.get_json("/api/v1/miniapps/installed", Some(&token), &[])
                        .await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        MiniAppOp::WebMcpCall {
            bot_username,
            short_name,
            tool_name,
            args,
        } => {
            let payload = json!({
                "action": "webmcp_call",
                "tool": tool_name,
                "arguments": args,
            });
            let (_src, val) = backend
                .call_ipc_or_http(
                    "miniapp.webmcp_call",
                    json!({
                        "bot_username": bot_username,
                        "short_name": short_name,
                        "tool_name": tool_name,
                        "arguments": args,
                    }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized(
                                "Bearer token required for 'miniapp webmcp-call'".into(),
                            )
                        })?;
                        http.post_json(
                            &format!("/api/v1/miniapps/{bot_username}/{short_name}/action"),
                            Some(&token),
                            &payload,
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
    }
}

async fn handle_task(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: TaskOp,
) -> Result<CliOutcome, CliError> {
    match op {
        TaskOp::List => {
            let (_src, val) = backend
                .call_ipc_or_http("task.list", json!({}), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'task list'".into())
                    })?;
                    http.get_json("/api/v1/tasks", Some(&token), &[]).await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        TaskOp::Create {
            title,
            target_node_id,
        } => {
            let mut payload = json!({ "title": title, "goal": title });
            if let Some(nid) = target_node_id {
                payload["target_node_id"] = Value::String(nid);
            }
            let (_src, val) = backend
                .call_ipc_or_http("task.create", payload.clone(), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'task create'".into())
                    })?;
                    http.post_json("/api/v1/tasks", Some(&token), &payload).await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        TaskOp::Approve {
            task_id,
            decision,
            reason,
        } => {
            let mut payload = json!({
                "decision": decision,
                "approved": decision != "deny",
            });
            if let Some(r) = reason {
                payload["reason"] = Value::String(r);
            }
            let (_src, val) = backend
                .call_ipc_or_http(
                    "task.approve",
                    json!({ "task_id": task_id, "decision": decision, "reason": payload.get("reason") }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'task approve'".into())
                        })?;
                        http.post_json(
                            &format!("/api/v1/tasks/{task_id}/approve"),
                            Some(&token),
                            &payload,
                        )
                        .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
    }
}

async fn handle_mcp(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    op: McpOp,
) -> Result<CliOutcome, CliError> {
    match op {
        McpOp::Status { url } => {
            let mcp_url = normalize_mcp_url(
                url.as_deref(),
                backend.server_url(),
                opts.server_explicit,
            );
            let base = mcp_url.strip_suffix("/mcp").unwrap_or(&mcp_url).to_string();
            let (_src, val) = backend
                .call_ipc_or_http("mcp.status", json!({ "mcp_url": mcp_url }), |http, _| async move {
                    let client = if url.is_some() {
                        crate::http_client::ActionHttpClient::new(&base)?
                    } else {
                        http
                    };
                    let meta = client
                        .get_json("/.well-known/oauth-protected-resource", None, &[])
                        .await?;
                    Ok(json!({
                        "mcp_url": mcp_url,
                        "protected_resource": meta,
                    }))
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        McpOp::Grants => {
            let (_src, val) = backend
                .call_ipc_or_http("mcp.grants", json!({}), |http, tok| async move {
                    let token = tok.ok_or_else(|| {
                        CliError::Unauthorized("Bearer token required for 'mcp grants'".into())
                    })?;
                    http.get_json("/api/v1/mcp/grants", Some(&token), &[])
                        .await
                })
                .await?;
            render_output(opts, val, render_default_text)
        }
        McpOp::Revoke { family_id } => {
            let (_src, val) = backend
                .call_ipc_or_http(
                    "mcp.revoke",
                    json!({ "family_id": family_id }),
                    |http, tok| async move {
                        let token = tok.ok_or_else(|| {
                            CliError::Unauthorized("Bearer token required for 'mcp revoke'".into())
                        })?;
                        http.delete_json(&format!("/api/v1/mcp/grants/{family_id}"), Some(&token), &[])
                            .await
                    },
                )
                .await?;
            render_output(opts, val, render_default_text)
        }
        McpOp::Config { client, url } => {
            let mcp_url = normalize_mcp_url(
                url.as_deref(),
                backend.server_url(),
                opts.server_explicit,
            );
            let cfg = build_client_config(client, &mcp_url);
            render_output(opts, cfg, |v| {
                serde_json::to_string_pretty(&v.get("config").unwrap_or(v))
                    .unwrap_or_else(|_| v.to_string())
            })
        }
        McpOp::ServeStdio { url } => {
            let mcp_url = normalize_mcp_url(
                url.as_deref(),
                backend.server_url(),
                opts.server_explicit,
            );
            let http = backend.http_client()?;
            let token = backend.token().map(String::from);
            let mut mcp_session_id: Option<String> = None;

            let stdin = std::io::stdin();
            let mut stdout = std::io::stdout();
            for line_res in stdin.lock().lines() {
                let line = line_res.map_err(|e| {
                    CliError::Execution(format!("Failed reading MCP stdio input: {e}"))
                })?;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let rpc_req: Value = serde_json::from_str(trimmed).map_err(|e| {
                    CliError::Validation(format!("Invalid JSON-RPC line on stdin: {e}"))
                })?;
                let rpc_id = rpc_req.get("id").cloned();

                match http
                    .post_mcp_jsonrpc(
                        &mcp_url,
                        token.as_deref(),
                        mcp_session_id.as_deref(),
                        &rpc_req,
                    )
                    .await
                {
                    Ok((next_sid, resp_val)) => {
                        if next_sid.is_some() {
                            mcp_session_id = next_sid;
                        }
                        if rpc_id.is_some() && !resp_val.is_null() {
                            let serialized = serde_json::to_string(&resp_val)
                                .unwrap_or_else(|_| "{}".into());
                            writeln!(stdout, "{serialized}").map_err(|e| {
                                CliError::Execution(format!("Failed writing MCP stdout: {e}"))
                            })?;
                            let _ = stdout.flush();
                        }
                    }
                    Err(err) => {
                        if let Some(id) = rpc_id {
                            let err_rpc = json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": {
                                    "code": -32000,
                                    "message": err.to_string(),
                                }
                            });
                            writeln!(stdout, "{err_rpc}").map_err(|e| {
                                CliError::Execution(format!("Failed writing MCP stdout: {e}"))
                            })?;
                            let _ = stdout.flush();
                        }
                    }
                }
            }
            Ok(CliOutcome::ok(String::new()))
        }
    }
}
