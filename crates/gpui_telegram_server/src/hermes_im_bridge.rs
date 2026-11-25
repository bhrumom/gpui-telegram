use crate::db::miniapp_store::MiniAppRecord;
use crate::db::{ChatMemberRecord, ChatRecord, ForumTopicRecord, ServerDatabase};
use crate::messaging::pipeline::MessagePipeline;
use crate::miniapp_webmcp::{
    MiniAppWebMcpBroker, DEFAULT_TAKEOVER_WAIT_TIMEOUT, DEFAULT_WEBMCP_RPC_TIMEOUT,
};
use crate::search::hybrid_memory::HybridMemoryEngine;
use crate::search::schema::{SearchDocument, SearchQueryParams};
use crate::search::tantivy_indexer::TantivyIndexer;
use crate::server::{extract_message_content_text, AppState};
use crate::sync::pts_engine::PtsEngine;
use crate::ws::GatewayHub;
use chrono::Utc;
use gpui_telegram_agent::hermes::toolsets::{BoxFuture, PlatformImBridge, PlatformImCaller};
use gpui_telegram_core::types::{ChatId, MsgId, PeerId, TopicId, UserId};
use gpui_telegram_proto::cards::GenerativeCard;
use gpui_telegram_proto::contracts::ForumTopicDto;
use gpui_telegram_proto::message::{MessageContent, MessageItem, ReplyHeader};
use gpui_telegram_proto::miniapp_contracts::{
    MiniAppActionRequest, MiniAppManifestDto, MiniAppUiSchema, MiniAppWebMcpCommand,
    WebMcpToolDescriptor,
};
use gpui_telegram_proto::sync::{PtsUpdate, ReactionEvent, UpdateEvent};
use gpui_telegram_proto::ws::WsServerFrame;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock, Weak};
use uuid::Uuid;

fn extract_webmcp_page_error(summary: Option<&Value>) -> Option<String> {
    let obj = summary?.as_object()?;
    if let Some(boot_err) = obj
        .get("boot_error")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(boot_err.to_string());
    }
    if let Some(arr) = obj.get("page_errors").and_then(|v| v.as_array()) {
        let msgs: Vec<&str> = arr
            .iter()
            .filter_map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()))
            .collect();
        if !msgs.is_empty() {
            return Some(msgs.join("; "));
        }
    }
    if let Some(toast) = obj
        .get("visible_toast")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let lower = toast.to_ascii_lowercase();
        const INDICATORS: [&str; 10] = [
            "失败",
            "error",
            "exception",
            "compileerror",
            "wasm",
            "404",
            "500",
            "无法",
            "崩溃",
            "failed",
        ];
        if INDICATORS.iter().any(|ind| lower.contains(ind)) {
            return Some(toast.to_string());
        }
    }
    None
}

pub struct ServerPlatformImBridge {
    db: Arc<ServerDatabase>,
    pipeline: Arc<MessagePipeline>,
    pts_engine: Arc<PtsEngine>,
    gateway_hub: Weak<GatewayHub>,
    search_indexer: Weak<TantivyIndexer>,
    hybrid_memory: Weak<HybridMemoryEngine>,
    webmcp_broker: Weak<MiniAppWebMcpBroker>,
    community_search: Weak<gpui_telegram_community::CommunitySearchIndexer>,
    community_bot_guard: Weak<gpui_telegram_community::BotLoopGuard>,
    state_weak: Arc<OnceLock<Weak<AppState>>>,
}

impl ServerPlatformImBridge {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<ServerDatabase>,
        pipeline: Arc<MessagePipeline>,
        pts_engine: Arc<PtsEngine>,
        gateway_hub: Weak<GatewayHub>,
        search_indexer: Weak<TantivyIndexer>,
        hybrid_memory: Weak<HybridMemoryEngine>,
        webmcp_broker: Weak<MiniAppWebMcpBroker>,
        community_search: Weak<gpui_telegram_community::CommunitySearchIndexer>,
        community_bot_guard: Weak<gpui_telegram_community::BotLoopGuard>,
        state_weak: Arc<OnceLock<Weak<AppState>>>,
    ) -> Self {
        Self {
            db,
            pipeline,
            pts_engine,
            gateway_hub,
            search_indexer,
            hybrid_memory,
            webmcp_broker,
            community_search,
            community_bot_guard,
            state_weak,
        }
    }

    fn resolve_caller_user(&self, caller: &PlatformImCaller) -> Result<UserId, String> {
        if let Some(uid) = caller.user_id.filter(|&u| u > 0) {
            return Ok(UserId(uid));
        }
        if let Some(bid) = caller.bot_id.filter(|&b| b > 0) {
            return Ok(UserId(bid as u64));
        }
        if let Some(ref agent_id) = caller.bound_agent_id {
            if let Ok(bots) = self.db.list_all_bots() {
                if let Some(bot) = bots.into_iter().find(|b| b.agent_id == *agent_id) {
                    return Ok(UserId(bot.bot_user_id as u64));
                }
            }
            if let Ok(Some(bot)) = self.db.get_bot_by_username(agent_id) {
                return Ok(UserId(bot.bot_user_id as u64));
            }
        }
        Err("Authenticated caller user_id or bot_id is required for PlatformIm operations".into())
    }

    fn reject_cross_chat_arg(args: &Value, pinned: i64) -> Result<(), String> {
        if let Some(explicit_val) = args.get("chat_id").filter(|v| !v.is_null()) {
            if explicit_val.as_i64() != Some(pinned) {
                return Err(format!(
                    "Cross-chat target mismatch: caller context is pinned to chat {pinned}, got {explicit_val}"
                ));
            }
        }
        Ok(())
    }

    fn require_chat_id(args: &Value, caller: &PlatformImCaller) -> Result<ChatId, String> {
        if let Some(pinned) = caller.chat_id {
            Self::reject_cross_chat_arg(args, pinned)?;
            if pinned <= 0 {
                return Err("Positive 'chat_id' is required".to_string());
            }
            return Ok(ChatId(pinned));
        }
        args.get("chat_id")
            .and_then(|v| v.as_i64())
            .filter(|&c| c > 0)
            .map(ChatId)
            .ok_or_else(|| "Positive 'chat_id' is required".to_string())
    }

    fn check_membership(&self, chat_id: ChatId, user_id: UserId) -> Result<(ChatRecord, bool), String> {
        let chat = self
            .db
            .get_chat(chat_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Chat {} not found", chat_id.0))?;
        let member = self
            .db
            .get_chat_member(chat_id, user_id)
            .map_err(|e| e.to_string())?;
        let is_creator = chat.creator_id == user_id;
        let is_active = is_creator
            || member
                .as_ref()
                .is_some_and(|m| !ServerDatabase::is_inactive_member_role(&m.role));
        if !is_active {
            return Err(format!("User {} is not an active member of chat {}", user_id.0, chat_id.0));
        }
        let is_admin = is_creator
            || member.as_ref().is_some_and(|m| {
                ServerDatabase::is_admin_member_role(&m.role) || m.admin_rights != 0
            });
        Ok((chat, is_admin))
    }

    fn check_send_permitted(&self, chat_id: ChatId, user_id: UserId) -> Result<(ChatRecord, bool), String> {
        let (chat, is_admin) = self.check_membership(chat_id, user_id)?;
        if let Some(m) = self.db.get_chat_member(chat_id, user_id).map_err(|e| e.to_string())? {
            if m.role.trim().eq_ignore_ascii_case("restricted") && (m.banned_rights & 1) != 0 {
                return Err("User is restricted from sending messages".to_string());
            }
        }
        if matches!(chat.chat_type, 0 | 4) {
            if let Ok(members) = self.db.get_chat_members(chat_id) {
                for peer_uid in members {
                    if peer_uid != user_id
                        && (self.db.is_user_blocked(peer_uid, user_id).unwrap_or(false)
                            || self.db.is_user_blocked(user_id, peer_uid).unwrap_or(false))
                    {
                        return Err("Cannot send messages in a blocked direct chat".to_string());
                    }
                }
            }
        }
        Ok((chat, is_admin))
    }

    fn broadcast_pts(&self, chat_id: ChatId, update: PtsUpdate) {
        if let Some(hub) = self.gateway_hub.upgrade() {
            hub.broadcast_to_chat_and_members(chat_id, None, WsServerFrame::PtsPush(update));
        }
    }

    fn broadcast_event(&self, chat_id: ChatId, event: UpdateEvent) {
        if let Ok((_pts, update)) = self.pts_engine.advance_pts(chat_id, 1, event.clone()) {
            self.broadcast_pts(chat_id, update);
        } else if let Some(hub) = self.gateway_hub.upgrade() {
            hub.broadcast_to_chat_and_members(chat_id, None, WsServerFrame::Update(event));
        }
    }

    fn resolve_target_miniapp(
        &self,
        args: &Value,
        caller: &PlatformImCaller,
        caller_uid: UserId,
    ) -> Result<(MiniAppRecord, Option<ChatId>), String> {
        let explicit_bot = args
            .get("bot_username")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().trim_start_matches('@').to_string())
            .filter(|s| !s.is_empty());
        let explicit_short = args
            .get("short_name")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let app = if let (Some(bot_un), Some(short)) = (&explicit_bot, &explicit_short) {
            self.db
                .get_mini_app(bot_un, short)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("MiniApp '{bot_un}/{short}' not found"))?
        } else if let Some(bot_un) = &explicit_bot {
            let bot = self
                .db
                .get_bot_by_username(bot_un)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("Bot '@{bot_un}' not found"))?;
            self.db
                .list_mini_apps_for_bot(bot.bot_user_id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .next()
                .ok_or_else(|| format!("Bot '@{bot_un}' has no attached Mini App"))?
        } else if let Some(bot_id) = caller.bot_id.filter(|&id| id > 0) {
            let apps = self
                .db
                .list_mini_apps_for_bot(bot_id)
                .map_err(|e| e.to_string())?;
            if let Some(short) = &explicit_short {
                apps.into_iter()
                    .find(|a| a.short_name.eq_ignore_ascii_case(short))
                    .ok_or_else(|| format!("MiniApp '{short}' not found on caller bot {bot_id}"))?
            } else {
                apps.into_iter()
                    .next()
                    .ok_or_else(|| format!("Caller bot {bot_id} has no attached Mini App"))?
            }
        } else {
            return Err("'bot_username' and 'short_name' are required".to_string());
        };

        let explicit_chat = args.get("chat_id").and_then(|v| v.as_i64()).filter(|&c| c > 0);
        let target_chat = if let Some(pinned) = caller.chat_id {
            Self::reject_cross_chat_arg(args, pinned)?;
            Some(ChatId(pinned)).filter(|c| c.0 > 0)
        } else {
            explicit_chat.map(ChatId)
        };
        if let Some(chat_id) = target_chat {
            let _ = self.check_membership(chat_id, caller_uid)?;
        }
        Ok((app, target_chat))
    }

    fn headless_webmcp_catalog(app: &MiniAppRecord) -> Vec<WebMcpToolDescriptor> {
        vec![
            WebMcpToolDescriptor {
                name: "get_state".to_string(),
                description: format!("读取 {} 小程序当前界面状态与字段", app.title),
                input_schema: json!({ "type": "object", "properties": {} }),
                read_only: true,
            },
            WebMcpToolDescriptor {
                name: "page_inspect".to_string(),
                description: "探测当前网页可交互元素（按钮、输入框、表单项）".to_string(),
                input_schema: json!({ "type": "object", "properties": {} }),
                read_only: true,
            },
            WebMcpToolDescriptor {
                name: "page_click".to_string(),
                description: "点击网页内匹配 selector 或按钮文案的元素".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "selector": { "type": "string" },
                        "text": { "type": "string" }
                    }
                }),
                read_only: false,
            },
            WebMcpToolDescriptor {
                name: "page_fill".to_string(),
                description: "填写输入框或更新指定组件值".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["value"],
                    "properties": {
                        "selector": { "type": "string" },
                        "widget_id": { "type": "string" },
                        "value": {}
                    }
                }),
                read_only: false,
            },
            WebMcpToolDescriptor {
                name: "page_press_main_button".to_string(),
                description: "点击 Mini App 底部主操作按钮".to_string(),
                input_schema: json!({ "type": "object", "properties": {} }),
                read_only: false,
            },
        ]
    }

    fn execute_headless_webmcp_tool(
        &self,
        app: &MiniAppRecord,
        caller_uid: UserId,
        chat_id: i64,
        tool_name: &str,
        args: &Value,
    ) -> Result<Value, String> {
        let uid = caller_uid.0 as i64;
        let latest = self
            .db
            .get_latest_mini_app_state_for_bot_user(uid, app.bot_user_id)
            .ok()
            .flatten();
        let current_state: Value = latest
            .as_ref()
            .and_then(|s| serde_json::from_str(&s.state_json).ok())
            .unwrap_or_else(|| crate::routes::bots_miniapps::initial_state_for_app(app));

        if matches!(tool_name, "get_state" | "page_inspect") {
            return Ok(json!({
                "success": true,
                "mode": "headless_db",
                "state": current_state,
            }));
        }

        let mapped_req = match tool_name {
            "page_click" => {
                let selector = args
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let widget_id = args
                    .get("widget_id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let schema: Option<MiniAppUiSchema> =
                    serde_json::from_str(&app.ui_schema_json).ok();
                let matches_main = selector.is_some_and(|s| {
                    matches!(
                        s.to_ascii_lowercase().as_str(),
                        "#main-btn" | ".main-btn" | "main_button" | "main-button"
                    )
                }) || text.is_some_and(|t| {
                    schema
                        .as_ref()
                        .and_then(|sc| sc.main_button.as_ref())
                        .is_some_and(|mb| {
                            !mb.text.trim().is_empty() && mb.text.trim().eq_ignore_ascii_case(t)
                        })
                });
                let matches_secondary = selector.is_some_and(|s| {
                    matches!(
                        s.to_ascii_lowercase().as_str(),
                        "#secondary-btn"
                            | ".secondary-btn"
                            | "secondary_button"
                            | "secondary-button"
                    )
                }) || text.is_some_and(|t| {
                    schema
                        .as_ref()
                        .and_then(|sc| sc.secondary_button.as_ref())
                        .is_some_and(|sb| {
                            !sb.text.trim().is_empty() && sb.text.trim().eq_ignore_ascii_case(t)
                        })
                });
                if matches_main {
                    MiniAppActionRequest {
                        action_type: "main_button_click".to_string(),
                        chat_id: Some(chat_id),
                        ..Default::default()
                    }
                } else if matches_secondary {
                    MiniAppActionRequest {
                        action_type: "secondary_button_click".to_string(),
                        chat_id: Some(chat_id),
                        ..Default::default()
                    }
                } else if let Some(wid) =
                    widget_id.or_else(|| selector.and_then(|s| s.strip_prefix('#')))
                {
                    MiniAppActionRequest {
                        action_type: "widget_event".to_string(),
                        widget_id: Some(wid.to_string()),
                        value: args
                            .get("value")
                            .cloned()
                            .or_else(|| text.map(|t| json!(t))),
                        chat_id: Some(chat_id),
                        ..Default::default()
                    }
                } else if let Some((wid, opt_val)) = text.and_then(|t| {
                    schema.as_ref().and_then(|sc| {
                        sc.widgets.iter().find_map(|w| {
                            w.options
                                .iter()
                                .find(|opt| opt.trim().eq_ignore_ascii_case(t))
                                .map(|opt| (w.id.clone(), opt.clone()))
                        })
                    })
                }) {
                    MiniAppActionRequest {
                        action_type: "widget_event".to_string(),
                        widget_id: Some(wid),
                        value: Some(json!(opt_val)),
                        chat_id: Some(chat_id),
                        ..Default::default()
                    }
                } else if selector.is_none() && text.is_none() {
                    return Err("page_click 需要提供 'selector' 或 'text' 参数".to_string());
                } else {
                    MiniAppActionRequest {
                        action_type: "page_click".to_string(),
                        widget_id: selector.or(text).map(str::to_string),
                        value: text.or(selector).map(|s| json!(s)),
                        chat_id: Some(chat_id),
                        ..Default::default()
                    }
                }
            }
            "page_fill" => MiniAppActionRequest {
                action_type: "widget_event".to_string(),
                widget_id: args
                    .get("widget_id")
                    .or_else(|| args.get("selector"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                value: args.get("value").cloned(),
                chat_id: Some(chat_id),
                ..Default::default()
            },
            "page_press_main_button" | "main_button_click" => MiniAppActionRequest {
                action_type: "main_button_click".to_string(),
                chat_id: Some(chat_id),
                ..Default::default()
            },
            "page_press_secondary_button" | "secondary_button_click" => MiniAppActionRequest {
                action_type: "secondary_button_click".to_string(),
                chat_id: Some(chat_id),
                ..Default::default()
            },
            other => MiniAppActionRequest {
                action_type: other.to_string(),
                widget_id: args.get("widget_id").and_then(|v| v.as_str()).map(str::to_string),
                value: args.get("value").cloned(),
                data: args.get("data").and_then(|v| v.as_str()).map(str::to_string),
                chat_id: Some(chat_id),
                ..Default::default()
            },
        };
        let (next_state, toast, _, _) =
            crate::routes::bots_miniapps::reduce_mini_app_action(app, current_state, &mapped_req);
        let state_str = serde_json::to_string(&next_state).unwrap_or_else(|_| "{}".to_string());
        if let Some(ref existing) = latest {
            let _ = self
                .db
                .update_mini_app_session_state(&existing.query_id, &state_str);
        } else {
            let qid = format!("ma_{}", Uuid::new_v4().simple());
            let _ = self.db.create_mini_app_session(
                &qid,
                uid,
                &app.app_id,
                app.bot_user_id,
                chat_id,
                "inline",
                "",
                "",
                Utc::now().timestamp(),
                &state_str,
            );
        }
        Ok(json!({
            "success": true,
            "mode": "headless_db",
            "toast": toast,
            "state": next_state,
        }))
    }
}

impl PlatformImBridge for ServerPlatformImBridge {
    fn execute_im_tool<'a>(
        &'a self,
        tool_name: &'a str,
        args: &'a Value,
        caller: PlatformImCaller,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let now_ms = Utc::now().timestamp_millis();
            if gpui_telegram_agent::is_community_tool(tool_name) {
                let ctx = crate::routes::community::hermes_bridge::CommunityBridgeContext {
                    db: &self.db,
                    gateway_hub: &self.gateway_hub,
                    community_search: &self.community_search,
                    community_bot_guard: &self.community_bot_guard,
                    state_weak: &self.state_weak,
                };
                return crate::routes::community::hermes_bridge::execute_community_im_tool(
                    &ctx, tool_name, args, &caller, now_ms,
                );
            }

            let caller_uid = self.resolve_caller_user(&caller)?;

            match tool_name {
                "send_message" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let _ = self.check_send_permitted(chat_id, caller_uid)?;

                    let content = if let Some(card_val) = args.get("generative_card") {
                        let card: GenerativeCard = serde_json::from_value(card_val.clone())
                            .map_err(|e| format!("Invalid generative_card: {e}"))?;
                        MessageContent::GenerativeCard(card)
                    } else {
                        let text = args
                            .get("text")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .ok_or_else(|| "Non-empty 'text' or 'generative_card' is required".to_string())?;
                        MessageContent::plain_text(text)
                    };

                    let mut msg = MessageItem::new(
                        MsgId(0),
                        chat_id,
                        caller_uid,
                        PeerId::Chat(chat_id),
                        content.clone(),
                    );
                    if let Some(rid) = args.get("reply_to_id").and_then(|v| v.as_i64()) {
                        msg.reply_to = Some(ReplyHeader::new(MsgId(rid)));
                    }
                    msg.topic_id = args.get("topic_id").and_then(|v| v.as_i64()).map(TopicId);
                    msg.flags.is_silent = args.get("silent").and_then(|v| v.as_bool()).unwrap_or(false);
                    msg.client_nonce = args
                        .get("client_nonce")
                        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));

                    let pts_update = self
                        .pipeline
                        .process_incoming_message_strict(msg)
                        .map_err(|e| e.to_string())?;

                    let (msg_id, date_ms, indexed_text) = match &pts_update.event {
                        UpdateEvent::NewMessage(saved) => (
                            saved.id.0,
                            saved.date.timestamp_millis(),
                            extract_message_content_text(&saved.content),
                        ),
                        _ => (0, now_ms, extract_message_content_text(&content)),
                    };

                    if let Some(idx) = self.search_indexer.upgrade() {
                        idx.index_message_nonblocking(SearchDocument::new(
                            msg_id.to_string(),
                            chat_id.0.to_string(),
                            caller_uid.0.to_string(),
                            date_ms,
                            indexed_text.clone(),
                            None,
                        ));
                        idx.commit_nonblocking();
                    }
                    if let Some(hm) = self.hybrid_memory.upgrade() {
                        let doc_id = format!("{}:{}", chat_id.0, msg_id);
                        hm.index_message_or_file(
                            &doc_id,
                            chat_id.0,
                            "message",
                            &indexed_text,
                            vec![],
                        );
                    }
                    let pts = pts_update.pts;
                    self.broadcast_pts(chat_id, pts_update);
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "message_id": msg_id,
                        "pts": pts,
                        "text": indexed_text,
                    }))
                }
                "edit_message" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let (_chat, is_admin) = self.check_membership(chat_id, caller_uid)?;
                    let msg_id = args
                        .get("message_id")
                        .and_then(|v| v.as_i64())
                        .filter(|&id| id > 0)
                        .ok_or_else(|| "Positive 'message_id' is required".to_string())?;
                    let text = args
                        .get("text")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "Non-empty 'text' is required".to_string())?;
                    let existing = self
                        .db
                        .get_message(chat_id, MsgId(msg_id))
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Message {msg_id} not found"))?;
                    if existing.sender_id != caller_uid && !is_admin {
                        return Err("Permission denied: cannot edit another user's message".into());
                    }
                    let pts_update = self
                        .pipeline
                        .edit_message(chat_id, MsgId(msg_id), MessageContent::plain_text(text))
                        .map_err(|e| e.to_string())?;
                    if let Some(idx) = self.search_indexer.upgrade() {
                        idx.index_message_nonblocking(SearchDocument::new(
                            msg_id.to_string(),
                            chat_id.0.to_string(),
                            existing.sender_id.0.to_string(),
                            existing.date_ms,
                            text.to_string(),
                            None,
                        ));
                        idx.commit_nonblocking();
                    }
                    if let Some(hm) = self.hybrid_memory.upgrade() {
                        let doc_id = format!("{}:{}", chat_id.0, msg_id);
                        hm.index_message_or_file(
                            &doc_id,
                            chat_id.0,
                            "message",
                            text,
                            vec![],
                        );
                    }
                    let pts = pts_update.pts;
                    self.broadcast_pts(chat_id, pts_update);
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "message_id": msg_id,
                        "pts": pts,
                        "text": text,
                    }))
                }
                "delete_message" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let (_chat, is_admin) = self.check_membership(chat_id, caller_uid)?;
                    let mut ids: Vec<i64> = args
                        .get("message_ids")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|x| x.as_i64()).collect())
                        .unwrap_or_default();
                    if ids.is_empty() {
                        if let Some(id) = args.get("message_id").and_then(|v| v.as_i64()) {
                            ids.push(id);
                        }
                    }
                    if ids.is_empty() {
                        return Err("'message_id' or 'message_ids' is required".into());
                    }
                    for &id in &ids {
                        if let Ok(Some(m)) = self.db.get_message(chat_id, MsgId(id)) {
                            if m.sender_id != caller_uid && !is_admin {
                                return Err("Permission denied: cannot delete another user's message".into());
                            }
                        }
                    }
                    let revoke = args.get("revoke").and_then(|v| v.as_bool()).unwrap_or(true);
                    let prev_pins = self.db.list_pinned_messages(chat_id).unwrap_or_default();
                    let removed_pinned = prev_pins.iter().any(|pid| ids.contains(pid));
                    let msg_ids: Vec<MsgId> = ids.iter().copied().map(MsgId).collect();
                    let pts_update = self
                        .pipeline
                        .delete_messages(chat_id, msg_ids, revoke)
                        .map_err(|e| e.to_string())?;
                    if let Some(idx) = self.search_indexer.upgrade() {
                        for &id in &ids {
                            idx.delete_chat_message_nonblocking(chat_id.0.to_string(), id.to_string());
                        }
                        idx.commit_nonblocking();
                    }
                    if let Some(hm) = self.hybrid_memory.upgrade() {
                        for &id in &ids {
                            let doc_id = format!("{}:{}", chat_id.0, id);
                            hm.remove_document(&doc_id);
                        }
                    }
                    let pts = pts_update.pts;
                    self.broadcast_pts(chat_id, pts_update);
                    if removed_pinned {
                        let pinned_ids = self.db.list_pinned_messages(chat_id).unwrap_or_default();
                        self.broadcast_event(
                            chat_id,
                            UpdateEvent::PinnedMessagesChanged {
                                chat_id,
                                pinned_ids: pinned_ids.into_iter().map(MsgId).collect(),
                            },
                        );
                    }
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "deleted_ids": ids,
                        "pts": pts,
                    }))
                }
                "pin_message" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let (_chat, is_admin) = self.check_membership(chat_id, caller_uid)?;
                    if !is_admin {
                        return Err("Admin rights required to pin/unpin messages".into());
                    }
                    let msg_id = args
                        .get("message_id")
                        .and_then(|v| v.as_i64())
                        .filter(|&id| id > 0)
                        .ok_or_else(|| "Positive 'message_id' is required".to_string())?;
                    let unpin = args.get("unpin").and_then(|v| v.as_bool()).unwrap_or(false);
                    if !unpin {
                        match self.db.get_message(chat_id, MsgId(msg_id)) {
                            Ok(Some(m)) if m.deleted_at_ms.is_none() => {}
                            Ok(_) => return Err("Message not found or deleted".into()),
                            Err(e) => return Err(e.to_string()),
                        }
                    }
                    let pinned_ids = if unpin {
                        self.db
                            .unpin_message(chat_id, Some(MsgId(msg_id)))
                            .map_err(|e| e.to_string())?
                    } else {
                        self.db
                            .pin_message(chat_id, MsgId(msg_id), caller_uid)
                            .map_err(|e| e.to_string())?
                    };
                    self.broadcast_event(
                        chat_id,
                        UpdateEvent::PinnedMessagesChanged {
                            chat_id,
                            pinned_ids: pinned_ids.iter().copied().map(MsgId).collect(),
                        },
                    );
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "pinned_message_ids": pinned_ids,
                    }))
                }
                "add_reaction" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let _ = self.check_membership(chat_id, caller_uid)?;
                    let msg_id = args
                        .get("message_id")
                        .and_then(|v| v.as_i64())
                        .filter(|&id| id > 0)
                        .ok_or_else(|| "Positive 'message_id' is required".to_string())?;
                    let emoji = args
                        .get("emoji")
                        .or_else(|| args.get("reaction"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "'emoji' is required".to_string())?;
                    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("toggle");
                    let (pts, details, counts) = self
                        .db
                        .toggle_reaction_atomic(chat_id, MsgId(msg_id), caller_uid, emoji, action)
                        .map_err(|e| e.to_string())?;
                    let ev = UpdateEvent::ReactionUpdated(ReactionEvent::new(
                        chat_id,
                        MsgId(msg_id),
                        caller_uid,
                        counts.clone(),
                    ));
                    self.broadcast_pts(
                        chat_id,
                        PtsUpdate::new(chat_id, pts, 1, ev, now_ms),
                    );
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "message_id": msg_id,
                        "pts": pts,
                        "reactions": counts,
                        "details": details,
                    }))
                }
                "search_messages" => {
                    let query = args
                        .get("query")
                        .or_else(|| args.get("q"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "Non-empty 'query' is required".to_string())?;
                    let limit = args
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(20)
                        .clamp(1, 100) as usize;
                    let scoped_chat = if let Some(pinned) = caller.chat_id {
                        Self::reject_cross_chat_arg(args, pinned)?;
                        if pinned <= 0 {
                            return Err("Positive 'chat_id' is required".to_string());
                        }
                        Some(pinned)
                    } else {
                        args.get("chat_id").and_then(|v| v.as_i64()).filter(|&c| c > 0)
                    };
                    let allowed_chats: Vec<i64> = if let Some(cid) = scoped_chat {
                        let _ = self.check_membership(ChatId(cid), caller_uid)?;
                        vec![cid]
                    } else {
                        self.db
                            .list_user_member_chat_ids(caller_uid)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|c| c.0)
                            .collect()
                    };

                    let mut hits_json = Vec::new();
                    if let Some(hm) = self.hybrid_memory.upgrade() {
                        let hybrid_hits = hm.search_with_limit(query, None, &allowed_chats, None, limit);
                        for h in hybrid_hits {
                            hits_json.push(json!({
                                "message_id": h.message_id,
                                "chat_id": h.chat_id,
                                "content": h.content,
                                "rrf_score": h.rrf_score,
                                "text_rank": h.text_rank,
                                "vector_rank": h.vector_rank,
                                "graph_rank": h.graph_rank,
                            }));
                        }
                    }
                    if hits_json.is_empty() {
                        if let Some(idx) = self.search_indexer.upgrade() {
                            if let Ok(t_hits) = idx.search_hits_for_chats(query, &allowed_chats, limit) {
                                for h in t_hits {
                                    hits_json.push(json!({
                                        "message_id": h.message_id,
                                        "chat_id": h.chat_id.parse::<i64>().unwrap_or(0),
                                        "content": h.snippet_html,
                                        "score": h.score,
                                        "date_ms": h.date,
                                    }));
                                }
                            } else if let Ok(t_hits) = idx.search_hits(&SearchQueryParams::new(query).with_limit(limit)) {
                                for h in t_hits {
                                    if let Ok(cid) = h.chat_id.parse::<i64>() {
                                        if allowed_chats.contains(&cid) {
                                            hits_json.push(json!({
                                                "message_id": h.message_id,
                                                "chat_id": cid,
                                                "content": h.snippet_html,
                                                "score": h.score,
                                            }));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(json!({
                        "query": query,
                        "count": hits_json.len(),
                        "hits": hits_json,
                    }))
                }
                "create_channel" => {
                    let title = args
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "Non-empty 'title' is required".to_string())?
                        .to_string();
                    let is_forum = args.get("is_forum").and_then(|v| v.as_bool()).unwrap_or(false);
                    let mut chat_type: u8 = match args
                        .get("chat_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("channel")
                        .to_ascii_lowercase()
                        .as_str()
                    {
                        "group" => 1,
                        "supergroup" => 2,
                        "channel" => 3,
                        _ => 3,
                    };
                    if is_forum && chat_type == 1 {
                        chat_type = 2;
                    }
                    let about = args.get("about").and_then(|v| v.as_str()).map(ToString::to_string);
                    let username = args
                        .get("username")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().trim_start_matches('@').to_string())
                        .filter(|s| !s.is_empty());
                    let chat_id = self.db.next_chat_id().map_err(|e| e.to_string())?;
                    let peer_type = if matches!(chat_type, 2 | 3) { 3 } else { 2 };
                    let rec = ChatRecord {
                        chat_id,
                        chat_type,
                        peer_type,
                        title: title.clone(),
                        username,
                        about,
                        creator_id: caller_uid,
                        is_forum,
                        flags: 0,
                        photo_file_id: None,
                        slowmode_seconds: 0,
                        linked_chat_id: None,
                        pinned_message_id: None,
                        pts: 0,
                        last_message_id: MsgId(0),
                        last_message_date_ms: 0,
                        unread_count: 0,
                        account_generation: 1,
                        created_at_ms: now_ms,
                        updated_at_ms: now_ms,
                    };
                    let mut members = vec![ChatMemberRecord {
                        chat_id,
                        user_id: caller_uid,
                        role: "owner".into(),
                        permissions_bitmask: 0xFFFF,
                        admin_title: None,
                        admin_rights: 0xFFFF,
                        banned_rights: 0,
                        until_date_ms: None,
                        joined_at_ms: now_ms,
                        invited_by: None,
                        updated_at_ms: now_ms,
                    }];
                    if let Some(arr) = args.get("member_ids").and_then(|v| v.as_array()) {
                        for m in arr.iter().filter_map(|x| x.as_u64()).map(UserId) {
                            if m != caller_uid && !members.iter().any(|r| r.user_id == m) {
                                members.push(ChatMemberRecord {
                                    chat_id,
                                    user_id: m,
                                    role: "member".into(),
                                    permissions_bitmask: 0xFFFF,
                                    admin_title: None,
                                    admin_rights: 0,
                                    banned_rights: 0,
                                    until_date_ms: None,
                                    joined_at_ms: now_ms,
                                    invited_by: Some(caller_uid),
                                    updated_at_ms: now_ms,
                                });
                            }
                        }
                    }
                    self.db
                        .create_chat_with_members(&rec, &members)
                        .map_err(|e| e.to_string())?;
                    if let Some(hub) = self.gateway_hub.upgrade() {
                        for m in &members {
                            hub.subscribe_chat(chat_id.0, m.user_id);
                        }
                    }
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "title": title,
                        "chat_type": chat_type,
                        "is_forum": is_forum,
                    }))
                }
                "manage_topic" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let (_chat, is_admin) = self.check_membership(chat_id, caller_uid)?;
                    let action = args
                        .get("action")
                        .and_then(|v| v.as_str())
                        .unwrap_or("list")
                        .to_ascii_lowercase();
                    match action.as_str() {
                        "list" => {
                            let topics = self.db.list_forum_topics(chat_id).map_err(|e| e.to_string())?;
                            Ok(json!({ "chat_id": chat_id.0, "topics": topics }))
                        }
                        "create" => {
                            let title = args
                                .get("title")
                                .and_then(|v| v.as_str())
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .ok_or_else(|| "Non-empty 'title' is required".to_string())?
                                .to_string();
                            let existing = self.db.list_forum_topics(chat_id).map_err(|e| e.to_string())?;
                            let next_id = existing.iter().map(|t| t.topic_id.0).max().unwrap_or(0) + 1;
                            let topic = ForumTopicRecord {
                                chat_id,
                                topic_id: TopicId(next_id),
                                title: title.clone(),
                                icon_color: 0x6FB9F0,
                                icon_emoji_id: None,
                                creator_id: caller_uid,
                                is_pinned: args.get("is_pinned").and_then(|v| v.as_bool()).unwrap_or(false),
                                is_closed: false,
                                is_hidden: false,
                                last_message_id: None,
                                unread_count: 0,
                                created_at_ms: now_ms,
                            };
                            self.db.create_forum_topic(&topic).map_err(|e| e.to_string())?;
                            let dto = ForumTopicDto {
                                chat_id: topic.chat_id.0,
                                topic_id: topic.topic_id.0,
                                title: topic.title.clone(),
                                icon_color: topic.icon_color,
                                icon_emoji_id: topic.icon_emoji_id.map(|v| v as i64),
                                creator_id: topic.creator_id.0,
                                is_closed: topic.is_closed,
                                is_pinned: topic.is_pinned,
                                is_hidden: topic.is_hidden,
                                unread_count: topic.unread_count,
                                last_message_id: topic.last_message_id.map(|m| m.0),
                                created_at_ms: topic.created_at_ms,
                            };
                            self.broadcast_event(
                                chat_id,
                                UpdateEvent::ForumTopicChanged { chat_id, topic: dto.clone() },
                            );
                            Ok(json!({ "success": true, "chat_id": chat_id.0, "topic": dto }))
                        }
                        "update" => {
                            if !is_admin {
                                return Err("Admin rights required to update forum topics".into());
                            }
                            let topic_id = args
                                .get("topic_id")
                                .and_then(|v| v.as_i64())
                                .filter(|&t| t > 0)
                                .ok_or_else(|| "Positive 'topic_id' is required".to_string())?;
                            let updated = self
                                .db
                                .update_forum_topic(
                                    chat_id,
                                    TopicId(topic_id),
                                    args.get("title").and_then(|v| v.as_str()),
                                    None,
                                    None,
                                    args.get("is_pinned").and_then(|v| v.as_bool()),
                                    args.get("is_closed").and_then(|v| v.as_bool()),
                                    args.get("is_hidden").and_then(|v| v.as_bool()),
                                )
                                .map_err(|e| e.to_string())?;
                            let dto = ForumTopicDto {
                                chat_id: updated.chat_id.0,
                                topic_id: updated.topic_id.0,
                                title: updated.title.clone(),
                                icon_color: updated.icon_color,
                                icon_emoji_id: updated.icon_emoji_id.map(|v| v as i64),
                                creator_id: updated.creator_id.0,
                                is_closed: updated.is_closed,
                                is_pinned: updated.is_pinned,
                                is_hidden: updated.is_hidden,
                                unread_count: updated.unread_count,
                                last_message_id: updated.last_message_id.map(|m| m.0),
                                created_at_ms: updated.created_at_ms,
                            };
                            self.broadcast_event(
                                chat_id,
                                UpdateEvent::ForumTopicChanged { chat_id, topic: dto.clone() },
                            );
                            Ok(json!({ "success": true, "chat_id": chat_id.0, "topic": dto }))
                        }
                        "delete" => {
                            if !is_admin {
                                return Err("Admin rights required to delete forum topics".into());
                            }
                            let topic_id = args
                                .get("topic_id")
                                .and_then(|v| v.as_i64())
                                .filter(|&t| t > 0)
                                .ok_or_else(|| "Positive 'topic_id' is required".to_string())?;
                            let deleted = self
                                .db
                                .delete_forum_topic(chat_id, TopicId(topic_id))
                                .map_err(|e| e.to_string())?;
                            Ok(json!({ "success": deleted, "chat_id": chat_id.0, "topic_id": topic_id }))
                        }
                        other => Err(format!("Unsupported manage_topic action '{other}'")),
                    }
                }
                "list_members" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let _ = self.check_membership(chat_id, caller_uid)?;
                    let members = self
                        .db
                        .get_chat_member_details(chat_id)
                        .map_err(|e| e.to_string())?;
                    Ok(json!({ "chat_id": chat_id.0, "count": members.len(), "members": members }))
                }
                "kick_member" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let (_chat, is_admin) = self.check_membership(chat_id, caller_uid)?;
                    if !is_admin {
                        return Err("Admin rights required to kick members".into());
                    }
                    let target_uid = args
                        .get("user_id")
                        .and_then(|v| v.as_u64())
                        .filter(|&u| u > 0)
                        .ok_or_else(|| "Positive 'user_id' is required".to_string())?;
                    let removed = self
                        .db
                        .remove_chat_member(chat_id, UserId(target_uid))
                        .map_err(|e| e.to_string())?;
                    if removed {
                        if let Some(hub) = self.gateway_hub.upgrade() {
                            hub.unsubscribe_chat(chat_id.0, UserId(target_uid));
                        }
                    }
                    Ok(json!({
                        "success": removed,
                        "chat_id": chat_id.0,
                        "kicked_user_id": target_uid,
                    }))
                }
                "create_poll" => {
                    let chat_id = Self::require_chat_id(args, &caller)?;
                    let _ = self.check_send_permitted(chat_id, caller_uid)?;
                    let question = args
                        .get("question")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "Non-empty 'question' is required".to_string())?;
                    let options: Vec<String> = args
                        .get("options")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str().map(str::trim).filter(|s| !s.is_empty()).map(ToString::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    let is_anonymous = args.get("is_anonymous").and_then(|v| v.as_bool()).unwrap_or(true);
                    let multiple_choice = args.get("multiple_choice").and_then(|v| v.as_bool()).unwrap_or(false);
                    let is_quiz = args.get("is_quiz").and_then(|v| v.as_bool()).unwrap_or(false);
                    let correct_option_id = args.get("correct_option_id").and_then(|v| v.as_u64()).map(|n| n as u8);
                    let topic_id = args.get("topic_id").and_then(|v| v.as_i64());

                    let (msg_id, pts, poll, item) = self
                        .db
                        .create_poll_atomic(
                            chat_id,
                            caller_uid,
                            question,
                            &options,
                            is_anonymous,
                            multiple_choice,
                            is_quiz,
                            correct_option_id,
                            None,
                            topic_id,
                        )
                        .map_err(|e| e.to_string())?;
                    self.broadcast_pts(
                        chat_id,
                        PtsUpdate::new(chat_id, pts, 1, UpdateEvent::NewMessage(item), now_ms),
                    );
                    let poll_id_num = poll.poll_id;
                    Ok(json!({
                        "success": true,
                        "chat_id": chat_id.0,
                        "message_id": msg_id.0,
                        "poll_id": poll_id_num,
                        "pts": pts,
                        "poll": poll,
                    }))
                }
                "vote_poll" => {
                    let poll_id = args
                        .get("poll_id")
                        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                        .ok_or_else(|| "Numeric 'poll_id' is required".to_string())?;
                    let (poll_chat_id, _) = self
                        .db
                        .get_poll_for_user(poll_id, caller_uid)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("Poll {poll_id} not found"))?;
                    if let Some(pinned) = caller.chat_id {
                        Self::reject_cross_chat_arg(args, pinned)?;
                        if pinned != poll_chat_id.0 {
                            return Err(format!(
                                "Cross-chat target mismatch: poll {poll_id} belongs to chat {}, not pinned chat {pinned}",
                                poll_chat_id.0
                            ));
                        }
                    }
                    if let Some(explicit_chat) = args.get("chat_id").and_then(|v| v.as_i64()) {
                        if explicit_chat != poll_chat_id.0 {
                            return Err(format!(
                                "Poll {poll_id} belongs to chat {}, not chat {}",
                                poll_chat_id.0, explicit_chat
                            ));
                        }
                    }
                    let is_member = self
                        .db
                        .is_user_in_chat(poll_chat_id, caller_uid)
                        .map_err(|e| e.to_string())?;
                    if !is_member {
                        return Err(format!(
                            "User {} is not an active member of chat {}",
                            caller_uid.0, poll_chat_id.0
                        ));
                    }
                    let _ = self.check_membership(poll_chat_id, caller_uid)?;
                    let mut option_ids: Vec<u8> = args
                        .get("option_ids")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|x| x.as_u64().map(|n| n as u8)).collect())
                        .unwrap_or_default();
                    if option_ids.is_empty() {
                        if let Some(idx) = args.get("option_index").and_then(|v| v.as_u64()) {
                            option_ids.push(idx as u8);
                        }
                    }
                    let (cid, mid, pts, poll) = self
                        .db
                        .vote_poll_atomic(poll_id, caller_uid, &option_ids)
                        .map_err(|e| e.to_string())?;
                    self.broadcast_pts(
                        cid,
                        PtsUpdate::new(
                            cid,
                            pts,
                            1,
                            UpdateEvent::PollUpdated {
                                chat_id: cid,
                                message_id: mid,
                                poll: poll.clone(),
                            },
                            now_ms,
                        ),
                    );
                    Ok(json!({
                        "success": true,
                        "chat_id": cid.0,
                        "message_id": mid.0,
                        "pts": pts,
                        "poll": poll,
                    }))
                }
                "launch_miniapp" => {
                    let bot_username = args
                        .get("bot_username")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "'bot_username' is required".to_string())?;
                    let short_name = args
                        .get("short_name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "'short_name' is required".to_string())?;
                    let app = self
                        .db
                        .get_mini_app(bot_username, short_name)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("MiniApp '{bot_username}/{short_name}' not found"))?;
                    let chat_id = if let Some(pinned) = caller.chat_id {
                        Self::reject_cross_chat_arg(args, pinned)?;
                        pinned
                    } else {
                        args.get("chat_id").and_then(|v| v.as_i64()).unwrap_or(0)
                    };
                    if chat_id > 0 {
                        let _ = self.check_membership(ChatId(chat_id), caller_uid)?;
                    }
                    let start_param = args.get("start_param").and_then(|v| v.as_str()).unwrap_or("");
                    let query_id = format!("ma_{}", Uuid::new_v4().simple());
                    let session = self
                        .db
                        .create_mini_app_session(
                            &query_id,
                            caller_uid.0 as i64,
                            &app.app_id,
                            app.bot_user_id,
                            chat_id,
                            "inline",
                            start_param,
                            "",
                            Utc::now().timestamp(),
                            "{}",
                        )
                        .map_err(|e| e.to_string())?;
                    let ui_schema: MiniAppUiSchema =
                        serde_json::from_str(&app.ui_schema_json).unwrap_or_default();
                    let peer_types: Vec<String> =
                        serde_json::from_str(&app.peer_types_json).unwrap_or_default();
                    let manifest = MiniAppManifestDto {
                        app_id: app.app_id.clone(),
                        bot_username: app.bot_username.clone(),
                        short_name: app.short_name.clone(),
                        title: app.title.clone(),
                        description: app.description.clone(),
                        category: app.category.clone(),
                        icon_emoji: app.icon_emoji.clone(),
                        web_url: app.web_url.clone(),
                        version: "1.0.0".to_string(),
                        peer_types,
                        requested_permissions: Vec::new(),
                        ui_schema,
                    };
                    let agent_manifest = manifest.to_agent_manifest();
                    Ok(json!({
                        "success": true,
                        "app_id": app.app_id,
                        "query_id": session.query_id,
                        "session": session,
                        "manifest": manifest,
                        "agent_manifest": agent_manifest,
                    }))
                }
                "invoke_miniapp_action" => {
                    let bot_username = args
                        .get("bot_username")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "'bot_username' is required".to_string())?;
                    let short_name = args
                        .get("short_name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "'short_name' is required".to_string())?;
                    let action_type = args
                        .get("action_type")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "'action_type' is required".to_string())?;
                    let app = self
                        .db
                        .get_mini_app(bot_username, short_name)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("MiniApp '{bot_username}/{short_name}' not found"))?;
                    let explicit_chat = args.get("chat_id").and_then(|v| v.as_i64()).filter(|&c| c > 0);
                    let target_chat = if let Some(pinned) = caller.chat_id {
                        Self::reject_cross_chat_arg(args, pinned)?;
                        Some(pinned).filter(|&c| c > 0)
                    } else {
                        explicit_chat
                    };
                    if let Some(chat_id) = target_chat {
                        let _ = self.check_membership(ChatId(chat_id), caller_uid)?;
                    }
                    if let Some(qid) = args.get("query_id").and_then(|v| v.as_str()) {
                        let session = self
                            .db
                            .get_mini_app_session(qid)
                            .map_err(|e| e.to_string())?
                            .ok_or_else(|| format!("MiniApp session '{qid}' not found"))?;
                        if session.user_id != caller_uid.0 as i64 {
                            return Err("Permission denied: MiniApp session belongs to another user".into());
                        }
                        if session.app_id != app.app_id {
                            return Err("MiniApp session does not match target MiniApp".into());
                        }
                        if let Some(pinned) = caller.chat_id {
                            if session.chat_id != pinned {
                                return Err(format!(
                                    "Cross-chat target mismatch: MiniApp session '{qid}' belongs to chat {}, not pinned chat {pinned}",
                                    session.chat_id
                                ));
                            }
                        } else if let Some(cid) = explicit_chat {
                            if session.chat_id != cid {
                                return Err(format!(
                                    "MiniApp session '{qid}' belongs to chat {}, not chat {cid}",
                                    session.chat_id
                                ));
                            }
                        }
                        if session.chat_id > 0 {
                            let _ = self.check_membership(ChatId(session.chat_id), caller_uid)?;
                        }
                    }

                    let ui_schema: MiniAppUiSchema =
                        serde_json::from_str(&app.ui_schema_json).unwrap_or_default();
                    let peer_types: Vec<String> =
                        serde_json::from_str(&app.peer_types_json).unwrap_or_default();
                    let manifest_dto = MiniAppManifestDto {
                        app_id: app.app_id.clone(),
                        bot_username: app.bot_username.clone(),
                        short_name: app.short_name.clone(),
                        title: app.title.clone(),
                        description: app.description.clone(),
                        category: app.category.clone(),
                        icon_emoji: app.icon_emoji.clone(),
                        web_url: app.web_url.clone(),
                        version: "1.0.0".to_string(),
                        peer_types,
                        requested_permissions: Vec::new(),
                        ui_schema,
                    };
                    let agent_manifest = manifest_dto.to_agent_manifest();
                    let mut action_params = serde_json::Map::new();
                    if let Some(v) = args.get("value").filter(|v| !v.is_null()) {
                        action_params.insert("value".to_string(), v.clone());
                    }
                    if let Some(d) = args.get("data").filter(|v| !v.is_null()) {
                        action_params.insert("data".to_string(), d.clone());
                    }
                    if let Some(w) = args.get("widget_id").filter(|v| !v.is_null()) {
                        action_params.insert("widget_id".to_string(), w.clone());
                    }
                    if let Some(obj) = args.get("params").and_then(|v| v.as_object()) {
                        for (k, v) in obj {
                            action_params.insert(k.clone(), v.clone());
                        }
                    }
                    let approved = args.get("approved").and_then(|v| v.as_bool()).unwrap_or(false);
                    let execution = agent_manifest.execute_action(
                        action_type,
                        &Value::Object(action_params.clone()),
                        approved,
                    )?;

                    let mapped_action = if let Some(wid) = action_type.strip_prefix("widget:") {
                        ("widget_event".to_string(), Some(wid.to_string()))
                    } else if action_type == "main_button" {
                        ("main_button_click".to_string(), None)
                    } else if action_type == "secondary_button" {
                        ("secondary_button_click".to_string(), None)
                    } else {
                        (
                            action_type.to_string(),
                            args.get("widget_id")
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                        )
                    };
                    let mut headless_args = serde_json::Map::new();
                    if let Some(wid) = &mapped_action.1 {
                        headless_args.insert("widget_id".to_string(), json!(wid));
                    }
                    if let Some(v) = args.get("value").filter(|v| !v.is_null()) {
                        headless_args.insert("value".to_string(), v.clone());
                    }
                    if let Some(d) = args.get("data").filter(|v| !v.is_null()) {
                        headless_args.insert("data".to_string(), d.clone());
                    }
                    for (k, v) in action_params {
                        headless_args.entry(k).or_insert(v);
                    }
                    let reduced = self.execute_headless_webmcp_tool(
                        &app,
                        caller_uid,
                        target_chat.unwrap_or(0),
                        &mapped_action.0,
                        &Value::Object(headless_args),
                    )?;

                    let mut next_state = reduced.get("state").cloned().unwrap_or_else(|| json!({}));
                    if let Some(obj) = next_state.as_object_mut() {
                        obj.insert("last_action".to_string(), json!(action_type));
                        obj.insert("invoked_by".to_string(), json!(caller_uid.0));
                        obj.insert("invoked_at_ms".to_string(), json!(now_ms));
                    }
                    if let Some(qid) = args.get("query_id").and_then(|v| v.as_str()) {
                        let state_str =
                            serde_json::to_string(&next_state).unwrap_or_else(|_| "{}".into());
                        self.db
                            .update_mini_app_session_state(qid, &state_str)
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(json!({
                        "success": true,
                        "app_id": app.app_id,
                        "action_type": action_type,
                        "execution": execution,
                        "headless_result": reduced,
                        "state": next_state,
                    }))
                }
                "webmcp_list_miniapps" => {
                    let uid = caller_uid.0 as i64;
                    let mut seen = std::collections::HashSet::new();
                    let mut items = Vec::new();
                    let has_live_client = self
                        .webmcp_broker
                        .upgrade()
                        .is_some_and(|b| b.has_connected_client(caller_uid));
                    if let Some(bid) = caller.bot_id.filter(|&id| id > 0) {
                        if let Ok(companion_apps) = self.db.list_mini_apps_for_bot(bid) {
                            for app in companion_apps {
                                let k = format!("{}/{}", app.bot_username, app.short_name);
                                if seen.insert(k) {
                                    items.push(json!({
                                        "bot_username": app.bot_username,
                                        "short_name": app.short_name,
                                        "title": app.title,
                                        "description": app.description,
                                        "companion": true,
                                        "has_live_client": has_live_client,
                                    }));
                                }
                            }
                        }
                    }
                    for (app, _, _) in self
                        .db
                        .list_user_installed_mini_apps(uid)
                        .unwrap_or_default()
                    {
                        let k = format!("{}/{}", app.bot_username, app.short_name);
                        if seen.insert(k) {
                            items.push(json!({
                                "bot_username": app.bot_username,
                                "short_name": app.short_name,
                                "title": app.title,
                                "description": app.description,
                                "companion": false,
                                "has_live_client": has_live_client,
                            }));
                        }
                    }
                    for app in self.db.search_published_mini_apps("", None).unwrap_or_default() {
                        let k = format!("{}/{}", app.bot_username, app.short_name);
                        if seen.insert(k) {
                            items.push(json!({
                                "bot_username": app.bot_username,
                                "short_name": app.short_name,
                                "title": app.title,
                                "description": app.description,
                                "companion": false,
                                "has_live_client": has_live_client,
                            }));
                        }
                    }
                    Ok(json!({
                        "miniapps": items,
                        "count": items.len(),
                        "has_live_client": has_live_client,
                    }))
                }
                "webmcp_discover" => {
                    let (app, target_chat) = self.resolve_target_miniapp(args, &caller, caller_uid)?;
                    let chat_id = target_chat.map(|c| c.0).unwrap_or(0);
                    if let Some(broker) = self.webmcp_broker.upgrade() {
                        let cmd = MiniAppWebMcpCommand {
                            request_id: format!("wmcp_{}", Uuid::new_v4().simple()),
                            bot_username: app.bot_username.clone(),
                            short_name: app.short_name.clone(),
                            chat_id,
                            bot_user_id: app.bot_user_id,
                            app_title: app.title.clone(),
                            operation: "discover".to_string(),
                            tool_name: None,
                            arguments: json!({}),
                            status_message: Some(format!("正在发现 {} 的 WebMCP 工具...", app.title)),
                        };
                        if let Some(resp) = broker
                            .dispatch_to_client(caller_uid, cmd, DEFAULT_WEBMCP_RPC_TIMEOUT)
                            .await
                            .map_err(|rpc_err| {
                                format!(
                                    "小程序「{}」桌面窗口未在超时时间内响应（{rpc_err}），请确认用户已打开小程序且页面加载正常后重试",
                                    app.title
                                )
                            })?
                        {
                            if !resp.ok {
                                return Err(resp.error.unwrap_or_else(|| "WebMCP discover failed".into()));
                            }
                            let page_error = extract_webmcp_page_error(resp.state_summary.as_ref());
                            let mut payload = json!({
                                "success": true,
                                "mode": "live_webmcp",
                                "webview_connected": true,
                                "bot_username": app.bot_username,
                                "short_name": app.short_name,
                                "app_title": app.title,
                                "tools": resp.tools,
                                "state_summary": resp.state_summary,
                            });
                            if let Some(err) = page_error {
                                if let Some(obj) = payload.as_object_mut() {
                                    obj.insert(
                                        "warning".to_string(),
                                        json!(format!(
                                            "Mini App page reported an error: {err}. Do NOT claim Mini App actions succeeded; report this exact error to the user."
                                        )),
                                    );
                                    obj.insert("page_error".to_string(), json!(err));
                                }
                            }
                            return Ok(payload);
                        }
                    }
                    let catalog = Self::headless_webmcp_catalog(&app);
                    let state_res = self.execute_headless_webmcp_tool(
                        &app,
                        caller_uid,
                        chat_id,
                        "get_state",
                        &json!({}),
                    )?;
                    Ok(json!({
                        "success": true,
                        "mode": "headless_fallback",
                        "webview_connected": false,
                        "notice": format!(
                            "当前没有已连接的桌面小程序 WebView，仅列出通用无头工具。如需执行「{}」页面专属工具，请提示用户在桌面端打开「{}」小程序。",
                            app.title, app.title
                        ),
                        "bot_username": app.bot_username,
                        "short_name": app.short_name,
                        "app_title": app.title,
                        "tools": catalog,
                        "state_summary": state_res,
                    }))
                }
                "webmcp_call_tool" => {
                    let tool = args
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "'tool_name' is required".to_string())?;
                    let tool_args = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
                    let status_msg = args
                        .get("status_message")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    let (app, target_chat) = self.resolve_target_miniapp(args, &caller, caller_uid)?;
                    let chat_id = target_chat.map(|c| c.0).unwrap_or(0);

                    if let Some(broker) = self.webmcp_broker.upgrade() {
                        let cmd = MiniAppWebMcpCommand {
                            request_id: format!("wmcp_{}", Uuid::new_v4().simple()),
                            bot_username: app.bot_username.clone(),
                            short_name: app.short_name.clone(),
                            chat_id,
                            bot_user_id: app.bot_user_id,
                            app_title: app.title.clone(),
                            operation: "call_tool".to_string(),
                            tool_name: Some(tool.to_string()),
                            arguments: tool_args.clone(),
                            status_message: status_msg,
                        };
                        if let Some(resp) = broker
                            .dispatch_to_client(caller_uid, cmd, DEFAULT_WEBMCP_RPC_TIMEOUT)
                            .await
                            .map_err(|rpc_err| {
                                format!(
                                    "小程序「{}」桌面窗口未在超时时间内响应（{rpc_err}），请确认用户已打开小程序且页面加载正常后重试",
                                    app.title
                                )
                            })?
                        {
                            let page_error = extract_webmcp_page_error(resp.state_summary.as_ref());
                            if !resp.ok {
                                let registered_tools: Vec<String> = resp
                                    .tools
                                    .iter()
                                    .map(|t| t.name.clone())
                                    .filter(|n| !n.trim().is_empty())
                                    .collect();
                                let mut err_msg = resp
                                    .error
                                    .unwrap_or_else(|| format!("WebMCP tool '{tool}' failed"));
                                if let Some(page_err) = page_error {
                                    err_msg.push_str(&format!("（小程序页面异常: {page_err}）"));
                                }
                                if !registered_tools.is_empty() {
                                    err_msg.push_str(&format!(
                                        " | 当前已注册工具: [{}]",
                                        registered_tools.join(", ")
                                    ));
                                } else {
                                    err_msg.push_str(&format!(
                                        " | 请先在桌面端打开或刷新「{}」小程序窗口后重试，严禁声称操作已成功",
                                        app.title
                                    ));
                                }
                                return Err(err_msg);
                            }
                            let mut payload = json!({
                                "success": true,
                                "mode": "live_webmcp",
                                "bot_username": app.bot_username,
                                "short_name": app.short_name,
                                "tool_name": tool,
                                "result": resp.result,
                                "state_summary": resp.state_summary,
                            });
                            if let Some(err) = page_error {
                                if let Some(obj) = payload.as_object_mut() {
                                    obj.insert(
                                        "warning".to_string(),
                                        json!(format!(
                                            "Mini App page reported an error: {err}. Do NOT claim Mini App actions succeeded; report this exact error to the user."
                                        )),
                                    );
                                    obj.insert("page_error".to_string(), json!(err));
                                }
                            }
                            return Ok(payload);
                        }
                    }

                    const HEADLESS_SUPPORTED_TOOLS: [&str; 10] = [
                        "get_state",
                        "page_inspect",
                        "page_click",
                        "page_fill",
                        "page_press_main_button",
                        "page_press_secondary_button",
                        "widget_event",
                        "main_button_click",
                        "secondary_button_click",
                        "send_data",
                    ];
                    if !HEADLESS_SUPPORTED_TOOLS.contains(&tool) {
                        return Err(format!(
                            "未注册的 WebMCP 工具: {tool}（当前没有已连接的桌面小程序 WebView，请在桌面端打开「{}」小程序后重试；无头模式仅支持通用工具: {}）",
                            app.title,
                            HEADLESS_SUPPORTED_TOOLS.join(", ")
                        ));
                    }

                    let fallback_res = self.execute_headless_webmcp_tool(
                        &app,
                        caller_uid,
                        chat_id,
                        tool,
                        &tool_args,
                    )?;
                    Ok(json!({
                        "success": true,
                        "mode": "headless_fallback",
                        "bot_username": app.bot_username,
                        "short_name": app.short_name,
                        "tool_name": tool,
                        "result": fallback_res,
                    }))
                }
                "miniapp_request_takeover" => {
                    let reason = args
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| "'reason' is required".to_string())?;
                    let (app, target_chat) = self.resolve_target_miniapp(args, &caller, caller_uid)?;
                    let chat_id = target_chat.unwrap_or(ChatId(0));
                    let broker = self
                        .webmcp_broker
                        .upgrade()
                        .ok_or_else(|| "WebMCP broker is unavailable".to_string())?;
                    let evt = broker
                        .request_takeover_and_wait(
                            caller_uid,
                            &app.bot_username,
                            &app.short_name,
                            chat_id,
                            UserId(app.bot_user_id.max(0) as u64),
                            &app.title,
                            reason,
                            DEFAULT_TAKEOVER_WAIT_TIMEOUT,
                        )
                        .await?;
                    Ok(json!({
                        "success": true,
                        "resumed": true,
                        "bot_username": app.bot_username,
                        "short_name": app.short_name,
                        "mode": evt.mode,
                        "user_note": evt.user_note,
                        "state_snapshot": evt.state_snapshot,
                    }))
                }
                other => Err(format!("Unsupported PlatformIm tool '{other}'")),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::AppState;
    use crate::ServerConfig;
    use gpui_telegram_agent::hermes::toolsets::{HermesToolExecutor, ToolsetPreset};
    use std::sync::atomic::AtomicBool;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_server_platform_im_bridge_executes_all_14_tools_via_hermes() {
        let dir = TempDir::new().expect("create tempdir");
        let db_path = dir.path().join("im_bridge_test.db");
        let db = Arc::new(ServerDatabase::open(&db_path).expect("open db"));
        let mut cfg = ServerConfig::default();
        cfg.media_storage_path = dir.path().join("media").display().to_string();
        let state = Arc::new(AppState::with_db(cfg, db.clone()));

        let (alice, _, _) = state
            .session_manager
            .register("alice_im", "Pass123456!", "Alice", None, "CLI")
            .unwrap();
        let (bob, _, _) = state
            .session_manager
            .register("bob_im", "Pass123456!", "Bob", None, "CLI")
            .unwrap();
        let now_ms = Utc::now().timestamp_millis();
        for (uid, uname, fname) in [
            (alice.user_id, "alice_im", "Alice"),
            (bob.user_id, "bob_im", "Bob"),
        ] {
            db.create_user(&crate::db::schema::UserRecord {
                user_id: uid,
                username: Some(uname.to_string()),
                phone: None,
                first_name: fname.to_string(),
                last_name: None,
                bio: None,
                password_hash: None,
                is_bot: false,
                is_verified: false,
                is_support: false,
                is_scam: false,
                is_fake: false,
                profile_photo_file_id: None,
                account_generation: 1,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            })
            .expect("create_user in db");
        }
        let alice_uid_str = alice.user_id.0.to_string();

        // Register bot first and seed a published MiniApp under @pomodoro_test_bot/timer
        let bot = db
            .create_registered_bot(
                alice.user_id.0 as i64,
                "pomodoro_test_bot",
                "Pomodoro Bot",
                "Pomodoro bot",
                "Focus",
                "helpful",
                "",
                "",
                "[]",
                false,
                true,
            )
            .expect("register bot");
        let schema_json = crate::db::miniapp_store::default_pomodoro_ui_schema().to_string();
        db.create_or_upsert_mini_app_with_options(
            bot.bot_user_id,
            "pomodoro_test_bot",
            "timer",
            "Pomodoro Timer",
            "Focus timer",
            "productivity",
            "🍅",
            None,
            &schema_json,
            true,
            true,
            false,
            "",
            "",
            None,
            true,
        )
        .expect("seed miniapp");

        let ctx = state.fabushi_core().build_tool_context_with_user(
            "sess_platform_im_test",
            None,
            None,
            Some(&alice_uid_str),
            1,
            Some(ToolsetPreset::Full),
            0,
            Arc::new(AtomicBool::new(false)),
        );
        assert!(
            ctx.platform_im_bridge.is_some(),
            "AppState::with_db must wire ServerPlatformImBridge into HermesEngine"
        );

        // 1. create_channel (forum group so topic management works too)
        let r_create = HermesToolExecutor::execute(
            "create_channel",
            json!({
                "title": "M4 Platform IM War Room",
                "chat_type": "supergroup",
                "is_forum": true,
                "about": "Integration test chat",
                "member_ids": [bob.user_id.0]
            }),
            &ctx,
        )
        .await;
        assert!(r_create.success, "create_channel failed: {:?}", r_create.error);
        let v_create: Value = serde_json::from_str(&r_create.output).unwrap();
        let chat_id = v_create["chat_id"].as_i64().unwrap();
        assert!(chat_id > 0);

        // 2. list_members
        let r_members =
            HermesToolExecutor::execute("list_members", json!({ "chat_id": chat_id }), &ctx).await;
        assert!(r_members.success, "list_members failed: {:?}", r_members.error);
        let v_members: Value = serde_json::from_str(&r_members.output).unwrap();
        assert_eq!(v_members["count"].as_u64(), Some(2));

        // 3. manage_topic (create + list)
        let r_topic = HermesToolExecutor::execute(
            "manage_topic",
            json!({ "chat_id": chat_id, "action": "create", "title": "Hybrid Search" }),
            &ctx,
        )
        .await;
        assert!(r_topic.success, "manage_topic failed: {:?}", r_topic.error);
        let v_topic: Value = serde_json::from_str(&r_topic.output).unwrap();
        let topic_id = v_topic["topic"]["topic_id"].as_i64().unwrap();

        // 4. send_message (including GenerativeCard or text)
        let r_send = HermesToolExecutor::execute(
            "send_message",
            json!({
                "chat_id": chat_id,
                "topic_id": topic_id,
                "text": "Platform IM native tool check alpha99"
            }),
            &ctx,
        )
        .await;
        assert!(r_send.success, "send_message failed: {:?}", r_send.error);
        let v_send: Value = serde_json::from_str(&r_send.output).unwrap();
        let msg_id = v_send["message_id"].as_i64().unwrap();

        // 5. edit_message
        let r_edit = HermesToolExecutor::execute(
            "edit_message",
            json!({
                "chat_id": chat_id,
                "message_id": msg_id,
                "text": "Platform IM native tool check beta88"
            }),
            &ctx,
        )
        .await;
        assert!(r_edit.success, "edit_message failed: {:?}", r_edit.error);

        // 6. pin_message
        let r_pin = HermesToolExecutor::execute(
            "pin_message",
            json!({ "chat_id": chat_id, "message_id": msg_id }),
            &ctx,
        )
        .await;
        assert!(r_pin.success, "pin_message failed: {:?}", r_pin.error);

        // 7. add_reaction
        let r_react = HermesToolExecutor::execute(
            "add_reaction",
            json!({ "chat_id": chat_id, "message_id": msg_id, "emoji": "🚀", "action": "add" }),
            &ctx,
        )
        .await;
        assert!(r_react.success, "add_reaction failed: {:?}", r_react.error);

        // 8. search_messages (powered by HybridMemoryEngine / TantivyIndexer with ACL)
        let r_search = HermesToolExecutor::execute(
            "search_messages",
            json!({ "chat_id": chat_id, "query": "beta88" }),
            &ctx,
        )
        .await;
        assert!(r_search.success, "search_messages failed: {:?}", r_search.error);
        let v_search: Value = serde_json::from_str(&r_search.output).unwrap();
        assert!(v_search["count"].as_u64().unwrap_or(0) >= 1);

        // 9. create_poll & 10. vote_poll
        let r_poll = HermesToolExecutor::execute(
            "create_poll",
            json!({
                "chat_id": chat_id,
                "question": "Ship M4?",
                "options": ["Yes", "Now"]
            }),
            &ctx,
        )
        .await;
        assert!(r_poll.success, "create_poll failed: {:?}", r_poll.error);
        let v_poll: Value = serde_json::from_str(&r_poll.output).unwrap();
        let poll_id = v_poll["poll_id"].as_i64().unwrap();

        let r_vote = HermesToolExecutor::execute(
            "vote_poll",
            json!({ "chat_id": chat_id, "poll_id": poll_id, "option_ids": [1] }),
            &ctx,
        )
        .await;
        assert!(r_vote.success, "vote_poll failed: {:?}", r_vote.error);

        // 11. launch_miniapp & 12. invoke_miniapp_action
        let r_launch = HermesToolExecutor::execute(
            "launch_miniapp",
            json!({ "bot_username": "pomodoro_test_bot", "short_name": "timer", "chat_id": chat_id }),
            &ctx,
        )
        .await;
        assert!(r_launch.success, "launch_miniapp failed: {:?}", r_launch.error);
        let v_launch: Value = serde_json::from_str(&r_launch.output).unwrap();
        let query_id = v_launch["query_id"].as_str().unwrap().to_string();
        assert!(v_launch["agent_manifest"]["actions"].is_array());

        let r_invoke = HermesToolExecutor::execute(
            "invoke_miniapp_action",
            json!({
                "bot_username": "pomodoro_test_bot",
                "short_name": "timer",
                "action_type": "widget:completed_sessions",
                "query_id": query_id,
                "value": 25
            }),
            &ctx,
        )
        .await;
        assert!(r_invoke.success, "invoke_miniapp_action failed: {:?}", r_invoke.error);

        // Undeclared MiniApp action must be rejected
        let r_bad_invoke = HermesToolExecutor::execute(
            "invoke_miniapp_action",
            json!({
                "bot_username": "pomodoro_test_bot",
                "short_name": "timer",
                "action_type": "undeclared_action_xyz",
                "query_id": query_id,
            }),
            &ctx,
        )
        .await;
        assert!(!r_bad_invoke.success, "undeclared action_type must be rejected");

        // 13. kick_member
        let r_kick = HermesToolExecutor::execute(
            "kick_member",
            json!({ "chat_id": chat_id, "user_id": bob.user_id.0 }),
            &ctx,
        )
        .await;
        assert!(r_kick.success, "kick_member failed: {:?}", r_kick.error);

        // 14. delete_message
        let r_del = HermesToolExecutor::execute(
            "delete_message",
            json!({ "chat_id": chat_id, "message_id": msg_id, "revoke": true }),
            &ctx,
        )
        .await;
        assert!(r_del.success, "delete_message failed: {:?}", r_del.error);
    }

    #[tokio::test]
    async fn test_platform_im_vote_poll_acl_miniapp_session_and_cross_chat_memory() {
        let dir = TempDir::new().expect("create tempdir");
        let db_path = dir.path().join("im_bridge_acl_test.db");
        let db = Arc::new(ServerDatabase::open(&db_path).expect("open db"));
        let mut cfg = ServerConfig::default();
        cfg.media_storage_path = dir.path().join("media").display().to_string();
        let state = Arc::new(AppState::with_db(cfg, db.clone()));

        let (alice, _, _) = state
            .session_manager
            .register("alice_acl", "Pass123456!", "Alice", None, "CLI")
            .unwrap();
        let (eve, _, _) = state
            .session_manager
            .register("eve_acl", "Pass123456!", "Eve", None, "CLI")
            .unwrap();
        let now_ms = Utc::now().timestamp_millis();
        for (uid, uname, fname) in [
            (alice.user_id, "alice_acl", "Alice"),
            (eve.user_id, "eve_acl", "Eve"),
        ] {
            db.create_user(&crate::db::schema::UserRecord {
                user_id: uid,
                username: Some(uname.to_string()),
                phone: None,
                first_name: fname.to_string(),
                last_name: None,
                bio: None,
                password_hash: None,
                is_bot: false,
                is_verified: false,
                is_support: false,
                is_scam: false,
                is_fake: false,
                profile_photo_file_id: None,
                account_generation: 1,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            })
            .unwrap();
        }

        let bot = db
            .create_registered_bot(
                alice.user_id.0 as i64,
                "acl_pomodoro_bot",
                "Pomodoro Bot",
                "Pomodoro bot",
                "Focus",
                "helpful",
                "",
                "",
                "[]",
                false,
                true,
            )
            .unwrap();
        let schema_json = crate::db::miniapp_store::default_pomodoro_ui_schema().to_string();
        db.create_or_upsert_mini_app_with_options(
            bot.bot_user_id,
            "acl_pomodoro_bot",
            "timer",
            "Pomodoro Timer",
            "Focus timer",
            "productivity",
            "🍅",
            None,
            &schema_json,
            true,
            true,
            false,
            "",
            "",
            None,
            true,
        )
        .unwrap();

        let ctx_alice = state.fabushi_core().build_tool_context_with_user(
            "sess_alice_acl",
            None,
            None,
            Some(&alice.user_id.0.to_string()),
            1,
            Some(ToolsetPreset::Full),
            0,
            Arc::new(AtomicBool::new(false)),
        );
        let ctx_eve = state.fabushi_core().build_tool_context_with_user(
            "sess_eve_acl",
            None,
            None,
            Some(&eve.user_id.0.to_string()),
            1,
            Some(ToolsetPreset::Full),
            0,
            Arc::new(AtomicBool::new(false)),
        );

        let r_chat1 = HermesToolExecutor::execute(
            "create_channel",
            json!({ "title": "Alice Secret 1", "chat_type": "group" }),
            &ctx_alice,
        )
        .await;
        let chat1 = serde_json::from_str::<Value>(&r_chat1.output).unwrap()["chat_id"]
            .as_i64()
            .unwrap();
        let r_chat2 = HermesToolExecutor::execute(
            "create_channel",
            json!({ "title": "Alice Secret 2", "chat_type": "group" }),
            &ctx_alice,
        )
        .await;
        let chat2 = serde_json::from_str::<Value>(&r_chat2.output).unwrap()["chat_id"]
            .as_i64()
            .unwrap();

        // Non-member Eve must not be able to vote in Alice's poll
        let r_poll = HermesToolExecutor::execute(
            "create_poll",
            json!({ "chat_id": chat1, "question": "Secret?", "options": ["Yes", "No"] }),
            &ctx_alice,
        )
        .await;
        let poll_id = serde_json::from_str::<Value>(&r_poll.output).unwrap()["poll_id"]
            .as_i64()
            .unwrap();
        let r_eve_vote = HermesToolExecutor::execute(
            "vote_poll",
            json!({ "poll_id": poll_id, "option_ids": [0] }),
            &ctx_eve,
        )
        .await;
        assert!(!r_eve_vote.success, "non-member vote_poll must be rejected");

        // Cross-user MiniApp query_id mutation must be rejected
        let r_launch = HermesToolExecutor::execute(
            "launch_miniapp",
            json!({ "bot_username": "acl_pomodoro_bot", "short_name": "timer" }),
            &ctx_alice,
        )
        .await;
        let query_id = serde_json::from_str::<Value>(&r_launch.output).unwrap()["query_id"]
            .as_str()
            .unwrap()
            .to_string();
        let r_eve_ma = HermesToolExecutor::execute(
            "invoke_miniapp_action",
            json!({
                "bot_username": "acl_pomodoro_bot",
                "short_name": "timer",
                "action_type": "main_button_click",
                "query_id": query_id,
                "value": "hijack",
            }),
            &ctx_eve,
        )
        .await;
        assert!(!r_eve_ma.success, "cross-user MiniApp session mutation must fail");

        // Cross-chat msg_id = 1 isolation via send_message & delete_message (use fresh chat3 & chat1 before poll or check msg 2)
        let r_chat3 = HermesToolExecutor::execute(
            "create_channel",
            json!({ "title": "Alice Secret 3", "chat_type": "group" }),
            &ctx_alice,
        )
        .await;
        let chat3 = serde_json::from_str::<Value>(&r_chat3.output).unwrap()["chat_id"]
            .as_i64()
            .unwrap();
        let m1 = HermesToolExecutor::execute(
            "send_message",
            json!({ "chat_id": chat3, "text": "alpha_cross_chat_token_111" }),
            &ctx_alice,
        )
        .await;
        assert!(m1.success);
        let m1_id = serde_json::from_str::<Value>(&m1.output).unwrap()["message_id"]
            .as_i64()
            .unwrap();
        let m2 = HermesToolExecutor::execute(
            "send_message",
            json!({ "chat_id": chat2, "text": "beta_cross_chat_token_222" }),
            &ctx_alice,
        )
        .await;
        assert!(m2.success);
        let m2_id = serde_json::from_str::<Value>(&m2.output).unwrap()["message_id"]
            .as_i64()
            .unwrap();
        assert_eq!(m1_id, m2_id, "both chats start at message_id 1");

        let s1 = HermesToolExecutor::execute(
            "search_messages",
            json!({ "chat_id": chat3, "query": "alpha_cross_chat_token_111" }),
            &ctx_alice,
        )
        .await;
        assert!(serde_json::from_str::<Value>(&s1.output).unwrap()["count"].as_u64().unwrap_or(0) >= 1);

        let del2 = HermesToolExecutor::execute(
            "delete_message",
            json!({ "chat_id": chat2, "message_id": m2_id, "revoke": true }),
            &ctx_alice,
        )
        .await;
        assert!(del2.success);
        let s1_after = HermesToolExecutor::execute(
            "search_messages",
            json!({ "chat_id": chat3, "query": "alpha_cross_chat_token_111" }),
            &ctx_alice,
        )
        .await;
        assert!(
            serde_json::from_str::<Value>(&s1_after.output).unwrap()["count"].as_u64().unwrap_or(0) >= 1,
            "deleting chat2 msg 1 must not delete chat3 msg 1"
        );
    }

    #[tokio::test]
    async fn test_webmcp_list_discover_and_headless_execution_persists_real_state() {
        let dir = TempDir::new().expect("create tempdir");
        let db_path = dir.path().join("webmcp_headless_test.db");
        let db = Arc::new(ServerDatabase::open(&db_path).expect("open db"));
        let mut cfg = ServerConfig::default();
        cfg.media_storage_path = dir.path().join("media").display().to_string();
        let state = Arc::new(AppState::with_db(cfg, db.clone()));

        let (alice, _, _) = state
            .session_manager
            .register("alice_webmcp", "Pass123456!", "Alice", None, "CLI")
            .unwrap();
        let now_ms = Utc::now().timestamp_millis();
        db.create_user(&crate::db::schema::UserRecord {
            user_id: alice.user_id,
            username: Some("alice_webmcp".to_string()),
            phone: None,
            first_name: "Alice".to_string(),
            last_name: None,
            bio: None,
            password_hash: None,
            is_bot: false,
            is_verified: false,
            is_support: false,
            is_scam: false,
            is_fake: false,
            profile_photo_file_id: None,
            account_generation: 1,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
        .unwrap();

        let bot = db
            .create_registered_bot(
                alice.user_id.0 as i64,
                "shop_bot",
                "Shop Helper",
                "Generic Mini App companion bot",
                "Coffee & Shop Mini App",
                "helpful",
                "",
                "https://shop.example/app",
                "[]",
                false,
                true,
            )
            .expect("register shop_bot");
        let bot_id = bot.bot_user_id;
        let schema_json = serde_json::json!({
            "title": "Coffee Shop",
            "widgets": [
                { "id": "item_name", "kind": "TextInput", "label": "Item", "value": "Latte" }
            ],
            "main_button": { "text": "Place Order", "is_visible": true, "is_active": true }
        })
        .to_string();
        db.create_or_upsert_mini_app_with_options(
            bot_id,
            "shop_bot",
            "shop",
            "Coffee Shop",
            "Order coffee via Mini App",
            "shopping",
            "☕",
            Some("https://shop.example/app"),
            &schema_json,
            true,
            true,
            false,
            "",
            "",
            None,
            false,
        )
        .expect("seed shop miniapp");
        let (chat_rec, _) = db.ensure_companion_bot_chat_record(alice.user_id, UserId(bot_id as u64)).unwrap();

        let alice_uid_str = alice.user_id.0.to_string();
        let mut ctx = state.fabushi_core().build_tool_context_with_user(
            "sess_webmcp_test",
            Some(chat_rec.0),
            None,
            Some(&alice_uid_str),
            1,
            Some(ToolsetPreset::Full),
            0,
            Arc::new(AtomicBool::new(false)),
        );
        ctx.bot_id = Some(bot_id);
        assert!(ctx.platform_im_bridge.is_some());

        let listed = HermesToolExecutor::execute("webmcp_list_miniapps", json!({}), &ctx).await;
        assert!(listed.success, "webmcp_list_miniapps failed: {:?}", listed.error);
        let listed_v: Value = serde_json::from_str(&listed.output).unwrap();
        assert!(listed_v["count"].as_u64().unwrap_or(0) >= 1);

        let disc = HermesToolExecutor::execute("webmcp_discover", json!({}), &ctx).await;
        assert!(disc.success, "webmcp_discover failed: {:?}", disc.error);
        let disc_v: Value = serde_json::from_str(&disc.output).unwrap();
        let tool_names: Vec<&str> = disc_v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        for expected in [
            "get_state",
            "page_inspect",
            "page_click",
            "page_fill",
            "page_press_main_button",
        ] {
            assert!(
                tool_names.contains(&expected),
                "missing universal WebMCP tool '{expected}' in {tool_names:?}"
            );
        }

        let called = HermesToolExecutor::execute(
            "webmcp_call_tool",
            json!({
                "tool_name": "page_fill",
                "arguments": { "widget_id": "item_name", "value": "Matcha Latte" }
            }),
            &ctx,
        )
        .await;
        assert!(called.success, "webmcp_call_tool failed: {:?}", called.error);
        let called_v: Value = serde_json::from_str(&called.output).unwrap();
        assert_eq!(called_v["result"]["state"]["item_name"], "Matcha Latte");

        let state_res = HermesToolExecutor::execute(
            "webmcp_call_tool",
            json!({ "tool_name": "get_state", "arguments": {} }),
            &ctx,
        )
        .await;
        assert!(state_res.success);
        let state_v: Value = serde_json::from_str(&state_res.output).unwrap();
        assert_eq!(state_v["result"]["state"]["item_name"], "Matcha Latte");

        let unregistered = HermesToolExecutor::execute(
            "webmcp_call_tool",
            json!({
                "tool_name": "fabushi_send_dharma",
                "arguments": { "sutra": "金刚经" }
            }),
            &ctx,
        )
        .await;
        assert!(
            !unregistered.success,
            "headless fallback must reject unregistered custom tool: {:?}",
            unregistered.output
        );
        let unreg_err = unregistered.error.as_deref().unwrap_or("");
        assert!(
            unreg_err.contains("未注册的 WebMCP 工具: fabushi_send_dharma"),
            "unexpected unregistered tool error: {unreg_err}"
        );
    }

    #[test]
    fn extract_webmcp_page_error_detects_boot_page_and_toast_errors() {
        assert_eq!(extract_webmcp_page_error(None), None);
        assert_eq!(extract_webmcp_page_error(Some(&json!({}))), None);

        let boot = json!({ "boot_error": "HTTP 404 (/api/v1/fabushi-miniapp/app.wasm)" });
        assert_eq!(
            extract_webmcp_page_error(Some(&boot)).as_deref(),
            Some("HTTP 404 (/api/v1/fabushi-miniapp/app.wasm)")
        );

        let page_errs = json!({ "page_errors": ["TypeError: x is not a function", "WASM trap"] });
        assert_eq!(
            extract_webmcp_page_error(Some(&page_errs)).as_deref(),
            Some("TypeError: x is not a function; WASM trap")
        );

        let err_toast = json!({
            "visible_toast": "WASM 加载失败: CompileError: WebAssembly.Module doesn't parse at byte 0: expected a module of at least 8 bytes"
        });
        assert_eq!(
            extract_webmcp_page_error(Some(&err_toast)).as_deref(),
            Some("WASM 加载失败: CompileError: WebAssembly.Module doesn't parse at byte 0: expected a module of at least 8 bytes")
        );

        let normal_toast = json!({ "visible_toast": "已保存" });
        assert_eq!(extract_webmcp_page_error(Some(&normal_toast)), None);
    }

    #[tokio::test]
    async fn test_headless_webmcp_discover_catalog_matches_call_tool_and_generic_error_guidance() {
        use gpui_telegram_proto::miniapp_contracts::{MiniAppControlMode, MiniAppWebMcpResponse};
        use tokio::sync::mpsc;

        let dir = TempDir::new().expect("create tempdir");
        let db_path = dir.path().join("webmcp_catalog_match.db");
        let db = Arc::new(ServerDatabase::open(&db_path).expect("open db"));
        let mut cfg = ServerConfig::default();
        cfg.media_storage_path = dir.path().join("media").display().to_string();
        let state = Arc::new(AppState::with_db(cfg, db.clone()));

        let (alice, _, _) = state
            .session_manager
            .register("alice_cat", "Pass123456!", "AliceCat", None, "CLI")
            .unwrap();
        let now_ms = Utc::now().timestamp_millis();
        db.create_user(&crate::db::schema::UserRecord {
            user_id: alice.user_id,
            username: Some("alice_cat".to_string()),
            phone: None,
            first_name: "AliceCat".to_string(),
            last_name: None,
            bio: None,
            password_hash: None,
            is_bot: false,
            is_verified: false,
            is_support: false,
            is_scam: false,
            is_fake: false,
            profile_photo_file_id: None,
            account_generation: 1,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
        .unwrap();

        let bot = db
            .create_registered_bot(
                alice.user_id.0 as i64,
                "cat_bot",
                "Cat Helper",
                "desc",
                "about",
                "persona",
                "",
                "https://cat.example/app",
                "[]",
                false,
                true,
            )
            .unwrap();
        let schema_json = serde_json::json!({
            "title": "Coffee Shop",
            "widgets": [
                { "id": "item_name", "kind": "TextInput", "label": "Item", "value": "Latte" }
            ],
            "main_button": { "text": "Place Order", "is_visible": true, "is_active": true }
        })
        .to_string();
        db.create_or_upsert_mini_app_with_options(
            bot.bot_user_id,
            "cat_bot",
            "shop",
            "Coffee Shop",
            "Order coffee",
            "shopping",
            "☕",
            Some("https://cat.example/app"),
            &schema_json,
            true,
            true,
            false,
            "",
            "",
            None,
            false,
        )
        .unwrap();
        let (chat_rec, _) = db
            .ensure_companion_bot_chat_record(alice.user_id, UserId(bot.bot_user_id as u64))
            .unwrap();

        let alice_uid_str = alice.user_id.0.to_string();
        let mut ctx = state.fabushi_core().build_tool_context_with_user(
            "sess_cat",
            Some(chat_rec.0),
            None,
            Some(&alice_uid_str),
            1,
            Some(ToolsetPreset::Full),
            0,
            Arc::new(AtomicBool::new(false)),
        );
        ctx.bot_id = Some(bot.bot_user_id);

        // 1. Every tool returned by headless webmcp_discover must be accepted by headless webmcp_call_tool
        let disc = HermesToolExecutor::execute("webmcp_discover", json!({}), &ctx).await;
        assert!(disc.success);
        let disc_v: Value = serde_json::from_str(&disc.output).unwrap();
        let tool_names: Vec<String> = disc_v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect();
        assert!(!tool_names.is_empty());

        for tool_name in &tool_names {
            let call_args = match tool_name.as_str() {
                "page_click" => json!({ "text": "Place Order" }),
                "page_fill" => json!({ "widget_id": "item_name", "value": "Cortado" }),
                _ => json!({}),
            };
            let res = HermesToolExecutor::execute(
                "webmcp_call_tool",
                json!({ "tool_name": tool_name, "arguments": call_args }),
                &ctx,
            )
            .await;
            assert!(
                res.success,
                "headless catalog tool '{tool_name}' must be accepted by headless webmcp_call_tool, got {:?}",
                res.error
            );
        }

        // 2. Live WebView !ok with empty registered_tools and generic error (no "未注册"/"not registered")
        // must STILL surface desktop-open/refresh guidance
        let (tx, mut rx) = mpsc::unbounded_channel();
        state.gateway_hub.register(alice.user_id, tx);
        let state_bg = Arc::clone(&state);
        let responder = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                if let WsServerFrame::Update(UpdateEvent::MiniAppWebMcpCommand(cmd)) = frame {
                    state_bg.webmcp_broker.resolve_rpc_response(
                        alice.user_id,
                        MiniAppWebMcpResponse {
                            request_id: cmd.request_id,
                            ok: false,
                            result: json!({}),
                            error: Some("TypeError: cannot read properties of undefined".to_string()),
                            tools: Vec::new(),
                            state_summary: None,
                            control_mode: MiniAppControlMode::AiControlling,
                        },
                    );
                    break;
                }
            }
        });

        let generic_fail = HermesToolExecutor::execute(
            "webmcp_call_tool",
            json!({ "tool_name": "custom_tool", "arguments": {} }),
            &ctx,
        )
        .await;
        assert!(!generic_fail.success);
        let err_msg = generic_fail.error.as_deref().unwrap_or("");
        assert!(
            err_msg.contains("TypeError: cannot read properties of undefined"),
            "got {err_msg}"
        );
        assert!(
            err_msg.contains("请先在桌面端打开或刷新「Coffee Shop」小程序窗口后重试"),
            "missing desktop-open guidance on generic error + empty tools: {err_msg}"
        );
        let _ = responder.await;
    }
}
