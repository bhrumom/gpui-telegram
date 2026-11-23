//! BotAttachWebView: Telegram Mini Apps interactive native GPUI runtime, sheet & AI Co-Pilot.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{Bounds, Context, Pixels, Window};
use gpui_telegram_proto::miniapp_contracts::{
    CloudStorageItem, MiniAppActionButton, MiniAppActionRequest, MiniAppActionResponse,
    MiniAppControlMode, MiniAppLaunchContext, MiniAppUiSchema, MiniAppWebMcpCommand,
    MiniAppWebMcpResponse, WebMcpToolDescriptor,
};

#[cfg(target_os = "macos")]
use gpui::{AsyncApp, WeakEntity};
#[cfg(target_os = "macos")]
use super::macos_wkwebview::MacOsWkWebView;
use crate::theme::TdesktopThemePalette;

/// Timeout before prolonging active bot webview session (60 seconds).
pub const K_PROLONG_TIMEOUT_SECS: u64 = 60;
/// Timeout before refreshing attach bots list (3600 seconds).
pub const K_REFRESH_BOTS_TIMEOUT_SECS: u64 = 3600;
/// Limit for popular app bots in catalog searches.
pub const K_POPULAR_APP_BOTS_LIMIT: usize = 100;

/// Bitmask for allowed peer types where an attach menu bot can be launched.
pub mod peer_types {
    pub const SAME_BOT: u8 = 0x01;
    pub const BOT: u8 = 0x02;
    pub const USER: u8 = 0x04;
    pub const GROUP: u8 = 0x08;
    pub const BROADCAST: u8 = 0x10;
}

pub fn peer_matches_types(peer_type: u8, allowed_types: u8) -> bool {
    (peer_type & allowed_types) != 0
}

pub fn peer_types_mask_from_strings(peer_types_vec: &[String]) -> u8 {
    if peer_types_vec.is_empty() {
        return peer_types::SAME_BOT
            | peer_types::BOT
            | peer_types::USER
            | peer_types::GROUP
            | peer_types::BROADCAST;
    }
    let mut mask = 0u8;
    for raw in peer_types_vec {
        match raw.trim().to_ascii_lowercase().as_str() {
            "same_bot" | "samebot" => mask |= peer_types::SAME_BOT,
            "bot" | "bots" => mask |= peer_types::BOT,
            "user" | "users" | "private" => mask |= peer_types::USER,
            "group" | "groups" | "chat" | "supergroup" => mask |= peer_types::GROUP,
            "broadcast" | "channel" | "channels" => mask |= peer_types::BROADCAST,
            _ => {}
        }
    }
    if mask == 0 {
        peer_types::SAME_BOT | peer_types::BOT | peer_types::USER | peer_types::GROUP
    } else {
        mask
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachBotEntry {
    pub bot_id: u64,
    pub name: String,
    pub short_name: String,
    pub allowed_peer_types: u8,
    pub in_main_menu: bool,
    pub in_attach_menu: bool,
    pub disclaimer_required: bool,
    pub request_write_access: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotWebViewInputTarget {
    WidgetText(String),
    CopilotPrompt,
    CloudKey,
    CloudValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiniAppPopupButton {
    pub id: String,
    pub btn_type: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiniAppPopupState {
    pub title: String,
    pub message: String,
    pub buttons: Vec<MiniAppPopupButton>,
}

#[derive(Debug, Clone)]
pub enum BotWebViewPendingAction {
    Close,
    Minimize,
    Reload {
        bot_username: String,
        short_name: String,
        chat_id: Option<i64>,
        custom_url: Option<String>,
    },
    AcceptDisclaimer {
        bot_username: String,
        short_name: String,
        allow_write_access: bool,
    },
    AllowBotSendMessage {
        bot_id: u64,
        bot_username: String,
        app_id: Option<String>,
        app_title: String,
    },
    OpenBotProfile(String),
    OpenMiniAppTos,
    OpenTelegramLink(String),
    OpenCompanionChat {
        companion_chat_id: i64,
        bot_username: String,
    },
    OpenInBrowser {
        path: String,
    },
    UninstallApp {
        bot_username: String,
        short_name: String,
    },
    SendAppAction {
        bot_username: String,
        short_name: String,
        req: MiniAppActionRequest,
    },
    CloudStorageRefresh {
        bot_username: String,
        short_name: String,
    },
    CloudStorageSet {
        bot_username: String,
        short_name: String,
        key: String,
        value: String,
    },
    CloudStorageDelete {
        bot_username: String,
        short_name: String,
        key: String,
    },
    TakeoverMiniApp,
    FinishTakeoverAndResumeAi {
        user_note: Option<String>,
        state_snapshot: Option<serde_json::Value>,
    },
    SwitchToFloatingPip,
    ExpandFromFloatingPip,
    WebMcpResponseReady(MiniAppWebMcpResponse),
}

/// Authentic BotAttachWebView component state + full Telegram Desktop Mini App interactive shell.
pub struct BotAttachWebView {
    pub active_bot: Option<AttachBotEntry>,
    pub url: String,
    pub is_open: bool,
    pub is_fullscreen: bool,
    pub theme_params_json: String,
    pub disclaimer_accepted: bool,
    pub last_active_ts: u64,
    pub updated_at: u64,

    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub bot_id: u64,
    pub app_id: Option<String>,
    pub bot_username: String,
    pub companion_chat_id: Option<i64>,
    pub source_chat_id: Option<i64>,
    pub launch_context: Option<MiniAppLaunchContext>,
    pub ui_schema: MiniAppUiSchema,
    pub cloud_storage: Vec<CloudStorageItem>,
    pub active_tab: u8,
    pub menu_open: bool,
    pub confirm_close_open: bool,
    pub write_access_prompt_open: bool,
    pub allows_write_to_pm: bool,
    pub custom_popup: Option<MiniAppPopupState>,
    pub closing_confirmation: bool,
    pub header_color_override: Option<String>,
    pub background_color_override: Option<String>,
    pub server_base_url: String,
    pub is_dirty: bool,
    pub is_loading: bool,
    pub status_toast: Option<String>,
    pub allow_write_access_checked: bool,
    pub copilot_expanded: bool,
    pub copilot_prompt: String,
    pub copilot_last_reply: Option<String>,
    pub focused_input: Option<BotWebViewInputTarget>,
    pub cloud_key_draft: String,
    pub cloud_value_draft: String,
    pub pending_actions: VecDeque<BotWebViewPendingAction>,
    pub external_modal_occluding: bool,
    pub control_mode: MiniAppControlMode,
    pub control_status_text: String,
    pub active_webmcp_tool: Option<String>,
    pub is_floating_pip: bool,
    pub pip_live_preview: bool,
    pub last_webmcp_state_summary: Option<serde_json::Value>,
    pub pending_webmcp_queue: VecDeque<MiniAppWebMcpCommand>,
    pub webmcp_page_ready: bool,
    pub webmcp_discovered_tools: Vec<WebMcpToolDescriptor>,
    pub webmcp_iframe_ready: bool,
    pub webmcp_needs_rediscover: bool,
    pub webmcp_ready_probe_sent: bool,
    pub(crate) webmcp_internal_seq: u64,
    pub launching_short_name: String,
    #[cfg(target_os = "macos")]
    pub(crate) macos_webview: Option<MacOsWkWebView>,
    #[cfg(target_os = "macos")]
    pub(crate) bridge_listener_started: bool,
}

impl BotAttachWebView {
    pub fn new() -> Self {
        let palette = TdesktopThemePalette::default();
        Self {
            active_bot: None,
            url: String::new(),
            is_open: false,
            is_fullscreen: false,
            theme_params_json: Self::build_theme_params_json(&palette),
            disclaimer_accepted: false,
            last_active_ts: 0,
            updated_at: 0,
            palette,
            language_id: "en".to_string(),
            bot_id: 0,
            app_id: None,
            bot_username: String::new(),
            companion_chat_id: None,
            source_chat_id: None,
            launch_context: None,
            ui_schema: MiniAppUiSchema::default(),
            cloud_storage: Vec::new(),
            active_tab: 0,
            menu_open: false,
            confirm_close_open: false,
            write_access_prompt_open: false,
            allows_write_to_pm: false,
            custom_popup: None,
            closing_confirmation: false,
            header_color_override: None,
            background_color_override: None,
            server_base_url: String::new(),
            is_dirty: false,
            is_loading: false,
            status_toast: None,
            allow_write_access_checked: true,
            copilot_expanded: true,
            copilot_prompt: String::new(),
            copilot_last_reply: None,
            focused_input: None,
            cloud_key_draft: String::new(),
            cloud_value_draft: String::new(),
            pending_actions: VecDeque::new(),
            external_modal_occluding: false,
            control_mode: MiniAppControlMode::Idle,
            control_status_text: String::new(),
            active_webmcp_tool: None,
            is_floating_pip: false,
            pip_live_preview: true,
            last_webmcp_state_summary: None,
            pending_webmcp_queue: VecDeque::new(),
            webmcp_page_ready: false,
            webmcp_discovered_tools: Vec::new(),
            webmcp_iframe_ready: false,
            webmcp_needs_rediscover: false,
            webmcp_ready_probe_sent: false,
            webmcp_internal_seq: 0,
            launching_short_name: String::new(),
            #[cfg(target_os = "macos")]
            macos_webview: None,
            #[cfg(target_os = "macos")]
            bridge_listener_started: false,
        }
    }

    fn build_theme_params_json(palette: &TdesktopThemePalette) -> String {
        format!(
            "{{\"bg_color\":\"#{:06x}\",\"secondary_bg_color\":\"#{:06x}\",\"text_color\":\"#{:06x}\",\"hint_color\":\"#{:06x}\",\"button_color\":\"#{:06x}\"}}",
            palette.modal_bg & 0xFFFFFF,
            palette.card_bg & 0xFFFFFF,
            palette.text_primary & 0xFFFFFF,
            palette.text_secondary & 0xFFFFFF,
            palette.accent & 0xFFFFFF
        )
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.palette = palette;
        self.theme_params_json = Self::build_theme_params_json(&palette);
        let theme_ev = format!("{{\"theme_params\":{}}}", self.theme_params_json);
        self.dispatch_js_event("theme_changed", &theme_ev);
    }

    pub fn set_language(&mut self, lang_id: impl Into<String>) {
        self.language_id = lang_id.into();
    }

    pub fn set_server_base_url(&mut self, base_url: impl Into<String>) {
        self.server_base_url = base_url.into().trim_end_matches('/').to_string();
    }

    pub fn launch(&mut self, bot: AttachBotEntry, url: impl Into<String>) {
        let ts = now_unix_secs();
        self.disclaimer_accepted = !bot.disclaimer_required;
        self.allow_write_access_checked = bot.request_write_access;
        if bot.bot_id > 0 {
            self.bot_id = bot.bot_id;
        }
        if self.bot_username.is_empty() {
            self.bot_username = bot.short_name.clone();
        }
        self.active_bot = Some(bot);
        self.url = url.into();
        self.is_open = true;
        self.menu_open = false;
        self.confirm_close_open = false;
        self.write_access_prompt_open = false;
        self.custom_popup = None;
        self.last_active_ts = ts;
        self.updated_at = ts;
    }

    pub fn load_context(&mut self, ctx: MiniAppLaunchContext) {
        let tos_accepted = ctx.app.app_tos_accepted || ctx.app.is_installed;
        let entry = AttachBotEntry {
            bot_id: ctx.app.bot_user_id.max(0) as u64,
            name: ctx.app.title.clone(),
            short_name: ctx.app.short_name.clone(),
            allowed_peer_types: peer_types_mask_from_strings(&ctx.app.peer_types),
            in_main_menu: true,
            in_attach_menu: ctx.companion_bot.allow_attach_menu,
            disclaimer_required: !tos_accepted,
            request_write_access: true,
        };
        let deep_url = if ctx.app.web_url.is_empty() {
            ctx.app.gpui_uri()
        } else {
            ctx.app.web_url.clone()
        };
        self.bot_id = entry.bot_id;
        self.app_id = if ctx.app.app_id.trim().is_empty() {
            None
        } else {
            Some(ctx.app.app_id.clone())
        };
        self.bot_username = ctx.app.bot_username.clone();
        self.launching_short_name = ctx.app.short_name.clone();
        self.companion_chat_id = Some(ctx.companion_chat_id);
        self.source_chat_id = ctx.source_chat_id;
        self.ui_schema = ctx.app.ui_schema.clone();
        self.cloud_storage = ctx.cloud_storage.clone();
        self.allows_write_to_pm =
            ctx.companion_bot.allows_write_to_pm || ctx.app.has_write_access;
        self.header_color_override = ctx.app.header_color.clone();
        self.background_color_override = ctx.app.background_color.clone();
        self.closing_confirmation = false;
        self.launch(entry, deep_url);
        self.disclaimer_accepted = tos_accepted;
        self.allow_write_access_checked = true;
        self.is_dirty = false;
        self.is_loading = false;
        self.status_toast = None;
        self.webmcp_page_ready = false;
        self.webmcp_iframe_ready = false;
        self.webmcp_needs_rediscover = false;
        self.webmcp_ready_probe_sent = false;
        self.webmcp_discovered_tools.clear();
        self.last_webmcp_state_summary = None;
        self.launch_context = Some(ctx);
    }

    pub fn close(&mut self) {
        self.destroy_native_webview();
        self.is_open = false;
        self.is_floating_pip = false;
        self.pip_live_preview = true;
        self.control_mode = MiniAppControlMode::Idle;
        self.control_status_text.clear();
        self.active_webmcp_tool = None;
        self.last_webmcp_state_summary = None;
        self.pending_webmcp_queue.clear();
        self.webmcp_page_ready = false;
        self.webmcp_iframe_ready = false;
        self.webmcp_needs_rediscover = false;
        self.webmcp_ready_probe_sent = false;
        self.webmcp_discovered_tools.clear();
        self.active_bot = None;
        self.url.clear();
        self.app_id = None;
        self.launching_short_name.clear();
        self.menu_open = false;
        self.confirm_close_open = false;
        self.write_access_prompt_open = false;
        self.custom_popup = None;
        self.focused_input = None;
    }

    /// Clears previous Mini App runtime/WebMCP/native WebView state before starting a new `/launch`
    /// on a reused `BotAttachWebView` overlay entity.
    pub fn reset_for_new_launch(&mut self) {
        self.destroy_native_webview();
        self.active_bot = None;
        self.launch_context = None;
        self.app_id = None;
        self.bot_id = 0;
        self.bot_username.clear();
        self.companion_chat_id = None;
        self.source_chat_id = None;
        self.url.clear();
        self.disclaimer_accepted = false;
        self.is_fullscreen = false;
        self.last_active_ts = 0;
        self.updated_at = 0;
        self.ui_schema = MiniAppUiSchema::default();
        self.cloud_storage.clear();
        self.active_tab = 0;
        self.menu_open = false;
        self.confirm_close_open = false;
        self.write_access_prompt_open = false;
        self.allows_write_to_pm = false;
        self.custom_popup = None;
        self.closing_confirmation = false;
        self.header_color_override = None;
        self.background_color_override = None;
        self.is_dirty = false;
        self.status_toast = None;
        self.copilot_prompt.clear();
        self.copilot_last_reply = None;
        self.focused_input = None;
        self.cloud_key_draft.clear();
        self.cloud_value_draft.clear();
        self.control_mode = MiniAppControlMode::Idle;
        self.control_status_text.clear();
        self.active_webmcp_tool = None;
        self.is_floating_pip = false;
        self.pip_live_preview = true;
        self.last_webmcp_state_summary = None;
        self.pending_webmcp_queue.clear();
        self.webmcp_page_ready = false;
        self.webmcp_discovered_tools.clear();
        self.webmcp_iframe_ready = false;
        self.webmcp_needs_rediscover = false;
        self.webmcp_ready_probe_sent = false;
        self.launching_short_name.clear();
        self.pending_actions.clear();
    }

    pub fn minimize(&mut self) {
        self.is_open = false;
        self.menu_open = false;
        self.confirm_close_open = false;
        self.write_access_prompt_open = false;
        self.custom_popup = None;
        self.hide_native_webview();
    }

    pub fn set_fullscreen(&mut self, fullscreen: bool) {
        self.is_fullscreen = fullscreen;
    }

    pub fn accept_disclaimer(&mut self) {
        self.disclaimer_accepted = true;
        if let Some(ref mut bot) = self.active_bot {
            bot.disclaimer_required = false;
        }
    }

    pub fn apply_action_response(&mut self, resp: MiniAppActionResponse) {
        self.is_loading = false;
        self.ui_schema = resp.ui_schema;
        if let Some(cid) = resp.companion_chat_id {
            self.companion_chat_id = Some(cid);
        }
        if let Some(reply) = resp.ai_reply.or(resp.companion_bot_message) {
            self.copilot_last_reply = Some(reply);
            self.copilot_expanded = true;
        }
        if let Some(toast) = resp.toast {
            self.status_toast = Some(toast);
        }
        if resp.close_app {
            self.close();
            self.pending_actions.push_back(BotWebViewPendingAction::Close);
        }
    }

    pub fn apply_cloud_storage(&mut self, items: Vec<CloudStorageItem>) {
        self.cloud_storage = items;
    }

    pub fn take_pending_actions(&mut self) -> Vec<BotWebViewPendingAction> {
        self.pending_actions.drain(..).collect()
    }

    pub fn has_focused_text_input(&self) -> bool {
        self.is_open && self.focused_input.is_some()
    }

    pub fn active_input_text(&self) -> String {
        match &self.focused_input {
            Some(BotWebViewInputTarget::CopilotPrompt) => self.copilot_prompt.clone(),
            Some(BotWebViewInputTarget::CloudKey) => self.cloud_key_draft.clone(),
            Some(BotWebViewInputTarget::CloudValue) => self.cloud_value_draft.clone(),
            Some(BotWebViewInputTarget::WidgetText(id)) => self
                .ui_schema
                .widgets
                .iter()
                .find(|w| &w.id == id)
                .map(|w| w.value.clone())
                .unwrap_or_default(),
            None => String::new(),
        }
    }

    pub fn push_text_to_focused_input(&mut self, text: &str) -> bool {
        let Some(target) = self.focused_input.clone() else {
            return false;
        };
        match target {
            BotWebViewInputTarget::CopilotPrompt => {
                self.copilot_prompt.push_str(text);
                true
            }
            BotWebViewInputTarget::CloudKey => {
                self.cloud_key_draft.push_str(text);
                true
            }
            BotWebViewInputTarget::CloudValue => {
                self.cloud_value_draft.push_str(text);
                true
            }
            BotWebViewInputTarget::WidgetText(id) => {
                if let Some(w) = self.ui_schema.widgets.iter_mut().find(|w| w.id == id) {
                    w.value.push_str(text);
                    self.is_dirty = true;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn backspace_focused_input(&mut self) -> bool {
        let Some(target) = self.focused_input.clone() else {
            return false;
        };
        match target {
            BotWebViewInputTarget::CopilotPrompt => {
                self.copilot_prompt.pop();
                true
            }
            BotWebViewInputTarget::CloudKey => {
                self.cloud_key_draft.pop();
                true
            }
            BotWebViewInputTarget::CloudValue => {
                self.cloud_value_draft.pop();
                true
            }
            BotWebViewInputTarget::WidgetText(id) => {
                if let Some(w) = self.ui_schema.widgets.iter_mut().find(|w| w.id == id) {
                    w.value.pop();
                    self.is_dirty = true;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn submit_focused_input(&mut self) -> bool {
        let Some(target) = self.focused_input.clone() else {
            return false;
        };
        match target {
            BotWebViewInputTarget::CopilotPrompt => {
                self.trigger_ai_copilot();
                true
            }
            BotWebViewInputTarget::CloudKey | BotWebViewInputTarget::CloudValue => {
                self.trigger_save_cloud_storage();
                true
            }
            BotWebViewInputTarget::WidgetText(id) => {
                if let Some(val) = self
                    .ui_schema
                    .widgets
                    .iter()
                    .find(|w| w.id == id)
                    .map(|w| w.value.clone())
                {
                    self.queue_widget_event(id, serde_json::json!(val));
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn current_short_name(&self) -> String {
        if self.is_loading && !self.launching_short_name.is_empty() {
            return self.launching_short_name.clone();
        }
        self.active_bot
            .as_ref()
            .map(|b| b.short_name.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| self.launch_context.as_ref().map(|c| c.app.short_name.clone()))
            .filter(|s| !s.is_empty())
            .or_else(|| {
                (!self.launching_short_name.is_empty()).then(|| self.launching_short_name.clone())
            })
            .unwrap_or_else(|| "app".to_string())
    }

    pub(crate) fn target_chat_i64(&self) -> Option<i64> {
        self.source_chat_id.or(self.companion_chat_id)
    }

    pub fn queue_widget_event(&mut self, widget_id: String, value: serde_json::Value) {
        self.is_dirty = true;
        self.is_loading = true;
        self.pending_actions
            .push_back(BotWebViewPendingAction::SendAppAction {
                bot_username: self.bot_username.clone(),
                short_name: self.current_short_name(),
                req: MiniAppActionRequest::widget_event(
                    widget_id,
                    value,
                    self.target_chat_i64(),
                ),
            });
    }

    pub fn trigger_ai_copilot(&mut self) {
        let prompt = self.copilot_prompt.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        self.copilot_prompt.clear();
        self.is_loading = true;
        self.pending_actions
            .push_back(BotWebViewPendingAction::SendAppAction {
                bot_username: self.bot_username.clone(),
                short_name: self.current_short_name(),
                req: MiniAppActionRequest::ai_copilot(prompt, self.target_chat_i64()),
            });
    }

    pub fn trigger_save_cloud_storage(&mut self) {
        let key = self.cloud_key_draft.trim().to_string();
        let val = self.cloud_value_draft.clone();
        if key.is_empty() {
            return;
        }
        self.pending_actions
            .push_back(BotWebViewPendingAction::CloudStorageSet {
                bot_username: self.bot_username.clone(),
                short_name: self.current_short_name(),
                key,
                value: val,
            });
        self.cloud_key_draft.clear();
        self.cloud_value_draft.clear();
    }

    /// Resolves a pending `web_app_request_write_access` prompt and notifies the loaded Mini App JS.
    pub fn resolve_write_access(&mut self, allowed: bool) {
        self.write_access_prompt_open = false;
        if allowed {
            self.allows_write_to_pm = true;
            self.dispatch_js_event("write_access_requested", r#"{"status":"allowed"}"#);
        } else {
            self.dispatch_js_event("write_access_requested", r#"{"status":"cancelled"}"#);
        }
    }

    /// Dismisses any active `showPopup` dialog and emits `popup_closed` back to the Mini App JS.
    pub fn resolve_custom_popup(&mut self, button_id: Option<&str>) {
        self.custom_popup = None;
        let payload = match button_id {
            Some(id) => serde_json::json!({ "button_id": id }).to_string(),
            None => "{}".to_string(),
        };
        self.dispatch_js_event("popup_closed", &payload);
    }

    /// Dispatches a Telegram WebApp event into the attached macOS `WKWebView` when active.
    pub fn dispatch_js_event(&self, event_type: &str, event_data_json: &str) {
        #[cfg(target_os = "macos")]
        if let Some(ref wv) = self.macos_webview {
            wv.dispatch_telegram_event(event_type, event_data_json);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (event_type, event_data_json);
    }

    /// Builds the resolved `http(s)://...#tgWebAppData=...` URL for loading in native `WKWebView`.
    pub fn resolved_webview_url(&self) -> String {
        let raw = self.url.trim();
        let server_base = self.server_base_url.trim().trim_end_matches('/');
        let base_url = if raw.is_empty() || raw.starts_with("gpui-miniapp://") {
            if server_base.is_empty() {
                return raw.to_string();
            }
            let bot = self.bot_username.trim().trim_start_matches('@');
            let short = self.current_short_name();
            format!("{server_base}/miniapps/{bot}/{short}")
        } else if raw.starts_with('/') {
            if server_base.is_empty() {
                return raw.to_string();
            }
            format!("{server_base}{raw}")
        } else {
            raw.to_string()
        };

        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            return base_url;
        }
        if base_url.contains("tgWebAppData=") {
            return base_url;
        }
        let init_qs = self
            .launch_context
            .as_ref()
            .map(|c| c.init_data.raw_query_string.as_str())
            .unwrap_or("");
        let frag = format!(
            "tgWebAppData={}&tgWebAppVersion=8.0&tgWebAppPlatform=tdesktop&tgWebAppThemeParams={}",
            percent_encode_component(init_qs),
            percent_encode_component(&self.theme_params_json),
        );
        if base_url.contains('#') {
            format!("{base_url}&{frag}")
        } else {
            format!("{base_url}#{frag}")
        }
    }

    /// Returns `true` when an AppKit `WKWebView` child view is currently attached.
    pub fn has_native_webview(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.macos_webview.is_some()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// Returns `true` when the attached macOS `WKWebView` is visible above the window canvas.
    pub fn is_native_webview_visible(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.macos_webview
                .as_ref()
                .is_some_and(|wv| !wv.is_hidden())
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// Hides the native child `WKWebView` without destroying its DOM/JS session (used on minimize/modal).
    pub fn hide_native_webview(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(ref mut wv) = self.macos_webview {
            wv.set_hidden(true);
        }
    }

    /// Parks the native child `WKWebView` in a 1x1 near-transparent frame so background PiP WebMCP executes smoothly.
    pub fn park_native_webview_for_pip(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(ref mut wv) = self.macos_webview {
            wv.park_for_pip();
        }
    }

    /// Updates live AI/takeover control status metadata shown in chrome banner and Floating PiP card.
    pub fn apply_control_status(
        &mut self,
        mode: MiniAppControlMode,
        status_text: &str,
        active_tool: Option<&str>,
    ) {
        self.control_mode = mode;
        if !status_text.trim().is_empty() {
            self.control_status_text = status_text.to_string();
        }
        self.active_webmcp_tool = active_tool.map(|s| s.to_string());
    }

    /// Enqueues or immediately executes a server WebMCP command against this Mini App WebView.
    pub fn dispatch_webmcp_command(&mut self, cmd: MiniAppWebMcpCommand) {
        if cmd.operation == "request_takeover" {
            self.control_mode = MiniAppControlMode::WaitingForTakeover;
            self.control_status_text = cmd
                .status_message
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "AI 请求您手动接管小程序".to_string());
            self.active_webmcp_tool = None;
            self.is_open = true;
            self.is_floating_pip = false;
            self.disclaimer_accepted = true;
            return;
        }

        self.control_mode = MiniAppControlMode::AiControlling;
        self.active_webmcp_tool = cmd.tool_name.clone();
        self.control_status_text = cmd
            .status_message
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                cmd.tool_name
                    .as_deref()
                    .map(|t| format!("正在调用 {t}"))
                    .unwrap_or_else(|| "AI 正在读取小程序状态".to_string())
            });
        self.pending_webmcp_queue.push_back(cmd);
        self.flush_pending_webmcp_queue();
    }

    /// Flushes queued WebMCP commands into the live `WKWebView` (or headless test fallback).
    pub fn flush_pending_webmcp_queue(&mut self) {
        if self.pending_webmcp_queue.is_empty() {
            return;
        }
        #[cfg(target_os = "macos")]
        let use_headless_fallback = MacOsWkWebView::is_headless_or_offline_test_env();
        #[cfg(not(target_os = "macos"))]
        let use_headless_fallback = true;

        if use_headless_fallback {
            while let Some(cmd) = self.pending_webmcp_queue.pop_front() {
                let is_discover = cmd.operation == "discover";
                let tools = if is_discover {
                    vec![
                        WebMcpToolDescriptor {
                            name: "get_state".to_string(),
                            description: "Read Mini App UI state".to_string(),
                            input_schema: serde_json::json!({ "type": "object" }),
                            read_only: true,
                        },
                        WebMcpToolDescriptor {
                            name: "page_inspect".to_string(),
                            description: "Inspect Mini App controls".to_string(),
                            input_schema: serde_json::json!({ "type": "object" }),
                            read_only: true,
                        },
                    ]
                } else {
                    Vec::new()
                };
                let summary = serde_json::json!({
                    "title": self.ui_schema.title,
                    "bot_username": self.bot_username,
                    "short_name": self.current_short_name(),
                    "control_mode": "ai_controlling",
                });
                self.last_webmcp_state_summary = Some(summary.clone());
                self.pending_actions
                    .push_back(BotWebViewPendingAction::WebMcpResponseReady(
                        MiniAppWebMcpResponse {
                            request_id: cmd.request_id,
                            ok: true,
                            result: serde_json::json!({
                                "headless_desktop": true,
                                "operation": cmd.operation,
                                "tool_name": cmd.tool_name,
                            }),
                            error: None,
                            tools,
                            state_summary: Some(summary),
                            control_mode: MiniAppControlMode::AiControlling,
                        },
                    ));
            }
            return;
        }

        #[cfg(target_os = "macos")]
        {
            if !self.webmcp_page_ready {
                return;
            }
            let Some(ref wv) = self.macos_webview else {
                return;
            };
            while let Some(cmd) = self.pending_webmcp_queue.pop_front() {
                let req_lit = serde_json::to_string(&cmd.request_id).unwrap_or_else(|_| "\"\"".to_string());
                let op_lit = serde_json::to_string(&cmd.operation).unwrap_or_else(|_| "\"\"".to_string());
                let tool_lit = cmd
                    .tool_name
                    .as_deref()
                    .and_then(|t| serde_json::to_string(t).ok())
                    .unwrap_or_else(|| "null".to_string());
                let args_lit = serde_json::to_string(&cmd.arguments).unwrap_or_else(|_| "{}".to_string());
                let js = format!(
                    "(function(){{try{{if(window.__GPUI_WEBMCP__&&typeof window.__GPUI_WEBMCP__.invoke==='function'){{window.__GPUI_WEBMCP__.invoke({req_lit},{op_lit},{tool_lit},{args_lit});}}else if(window.TelegramWebviewProxy&&typeof window.TelegramWebviewProxy.postEvent==='function'){{window.TelegramWebviewProxy.postEvent('webmcp_response',JSON.stringify({{request_id:{req_lit},ok:false,result:{{}},error:'WebMCP runtime not ready on page',tools:[],control_mode:'ai_controlling'}}));}}}}catch(e){{try{{window.TelegramWebviewProxy&&window.TelegramWebviewProxy.postEvent('webmcp_response',JSON.stringify({{request_id:{req_lit},ok:false,result:{{}},error:String(e),tools:[],control_mode:'ai_controlling'}}));}}catch(_){{}}}}}})();"
                );
                wv.evaluate_js(&js);
            }
        }
    }

    /// Detaches and releases the native child `WKWebView` (`removeFromSuperview`).
    pub fn destroy_native_webview(&mut self) {
        #[cfg(target_os = "macos")]
        {
            if let Some(mut wv) = self.macos_webview.take() {
                wv.destroy();
            }
            self.bridge_listener_started = false;
        }
    }

    /// Reloads the live `WKWebView` when attached, or queues an HTTP reload action in headless mode.
    pub fn reload_webview(&mut self) {
        self.webmcp_page_ready = false;
        self.webmcp_iframe_ready = false;
        self.webmcp_needs_rediscover = true;
        self.webmcp_ready_probe_sent = false;
        self.webmcp_discovered_tools.clear();
        #[cfg(target_os = "macos")]
        if let Some(ref mut wv) = self.macos_webview {
            wv.reload();
            self.is_loading = false;
            return;
        }
        self.is_loading = true;
        self.pending_actions.push_back(BotWebViewPendingAction::Reload {
            bot_username: self.bot_username.clone(),
            short_name: self.current_short_name(),
            chat_id: self.target_chat_i64(),
            custom_url: gpui_telegram_proto::miniapp_contracts::sanitize_webapp_custom_url(&self.url),
        });
    }

    /// Drains queued JS->Rust bridge events from the native `WKWebView` and dispatches them.
    pub fn drain_native_bridge_events(&mut self) -> bool {
        #[cfg(target_os = "macos")]
        {
            let events = match self.macos_webview.as_ref() {
                Some(wv) => wv.drain_events(),
                None => return false,
            };
            if events.is_empty() {
                return false;
            }
            for (event_type, event_data) in events {
                self.handle_bridge_event(&event_type, &event_data);
            }
            true
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// Synchronizes child macOS `WKWebView` attachment, URL, frame bounds, and bridge wakeup task.
    pub fn sync_native_webview(
        &mut self,
        window: &mut Window,
        bounds: Bounds<Pixels>,
        visible: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        #[cfg(target_os = "macos")]
        {
            if MacOsWkWebView::is_headless_or_offline_test_env() {
                return false;
            }
            if let Some(ref wv) = self.macos_webview {
                if !wv.matches_window(window) {
                    self.destroy_native_webview();
                }
            }
            if self.macos_webview.is_none() {
                if let Some((wv, mut rx)) = MacOsWkWebView::attach(window) {
                    self.macos_webview = Some(wv);
                    if !self.bridge_listener_started {
                        self.bridge_listener_started = true;
                        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                            while rx.recv().await.is_some() {
                                let alive = weak
                                    .update(app, |this, cx| {
                                        if this.drain_native_bridge_events() {
                                            cx.notify();
                                        }
                                    })
                                    .is_ok();
                                if !alive {
                                    break;
                                }
                            }
                        })
                        .detach();
                    }
                }
            }
            let had_events = self.drain_native_bridge_events();
            let resolved_url = self.resolved_webview_url();
            if let Some(ref mut wv) = self.macos_webview {
                if resolved_url.starts_with("http://") || resolved_url.starts_with("https://") {
                    let same_url = wv.loaded_url() == resolved_url;
                    if !same_url {
                        self.webmcp_ready_probe_sent = false;
                    }
                    wv.load_url(&resolved_url);
                    if same_url && !self.webmcp_page_ready && !self.webmcp_ready_probe_sent {
                        self.webmcp_ready_probe_sent = true;
                        wv.evaluate_js(
                            "try{window.__GPUI_WEBMCP__&&typeof window.__GPUI_WEBMCP__.checkReady==='function'&&window.__GPUI_WEBMCP__.checkReady();}catch(_){}",
                        );
                    }
                }
                if visible {
                    let zoom = if self.is_floating_pip && !self.is_open {
                        0.78
                    } else {
                        1.0
                    };
                    wv.set_page_zoom(zoom);
                    wv.sync_bounds(bounds, true);
                } else if self.is_floating_pip {
                    wv.park_for_pip();
                } else {
                    wv.sync_bounds(bounds, false);
                }
            }
            self.flush_pending_webmcp_queue();
            had_events
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (window, bounds, visible, cx);
            false
        }
    }

    /// Processes an incoming Telegram WebApp JS bridge event (`TelegramWebviewProxy.postEvent`).
    pub fn handle_bridge_event(&mut self, event_type: &str, event_data: &str) {
        let parsed: serde_json::Value = serde_json::from_str(event_data).unwrap_or_default();
        match event_type.trim() {
            "web_app_request_write_access" => {
                if self.allows_write_to_pm {
                    self.dispatch_js_event("write_access_requested", r#"{"status":"allowed"}"#);
                } else {
                    self.write_access_prompt_open = true;
                }
            }
            "web_app_setup_main_button" => {
                apply_setup_button_json(&mut self.ui_schema.main_button, &parsed);
            }
            "web_app_setup_secondary_button" => {
                apply_setup_button_json(&mut self.ui_schema.secondary_button, &parsed);
            }
            "web_app_setup_back_button" => {
                if let Some(vis) = parsed.get("is_visible").and_then(|v| v.as_bool()) {
                    self.ui_schema.back_button_visible = vis;
                }
            }
            "web_app_setup_settings_button" => {
                if let Some(vis) = parsed.get("is_visible").and_then(|v| v.as_bool()) {
                    self.ui_schema.settings_button_visible = vis;
                }
            }
            "web_app_setup_closing_behavior" => {
                if let Some(need) = parsed.get("need_confirmation").and_then(|v| v.as_bool()) {
                    self.closing_confirmation = need;
                }
            }
            "web_app_open_link" => {
                if let Some(url) = parsed.get("url").and_then(|v| v.as_str()) {
                    let clean = url.trim();
                    if clean.starts_with("http://") || clean.starts_with("https://") {
                        self.pending_actions.push_back(
                            BotWebViewPendingAction::OpenInBrowser {
                                path: clean.to_string(),
                            },
                        );
                    }
                }
            }
            "web_app_open_tg_link" => {
                let path_full = parsed
                    .get("path_full")
                    .or_else(|| parsed.get("url"))
                    .or_else(|| parsed.get("path"))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .unwrap_or("");
                if !path_full.is_empty() {
                    self.pending_actions
                        .push_back(BotWebViewPendingAction::OpenTelegramLink(
                            path_full.to_string(),
                        ));
                }
            }
            "web_app_open_popup" => {
                let title = parsed
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let message = parsed
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let buttons = parsed
                    .get("buttons")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|item| {
                                let id = item
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let btn_type = item
                                    .get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("default")
                                    .to_string();
                                let text = item
                                    .get("text")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .trim()
                                    .to_string();
                                let label = if !text.is_empty() {
                                    text
                                } else {
                                    match btn_type.as_str() {
                                        "ok" => "OK".to_string(),
                                        "cancel" => "Cancel".to_string(),
                                        "close" => "Close".to_string(),
                                        _ => String::new(),
                                    }
                                };
                                if label.is_empty() {
                                    None
                                } else {
                                    Some(MiniAppPopupButton {
                                        id: if id.is_empty() { btn_type.clone() } else { id },
                                        btn_type,
                                        text: label,
                                    })
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let buttons = if buttons.is_empty() {
                    vec![MiniAppPopupButton {
                        id: "ok".to_string(),
                        btn_type: "ok".to_string(),
                        text: "OK".to_string(),
                    }]
                } else {
                    buttons
                };
                self.custom_popup = Some(MiniAppPopupState {
                    title,
                    message,
                    buttons,
                });
            }
            "web_app_data_send" => {
                let data = parsed
                    .get("data")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| event_data.to_string());
                self.pending_actions
                    .push_back(BotWebViewPendingAction::SendAppAction {
                        bot_username: self.bot_username.clone(),
                        short_name: self.current_short_name(),
                        req: MiniAppActionRequest::send_data(data, self.target_chat_i64()),
                    });
            }
            "web_app_close" => {
                self.close();
                self.pending_actions.push_back(BotWebViewPendingAction::Close);
            }
            "web_app_set_header_color" => {
                if let Some(color) = parsed.get("color").and_then(|v| v.as_str()) {
                    let clean = color.trim();
                    self.header_color_override = (!clean.is_empty()).then(|| clean.to_string());
                }
            }
            "web_app_set_background_color" => {
                if let Some(color) = parsed.get("color").and_then(|v| v.as_str()) {
                    let clean = color.trim();
                    self.background_color_override = (!clean.is_empty()).then(|| clean.to_string());
                }
            }
            "web_app_ready" => {}
            "__gpui_webmcp_page_ready" | "gpui_webmcp_ready" | "iframe_ready" => {
                if parsed
                    .get("host_wrapper_only")
                    .and_then(|v| v.as_bool())
                    == Some(true)
                {
                    return;
                }
                self.webmcp_page_ready = true;
                self.webmcp_iframe_ready = true;
                self.webmcp_ready_probe_sent = false;
                if let Some(tools_val) = parsed.get("tools") {
                    if let Ok(tools) =
                        serde_json::from_value::<Vec<WebMcpToolDescriptor>>(tools_val.clone())
                    {
                        self.webmcp_discovered_tools = tools;
                    }
                }
                if let Some(summary) = parsed.get("state_summary").filter(|v| v.is_object()) {
                    self.last_webmcp_state_summary = Some(summary.clone());
                }
                let should_rediscover =
                    self.webmcp_needs_rediscover || self.webmcp_discovered_tools.is_empty();
                self.webmcp_needs_rediscover = false;
                if should_rediscover
                    && self
                        .pending_webmcp_queue
                        .front()
                        .is_none_or(|c| c.operation != "discover")
                {
                    #[cfg(target_os = "macos")]
                    if !MacOsWkWebView::is_headless_or_offline_test_env() {
                        if let Some(ref wv) = self.macos_webview {
                            self.webmcp_internal_seq = self.webmcp_internal_seq.wrapping_add(1);
                            let req_id =
                                format!("__gpui_internal_discover_{}", self.webmcp_internal_seq);
                            let req_lit = serde_json::to_string(&req_id)
                                .unwrap_or_else(|_| "\"__gpui_internal_discover_0\"".to_string());
                            let js = format!(
                                "(function(){{try{{if(window.__GPUI_WEBMCP__&&typeof window.__GPUI_WEBMCP__.invoke==='function'){{window.__GPUI_WEBMCP__.invoke({req_lit},'discover',null,{{}});}}}}catch(_){{}}}})();"
                            );
                            wv.evaluate_js(&js);
                        }
                    }
                }
                self.flush_pending_webmcp_queue();
            }
            "__gpui_webmcp_page_unready" | "gpui_webmcp_unready" => {
                self.webmcp_page_ready = false;
                self.webmcp_iframe_ready = false;
                self.webmcp_discovered_tools.clear();
                self.webmcp_needs_rediscover = true;
                self.webmcp_ready_probe_sent = false;
            }
            "gpui_webmcp_tools_changed" => {
                if let Some(tools_val) = parsed.get("tools") {
                    if let Ok(tools) =
                        serde_json::from_value::<Vec<WebMcpToolDescriptor>>(tools_val.clone())
                    {
                        self.webmcp_discovered_tools = tools;
                    }
                }
                self.webmcp_page_ready = true;
                self.webmcp_iframe_ready = true;
                self.flush_pending_webmcp_queue();
            }
            "webmcp_state_update" => {
                let summary = parsed.get("state_summary").cloned().unwrap_or(parsed);
                if summary.is_object() {
                    self.last_webmcp_state_summary = Some(summary);
                } else if let Some(s) = summary.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    self.last_webmcp_state_summary = Some(serde_json::json!({ "status_text": s }));
                }
            }
            "web_app_cloud_storage_set" => {
                if let Some(key) = parsed
                    .get("key")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    let value = parsed
                        .get("value")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    self.pending_actions
                        .push_back(BotWebViewPendingAction::CloudStorageSet {
                            bot_username: self.bot_username.clone(),
                            short_name: self.current_short_name(),
                            key: key.to_string(),
                            value,
                        });
                }
            }
            "web_app_cloud_storage_delete" => {
                if let Some(key) = parsed
                    .get("key")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    self.pending_actions
                        .push_back(BotWebViewPendingAction::CloudStorageDelete {
                            bot_username: self.bot_username.clone(),
                            short_name: self.current_short_name(),
                            key: key.to_string(),
                        });
                }
            }
            "webmcp_response" => {
                if let Ok(resp) = serde_json::from_str::<MiniAppWebMcpResponse>(event_data) {
                    if !resp.tools.is_empty() {
                        self.webmcp_discovered_tools = resp.tools.clone();
                    }
                    if let Some(ref summary) = resp.state_summary {
                        self.last_webmcp_state_summary = Some(summary.clone());
                    }
                    if resp.request_id.starts_with("__gpui_internal_discover_") {
                        return;
                    }
                    self.active_webmcp_tool = None;
                    if resp.ok {
                        self.control_status_text = "AI 操作已完成，等待下一步指令".to_string();
                    } else if let Some(ref err) = resp.error {
                        self.control_status_text = format!("操作提示: {err}");
                    }
                    self.pending_actions
                        .push_back(BotWebViewPendingAction::WebMcpResponseReady(resp));
                }
            }
            _ => {}
        }
    }
}

fn apply_setup_button_json(slot: &mut Option<MiniAppActionButton>, parsed: &serde_json::Value) {
    let btn = slot.get_or_insert_with(|| MiniAppActionButton {
        is_active: true,
        ..Default::default()
    });
    if let Some(v) = parsed.get("is_visible").and_then(|v| v.as_bool()) {
        btn.is_visible = v;
    }
    if let Some(v) = parsed.get("is_active").and_then(|v| v.as_bool()) {
        btn.is_active = v;
    }
    if let Some(v) = parsed.get("is_progress_visible").and_then(|v| v.as_bool()) {
        btn.is_progress_visible = v;
    }
    if let Some(text) = parsed.get("text").and_then(|v| v.as_str()) {
        btn.text = text.to_string();
    }
    if let Some(color_val) = parsed.get("color") {
        if color_val.is_null() {
            btn.color = None;
        } else if let Some(s) = color_val.as_str() {
            let trimmed = s.trim();
            btn.color = (!trimmed.is_empty()).then(|| trimmed.to_string());
        }
    }
    if let Some(tc_val) = parsed.get("text_color") {
        if tc_val.is_null() {
            btn.text_color = None;
        } else if let Some(s) = tc_val.as_str() {
            let trimmed = s.trim();
            btn.text_color = (!trimmed.is_empty()).then(|| trimmed.to_string());
        }
    }
}

fn percent_encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        let is_unreserved = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if is_unreserved {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    out
}

impl Default for BotAttachWebView {
    fn default() -> Self {
        Self::new()
    }
}
