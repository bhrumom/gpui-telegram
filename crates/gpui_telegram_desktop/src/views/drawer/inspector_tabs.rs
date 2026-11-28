//! Inspector Drawer Tabs models and Profile/Members GPUI renderers (`info/`).

use crate::i18n::tr_str;
use crate::theme::assets::{IconName, tg_icon};
use crate::theme::palette::TdesktopThemePalette;
use crate::views::settings::sections::settings_ai_providers::{
    SubscriptionAccountDto, sub_dto_to_account_dto,
};
use crate::views::ContextInspector;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_client::net::hermes_client::MagpieStateDto;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InspectorTab {
    Profile,
    Members,
    SharedMedia,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SharedMediaSubTab {
    Media,
    Files,
    Voice,
    Links,
    Music,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberItem {
    pub id: u64,
    pub name: String,
    pub initials: String,
    pub color: u32,
    pub role: String,
    pub is_online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMediaItem {
    pub id: u64,
    pub url: String,
    pub local_path: Option<String>,
    pub caption: String,
    pub is_video: bool,
    pub duration_sec: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedFileItem {
    pub name: String,
    pub size_text: String,
    pub extension: String,
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedLinkItem {
    pub title: String,
    pub url: String,
    pub domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedVoiceItem {
    pub id: u64,
    pub title: String,
    pub duration_sec: u32,
    pub date: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InspectorSubpage {
    #[default]
    Main,
    ComputersList,
    BotConfigEditor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BotConfigFocusField {
    #[default]
    None,
    Persona,
    Model,
    CodingWorkspace,
    CodingModel,
    MemoryDraft,
}

/// Active tab of the inline Bot Companion SidePanel beside `ConversationStage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BotCompanionTab {
    #[default]
    Workflow,
    Knowledge,
    Activity,
}

/// Live memory entry belonging to the currently selected native Bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotFusionMemoryItem {
    pub id: String,
    pub content: String,
    pub origin: String,
    pub updated_at: String,
}

/// Live skill installed on the currently selected native Bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotFusionSkillItem {
    pub skill_id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

/// Registered MCP server available to the currently selected native Bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotFusionMcpItem {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub enabled: bool,
}

/// Scheduled routine owned by the currently selected native Bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotFusionRoutineItem {
    pub id: String,
    pub name: String,
    pub schedule_label: String,
    pub active: bool,
    pub instruction: String,
}

/// Workflow summary associated with the active Bot workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotFusionWorkflowItem {
    pub id: String,
    pub title: String,
    pub step_count: usize,
    pub last_run_status: Option<String>,
    pub last_run_id: Option<String>,
    pub pending_approval_node: Option<String>,
    pub node_labels: Vec<String>,
}

/// Queued message waiting in the Bot's OpenBot DrainScheduler mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotFusionQueueItem {
    pub delivery_id: String,
    pub text: String,
    pub status: String,
}

/// Interactive `ask_ui` card surfaced directly above `CanonicalComposer` (e.g. Workflow approval gate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotFusionAskCard {
    pub workflow_id: String,
    pub run_id: String,
    pub node_id: String,
    pub title: String,
    pub prompt: String,
}

/// Collapsible execution step entry rendered in Bot Activity & inline steps timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotFusionStepItem {
    pub label: String,
    pub status: String,
    pub detail: String,
}

/// `$` composer trigger candidate exposing THIS Bot's Skills, Routines, Workflows & quick actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotFusionDollarCandidate {
    pub token: String,
    pub title: String,
    pub subtitle: String,
    pub badge: String,
}

/// Authoritative snapshot of OpenBot/Orca native capabilities bound 1:1 to the active Fabushi Bot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BotFusionSnapshot {
    pub fabushi_agent_id: String,
    pub openbot_agent_id: String,
    pub bot_display_name: String,
    pub memories: Vec<BotFusionMemoryItem>,
    pub skills: Vec<BotFusionSkillItem>,
    pub mcp_servers: Vec<BotFusionMcpItem>,
    pub routines: Vec<BotFusionRoutineItem>,
    pub workflows: Vec<BotFusionWorkflowItem>,
    pub queue_items: Vec<BotFusionQueueItem>,
    pub queue_paused: bool,
    pub active_turn_id: Option<String>,
    pub ask_cards: Vec<BotFusionAskCard>,
    pub recent_steps: Vec<BotFusionStepItem>,
    pub side_question_last: Option<(String, String)>,
    pub dollar_candidates: Vec<BotFusionDollarCandidate>,
    pub companion_open: bool,
    pub companion_tab: BotCompanionTab,
    pub selected_workflow_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectorAction {
    StartVoiceCall(String),
    StartVideoCall(String),
    CopyText(String),
    OpenAddMember,
    CycleMemberRole(u64),
    RevokeInviteLink,
    ToggleNotifications(bool),
    ChangeAutoDelete(String),
    OpenMedia { url: String, is_video: bool },
    OpenEditPeer,
    LaunchMiniApp(String),
    ToggleAttachMenuBot(u64),
    RunBotCommand(String),
    OpenBotComputersSubpage,
    CloseBotComputersSubpage,
    OpenRemoteWorkbench { node_id: String },
    OpenComputerDetailInSettings { node_id: String },
    OpenCreateCloudComputerModal { bind_bot_id: Option<u64> },
    BindBotToComputer { bot_id: u64, node_id: Option<String> },
    SaveBotComputerConfig {
        bot_id: u64,
        persona_prompt: String,
        preferred_model: String,
        approval_mode: gpui_telegram_proto::ComputerRunPolicy,
        allow_cross_device_control: bool,
        allow_cloud_self_config: bool,
    },
    PatchFabushiBot {
        bot_id: u64,
        req: gpui_telegram_proto::miniapp_contracts::PatchFabushiBotRequest,
    },
    FocusBotComposer,
    OpenMiniAppTos,
    OpenBotPrivacyPolicy(String),
    OpenBotSettings(u64),
    OpenAiProvidersSettings,
    OpenAddBotToGroup(u64),
    ReportBot(u64),
    ToggleBlockBot(u64),
    // Native Bot Fusion (OpenBot + Orca unified inside existing Bot Inspector)
    AddBotMemory(String),
    DeleteBotMemory(String),
    TidyBotMemories,
    InstallBotSkill {
        slug: String,
        name: String,
        description: String,
    },
    ToggleBotSkill {
        skill_id: String,
        enabled: bool,
    },
    CreatePresetRoutine {
        preset: String,
    },
    ToggleBotRoutine {
        routine_id: String,
        active: bool,
    },
    RunBotRoutineNow {
        routine_id: String,
    },
    CreateStarterWorkflow,
    RunBotWorkflow {
        workflow_id: String,
    },
    DecideBotWorkflowApproval {
        workflow_id: String,
        run_id: String,
        node_id: String,
        approve: bool,
    },
    OpenBotCompanion(BotCompanionTab),
}

/// Summary of a published/user Book, Music, or Game creation produced by the active Bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotPublishedCreationSummary {
    pub creation_id: String,
    pub title: String,
    pub kind_label: String,
    pub status_label: String,
    pub kind_code: &'static str,
}

/// Drill-down subpages inside the embedded OpenBot right `SidePanel` `Settings` pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenBotSettingsSubpage {
    Creations,
    BackupModels,
    Instructions,
    Memories,
    Skills,
    DataTables,
    Routines,
    Workflows,
    Usage,
    Permissions,
    Folders,
    Advanced,
    SharedMedia,
    ChannelMemories,
    ChannelRoutines,
    ChannelFolders,
}

impl OpenBotSettingsSubpage {
    pub fn title(self, lang: &str) -> &'static str {
        match self {
            Self::Creations => tr_str(lang, "作品集", "Published Creations"),
            Self::BackupModels => tr_str(lang, "备用模型", "Backup Models"),
            Self::Instructions => tr_str(lang, "指令 (AGENTS.md)", "Instructions"),
            Self::Memories => tr_str(lang, "记忆", "Memories"),
            Self::Skills => tr_str(lang, "技能与 MCP", "Skills & MCP"),
            Self::DataTables => tr_str(lang, "数据表", "Data Tables"),
            Self::Routines => tr_str(lang, "自动化任务", "Routines"),
            Self::Workflows => tr_str(lang, "工作流", "Workflows"),
            Self::Usage => tr_str(lang, "用量", "Usage"),
            Self::Permissions => tr_str(lang, "权限", "Permissions"),
            Self::Folders => tr_str(lang, "文件夹", "Folders"),
            Self::Advanced => tr_str(lang, "高级", "Advanced"),
            Self::SharedMedia => tr_str(lang, "共享媒体与文件", "Shared Media & Files"),
            Self::ChannelMemories => tr_str(lang, "频道记忆", "Channel Memories"),
            Self::ChannelRoutines => tr_str(lang, "频道自动化任务", "Channel Routines"),
            Self::ChannelFolders => tr_str(lang, "频道文件夹", "Channel Folders"),
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Creations => "creations",
            Self::BackupModels => "backup_models",
            Self::Instructions => "instructions",
            Self::Memories => "memories",
            Self::Skills => "skills",
            Self::DataTables => "data_tables",
            Self::Routines => "routines",
            Self::Workflows => "workflows",
            Self::Usage => "usage",
            Self::Permissions => "permissions",
            Self::Folders => "folders",
            Self::Advanced => "advanced",
            Self::SharedMedia => "shared_media",
            Self::ChannelMemories => "channel_memories",
            Self::ChannelRoutines => "channel_routines",
            Self::ChannelFolders => "channel_folders",
        }
    }
}

/// Active inline IME text input inside the embedded OpenBot `Settings` pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenBotInspectorTextField {
    BotName,
    BotTitle,
    BotDescription,
    BotInstructions,
    BotBackupModelDraft,
    BotFolderDraft,
    ChannelName,
    ChannelTitle,
    ChannelInstructions,
    ChannelMemoryDraft,
    ChannelFolderDraft,
}

/// Explicit inline confirmation guard for permission widening & destructive actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenBotConfirmGuard {
    WidenPermissionsFull,
    BotNewSession,
    BotDeleteHistory,
    BotDeleteAgent,
    ChannelNewConversation,
    ChannelCompactContext,
    ChannelDeleteHistory,
    ChannelRemoveMember { user_id: u64, name: String },
    ContactBlock,
}

impl OpenBotConfirmGuard {
    /// Returns `true` for destructive guards that require `ConfirmDialog`'s cancel-first focus (`PAIR-24`).
    pub const fn is_destructive(&self) -> bool {
        matches!(
            self,
            Self::WidenPermissionsFull
                | Self::BotDeleteHistory
                | Self::BotDeleteAgent
                | Self::ChannelDeleteHistory
                | Self::ChannelRemoveMember { .. }
                | Self::ContactBlock
        )
    }
}

/// Group/Channel browser access policy matching OpenBot `ChannelBrowser`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ChannelBrowserAccess {
    #[default]
    Everyone,
    Member(u64),
    None,
}

impl ChannelBrowserAccess {
    pub fn as_policy_str(&self) -> String {
        match self {
            Self::Everyone => "everyone".to_string(),
            Self::Member(uid) => format!("member:{uid}"),
            Self::None => "none".to_string(),
        }
    }
}

/// Additive M4 actions emitted by embedded OpenBot Bot/Channel/Contact settings panes
/// and drained directly by `AppShell` (`shell_bot_fusion.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenBotInspectorAction {
    OpenSidePanelPane(crate::views::openbot::side_panel::SidePanelPaneKind),
    OpenWorkflowsPage,
    OpenPublishedCreation {
        creation_id: String,
        kind_code: &'static str,
    },
    OpenContentHubCreations,
    UpdateChatInfo {
        title: String,
        about: String,
    },
    PromoteChannelLead {
        user_id: u64,
        member_name: String,
    },
    RemoveChannelMember {
        user_id: u64,
        member_name: String,
    },
    CompactChannelContext,
    StartNewChannelConversation,
    DeleteChatHistory,
    DeleteBot {
        bot_id: u64,
        bot_name: String,
    },
    SetChatBubbleColor(Option<String>),
    SetBotBusyMode(String),
    SetBotPermissions {
        access: String,
        computer_use: bool,
        allow_automation: bool,
    },
    SetChannelBrowserPolicy(String),
    SetChannelContextIsolation(bool),
    SetChannelIcon {
        icon_name: Option<String>,
        hue: Option<u16>,
    },
    NotifyStatus(String),
}

/// Live AI provider & model catalog synchronized from Settings (`MagpieStateDto` + discovered subscriptions).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InspectorAiModelCatalog {
    /// Active provider ID in Settings (empty when no provider is configured/selected).
    pub active_provider_id: String,
    /// Human-readable active provider name in Settings.
    pub active_provider_name: String,
    /// Currently active model in Settings (empty when none configured).
    pub active_model: String,
    /// Whether Settings currently requires selecting/configuring a provider.
    pub needs_provider_selection: bool,
    /// True once at least one Settings/Magpie snapshot has been synced.
    pub synced: bool,
    /// Real models available on the currently active Settings provider/subscription.
    pub active_provider_models: Vec<String>,
    /// Routing groups (`group/<id>`) configured in Settings.
    pub routing_group_models: Vec<String>,
}

impl InspectorAiModelCatalog {
    /// Returns `true` for empty or sentinel placeholder model strings that mean "inherit Settings default".
    pub fn is_placeholder_model(model: &str) -> bool {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return true;
        }
        matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "default"
                | "默认"
                | "默认模型"
                | "auto"
                | "hermes-local-v1"
                | "local_hermes/hermes-local-v1"
        )
    }

    fn is_usable_model(model: &str, allow_local_hermes: bool) -> bool {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            return false;
        }
        if allow_local_hermes {
            !matches!(
                trimmed.to_ascii_lowercase().as_str(),
                "default" | "默认" | "默认模型" | "auto"
            )
        } else {
            !Self::is_placeholder_model(trimmed)
        }
    }

    /// Builds a live model catalog directly from Settings' `MagpieStateDto` and subscription DTOs.
    pub fn from_magpie_state(
        state: &MagpieStateDto,
        server_subs: &[SubscriptionAccountDto],
        local_subs: &[SubscriptionAccountDto],
    ) -> Self {
        let mut merged_subs: Vec<SubscriptionAccountDto> = if !server_subs.is_empty() {
            server_subs.to_vec()
        } else {
            state
                .subscriptions
                .iter()
                .map(sub_dto_to_account_dto)
                .collect()
        };

        for local in local_subs {
            if let Some(existing) = merged_subs.iter_mut().find(|s| s.id == local.id) {
                if !local.models.is_empty() {
                    let mut combined = local.models.clone();
                    for m in &existing.models {
                        if !combined.iter().any(|c| c.eq_ignore_ascii_case(m)) {
                            combined.push(m.clone());
                        }
                    }
                    existing.models = combined;
                }
            } else {
                merged_subs.push(local.clone());
            }
        }

        let active_pid_raw = state.active_provider.trim();
        let active_sub = if state.needs_provider_selection {
            None
        } else {
            merged_subs
                .iter()
                .find(|sub| {
                    !active_pid_raw.is_empty()
                        && (sub.id == active_pid_raw
                            || (sub.kind == "google_vertex"
                                && active_pid_raw == "magpie_provider:google-vertex"))
                })
                .or_else(|| {
                    active_pid_raw
                        .is_empty()
                        .then(|| merged_subs.iter().find(|sub| sub.active))
                        .flatten()
                })
        };

        let active_prov = if state.needs_provider_selection {
            None
        } else {
            state
                .providers
                .iter()
                .find(|p| !active_pid_raw.is_empty() && p.provider_id == active_pid_raw)
                .or_else(|| state.providers.iter().find(|p| p.is_active))
        };

        let allow_local = active_pid_raw == "local_hermes"
            && state.allow_local_fallback
            && !state.needs_provider_selection;

        let active_provider_id = if state.needs_provider_selection {
            String::new()
        } else if !active_pid_raw.is_empty() {
            active_pid_raw.to_string()
        } else if let Some(sub) = active_sub {
            sub.id.clone()
        } else if let Some(prov) = active_prov {
            prov.provider_id.clone()
        } else {
            String::new()
        };

        let active_provider_name = if state.needs_provider_selection {
            String::new()
        } else if let Some(sub) = active_sub {
            sub.label
                .strip_prefix("Magpie ")
                .unwrap_or(&sub.label)
                .trim_end_matches(" (Magpie)")
                .trim()
                .to_string()
        } else if let Some(prov) = active_prov.filter(|p| !p.name.trim().is_empty()) {
            prov.name.trim().to_string()
        } else if !active_provider_id.is_empty() && active_provider_id != "local_hermes" {
            active_provider_id.clone()
        } else {
            String::new()
        };

        let active_model = if state.needs_provider_selection {
            String::new()
        } else if Self::is_usable_model(&state.active_model, allow_local) {
            state.active_model.trim().to_string()
        } else if let Some(m) = active_prov
            .map(|p| p.default_model.trim())
            .filter(|m| Self::is_usable_model(m, allow_local))
        {
            m.to_string()
        } else if let Some(m) = active_sub
            .and_then(|s| s.models.iter().find(|m| Self::is_usable_model(m, allow_local)))
        {
            m.trim().to_string()
        } else {
            String::new()
        };

        let mut active_provider_models: Vec<String> = Vec::new();
        let mut push_active_model = |candidate: &str| {
            let trimmed = candidate.trim();
            if Self::is_usable_model(trimmed, allow_local)
                && !active_provider_models
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(trimmed))
            {
                active_provider_models.push(trimmed.to_string());
            }
        };

        if !state.needs_provider_selection {
            push_active_model(&active_model);
            if let Some(sub) = active_sub {
                for m in &sub.models {
                    push_active_model(m);
                }
            }
            if let Some(prov) = active_prov {
                push_active_model(&prov.default_model);
                for m in &prov.models {
                    push_active_model(m);
                }
                let preset_key = prov
                    .preset_id
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(prov.provider_id.as_str());
                if let Some(preset) = state.presets.iter().find(|pr| pr.id == preset_key) {
                    if let Some(reg_id) = prov.region_id.as_deref() {
                        if let Some(reg) = preset.regions.iter().find(|r| r.id == reg_id) {
                            for m in &reg.models {
                                push_active_model(m);
                            }
                        }
                    }
                    push_active_model(&preset.default_model);
                    for m in &preset.suggested_models {
                        push_active_model(m);
                    }
                }
            }
        }

        let mut routing_group_models: Vec<String> = Vec::new();
        for group in &state.routing_groups {
            let gid = group.id.trim();
            if !gid.is_empty() {
                let target = format!("group/{gid}");
                if !routing_group_models
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(&target))
                {
                    routing_group_models.push(target);
                }
            }
        }

        Self {
            active_provider_id,
            active_provider_name,
            active_model,
            needs_provider_selection: state.needs_provider_selection,
            synced: true,
            active_provider_models,
            routing_group_models,
        }
    }

    /// Returns all real selectable model IDs from Settings (active provider models followed by routing groups).
    pub fn selectable_models(&self) -> Vec<String> {
        let mut out = self.active_provider_models.clone();
        for g in &self.routing_group_models {
            if !out.iter().any(|existing| existing.eq_ignore_ascii_case(g)) {
                out.push(g.clone());
            }
        }
        out
    }

    /// Human-readable summary of the active Settings provider & model (e.g. `"Google Vertex AI · gemini-2.5-pro"`).
    pub fn active_summary(&self) -> Option<String> {
        if self.needs_provider_selection {
            return None;
        }
        if !self.active_provider_name.is_empty() && !self.active_model.is_empty() {
            Some(format!("{} · {}", self.active_provider_name, self.active_model))
        } else if !self.active_model.is_empty() {
            Some(self.active_model.clone())
        } else if !self.active_provider_name.is_empty() {
            Some(self.active_provider_name.clone())
        } else {
            None
        }
    }

    /// Returns `true` when `model` is empty (follows Settings) or matches a real model available in Settings.
    pub fn is_model_consistent_with_settings(&self, model: &str) -> bool {
        let trimmed = model.trim();
        if trimmed.is_empty() || !self.synced {
            return true;
        }
        if self.needs_provider_selection {
            return false;
        }
        let selectable = self.selectable_models();
        if selectable.is_empty() {
            return self.active_model.is_empty() || self.active_model.eq_ignore_ascii_case(trimmed);
        }
        selectable.iter().any(|m| m.eq_ignore_ascii_case(trimmed))
    }
}

pub struct ProfileTabModel {
    pub name: String,
    pub username: String,
    pub phone: String,
    pub bio: String,
    pub status: String,
    pub avatar_initials: String,
    pub avatar_color: u32,
    pub avatar_path: Option<String>,
    pub facts: Vec<(String, String)>,
    pub is_notifications_enabled: bool,
    pub is_bot: bool,
    pub bot_id: u64,
    pub bot_persona_prompt: String,
    pub bot_commands: Vec<(String, String)>,
    pub miniapp_title: Option<String>,
    pub miniapp_url: Option<String>,
    pub in_attach_menu: bool,
    pub bot_mau_count: i64,
    pub bot_has_main_app: bool,
    pub bot_privacy_policy_url: String,
    pub bot_has_settings: bool,
    pub bot_can_join_groups: bool,
    pub is_viewer_owner: bool,
    pub is_verified: bool,
    pub is_blocked: bool,
}

pub struct MembersTabModel {
    pub members: Vec<MemberItem>,
    pub search_query: String,
    pub invite_link: String,
    pub is_group_or_channel: bool,
}

pub struct SharedMediaTabModel {
    pub active_subtab: SharedMediaSubTab,
    pub media_count: u32,
    pub media_items: Vec<SharedMediaItem>,
    pub files: Vec<SharedFileItem>,
    pub links: Vec<SharedLinkItem>,
    pub voice_notes: Vec<SharedVoiceItem>,
    pub music_items: Vec<SharedFileItem>,
}

pub struct SettingsTabModel {
    pub e2ee_fingerprint: String,
    pub auto_delete_period: String,
    pub notification_sound: String,
}

pub struct InspectorTabsView;

impl InspectorTabsView {
    pub fn render_profile(
        model: &ProfileTabModel,
        is_secret_chat_active: bool,
        is_contact_blocked: bool,
        copied_feedback: Option<&String>,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<ContextInspector>,
    ) -> AnyElement {
        let avatar_color = if model.avatar_color != 0 { model.avatar_color } else { pal.accent };

        let fallback_initials = {
            let trimmed_init = model.avatar_initials.trim();
            if !trimmed_init.is_empty() && trimmed_init != "?" {
                trimmed_init.to_string()
            } else {
                let derived = crate::views::app_shell::shell_truth::derive_initials(&model.name);
                if derived.is_empty() {
                    "?".to_string()
                } else {
                    derived
                }
            }
        };

        let avatar = if let Some(ref path) = model.avatar_path {
            div().w(px(80.0)).h(px(80.0)).rounded_full().overflow_hidden()
                .child(img(path.clone()).w(px(80.0)).h(px(80.0)).object_fit(ObjectFit::Cover))
                .into_any_element()
        } else {
            div().w(px(80.0)).h(px(80.0)).rounded_full().bg(rgb(avatar_color)).flex().items_center().justify_center()
                .text_size(px(26.0)).font_weight(FontWeight::BOLD).text_color(rgb(pal.active_button_fg))
                .child(fallback_initials)
                .into_any_element()
        };

        let mut rows: Vec<(String, String)> = Vec::new();
        if !model.phone.is_empty() {
            rows.push((tr_str(lang, "手机号", "Mobile").into(), model.phone.clone()));
        }
        if !model.username.is_empty() {
            rows.push((tr_str(lang, "用户名", "Username").into(), model.username.clone()));
        }
        if !model.bio.is_empty() {
            rows.push((tr_str(lang, "简介", "Bio").into(), model.bio.clone()));
        }
        rows.extend(model.facts.iter().cloned());

        div().flex().flex_col().gap(px(12.0))
            .child(div().flex().flex_col().items_center().gap(px(6.0)).py(px(8.0)).child(avatar)
                .child(div().text_size(px(16.0)).font_weight(FontWeight::BOLD).text_color(rgb(pal.text_primary))
                    .child(if model.name.is_empty() { tr_str(lang, "未选择对话", "No chat selected").to_string() } else { model.name.clone() }))
                .child(div().text_size(px(12.0)).text_color(rgb(pal.text_secondary)).child(model.status.clone())))
            .child(div().flex().items_center().justify_around().gap(px(6.0))
                .child(Self::action_pill(IconName::Mute, tr_str(lang, "静音", "Mute"), pal, move |this| {
                    this.profile.is_notifications_enabled = !this.profile.is_notifications_enabled;
                    this.pending_action = Some(InspectorAction::ToggleNotifications(this.profile.is_notifications_enabled));
                }, cx))
                .child(Self::action_pill(IconName::Edit, tr_str(lang, "编辑", "More"), pal, |this| {
                    this.pending_action = Some(InspectorAction::OpenEditPeer);
                }, cx)))
            .children(rows.into_iter().map(|(label, val)| {
                let cp = val.clone();
                let copied_prefix = tr_str(lang, "已复制", "Copied").to_string();
                div().cursor_pointer().p(px(10.0)).rounded(px(8.0)).bg(rgb(pal.card_bg)).hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                    .flex().items_center().justify_between()
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _e, _w, cx| {
                        this.show_copied_feedback(format!("{}: {}", copied_prefix, cp), cx);
                        this.pending_action = Some(InspectorAction::CopyText(cp.clone()));
                        cx.notify();
                    }))
                    .child(div().flex().flex_col().gap(px(2.0))
                        .child(div().text_size(px(12.5)).text_color(rgb(pal.text_primary)).child(val))
                        .child(div().text_size(px(11.0)).text_color(rgb(pal.text_secondary)).child(label)))
                    .child(tg_icon(IconName::Copy, 14.0, pal.icon_fg))
            }))
            .when_some(copied_feedback.cloned(), |el, hint| {
                el.child(div().text_size(px(11.0)).text_color(rgb(pal.success)).child(hint))
            })
            .when(model.is_bot, |el| {
                let mini_title = model
                    .miniapp_title
                    .clone()
                    .unwrap_or_else(|| tr_str(lang, "打开小程序", "Open Mini App").to_string());
                let mini_url = model.miniapp_url.clone();
                let bot_id = model.bot_id;
                let in_attach = model.in_attach_menu;
                let persona = model.bot_persona_prompt.clone();
                let cmds = model.bot_commands.clone();
                el.child(
                    div()
                        .w_full()
                        .p(px(10.0))
                        .rounded(px(10.0))
                        .bg(rgb(pal.card_bg))
                        .border_1()
                        .border_color(rgb(pal.border_fg))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(tg_icon(IconName::BotRobot, 15.0, pal.accent))
                                        .child(
                                            div()
                                                .text_size(px(12.5))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(pal.text_primary))
                                                .child(tr_str(lang, "机器人与小程序", "Bot & Mini App")),
                                        ),
                                )
                                .child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(999.0))
                                        .bg(rgb(pal.composer_bg))
                                        .text_size(px(10.5))
                                        .text_color(rgb(pal.accent))
                                        .child("AI Companion"),
                                ),
                        )
                        .when_some(mini_url, |col, url| {
                            col.child(
                                div()
                                    .id("inspector-launch-miniapp-btn")
                                    .cursor_pointer()
                                    .w_full()
                                    .h(px(34.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(pal.accent))
                                    .hover(|s| s.bg(rgb(pal.accent_hover)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(6.0))
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0xFFFFFF))
                                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                        this.pending_action = Some(InspectorAction::LaunchMiniApp(url.clone()));
                                        cx.notify();
                                    }))
                                    .child("🚀")
                                    .child(mini_title),
                            )
                        })
                        .when(bot_id > 0, |col| {
                            col.child(
                                div()
                                    .id("inspector-toggle-attach-btn")
                                    .cursor_pointer()
                                    .w_full()
                                    .py(px(6.0))
                                    .px(px(8.0))
                                    .rounded(px(7.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(rgb(pal.border_fg))
                                    .hover(|s| s.bg(rgb(pal.dialogs_hover_bg)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal.text_link))
                                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                        this.pending_action = Some(InspectorAction::ToggleAttachMenuBot(bot_id));
                                        cx.notify();
                                    }))
                                    .child(if in_attach {
                                        tr_str(lang, "从附件菜单移除", "Remove from Attachment Menu")
                                    } else {
                                        tr_str(lang, "+ 添加到附件菜单", "+ Add to Attachment Menu")
                                    }),
                            )
                        })
                        .when(!persona.trim().is_empty(), |col| {
                            col.child(
                                div()
                                    .p(px(8.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(pal.input_bg))
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_secondary))
                                            .child(tr_str(lang, "AI 人设 / System Persona", "AI Persona")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(rgb(pal.text_primary))
                                            .child(persona),
                                    ),
                            )
                        })
                        .when(!cmds.is_empty(), |col| {
                            col.child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(5.0))
                                    .children(cmds.into_iter().map(|(cmd, desc)| {
                                        let run_cmd = if cmd.starts_with('/') {
                                            cmd.clone()
                                        } else {
                                            format!("/{}", cmd)
                                        };
                                        let label = if desc.is_empty() {
                                            run_cmd.clone()
                                        } else {
                                            format!("{} · {}", run_cmd, desc)
                                        };
                                        div()
                                            .cursor_pointer()
                                            .px(px(8.0))
                                            .py(px(4.0))
                                            .rounded(px(6.0))
                                            .bg(rgb(pal.input_bg))
                                            .hover(|s| s.bg(rgb(pal.dialogs_hover_bg)))
                                            .text_size(px(11.0))
                                            .text_color(rgb(pal.text_link))
                                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                                this.pending_action = Some(InspectorAction::RunBotCommand(run_cmd.clone()));
                                                cx.notify();
                                            }))
                                            .child(label)
                                    })),
                            )
                        }),
                )
            })
            .child(div().flex().gap(px(6.0))
                .child(Self::action_pill(
                    IconName::Lock,
                    if is_secret_chat_active {
                        tr_str(lang, "已加密", "E2EE")
                    } else {
                        tr_str(lang, "加密聊天", "Secret")
                    },
                    pal,
                    |this| {
                        this.is_secret_chat_active = !this.is_secret_chat_active;
                    },
                    cx,
                ))
                .child(Self::action_pill(
                    IconName::Close,
                    if is_contact_blocked {
                        tr_str(lang, "取消屏蔽", "Unblock")
                    } else {
                        tr_str(lang, "屏蔽", "Block")
                    },
                    pal,
                    |this| {
                        this.is_contact_blocked = !this.is_contact_blocked;
                    },
                    cx,
                )))
            .into_any_element()
    }

    fn action_pill(icon: IconName, label: &str, pal: TdesktopThemePalette, on_tap: impl Fn(&mut ContextInspector) + 'static, cx: &mut Context<ContextInspector>) -> impl IntoElement {
        div().cursor_pointer().flex_1().py(px(8.0)).rounded(px(8.0)).bg(rgb(pal.card_bg)).hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
            .flex().flex_col().items_center().gap(px(4.0))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _e, _w, cx| { on_tap(this); cx.notify(); }))
            .child(tg_icon(icon, 16.0, pal.accent))
            .child(div().text_size(px(11.0)).text_color(rgb(pal.text_link)).child(label.to_string()))
    }
}
