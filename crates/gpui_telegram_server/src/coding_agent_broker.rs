//! Real-time Coding Agent Broker: routes coding turns, interactive approvals,
//! slash-command controls, and host probes to a single targeted local desktop host
//! over WebSocket frames, and streams markdown updates back into Telegram chats.

use crate::db::miniapp_store::BotRegistryRecord;
use crate::routes::bots_miniapps::send_bot_chat_reply_in_topic;
use crate::routes::broadcast_chat_pts;
use crate::routes::hermes::BotTypingGuard;
use crate::server::AppState;
use crate::ws::gateway_hub::GatewayHub;
use gpui_telegram_core::types::{ChatId, MsgId, TopicId, UserId};
use gpui_telegram_proto::coding_agent_contracts::{
    CodingAgentBotConfig, CodingApprovalDecision, CodingControlAction, CodingDiffStat,
    CodingHostCommand, CodingHostEvent, CodingTokenUsage, DetectedCodingProvider,
};
use gpui_telegram_proto::message::{InlineKeyboardButton, MessageContent};
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tracing::{debug, warn};
use uuid::Uuid;

pub const DEFAULT_CODING_CONTROL_TIMEOUT: Duration = Duration::from_secs(20);
pub const DEFAULT_CODING_PROBE_TIMEOUT: Duration = Duration::from_secs(6);
pub const CODING_STREAM_EDIT_THROTTLE: Duration = Duration::from_millis(120);
const MAX_RECENT_TURN_META: usize = 512;
const MAX_STREAM_STEPS_SHOWN: usize = 12;

#[derive(Debug, Clone, Serialize)]
pub struct RegisteredCodingHost {
    pub owner_user_id: u64,
    pub device_id: String,
    pub display_name: String,
    pub os_name: String,
    pub providers: Vec<DetectedCodingProvider>,
    #[serde(skip_serializing)]
    pub session_id: Option<String>,
    pub last_seen_ms: i64,
    pub online: bool,
}

#[derive(Debug, Clone)]
struct CodingToolStepState {
    call_id: String,
    tool_name: String,
    title: String,
    status: String,
    output_preview: Option<String>,
}

struct ActiveTurnInner {
    turn_id: String,
    owner_user_id: u64,
    bot_user_id: i64,
    chat_id: i64,
    topic_id: Option<i64>,
    allow_group_members: bool,
    target_device_id: Option<String>,
    target_session_id: Option<String>,
    provider_session_id: Option<String>,
    workspace_path: String,
    worktree_path: Option<String>,
    git_branch: Option<String>,
    turn_idx: u64,
    thinking_buf: String,
    tool_steps: Vec<CodingToolStepState>,
    text_buf: String,
    bot_msg_id: Option<MsgId>,
    last_rendered: String,
    last_kb_json: Option<String>,
    last_edit_at: Option<Instant>,
    dirty: bool,
    flush_scheduled: bool,
    finished: bool,
    typing_guard: Option<BotTypingGuard>,
}

struct TurnSession {
    inner: Mutex<ActiveTurnInner>,
}

#[derive(Debug, Clone)]
pub struct TurnLookupMeta {
    pub owner_user_id: u64,
    pub bot_user_id: i64,
    pub chat_id: i64,
    pub topic_id: Option<i64>,
    pub allow_group_members: bool,
    pub target_device_id: Option<String>,
    pub target_session_id: Option<String>,
}

#[derive(Debug, Clone)]
struct PendingApprovalInfo {
    owner_user_id: u64,
    bot_user_id: i64,
    chat_id: i64,
    topic_id: Option<i64>,
    allow_group_members: bool,
    target_device_id: Option<String>,
    target_session_id: Option<String>,
    kind: String,
    title: String,
    approval_msg_id: Option<MsgId>,
    resolved: bool,
}

#[derive(Debug, Clone)]
pub struct ApprovalResolveOutcome {
    pub already_resolved: bool,
    pub chat_id: i64,
    pub bot_user_id: i64,
    pub topic_id: Option<i64>,
    pub approval_msg_id: Option<MsgId>,
    pub kind: String,
    pub title: String,
}

struct PendingProbeRpc {
    owner_user_id: u64,
    tx: oneshot::Sender<CodingHostEvent>,
}

struct PendingControlRpc {
    owner_user_id: u64,
    tx: oneshot::Sender<CodingHostEvent>,
}

/// Edits an existing bot message in place, persisting optional inline keyboard changes
/// and propagating `InlineKeyboard` on the broadcast `EditMessage` PTS update.
pub fn edit_bot_chat_message(
    state: &Arc<AppState>,
    chat_id: ChatId,
    msg_id: MsgId,
    text: &str,
    kb_rows: Option<Vec<Vec<InlineKeyboardButton>>>,
) -> gpui_telegram_core::error::Result<()> {
    if let Some(ref rows) = kb_rows {
        let kb_json = serde_json::to_string(rows).unwrap_or_else(|_| "[]".to_string());
        let _ = state
            .db
            .save_message_extras(chat_id, msg_id, Some(&kb_json), None);
    }
    let mut pts_update = state.message_pipeline.edit_message(
        chat_id,
        msg_id,
        MessageContent::plain_text(text.trim()),
    )?;
    if let Some(rows) = kb_rows {
        if let UpdateEvent::EditMessage(ref mut item) = pts_update.event {
            item.inline_keyboard = Some(rows);
        }
    }
    broadcast_chat_pts(state, chat_id, pts_update);
    Ok(())
}

/// Coordinates single-device local Coding Agent Host execution and chat streaming.
pub struct CodingAgentBroker {
    gateway_hub: Weak<GatewayHub>,
    state_weak: Arc<OnceLock<Weak<AppState>>>,
    hosts: RwLock<HashMap<(u64, String), RegisteredCodingHost>>,
    active_turns: RwLock<HashMap<String, Arc<TurnSession>>>,
    chat_active_turns: RwLock<HashMap<(i64, i64, i64), String>>,
    turn_meta: RwLock<HashMap<String, TurnLookupMeta>>,
    pending_approvals: RwLock<HashMap<(String, String), PendingApprovalInfo>>,
    pending_probes: RwLock<HashMap<String, PendingProbeRpc>>,
    pending_controls: RwLock<HashMap<String, PendingControlRpc>>,
}

impl CodingAgentBroker {
    pub fn new(
        gateway_hub: Weak<GatewayHub>,
        state_weak: Arc<OnceLock<Weak<AppState>>>,
    ) -> Self {
        Self {
            gateway_hub,
            state_weak,
            hosts: RwLock::new(HashMap::new()),
            active_turns: RwLock::new(HashMap::new()),
            chat_active_turns: RwLock::new(HashMap::new()),
            turn_meta: RwLock::new(HashMap::new()),
            pending_approvals: RwLock::new(HashMap::new()),
            pending_probes: RwLock::new(HashMap::new()),
            pending_controls: RwLock::new(HashMap::new()),
        }
    }

    fn thread_key(bot_user_id: i64, chat_id: i64, topic_id: Option<i64>) -> (i64, i64, i64) {
        (bot_user_id, chat_id, topic_id.unwrap_or(0))
    }

    /// Marks hosts bound to `session_id` as offline when a WebSocket connection closes.
    pub fn mark_session_disconnected(&self, user_id: UserId, session_id: &str) {
        let mut hosts = self.hosts.write();
        for ((uid, _), host) in hosts.iter_mut() {
            if *uid == user_id.0 && host.session_id.as_deref() == Some(session_id) {
                host.online = false;
                host.session_id = None;
            }
        }
    }

    /// Lists registered hosts for `owner_user_id` with live WS session liveness reflected.
    pub fn hosts_for_user(&self, owner_user_id: u64) -> Vec<RegisteredCodingHost> {
        let hub_opt = self.gateway_hub.upgrade();
        let uid = UserId(owner_user_id);
        let mut out: Vec<RegisteredCodingHost> = self
            .hosts
            .read()
            .iter()
            .filter(|((u, _), _)| *u == owner_user_id)
            .map(|(_, h)| {
                let mut copy = h.clone();
                copy.online = Self::is_host_online(hub_opt.as_deref(), uid, h);
                copy
            })
            .collect();
        out.sort_by(|a, b| b.last_seen_ms.cmp(&a.last_seen_ms));
        out
    }

    fn is_host_online(
        hub_opt: Option<&GatewayHub>,
        owner_uid: UserId,
        host: &RegisteredCodingHost,
    ) -> bool {
        if !host.online {
            return false;
        }
        let Some(hub) = hub_opt else {
            return false;
        };
        if let Some(ref sid) = host.session_id {
            hub.has_user_session(owner_uid, sid)
        } else {
            hub.has_connected_user(owner_uid)
        }
    }

    fn record_turn_meta(&self, turn_id: &str, meta: TurnLookupMeta) {
        let mut map = self.turn_meta.write();
        if map.len() >= MAX_RECENT_TURN_META {
            let active = self.active_turns.read();
            map.retain(|tid, _| active.contains_key(tid));
            if map.len() >= MAX_RECENT_TURN_META {
                if let Some(oldest_key) = map
                    .keys()
                    .find(|tid| !active.contains_key(*tid))
                    .cloned()
                {
                    map.remove(&oldest_key);
                }
            }
        }
        map.insert(turn_id.to_string(), meta);
    }

    pub fn get_turn_meta(&self, turn_id: &str) -> Option<TurnLookupMeta> {
        if let Some(session) = self.active_turns.read().get(turn_id) {
            let inner = session.inner.lock();
            return Some(TurnLookupMeta {
                owner_user_id: inner.owner_user_id,
                bot_user_id: inner.bot_user_id,
                chat_id: inner.chat_id,
                topic_id: inner.topic_id,
                allow_group_members: inner.allow_group_members,
                target_device_id: inner.target_device_id.clone(),
                target_session_id: inner.target_session_id.clone(),
            });
        }
        self.turn_meta.read().get(turn_id).cloned()
    }

    /// Verifies whether `caller_user_id` is authorized to control/approve `turn_id`.
    /// Enforces Owner-Only unless live bot config enables `allow_group_members` and caller
    /// is an active member of the turn's chat.
    pub fn is_turn_caller_authorized(&self, turn_id: &str, caller_user_id: UserId) -> bool {
        let Some(meta) = self.get_turn_meta(turn_id) else {
            return false;
        };
        self.is_meta_caller_authorized(&meta, caller_user_id)
    }

    fn is_meta_caller_authorized(&self, meta: &TurnLookupMeta, caller_user_id: UserId) -> bool {
        if caller_user_id.0 == meta.owner_user_id {
            return true;
        }
        let Some(state) = self.state_weak.get().and_then(|w| w.upgrade()) else {
            return meta.allow_group_members;
        };
        let Ok(Some(bot)) = state.db.get_bot_by_user_id(meta.bot_user_id) else {
            return false;
        };
        if caller_user_id.0 as i64 == bot.owner_user_id {
            return true;
        }
        let Some(cfg) = bot.coding_agent_config() else {
            return false;
        };
        if !cfg.allow_group_members {
            return false;
        }
        crate::routes::check_active_membership(&state, ChatId(meta.chat_id), caller_user_id).is_ok()
    }

    /// Selects a SINGLE target WS connection for `owner_user_id` (never broadcasts to all!).
    /// Priority:
    /// 1) `bound_device_id` if specified -> strict single-session targeting; fails if offline.
    /// 2) Owner's most recently active registered coding host session -> single connection.
    /// 3) Single active WS connection of `owner_user_id` if no host registered yet.
    fn resolve_single_target(
        &self,
        owner_user_id: UserId,
        bound_device_id: Option<&str>,
    ) -> Result<(Option<String>, Option<String>, bool), String> {
        let Some(hub) = self.gateway_hub.upgrade() else {
            return Err("WebSocket 网关服务不可用。".to_string());
        };
        let clean_bound = bound_device_id
            .map(str::trim)
            .filter(|s| !s.is_empty());

        if let Some(dev) = clean_bound {
            let host_opt = self
                .hosts
                .read()
                .get(&(owner_user_id.0, dev.to_string()))
                .cloned();
            if let Some(host) = host_opt {
                if host.online {
                    if let Some(ref sid) = host.session_id {
                        if hub.has_user_session(owner_user_id, sid) {
                            return Ok((Some(dev.to_string()), Some(sid.clone()), false));
                        }
                    } else if hub.user_connection_count(owner_user_id) == 1 {
                        return Ok((Some(dev.to_string()), None, true));
                    }
                }
            }
            // Also support tests binding `bound_device_id` directly to an active WS session_id.
            if hub.has_user_session(owner_user_id, dev) {
                return Ok((Some(dev.to_string()), Some(dev.to_string()), false));
            }
            return Err(format!(
                "⚠️ 目标本地主机设备 `{dev}` 当前离线或未连接。请在该设备上打开并登录桌面客户端后重试。"
            ));
        }

        // Unpinned bot: pick the freshest online registered coding host for this owner.
        let hosts = self.hosts_for_user(owner_user_id.0);
        if let Some(online_host) = hosts.iter().find(|h| h.online) {
            return Ok((
                Some(online_host.device_id.clone()),
                online_host.session_id.clone(),
                true,
            ));
        }

        if hub.has_connected_user(owner_user_id) {
            return Ok((None, None, true));
        }

        if let Some(known) = hosts.first() {
            return Err(format!(
                "⚠️ 本地主机设备 `{}` ({}) 当前离线。请先打开并登录桌面客户端后再试。",
                known.device_id, known.display_name
            ));
        }
        Err("⚠️ 当前没有在线的本地主机桌面客户端。请先打开并登录桌面客户端后再试。".to_string())
    }

    fn send_frame_to_resolved_target(
        &self,
        owner_user_id: UserId,
        target_device_id: Option<&str>,
        target_session_id: Option<&str>,
        allow_single_fallback: bool,
        frame: WsServerFrame,
    ) -> Result<Option<String>, String> {
        let Some(hub) = self.gateway_hub.upgrade() else {
            return Err("WebSocket 网关服务不可用。".to_string());
        };
        if let Some(sid) = target_session_id {
            if hub.send_to_user_session(owner_user_id, sid, frame.clone()) {
                return Ok(Some(sid.to_string()));
            }
            if !allow_single_fallback {
                let dev_label = target_device_id.unwrap_or(sid);
                return Err(format!(
                    "⚠️ 目标本地主机设备 `{dev_label}` 当前离线或连接已断开。"
                ));
            }
        }
        if allow_single_fallback {
            if let Some(actual_sid) =
                hub.send_to_single_user_connection(owner_user_id, target_session_id, frame)
            {
                return Ok(actual_sid);
            }
        }
        if let Some(dev) = target_device_id {
            Err(format!("⚠️ 目标本地主机设备 `{dev}` 当前离线或未连接。"))
        } else {
            Err("⚠️ 当前没有在线的本地主机桌面客户端。请先打开并登录桌面客户端后再试。".to_string())
        }
    }

    /// Probes local coding CLI providers on a single target desktop host.
    pub async fn probe_host(
        &self,
        user_id: UserId,
        target_device_id: Option<&str>,
        timeout: Duration,
    ) -> Result<(String, Vec<DetectedCodingProvider>), String> {
        let (resolved_dev, resolved_sid, allow_fallback) =
            self.resolve_single_target(user_id, target_device_id)?;
        let req_id = format!("cprobe_{}", Uuid::new_v4().simple());
        let (tx, rx) = oneshot::channel();
        self.pending_probes.write().insert(
            req_id.clone(),
            PendingProbeRpc {
                owner_user_id: user_id.0,
                tx,
            },
        );

        let cmd = CodingHostCommand::ProbeProviders {
            req_id: req_id.clone(),
            owner_user_id: user_id.0,
            target_device_id: resolved_dev.clone(),
        };
        if let Err(err) = self.send_frame_to_resolved_target(
            user_id,
            resolved_dev.as_deref(),
            resolved_sid.as_deref(),
            allow_fallback,
            WsServerFrame::Update(UpdateEvent::CodingHostCommand(cmd)),
        ) {
            self.pending_probes.write().remove(&req_id);
            return Err(err);
        }

        let wait_res = tokio::time::timeout(timeout, rx).await;
        self.pending_probes.write().remove(&req_id);

        match wait_res {
            Ok(Ok(CodingHostEvent::ProbeResult {
                device_id,
                providers,
                ..
            })) => Ok((device_id, providers)),
            Ok(Ok(_)) => Err("主机返回了非预期的 Probe 响应".to_string()),
            Ok(Err(_)) => Err("主机 Probe 通道已关闭".to_string()),
            Err(_) => Err(format!("等待本地主机 Probe 响应超时 ({}s)", timeout.as_secs())),
        }
    }

    /// Dispatches a coding turn to a single targeted host and returns the allocated `turn_id`.
    #[allow(clippy::too_many_arguments)]
    pub async fn dispatch_turn(
        &self,
        reg_bot: &BotRegistryRecord,
        config: &CodingAgentBotConfig,
        chat_id: ChatId,
        topic_id: Option<i64>,
        is_forum_topic: bool,
        prompt: &str,
    ) -> Result<String, String> {
        let owner_uid = UserId(reg_bot.owner_user_id.max(0) as u64);
        let (resolved_dev, resolved_sid, allow_fallback) =
            self.resolve_single_target(owner_uid, config.bound_device_id.as_deref())?;

        let turn_id = format!("cturn_{}", Uuid::new_v4().simple());
        let typing_guard = self.state_weak.get().and_then(|w| w.upgrade()).map(|st| {
            let guard = BotTypingGuard::start(
                Arc::clone(&st),
                chat_id,
                UserId(reg_bot.bot_user_id as u64),
            );
            BotTypingGuard::emit_activity(
                &st,
                chat_id,
                UserId(reg_bot.bot_user_id as u64),
                "thinking",
                Some("正在派发任务至本地编程主机...".to_string()),
            );
            guard
        });

        let session = Arc::new(TurnSession {
            inner: Mutex::new(ActiveTurnInner {
                turn_id: turn_id.clone(),
                owner_user_id: owner_uid.0,
                bot_user_id: reg_bot.bot_user_id,
                chat_id: chat_id.0,
                topic_id,
                allow_group_members: config.allow_group_members,
                target_device_id: resolved_dev.clone(),
                target_session_id: resolved_sid.clone(),
                provider_session_id: None,
                workspace_path: config.workspace_path.clone(),
                worktree_path: None,
                git_branch: None,
                turn_idx: 0,
                thinking_buf: String::new(),
                tool_steps: Vec::new(),
                text_buf: String::new(),
                bot_msg_id: None,
                last_rendered: String::new(),
                last_kb_json: None,
                last_edit_at: None,
                dirty: false,
                flush_scheduled: false,
                finished: false,
                typing_guard,
            }),
        });

        let meta = TurnLookupMeta {
            owner_user_id: owner_uid.0,
            bot_user_id: reg_bot.bot_user_id,
            chat_id: chat_id.0,
            topic_id,
            allow_group_members: config.allow_group_members,
            target_device_id: resolved_dev.clone(),
            target_session_id: resolved_sid.clone(),
        };

        // Register turn BEFORE sending the WS frame so fast host responses never race ahead.
        self.active_turns.write().insert(turn_id.clone(), session.clone());
        self.chat_active_turns.write().insert(
            Self::thread_key(reg_bot.bot_user_id, chat_id.0, topic_id),
            turn_id.clone(),
        );
        self.record_turn_meta(&turn_id, meta.clone());

        let cmd = CodingHostCommand::StartTurn {
            turn_id: turn_id.clone(),
            owner_user_id: owner_uid.0,
            target_device_id: resolved_dev.clone(),
            bot_user_id: reg_bot.bot_user_id,
            chat_id: chat_id.0,
            topic_id,
            is_forum_topic,
            prompt: prompt.to_string(),
            config: config.clone(),
        };

        match self.send_frame_to_resolved_target(
            owner_uid,
            resolved_dev.as_deref(),
            resolved_sid.as_deref(),
            allow_fallback,
            WsServerFrame::Update(UpdateEvent::CodingHostCommand(cmd)),
        ) {
            Ok(actual_sid) => {
                if actual_sid != resolved_sid {
                    session.inner.lock().target_session_id = actual_sid.clone();
                    let mut updated_meta = meta;
                    updated_meta.target_session_id = actual_sid;
                    self.record_turn_meta(&turn_id, updated_meta);
                }
                Ok(turn_id)
            }
            Err(err) => {
                let guard = session.inner.lock().typing_guard.take();
                if let Some(g) = guard {
                    g.finish();
                }
                self.active_turns.write().remove(&turn_id);
                self.chat_active_turns
                    .write()
                    .remove(&Self::thread_key(reg_bot.bot_user_id, chat_id.0, topic_id));
                Err(err)
            }
        }
    }

    /// Dispatches a slash/button `CodingControlAction` to a single targeted host and awaits `ControlResult`.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_control(
        &self,
        reg_bot: &BotRegistryRecord,
        config: &CodingAgentBotConfig,
        chat_id: ChatId,
        topic_id: Option<i64>,
        is_forum_topic: bool,
        action: CodingControlAction,
        timeout: Duration,
    ) -> Result<CodingHostEvent, String> {
        let owner_uid = UserId(reg_bot.owner_user_id.max(0) as u64);
        let (resolved_dev, resolved_sid, allow_fallback) =
            self.resolve_single_target(owner_uid, config.bound_device_id.as_deref())?;

        let req_id = format!("cctrl_{}", Uuid::new_v4().simple());
        let (tx, rx) = oneshot::channel();
        self.pending_controls.write().insert(
            req_id.clone(),
            PendingControlRpc {
                owner_user_id: owner_uid.0,
                tx,
            },
        );

        let cmd = CodingHostCommand::Control {
            req_id: req_id.clone(),
            owner_user_id: owner_uid.0,
            target_device_id: resolved_dev.clone(),
            bot_user_id: reg_bot.bot_user_id,
            chat_id: chat_id.0,
            topic_id,
            is_forum_topic,
            config: config.clone(),
            action,
        };

        if let Err(err) = self.send_frame_to_resolved_target(
            owner_uid,
            resolved_dev.as_deref(),
            resolved_sid.as_deref(),
            allow_fallback,
            WsServerFrame::Update(UpdateEvent::CodingHostCommand(cmd)),
        ) {
            self.pending_controls.write().remove(&req_id);
            return Err(err);
        }

        let wait_res = tokio::time::timeout(timeout, rx).await;
        self.pending_controls.write().remove(&req_id);

        match wait_res {
            Ok(Ok(ev @ CodingHostEvent::ControlResult { .. })) => Ok(ev),
            Ok(Ok(_)) => Err("主机返回了非预期的 Control 响应".to_string()),
            Ok(Err(_)) => Err("主机 Control 通道已关闭".to_string()),
            Err(_) => Err(format!("等待本地主机执行命令超时 ({}s)", timeout.as_secs())),
        }
    }

    /// Resolves an interactive tool/command approval request from chat callback buttons.
    /// Guarantees single-execution semantics against duplicate button clicks.
    pub fn resolve_approval(
        &self,
        caller_user_id: UserId,
        turn_id: &str,
        request_id: &str,
        decision: CodingApprovalDecision,
    ) -> Result<ApprovalResolveOutcome, String> {
        if let Some(meta) = self.get_turn_meta(turn_id) {
            if !self.is_meta_caller_authorized(&meta, caller_user_id) {
                return Err("FORBIDDEN".to_string());
            }
        }
        if !self.active_turns.read().contains_key(turn_id) {
            return Err("该编程回合已结束，审批请求已失效。".to_string());
        }
        let key = (turn_id.to_string(), request_id.to_string());
        let (already_resolved, meta, snapshot) = {
            let mut map = self.pending_approvals.write();
            let Some(entry) = map.get_mut(&key) else {
                return Err("未找到该审批请求（可能已过期或回合已结束）。".to_string());
            };
            let meta = TurnLookupMeta {
                owner_user_id: entry.owner_user_id,
                bot_user_id: entry.bot_user_id,
                chat_id: entry.chat_id,
                topic_id: entry.topic_id,
                allow_group_members: entry.allow_group_members,
                target_device_id: entry.target_device_id.clone(),
                target_session_id: entry.target_session_id.clone(),
            };
            if !self.is_meta_caller_authorized(&meta, caller_user_id) {
                return Err("FORBIDDEN".to_string());
            }
            if entry.resolved {
                return Ok(ApprovalResolveOutcome {
                    already_resolved: true,
                    chat_id: entry.chat_id,
                    bot_user_id: entry.bot_user_id,
                    topic_id: entry.topic_id,
                    approval_msg_id: entry.approval_msg_id,
                    kind: entry.kind.clone(),
                    title: entry.title.clone(),
                });
            }
            entry.resolved = true;
            (false, meta, entry.clone())
        };
        let _ = already_resolved;

        let owner_uid = UserId(meta.owner_user_id);
        let cmd = CodingHostCommand::ResolveApproval {
            turn_id: turn_id.to_string(),
            request_id: request_id.to_string(),
            owner_user_id: meta.owner_user_id,
            target_device_id: meta.target_device_id.clone(),
            decision,
        };
        if let Err(err) = self.send_frame_to_resolved_target(
            owner_uid,
            meta.target_device_id.as_deref(),
            meta.target_session_id.as_deref(),
            meta.target_session_id.is_none(),
            WsServerFrame::Update(UpdateEvent::CodingHostCommand(cmd)),
        ) {
            if let Some(entry) = self.pending_approvals.write().get_mut(&key) {
                entry.resolved = false;
            }
            return Err(err);
        }

        Ok(ApprovalResolveOutcome {
            already_resolved: false,
            chat_id: snapshot.chat_id,
            bot_user_id: snapshot.bot_user_id,
            topic_id: snapshot.topic_id,
            approval_msg_id: snapshot.approval_msg_id,
            kind: snapshot.kind,
            title: snapshot.title,
        })
    }

    /// Cancels an active turn by `turn_id` on its exact target host session.
    pub fn cancel_turn(&self, caller_user_id: UserId, turn_id: &str) -> Result<(), String> {
        let Some(meta) = self.get_turn_meta(turn_id) else {
            return Err("未找到正在进行的编程任务。".to_string());
        };
        if !self.is_meta_caller_authorized(&meta, caller_user_id) {
            return Err("FORBIDDEN".to_string());
        }
        let owner_uid = UserId(meta.owner_user_id);
        let cmd = CodingHostCommand::CancelTurn {
            turn_id: turn_id.to_string(),
            owner_user_id: meta.owner_user_id,
            target_device_id: meta.target_device_id.clone(),
        };
        self.send_frame_to_resolved_target(
            owner_uid,
            meta.target_device_id.as_deref(),
            meta.target_session_id.as_deref(),
            meta.target_session_id.is_none(),
            WsServerFrame::Update(UpdateEvent::CodingHostCommand(cmd)),
        )?;
        Ok(())
    }

    /// Cancels whichever active turn is currently running for `(bot_user_id, chat_id, topic_id)`.
    pub fn cancel_active_turn_for_thread(
        &self,
        caller_user_id: UserId,
        bot_user_id: i64,
        chat_id: i64,
        topic_id: Option<i64>,
    ) -> Result<Option<String>, String> {
        let key = Self::thread_key(bot_user_id, chat_id, topic_id);
        let tid_opt = self.chat_active_turns.read().get(&key).cloned();
        let Some(turn_id) = tid_opt else {
            return Ok(None);
        };
        if !self.active_turns.read().contains_key(&turn_id) {
            self.chat_active_turns.write().remove(&key);
            return Ok(None);
        }
        self.cancel_turn(caller_user_id, &turn_id)?;
        Ok(Some(turn_id))
    }

    /// Handles incoming `WsClientFrame::CodingHostEvent` sent by a desktop coding host.
    pub fn handle_host_event(
        &self,
        user_id: UserId,
        session_id: Option<&str>,
        ev: CodingHostEvent,
    ) {
        match ev {
            CodingHostEvent::HostRegister {
                device_id,
                display_name,
                os_name,
                providers,
            } => {
                let dev = device_id.trim();
                if dev.is_empty() {
                    return;
                }
                let now_ms = chrono::Utc::now().timestamp_millis();
                self.hosts.write().insert(
                    (user_id.0, dev.to_string()),
                    RegisteredCodingHost {
                        owner_user_id: user_id.0,
                        device_id: dev.to_string(),
                        display_name,
                        os_name,
                        providers,
                        session_id: session_id.map(str::to_string),
                        last_seen_ms: now_ms,
                        online: true,
                    },
                );
            }
            CodingHostEvent::ProbeResult {
                req_id,
                device_id,
                providers,
            } => {
                let pending = {
                    let mut map = self.pending_probes.write();
                    if let Some(entry) = map.get(&req_id) {
                        if entry.owner_user_id != user_id.0 {
                            warn!(
                                req_id = %req_id,
                                expected = entry.owner_user_id,
                                actual = user_id.0,
                                "Ignoring ProbeResult from mismatched user"
                            );
                            return;
                        }
                    }
                    map.remove(&req_id)
                };
                if let Some(host) = self
                    .hosts
                    .write()
                    .get_mut(&(user_id.0, device_id.clone()))
                {
                    host.providers = providers.clone();
                    host.last_seen_ms = chrono::Utc::now().timestamp_millis();
                    host.online = true;
                    if session_id.is_some() {
                        host.session_id = session_id.map(str::to_string);
                    }
                }
                if let Some(p) = pending {
                    let _ = p.tx.send(CodingHostEvent::ProbeResult {
                        req_id,
                        device_id,
                        providers,
                    });
                }
            }
            ev @ CodingHostEvent::ControlResult { .. } => {
                let req_id = match &ev {
                    CodingHostEvent::ControlResult { req_id, .. } => req_id.clone(),
                    _ => unreachable!(),
                };
                let pending = {
                    let mut map = self.pending_controls.write();
                    if let Some(entry) = map.get(&req_id) {
                        if entry.owner_user_id != user_id.0 {
                            warn!(
                                req_id = %req_id,
                                expected = entry.owner_user_id,
                                actual = user_id.0,
                                "Ignoring ControlResult from mismatched user"
                            );
                            return;
                        }
                    }
                    map.remove(&req_id)
                };
                if let Some(p) = pending {
                    let _ = p.tx.send(ev);
                } else {
                    debug!(req_id = %req_id, "Received ControlResult for unknown/expired request");
                }
            }
            CodingHostEvent::TurnStarted {
                turn_id,
                provider_session_id,
                workspace_path,
                worktree_path,
                git_branch,
                turn_idx,
            } => {
                let Some(session) = self.active_turns.read().get(&turn_id).cloned() else {
                    return;
                };
                let (chat_id, bot_uid) = {
                    let mut inner = session.inner.lock();
                    if inner.owner_user_id != user_id.0 {
                        warn!("Ignoring TurnStarted from mismatched user {}", user_id.0);
                        return;
                    }
                    inner.provider_session_id = provider_session_id;
                    if !workspace_path.trim().is_empty() {
                        inner.workspace_path = workspace_path;
                    }
                    inner.worktree_path = worktree_path;
                    inner.git_branch = git_branch;
                    inner.turn_idx = turn_idx;
                    (ChatId(inner.chat_id), UserId(inner.bot_user_id as u64))
                };
                if let Some(state) = self.state_weak.get().and_then(|w| w.upgrade()) {
                    BotTypingGuard::emit_activity(
                        &state,
                        chat_id,
                        bot_uid,
                        "thinking",
                        Some(format!("已启动 Turn #{turn_idx}")),
                    );
                }
            }
            CodingHostEvent::TurnActivity {
                turn_id,
                state: activity_state,
                detail,
            } => {
                let Some(session) = self.active_turns.read().get(&turn_id).cloned() else {
                    return;
                };
                let (chat_id, bot_uid) = {
                    let inner = session.inner.lock();
                    if inner.owner_user_id != user_id.0 {
                        return;
                    }
                    (ChatId(inner.chat_id), UserId(inner.bot_user_id as u64))
                };
                if let Some(state) = self.state_weak.get().and_then(|w| w.upgrade()) {
                    BotTypingGuard::emit_activity(
                        &state,
                        chat_id,
                        bot_uid,
                        &activity_state,
                        detail,
                    );
                }
            }
            CodingHostEvent::TurnThinkingDelta { turn_id, delta } => {
                let Some(session) = self.active_turns.read().get(&turn_id).cloned() else {
                    return;
                };
                let flush_action = {
                    let mut inner = session.inner.lock();
                    if inner.owner_user_id != user_id.0 || inner.finished {
                        return;
                    }
                    inner.thinking_buf.push_str(&delta);
                    if inner.thinking_buf.trim().is_empty() {
                        None
                    } else {
                        Some(self.prepare_stream_flush(&mut inner))
                    }
                };
                if let Some(action) = flush_action {
                    self.execute_flush_action(&session, action);
                }
            }
            CodingHostEvent::TurnToolStep {
                turn_id,
                call_id,
                tool_name,
                title,
                status,
                output_preview,
            } => {
                let Some(session) = self.active_turns.read().get(&turn_id).cloned() else {
                    return;
                };
                let flush_action = {
                    let mut inner = session.inner.lock();
                    if inner.owner_user_id != user_id.0 || inner.finished {
                        return;
                    }
                    if let Some(existing) =
                        inner.tool_steps.iter_mut().find(|s| s.call_id == call_id)
                    {
                        existing.tool_name = tool_name;
                        existing.title = title;
                        existing.status = status;
                        if output_preview.is_some() {
                            existing.output_preview = output_preview;
                        }
                    } else {
                        inner.tool_steps.push(CodingToolStepState {
                            call_id,
                            tool_name,
                            title,
                            status,
                            output_preview,
                        });
                    }
                    Some(self.prepare_stream_flush(&mut inner))
                };
                if let Some(action) = flush_action {
                    self.execute_flush_action(&session, action);
                }
            }
            CodingHostEvent::TurnTextDelta { turn_id, delta } => {
                let Some(session) = self.active_turns.read().get(&turn_id).cloned() else {
                    return;
                };
                let flush_action = {
                    let mut inner = session.inner.lock();
                    if inner.owner_user_id != user_id.0 || inner.finished {
                        return;
                    }
                    inner.text_buf.push_str(&delta);
                    if inner.text_buf.trim().is_empty() && inner.bot_msg_id.is_none() {
                        None
                    } else {
                        Some(self.prepare_stream_flush(&mut inner))
                    }
                };
                if let Some(action) = flush_action {
                    self.execute_flush_action(&session, action);
                }
            }
            CodingHostEvent::TurnApprovalRequested {
                turn_id,
                request_id,
                kind,
                title,
                detail,
                rule_key,
            } => {
                let Some(session) = self.active_turns.read().get(&turn_id).cloned() else {
                    return;
                };
                let info_tuple = {
                    let mut inner = session.inner.lock();
                    if inner.owner_user_id != user_id.0 || inner.finished {
                        return;
                    }
                    // Flush any accumulated streaming content before posting the approval card.
                    if inner.dirty {
                        if let Some(state) = self.state_weak.get().and_then(|w| w.upgrade()) {
                            Self::flush_locked_inner(&state, &mut inner, false, None, None);
                        }
                    }
                    (
                        inner.owner_user_id,
                        inner.bot_user_id,
                        inner.chat_id,
                        inner.topic_id,
                        inner.allow_group_members,
                        inner.target_device_id.clone(),
                        inner.target_session_id.clone(),
                    )
                };
                let (
                    owner_user_id,
                    bot_user_id,
                    chat_id,
                    topic_id,
                    allow_group_members,
                    target_device_id,
                    target_session_id,
                ) = info_tuple;

                let key = (turn_id.clone(), request_id.clone());
                self.pending_approvals.write().insert(
                    key.clone(),
                    PendingApprovalInfo {
                        owner_user_id,
                        bot_user_id,
                        chat_id,
                        topic_id,
                        allow_group_members,
                        target_device_id,
                        target_session_id,
                        kind: kind.clone(),
                        title: title.clone(),
                        approval_msg_id: None,
                        resolved: false,
                    },
                );

                if let Some(state) = self.state_weak.get().and_then(|w| w.upgrade()) {
                    BotTypingGuard::emit_activity(
                        &state,
                        ChatId(chat_id),
                        UserId(bot_user_id as u64),
                        "approval",
                        Some(title.clone()),
                    );
                    let detail_clipped = truncate_chars(detail.trim(), 600);
                    let card_text = format!(
                        "🔐 **需要执行权限审批** (`{kind}`)\n**操作**：{title}\n**规则**：`{rule_key}`\n```\n{detail_clipped}\n```"
                    );
                    let kb_rows = vec![vec![
                        InlineKeyboardButton {
                            text: "✅ 允许本次".to_string(),
                            callback_data: Some(format!("coding:approve:{turn_id}:{request_id}:once")),
                            url: None,
                            web_app_url: None,
                        },
                        InlineKeyboardButton {
                            text: "🔓 本会话允许".to_string(),
                            callback_data: Some(format!(
                                "coding:approve:{turn_id}:{request_id}:session"
                            )),
                            url: None,
                            web_app_url: None,
                        },
                        InlineKeyboardButton {
                            text: "❌ 拒绝".to_string(),
                            callback_data: Some(format!("coding:approve:{turn_id}:{request_id}:deny")),
                            url: None,
                            web_app_url: None,
                        },
                    ]];
                    if let Ok(msg_id) = send_bot_chat_reply_in_topic(
                        &state,
                        ChatId(chat_id),
                        UserId(bot_user_id as u64),
                        &card_text,
                        kb_rows,
                        topic_id.map(TopicId),
                    ) {
                        if let Some(entry) = self.pending_approvals.write().get_mut(&key) {
                            entry.approval_msg_id = Some(msg_id);
                        }
                    }
                }
            }
            CodingHostEvent::TurnCompleted {
                turn_id,
                ok,
                final_text,
                error,
                turn_idx,
                checkpoint_ref: _,
                diff_stat,
                token_usage,
            } => {
                let Some(session) = self.active_turns.read().get(&turn_id).cloned() else {
                    return;
                };
                let (typing_guard, bot_user_id, chat_id, topic_id) = {
                    let mut inner = session.inner.lock();
                    if inner.owner_user_id != user_id.0 {
                        warn!("Ignoring TurnCompleted from mismatched user {}", user_id.0);
                        return;
                    }
                    inner.finished = true;
                    inner.dirty = false;
                    inner.flush_scheduled = false;
                    if turn_idx > 0 {
                        inner.turn_idx = turn_idx;
                    }
                    if !final_text.trim().is_empty()
                        && (inner.text_buf.trim().is_empty()
                            || final_text.len() >= inner.text_buf.len())
                    {
                        inner.text_buf = final_text;
                    }
                    if let Some(state) = self.state_weak.get().and_then(|w| w.upgrade()) {
                        let outcome = TurnCompletionRender {
                            ok,
                            error,
                            diff_stat,
                            token_usage,
                        };
                        Self::flush_locked_inner(&state, &mut inner, true, Some(&outcome), None);
                    }
                    (
                        inner.typing_guard.take(),
                        inner.bot_user_id,
                        inner.chat_id,
                        inner.topic_id,
                    )
                };

                if let Some(guard) = typing_guard {
                    guard.finish();
                }

                // Clean up active turn tracking (keeping turn_meta for late callback checks)
                // and purge any pending approvals belonging to this completed turn.
                self.active_turns.write().remove(&turn_id);
                let mut expired_approvals = Vec::new();
                {
                    let mut approvals = self.pending_approvals.write();
                    let stale_keys: Vec<(String, String)> = approvals
                        .keys()
                        .filter(|(tid, _)| tid == &turn_id)
                        .cloned()
                        .collect();
                    for k in stale_keys {
                        if let Some(info) = approvals.remove(&k) {
                            if !info.resolved {
                                expired_approvals.push(info);
                            }
                        }
                    }
                }
                if !expired_approvals.is_empty() {
                    if let Some(state) = self.state_weak.get().and_then(|w| w.upgrade()) {
                        for info in expired_approvals {
                            if let Some(mid) = info.approval_msg_id {
                                let expired_txt = format!(
                                    "🔐 **权限审批已失效** (`{}`)\n**操作**：{}\n**状态**：本轮执行已结束",
                                    info.kind, info.title
                                );
                                let _ = edit_bot_chat_message(
                                    &state,
                                    ChatId(info.chat_id),
                                    mid,
                                    &expired_txt,
                                    Some(Vec::new()),
                                );
                            }
                        }
                    }
                }
                let thread_k = Self::thread_key(bot_user_id, chat_id, topic_id);
                let mut chat_turns = self.chat_active_turns.write();
                if chat_turns.get(&thread_k).map(String::as_str) == Some(turn_id.as_str()) {
                    chat_turns.remove(&thread_k);
                }
            }
        }
    }

    fn prepare_stream_flush(&self, inner: &mut ActiveTurnInner) -> FlushDecision {
        inner.dirty = true;
        if inner.bot_msg_id.is_none()
            || inner
                .last_edit_at
                .is_none_or(|t| t.elapsed() >= CODING_STREAM_EDIT_THROTTLE)
        {
            if let Some(state) = self.state_weak.get().and_then(|w| w.upgrade()) {
                Self::flush_locked_inner(&state, inner, false, None, None);
            }
            FlushDecision::FlushedNow
        } else if !inner.flush_scheduled {
            inner.flush_scheduled = true;
            let elapsed = inner
                .last_edit_at
                .map(|t| t.elapsed())
                .unwrap_or(CODING_STREAM_EDIT_THROTTLE);
            let delay = CODING_STREAM_EDIT_THROTTLE.saturating_sub(elapsed);
            FlushDecision::ScheduleTimer(delay)
        } else {
            FlushDecision::AlreadyScheduled
        }
    }

    fn execute_flush_action(&self, session: &Arc<TurnSession>, decision: FlushDecision) {
        if let FlushDecision::ScheduleTimer(delay) = decision {
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                let session_weak = Arc::downgrade(session);
                let state_weak = Arc::clone(&self.state_weak);
                rt.spawn(async move {
                    tokio::time::sleep(delay).await;
                    let Some(sess) = session_weak.upgrade() else {
                        return;
                    };
                    let Some(state) = state_weak.get().and_then(|w| w.upgrade()) else {
                        return;
                    };
                    let mut inner = sess.inner.lock();
                    inner.flush_scheduled = false;
                    if inner.finished || !inner.dirty {
                        return;
                    }
                    Self::flush_locked_inner(&state, &mut inner, false, None, None);
                });
            }
        }
    }

    fn flush_locked_inner(
        state: &Arc<AppState>,
        inner: &mut ActiveTurnInner,
        is_final: bool,
        completion: Option<&TurnCompletionRender>,
        _unused: Option<()>,
    ) {
        let rendered = render_turn_markdown(inner, is_final, completion);
        if rendered.trim().is_empty() {
            return;
        }

        let kb_rows = if !is_final {
            vec![vec![InlineKeyboardButton {
                text: "⏹️ 停止".to_string(),
                callback_data: Some(format!("coding:stop:{}", inner.turn_id)),
                url: None,
                web_app_url: None,
            }]]
        } else if let Some(c) = completion {
            if let Some(ref diff) = c.diff_stat {
                if diff.files_changed > 0 {
                    vec![vec![
                        InlineKeyboardButton {
                            text: format!(
                                "📄 查看 Diff (+{} -{})",
                                diff.insertions, diff.deletions
                            ),
                            callback_data: Some(format!(
                                "coding:diff:{}:{}",
                                inner.bot_user_id, inner.turn_idx
                            )),
                            url: None,
                            web_app_url: None,
                        },
                        InlineKeyboardButton {
                            text: "↩️ 回滚本轮".to_string(),
                            callback_data: Some(format!(
                                "coding:rollback:{}:{}",
                                inner.bot_user_id, inner.turn_idx
                            )),
                            url: None,
                            web_app_url: None,
                        },
                        InlineKeyboardButton {
                            text: "🚀 Commit & PR".to_string(),
                            callback_data: Some(format!(
                                "coding:pr:{}:{}",
                                inner.bot_user_id, inner.turn_idx
                            )),
                            url: None,
                            web_app_url: None,
                        },
                    ]]
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let kb_json = serde_json::to_string(&kb_rows).unwrap_or_else(|_| "[]".to_string());
        let chat_id = ChatId(inner.chat_id);
        let bot_uid = UserId(inner.bot_user_id as u64);

        if let Some(msg_id) = inner.bot_msg_id {
            let kb_changed = inner.last_kb_json.as_deref() != Some(kb_json.as_str());
            if rendered != inner.last_rendered || kb_changed {
                let kb_arg = if kb_changed { Some(kb_rows) } else { None };
                if edit_bot_chat_message(state, chat_id, msg_id, &rendered, kb_arg).is_ok() {
                    inner.last_rendered = rendered;
                    inner.last_kb_json = Some(kb_json);
                }
            }
        } else if let Ok(msg_id) = send_bot_chat_reply_in_topic(
            state,
            chat_id,
            bot_uid,
            &rendered,
            kb_rows,
            inner.topic_id.map(TopicId),
        ) {
            inner.bot_msg_id = Some(msg_id);
            inner.last_rendered = rendered;
            inner.last_kb_json = Some(kb_json);
        }

        inner.last_edit_at = Some(Instant::now());
        inner.dirty = false;
    }
}

enum FlushDecision {
    FlushedNow,
    ScheduleTimer(Duration),
    AlreadyScheduled,
}

struct TurnCompletionRender {
    ok: bool,
    error: Option<String>,
    diff_stat: Option<CodingDiffStat>,
    token_usage: Option<CodingTokenUsage>,
}

fn render_turn_markdown(
    inner: &ActiveTurnInner,
    is_final: bool,
    completion: Option<&TurnCompletionRender>,
) -> String {
    let mut sections: Vec<String> = Vec::new();

    if !is_final && !inner.thinking_buf.trim().is_empty() {
        let bounded = tail_chars(inner.thinking_buf.trim(), 600);
        let quoted = bounded.replace('\n', "\n> ");
        sections.push(format!("💭 **思考过程**\n> {quoted}"));
    }

    if !inner.tool_steps.is_empty() {
        let total = inner.tool_steps.len();
        let skip = total.saturating_sub(MAX_STREAM_STEPS_SHOWN);
        let mut lines = Vec::new();
        if skip > 0 {
            lines.push(format!("… 已省略前 {skip} 个工具步骤"));
        }
        for (offset, step) in inner.tool_steps.iter().skip(skip).enumerate() {
            let idx = skip + offset + 1;
            let icon = match step.status.to_ascii_lowercase().as_str() {
                "completed" | "ok" | "success" | "done" => "✅",
                "failed" | "error" | "denied" | "cancelled" => "❌",
                _ => "⏳",
            };
            let mut line = format!("{idx}. {icon} **{}**: {}", step.tool_name, step.title);
            if let Some(ref preview) = step.output_preview {
                let p = preview.trim();
                if !p.is_empty() {
                    line.push_str(&format!(" — `{}`", truncate_chars(p, 72)));
                }
            }
            lines.push(line);
        }
        sections.push(lines.join("\n"));
    }

    if !inner.text_buf.trim().is_empty() {
        sections.push(inner.text_buf.trim().to_string());
    }

    if is_final {
        if let Some(c) = completion {
            if !c.ok || c.error.is_some() {
                let err_text = c
                    .error
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("执行已中断或未完成");
                sections.push(format!("⚠️ **执行异常**：{err_text}"));
            }
            if let Some(ref diff) = c.diff_stat {
                if diff.files_changed > 0 {
                    sections.push(format!(
                        "📝 **工作区变更**：{} 个文件 (+{} -{})",
                        diff.files_changed, diff.insertions, diff.deletions
                    ));
                }
            }
            if let Some(ref u) = c.token_usage {
                let mut parts = vec![
                    format!("输入 {}", u.input_tokens),
                    format!("输出 {}", u.output_tokens),
                ];
                if u.cached_tokens > 0 {
                    parts.push(format!("缓存 {}", u.cached_tokens));
                }
                if let Some(cost) = u.cost_usd {
                    parts.push(format!("${cost:.4}"));
                }
                sections.push(format!("📊 **Token 用量**：{}", parts.join(" · ")));
            }
        }
        if sections.is_empty() {
            if !inner.thinking_buf.trim().is_empty() {
                sections.push(inner.thinking_buf.trim().to_string());
            } else {
                sections.push("✅ 本轮执行已完成。".to_string());
            }
        }
    }

    sections.join("\n\n")
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let collected: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{collected}…")
    } else {
        collected
    }
}

fn tail_chars(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    let skipped: String = s.chars().skip(count - max_chars).collect();
    format!("…{skipped}")
}
