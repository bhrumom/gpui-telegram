//! Real-time WebMCP Mini App broker: bridges AI tool calls to connected desktop WebViews
//! via WebSocket frames, enforces human-takeover pause/resume synchronization, and wakes
//! companion AI bots when the user finishes manual takeover.

use crate::server::AppState;
use crate::ws::gateway_hub::GatewayHub;
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_proto::miniapp_contracts::{
    MiniAppControlMode, MiniAppTakeoverEvent, MiniAppWebMcpCommand, MiniAppWebMcpResponse,
};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::{debug, warn};

pub const DEFAULT_WEBMCP_RPC_TIMEOUT: Duration = Duration::from_secs(18);
pub const DEFAULT_TAKEOVER_WAIT_TIMEOUT: Duration = Duration::from_secs(300);

type TakeoverKey = (i64, String, String);

struct PendingRpc {
    sender_user_id: i64,
    tx: oneshot::Sender<MiniAppWebMcpResponse>,
}

#[derive(Default)]
struct TakeoverState {
    mode: MiniAppControlMode,
    chat_id: i64,
    bot_user_id: i64,
    app_title: String,
    waiters: Vec<oneshot::Sender<MiniAppTakeoverEvent>>,
}

/// Coordinates real-time WebMCP RPCs and human takeover handoffs for Mini Apps.
pub struct MiniAppWebMcpBroker {
    gateway_hub: Weak<GatewayHub>,
    state_weak: Arc<OnceLock<Weak<AppState>>>,
    pending_rpcs: RwLock<HashMap<String, PendingRpc>>,
    takeovers: RwLock<HashMap<TakeoverKey, TakeoverState>>,
}

impl MiniAppWebMcpBroker {
    pub fn new(
        gateway_hub: Weak<GatewayHub>,
        state_weak: Arc<OnceLock<Weak<AppState>>>,
    ) -> Self {
        Self {
            gateway_hub,
            state_weak,
            pending_rpcs: RwLock::new(HashMap::new()),
            takeovers: RwLock::new(HashMap::new()),
        }
    }

    fn key(user_id: i64, bot_username: &str, short_name: &str) -> TakeoverKey {
        (
            user_id,
            bot_username.trim().trim_start_matches('@').to_ascii_lowercase(),
            short_name.trim().to_ascii_lowercase(),
        )
    }

    /// Returns whether the user currently has at least one connected WebSocket client.
    pub fn has_connected_client(&self, user_id: UserId) -> bool {
        self.gateway_hub
            .upgrade()
            .is_some_and(|hub| hub.has_connected_user(user_id))
    }

    /// Current control mode for `(user_id, bot_username, short_name)`.
    pub fn current_mode(
        &self,
        user_id: i64,
        bot_username: &str,
        short_name: &str,
    ) -> MiniAppControlMode {
        let key = Self::key(user_id, bot_username, short_name);
        self.takeovers
            .read()
            .get(&key)
            .map(|entry| entry.mode)
            .unwrap_or(MiniAppControlMode::Idle)
    }

    /// Broadcasts `UpdateEvent::MiniAppControlStatus` to all active sessions of `user_id`.
    #[allow(clippy::too_many_arguments)]
    pub fn broadcast_control_status(
        &self,
        user_id: UserId,
        bot_username: &str,
        short_name: &str,
        chat_id: ChatId,
        bot_user_id: UserId,
        app_title: &str,
        mode: MiniAppControlMode,
        status_text: &str,
        active_tool: Option<&str>,
    ) {
        let clean_bot = bot_username.trim().trim_start_matches('@').to_string();
        let clean_short = short_name.trim().to_string();
        let key = Self::key(user_id.0 as i64, &clean_bot, &clean_short);
        {
            let mut map = self.takeovers.write();
            let entry = map.entry(key).or_default();
            entry.mode = mode;
            if chat_id.0 > 0 {
                entry.chat_id = chat_id.0;
            }
            if bot_user_id.0 > 0 {
                entry.bot_user_id = bot_user_id.0 as i64;
            }
            if !app_title.trim().is_empty() {
                entry.app_title = app_title.trim().to_string();
            }
        }

        if let Some(hub) = self.gateway_hub.upgrade() {
            hub.broadcast_to_user(
                user_id,
                WsServerFrame::Update(UpdateEvent::MiniAppControlStatus {
                    bot_username: clean_bot,
                    short_name: clean_short,
                    chat_id,
                    bot_user_id,
                    app_title: app_title.to_string(),
                    mode,
                    status_text: status_text.to_string(),
                    active_tool: active_tool.map(|s| s.to_string()),
                }),
            );
        }
    }

    /// Transitions any `AiControlling` sessions for `(user_id, bot_user_id)` back to `Idle`
    /// once the bot turn completes (without overriding `UserTakeover`).
    pub fn mark_idle_for_bot(&self, user_id: UserId, bot_user_id: UserId) {
        let mut to_broadcast = Vec::new();
        {
            let mut map = self.takeovers.write();
            for ((uid, bot_username, short_name), entry) in map.iter_mut() {
                if *uid == user_id.0 as i64
                    && entry.bot_user_id == bot_user_id.0 as i64
                    && entry.mode == MiniAppControlMode::AiControlling
                {
                    entry.mode = MiniAppControlMode::Idle;
                    to_broadcast.push((
                        bot_username.clone(),
                        short_name.clone(),
                        entry.chat_id,
                        entry.app_title.clone(),
                    ));
                }
            }
        }
        if let Some(hub) = self.gateway_hub.upgrade() {
            for (bot_username, short_name, chat_id, app_title) in to_broadcast {
                hub.broadcast_to_user(
                    user_id,
                    WsServerFrame::Update(UpdateEvent::MiniAppControlStatus {
                        bot_username,
                        short_name,
                        chat_id: ChatId(chat_id),
                        bot_user_id,
                        app_title,
                        mode: MiniAppControlMode::Idle,
                        status_text: "AI 操作已完成".to_string(),
                        active_tool: None,
                    }),
                );
            }
        }
    }

    /// If the user is currently taking over or paused for takeover, waits until takeover completes.
    pub async fn wait_if_user_taking_over(
        &self,
        user_id: i64,
        bot_username: &str,
        short_name: &str,
        timeout: Duration,
    ) -> Result<Option<MiniAppTakeoverEvent>, String> {
        let key = Self::key(user_id, bot_username, short_name);
        let rx_opt = {
            let mut map = self.takeovers.write();
            if let Some(entry) = map.get_mut(&key) {
                if matches!(
                    entry.mode,
                    MiniAppControlMode::UserTakeover | MiniAppControlMode::WaitingForTakeover
                ) {
                    let (tx, rx) = oneshot::channel();
                    entry.waiters.push(tx);
                    Some(rx)
                } else {
                    None
                }
            } else {
                None
            }
        };

        let Some(rx) = rx_opt else {
            return Ok(None);
        };

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(evt)) => Ok(Some(evt)),
            Ok(Err(_)) => Err("Takeover waiter cancelled before completion".to_string()),
            Err(_) => Err(
                "User is still in manual Mini App takeover; paused AI execution until user clicks Finish Takeover."
                    .to_string(),
            ),
        }
    }

    /// Dispatches a WebMCP command to the connected desktop client WebView.
    /// Returns `Ok(None)` when the user has no connected WebSocket session so callers can
    /// run a headless server fallback.
    pub async fn dispatch_to_client(
        &self,
        user_id: UserId,
        mut cmd: MiniAppWebMcpCommand,
        timeout: Duration,
    ) -> Result<Option<MiniAppWebMcpResponse>, String> {
        let _ = self
            .wait_if_user_taking_over(
                user_id.0 as i64,
                &cmd.bot_username,
                &cmd.short_name,
                timeout.max(Duration::from_secs(60)),
            )
            .await?;

        let Some(hub) = self.gateway_hub.upgrade() else {
            return Ok(None);
        };
        if !hub.has_connected_user(user_id) {
            return Ok(None);
        }

        if cmd.request_id.trim().is_empty() {
            cmd.request_id = uuid::Uuid::new_v4().to_string();
        }
        cmd.bot_username = cmd.bot_username.trim().trim_start_matches('@').to_string();
        cmd.short_name = cmd.short_name.trim().to_string();

        let status_text = cmd
            .status_message
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| match cmd.operation.as_str() {
                "discover" => "正在读取小程序 WebMCP 工具清单...".to_string(),
                "get_state" => "正在读取小程序当前界面状态...".to_string(),
                "call_tool" => format!(
                    "AI 正在操控小程序: {}",
                    cmd.tool_name.as_deref().unwrap_or("tool")
                ),
                _ => "AI 正在与小程序交互...".to_string(),
            });

        self.broadcast_control_status(
            user_id,
            &cmd.bot_username,
            &cmd.short_name,
            ChatId(cmd.chat_id),
            UserId(cmd.bot_user_id.max(0) as u64),
            &cmd.app_title,
            MiniAppControlMode::AiControlling,
            &status_text,
            cmd.tool_name.as_deref(),
        );

        let request_id = cmd.request_id.clone();
        let (tx, rx) = oneshot::channel();
        self.pending_rpcs.write().insert(
            request_id.clone(),
            PendingRpc {
                sender_user_id: user_id.0 as i64,
                tx,
            },
        );

        hub.broadcast_to_user(
            user_id,
            WsServerFrame::Update(UpdateEvent::MiniAppWebMcpCommand(cmd)),
        );

        let wait_res = tokio::time::timeout(timeout, rx).await;
        self.pending_rpcs.write().remove(&request_id);

        match wait_res {
            Ok(Ok(resp)) => Ok(Some(resp)),
            Ok(Err(_)) => Err("WebMCP client RPC channel dropped".to_string()),
            Err(_) => Err(format!(
                "Timed out after {}s waiting for client WebMCP response ({request_id})",
                timeout.as_secs()
            )),
        }
    }

    /// Requests human takeover in the client UI and pauses AI until the user finishes takeover.
    #[allow(clippy::too_many_arguments)]
    pub async fn request_takeover_and_wait(
        &self,
        user_id: UserId,
        bot_username: &str,
        short_name: &str,
        chat_id: ChatId,
        bot_user_id: UserId,
        app_title: &str,
        reason: &str,
        timeout: Duration,
    ) -> Result<MiniAppTakeoverEvent, String> {
        let clean_bot = bot_username.trim().trim_start_matches('@').to_string();
        let clean_short = short_name.trim().to_string();
        let key = Self::key(user_id.0 as i64, &clean_bot, &clean_short);
        let rx = {
            let mut map = self.takeovers.write();
            let entry = map.entry(key).or_default();
            entry.mode = MiniAppControlMode::WaitingForTakeover;
            if chat_id.0 > 0 {
                entry.chat_id = chat_id.0;
            }
            if bot_user_id.0 > 0 {
                entry.bot_user_id = bot_user_id.0 as i64;
            }
            if !app_title.trim().is_empty() {
                entry.app_title = app_title.trim().to_string();
            }
            let (tx, rx) = oneshot::channel();
            entry.waiters.push(tx);
            rx
        };

        let Some(hub) = self.gateway_hub.upgrade() else {
            return Err("GatewayHub unavailable".to_string());
        };
        if !hub.has_connected_user(user_id) {
            return Err(
                "No connected client session is available for interactive human takeover."
                    .to_string(),
            );
        }

        let cmd = MiniAppWebMcpCommand {
            request_id: uuid::Uuid::new_v4().to_string(),
            bot_username: clean_bot.clone(),
            short_name: clean_short.clone(),
            chat_id: chat_id.0,
            bot_user_id: bot_user_id.0 as i64,
            app_title: app_title.to_string(),
            operation: "request_takeover".to_string(),
            tool_name: None,
            arguments: serde_json::json!({ "reason": reason }),
            status_message: Some(reason.to_string()),
        };

        self.broadcast_control_status(
            user_id,
            &clean_bot,
            &clean_short,
            chat_id,
            bot_user_id,
            app_title,
            MiniAppControlMode::WaitingForTakeover,
            reason,
            None,
        );

        hub.broadcast_to_user(
            user_id,
            WsServerFrame::Update(UpdateEvent::MiniAppWebMcpCommand(cmd)),
        );

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(evt)) => Ok(evt),
            Ok(Err(_)) => Err("Takeover waiter dropped".to_string()),
            Err(_) => {
                // Revert waiting state to idle on timeout so future calls are not permanently blocked.
                self.broadcast_control_status(
                    user_id,
                    &clean_bot,
                    &clean_short,
                    chat_id,
                    bot_user_id,
                    app_title,
                    MiniAppControlMode::Idle,
                    "接管等待超时",
                    None,
                );
                Err(format!(
                    "Timed out after {}s waiting for user takeover completion",
                    timeout.as_secs()
                ))
            }
        }
    }

    /// Resolves an in-flight WebMCP RPC when a client replies via `WsClientFrame::MiniAppWebMcpResult`.
    pub fn resolve_rpc_response(&self, user_id: UserId, resp: MiniAppWebMcpResponse) -> bool {
        let pending_opt = {
            let mut map = self.pending_rpcs.write();
            if let Some(entry) = map.get(&resp.request_id) {
                if entry.sender_user_id != user_id.0 as i64 {
                    warn!(
                        request_id = %resp.request_id,
                        expected_user = entry.sender_user_id,
                        actual_user = user_id.0,
                        "Ignoring WebMCP response from mismatched user"
                    );
                    return false;
                }
            }
            map.remove(&resp.request_id)
        };

        if let Some(pending) = pending_opt {
            let _ = pending.tx.send(resp);
            true
        } else {
            debug!(
                request_id = %resp.request_id,
                user_id = user_id.0,
                "Received WebMCP response for unknown or expired request"
            );
            false
        }
    }

    /// Applies `WsClientFrame::MiniAppTakeoverState`, wakes any paused AI waiter, and if no
    /// waiter is active when the user finishes takeover (`Idle`/`AiControlling`), triggers AI
    /// resumption automatically.
    pub fn handle_takeover_event(&self, user_id: UserId, mut evt: MiniAppTakeoverEvent) {
        evt.bot_username = evt.bot_username.trim().trim_start_matches('@').to_string();
        evt.short_name = evt.short_name.trim().to_string();
        let key = Self::key(user_id.0 as i64, &evt.bot_username, &evt.short_name);

        let (drained_waiters, resolved_chat_id, resolved_bot_user_id, app_title) = {
            let mut map = self.takeovers.write();
            let entry = map.entry(key).or_default();
            entry.mode = evt.mode;
            if evt.chat_id > 0 {
                entry.chat_id = evt.chat_id;
            }
            if evt.bot_user_id > 0 {
                entry.bot_user_id = evt.bot_user_id;
            }
            let finished = matches!(
                evt.mode,
                MiniAppControlMode::Idle | MiniAppControlMode::AiControlling
            );
            let waiters = if finished {
                std::mem::take(&mut entry.waiters)
            } else {
                Vec::new()
            };
            (
                waiters,
                if evt.chat_id > 0 { evt.chat_id } else { entry.chat_id },
                if evt.bot_user_id > 0 {
                    evt.bot_user_id
                } else {
                    entry.bot_user_id
                },
                entry.app_title.clone(),
            )
        };

        if evt.chat_id <= 0 {
            evt.chat_id = resolved_chat_id;
        }
        if evt.bot_user_id <= 0 {
            evt.bot_user_id = resolved_bot_user_id;
        }

        let status_text = match evt.mode {
            MiniAppControlMode::UserTakeover => "用户正在手动接管小程序",
            MiniAppControlMode::WaitingForTakeover => "等待用户接管小程序",
            MiniAppControlMode::AiControlling => "AI 已重新接管并继续执行",
            MiniAppControlMode::Idle => "用户已结束接管，准备继续",
        };
        self.broadcast_control_status(
            user_id,
            &evt.bot_username,
            &evt.short_name,
            ChatId(resolved_chat_id),
            UserId(resolved_bot_user_id.max(0) as u64),
            &app_title,
            evt.mode,
            status_text,
            None,
        );

        let had_waiters = !drained_waiters.is_empty();
        for tx in drained_waiters {
            let _ = tx.send(evt.clone());
        }

        // If the user finished manual takeover outside a blocking waiter, wake the bot turn
        // with the latest state snapshot so the AI seamlessly continues where it left off.
        if !had_waiters
            && matches!(
                evt.mode,
                MiniAppControlMode::Idle | MiniAppControlMode::AiControlling
            )
            && resolved_chat_id > 0
            && resolved_bot_user_id > 0
        {
            if let Some(state) = self.state_weak.get().and_then(|w| w.upgrade()) {
                let bot_uid = UserId(resolved_bot_user_id as u64);
                let chat_id = ChatId(resolved_chat_id);
                let prompt = build_takeover_resume_prompt(&evt);
                tokio::spawn(async move {
                    crate::routes::bots_miniapps::handle_ai_bot_reply(
                        &state,
                        chat_id,
                        user_id,
                        bot_uid,
                        &prompt,
                    )
                    .await;
                });
            }
        }
    }
}

fn build_takeover_resume_prompt(evt: &MiniAppTakeoverEvent) -> String {
    let mut prompt = format!(
        "[系统通知] 用户已结束对小程序 `{}/{}` 的手动接管，请结合当前最新状态继续执行之前的任务，并使用简短中文向用户确认进展。",
        evt.bot_username, evt.short_name
    );
    if let Some(note) = evt
        .user_note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        prompt.push_str(&format!("\n用户补充说明: {note}"));
    }
    if let Some(snapshot) = evt.state_snapshot.as_ref() {
        if let Ok(json_str) = serde_json::to_string_pretty(snapshot) {
            let clipped: String = json_str.chars().take(1500).collect();
            prompt.push_str(&format!("\n当前小程序最新状态快照:\n```json\n{clipped}\n```"));
        }
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_takeover_pauses_and_resumes_waiter_on_finish() {
        let broker = Arc::new(MiniAppWebMcpBroker::new(
            Weak::new(),
            Arc::new(OnceLock::new()),
        ));
        let uid = UserId(42);
        broker.handle_takeover_event(
            uid,
            MiniAppTakeoverEvent {
                bot_username: "fabushi_bot".to_string(),
                short_name: "fabushi".to_string(),
                chat_id: 100,
                bot_user_id: 777001,
                mode: MiniAppControlMode::UserTakeover,
                user_note: Some("手动登录中".to_string()),
                state_snapshot: None,
            },
        );
        assert_eq!(
            broker.current_mode(42, "fabushi_bot", "fabushi"),
            MiniAppControlMode::UserTakeover
        );

        let broker_bg = Arc::clone(&broker);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            broker_bg.handle_takeover_event(
                uid,
                MiniAppTakeoverEvent {
                    bot_username: "@fabushi_bot".to_string(),
                    short_name: "fabushi".to_string(),
                    chat_id: 100,
                    bot_user_id: 777001,
                    mode: MiniAppControlMode::Idle,
                    user_note: Some("已选好经文".to_string()),
                    state_snapshot: Some(serde_json::json!({ "sutra": "金刚经" })),
                },
            );
        });

        let resumed = broker
            .wait_if_user_taking_over(42, "fabushi_bot", "fabushi", Duration::from_secs(2))
            .await
            .expect("should resume without timeout")
            .expect("should receive takeover completion event");
        assert_eq!(resumed.mode, MiniAppControlMode::Idle);
        assert_eq!(resumed.user_note.as_deref(), Some("已选好经文"));
        assert_eq!(
            broker.current_mode(42, "fabushi_bot", "fabushi"),
            MiniAppControlMode::Idle
        );
    }

    #[tokio::test]
    async fn test_resolve_rpc_response_rejects_wrong_user() {
        let broker = MiniAppWebMcpBroker::new(Weak::new(), Arc::new(OnceLock::new()));
        let (tx, rx) = oneshot::channel();
        broker.pending_rpcs.write().insert(
            "req-1".to_string(),
            PendingRpc {
                sender_user_id: 10,
                tx,
            },
        );
        let rejected = broker.resolve_rpc_response(
            UserId(99),
            MiniAppWebMcpResponse {
                request_id: "req-1".to_string(),
                ok: true,
                result: serde_json::json!({}),
                error: None,
                tools: vec![],
                state_summary: None,
                control_mode: MiniAppControlMode::AiControlling,
            },
        );
        assert!(!rejected);

        let accepted = broker.resolve_rpc_response(
            UserId(10),
            MiniAppWebMcpResponse {
                request_id: "req-1".to_string(),
                ok: true,
                result: serde_json::json!({ "done": true }),
                error: None,
                tools: vec![],
                state_summary: None,
                control_mode: MiniAppControlMode::AiControlling,
            },
        );
        assert!(accepted);
        let resp = rx.await.expect("receiver should receive response");
        assert_eq!(resp.result["done"], true);
    }
}
