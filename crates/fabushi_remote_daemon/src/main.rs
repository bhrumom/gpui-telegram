use fabushi_remote_engine::accessibility::AccessibilityEngine;
use fabushi_remote_engine::audio::AudioPipeline;
use fabushi_remote_engine::capture::{CaptureFormat, ScreenCaptureEngine};
use fabushi_remote_engine::clipboard::ClipboardSyncEngine;
use fabushi_remote_engine::control_lease::RemoteControlHub;
use fabushi_remote_engine::display_privacy::DisplayPrivacyManager;
use fabushi_remote_engine::file_transfer::FileTransferManager;
use fabushi_remote_engine::input::{KeyboardActionParams, MouseActionParams, NativeInputEngine};
use fabushi_remote_engine::ipc::{
    default_ipc_endpoint, BoxFuture, LocalIpcClient, LocalIpcHandler, LocalIpcServer,
};
use fabushi_remote_engine::power::PowerManager;
use fabushi_remote_engine::terminal::{handle_terminal_ipc_method, PtyMuxServer};
use gpui_telegram_proto::remote_control_contracts::{
    ClipboardPayloadDto, LocalIpcRequest, LocalIpcResponse, PowerActionKind, RemoteSessionDto,
};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonRunMode {
    Daemon,
    Agent,
    Foreground,
}

pub struct DaemonServices {
    pub mode: DaemonRunMode,
    pub control_hub: Arc<RemoteControlHub>,
    pub pty_mux: Arc<PtyMuxServer>,
    pub capture: Arc<ScreenCaptureEngine>,
    pub input: Arc<NativeInputEngine>,
    pub clipboard: Arc<ClipboardSyncEngine>,
    pub display_privacy: Arc<DisplayPrivacyManager>,
    pub accessibility: Arc<AccessibilityEngine>,
    pub audio: Arc<AudioPipeline>,
    pub file_transfer: Arc<FileTransferManager>,
}

impl DaemonServices {
    pub fn new(mode: DaemonRunMode, state_dir: PathBuf) -> anyhow::Result<Self> {
        let host_id = format!("daemon-{}", std::process::id());
        Ok(Self {
            mode,
            control_hub: RemoteControlHub::shared(),
            pty_mux: Arc::new(PtyMuxServer::new()),
            capture: Arc::new(ScreenCaptureEngine::new()),
            input: Arc::new(NativeInputEngine::new()),
            clipboard: Arc::new(ClipboardSyncEngine::new(host_id)),
            display_privacy: Arc::new(DisplayPrivacyManager::new()),
            accessibility: Arc::new(AccessibilityEngine),
            audio: Arc::new(AudioPipeline::new()),
            file_transfer: Arc::new(FileTransferManager::new(state_dir)?),
        })
    }
}

impl LocalIpcHandler for DaemonServices {
    fn handle(&self, req: LocalIpcRequest) -> BoxFuture<'_, LocalIpcResponse> {
        Box::pin(async move {
            if let Some(resp) = handle_terminal_ipc_method(&self.pty_mux, &req).await {
                return resp;
            }
            let p = &req.params;
            match req.method.as_str() {
                "status" => LocalIpcResponse::ok(
                    req.id,
                    serde_json::json!({
                        "service": "fabushi-remote-daemon",
                        "mode": match self.mode {
                            DaemonRunMode::Daemon => "daemon",
                            DaemonRunMode::Agent => "agent",
                            DaemonRunMode::Foreground => "foreground",
                        },
                        "version": env!("CARGO_PKG_VERSION"),
                        "pid": std::process::id(),
                        "active_sessions": self.control_hub.active_count(),
                        "terminal_sessions": self.pty_mux.list_sessions().len(),
                        "privacy_screen": self.display_privacy.is_privacy_screen_enabled(),
                        "override_epoch": self.control_hub.override_epoch(),
                    }),
                ),
                "echo" => LocalIpcResponse::ok(req.id, req.params),
                "sessions.list" => LocalIpcResponse::ok(
                    req.id,
                    serde_json::json!({ "sessions": self.control_hub.list_sessions() }),
                ),
                "sessions.register" => {
                    match serde_json::from_value::<RemoteSessionDto>(p.clone()) {
                        Ok(dto) => {
                            let registered = self.control_hub.register_session(dto);
                            LocalIpcResponse::ok(
                                req.id,
                                serde_json::to_value(registered).unwrap_or_default(),
                            )
                        }
                        Err(err) => LocalIpcResponse::err(
                            req.id,
                            "invalid_params",
                            format!("invalid RemoteSessionDto: {err}"),
                        ),
                    }
                }
                "sessions.disconnect" => {
                    let sid = p.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
                    let disconnected = self.control_hub.disconnect_session(sid);
                    LocalIpcResponse::ok(
                        req.id,
                        serde_json::json!({ "disconnected": disconnected, "session_id": sid }),
                    )
                }
                "sessions.disconnect_all" => {
                    let count = self.control_hub.disconnect_all();
                    LocalIpcResponse::ok(req.id, serde_json::json!({ "disconnected": count }))
                }
                "screen.displays" => match self.capture.list_displays() {
                    Ok(displays) => LocalIpcResponse::ok(
                        req.id,
                        serde_json::json!({ "displays": displays }),
                    ),
                    Err(err) => LocalIpcResponse::err(req.id, "display_list_failed", err.to_string()),
                },
                "screen.capture" => {
                    let display_idx = p
                        .get("display_index")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(self.display_privacy.active_display_index() as u64)
                        as u32;
                    let fmt = match p.get("format").and_then(|v| v.as_str()) {
                        Some("png") => CaptureFormat::Png,
                        _ => CaptureFormat::Jpeg { quality: 82 },
                    };
                    match self.capture.capture_or_cached(display_idx, 120, fmt) {
                        Ok(frame) => LocalIpcResponse::ok(
                            req.id,
                            serde_json::json!({
                                "width": frame.width,
                                "height": frame.height,
                                "mime_type": frame.mime_type,
                                "data_b64": frame.to_base64(),
                                "captured_at_ms": frame.captured_at_ms,
                                "display_id": frame.display_id,
                            }),
                        ),
                        Err(err) => LocalIpcResponse::err(req.id, "capture_failed", err.to_string()),
                    }
                }
                "input.mouse" => match serde_json::from_value::<MouseActionParams>(p.clone()) {
                    Ok(params) => match self.input.execute_mouse_action(&params) {
                        Ok(()) => LocalIpcResponse::ok(req.id, serde_json::json!({ "ok": true })),
                        Err(err) => LocalIpcResponse::err(req.id, "input_failed", err.to_string()),
                    },
                    Err(err) => LocalIpcResponse::err(req.id, "invalid_params", err.to_string()),
                },
                "input.keyboard" => match serde_json::from_value::<KeyboardActionParams>(p.clone()) {
                    Ok(params) => match self.input.execute_keyboard_action(&params) {
                        Ok(()) => LocalIpcResponse::ok(req.id, serde_json::json!({ "ok": true })),
                        Err(err) => LocalIpcResponse::err(req.id, "input_failed", err.to_string()),
                    },
                    Err(err) => LocalIpcResponse::err(req.id, "invalid_params", err.to_string()),
                },
                "input.relative_mouse" => {
                    let enabled = p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                    match self.input.set_relative_mouse(enabled) {
                        Ok(()) => LocalIpcResponse::ok(
                            req.id,
                            serde_json::json!({ "relative_mouse": enabled }),
                        ),
                        Err(err) => LocalIpcResponse::err(req.id, "input_failed", err.to_string()),
                    }
                }
                "clipboard.get" => match self.clipboard.get_local() {
                    Ok(payload) => LocalIpcResponse::ok(
                        req.id,
                        serde_json::to_value(payload).unwrap_or_default(),
                    ),
                    Err(err) => LocalIpcResponse::err(req.id, "clipboard_failed", err.to_string()),
                },
                "clipboard.set" => {
                    if let Ok(payload) = serde_json::from_value::<ClipboardPayloadDto>(p.clone()) {
                        if !payload.origin_id.is_empty() && payload.seq > 0 {
                            match self.clipboard.apply_remote(&payload) {
                                Ok(applied) => LocalIpcResponse::ok(
                                    req.id,
                                    serde_json::json!({ "applied": applied }),
                                ),
                                Err(err) => {
                                    LocalIpcResponse::err(req.id, "clipboard_failed", err.to_string())
                                }
                            }
                        } else if let Some(text) = payload.text.as_deref() {
                            match self.clipboard.set_local_text(text) {
                                Ok(res) => LocalIpcResponse::ok(
                                    req.id,
                                    serde_json::to_value(res).unwrap_or_default(),
                                ),
                                Err(err) => {
                                    LocalIpcResponse::err(req.id, "clipboard_failed", err.to_string())
                                }
                            }
                        } else if let Some(img) = payload.image_b64.as_deref() {
                            match self.clipboard.set_local_image_png_b64(img) {
                                Ok(res) => LocalIpcResponse::ok(
                                    req.id,
                                    serde_json::to_value(res).unwrap_or_default(),
                                ),
                                Err(err) => {
                                    LocalIpcResponse::err(req.id, "clipboard_failed", err.to_string())
                                }
                            }
                        } else {
                            LocalIpcResponse::err(req.id, "invalid_params", "missing clipboard content")
                        }
                    } else {
                        LocalIpcResponse::err(req.id, "invalid_params", "invalid clipboard payload")
                    }
                }
                "display.switch" => {
                    let idx = p.get("display_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    match self.display_privacy.switch_display(idx) {
                        Ok(active) => LocalIpcResponse::ok(
                            req.id,
                            serde_json::json!({ "display_index": active }),
                        ),
                        Err(err) => LocalIpcResponse::err(req.id, "display_failed", err.to_string()),
                    }
                }
                "display.privacy" => {
                    let enabled = p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                    let message = p.get("message").and_then(|v| v.as_str()).map(ToString::to_string);
                    match self.display_privacy.set_privacy_screen(enabled, message) {
                        Ok(state) => LocalIpcResponse::ok(
                            req.id,
                            serde_json::json!({ "privacy_screen": state }),
                        ),
                        Err(err) => LocalIpcResponse::err(req.id, "privacy_failed", err.to_string()),
                    }
                }
                "power.action" => match serde_json::from_value::<PowerActionKind>(p.clone()) {
                    Ok(action) => match PowerManager::execute_power_action(&action) {
                        Ok(msg) => LocalIpcResponse::ok(
                            req.id,
                            serde_json::json!({ "ok": true, "message": msg }),
                        ),
                        Err(err) => LocalIpcResponse::err(req.id, "power_failed", err.to_string()),
                    },
                    Err(err) => LocalIpcResponse::err(req.id, "invalid_params", err.to_string()),
                },
                "ax.tree" => {
                    let max_depth = p.get("max_depth").and_then(|v| v.as_u64()).map(|n| n as u32);
                    match self.accessibility.capture_tree(max_depth) {
                        Ok(tree) => LocalIpcResponse::ok(
                            req.id,
                            serde_json::to_value(tree).unwrap_or_default(),
                        ),
                        Err(err) => LocalIpcResponse::err(req.id, "ax_failed", err.to_string()),
                    }
                }
                "ax.action" => {
                    let element_id = p.get("element_id").and_then(|v| v.as_str()).unwrap_or("");
                    let action = p.get("action").and_then(|v| v.as_str()).unwrap_or("");
                    let value = p.get("value").and_then(|v| v.as_str());
                    match self
                        .accessibility
                        .perform_semantic_action(element_id, action, value)
                    {
                        Ok(()) => LocalIpcResponse::ok(req.id, serde_json::json!({ "ok": true })),
                        Err(err) => LocalIpcResponse::err(req.id, "ax_failed", err.to_string()),
                    }
                }
                "audio.mic_passthrough" => {
                    let enabled = p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                    let _ = self.audio.set_mic_passthrough(enabled);
                    LocalIpcResponse::ok(req.id, serde_json::json!({ "mic_passthrough": enabled }))
                }
                "file.tasks" => LocalIpcResponse::ok(
                    req.id,
                    serde_json::json!({ "tasks": self.file_transfer.list_tasks() }),
                ),
                other => LocalIpcResponse::err(
                    req.id,
                    "method_not_found",
                    format!("unknown daemon IPC method '{other}'"),
                ),
            }
        })
    }
}

fn default_transfer_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("FABUSHI_TRANSFER_STATE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    std::env::temp_dir().join(format!("fabushi-transfers-{}", std::process::id()))
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                if let Some(ref mut s) = sigterm {
                    s.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("fabushi-remote-daemon {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let mut socket_override: Option<PathBuf> = None;
    let mut mode = DaemonRunMode::Foreground;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" if i + 1 < args.len() => {
                socket_override = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--daemon" => {
                mode = DaemonRunMode::Daemon;
                i += 1;
            }
            "--agent" => {
                mode = DaemonRunMode::Agent;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    let endpoint = socket_override.unwrap_or_else(default_ipc_endpoint);

    if args.iter().any(|a| a == "--status") {
        let client = LocalIpcClient::new(endpoint);
        match client.call("status", serde_json::json!({})).await {
            Ok(resp) => {
                println!("{}", serde_json::to_string_pretty(&resp)?);
                return Ok(());
            }
            Err(err) => {
                eprintln!("daemon offline: {err}");
                std::process::exit(1);
            }
        }
    }

    let services = Arc::new(DaemonServices::new(mode, default_transfer_state_dir())?);
    let server = LocalIpcServer::bind(endpoint, services).await?;
    wait_for_shutdown_signal().await;
    server.shutdown();
    server.wait_stopped().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_telegram_proto::remote_control_contracts::LocalIpcEnvelope;

    #[tokio::test]
    async fn daemon_ipc_roundtrip_and_terminal_lifecycle() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let sock = tmp.path().join("daemon.sock");
        let state_dir = tmp.path().join("transfers");
        let services = Arc::new(DaemonServices::new(DaemonRunMode::Agent, state_dir).expect("services"));
        let server = LocalIpcServer::bind(sock.clone(), services)
            .await
            .expect("bind daemon ipc");
        let client = LocalIpcClient::new(sock);

        let st: LocalIpcEnvelope<serde_json::Value> =
            client.call_typed("status", serde_json::json!({})).await.expect("status");
        assert!(st.success);
        assert_eq!(st.data.as_ref().unwrap()["mode"], "agent");

        let created = client
            .call("lterm.new", serde_json::json!({ "name": "dterm" }))
            .await
            .expect("lterm.new");
        assert!(created.success);

        let has = client
            .call("lterm.has", serde_json::json!({ "name": "dterm" }))
            .await
            .expect("lterm.has");
        assert_eq!(has.data.as_ref().unwrap()["exists"], true);

        let exec_res = client
            .call(
                "lterm.exec",
                serde_json::json!({ "name": "dterm", "command": "echo daemon_ok", "timeout_ms": 5000 }),
            )
            .await
            .expect("lterm.exec");
        assert!(exec_res.success);
        assert_eq!(exec_res.data.as_ref().unwrap()["exit_code"], 0);
        assert!(exec_res.data.as_ref().unwrap()["output"]
            .as_str()
            .unwrap_or("")
            .contains("daemon_ok"));

        let killed = client
            .call("lterm.kill", serde_json::json!({ "name": "dterm" }))
            .await
            .expect("lterm.kill");
        assert_eq!(killed.data.as_ref().unwrap()["killed"], true);

        server.shutdown();
        server.wait_stopped().await;
    }
}
