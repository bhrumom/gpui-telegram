use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::remote_control_contracts::{
    AssistCodeMode, AssistStartRequest, McpOAuthGrantSummaryDto, RemoteAssistConfigDto,
    RemoteControlMode, RemoteSessionDto,
};
use gpui_telegram_proto::{
    AuthorizedControllerEntry, AuthorizedControllerKind, CloudAuthMethod, ComputerNodeDto,
    ComputerNodeKind, ComputerRunPolicy, TailnetNodeStatus, TailnetPeerLiveState, TailnetRelayMode,
    UpdateComputerPolicyRequest, UpsertCloudComputerRequest,
};

use crate::app::cli_installer::CliInstallStatus;
use crate::components::macos_permission_modal::{
    render_gemini_permission_modal, render_permission_rows_card,
};
use crate::components::ui::{
    TextInputController, delete_prev_word, pop_last_char, sanitize_paste_text,
};
use crate::i18n::tr_str;
use crate::platform::mac::macos_permissions_drag::{
    ScreenPermissionState, cached_permissions_snapshot, hide_drag_hud,
    permissions_snapshot_generation, request_permissions_refresh,
};
use crate::theme::assets::{IconName, tg_icon};
use crate::theme::palette::TdesktopThemePalette;
use crate::views::computer_workbench::{decode_data_url_image, render_ime_text_preview};
use crate::views::settings::settings_theme_ui::{
    settings_card, settings_container, settings_subsection_title,
};

const UU_BLUE: u32 = 0x1E6BFF;
const UU_BLUE_HOVER: u32 = 0x1557DB;
const UU_ONLINE_GREEN: u32 = 0x22C55E;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SettingsComputersPage {
    #[default]
    Overview,
    Detail(String),
    CloudForm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsComputersTab {
    #[default]
    MyDevices,
    RemoteAssist,
    CloudComputers,
    AiMcpHub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssistInputField {
    #[default]
    None,
    CustomCode,
    TargetAssistId,
    TargetAccessCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpClientPreset {
    Antigravity,
    Claude,
    Cursor,
    VsCode,
}

impl McpClientPreset {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Antigravity => "Antigravity",
            Self::Claude => "Claude",
            Self::Cursor => "Cursor",
            Self::VsCode => "VS Code",
        }
    }
}

pub fn format_assist_id_display(assist_id: &str) -> String {
    let digits: String = assist_id.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 9 {
        format!("{} {} {}", &digits[0..3], &digits[3..6], &digits[6..9])
    } else if assist_id.trim().is_empty() {
        "--- --- ---".to_string()
    } else {
        assist_id.to_string()
    }
}

pub fn is_valid_custom_assist_code(code: &str) -> bool {
    let trimmed = code.trim();
    let len = trimmed.len();
    (8..=16).contains(&len) && trimmed.bytes().all(|b| b.is_ascii_alphanumeric())
}

pub fn build_mcp_client_config_json(preset: McpClientPreset, endpoint_url: &str) -> String {
    let url = if endpoint_url.trim().is_empty() {
        "https://mcp.ombhrum.com/mcp"
    } else {
        endpoint_url.trim()
    };
    match preset {
        McpClientPreset::Antigravity => serde_json::json!({
            "mcpServers": {
                "fabushi": {
                    "serverUrl": url
                }
            }
        }),
        McpClientPreset::Claude => serde_json::json!({
            "mcpServers": {
                "fabushi": {
                    "type": "http",
                    "url": url
                }
            }
        }),
        McpClientPreset::Cursor => serde_json::json!({
            "mcpServers": {
                "fabushi": {
                    "url": url
                }
            }
        }),
        McpClientPreset::VsCode => serde_json::json!({
            "servers": {
                "fabushi": {
                    "type": "http",
                    "url": url
                }
            }
        }),
    }
    .to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CloudFormField {
    #[default]
    None,
    DisplayName,
    Host,
    Port,
    Username,
    Credential,
    AgentUrl,
    Workdir,
}

#[derive(Debug, Clone)]
pub enum SettingsComputersEvent {
    SelectNode(String),
    BackToOverview,
    OpenCreateCloudModal,
    SaveCloudServer(UpsertCloudComputerRequest),
    UpdatePolicy {
        node_id: String,
        req: UpdateComputerPolicyRequest,
    },
    DeleteNode(String),
    TestNode(String),
    OpenRemoteWorkbench(String),
    OpenRemoteTerminal(String),
    RefreshComputers,
    RefreshAssistInfo,
    ResetAssistTempCode,
    SetAssistCustomCode(String),
    SetAssistAllow(bool),
    CopyAssistInvite {
        invite_text: String,
    },
    StartAssistConnect(AssistStartRequest),
    DisconnectAllRemoteControl,
    RefreshMcpGrants,
    RevokeMcpGrant(String),
    CopyMcpClientConfig {
        client_name: String,
        config_json: String,
    },
    RefreshCliStatus,
    InstallFabushiCli,
    CloseRequested,
    BackToSettings,
}

pub struct SettingsComputersSections {
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub standalone_mode: bool,
    pub account_label: String,
    pub page: SettingsComputersPage,
    pub overview_tab: SettingsComputersTab,
    pub computers: Vec<ComputerNodeDto>,
    pub thumbnail_cache: HashMap<String, (String, Arc<gpui::Image>)>,
    pub tailnet_status: Option<TailnetNodeStatus>,
    pub known_bots: Vec<(u64, String)>,
    pub current_bot_id: Option<u64>,
    pub status_message: Option<String>,
    pub policy_menu_open: bool,
    pub device_picker_open: bool,
    pub bot_picker_open: bool,
    pub detail_more_open: bool,
    pub pending_event: Option<SettingsComputersEvent>,

    // Remote Control / Assist / AI MCP / CLI Hub state
    pub active_sessions: Vec<RemoteSessionDto>,
    pub assist_config: Option<RemoteAssistConfigDto>,
    pub assist_focus: AssistInputField,
    pub assist_custom_code_draft: String,
    pub assist_target_id_draft: String,
    pub assist_target_code_draft: String,
    pub input_assist_custom_code: Option<Entity<TextInputController>>,
    pub input_assist_target_id: Option<Entity<TextInputController>>,
    pub input_assist_target_code: Option<Entity<TextInputController>>,
    pub mcp_endpoint_url: String,
    pub mcp_grants: Vec<McpOAuthGrantSummaryDto>,
    pub cli_status: Option<CliInstallStatus>,

    // Cloud Server Form state + TextInputControllers
    pub editing_node_id: Option<String>,
    pub form_focus: CloudFormField,
    pub form_session_gen: u64,
    pub form_name: String,
    pub form_host: String,
    pub form_port: String,
    pub form_username: String,
    pub form_auth_method: CloudAuthMethod,
    pub form_credential: String,
    pub form_agent_url: String,
    pub form_workdir: String,
    pub form_bind_to_current_bot: bool,

    pub input_name: Option<Entity<TextInputController>>,
    pub input_host: Option<Entity<TextInputController>>,
    pub input_port: Option<Entity<TextInputController>>,
    pub input_username: Option<Entity<TextInputController>>,
    pub input_credential: Option<Entity<TextInputController>>,
    pub input_agent_url: Option<Entity<TextInputController>>,
    pub input_workdir: Option<Entity<TextInputController>>,

    // macOS Gemini-style permission setup modal & live polling state
    pub permission_modal_open: bool,
    permission_poll_started: bool,
    last_perm_gen: u64,
    last_perm_screen_state: ScreenPermissionState,
    last_perm_ax_active: bool,
    last_perm_hud_visible: bool,
    last_perm_stale_screen: bool,
    last_perm_stale_ax: bool,
    last_perm_drag_seq: u64,
}

impl SettingsComputersSections {
    pub fn new_live(palette: TdesktopThemePalette, lang: &str) -> Self {
        Self {
            palette,
            language_id: lang.to_string(),
            standalone_mode: false,
            account_label: String::new(),
            page: SettingsComputersPage::Overview,
            overview_tab: SettingsComputersTab::MyDevices,
            computers: Vec::new(),
            thumbnail_cache: HashMap::new(),
            tailnet_status: None,
            known_bots: Vec::new(),
            current_bot_id: None,
            status_message: None,
            policy_menu_open: false,
            device_picker_open: false,
            bot_picker_open: false,
            detail_more_open: false,
            pending_event: None,
            active_sessions: Vec::new(),
            assist_config: None,
            assist_focus: AssistInputField::None,
            assist_custom_code_draft: String::new(),
            assist_target_id_draft: String::new(),
            assist_target_code_draft: String::new(),
            input_assist_custom_code: None,
            input_assist_target_id: None,
            input_assist_target_code: None,
            mcp_endpoint_url: "https://mcp.ombhrum.com/mcp".to_string(),
            mcp_grants: Vec::new(),
            cli_status: None,
            editing_node_id: None,
            form_focus: CloudFormField::None,
            form_session_gen: 0,
            form_name: String::new(),
            form_host: String::new(),
            form_port: "22".to_string(),
            form_username: "root".to_string(),
            form_auth_method: CloudAuthMethod::SshPassword,
            form_credential: String::new(),
            form_agent_url: String::new(),
            form_workdir: "/workspace".to_string(),
            form_bind_to_current_bot: false,
            input_name: None,
            input_host: None,
            input_port: None,
            input_username: None,
            input_credential: None,
            input_agent_url: None,
            input_workdir: None,
            permission_modal_open: false,
            permission_poll_started: false,
            last_perm_gen: 0,
            last_perm_screen_state: ScreenPermissionState::NotGranted,
            last_perm_ax_active: false,
            last_perm_hud_visible: false,
            last_perm_stale_screen: false,
            last_perm_stale_ax: false,
            last_perm_drag_seq: 0,
        }
    }

    fn ensure_permission_poll_loop(&mut self, cx: &mut Context<Self>) {
        if self.permission_poll_started
            || !cfg!(target_os = "macos")
            || cfg!(test)
            || std::env::var_os("GPUI_OFFLINE_TEST").is_some()
        {
            return;
        }
        self.permission_poll_started = true;
        let init_snap = cached_permissions_snapshot();
        self.last_perm_gen = permissions_snapshot_generation();
        self.last_perm_screen_state = init_snap.screen_state;
        self.last_perm_ax_active = init_snap.accessibility_granted;
        self.last_perm_hud_visible = init_snap.hud_visible;
        self.last_perm_stale_screen = init_snap.stale_screen_suspected;
        self.last_perm_stale_ax = init_snap.stale_ax_suspected;
        self.last_perm_drag_seq = init_snap.last_drag_seq;
        if init_snap.any_missing() {
            request_permissions_refresh(true);
        }

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| loop {
            app.background_executor()
                .timer(Duration::from_millis(450))
                .await;
            let keep = weak
                .update(app, |this, cx| {
                    let before_snap = cached_permissions_snapshot();
                    let perm_ui_visible = this.permission_modal_open
                        || (this.page == SettingsComputersPage::Overview
                            && this.overview_tab == SettingsComputersTab::MyDevices);
                    if perm_ui_visible || before_snap.hud_visible {
                        if before_snap.any_missing() || before_snap.hud_visible {
                            request_permissions_refresh(false);
                        }
                        let snap = cached_permissions_snapshot();
                        let perm_gen = permissions_snapshot_generation();
                        if perm_gen != this.last_perm_gen
                            || snap.screen_state != this.last_perm_screen_state
                            || snap.accessibility_granted != this.last_perm_ax_active
                            || snap.hud_visible != this.last_perm_hud_visible
                            || snap.stale_screen_suspected != this.last_perm_stale_screen
                            || snap.stale_ax_suspected != this.last_perm_stale_ax
                            || snap.last_drag_seq != this.last_perm_drag_seq
                        {
                            this.last_perm_gen = perm_gen;
                            this.last_perm_screen_state = snap.screen_state;
                            this.last_perm_ax_active = snap.accessibility_granted;
                            this.last_perm_hud_visible = snap.hud_visible;
                            this.last_perm_stale_screen = snap.stale_screen_suspected;
                            this.last_perm_stale_ax = snap.stale_ax_suspected;
                            this.last_perm_drag_seq = snap.last_drag_seq;
                            cx.notify();
                        }
                    }
                    true
                })
                .unwrap_or(false);
            if !keep {
                break;
            }
        })
        .detach();
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.palette = palette;
    }

    pub fn set_language(&mut self, lang: &str) {
        self.language_id = lang.to_string();
    }

    pub fn sync_computers(&mut self, computers: Vec<ComputerNodeDto>) {
        self.thumbnail_cache
            .retain(|nid, _| computers.iter().any(|c| c.node_id == *nid));
        for node in &computers {
            match node.latest_frame_data_url.as_deref() {
                Some(url) if !url.is_empty() => {
                    let unchanged = self
                        .thumbnail_cache
                        .get(&node.node_id)
                        .is_some_and(|(cached_url, _)| cached_url == url);
                    if !unchanged {
                        if let Some(img) = decode_data_url_image(url) {
                            self.thumbnail_cache
                                .insert(node.node_id.clone(), (url.to_string(), img));
                        }
                    }
                }
                _ => {
                    self.thumbnail_cache.remove(&node.node_id);
                }
            }
        }
        self.computers = computers;
    }

    pub fn sync_account_label(&mut self, label: String) {
        self.account_label = label;
    }

    pub fn sync_tailnet_status(&mut self, status: Option<TailnetNodeStatus>) {
        self.tailnet_status = status;
    }

    pub fn sync_known_bots(&mut self, bots: Vec<(u64, String)>, current_bot_id: Option<u64>) {
        self.known_bots = bots;
        self.current_bot_id = current_bot_id;
    }

    pub fn sync_active_sessions(&mut self, sessions: Vec<RemoteSessionDto>) {
        self.active_sessions = sessions;
    }

    pub fn sync_assist_config(&mut self, config: Option<RemoteAssistConfigDto>) {
        self.assist_config = config;
    }

    pub fn sync_mcp_grants(
        &mut self,
        endpoint_url: Option<String>,
        grants: Vec<McpOAuthGrantSummaryDto>,
    ) {
        if let Some(url) = endpoint_url.filter(|u| !u.trim().is_empty()) {
            self.mcp_endpoint_url = url;
        }
        self.mcp_grants = grants;
    }

    pub fn sync_cli_status(&mut self, status: Option<CliInstallStatus>) {
        self.cli_status = status;
    }

    pub fn set_overview_tab(&mut self, tab: SettingsComputersTab) {
        self.overview_tab = tab;
        self.assist_focus = AssistInputField::None;
        match tab {
            SettingsComputersTab::RemoteAssist => {
                if self.assist_config.is_none() {
                    self.pending_event = Some(SettingsComputersEvent::RefreshAssistInfo);
                }
            }
            SettingsComputersTab::AiMcpHub => {
                if self.mcp_grants.is_empty() {
                    self.pending_event = Some(SettingsComputersEvent::RefreshMcpGrants);
                }
            }
            _ => {}
        }
    }

    pub fn formatted_assist_invite_text(&self) -> String {
        let cfg = self.assist_config.as_ref();
        let raw_id = cfg.map(|c| c.assist_id.as_str()).unwrap_or("");
        let id = format_assist_id_display(raw_id);
        let code = cfg
            .and_then(|c| c.current_code.as_deref())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("------");
        if self.language_id.starts_with("zh") {
            format!("Fabushi 远程协助设备码: {id}  验证码: {code}")
        } else {
            format!("Fabushi Remote Assist ID: {id}  Code: {code}")
        }
    }

    pub fn submit_assist_custom_code(&mut self) -> bool {
        let code = self.assist_custom_code_draft.trim().to_string();
        if !is_valid_custom_assist_code(&code) {
            self.status_message = Some(
                tr_str(
                    &self.language_id,
                    "自定义验证码需为 8-16 位数字或字母",
                    "Custom access code must be 8-16 alphanumeric characters",
                )
                .to_string(),
            );
            return false;
        }
        self.assist_focus = AssistInputField::None;
        self.status_message = None;
        self.pending_event = Some(SettingsComputersEvent::SetAssistCustomCode(code));
        true
    }

    pub fn submit_assist_connect(&mut self) -> bool {
        let digits: String = self
            .assist_target_id_draft
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect();
        let code = self.assist_target_code_draft.trim().to_string();
        if digits.len() != 9 || code.is_empty() {
            self.status_message = Some(
                tr_str(
                    &self.language_id,
                    "请输入 9 位设备协助码和验证码",
                    "Please enter a 9-digit Assist ID and access code",
                )
                .to_string(),
            );
            return false;
        }
        self.assist_focus = AssistInputField::None;
        self.status_message = None;
        self.pending_event = Some(SettingsComputersEvent::StartAssistConnect(
            AssistStartRequest {
                assist_id: digits,
                access_code: code,
                requester_device_id: None,
                requester_name: None,
                requester_platform: None,
                mode: RemoteControlMode::Desktop,
            },
        ));
        true
    }

    pub fn trigger_copy_mcp_preset(&mut self, preset: McpClientPreset) {
        let json = build_mcp_client_config_json(preset, &self.mcp_endpoint_url);
        let name = preset.display_name().to_string();
        self.status_message = Some(format!(
            "{} {}",
            tr_str(&self.language_id, "已复制 MCP 配置：", "Copied MCP config for"),
            name
        ));
        self.pending_event = Some(SettingsComputersEvent::CopyMcpClientConfig {
            client_name: name,
            config_json: json,
        });
    }

    pub fn open_overview(&mut self) {
        self.page = SettingsComputersPage::Overview;
        self.form_focus = CloudFormField::None;
        self.assist_focus = AssistInputField::None;
        self.policy_menu_open = false;
        self.device_picker_open = false;
        self.bot_picker_open = false;
        self.detail_more_open = false;
    }

    pub fn open_detail(&mut self, node_id: &str) {
        self.page = SettingsComputersPage::Detail(node_id.to_string());
        self.form_focus = CloudFormField::None;
        self.assist_focus = AssistInputField::None;
        self.policy_menu_open = false;
        self.device_picker_open = false;
        self.bot_picker_open = false;
        self.detail_more_open = false;
    }

    pub fn open_cloud_form(
        &mut self,
        bind_bot_id: Option<u64>,
        edit_node: Option<&ComputerNodeDto>,
    ) {
        self.page = SettingsComputersPage::CloudForm;
        self.form_focus = CloudFormField::DisplayName;
        self.form_session_gen = self.form_session_gen.wrapping_add(1);
        self.assist_focus = AssistInputField::None;
        self.policy_menu_open = false;
        self.device_picker_open = false;
        self.bot_picker_open = false;
        self.detail_more_open = false;
        if let Some(id) = bind_bot_id {
            self.current_bot_id = Some(id);
            self.form_bind_to_current_bot = true;
        } else {
            self.form_bind_to_current_bot = false;
        }

        if let Some(node) = edit_node {
            self.editing_node_id = Some(node.node_id.clone());
            self.form_name = node.display_name.clone();
            self.form_host = node.host.clone().unwrap_or_else(|| node.ip_address.clone());
            self.form_port = node.ssh_port.unwrap_or(22).to_string();
            self.form_username = node.ssh_username.clone().unwrap_or_else(|| "root".to_string());
            self.form_auth_method = node.auth_method.unwrap_or(CloudAuthMethod::SshPassword);
            self.form_credential.clear();
            self.form_agent_url = node.agent_endpoint_url.clone().unwrap_or_default();
            self.form_workdir = node.workdir.clone().unwrap_or_else(|| "/workspace".to_string());
        } else {
            self.editing_node_id = None;
            self.form_name.clear();
            self.form_host.clear();
            self.form_port = "22".to_string();
            self.form_username = "root".to_string();
            self.form_auth_method = CloudAuthMethod::SshPassword;
            self.form_credential.clear();
            self.form_agent_url.clear();
            self.form_workdir = "/workspace".to_string();
        }
    }

    pub fn take_event(&mut self) -> Option<SettingsComputersEvent> {
        self.pending_event.take()
    }

    fn ensure_form_inputs(&mut self, cx: &mut Context<Self>) {
        if self.input_name.is_none() {
            self.input_name = Some(cx.new(|_| TextInputController::new()));
        }
        if self.input_host.is_none() {
            self.input_host = Some(cx.new(|_| TextInputController::new()));
        }
        if self.input_port.is_none() {
            self.input_port = Some(cx.new(|_| TextInputController::new()));
        }
        if self.input_username.is_none() {
            self.input_username = Some(cx.new(|_| TextInputController::new()));
        }
        if self.input_credential.is_none() {
            self.input_credential = Some(cx.new(|_| TextInputController::new()));
        }
        if self.input_agent_url.is_none() {
            self.input_agent_url = Some(cx.new(|_| TextInputController::new()));
        }
        if self.input_workdir.is_none() {
            self.input_workdir = Some(cx.new(|_| TextInputController::new()));
        }
        if self.input_assist_custom_code.is_none() {
            self.input_assist_custom_code = Some(cx.new(|_| TextInputController::new()));
        }
        if self.input_assist_target_id.is_none() {
            self.input_assist_target_id = Some(cx.new(|_| TextInputController::new()));
        }
        if self.input_assist_target_code.is_none() {
            self.input_assist_target_code = Some(cx.new(|_| TextInputController::new()));
        }
    }

    pub fn is_input_active(&self) -> bool {
        (self.page == SettingsComputersPage::CloudForm && self.form_focus != CloudFormField::None)
            || (self.page == SettingsComputersPage::Overview
                && self.overview_tab == SettingsComputersTab::RemoteAssist
                && self.assist_focus != AssistInputField::None)
    }

    fn active_controller(&self) -> Option<&Entity<TextInputController>> {
        if self.page == SettingsComputersPage::CloudForm {
            return match self.form_focus {
                CloudFormField::DisplayName => self.input_name.as_ref(),
                CloudFormField::Host => self.input_host.as_ref(),
                CloudFormField::Port => self.input_port.as_ref(),
                CloudFormField::Username => self.input_username.as_ref(),
                CloudFormField::Credential => self.input_credential.as_ref(),
                CloudFormField::AgentUrl => self.input_agent_url.as_ref(),
                CloudFormField::Workdir => self.input_workdir.as_ref(),
                CloudFormField::None => None,
            };
        }
        if self.page == SettingsComputersPage::Overview
            && self.overview_tab == SettingsComputersTab::RemoteAssist
        {
            return match self.assist_focus {
                AssistInputField::CustomCode => self.input_assist_custom_code.as_ref(),
                AssistInputField::TargetAssistId => self.input_assist_target_id.as_ref(),
                AssistInputField::TargetAccessCode => self.input_assist_target_code.as_ref(),
                AssistInputField::None => None,
            };
        }
        None
    }

    pub fn active_field_text(&self) -> &str {
        if self.page == SettingsComputersPage::CloudForm {
            return match self.form_focus {
                CloudFormField::DisplayName => &self.form_name,
                CloudFormField::Host => &self.form_host,
                CloudFormField::Port => &self.form_port,
                CloudFormField::Username => &self.form_username,
                CloudFormField::Credential => &self.form_credential,
                CloudFormField::AgentUrl => &self.form_agent_url,
                CloudFormField::Workdir => &self.form_workdir,
                CloudFormField::None => "",
            };
        }
        if self.page == SettingsComputersPage::Overview
            && self.overview_tab == SettingsComputersTab::RemoteAssist
        {
            return match self.assist_focus {
                AssistInputField::CustomCode => &self.assist_custom_code_draft,
                AssistInputField::TargetAssistId => &self.assist_target_id_draft,
                AssistInputField::TargetAccessCode => &self.assist_target_code_draft,
                AssistInputField::None => "",
            };
        }
        ""
    }

    fn active_field_mut(&mut self) -> Option<&mut String> {
        if self.page == SettingsComputersPage::CloudForm {
            return match self.form_focus {
                CloudFormField::DisplayName => Some(&mut self.form_name),
                CloudFormField::Host => Some(&mut self.form_host),
                CloudFormField::Port => Some(&mut self.form_port),
                CloudFormField::Username => Some(&mut self.form_username),
                CloudFormField::Credential => Some(&mut self.form_credential),
                CloudFormField::AgentUrl => Some(&mut self.form_agent_url),
                CloudFormField::Workdir => Some(&mut self.form_workdir),
                CloudFormField::None => None,
            };
        }
        if self.page == SettingsComputersPage::Overview
            && self.overview_tab == SettingsComputersTab::RemoteAssist
        {
            return match self.assist_focus {
                AssistInputField::CustomCode => Some(&mut self.assist_custom_code_draft),
                AssistInputField::TargetAssistId => Some(&mut self.assist_target_id_draft),
                AssistInputField::TargetAccessCode => Some(&mut self.assist_target_code_draft),
                AssistInputField::None => None,
            };
        }
        None
    }

    pub fn active_input_text_and_preedit(&self, cx: &App) -> (String, Option<String>) {
        let committed = self.active_field_text().to_string();
        if let Some(ctrl) = self.active_controller() {
            ctrl.read(cx).text_and_preedit(committed)
        } else {
            (committed, None)
        }
    }

    pub fn clear_active_input(&mut self, cx: &mut Context<Self>) {
        if let Some(ctrl) = self.active_controller().cloned() {
            ctrl.update(cx, |c, _| c.clear_preedit());
        }
        if let Some(dst) = self.active_field_mut() {
            dst.clear();
        }
        cx.notify();
    }

    pub fn push_input_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let allow_nl = self.page == SettingsComputersPage::CloudForm
            && self.form_focus == CloudFormField::Credential
            && self.form_auth_method == CloudAuthMethod::SshPrivateKey;
        let clean = sanitize_paste_text(text, allow_nl);
        if clean.is_empty() {
            return;
        }
        if let Some(ctrl) = self.active_controller().cloned() {
            ctrl.update(cx, |c, _| c.clear_preedit());
        }
        if let Some(dst) = self.active_field_mut() {
            dst.push_str(&clean);
        }
        cx.notify();
    }

    pub fn handle_non_char_key(
        &mut self,
        key: &str,
        is_shift: bool,
        is_alt: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.is_input_active() {
            return false;
        }
        match key {
            "backspace" | "delete" => {
                if let Some(dst) = self.active_field_mut() {
                    let changed = if is_alt {
                        delete_prev_word(dst)
                    } else {
                        pop_last_char(dst)
                    };
                    if changed {
                        cx.notify();
                    }
                }
                true
            }
            "tab" => {
                if self.page == SettingsComputersPage::CloudForm {
                    let order = [
                        CloudFormField::DisplayName,
                        CloudFormField::Host,
                        CloudFormField::Port,
                        CloudFormField::Username,
                        CloudFormField::Credential,
                        CloudFormField::AgentUrl,
                        CloudFormField::Workdir,
                    ];
                    if let Some(pos) = order.iter().position(|f| *f == self.form_focus) {
                        let next = if is_shift {
                            (pos + order.len() - 1) % order.len()
                        } else {
                            (pos + 1) % order.len()
                        };
                        self.form_focus = order[next];
                        cx.notify();
                    }
                    return true;
                }
                if self.overview_tab == SettingsComputersTab::RemoteAssist {
                    self.assist_focus = match self.assist_focus {
                        AssistInputField::TargetAssistId => AssistInputField::TargetAccessCode,
                        AssistInputField::TargetAccessCode => AssistInputField::TargetAssistId,
                        other => other,
                    };
                    cx.notify();
                }
                true
            }
            "enter" => {
                if self.page == SettingsComputersPage::CloudForm {
                    self.submit_cloud_form();
                } else if self.overview_tab == SettingsComputersTab::RemoteAssist {
                    match self.assist_focus {
                        AssistInputField::CustomCode => {
                            self.submit_assist_custom_code();
                        }
                        AssistInputField::TargetAssistId | AssistInputField::TargetAccessCode => {
                            self.submit_assist_connect();
                        }
                        AssistInputField::None => {}
                    }
                }
                cx.notify();
                true
            }
            _ => false,
        }
    }

    pub fn replace_and_mark_input(
        &mut self,
        new_text: &str,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.ensure_form_inputs(cx);
        let commit = self
            .active_controller()
            .cloned()
            .and_then(|c| c.update(cx, |ctrl, _| ctrl.handle_replace_and_mark(new_text)));
        if let Some(txt) = commit.as_ref() {
            if let Some(dst) = self.active_field_mut() {
                dst.push_str(txt);
            }
        }
        cx.notify();
        commit
    }

    pub fn set_ime_preedit(&mut self, preedit: Option<&str>, cx: &mut Context<Self>) {
        self.ensure_form_inputs(cx);
        if let Some(ctrl) = self.active_controller().cloned() {
            ctrl.update(cx, |c, _| {
                c.ime_preedit = preedit.filter(|s| !s.is_empty()).map(str::to_string);
            });
            cx.notify();
        }
    }

    pub fn clear_all_preedits(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        let controllers = [
            self.input_name.clone(),
            self.input_host.clone(),
            self.input_port.clone(),
            self.input_username.clone(),
            self.input_credential.clone(),
            self.input_agent_url.clone(),
            self.input_workdir.clone(),
            self.input_assist_custom_code.clone(),
            self.input_assist_target_id.clone(),
            self.input_assist_target_code.clone(),
        ];
        for ctrl in controllers.into_iter().flatten() {
            let needs_clear = {
                let r = ctrl.read(cx);
                r.ime_preedit.is_some() || r.just_committed_ime || r.last_ime_commit_at.is_some()
            };
            if needs_clear {
                ctrl.update(cx, |c, _| {
                    c.clear_ime_state();
                });
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
        changed
    }

    pub fn submit_cloud_form(&mut self) -> bool {
        let host = self.form_host.trim().to_string();
        if host.is_empty() {
            self.status_message = Some(
                tr_str(
                    &self.language_id,
                    "请填写云服务器的主机地址或 IP",
                    "Please enter a cloud host or IP address",
                )
                .to_string(),
            );
            return false;
        }
        let port = self.form_port.trim().parse::<u16>().unwrap_or(22);
        let display_name = if self.form_name.trim().is_empty() {
            host.clone()
        } else {
            self.form_name.trim().to_string()
        };
        let username = if self.form_username.trim().is_empty() {
            "root".to_string()
        } else {
            self.form_username.trim().to_string()
        };
        let secret = (!self.form_credential.trim().is_empty())
            .then(|| self.form_credential.trim().to_string());
        let agent_endpoint_url = (!self.form_agent_url.trim().is_empty())
            .then(|| self.form_agent_url.trim().to_string());
        let workdir = (!self.form_workdir.trim().is_empty())
            .then(|| self.form_workdir.trim().to_string());
        let bind_bot_id = if self.form_bind_to_current_bot {
            self.current_bot_id
        } else {
            None
        };

        self.form_focus = CloudFormField::None;
        self.status_message = None;
        self.pending_event = Some(SettingsComputersEvent::SaveCloudServer(
            UpsertCloudComputerRequest {
                node_id: self.editing_node_id.clone(),
                display_name,
                host,
                port,
                username,
                auth_method: self.form_auth_method,
                secret,
                agent_endpoint_url,
                workdir,
                bind_bot_id,
                verify_connection: true,
            },
        ));
        true
    }

    fn policy_label(policy: ComputerRunPolicy, lang: &str) -> &'static str {
        if lang.starts_with("zh") {
            policy.label_zh()
        } else {
            policy.label_en()
        }
    }

    fn node_endpoint(node: &ComputerNodeDto) -> String {
        let host_opt = node
            .host
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let ip = node.ip_address.trim();
        match (node.kind, host_opt) {
            (ComputerNodeKind::LocalDevice, Some(ts_ip)) if !ip.is_empty() && ts_ip != ip => {
                format!("{ts_ip} · {ip}")
            }
            (_, Some(h)) => h.to_string(),
            (_, None) => ip.to_string(),
        }
    }

    fn find_tailnet_peer(&self, node: &ComputerNodeDto) -> Option<&TailnetPeerLiveState> {
        let ts = self.tailnet_status.as_ref()?;
        ts.peers.iter().find(|p| {
            p.node_id == node.node_id
                || node
                    .device_id
                    .as_deref()
                    .is_some_and(|did| !did.is_empty() && p.device_id == did)
        })
    }

    fn relay_mode_label(mode: TailnetRelayMode, lang: &str) -> &'static str {
        if lang.starts_with("zh") {
            mode.label_zh()
        } else {
            mode.label_en()
        }
    }

    fn format_peer_transport(peer: &TailnetPeerLiveState, lang: &str) -> String {
        let mode = Self::relay_mode_label(peer.relay_mode, lang);
        match peer.rtt_ms {
            Some(ms) => format!("{mode} · {ms}ms"),
            None => mode.to_string(),
        }
    }

    fn render_tailnet_mesh_card(&self, pal: TdesktopThemePalette, lang: &str) -> AnyElement {
        let ts = self.tailnet_status.as_ref();
        let running = ts.is_some_and(|s| s.running);
        let current_local = self
            .computers
            .iter()
            .find(|c| c.kind == ComputerNodeKind::LocalDevice && c.is_current_device);

        let tailnet_ip = ts
            .map(|s| s.tailnet_ip.trim())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                current_local
                    .and_then(|c| c.host.as_deref())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| tr_str(lang, "分配中...", "Assigning..."));

        let lan_ip = ts
            .map(|s| s.lan_ip.trim())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                current_local
                    .map(|c| c.ip_address.trim())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or("--");

        let active_mode = ts
            .map(|s| {
                if s.peers
                    .iter()
                    .any(|p| p.online && p.relay_mode == TailnetRelayMode::DirectUdp)
                {
                    TailnetRelayMode::DirectUdp
                } else {
                    s.relay_mode
                }
            })
            .unwrap_or(TailnetRelayMode::HttpFallback);
        let mode_label = Self::relay_mode_label(active_mode, lang);
        let transport_text = match ts.and_then(|s| s.derp_rtt_ms) {
            Some(rtt) if running => format!("{mode_label} · {rtt}ms"),
            _ => mode_label.to_string(),
        };

        let status_badge = if running {
            tr_str(lang, "已组网 · 内置免服务", "Mesh Active · Zero-Service")
        } else {
            tr_str(lang, "正在连接 Tailnet...", "Connecting Tailnet...")
        };
        let status_color = if running { pal.success } else { pal.accent };
        let peer_count = ts
            .map(|s| s.peers.iter().filter(|p| p.online).count())
            .unwrap_or(0);
        let udp_port = ts.map(|s| s.local_udp_port).unwrap_or(0);
        let udp_text = if udp_port > 0 {
            format!("UDP :{udp_port}")
        } else {
            tr_str(lang, "内置 Mesh", "Embedded Mesh").to_string()
        };

        settings_card(pal)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .w(px(8.0))
                                            .h(px(8.0))
                                            .rounded_full()
                                            .bg(rgb(status_color)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child("Tailscale Mesh"),
                                    )
                                    .child(
                                        div()
                                            .px(px(8.0))
                                            .py(px(2.0))
                                            .rounded(px(999.0))
                                            .bg(TdesktopThemePalette::rgba_with_alpha(
                                                status_color,
                                                0.14,
                                            ))
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(status_color))
                                            .child(status_badge.to_string()),
                                    ),
                            )
                            .child(
                                div()
                                    .px(px(8.0))
                                    .py(px(2.0))
                                    .rounded(px(999.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.12))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(tg_icon(IconName::Lock, 11.0, pal.success))
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.success))
                                            .child("X25519 + ChaCha20-Poly1305"),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(10.0))
                            .text_size(px(11.5))
                            .text_color(rgb(pal.text_secondary))
                            .child(format!("Tailnet IP: {tailnet_ip}"))
                            .child(format!("LAN: {lan_ip}"))
                            .child(format!(
                                "{}: {transport_text}",
                                tr_str(lang, "传输", "Transport")
                            ))
                            .child(udp_text)
                            .when(peer_count > 0, |row| {
                                row.child(format!(
                                    "{}: {peer_count}",
                                    tr_str(lang, "在线节点", "Online Peers")
                                ))
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_uu_sidebar(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_dark = pal.mode.is_dark();
        let sidebar_bg = if is_dark { 0x13161C } else { 0xF4F6FA };
        let nav_hover = if is_dark { 0x1D222C } else { 0xEAEFF7 };
        let nav_active_bg = TdesktopThemePalette::rgba_with_alpha(UU_BLUE, if is_dark { 0.16 } else { 0.10 });

        let local_online_count = self
            .computers
            .iter()
            .filter(|c| {
                c.kind == ComputerNodeKind::LocalDevice
                    && (c.online || self.find_tailnet_peer(c).is_some_and(|p| p.online))
            })
            .count();
        let local_total_count = self
            .computers
            .iter()
            .filter(|c| c.kind == ComputerNodeKind::LocalDevice)
            .count();
        let cloud_total_count = self
            .computers
            .iter()
            .filter(|c| c.kind == ComputerNodeKind::CloudServer)
            .count();
        let mcp_count = self.mcp_grants.len();
        let allow_assist = self.assist_config.as_ref().is_some_and(|c| c.allow_assist);
        let active_sessions_count = self.active_sessions.len();

        let ts = self.tailnet_status.as_ref();
        let ts_running = ts.is_some_and(|s| s.running);
        let ts_ip = ts
            .map(|s| s.tailnet_ip.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("--");
        let ts_mode_lbl = ts
            .map(|s| {
                let active_m = if s
                    .peers
                    .iter()
                    .any(|p| p.online && p.relay_mode == TailnetRelayMode::DirectUdp)
                {
                    TailnetRelayMode::DirectUdp
                } else {
                    s.relay_mode
                };
                match s.derp_rtt_ms {
                    Some(ms) if ts_running => format!("{} · {ms}ms", Self::relay_mode_label(active_m, lang)),
                    _ => Self::relay_mode_label(active_m, lang).to_string(),
                }
            })
            .unwrap_or_else(|| tr_str(lang, "组网中...", "Connecting...").to_string());

        let account_sub = if !self.account_label.trim().is_empty() {
            format!(
                "{} · {}",
                self.account_label.trim(),
                tr_str(lang, "在线", "Online")
            )
        } else {
            tr_str(lang, "本机在线 · 已就绪", "Host Online · Ready").to_string()
        };

        let mac_perm_snap = if cfg!(target_os = "macos") {
            Some(cached_permissions_snapshot())
        } else {
            None
        };

        let nav_items: [(SettingsComputersTab, &'static str, IconName, &str, Option<usize>); 4] = [
            (
                SettingsComputersTab::MyDevices,
                "sc-tab-my-devices",
                IconName::ComputerDesktop,
                tr_str(lang, "我的设备", "My Devices"),
                if local_total_count > 0 {
                    Some(local_online_count.max(local_total_count))
                } else {
                    None
                },
            ),
            (
                SettingsComputersTab::RemoteAssist,
                "sc-tab-remote-assist",
                IconName::ScreenShare,
                tr_str(lang, "远程协助", "Remote Assist"),
                None,
            ),
            (
                SettingsComputersTab::CloudComputers,
                "sc-tab-cloud",
                IconName::FolderWork,
                tr_str(lang, "云电脑", "Cloud PCs"),
                if cloud_total_count > 0 {
                    Some(cloud_total_count)
                } else {
                    None
                },
            ),
            (
                SettingsComputersTab::AiMcpHub,
                "sc-tab-ai-mcp",
                IconName::BotRobot,
                tr_str(lang, "AI & MCP", "AI & MCP"),
                if mcp_count > 0 { Some(mcp_count) } else { None },
            ),
        ];

        div()
            .w(px(210.0))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(sidebar_bg))
            .border_r_1()
            .border_color(pal.separator_rgba())
            .flex()
            .flex_col()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .when(!self.standalone_mode, |col| {
                        col.child(
                            div()
                                .px(px(12.0))
                                .pt(px(12.0))
                                .child(
                                    div()
                                        .id("uu-back-to-settings-btn")
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(5.0))
                                        .rounded(px(8.0))
                                        .hover(move |s| s.bg(rgb(nav_hover)))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.0))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.pending_event =
                                                    Some(SettingsComputersEvent::BackToSettings);
                                                cx.notify();
                                            }),
                                        )
                                        .child(tg_icon(IconName::BackArrow, 12.5, pal.text_secondary))
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(pal.text_secondary))
                                                .child(tr_str(lang, "返回设置", "Back to Settings")),
                                        ),
                                ),
                        )
                    })
                    .child(
                        div()
                            .px(px(14.0))
                            .pt(px(if self.standalone_mode { 18.0 } else { 8.0 }))
                            .pb(px(12.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .w(px(34.0))
                                    .h(px(34.0))
                                    .flex_shrink_0()
                                    .rounded(px(10.0))
                                    .bg(linear_gradient(
                                        135.0,
                                        linear_color_stop(rgb(0x3B82F6), 0.0),
                                        linear_color_stop(rgb(UU_BLUE), 1.0),
                                    ))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(tg_icon(IconName::ComputerDesktop, 18.0, 0xFFFFFF)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .truncate()
                                            .child(tr_str(lang, "UU 远程电脑", "Remote Desktop")),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(5.0))
                                            .child(
                                                div()
                                                    .w(px(6.5))
                                                    .h(px(6.5))
                                                    .flex_shrink_0()
                                                    .rounded_full()
                                                    .bg(rgb(UU_ONLINE_GREEN)),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(pal.text_secondary))
                                                    .truncate()
                                                    .child(account_sub),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .px(px(10.0))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .children(nav_items.into_iter().map(|(tab, id, icon, label, badge)| {
                                let active =
                                    self.page == SettingsComputersPage::Overview
                                        && self.overview_tab == tab;
                                let fg = if active { UU_BLUE } else { pal.text_primary };
                                let icon_fg = if active { UU_BLUE } else { pal.icon_fg };
                                div()
                                    .id(id)
                                    .relative()
                                    .cursor_pointer()
                                    .h(px(40.0))
                                    .px(px(12.0))
                                    .rounded(px(10.0))
                                    .when(active, |el| el.bg(nav_active_bg))
                                    .hover(move |s| if active { s } else { s.bg(rgb(nav_hover)) })
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.page = SettingsComputersPage::Overview;
                                            this.set_overview_tab(tab);
                                            cx.notify();
                                        }),
                                    )
                                    .when(active, |el| {
                                        el.child(
                                            div()
                                                .absolute()
                                                .left(px(0.0))
                                                .top(px(10.0))
                                                .bottom(px(10.0))
                                                .w(px(3.0))
                                                .rounded_r(px(3.0))
                                                .bg(rgb(UU_BLUE)),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(10.0))
                                            .child(tg_icon(icon, 15.5, icon_fg))
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .font_weight(if active {
                                                        FontWeight::SEMIBOLD
                                                    } else {
                                                        FontWeight::MEDIUM
                                                    })
                                                    .text_color(rgb(fg))
                                                    .child(label),
                                            ),
                                    )
                                    .when_some(badge, |row, cnt| {
                                        row.child(
                                            div()
                                                .px(px(7.0))
                                                .py(px(1.5))
                                                .rounded_full()
                                                .bg(if active {
                                                    TdesktopThemePalette::rgba_with_alpha(UU_BLUE, 0.20)
                                                } else {
                                                    TdesktopThemePalette::rgba_with_alpha(
                                                        pal.text_secondary,
                                                        0.14,
                                                    )
                                                })
                                                .text_size(px(10.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(if active {
                                                    UU_BLUE
                                                } else {
                                                    pal.text_secondary
                                                }))
                                                .child(cnt.to_string()),
                                        )
                                    })
                            })),
                    ),
            )
            .child(
                div()
                    .p(px(12.0))
                    .border_t_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .when(active_sessions_count > 0, |col| {
                        col.child(
                            div()
                                .id("sc-disconnect-all-btn")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(7.0))
                                .rounded(px(8.0))
                                .bg(rgb(pal.danger))
                                .hover(|s| s.opacity(0.9))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(px(6.0))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.pending_event = Some(
                                            SettingsComputersEvent::DisconnectAllRemoteControl,
                                        );
                                        cx.notify();
                                    }),
                                )
                                .child(
                                    div()
                                        .w(px(6.0))
                                        .h(px(6.0))
                                        .rounded_full()
                                        .bg(rgb(0xFFFFFF)),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .child(if lang.starts_with("zh") {
                                            format!("断开所有远控 ({active_sessions_count})")
                                        } else {
                                            format!("Disconnect All ({active_sessions_count})")
                                        }),
                                ),
                        )
                    })
                    .child(
                        div()
                            .id("uu-sidebar-allow-assist-toggle")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(9.0))
                            .bg(rgb(pal.card_bg))
                            .border_1()
                            .border_color(pal.separator_rgba())
                            .hover(move |s| s.bg(rgb(nav_hover)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    let next = !allow_assist;
                                    if let Some(c) = this.assist_config.as_mut() {
                                        c.allow_assist = next;
                                    }
                                    this.pending_event =
                                        Some(SettingsComputersEvent::SetAssistAllow(next));
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(pal.text_primary))
                                    .child(tr_str(lang, "允许远控本机", "Allow Remote")),
                            )
                            .child(
                                div()
                                    .w(px(32.0))
                                    .h(px(18.0))
                                    .rounded_full()
                                    .bg(rgb(if allow_assist {
                                        UU_ONLINE_GREEN
                                    } else if is_dark {
                                        0x3A3F4B
                                    } else {
                                        0xCBD5E1
                                    }))
                                    .p(px(2.0))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .when(allow_assist, |el| el.justify_end())
                                    .child(
                                        div()
                                            .w(px(14.0))
                                            .h(px(14.0))
                                            .rounded_full()
                                            .bg(rgb(0xFFFFFF)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(9.0))
                            .bg(rgb(pal.card_bg))
                            .border_1()
                            .border_color(pal.separator_rgba())
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .w(px(6.5))
                                            .h(px(6.5))
                                            .rounded_full()
                                            .bg(rgb(if ts_running {
                                                UU_ONLINE_GREEN
                                            } else {
                                                UU_BLUE
                                            })),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(if ts_running {
                                                tr_str(lang, "Tailnet 已组网", "Tailnet Active")
                                            } else {
                                                tr_str(lang, "Tailnet 连接中", "Tailnet Syncing")
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .truncate()
                                    .child(format!("{ts_ip} · {ts_mode_lbl}")),
                            ),
                    )
                    .when_some(mac_perm_snap, |col, snap| {
                        let ready = snap.all_ready();
                        let pill_color = if ready { UU_ONLINE_GREEN } else { 0xF59E0B };
                        col.child(
                            div()
                                .id("uu-sidebar-perm-pill")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(7.0))
                                .rounded(px(9.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(
                                    pill_color,
                                    if ready { 0.10 } else { 0.15 },
                                ))
                                .border_1()
                                .border_color(TdesktopThemePalette::rgba_with_alpha(
                                    pill_color,
                                    0.32,
                                ))
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.permission_modal_open = true;
                                        request_permissions_refresh(true);
                                        cx.notify();
                                    }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(tg_icon(
                                            if ready {
                                                IconName::SelectCheck
                                            } else {
                                                IconName::Lock
                                            },
                                            12.0,
                                            pill_color,
                                        ))
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(pill_color))
                                                .child(if ready {
                                                    tr_str(lang, "系统权限已就绪", "Permissions OK")
                                                } else if snap.screen_state.needs_restart() {
                                                    tr_str(lang, "需重启以生效", "Restart Needed")
                                                } else {
                                                    tr_str(lang, "开启系统权限", "Grant Permissions")
                                                }),
                                        ),
                                )
                                .child(tg_icon(IconName::ChevronRight, 11.0, pill_color)),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_uu_header(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let locals_online = self
            .computers
            .iter()
            .filter(|c| {
                c.kind == ComputerNodeKind::LocalDevice
                    && (c.online || self.find_tailnet_peer(c).is_some_and(|p| p.online))
            })
            .count();
        let clouds_count = self
            .computers
            .iter()
            .filter(|c| c.kind == ComputerNodeKind::CloudServer)
            .count();

        let (title, badge_opt): (String, Option<(String, u32)>) = match &self.page {
            SettingsComputersPage::Overview => match self.overview_tab {
                SettingsComputersTab::MyDevices => (
                    tr_str(lang, "我的设备", "My Devices").to_string(),
                    Some((
                        if lang.starts_with("zh") {
                            format!("{locals_online} 台在线")
                        } else {
                            format!("{locals_online} online")
                        },
                        UU_ONLINE_GREEN,
                    )),
                ),
                SettingsComputersTab::RemoteAssist => {
                    let allowed = self.assist_config.as_ref().is_some_and(|c| c.allow_assist);
                    (
                        tr_str(lang, "远程协助", "Remote Assist").to_string(),
                        Some((
                            if allowed {
                                tr_str(lang, "已开启协助", "Assist Ready").to_string()
                            } else {
                                tr_str(lang, "协助已关闭", "Assist Off").to_string()
                            },
                            if allowed { UU_ONLINE_GREEN } else { pal.text_secondary },
                        )),
                    )
                }
                SettingsComputersTab::CloudComputers => (
                    tr_str(lang, "云电脑", "Cloud PCs").to_string(),
                    Some((
                        if lang.starts_with("zh") {
                            format!("{clouds_count} 台云主机")
                        } else {
                            format!("{clouds_count} cloud host(s)")
                        },
                        UU_BLUE,
                    )),
                ),
                SettingsComputersTab::AiMcpHub => (
                    tr_str(lang, "AI & 官方 MCP 授权中心", "AI & Official MCP Hub").to_string(),
                    Some(("OAuth 2.1".to_string(), 0x14B8A6)),
                ),
            },
            SettingsComputersPage::Detail(nid) => {
                let name = self
                    .computers
                    .iter()
                    .find(|c| c.node_id == *nid)
                    .map(|c| c.display_name.clone())
                    .unwrap_or_else(|| tr_str(lang, "设备管理", "Device Settings").to_string());
                (name, None)
            }
            SettingsComputersPage::CloudForm => (
                if self.editing_node_id.is_some() {
                    tr_str(lang, "编辑云电脑", "Edit Cloud Computer").to_string()
                } else {
                    tr_str(lang, "添加云电脑", "Add Cloud Computer").to_string()
                },
                None,
            ),
        };

        div()
            .h(px(56.0))
            .flex_shrink_0()
            .w_full()
            .px(px(22.0))
            .bg(rgb(pal.topbar_bg))
            .border_b_1()
            .border_color(pal.separator_rgba())
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .min_w(px(0.0))
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.text_primary))
                            .truncate()
                            .child(title),
                    )
                    .when_some(badge_opt, |row, (label, color)| {
                        row.child(
                            div()
                                .flex_shrink_0()
                                .px(px(8.0))
                                .py(px(2.5))
                                .rounded_full()
                                .bg(TdesktopThemePalette::rgba_with_alpha(color, 0.14))
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(color))
                                .child(label),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("sc-refresh-btn")
                            .cursor_pointer()
                            .flex_shrink_0()
                            .px(px(11.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(rgb(pal.border_fg))
                            .hover(|s| s.bg(rgb(pal.hover_bg)))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_primary))
                            .whitespace_nowrap()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.pending_event =
                                        Some(SettingsComputersEvent::RefreshComputers);
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "刷新", "Refresh")),
                    )
                    .child(
                        div()
                            .id("sc-create-cloud-btn")
                            .cursor_pointer()
                            .flex_shrink_0()
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(rgb(UU_BLUE))
                            .hover(|s| s.bg(rgb(UU_BLUE_HOVER)))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xFFFFFF))
                            .whitespace_nowrap()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.open_cloud_form(this.current_bot_id, None);
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "+ 添加云电脑", "+ Add Cloud PC")),
                    )
                    .child(
                        div()
                            .id("uu-computers-close-btn")
                            .cursor_pointer()
                            .flex_shrink_0()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded_full()
                            .bg(rgb(pal.input_bg))
                            .hover(|s| s.bg(rgb(pal.hover_bg)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.pending_event =
                                        Some(SettingsComputersEvent::CloseRequested);
                                    cx.notify();
                                }),
                            )
                            .child(tg_icon(IconName::Close, 13.0, pal.icon_fg)),
                    ),
            )
            .into_any_element()
    }

    fn render_uu_device_card(
        &self,
        node: &ComputerNodeDto,
        idx: usize,
        is_local: bool,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let nid_enter = node.node_id.clone();
        let nid_thumb = node.node_id.clone();
        let nid_term = node.node_id.clone();
        let nid_detail = node.node_id.clone();

        let peer_opt = self.find_tailnet_peer(node);
        let is_online = node.online || peer_opt.is_some_and(|p| p.online);
        let endpoint = if let Some(peer) = peer_opt {
            let ts_ip = peer.tailnet_ip.trim();
            let lan = if !peer.lan_ip.trim().is_empty() {
                peer.lan_ip.trim()
            } else {
                node.ip_address.trim()
            };
            if !ts_ip.is_empty() && !lan.is_empty() && ts_ip != lan {
                format!("{ts_ip} · {lan}")
            } else if !ts_ip.is_empty() {
                ts_ip.to_string()
            } else {
                Self::node_endpoint(node)
            }
        } else {
            Self::node_endpoint(node)
        };

        let transport_chip = if let Some(peer) = peer_opt {
            Some(Self::format_peer_transport(peer, lang))
        } else if is_local && node.is_current_device {
            self.tailnet_status
                .as_ref()
                .filter(|ts| ts.running)
                .map(|ts| Self::relay_mode_label(ts.relay_mode, lang).to_string())
        } else {
            None
        };

        let os_lower = node.os_name.to_ascii_lowercase();
        let is_mac = os_lower.contains("mac") || os_lower.contains("darwin");
        let is_win = os_lower.contains("win");
        let os_short = if !is_local {
            tr_str(lang, "云电脑", "Cloud PC")
        } else if is_mac {
            "macOS"
        } else if is_win {
            "Windows"
        } else if os_lower.contains("linux") {
            "Linux"
        } else {
            "Desktop"
        };

        let (grad_start, grad_end) = if !is_online {
            (0x1C2029, 0x2B313F)
        } else if !is_local {
            (0x0F172A, 0x1D4ED8)
        } else if is_mac {
            (0x132247, 0x2563EB)
        } else if is_win {
            (0x0B2B5C, 0x0EA5E9)
        } else {
            (0x111827, 0x0F766E)
        };

        let cached_thumb = self
            .thumbnail_cache
            .get(&node.node_id)
            .map(|(_, img)| img.clone());

        let online_pill_text = match (is_online, transport_chip.as_deref()) {
            (true, Some(t)) => format!("{} · {t}", tr_str(lang, "在线", "Online")),
            (true, None) => tr_str(lang, "在线", "Online").to_string(),
            (false, _) => tr_str(lang, "离线", "Offline").to_string(),
        };

        let meta_line = {
            let pol = Self::policy_label(node.run_policy, lang);
            if endpoint.trim().is_empty() {
                pol.to_string()
            } else {
                format!("{endpoint} · {pol}")
            }
        };

        let preview_content: AnyElement = if let Some(arc_img) = cached_thumb {
            img(arc_img)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element()
        } else {
            // Stylized NetEase UU Remote desktop preview wallpaper + mini window + dock
            div()
                .size_full()
                .bg(linear_gradient(
                    135.0,
                    linear_color_stop(rgb(grad_start), 0.0),
                    linear_color_stop(rgb(grad_end), 1.0),
                ))
                .flex()
                .flex_col()
                .justify_between()
                .child(
                    // Subtle top OS menu bar
                    div()
                        .w_full()
                        .h(px(14.0))
                        .bg(rgba(0x00000038))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .child(div().w(px(14.0)).h(px(3.0)).rounded_full().bg(rgba(0xFFFFFF44)))
                                .child(div().w(px(10.0)).h(px(3.0)).rounded_full().bg(rgba(0xFFFFFF2E))),
                        )
                        .child(div().w(px(18.0)).h(px(3.0)).rounded_full().bg(rgba(0xFFFFFF38))),
                )
                .child(
                    // Centered stylized translucent app window silhouette
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px(px(28.0))
                        .py(px(10.0))
                        .child(
                            div()
                                .w(px(164.0))
                                .h(px(68.0))
                                .rounded(px(8.0))
                                .bg(rgba(0x0B122466))
                                .border_1()
                                .border_color(rgba(0xFFFFFF24))
                                .p(px(8.0))
                                .flex()
                                .flex_col()
                                .justify_between()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .child(div().w(px(5.0)).h(px(5.0)).rounded_full().bg(rgb(0xFF5F56)))
                                        .child(div().w(px(5.0)).h(px(5.0)).rounded_full().bg(rgb(0xFFBD2E)))
                                        .child(div().w(px(5.0)).h(px(5.0)).rounded_full().bg(rgb(0x27C93F))),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(5.0))
                                        .child(div().w(px(96.0)).h(px(5.0)).rounded_full().bg(rgba(0xFFFFFF3B)))
                                        .child(div().w(px(124.0)).h(px(4.0)).rounded_full().bg(rgba(0xFFFFFF22)))
                                        .child(div().w(px(72.0)).h(px(4.0)).rounded_full().bg(rgba(0xFFFFFF1C))),
                                ),
                        ),
                )
                .child(
                    // Bottom mini macOS / Windows floating dock pill
                    div()
                        .w_full()
                        .pb(px(7.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .px(px(10.0))
                                .py(px(3.5))
                                .rounded_full()
                                .bg(rgba(0xFFFFFF26))
                                .border_1()
                                .border_color(rgba(0xFFFFFF2E))
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .child(div().w(px(6.0)).h(px(6.0)).rounded(px(2.0)).bg(rgb(0x60A5FA)))
                                .child(div().w(px(6.0)).h(px(6.0)).rounded(px(2.0)).bg(rgb(0x34D399)))
                                .child(div().w(px(6.0)).h(px(6.0)).rounded(px(2.0)).bg(rgb(0xFBBF24)))
                                .child(div().w(px(6.0)).h(px(6.0)).rounded(px(2.0)).bg(rgb(0xA78BFA))),
                        ),
                )
                .into_any_element()
        };

        div()
            .id((if is_local { "sc-local-row" } else { "sc-cloud-row" }, idx))
            .w(px(336.0))
            .rounded(px(14.0))
            .bg(rgb(pal.card_bg))
            .border_1()
            .border_color(pal.separator_rgba())
            .hover(|s| s.border_color(rgb(UU_BLUE)))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .id((if is_local { "uu-local-thumb" } else { "uu-cloud-thumb" }, idx))
                    .relative()
                    .cursor_pointer()
                    .w_full()
                    .h(px(136.0))
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.pending_event = Some(
                                SettingsComputersEvent::OpenRemoteWorkbench(nid_thumb.clone()),
                            );
                            cx.notify();
                        }),
                    )
                    .child(preview_content)
                    .child(
                        div()
                            .absolute()
                            .top(px(9.0))
                            .left(px(9.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .px(px(7.0))
                                    .py(px(2.0))
                                    .rounded(px(6.0))
                                    .bg(rgba(0x090D16B8))
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0xF8FAFC))
                                    .child(os_short),
                            )
                            .when(is_local && node.is_current_device, |row| {
                                row.child(
                                    div()
                                        .px(px(7.0))
                                        .py(px(2.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(UU_BLUE))
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .child(tr_str(lang, "本机", "This PC")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(9.0))
                            .right(px(9.0))
                            .px(px(8.0))
                            .py(px(2.5))
                            .rounded_full()
                            .bg(rgba(0x090D16BF))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .w(px(6.0))
                                    .h(px(6.0))
                                    .rounded_full()
                                    .bg(rgb(if is_online {
                                        0x4ADE80
                                    } else {
                                        0x94A3B8
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(0xF8FAFC))
                                    .child(online_pill_text),
                            ),
                    ),
            )
            .child(
                div()
                    .p(px(13.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .truncate()
                                            .child(node.display_name.clone()),
                                    )
                                    .when(!node.resolution.trim().is_empty(), |row| {
                                        row.child(
                                            div()
                                                .flex_shrink_0()
                                                .text_size(px(10.5))
                                                .text_color(rgb(pal.text_secondary))
                                                .child(node.resolution.clone()),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .truncate()
                                    .child(meta_line),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(7.0))
                            .child(
                                div()
                                    .id((if is_local { "sc-local-wb" } else { "sc-cloud-wb" }, idx))
                                    .cursor_pointer()
                                    .flex_1()
                                    .h(px(32.0))
                                    .px(px(12.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(UU_BLUE))
                                    .hover(|s| s.bg(rgb(UU_BLUE_HOVER)))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(6.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.pending_event = Some(
                                                SettingsComputersEvent::OpenRemoteWorkbench(
                                                    nid_enter.clone(),
                                                ),
                                            );
                                            cx.notify();
                                        }),
                                    )
                                    .child(tg_icon(IconName::Play, 11.0, 0xFFFFFF))
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(0xFFFFFF))
                                            .whitespace_nowrap()
                                            .child(tr_str(lang, "进入桌面", "Enter Desktop")),
                                    ),
                            )
                            .child(
                                div()
                                    .id((if is_local { "uu-local-term" } else { "uu-cloud-term" }, idx))
                                    .cursor_pointer()
                                    .flex_shrink_0()
                                    .h(px(32.0))
                                    .px(px(10.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(rgb(pal.border_fg))
                                    .hover(|s| s.bg(rgb(pal.hover_bg)))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(5.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.pending_event = Some(
                                                SettingsComputersEvent::OpenRemoteTerminal(
                                                    nid_term.clone(),
                                                ),
                                            );
                                            cx.notify();
                                        }),
                                    )
                                    .child(tg_icon(IconName::TerminalMcp, 13.0, pal.icon_fg))
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(pal.text_primary))
                                            .whitespace_nowrap()
                                            .child(tr_str(lang, "终端", "Shell")),
                                    ),
                            )
                            .child(
                                div()
                                    .id((if is_local { "sc-local-open" } else { "sc-cloud-open" }, idx))
                                    .cursor_pointer()
                                    .flex_shrink_0()
                                    .w(px(32.0))
                                    .h(px(32.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(rgb(pal.border_fg))
                                    .hover(|s| s.bg(rgb(pal.hover_bg)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.open_detail(&nid_detail);
                                            this.pending_event = Some(
                                                SettingsComputersEvent::SelectNode(
                                                    nid_detail.clone(),
                                                ),
                                            );
                                            cx.notify();
                                        }),
                                    )
                                    .child(tg_icon(IconName::SettingsGear, 14.0, pal.icon_fg)),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_uu_add_cloud_ghost_card(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("uu-add-cloud-ghost-card")
            .cursor_pointer()
            .w(px(336.0))
            .min_h(px(236.0))
            .rounded(px(14.0))
            .bg(TdesktopThemePalette::rgba_with_alpha(
                pal.card_bg,
                if pal.mode.is_dark() { 0.55 } else { 0.7 },
            ))
            .border_1()
            .border_color(rgb(pal.border_fg))
            .hover(|s| {
                s.border_color(rgb(UU_BLUE))
                    .bg(TdesktopThemePalette::rgba_with_alpha(UU_BLUE, 0.05))
            })
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .p(px(20.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.open_cloud_form(this.current_bot_id, None);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(44.0))
                    .h(px(44.0))
                    .rounded_full()
                    .bg(TdesktopThemePalette::rgba_with_alpha(UU_BLUE, 0.14))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(22.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(UU_BLUE))
                    .child("+"),
            )
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.text_primary))
                    .child(tr_str(lang, "添加云电脑 / 远端主机", "Add Cloud PC / Host")),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(pal.text_secondary))
                    .text_center()
                    .child(tr_str(
                        lang,
                        "支持 24/7 常驻云服务器，一键直连桌面与终端",
                        "Connect 24/7 cloud hosts with instant desktop & shell",
                    )),
            )
            .into_any_element()
    }

    fn render_uu_quick_assist_strip(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cfg = self.assist_config.as_ref();
        let assist_id_fmt =
            format_assist_id_display(cfg.map(|c| c.assist_id.as_str()).unwrap_or(""));
        let current_code = cfg
            .and_then(|c| c.current_code.as_deref())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("------")
            .to_string();

        settings_card(pal)
            .child(
                div()
                    .px(px(16.0))
                    .py(px(13.0))
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(18.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(pal.text_secondary))
                                            .child(tr_str(
                                                lang,
                                                "本机远程协助识别码",
                                                "This Device Assist ID",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(17.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(UU_BLUE))
                                            .child(assist_id_fmt),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(1.0))
                                    .h(px(28.0))
                                    .bg(pal.separator_rgba()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(pal.text_secondary))
                                            .child(tr_str(lang, "安全验证码", "Access Code")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(current_code),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .id("uu-quick-copy-assist-btn")
                                    .cursor_pointer()
                                    .px(px(11.0))
                                    .py(px(6.5))
                                    .rounded(px(8.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(rgb(pal.border_fg))
                                    .hover(|s| s.bg(rgb(pal.hover_bg)))
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            let invite = this.formatted_assist_invite_text();
                                            this.status_message = Some(format!(
                                                "{} {invite}",
                                                tr_str(
                                                    &this.language_id,
                                                    "已复制协助邀请：",
                                                    "Copied invite:"
                                                )
                                            ));
                                            this.pending_event = Some(
                                                SettingsComputersEvent::CopyAssistInvite {
                                                    invite_text: invite,
                                                },
                                            );
                                            cx.notify();
                                        }),
                                    )
                                    .child(tg_icon(IconName::Copy, 12.5, pal.icon_fg))
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(tr_str(lang, "复制邀请", "Copy Invite")),
                                    ),
                            )
                            .child(
                                div()
                                    .id("uu-goto-remote-assist-btn")
                                    .cursor_pointer()
                                    .px(px(12.0))
                                    .py(px(6.5))
                                    .rounded(px(8.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(UU_BLUE, 0.14))
                                    .hover(|s| {
                                        s.bg(TdesktopThemePalette::rgba_with_alpha(UU_BLUE, 0.22))
                                    })
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(UU_BLUE))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.set_overview_tab(
                                                SettingsComputersTab::RemoteAssist,
                                            );
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "远程协助他人 →", "Assist Others →")),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_uu_compact_permissions_strip(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let snap = cached_permissions_snapshot();
        if snap.all_ready() {
            return settings_card(pal)
                .child(
                    div()
                        .px(px(14.0))
                        .py(px(10.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .child(tg_icon(IconName::SelectCheck, 14.0, UU_ONLINE_GREEN))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(
                                            lang,
                                            "macOS 系统权限已全部生效（辅助功能、屏幕和系统音频）",
                                            "macOS system permissions active (Accessibility & Screen Recording)",
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .id("settings-perm-recheck-btn")
                                        .cursor_pointer()
                                        .px(px(9.0))
                                        .py(px(4.0))
                                        .rounded(px(7.0))
                                        .bg(rgb(pal.input_bg))
                                        .border_1()
                                        .border_color(rgb(pal.border_fg))
                                        .hover(|s| s.bg(rgb(pal.hover_bg)))
                                        .text_size(px(11.5))
                                        .text_color(rgb(pal.text_secondary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_, _, _, cx| {
                                                cx.stop_propagation();
                                                request_permissions_refresh(true);
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "重新检测", "Re-check")),
                                )
                                .child(
                                    div()
                                        .id("settings-perm-wizard-btn")
                                        .cursor_pointer()
                                        .px(px(9.0))
                                        .py(px(4.0))
                                        .rounded(px(7.0))
                                        .bg(rgb(pal.input_bg))
                                        .border_1()
                                        .border_color(rgb(pal.border_fg))
                                        .hover(|s| s.bg(rgb(pal.hover_bg)))
                                        .text_size(px(11.5))
                                        .text_color(rgb(pal.text_primary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.permission_modal_open = true;
                                                request_permissions_refresh(true);
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "授权向导", "Guide")),
                                ),
                        ),
                )
                .into_any_element();
        }
        self.render_macos_permissions_card(pal, lang, cx)
    }

    fn render_overview(&self, pal: TdesktopThemePalette, lang: &str, cx: &mut Context<Self>) -> AnyElement {
        let locals: Vec<&ComputerNodeDto> = self
            .computers
            .iter()
            .filter(|c| c.kind == ComputerNodeKind::LocalDevice)
            .collect();
        let clouds: Vec<&ComputerNodeDto> = self
            .computers
            .iter()
            .filter(|c| c.kind == ComputerNodeKind::CloudServer)
            .collect();

        let mut content = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .when_some(self.status_message.as_ref(), |col, msg| {
                col.child(
                    div()
                        .px(px(12.0))
                        .py(px(9.0))
                        .rounded(px(9.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(UU_BLUE, 0.14))
                        .border_1()
                        .border_color(TdesktopThemePalette::rgba_with_alpha(UU_BLUE, 0.28))
                        .text_size(px(12.0))
                        .text_color(rgb(pal.text_primary))
                        .child(msg.clone()),
                )
            });

        content = match self.overview_tab {
            SettingsComputersTab::MyDevices => {
                let perm_needs_attention =
                    cfg!(target_os = "macos") && !cached_permissions_snapshot().all_ready();
                let mut device_grid = div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(14.0));
                if locals.is_empty() && clouds.is_empty() {
                    device_grid = device_grid.child(
                        settings_card(pal).child(
                            div()
                                .p(px(28.0))
                                .w_full()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .w(px(48.0))
                                        .h(px(48.0))
                                        .rounded(px(14.0))
                                        .bg(TdesktopThemePalette::rgba_with_alpha(UU_BLUE, 0.14))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(tg_icon(IconName::ComputerDesktop, 24.0, UU_BLUE)),
                                )
                                .child(
                                    div()
                                        .text_size(px(14.5))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(
                                            lang,
                                            "暂未同步到同账号设备",
                                            "No devices synced yet",
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(tr_str(
                                            lang,
                                            "在其他 Mac / Windows 电脑登录同一账号后将自动出现，或点击下方刷新同步。",
                                            "Sign in on another Mac/Windows device to see it here automatically.",
                                        )),
                                )
                                .child(
                                    div()
                                        .id("uu-empty-sync-btn")
                                        .cursor_pointer()
                                        .mt(px(4.0))
                                        .px(px(16.0))
                                        .py(px(7.5))
                                        .rounded(px(8.0))
                                        .bg(rgb(UU_BLUE))
                                        .hover(|s| s.bg(rgb(UU_BLUE_HOVER)))
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.pending_event = Some(
                                                    SettingsComputersEvent::RefreshComputers,
                                                );
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "立即同步设备", "Sync Devices")),
                                ),
                        ),
                    );
                } else {
                    for (idx, node) in locals.iter().enumerate() {
                        device_grid = device_grid.child(self.render_uu_device_card(
                            node, idx, true, pal, lang, cx,
                        ));
                    }
                    for (idx, node) in clouds.iter().enumerate() {
                        device_grid = device_grid.child(self.render_uu_device_card(
                            node, idx, false, pal, lang, cx,
                        ));
                    }
                    if locals.len() + clouds.len() <= 1 {
                        device_grid =
                            device_grid.child(self.render_uu_add_cloud_ghost_card(pal, lang, cx));
                    }
                }

                content
                    .when(perm_needs_attention, |col| {
                        col.child(self.render_macos_permissions_card(pal, lang, cx))
                    })
                    .child(device_grid)
                    .child(self.render_uu_quick_assist_strip(pal, lang, cx))
                    .child(self.render_tailnet_mesh_card(pal, lang))
                    .when(cfg!(target_os = "macos") && !perm_needs_attention, |col| {
                        col.child(self.render_uu_compact_permissions_strip(pal, lang, cx))
                    })
            }
            SettingsComputersTab::RemoteAssist => {
                content.child(self.render_remote_assist_tab(pal, lang, cx))
            }
            SettingsComputersTab::CloudComputers => {
                if clouds.is_empty() {
                    content.child(
                        settings_card(pal).child(
                            div()
                                .p(px(36.0))
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .w(px(52.0))
                                        .h(px(52.0))
                                        .rounded(px(14.0))
                                        .bg(TdesktopThemePalette::rgba_with_alpha(UU_BLUE, 0.14))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(tg_icon(IconName::ComputerDesktop, 24.0, UU_BLUE)),
                                )
                                .child(
                                    div()
                                        .text_size(px(15.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(
                                            lang,
                                            "添加常驻云电脑",
                                            "Add an Always-On Cloud PC",
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(rgb(pal.text_secondary))
                                        .text_center()
                                        .child(tr_str(
                                            lang,
                                            "绑定 Linux / macOS / Windows 云服务器，随时随地一键进入远程桌面与终端。",
                                            "Connect Linux, macOS, or Windows cloud servers for instant remote desktop and shell access.",
                                        )),
                                )
                                .child(
                                    div()
                                        .id("sc-empty-create-cloud")
                                        .cursor_pointer()
                                        .mt(px(6.0))
                                        .px(px(16.0))
                                        .py(px(8.0))
                                        .rounded(px(8.0))
                                        .bg(rgb(UU_BLUE))
                                        .hover(|s| s.bg(rgb(UU_BLUE_HOVER)))
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.open_cloud_form(this.current_bot_id, None);
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "+ 添加云电脑", "+ Add Cloud PC")),
                                ),
                        ),
                    )
                } else {
                    let mut grid = div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap(px(14.0));
                    for (idx, node) in clouds.iter().enumerate() {
                        grid = grid.child(self.render_uu_device_card(
                            node, idx, false, pal, lang, cx,
                        ));
                    }
                    grid = grid.child(self.render_uu_add_cloud_ghost_card(pal, lang, cx));
                    content.child(grid)
                }
            }
            SettingsComputersTab::AiMcpHub => {
                content.child(self.render_ai_mcp_hub_tab(pal, lang, cx))
            }
        };

        content.into_any_element()
    }

    fn render_remote_assist_tab(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cfg = self.assist_config.as_ref();
        let assist_id_fmt = format_assist_id_display(cfg.map(|c| c.assist_id.as_str()).unwrap_or(""));
        let allow_assist = cfg.is_some_and(|c| c.allow_assist);
        let current_code = cfg
            .and_then(|c| c.current_code.as_deref())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("------")
            .to_string();
        let code_mode_label = match cfg.map(|c| c.code_mode) {
            Some(AssistCodeMode::Custom) => tr_str(lang, "自定义验证码", "Custom Code"),
            _ => tr_str(lang, "临时验证码 (定时失效)", "Temporary Code"),
        };

        let (custom_text, custom_pre) = self
            .input_assist_custom_code
            .as_ref()
            .map(|c| c.read(cx).text_and_preedit(self.assist_custom_code_draft.clone()))
            .unwrap_or_else(|| (self.assist_custom_code_draft.clone(), None));
        let (target_id_text, target_id_pre) = self
            .input_assist_target_id
            .as_ref()
            .map(|c| c.read(cx).text_and_preedit(self.assist_target_id_draft.clone()))
            .unwrap_or_else(|| (self.assist_target_id_draft.clone(), None));
        let (target_code_text, target_code_pre) = self
            .input_assist_target_code
            .as_ref()
            .map(|c| c.read(cx).text_and_preedit(self.assist_target_code_draft.clone()))
            .unwrap_or_else(|| (self.assist_target_code_draft.clone(), None));

        div()
            .w_full()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(16.0))
            .child(
                // Left Card: Allow others to assist this computer (1:1 NetEase UU Remote style)
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .rounded(px(14.0))
                    .bg(rgb(pal.card_bg))
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .p(px(18.0))
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(14.5))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(tr_str(
                                                lang,
                                                "允许他人协助我",
                                                "Share This Device",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(rgb(pal.text_secondary))
                                            .child(tr_str(
                                                lang,
                                                "将设备识别码与验证码发送给对方即可接受协助",
                                                "Share your 9-digit ID & code for instant assist",
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .id("sc-assist-allow-toggle")
                                    .cursor_pointer()
                                    .flex_shrink_0()
                                    .px(px(11.0))
                                    .py(px(5.0))
                                    .rounded_full()
                                    .bg(rgb(if allow_assist {
                                        UU_ONLINE_GREEN
                                    } else {
                                        pal.input_bg
                                    }))
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(if allow_assist {
                                        0xFFFFFF
                                    } else {
                                        pal.text_primary
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            let next = !allow_assist;
                                            if let Some(c) = this.assist_config.as_mut() {
                                                c.allow_assist = next;
                                            }
                                            this.pending_event = Some(
                                                SettingsComputersEvent::SetAssistAllow(next),
                                            );
                                            cx.notify();
                                        }),
                                    )
                                    .child(if allow_assist {
                                        tr_str(lang, "已允许", "Allowed")
                                    } else {
                                        tr_str(lang, "已禁止", "Blocked")
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .p(px(16.0))
                            .rounded(px(12.0))
                            .bg(rgb(pal.window_bg))
                            .border_1()
                            .border_color(pal.separator_rgba())
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.0))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(pal.text_secondary))
                                            .child(tr_str(
                                                lang,
                                                "本机 9 位设备识别码",
                                                "My 9-Digit Device ID",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(26.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(UU_BLUE))
                                            .child(assist_id_fmt),
                                    ),
                            )
                            .child(div().w_full().h(px(1.0)).bg(pal.separator_rgba()))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.0))
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(pal.text_secondary))
                                                    .child(code_mode_label),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(20.0))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(rgb(pal.text_primary))
                                                    .child(current_code),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("sc-assist-reset-code-btn")
                                            .cursor_pointer()
                                            .flex_shrink_0()
                                            .px(px(10.0))
                                            .py(px(6.0))
                                            .rounded(px(8.0))
                                            .bg(rgb(pal.input_bg))
                                            .border_1()
                                            .border_color(rgb(pal.border_fg))
                                            .hover(|s| s.bg(rgb(pal.hover_bg)))
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.pending_event = Some(
                                                        SettingsComputersEvent::ResetAssistTempCode,
                                                    );
                                                    cx.notify();
                                                }),
                                            )
                                            .child(tr_str(lang, "刷新临时码", "Refresh")),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("sc-assist-copy-invite-btn")
                            .cursor_pointer()
                            .w_full()
                            .h(px(36.0))
                            .rounded(px(9.0))
                            .bg(rgb(UU_BLUE))
                            .hover(|s| s.bg(rgb(UU_BLUE_HOVER)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_center()
                            .gap(px(6.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    let invite = this.formatted_assist_invite_text();
                                    this.status_message = Some(format!(
                                        "{} {invite}",
                                        tr_str(
                                            &this.language_id,
                                            "已复制协助邀请：",
                                            "Copied invite:"
                                        )
                                    ));
                                    this.pending_event = Some(
                                        SettingsComputersEvent::CopyAssistInvite {
                                            invite_text: invite,
                                        },
                                    );
                                    cx.notify();
                                }),
                            )
                            .child(tg_icon(IconName::Copy, 13.5, 0xFFFFFF))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0xFFFFFF))
                                    .child(tr_str(lang, "复制识别码与验证码", "Copy Assist Invite")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .id("sc-assist-custom-code-input")
                                    .cursor_text()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .min_h(px(34.0))
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(rgb(
                                        if self.assist_focus == AssistInputField::CustomCode {
                                            UU_BLUE
                                        } else {
                                            pal.border_fg
                                        },
                                    ))
                                    .flex()
                                    .items_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.ensure_form_inputs(cx);
                                            this.assist_focus = AssistInputField::CustomCode;
                                            cx.notify();
                                        }),
                                    )
                                    .child(render_ime_text_preview(
                                        &custom_text,
                                        custom_pre.as_deref(),
                                        tr_str(
                                            lang,
                                            "自定义固定验证码（8-16位）",
                                            "Custom code (8-16 chars)",
                                        ),
                                        self.assist_focus == AssistInputField::CustomCode,
                                        false,
                                        pal.text_primary,
                                        pal,
                                    )),
                            )
                            .child(
                                div()
                                    .id("sc-assist-save-custom-btn")
                                    .cursor_pointer()
                                    .flex_shrink_0()
                                    .px(px(11.0))
                                    .py(px(7.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(rgb(pal.border_fg))
                                    .hover(|s| s.bg(rgb(pal.hover_bg)))
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.submit_assist_custom_code();
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "保存", "Save")),
                            ),
                    ),
            )
            .child(
                // Right Card: Assist another computer (1:1 NetEase UU Remote style)
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .rounded(px(14.0))
                    .bg(rgb(pal.card_bg))
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .p(px(18.0))
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_size(px(14.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(tr_str(
                                        lang,
                                        "远程协助他人",
                                        "Assist Another Computer",
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(
                                        lang,
                                        "输入对方设备码与验证码，立即建立加密桌面远控",
                                        "Enter your partner's 9-digit ID & code to connect",
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(lang, "对方 9 位设备识别码", "Partner 9-Digit ID")),
                            )
                            .child(
                                div()
                                    .id("sc-assist-target-id-input")
                                    .cursor_text()
                                    .w_full()
                                    .min_h(px(40.0))
                                    .px(px(12.0))
                                    .py(px(8.0))
                                    .rounded(px(9.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(rgb(
                                        if self.assist_focus == AssistInputField::TargetAssistId {
                                            UU_BLUE
                                        } else {
                                            pal.border_fg
                                        },
                                    ))
                                    .flex()
                                    .items_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.ensure_form_inputs(cx);
                                            this.assist_focus = AssistInputField::TargetAssistId;
                                            cx.notify();
                                        }),
                                    )
                                    .child(render_ime_text_preview(
                                        &target_id_text,
                                        target_id_pre.as_deref(),
                                        tr_str(lang, "例如 482 910 375", "e.g. 482 910 375"),
                                        self.assist_focus == AssistInputField::TargetAssistId,
                                        false,
                                        pal.text_primary,
                                        pal,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(lang, "安全验证码", "Access Code")),
                            )
                            .child(
                                div()
                                    .id("sc-assist-target-code-input")
                                    .cursor_text()
                                    .w_full()
                                    .min_h(px(40.0))
                                    .px(px(12.0))
                                    .py(px(8.0))
                                    .rounded(px(9.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(rgb(
                                        if self.assist_focus == AssistInputField::TargetAccessCode {
                                            UU_BLUE
                                        } else {
                                            pal.border_fg
                                        },
                                    ))
                                    .flex()
                                    .items_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.ensure_form_inputs(cx);
                                            this.assist_focus = AssistInputField::TargetAccessCode;
                                            cx.notify();
                                        }),
                                    )
                                    .child(render_ime_text_preview(
                                        &target_code_text,
                                        target_code_pre.as_deref(),
                                        tr_str(lang, "输入对方提供的验证码", "Enter access code"),
                                        self.assist_focus == AssistInputField::TargetAccessCode,
                                        true,
                                        pal.text_primary,
                                        pal,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id("sc-assist-connect-btn")
                            .cursor_pointer()
                            .mt(px(4.0))
                            .w_full()
                            .h(px(40.0))
                            .rounded(px(9.0))
                            .bg(rgb(UU_BLUE))
                            .hover(|s| s.bg(rgb(UU_BLUE_HOVER)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_center()
                            .gap(px(6.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.submit_assist_connect();
                                    cx.notify();
                                }),
                            )
                            .child(tg_icon(IconName::Play, 12.0, 0xFFFFFF))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xFFFFFF))
                                    .child(tr_str(lang, "立即连接桌面", "Connect Now")),
                            ),
                    )
                    .child(
                        div()
                            .pt(px(2.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(tg_icon(IconName::Lock, 12.0, UU_ONLINE_GREEN))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(
                                        lang,
                                        "全程端到端加密传输（X25519 + ChaCha20-Poly1305）",
                                        "End-to-end encrypted (X25519 + ChaCha20-Poly1305)",
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_ai_mcp_hub_tab(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cli = self.cli_status.as_ref();
        let cli_installed = cli.is_some_and(|c| c.installed);
        let cli_on_path = cli.is_some_and(|c| c.on_path);
        let cli_path_str = cli
            .map(|c| c.display_path())
            .unwrap_or_else(|| "~/.local/bin/fabushi".to_string());
        let cli_msg = cli
            .map(|c| c.status_message.clone())
            .unwrap_or_else(|| tr_str(lang, "点击刷新检测 CLI 状态", "Click refresh to inspect CLI status").to_string());

        let mut grants_col = settings_card(pal);
        if self.mcp_grants.is_empty() {
            grants_col = grants_col.child(
                div()
                    .p(px(16.0))
                    .text_size(px(12.5))
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(
                        lang,
                        "当前没有已授权的官方 MCP AI 客户端令牌。在 Antigravity / Claude / Cursor 中配置上方 MCP 地址并通过 OAuth 授权后，可在此实时查看与吊销权限。",
                        "No active OAuth 2.1 MCP grants yet. Authorized AI clients appear here for inspection and revocation.",
                    )),
            );
        } else {
            for (idx, grant) in self.mcp_grants.iter().enumerate() {
                let fid = grant.family_id.clone();
                let client_label = if grant.client_name.trim().is_empty() {
                    grant.client_id.clone()
                } else {
                    grant.client_name.clone()
                };
                let scopes = grant.scopes.join(", ");
                let btn_id = SharedString::from(format!("sc-revoke-mcp-grant-{idx}"));
                grants_col = grants_col.child(
                    div()
                        .px(px(16.0))
                        .py(px(12.0))
                        .border_b_1()
                        .border_color(rgb(pal.border_fg))
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(client_label),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!(
                                            "scopes: [{}] · devices: {}",
                                            scopes,
                                            if grant.allowed_node_ids.is_empty() {
                                                "all".to_string()
                                            } else {
                                                grant.allowed_node_ids.len().to_string()
                                            }
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .id(btn_id)
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(7.0))
                                .bg(rgb(pal.danger))
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xFFFFFF))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.pending_event =
                                            Some(SettingsComputersEvent::RevokeMcpGrant(fid.clone()));
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "吊销授权", "Revoke")),
                        ),
                );
            }
        }

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(settings_subsection_title(
                tr_str(lang, "官方 MCP Remote URL 与一键配置", "Official MCP Endpoint & One-Click Setup"),
                pal,
            ))
            .child(
                settings_card(pal).child(
                    div()
                        .p(px(16.0))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(rgb(pal.text_secondary))
                                                .child(tr_str(
                                                    lang,
                                                    "官方 Streamable HTTP + OAuth 2.1 MCP 地址",
                                                    "Official Streamable HTTP + OAuth 2.1 MCP URL",
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(13.5))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(0x14B8A6))
                                                .child(self.mcp_endpoint_url.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("sc-mcp-refresh-grants-btn")
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(6.0))
                                        .rounded(px(8.0))
                                        .bg(rgb(pal.input_bg))
                                        .border_1()
                                        .border_color(rgb(pal.border_fg))
                                        .text_size(px(12.0))
                                        .text_color(rgb(pal.text_primary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.pending_event =
                                                    Some(SettingsComputersEvent::RefreshMcpGrants);
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "刷新授权", "Refresh Grants")),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.0))
                                .children([
                                    (McpClientPreset::Antigravity, "sc-mcp-copy-antigravity"),
                                    (McpClientPreset::Claude, "sc-mcp-copy-claude"),
                                    (McpClientPreset::Cursor, "sc-mcp-copy-cursor"),
                                    (McpClientPreset::VsCode, "sc-mcp-copy-vscode"),
                                ]
                                .into_iter()
                                .map(|(preset, id)| {
                                    div()
                                        .id(id)
                                        .cursor_pointer()
                                        .px(px(11.0))
                                        .py(px(6.0))
                                        .rounded(px(8.0))
                                        .bg(rgb(pal.accent))
                                        .hover(|s| s.bg(rgb(pal.accent_hover)))
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.active_button_fg))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.trigger_copy_mcp_preset(preset);
                                                cx.notify();
                                            }),
                                        )
                                        .child(format!(
                                            "{} {}",
                                            tr_str(lang, "复制配置:", "Copy"),
                                            preset.display_name()
                                        ))
                                })),
                        ),
                ),
            )
            .child(settings_subsection_title(
                tr_str(lang, "已授权的 AI MCP 客户端 (OAuth 2.1)", "Authorized MCP Clients"),
                pal,
            ))
            .child(grants_col)
            .child(settings_subsection_title(
                tr_str(lang, "fabushi CLI 与本地 IPC 守护通道", "fabushi CLI & Local IPC Host"),
                pal,
            ))
            .child(
                settings_card(pal).child(
                    div()
                        .p(px(16.0))
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(format!(
                                            "fabushi CLI · {} · {}",
                                            if cli_installed {
                                                tr_str(lang, "已安装", "Installed")
                                            } else {
                                                tr_str(lang, "未安装到系统路径", "Not Installed")
                                            },
                                            if cli_on_path {
                                                "PATH OK"
                                            } else {
                                                tr_str(lang, "未在 PATH 中", "Not in PATH")
                                            }
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("{cli_path_str} · {cli_msg}")),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .id("sc-cli-refresh-btn")
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(6.0))
                                        .rounded(px(8.0))
                                        .bg(rgb(pal.input_bg))
                                        .border_1()
                                        .border_color(rgb(pal.border_fg))
                                        .text_size(px(12.0))
                                        .text_color(rgb(pal.text_primary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.pending_event =
                                                    Some(SettingsComputersEvent::RefreshCliStatus);
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "检测状态", "Check PATH")),
                                )
                                .child(
                                    div()
                                        .id("sc-cli-install-btn")
                                        .cursor_pointer()
                                        .px(px(12.0))
                                        .py(px(6.0))
                                        .rounded(px(8.0))
                                        .bg(rgb(pal.accent))
                                        .hover(|s| s.bg(rgb(pal.accent_hover)))
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.active_button_fg))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.pending_event =
                                                    Some(SettingsComputersEvent::InstallFabushiCli);
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "一键安装 CLI 到 PATH", "Install CLI")),
                                ),
                        ),
                ),
            )
            .into_any_element()
    }

    fn render_macos_permissions_card(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let snap = cached_permissions_snapshot();
        let all_ready = snap.all_ready();
        let bundle_path = snap.signing.drag_target_path().to_string();

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .px(px(4.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(if all_ready {
                                pal.success
                            } else {
                                pal.text_secondary
                            }))
                            .child(if all_ready {
                                tr_str(
                                    lang,
                                    "远程控制所需系统权限已全部生效",
                                    "All remote control system permissions are active",
                                )
                            } else if snap.screen_state.needs_restart() {
                                tr_str(
                                    lang,
                                    "录屏权限已在系统设置中开启，点击下方“重启 Fabushi”即可生效",
                                    "Screen Recording enabled in System Settings — click Restart below to apply",
                                )
                            } else {
                                tr_str(
                                    lang,
                                    "支持拖拽悬浮应用卡片到系统设置列表一键授权，状态自动实时更新",
                                    "Drag the floating app card into System Settings to grant — status updates live",
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .id("settings-perm-recheck-btn")
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(999.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(
                                        pal.text_primary,
                                        0.07,
                                    ))
                                    .border_1()
                                    .border_color(rgb(pal.border_fg))
                                    .hover(move |s| s.bg(rgb(pal.hover_bg)))
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(pal.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_, _, _, cx| {
                                            cx.stop_propagation();
                                            request_permissions_refresh(true);
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "重新检测", "Re-check")),
                            )
                            .child(
                                div()
                                    .id("settings-perm-wizard-btn")
                                    .cursor_pointer()
                                    .px(px(11.0))
                                    .py(px(4.0))
                                    .rounded(px(999.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                                    .border_1()
                                    .border_color(rgb(pal.accent))
                                    .hover(move |s| s.bg(rgb(pal.hover_bg)))
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.permission_modal_open = true;
                                            request_permissions_refresh(true);
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "打开授权向导", "Open Setup Guide")),
                            ),
                    ),
            )
            .child(render_permission_rows_card("settings", &snap, pal, lang))
            .when(snap.hud_visible, |col| {
                col.child(
                    div()
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                        .border_1()
                        .border_color(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.3))
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(11.5))
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(
                                    lang,
                                    "悬浮拖拽卡片已就绪：切换至“系统设置”后按住 Fabushi 拖入授权列表即可。",
                                    "Floating drag card is active — switch to System Settings and drag Fabushi into the list.",
                                )),
                        )
                        .child(
                            div()
                                .id("settings-perm-hide-hud-btn")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.0))
                                .rounded(px(999.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(rgb(pal.border_fg))
                                .text_size(px(11.5))
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_, _, _, cx| {
                                        cx.stop_propagation();
                                        hide_drag_hud();
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "隐藏卡片", "Hide")),
                        ),
                )
            })
            .when(snap.any_missing() && !bundle_path.is_empty(), |col| {
                col.child(
                    div()
                        .px(px(4.0))
                        .text_size(px(11.0))
                        .text_color(rgb(pal.text_secondary))
                        .truncate()
                        .child(if lang.starts_with("zh") {
                            format!("当前运行应用路径：{bundle_path}")
                        } else {
                            format!("Running app path: {bundle_path}")
                        }),
                )
            })
            .into_any_element()
    }

    fn render_detail(
        &self,
        node_id: &str,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(node) = self.computers.iter().find(|c| c.node_id == node_id) else {
            return settings_container(pal)
                .child(self.detail_back_bar(None, pal, lang, cx))
                .child(
                    settings_card(pal).child(
                        div()
                            .p(px(20.0))
                            .text_size(px(13.0))
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(
                                lang,
                                "未找到此计算机节点，可能已被删除或尚未同步。",
                                "Computer node not found or removed.",
                            )),
                    ),
                )
                .into_any_element();
        };

        let peer_opt = self.find_tailnet_peer(node);
        let is_local = node.kind == ComputerNodeKind::LocalDevice;
        let is_cloud = node.kind == ComputerNodeKind::CloudServer;
        let is_online = node.online || peer_opt.is_some_and(|p| p.online);
        let can_remove_offline_local = is_local && !node.is_current_device && !node.online;
        let endpoint = if let Some(peer) = peer_opt {
            let ts_ip = peer.tailnet_ip.trim();
            let lan = if !peer.lan_ip.trim().is_empty() {
                peer.lan_ip.trim()
            } else {
                node.ip_address.trim()
            };
            if !ts_ip.is_empty() && !lan.is_empty() && ts_ip != lan {
                format!("{ts_ip} · {lan}")
            } else if !ts_ip.is_empty() {
                ts_ip.to_string()
            } else {
                Self::node_endpoint(node)
            }
        } else {
            Self::node_endpoint(node)
        };
        let local_tailnet_ip = if is_local {
            peer_opt
                .map(|p| p.tailnet_ip.trim())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    node.host
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty() && *s != node.ip_address.trim())
                })
                .or_else(|| {
                    if node.is_current_device {
                        self.tailnet_status
                            .as_ref()
                            .map(|ts| ts.tailnet_ip.trim())
                            .filter(|s| !s.is_empty())
                    } else {
                        None
                    }
                })
                .map(str::to_string)
        } else {
            None
        };
        let local_transport_badge = if is_local {
            if let Some(peer) = peer_opt {
                Some((
                    Self::format_peer_transport(peer, lang),
                    peer.relay_mode == TailnetRelayMode::DirectUdp,
                ))
            } else if node.is_current_device {
                self.tailnet_status.as_ref().filter(|ts| ts.running).map(|ts| {
                    (
                        Self::relay_mode_label(ts.relay_mode, lang).to_string(),
                        ts.relay_mode == TailnetRelayMode::DirectUdp,
                    )
                })
            } else {
                None
            }
        } else {
            None
        };
        let badge = match node.kind {
            ComputerNodeKind::LocalDevice if node.is_current_device => {
                tr_str(lang, "当前设备", "Current Device")
            }
            ComputerNodeKind::LocalDevice => tr_str(lang, "同账号设备", "Account Device"),
            ComputerNodeKind::CloudServer => tr_str(lang, "云服务器", "Cloud Server"),
            ComputerNodeKind::CloudSandbox => tr_str(lang, "云沙箱", "Cloud Sandbox"),
        };
        let nid_wb = node.node_id.clone();
        let nid_policy = node.node_id.clone();
        let nid_add_dev = node.node_id.clone();
        let nid_add_bot = node.node_id.clone();
        let nid_remove = node.node_id.clone();
        let nid_test = node.node_id.clone();
        let nid_del = node.node_id.clone();
        let nid_del_local = node.node_id.clone();
        let node_clone_for_edit = node.clone();

        // Middle ACL card: 谁可以控制 <device>
        let ctrl_heading = if lang.starts_with("zh") {
            format!("谁可以控制 {}", node.display_name)
        } else {
            format!("Who can control {}", node.display_name)
        };

        let mut acl_card = settings_card(pal).child(
            div()
                .px(px(16.0))
                .py(px(12.0))
                .border_b_1()
                .border_color(rgb(pal.border_fg))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(ctrl_heading),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("sc-acl-add-device-btn")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(5.0))
                                .rounded(px(7.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                .hover(|s| s.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.24)))
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.device_picker_open = !this.device_picker_open;
                                        this.bot_picker_open = false;
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "+ 添加设备", "+ Add Device")),
                        )
                        .child(
                            div()
                                .id("sc-acl-add-bot-btn")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(5.0))
                                .rounded(px(7.0))
                                .bg(rgb(pal.input_bg))
                                .border_1()
                                .border_color(rgb(pal.border_fg))
                                .hover(|s| s.bg(rgb(pal.hover_bg)))
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.bot_picker_open = !this.bot_picker_open;
                                        this.device_picker_open = false;
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "+ 授权 AI Bot", "+ Authorize Bot")),
                        ),
                ),
        );

        if node.authorized_controllers.is_empty() {
            acl_card = acl_card.child(
                div()
                    .p(px(22.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .w(px(38.0))
                            .h(px(38.0))
                            .rounded(px(10.0))
                            .bg(rgb(pal.input_bg))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(tg_icon(IconName::ComputerDesktop, 18.0, pal.icon_fg)),
                    )
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(tr_str(lang, "没有已授权的设备", "No authorized devices")),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(
                                lang,
                                "添加设备以远程控制此计算机。",
                                "Add a device or AI Bot to allow remote control of this computer.",
                            )),
                    ),
            );
        } else {
            for (idx, entry) in node.authorized_controllers.iter().enumerate() {
                let cid = entry.controller_id.clone();
                let nid = nid_remove.clone();
                let kind_label = match entry.kind {
                    AuthorizedControllerKind::Device => tr_str(lang, "设备", "Device"),
                    AuthorizedControllerKind::Bot => "AI Bot",
                    AuthorizedControllerKind::AccountAllBots => {
                        tr_str(lang, "全部 Bot", "All Bots")
                    }
                };
                acl_card = acl_card.child(
                    div()
                        .px(px(16.0))
                        .py(px(10.0))
                        .border_b_1()
                        .border_color(rgb(pal.border_fg))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(tg_icon(
                                    match entry.kind {
                                        AuthorizedControllerKind::Device => {
                                            IconName::ComputerDesktop
                                        }
                                        _ => IconName::BotRobot,
                                    },
                                    15.0,
                                    pal.accent,
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(pal.text_primary))
                                                .child(entry.display_name.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(rgb(pal.text_secondary))
                                                .child(format!("{} · {}", kind_label, cid)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id(("sc-acl-remove", idx))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(6.0))
                                .bg(rgb(pal.input_bg))
                                .hover(|s| s.bg(rgb(pal.hover_bg)))
                                .text_size(px(11.5))
                                .text_color(rgb(pal.danger))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.pending_event =
                                            Some(SettingsComputersEvent::UpdatePolicy {
                                                node_id: nid.clone(),
                                                req: UpdateComputerPolicyRequest {
                                                    remove_controller_id: Some(cid.clone()),
                                                    ..Default::default()
                                                },
                                            });
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "移除", "Remove")),
                        ),
                );
            }
        }

        if self.device_picker_open {
            let other_locals: Vec<&ComputerNodeDto> = self
                .computers
                .iter()
                .filter(|c| c.kind == ComputerNodeKind::LocalDevice && c.node_id != node.node_id)
                .collect();
            let nid_all_dev = nid_add_dev.clone();
            let mut picker = div()
                .p(px(12.0))
                .bg(rgb(pal.input_bg))
                .border_t_1()
                .border_color(rgb(pal.border_fg))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(lang, "选择同账号登录设备授权", "Authorize Account Device")),
                )
                .child(
                    div()
                        .id("sc-pick-all-devices")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(7.0))
                        .rounded(px(7.0))
                        .bg(rgb(pal.card_bg))
                        .hover(|s| s.bg(rgb(pal.hover_bg)))
                        .text_size(px(12.0))
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.device_picker_open = false;
                                this.pending_event = Some(SettingsComputersEvent::UpdatePolicy {
                                    node_id: nid_all_dev.clone(),
                                    req: UpdateComputerPolicyRequest {
                                        add_controller: Some(AuthorizedControllerEntry {
                                            controller_id: "account_all_devices".to_string(),
                                            kind: AuthorizedControllerKind::Device,
                                            display_name: tr_str(
                                                &this.language_id,
                                                "允许所有同账号设备",
                                                "All Same-Account Devices",
                                            )
                                            .to_string(),
                                            added_at_ms: 0,
                                        }),
                                        ..Default::default()
                                    },
                                });
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "✓ 允许所有同账号设备", "✓ Allow All Account Devices")),
                );
            if other_locals.is_empty() {
                picker = picker.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(
                            lang,
                            "当前账号暂无其他在线桌面设备",
                            "No other logged-in devices on this account",
                        )),
                );
            } else {
                for (i, dev) in other_locals.into_iter().enumerate() {
                    let target_id = dev
                        .device_id
                        .clone()
                        .unwrap_or_else(|| dev.node_id.clone());
                    let label = format!("{} ({})", dev.display_name, Self::node_endpoint(dev));
                    let nid = nid_add_dev.clone();
                    picker = picker.child(
                        div()
                            .id(("sc-pick-dev", i))
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(7.0))
                            .rounded(px(7.0))
                            .bg(rgb(pal.card_bg))
                            .hover(|s| s.bg(rgb(pal.hover_bg)))
                            .text_size(px(12.0))
                            .text_color(rgb(pal.text_primary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.device_picker_open = false;
                                    this.pending_event =
                                        Some(SettingsComputersEvent::UpdatePolicy {
                                            node_id: nid.clone(),
                                            req: UpdateComputerPolicyRequest {
                                                add_controller: Some(AuthorizedControllerEntry {
                                                    controller_id: target_id.clone(),
                                                    kind: AuthorizedControllerKind::Device,
                                                    display_name: label.clone(),
                                                    added_at_ms: 0,
                                                }),
                                                ..Default::default()
                                            },
                                        });
                                    cx.notify();
                                }),
                            )
                            .child(format!("{} ({})", dev.display_name, Self::node_endpoint(dev))),
                    );
                }
            }
            acl_card = acl_card.child(picker);
        }

        if self.bot_picker_open {
            let nid_all_bots = nid_add_bot.clone();
            let mut picker = div()
                .p(px(12.0))
                .bg(rgb(pal.input_bg))
                .border_t_1()
                .border_color(rgb(pal.border_fg))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(lang, "选择授权的 AI Bot", "Authorize AI Bot")),
                )
                .child(
                    div()
                        .id("sc-pick-all-bots")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(7.0))
                        .rounded(px(7.0))
                        .bg(rgb(pal.card_bg))
                        .hover(|s| s.bg(rgb(pal.hover_bg)))
                        .text_size(px(12.0))
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.bot_picker_open = false;
                                this.pending_event = Some(SettingsComputersEvent::UpdatePolicy {
                                    node_id: nid_all_bots.clone(),
                                    req: UpdateComputerPolicyRequest {
                                        add_controller: Some(AuthorizedControllerEntry {
                                            controller_id: "account_all_bots".to_string(),
                                            kind: AuthorizedControllerKind::AccountAllBots,
                                            display_name: tr_str(
                                                &this.language_id,
                                                "允许账号下所有 AI Bot",
                                                "All Account AI Bots",
                                            )
                                            .to_string(),
                                            added_at_ms: 0,
                                        }),
                                        ..Default::default()
                                    },
                                });
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "✓ 允许账号下所有 AI Bot", "✓ Allow All Account AI Bots")),
                );
            for (i, (bid, bname)) in self.known_bots.iter().enumerate() {
                let bid_str = format!("bot:{bid}");
                let label = bname.clone();
                let nid = nid_add_bot.clone();
                picker = picker.child(
                    div()
                        .id(("sc-pick-bot", i))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(7.0))
                        .rounded(px(7.0))
                        .bg(rgb(pal.card_bg))
                        .hover(|s| s.bg(rgb(pal.hover_bg)))
                        .text_size(px(12.0))
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.bot_picker_open = false;
                                this.pending_event = Some(SettingsComputersEvent::UpdatePolicy {
                                    node_id: nid.clone(),
                                    req: UpdateComputerPolicyRequest {
                                        add_controller: Some(AuthorizedControllerEntry {
                                            controller_id: bid_str.clone(),
                                            kind: AuthorizedControllerKind::Bot,
                                            display_name: label.clone(),
                                            added_at_ms: 0,
                                        }),
                                        ..Default::default()
                                    },
                                });
                                cx.notify();
                            }),
                        )
                        .child(format!("{bname} (#{bid})")),
                );
            }
            acl_card = acl_card.child(picker);
        }

        // Bottom Run Policy Card: 在此计算机上运行
        let policies = [
            ComputerRunPolicy::AskEveryTime,
            ComputerRunPolicy::AllowReadOnly,
            ComputerRunPolicy::AlwaysAllowAuthorized,
            ComputerRunPolicy::Disabled,
        ];
        let mut policy_card = settings_card(pal).child(
            div()
                .p(px(16.0))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(lang, "在此计算机上运行", "Run on this computer")),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(tr_str(
                                    lang,
                                    "AI Bot 和所有 Agent 在此计算机上执行每条命令或修改文件前，都会先征求你的同意。",
                                    "AI Bots and Agents will follow this safety rule before running commands or modifying files here.",
                                )),
                        ),
                )
                .child(
                    div()
                        .id("sc-policy-dropdown-btn")
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(rgb(pal.border_fg))
                        .hover(|s| s.bg(rgb(pal.hover_bg)))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.policy_menu_open = !this.policy_menu_open;
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(Self::policy_label(node.run_policy, lang)),
                        )
                        .child(tg_icon(IconName::ChevronDown, 12.0, pal.icon_fg)),
                ),
        );

        if self.policy_menu_open {
            let mut menu = div()
                .p(px(10.0))
                .border_t_1()
                .border_color(rgb(pal.border_fg))
                .bg(rgb(pal.input_bg))
                .flex()
                .flex_col()
                .gap(px(4.0));
            for (idx, pol) in policies.into_iter().enumerate() {
                let selected = node.run_policy == pol;
                let nid = nid_policy.clone();
                menu = menu.child(
                    div()
                        .id(("sc-policy-choice", idx))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(8.0))
                        .rounded(px(7.0))
                        .bg(if selected {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                        } else {
                            rgb(pal.card_bg).into()
                        })
                        .hover(|s| s.bg(rgb(pal.hover_bg)))
                        .flex()
                        .items_center()
                        .justify_between()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.policy_menu_open = false;
                                this.pending_event = Some(SettingsComputersEvent::UpdatePolicy {
                                    node_id: nid.clone(),
                                    req: UpdateComputerPolicyRequest {
                                        run_policy: Some(pol),
                                        ..Default::default()
                                    },
                                });
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(if selected {
                                    FontWeight::BOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(rgb(if selected {
                                    pal.accent
                                } else {
                                    pal.text_primary
                                }))
                                .child(Self::policy_label(pol, lang)),
                        )
                        .when(selected, |row| {
                            row.child(tg_icon(IconName::SelectCheck, 13.0, pal.accent))
                        }),
                );
            }
            policy_card = policy_card.child(menu);
        }

        let top_more_menu = if self.detail_more_open {
            Some(
                div()
                    .p(px(10.0))
                    .border_t_1()
                    .border_color(rgb(pal.border_fg))
                    .bg(rgb(pal.input_bg))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("sc-detail-test-btn")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(7.0))
                            .bg(rgb(pal.card_bg))
                            .border_1()
                            .border_color(rgb(pal.border_fg))
                            .text_size(px(12.0))
                            .text_color(rgb(pal.text_primary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.detail_more_open = false;
                                    this.pending_event =
                                        Some(SettingsComputersEvent::TestNode(nid_test.clone()));
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "测试连接", "Test Connection")),
                    )
                    .when(is_cloud, |row| {
                        let edit_snapshot = node_clone_for_edit.clone();
                        row.child(
                            div()
                                .id("sc-detail-edit-cloud")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(7.0))
                                .bg(rgb(pal.card_bg))
                                .border_1()
                                .border_color(rgb(pal.border_fg))
                                .text_size(px(12.0))
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.open_cloud_form(this.current_bot_id, Some(&edit_snapshot));
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "编辑配置", "Edit Cloud Config")),
                        )
                        .child(
                            div()
                                .id("sc-detail-del-cloud")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(7.0))
                                .bg(rgb(pal.card_bg))
                                .border_1()
                                .border_color(rgb(pal.danger))
                                .text_size(px(12.0))
                                .text_color(rgb(pal.danger))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.detail_more_open = false;
                                        this.pending_event = Some(
                                            SettingsComputersEvent::DeleteNode(nid_del.clone()),
                                        );
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "删除云服务器", "Delete Cloud Server")),
                        )
                    }),
            )
        } else {
            None
        };

        settings_container(pal)
            .child(self.detail_back_bar(
                Some(tr_str(lang, "计算机详情", "Computer Details")),
                pal,
                lang,
                cx,
            ))
            .when_some(self.status_message.as_ref(), |col, msg| {
                col.child(
                    div()
                        .p(px(10.0))
                        .rounded(px(8.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                        .text_size(px(12.0))
                        .text_color(rgb(pal.text_primary))
                        .child(msg.clone()),
                )
            })
            .child(
                settings_card(pal)
                    .child(
                        div()
                            .p(px(16.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.0))
                                    .child(
                                        div()
                                            .w(px(44.0))
                                            .h(px(44.0))
                                            .rounded(px(12.0))
                                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(tg_icon(IconName::ComputerDesktop, 22.0, pal.accent)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(8.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(15.0))
                                                            .font_weight(FontWeight::BOLD)
                                                            .text_color(rgb(pal.text_primary))
                                                            .child(node.display_name.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .px(px(8.0))
                                                            .py(px(2.0))
                                                            .rounded(px(999.0))
                                                            .bg(rgb(pal.input_bg))
                                                            .text_size(px(11.0))
                                                            .text_color(rgb(pal.text_secondary))
                                                            .child(badge.to_string()),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.0))
                                                    .child(
                                                        div()
                                                            .w(px(7.0))
                                                            .h(px(7.0))
                                                            .rounded_full()
                                                            .bg(rgb(if is_online {
                                                                pal.success
                                                            } else {
                                                                pal.text_secondary
                                                            })),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .text_color(rgb(pal.text_secondary))
                                                            .child(format!(
                                                                "{} · {} · {}",
                                                                if is_online {
                                                                    tr_str(lang, "在线", "Online")
                                                                } else {
                                                                    tr_str(lang, "离线", "Offline")
                                                                },
                                                                endpoint,
                                                                node.os_name,
                                                            )),
                                                    ),
                                            )
                                            .when(
                                                local_tailnet_ip.is_some()
                                                    || local_transport_badge.is_some(),
                                                |col| {
                                                    col.child(
                                                        div()
                                                            .flex()
                                                            .flex_wrap()
                                                            .items_center()
                                                            .gap(px(6.0))
                                                            .when_some(
                                                                local_tailnet_ip.clone(),
                                                                |row, ts_ip| {
                                                                    row.child(
                                                                        div()
                                                                            .px(px(7.0))
                                                                            .py(px(1.5))
                                                                            .rounded(px(999.0))
                                                                            .bg(rgb(pal.input_bg))
                                                                            .text_size(px(10.5))
                                                                            .text_color(rgb(
                                                                                pal.text_primary,
                                                                            ))
                                                                            .child(format!(
                                                                                "Tailnet: {ts_ip}"
                                                                            )),
                                                                    )
                                                                },
                                                            )
                                                            .when_some(
                                                                local_transport_badge.clone(),
                                                                |row, (t_label, is_direct)| {
                                                                    let t_color = if is_direct {
                                                                        pal.success
                                                                    } else {
                                                                        pal.accent
                                                                    };
                                                                    row.child(
                                                                        div()
                                                                            .px(px(7.0))
                                                                            .py(px(1.5))
                                                                            .rounded(px(999.0))
                                                                            .bg(TdesktopThemePalette::rgba_with_alpha(
                                                                                t_color,
                                                                                0.14,
                                                                            ))
                                                                            .flex()
                                                                            .items_center()
                                                                            .gap(px(4.0))
                                                                            .child(tg_icon(
                                                                                IconName::Lock,
                                                                                10.0,
                                                                                t_color,
                                                                            ))
                                                                            .child(
                                                                                div()
                                                                                    .text_size(px(10.5))
                                                                                    .font_weight(
                                                                                        FontWeight::SEMIBOLD,
                                                                                    )
                                                                                    .text_color(rgb(
                                                                                        t_color,
                                                                                    ))
                                                                                    .child(format!(
                                                                                        "{t_label} · WireGuard E2E"
                                                                                    )),
                                                                            ),
                                                                    )
                                                                },
                                                            ),
                                                    )
                                                },
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .when(can_remove_offline_local, |row| {
                                        let nid_rm = nid_del_local.clone();
                                        row.child(
                                            div()
                                                .id("sc-detail-remove-offline-device")
                                                .cursor_pointer()
                                                .px(px(10.0))
                                                .py(px(7.0))
                                                .rounded(px(8.0))
                                                .bg(rgb(pal.input_bg))
                                                .border_1()
                                                .border_color(rgb(pal.danger))
                                                .hover(|s| {
                                                    s.bg(TdesktopThemePalette::rgba_with_alpha(
                                                        pal.danger,
                                                        0.12,
                                                    ))
                                                })
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(pal.danger))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.pending_event = Some(
                                                            SettingsComputersEvent::DeleteNode(
                                                                nid_rm.clone(),
                                                            ),
                                                        );
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(tr_str(
                                                    lang,
                                                    "移除离线设备",
                                                    "Remove Offline Device",
                                                )),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id("sc-detail-remote-control")
                                            .cursor_pointer()
                                            .px(px(12.0))
                                            .py(px(7.0))
                                            .rounded(px(8.0))
                                            .bg(rgb(pal.accent))
                                            .hover(|s| s.bg(rgb(pal.accent_hover)))
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.active_button_fg))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.pending_event = Some(
                                                        SettingsComputersEvent::OpenRemoteWorkbench(
                                                            nid_wb.clone(),
                                                        ),
                                                    );
                                                    cx.notify();
                                                }),
                                            )
                                            .child(tr_str(lang, "远程操控", "Remote Control")),
                                    )
                                    .child(
                                        div()
                                            .id("sc-detail-more-btn")
                                            .cursor_pointer()
                                            .w(px(30.0))
                                            .h(px(30.0))
                                            .rounded(px(8.0))
                                            .bg(rgb(pal.input_bg))
                                            .hover(|s| s.bg(rgb(pal.hover_bg)))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.detail_more_open = !this.detail_more_open;
                                                    cx.notify();
                                                }),
                                            )
                                            .child(tg_icon(IconName::MoreVertical, 15.0, pal.icon_fg)),
                                    ),
                            ),
                    )
                    .when_some(top_more_menu, |card, menu| card.child(menu)),
            )
            .child(acl_card)
            .child(policy_card)
            .into_any_element()
    }

    fn detail_back_bar(
        &self,
        title: Option<&str>,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("sc-detail-back-bar")
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(px(13.5))
            .font_weight(FontWeight::BOLD)
            .text_color(rgb(pal.accent))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.open_overview();
                    this.pending_event = Some(SettingsComputersEvent::BackToOverview);
                    cx.notify();
                }),
            )
            .child(tg_icon(IconName::BackArrow, 15.0, pal.accent))
            .child(
                title
                    .unwrap_or_else(|| tr_str(lang, "返回电脑列表", "Back to Computers"))
                    .to_string(),
            )
            .into_any_element()
    }

    fn render_cloud_form(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let auth_choices = [
            (
                CloudAuthMethod::SshPassword,
                tr_str(lang, "SSH 密码", "SSH Password"),
            ),
            (
                CloudAuthMethod::SshPrivateKey,
                tr_str(lang, "SSH 私钥", "SSH Private Key"),
            ),
            (
                CloudAuthMethod::AgentHttpToken,
                tr_str(lang, "Agent HTTP Token", "Agent HTTP Token"),
            ),
        ];

        settings_container(pal)
            .child(self.detail_back_bar(
                Some(tr_str(lang, "配置云服务器", "Configure Cloud Server")),
                pal,
                lang,
                cx,
            ))
            .when_some(self.status_message.as_ref(), |col, msg| {
                col.child(
                    div()
                        .p(px(10.0))
                        .rounded(px(8.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                        .text_size(px(12.0))
                        .text_color(rgb(pal.danger))
                        .child(msg.clone()),
                )
            })
            .child(
                settings_card(pal)
                    .p(px(16.0))
                    .gap(px(12.0))
                    .child(self.form_field_box(
                        "sc-f-name",
                        tr_str(lang, "名称", "Display Name"),
                        &self.form_name,
                        self.input_name.as_ref(),
                        CloudFormField::DisplayName,
                        "prod-cloud-01",
                        false,
                        pal,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div().flex_1().child(self.form_field_box(
                                    "sc-f-host",
                                    tr_str(lang, "主机地址 / IP", "Host / IP"),
                                    &self.form_host,
                                    self.input_host.as_ref(),
                                    CloudFormField::Host,
                                    "203.0.113.10",
                                    false,
                                    pal,
                                    cx,
                                )),
                            )
                            .child(
                                div().w(px(110.0)).child(self.form_field_box(
                                    "sc-f-port",
                                    tr_str(lang, "端口", "Port"),
                                    &self.form_port,
                                    self.input_port.as_ref(),
                                    CloudFormField::Port,
                                    "22",
                                    false,
                                    pal,
                                    cx,
                                )),
                            ),
                    )
                    .child(self.form_field_box(
                        "sc-f-user",
                        tr_str(lang, "用户名", "Username"),
                        &self.form_username,
                        self.input_username.as_ref(),
                        CloudFormField::Username,
                        "root",
                        false,
                        pal,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(lang, "认证方式", "Auth Method")),
                            )
                            .child(div().flex().flex_wrap().gap(px(6.0)).children(
                                auth_choices.into_iter().enumerate().map(|(i, (method, label))| {
                                    let active = self.form_auth_method == method;
                                    div()
                                        .id(("sc-auth-method", i))
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(6.0))
                                        .rounded(px(8.0))
                                        .bg(if active {
                                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                                        } else {
                                            rgb(pal.input_bg).into()
                                        })
                                        .border_1()
                                        .border_color(rgb(if active {
                                            pal.accent
                                        } else {
                                            pal.border_fg
                                        }))
                                        .text_size(px(12.0))
                                        .font_weight(if active {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .text_color(rgb(if active {
                                            pal.accent
                                        } else {
                                            pal.text_primary
                                        }))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.form_auth_method = method;
                                                cx.notify();
                                            }),
                                        )
                                        .child(label.to_string())
                                }),
                            )),
                    )
                    .child(self.form_field_box(
                        "sc-f-cred",
                        tr_str(lang, "密码 / 私钥 / Token", "Password / Private Key / Token"),
                        &self.form_credential,
                        self.input_credential.as_ref(),
                        CloudFormField::Credential,
                        if self.form_auth_method == CloudAuthMethod::SshPrivateKey {
                            "-----BEGIN OPENSSH PRIVATE KEY-----"
                        } else {
                            "••••••••"
                        },
                        self.form_auth_method == CloudAuthMethod::SshPassword,
                        pal,
                        cx,
                    ))
                    .child(self.form_field_box(
                        "sc-f-agent-url",
                        tr_str(lang, "Agent URL (可选)", "Agent URL (Optional)"),
                        &self.form_agent_url,
                        self.input_agent_url.as_ref(),
                        CloudFormField::AgentUrl,
                        "http://203.0.113.10:8787",
                        false,
                        pal,
                        cx,
                    ))
                    .child(self.form_field_box(
                        "sc-f-workdir",
                        tr_str(lang, "工作目录", "Workdir"),
                        &self.form_workdir,
                        self.input_workdir.as_ref(),
                        CloudFormField::Workdir,
                        "/workspace",
                        false,
                        pal,
                        cx,
                    ))
                    .child(
                        div()
                            .id("sc-f-bind-bot-toggle")
                            .cursor_pointer()
                            .py(px(6.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.form_bind_to_current_bot = !this.form_bind_to_current_bot;
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(pal.text_primary))
                                            .child(tr_str(
                                                lang,
                                                "绑定到当前 Bot",
                                                "Bind to Current Bot",
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(pal.text_secondary))
                                            .child(match self.current_bot_id {
                                                Some(bid) => format!("Bot #{bid}"),
                                                None => tr_str(
                                                    lang,
                                                    "当前未在 Bot 对话中打开",
                                                    "No active Bot selected",
                                                )
                                                .to_string(),
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(38.0))
                                    .h(px(20.0))
                                    .rounded_full()
                                    .bg(rgb(if self.form_bind_to_current_bot {
                                        pal.accent
                                    } else {
                                        pal.input_bg
                                    }))
                                    .border_1()
                                    .border_color(rgb(if self.form_bind_to_current_bot {
                                        pal.accent
                                    } else {
                                        pal.border_fg
                                    }))
                                    .px(px(2.0))
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .when(!self.form_bind_to_current_bot, |el| el.justify_start())
                                    .child(div().w(px(14.0)).h(px(14.0)).rounded_full().bg(rgb(0xFFFFFF))),
                            ),
                    )
                    .child(
                        div()
                            .pt(px(6.0))
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .id("sc-f-cancel-btn")
                                    .cursor_pointer()
                                    .px(px(14.0))
                                    .py(px(8.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(rgb(pal.border_fg))
                                    .text_size(px(12.5))
                                    .text_color(rgb(pal.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.open_overview();
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "取消", "Cancel")),
                            )
                            .child(
                                div()
                                    .id("sc-f-save-btn")
                                    .cursor_pointer()
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(pal.accent))
                                    .hover(|s| s.bg(rgb(pal.accent_hover)))
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.active_button_fg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.submit_cloud_form();
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(
                                        lang,
                                        "测试连接并保存到云端",
                                        "Test & Save to Cloud",
                                    )),
                            ),
                    ),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn form_field_box(
        &self,
        id: &'static str,
        label: &str,
        value: &str,
        ctrl: Option<&Entity<TextInputController>>,
        field: CloudFormField,
        placeholder: &str,
        masked: bool,
        pal: TdesktopThemePalette,
        cx: &mut Context<Self>,
    ) -> Div {
        let focused = self.form_focus == field;
        let (committed, preedit) = ctrl
            .map(|c| c.read(cx).text_and_preedit(value.to_string()))
            .unwrap_or_else(|| (value.to_string(), None));

        div()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.text_secondary))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .id(id)
                    .cursor_text()
                    .min_h(px(36.0))
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(rgb(if focused { pal.accent } else { pal.border_fg }))
                    .flex()
                    .items_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.ensure_form_inputs(cx);
                            this.form_focus = field;
                            cx.notify();
                        }),
                    )
                    .child(render_ime_text_preview(
                        &committed,
                        preedit.as_deref(),
                        placeholder,
                        focused,
                        masked,
                        pal.text_primary,
                        pal,
                    )),
            )
    }
}

impl Render for SettingsComputersSections {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_form_inputs(cx);
        self.ensure_permission_poll_loop(cx);
        let pal = self.palette;
        let lang = self.language_id.clone();
        let is_overview = self.page == SettingsComputersPage::Overview;
        let page_view = match self.page.clone() {
            SettingsComputersPage::Overview => self.render_overview(pal, &lang, cx),
            SettingsComputersPage::Detail(node_id) => {
                self.render_detail(&node_id, pal, &lang, cx)
            }
            SettingsComputersPage::CloudForm => self.render_cloud_form(pal, &lang, cx),
        };
        div()
            .relative()
            .size_full()
            .bg(rgb(pal.window_bg))
            .flex()
            .flex_row()
            .overflow_hidden()
            .child(self.render_uu_sidebar(pal, &lang, cx))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(self.render_uu_header(pal, &lang, cx))
                    .child(
                        div()
                            .id("uu-computers-content-scroll")
                            .flex_1()
                            .min_h(px(0.0))
                            .w_full()
                            .overflow_y_scroll()
                            .when(is_overview, |el| el.p(px(22.0)))
                            .child(page_view),
                    ),
            )
            .when(self.permission_modal_open && cfg!(target_os = "macos"), |el| {
                let snap = cached_permissions_snapshot();
                el.child(render_gemini_permission_modal(
                    &snap,
                    pal,
                    &lang,
                    cx.listener(|this, _, _, cx| {
                        this.permission_modal_open = false;
                        cx.notify();
                    }),
                    cx.listener(|this, _, _, cx| {
                        this.permission_modal_open = false;
                        cx.notify();
                    }),
                ))
            })
    }
}
