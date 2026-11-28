//! ContextInspectorDrawer 320px Right Drawer (`SPEC-02-GPUI-COMPONENT-HIERARCHY` § 2.5).

use std::collections::HashSet;
use std::time::Duration;

use crate::components::grok_bot_avatar::{
    render_grok_bot_avatar, render_grok_persona_avatar, trigger_grok_avatar_spin,
    GrokBotAvatarProps, GrokPersonaState,
};
use crate::components::ui::{TextInputController, delete_prev_word, pop_last_char};
use crate::i18n::tr_str;
use crate::theme::assets::{IconName, tg_icon};
use crate::theme::palette::TdesktopThemePalette;
use crate::views::drawer::bot_computer_inspector::{
    render_bot_agent_and_computer_tab, render_bot_avatar_editor_popover,
    render_bot_computers_subpage, render_bot_config_editor, render_bot_profile_main,
    render_bot_settings_tab, render_openbot_bot_settings_root, render_openbot_bot_subpage,
};
use crate::views::drawer::inspector_panels::{
    render_contact_profile_panel, render_members_panel_with_picker,
    render_openbot_channel_settings_root, render_openbot_channel_subpage,
    render_openbot_contact_profile_root, render_settings_and_stats_panel,
    render_shared_media_panel, segmented_active_bg, segmented_pill_shadow,
};
use crate::views::drawer::inspector_tabs::*;
use crate::views::drawer::statistics::info_statistics_inner_widget::ChartDataPoint;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::miniapp_contracts::CLASSIC_INITIALS_AVATAR_SHAPE;
use gpui_telegram_proto::{BotComputerProfileDto, ComputerNodeDto, ComputerRunPolicy};
use openbot_contracts::avatar::BloubExpression;

pub const DRAWER_WIDTH: f32 = 320.0;
pub const CONTEXT_INSPECTOR_WIDTH: f32 = DRAWER_WIDTH;
pub const HEADER_HEIGHT: f32 = 54.0;
pub const CONTEXT_INSPECTOR_HEADER_HEIGHT: f32 = HEADER_HEIGHT;
pub const AVATAR_SIZE: f32 = 80.0;
pub const CONTEXT_INSPECTOR_AVATAR_SIZE: f32 = AVATAR_SIZE;
pub const OPENBOT_SIDE_PANEL_DEFAULT_WIDTH: f32 = 360.0;
pub const INSPECTOR_COPIED_FEEDBACK_DURATION_MS: u64 = 2_000;

/// A contact or Fabushi AI Bot eligible to be added to the active group chat from the Inspector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddMemberCandidate {
    pub user_id: u64,
    pub display_name: String,
    pub username: Option<String>,
    pub initials: String,
    pub avatar_color: u32,
    pub is_bot: bool,
    pub bot_hue: Option<String>,
}

/// Builds a deduplicated list of contacts and Fabushi AI bots not yet present in the active group.
pub fn build_add_member_candidates(
    existing_member_ids: &[u64],
    contacts: &[gpui_telegram_proto::contracts::auth::ContactDto],
    bots: &[gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto],
) -> Vec<AddMemberCandidate> {
    let existing: HashSet<u64> = existing_member_ids.iter().copied().collect();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut out: Vec<AddMemberCandidate> = Vec::new();

    for fb in bots {
        let uid = fb.bot_user_id.max(0) as u64;
        if uid == 0 || existing.contains(&uid) || !seen.insert(uid) {
            continue;
        }
        let uname = fb.username.trim().trim_start_matches('@').to_string();
        let display_name = if fb.display_name.trim().is_empty() {
            if !uname.is_empty() {
                uname.clone()
            } else {
                fb.agent_id.clone()
            }
        } else {
            fb.display_name.trim().to_string()
        };
        let initials = {
            let d = crate::views::app_shell::shell_truth::derive_initials(&display_name);
            if d.is_empty() { "B".to_string() } else { d }
        };
        let hue = if TdesktopThemePalette::is_tinted_bot_hue(&fb.avatar_color) {
            fb.avatar_color.clone()
        } else {
            TdesktopThemePalette::bot_hue_name_for_id(uid).to_string()
        };
        let (fill, _, _) = TdesktopThemePalette::bot_hue_colors(&hue, true);
        out.push(AddMemberCandidate {
            user_id: uid,
            display_name,
            username: (!uname.is_empty()).then_some(uname),
            initials,
            avatar_color: fill,
            is_bot: true,
            bot_hue: Some(hue),
        });
    }

    for c in contacts {
        if c.user_id == 0 || existing.contains(&c.user_id) || !seen.insert(c.user_id) {
            continue;
        }
        let mut full = c.first_name.trim().to_string();
        if let Some(last) = c.last_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if !full.is_empty() {
                full.push(' ');
            }
            full.push_str(last);
        }
        let uname = c
            .username
            .as_deref()
            .map(|u| u.trim().trim_start_matches('@').to_string())
            .filter(|u| !u.is_empty());
        let display_name = if full.is_empty() {
            uname.clone().unwrap_or_else(|| format!("User {}", c.user_id))
        } else {
            full
        };
        let initials = {
            let d = crate::views::app_shell::shell_truth::derive_initials(&display_name);
            if d.is_empty() { "U".to_string() } else { d }
        };
        let hue = TdesktopThemePalette::bot_hue_name_for_id(c.user_id);
        let (fill, _, _) = TdesktopThemePalette::bot_hue_colors(hue, true);
        out.push(AddMemberCandidate {
            user_id: c.user_id,
            display_name,
            username: uname,
            initials,
            avatar_color: fill,
            is_bot: c.is_bot,
            bot_hue: c.is_bot.then(|| hue.to_string()),
        });
    }

    out
}

/// Filters `AddMemberCandidate` rows case-insensitively by display name, `@username`, ID, or bot tag.
pub fn filter_add_member_candidates<'a>(
    candidates: &'a [AddMemberCandidate],
    query: &str,
) -> Vec<&'a AddMemberCandidate> {
    let q = query.trim().trim_start_matches('@').to_ascii_lowercase();
    if q.is_empty() {
        return candidates.iter().collect();
    }
    candidates
        .iter()
        .filter(|c| {
            c.display_name.to_ascii_lowercase().contains(&q)
                || c.username
                    .as_deref()
                    .is_some_and(|u| u.to_ascii_lowercase().contains(&q))
                || c.user_id.to_string().contains(&q)
                || (c.is_bot && (q == "bot" || q == "ai"))
        })
        .collect()
}

pub struct ContextInspector {
    pub is_open: bool,
    pub active_tab: InspectorTab,
    pub subpage: InspectorSubpage,
    pub profile: ProfileTabModel,
    pub members: MembersTabModel,
    pub shared_media: SharedMediaTabModel,
    pub settings: SettingsTabModel,
    pub media_items: Vec<SharedMediaItem>,
    pub channel_stats: Vec<ChartDataPoint>,
    pub is_secret_chat_active: bool,
    pub is_contact_blocked: bool,
    pub pending_media_open: Option<(String, bool)>,
    pub pending_action: Option<InspectorAction>,
    pub pending_leave_chat: bool,
    pub copied_feedback: Option<String>,
    pub copied_feedback_gen: u64,
    pub channel_boost_level: u32,
    pub channel_boosts_current: u32,
    pub channel_boosts_target: u32,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub computers: Vec<ComputerNodeDto>,
    pub bot_computer_profile: Option<BotComputerProfileDto>,
    pub ai_models: InspectorAiModelCatalog,
    pub bot_config_focus: BotConfigFocusField,
    pub bot_draft_persona: String,
    pub bot_draft_model: String,
    pub bot_draft_approval_mode: ComputerRunPolicy,
    pub bot_draft_allow_cross_device: bool,
    pub bot_draft_allow_cloud_self_config: bool,
    pub persona_input: Option<Entity<TextInputController>>,
    pub model_input: Option<Entity<TextInputController>>,
    pub active_fabushi_bot: Option<gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto>,
    pub coding_draft_bot_id: i64,
    pub coding_draft_workspace: String,
    pub coding_draft_use_worktree: bool,
    pub coding_draft_permission_mode: gpui_telegram_proto::CodingPermissionMode,
    pub coding_draft_allow_group_members: bool,
    pub coding_draft_model_override: String,
    pub coding_workspace_input: Option<Entity<TextInputController>>,
    pub coding_model_input: Option<Entity<TextInputController>>,
    pub collapsed_sections: HashSet<&'static str>,
    pub avatar_editor_open: bool,
    pub add_member_picker_open: bool,
    pub add_member_search_focused: bool,
    pub add_member_query: String,
    pub add_member_candidates: Vec<AddMemberCandidate>,
    pub add_member_search_input: Option<Entity<TextInputController>>,
    pub pending_add_member_user_id: Option<u64>,
    pub bot_fusion: BotFusionSnapshot,
    pub memory_draft: String,
    pub memory_input: Option<Entity<TextInputController>>,
    // M4 OpenBot Unified SidePanel hosting & embedded Bot/Channel/Contact settings state
    pub embedded_in_side_panel: bool,
    pub openbot_subpage: Option<OpenBotSettingsSubpage>,
    pub pending_openbot_action: Option<OpenBotInspectorAction>,
    pub pending_confirm_guard: Option<OpenBotConfirmGuard>,
    pub openbot_text_focus: Option<OpenBotInspectorTextField>,
    pub openbot_text_input: Option<Entity<TextInputController>>,
    pub bot_draft_name: String,
    pub bot_draft_title: String,
    pub bot_draft_description: String,
    pub bot_autosave: BotIdentityAutosaveState,
    /// Pending `InspectorAction::PatchFabushiBot` flushed right before a peer switch (`PAIR-22`).
    /// Intentionally preserved across `clear_peer_facts()` so `AppShell` can dispatch it reliably.
    pub pending_flushed_bot_patch: Option<InspectorAction>,
    pub bot_avatar_style_flat: bool,
    pub bot_avatar_expression: BloubExpression,
    pub bot_avatar_custom_photo: Option<String>,
    pub bot_chat_bubble_color: Option<String>,
    pub bot_backup_models_auto: bool,
    pub bot_backup_models: Vec<String>,
    pub bot_backup_model_draft: String,
    pub bot_access_level: String,
    pub bot_computer_use: bool,
    pub bot_allow_automation: bool,
    pub bot_additional_folders: Vec<String>,
    pub bot_excluded_folders: Vec<String>,
    pub bot_folder_draft: String,
    pub bot_folder_draft_excluded: bool,
    pub bot_busy_mode: String,
    pub bot_is_busy: bool,
    pub bot_busy_toast_message: Option<String>,
    pub bot_pending_access_level: Option<String>,
    pub bot_data_tables: Vec<(String, usize)>,
    pub published_creations: Vec<BotPublishedCreationSummary>,
    // M4 OpenBot ChannelSettingsPane state
    pub channel_picture_editor_open: bool,
    pub channel_icon_name: Option<String>,
    pub channel_icon_hue: u16,
    pub channel_custom_image: Option<String>,
    pub channel_draft_name: String,
    pub channel_draft_title: String,
    pub channel_draft_instructions: String,
    pub channel_browser_access: ChannelBrowserAccess,
    pub channel_context_isolated: bool,
    pub channel_archived: bool,
    pub channel_provider_supports_compact: bool,
    pub channel_lead_user_id: Option<u64>,
    pub channel_memories: Vec<String>,
    pub channel_memory_draft: String,
    pub channel_folders: Vec<String>,
    pub channel_folder_draft: String,
    pub channel_validation_error: Option<String>,
}

impl ContextInspector {
    pub fn new() -> Self {
        Self {
            is_open: false,
            active_tab: InspectorTab::Profile,
            subpage: InspectorSubpage::Main,
            profile: Self::empty_profile(),
            members: Self::empty_members(),
            shared_media: Self::empty_media(),
            settings: Self::empty_settings(),
            media_items: Vec::new(),
            channel_stats: Vec::new(),
            is_secret_chat_active: false,
            is_contact_blocked: false,
            pending_media_open: None,
            pending_action: None,
            pending_leave_chat: false,
            copied_feedback: None,
            copied_feedback_gen: 0,
            channel_boost_level: 0,
            channel_boosts_current: 0,
            channel_boosts_target: 30,
            palette: TdesktopThemePalette::default(),
            language_id: "en".to_string(),
            computers: Vec::new(),
            bot_computer_profile: None,
            ai_models: InspectorAiModelCatalog::default(),
            bot_config_focus: BotConfigFocusField::None,
            bot_draft_persona: String::new(),
            bot_draft_model: String::new(),
            bot_draft_approval_mode: ComputerRunPolicy::AskEveryTime,
            bot_draft_allow_cross_device: true,
            bot_draft_allow_cloud_self_config: true,
            persona_input: None,
            model_input: None,
            active_fabushi_bot: None,
            coding_draft_bot_id: 0,
            coding_draft_workspace: String::new(),
            coding_draft_use_worktree: false,
            coding_draft_permission_mode: gpui_telegram_proto::CodingPermissionMode::Ask,
            coding_draft_allow_group_members: false,
            coding_draft_model_override: String::new(),
            coding_workspace_input: None,
            coding_model_input: None,
            collapsed_sections: HashSet::new(),
            avatar_editor_open: false,
            add_member_picker_open: false,
            add_member_search_focused: false,
            add_member_query: String::new(),
            add_member_candidates: Vec::new(),
            add_member_search_input: None,
            pending_add_member_user_id: None,
            bot_fusion: BotFusionSnapshot::default(),
            memory_draft: String::new(),
            memory_input: None,
            embedded_in_side_panel: false,
            openbot_subpage: None,
            pending_openbot_action: None,
            pending_confirm_guard: None,
            openbot_text_focus: None,
            openbot_text_input: None,
            bot_draft_name: String::new(),
            bot_draft_title: String::new(),
            bot_draft_description: String::new(),
            bot_autosave: BotIdentityAutosaveState::default(),
            pending_flushed_bot_patch: None,
            bot_avatar_style_flat: false,
            bot_avatar_expression: BloubExpression::Neutre,
            bot_avatar_custom_photo: None,
            bot_chat_bubble_color: None,
            bot_backup_models_auto: true,
            bot_backup_models: Vec::new(),
            bot_backup_model_draft: String::new(),
            bot_access_level: "ask".to_string(),
            bot_computer_use: true,
            bot_allow_automation: true,
            bot_additional_folders: Vec::new(),
            bot_excluded_folders: Vec::new(),
            bot_folder_draft: String::new(),
            bot_folder_draft_excluded: false,
            bot_busy_mode: "steer".to_string(),
            bot_is_busy: false,
            bot_busy_toast_message: None,
            bot_pending_access_level: None,
            bot_data_tables: Vec::new(),
            published_creations: Vec::new(),
            channel_picture_editor_open: false,
            channel_icon_name: None,
            channel_icon_hue: 215,
            channel_custom_image: None,
            channel_draft_name: String::new(),
            channel_draft_title: String::new(),
            channel_draft_instructions: String::new(),
            channel_browser_access: ChannelBrowserAccess::Everyone,
            channel_context_isolated: false,
            channel_archived: false,
            channel_provider_supports_compact: true,
            channel_lead_user_id: None,
            channel_memories: Vec::new(),
            channel_memory_draft: String::new(),
            channel_folders: Vec::new(),
            channel_folder_draft: String::new(),
            channel_validation_error: None,
        }
    }

    pub fn new_demo() -> Self {
        let mut s = Self::new();
        s.is_open = true;
        s.channel_boost_level = 8;
        s.profile = ProfileTabModel {
            name: "Alice Smith".into(),
            username: "@alice_smith".into(),
            phone: "+1 (555) 019-2834".into(),
            bio: "Rust & Systems engineer. Building GPUI Telegram 1:1 desktop client.".into(),
            status: "online".into(),
            avatar_initials: "AS".into(),
            avatar_color: 0x2AABEE,
            avatar_path: None,
            facts: Vec::new(),
            is_notifications_enabled: true,
            is_bot: false,
            bot_id: 0,
            bot_persona_prompt: String::new(),
            bot_commands: Vec::new(),
            miniapp_title: None,
            miniapp_url: None,
            in_attach_menu: false,
            bot_mau_count: 0,
            bot_has_main_app: false,
            bot_privacy_policy_url: String::new(),
            bot_has_settings: false,
            bot_can_join_groups: false,
            is_viewer_owner: false,
            is_verified: false,
            is_blocked: false,
        };
        s.members.members = vec![
            MemberItem { id: 1, name: "Alice Smith".into(), initials: "AS".into(), color: 0x2AABEE, role: "Owner".into(), is_online: true },
            MemberItem { id: 2, name: "Bob Builder".into(), initials: "BB".into(), color: 0x9B59B6, role: "Admin".into(), is_online: false },
        ];
        s.settings.e2ee_fingerprint = "🔐 🌟 🚀 ⚡".into();
        s.channel_boosts_current = 24;
        s
    }

    fn empty_profile() -> ProfileTabModel {
        ProfileTabModel {
            name: String::new(), username: String::new(), phone: String::new(), bio: String::new(),
            status: String::new(), avatar_initials: String::new(), avatar_color: 0, avatar_path: None,
            facts: Vec::new(), is_notifications_enabled: true,
            is_bot: false, bot_id: 0, bot_persona_prompt: String::new(), bot_commands: Vec::new(),
            miniapp_title: None, miniapp_url: None, in_attach_menu: false,
            bot_mau_count: 0, bot_has_main_app: false, bot_privacy_policy_url: String::new(),
            bot_has_settings: false, bot_can_join_groups: false, is_viewer_owner: false,
            is_verified: false, is_blocked: false,
        }
    }
    fn empty_members() -> MembersTabModel {
        MembersTabModel { members: Vec::new(), search_query: String::new(), invite_link: String::new(), is_group_or_channel: false }
    }
    fn empty_media() -> SharedMediaTabModel {
        SharedMediaTabModel {
            active_subtab: SharedMediaSubTab::Media, media_count: 0, media_items: Vec::new(),
            files: Vec::new(), links: Vec::new(), voice_notes: Vec::new(), music_items: Vec::new(),
        }
    }
    fn empty_settings() -> SettingsTabModel {
        SettingsTabModel { e2ee_fingerprint: String::new(), auto_delete_period: "Off".into(), notification_sound: "Default (Aurora)".into() }
    }

    pub fn clear_peer_facts(&mut self) {
        self.active_tab = InspectorTab::Profile;
        self.profile = Self::empty_profile();
        self.members = Self::empty_members();
        self.shared_media = Self::empty_media();
        self.settings = Self::empty_settings();
        self.media_items.clear();
        self.channel_stats.clear();
        self.channel_boost_level = 0;
        self.channel_boosts_current = 0;
        self.channel_boosts_target = 30;
        self.is_secret_chat_active = false;
        self.is_contact_blocked = false;
        self.pending_media_open = None;
        self.pending_action = None;
        self.pending_leave_chat = false;
        self.clear_copied_feedback();
        self.subpage = InspectorSubpage::Main;
        self.bot_computer_profile = None;
        self.bot_config_focus = BotConfigFocusField::None;
        self.bot_draft_persona.clear();
        self.bot_draft_model.clear();
        self.bot_draft_approval_mode = ComputerRunPolicy::AskEveryTime;
        self.bot_draft_allow_cross_device = true;
        self.bot_draft_allow_cloud_self_config = true;
        self.active_fabushi_bot = None;
        self.coding_draft_bot_id = 0;
        self.coding_draft_workspace.clear();
        self.coding_draft_use_worktree = false;
        self.coding_draft_permission_mode = gpui_telegram_proto::CodingPermissionMode::Ask;
        self.coding_draft_allow_group_members = false;
        self.coding_draft_model_override.clear();
        self.collapsed_sections.clear();
        self.avatar_editor_open = false;
        self.add_member_picker_open = false;
        self.add_member_search_focused = false;
        self.add_member_query.clear();
        self.add_member_candidates.clear();
        self.pending_add_member_user_id = None;
        self.openbot_subpage = None;
        self.pending_openbot_action = None;
        self.pending_confirm_guard = None;
        self.openbot_text_focus = None;
        self.bot_draft_name.clear();
        self.bot_draft_title.clear();
        self.bot_draft_description.clear();
        self.bot_avatar_style_flat = false;
        self.bot_avatar_expression = BloubExpression::Neutre;
        self.bot_avatar_custom_photo = None;
        self.bot_chat_bubble_color = None;
        self.bot_backup_models_auto = true;
        self.bot_backup_models.clear();
        self.bot_backup_model_draft.clear();
        self.bot_access_level = "ask".to_string();
        self.bot_computer_use = true;
        self.bot_allow_automation = true;
        self.bot_additional_folders.clear();
        self.bot_excluded_folders.clear();
        self.bot_folder_draft.clear();
        self.bot_folder_draft_excluded = false;
        self.bot_busy_mode = "steer".to_string();
        self.bot_data_tables.clear();
        self.published_creations.clear();
        self.channel_picture_editor_open = false;
        self.channel_icon_name = None;
        self.channel_icon_hue = 215;
        self.channel_custom_image = None;
        self.channel_draft_name.clear();
        self.channel_draft_title.clear();
        self.channel_draft_instructions.clear();
        self.channel_browser_access = ChannelBrowserAccess::Everyone;
        self.channel_context_isolated = false;
        self.channel_archived = false;
        self.channel_lead_user_id = None;
        self.channel_memories.clear();
        self.channel_memory_draft.clear();
        self.channel_folders.clear();
        self.channel_folder_draft.clear();
        self.channel_validation_error = None;
    }

    pub fn open_add_member_picker(&mut self, candidates: Vec<AddMemberCandidate>) {
        self.add_member_candidates = candidates;
        self.add_member_query.clear();
        self.add_member_picker_open = true;
        self.add_member_search_focused = true;
        self.active_tab = InspectorTab::Members;
    }

    pub fn close_add_member_picker(&mut self) {
        self.add_member_picker_open = false;
        self.add_member_search_focused = false;
        self.add_member_query.clear();
    }

    pub fn remove_add_member_candidate(&mut self, user_id: u64) {
        self.add_member_candidates.retain(|c| c.user_id != user_id);
    }

    /// Queues an add-member request for the active group and notifies `AppShell` via `InspectorAction::OpenAddMember`.
    pub fn request_add_member_to_active_group(&mut self, user_id: u64) {
        self.pending_add_member_user_id = Some(user_id);
        self.pending_action = Some(InspectorAction::OpenAddMember);
    }

    pub fn take_pending_add_member_user_id(&mut self) -> Option<u64> {
        self.pending_add_member_user_id.take()
    }

    pub fn is_section_collapsed(&self, key: &'static str) -> bool {
        self.collapsed_sections.contains(key)
    }

    pub fn toggle_section(&mut self, key: &'static str) {
        if !self.collapsed_sections.remove(key) {
            self.collapsed_sections.insert(key);
        }
    }

    pub fn clear_peer(&mut self) { self.is_open = false; self.active_tab = InspectorTab::Profile; self.clear_peer_facts(); }
    pub fn configure_for_cloud_workspace(&mut self, _server_url: &str) { self.clear_peer(); self.clear_ai_models(); }
    pub fn sync_ai_models(&mut self, catalog: InspectorAiModelCatalog) { self.ai_models = catalog; }
    pub fn clear_ai_models(&mut self) { self.ai_models = InspectorAiModelCatalog::default(); }
    pub fn set_palette(&mut self, palette: TdesktopThemePalette) { self.palette = palette; }
    pub fn set_language(&mut self, lang: impl Into<String>) { self.language_id = lang.into(); }
    pub fn take_action(&mut self) -> Option<InspectorAction> { self.pending_action.take() }
    pub fn take_openbot_action(&mut self) -> Option<OpenBotInspectorAction> {
        self.pending_openbot_action.take()
    }
    pub fn take_pending_leave_chat(&mut self) -> bool { std::mem::take(&mut self.pending_leave_chat) }

    pub fn sync_published_creations(&mut self, creations: Vec<BotPublishedCreationSummary>) {
        self.published_creations = creations;
    }

    /// Closes transient popovers/confirm guards owned by the embedded OpenBot inspector.
    pub fn close_openbot_popovers(&mut self) -> bool {
        let mut closed = false;
        if self.avatar_editor_open {
            self.avatar_editor_open = false;
            closed = true;
        }
        if self.channel_picture_editor_open {
            self.channel_picture_editor_open = false;
            closed = true;
        }
        if self.pending_confirm_guard.is_some() {
            self.pending_confirm_guard = None;
            closed = true;
        }
        if self.add_member_picker_open {
            self.close_add_member_picker();
            closed = true;
        }
        closed
    }

    pub fn navigate_back_subpage(&mut self) -> bool {
        if self.openbot_subpage.is_some() {
            self.openbot_subpage = None;
            self.pending_confirm_guard = None;
            self.openbot_text_focus = None;
            self.bot_config_focus = BotConfigFocusField::None;
            return true;
        }
        if self.subpage != InspectorSubpage::Main {
            self.subpage = InspectorSubpage::Main;
            self.bot_config_focus = BotConfigFocusField::None;
            self.pending_action = Some(InspectorAction::CloseBotComputersSubpage);
            return true;
        }
        false
    }

    pub fn focus_openbot_text_field(
        &mut self,
        field: OpenBotInspectorTextField,
        cx: &mut Context<Self>,
    ) {
        self.ensure_bot_inputs(cx);
        self.bot_config_focus = BotConfigFocusField::None;
        self.add_member_search_focused = false;
        self.openbot_text_focus = Some(field);
        if let Some(ref ctrl) = self.openbot_text_input {
            ctrl.update(cx, |c, _| c.clear_preedit());
        }
        cx.notify();
    }

    pub fn openbot_input_preedit(&self, cx: &App) -> Option<String> {
        self.openbot_text_input
            .as_ref()
            .and_then(|c| c.read(cx).ime_preedit.clone())
    }

    /// Commits edited Bot identity fields (`name`, `title`, `description`) via `PatchFabushiBot`.
    pub fn commit_bot_identity_draft(&mut self) -> bool {
        let name = self.bot_draft_name.trim().to_string();
        let title = self.bot_draft_title.trim().to_string();
        let desc = self.bot_draft_description.trim().to_string();
        let editing_name = self.openbot_text_focus == Some(OpenBotInspectorTextField::BotName);
        self.openbot_text_focus = None;
        if editing_name || !name.is_empty() {
            if let Err(err) = validate_bot_profile_name(&name) {
                self.bot_autosave.save_error = Some(err.to_string());
                return false;
            }
        }
        self.bot_autosave.save_error = None;
        if let Some(ref fab) = self.active_fabushi_bot {
            let bot_id = fab.bot_user_id.max(0) as u64;
            if bot_id > 0 {
                if !name.is_empty() {
                    self.profile.name = name.clone();
                }
                self.profile.bio = desc.clone();
                self.pending_action = Some(InspectorAction::PatchFabushiBot {
                    bot_id,
                    req: gpui_telegram_proto::miniapp_contracts::PatchFabushiBotRequest {
                        display_name: (!name.is_empty()).then_some(name),
                        title: Some(title),
                        description: Some(desc),
                        ..Default::default()
                    },
                });
                return true;
            }
        }
        if !name.is_empty() || !desc.is_empty() {
            self.pending_action = Some(InspectorAction::OpenEditPeer);
            return true;
        }
        false
    }

    /// Returns `true` while the inspected bot is actively running a turn (`UI-F16-03`).
    pub fn is_bot_busy(&self) -> bool {
        self.bot_is_busy
            || self.active_fabushi_bot.as_ref().is_some_and(|b| {
                matches!(
                    b.execution_state.trim().to_ascii_lowercase().as_str(),
                    "thinking" | "working" | "running" | "streaming"
                )
            })
    }

    /// Attempts to start a new chat session from Bot Settings; blocks with a busy toast while busy (`UI-F16-03`).
    pub fn try_start_new_chat_from_settings(&mut self) -> NewChatFromSettingsOutcome {
        if self.is_bot_busy() {
            let msg = "Agent is currently busy; wait for the active turn to finish before starting a new chat.";
            self.bot_busy_toast_message = Some(msg.to_string());
            NewChatFromSettingsOutcome {
                new_chat_busy_toast: true,
                chat_reset: false,
                toast_message: Some(msg),
            }
        } else {
            self.bot_busy_toast_message = None;
            self.pending_confirm_guard = Some(OpenBotConfirmGuard::BotNewSession);
            NewChatFromSettingsOutcome {
                new_chat_busy_toast: false,
                chat_reset: true,
                toast_message: None,
            }
        }
    }

    /// Attempts to arm Delete History confirmation from Bot Settings; disabled while the agent is busy (`UI-F16-03`).
    pub fn try_delete_bot_history_from_settings(&mut self) -> bool {
        if self.is_bot_busy() {
            self.bot_busy_toast_message =
                Some("Cannot delete chat history while the agent is busy.".to_string());
            false
        } else {
            self.bot_busy_toast_message = None;
            self.pending_confirm_guard = Some(OpenBotConfirmGuard::BotDeleteHistory);
            true
        }
    }

    /// Returns `true` iff transitioning from `current` to `next` widens permissions (`workspace < ask < full`).
    pub fn requires_permission_confirm(current: &str, next: &str) -> bool {
        fn rank(level: &str) -> u8 {
            match level.trim().to_ascii_lowercase().as_str() {
                "workspace" | "strict" => 0,
                "ask" => 1,
                "full" | "autonomous" => 2,
                _ => 1,
            }
        }
        rank(next) > rank(current)
    }

    /// Requests changing the bot's access level; widening requires explicit confirmation while narrowing/same applies immediately.
    /// Returns `true` if a confirmation dialog is required before applying.
    pub fn request_bot_access_level_change(&mut self, next: &str) -> bool {
        let normalized = match next.trim().to_ascii_lowercase().as_str() {
            "full" | "autonomous" => "full",
            "workspace" | "strict" => "workspace",
            _ => "ask",
        };
        if Self::requires_permission_confirm(&self.bot_access_level, normalized) {
            self.bot_pending_access_level = Some(normalized.to_string());
            self.pending_confirm_guard = Some(OpenBotConfirmGuard::WidenPermissionsFull);
            true
        } else {
            self.bot_access_level = normalized.to_string();
            self.bot_pending_access_level = None;
            self.pending_confirm_guard = None;
            self.pending_openbot_action = Some(OpenBotInspectorAction::SetBotPermissions {
                access: normalized.to_string(),
                computer_use: self.bot_computer_use,
                allow_automation: self.bot_allow_automation,
            });
            false
        }
    }

    /// Commits a pending permission-widening change after user confirmation.
    pub fn confirm_pending_access_level_change(&mut self) {
        let target = self
            .bot_pending_access_level
            .take()
            .unwrap_or_else(|| "full".to_string());
        self.bot_access_level = target.clone();
        self.pending_confirm_guard = None;
        self.pending_openbot_action = Some(OpenBotInspectorAction::SetBotPermissions {
            access: target,
            computer_use: self.bot_computer_use,
            allow_automation: self.bot_allow_automation,
        });
    }

    /// Commits edited Group/Channel Name, Title & Instructions via `OpenBotInspectorAction::UpdateChatInfo`.
    /// Reverts blank names on blur to `self.profile.name` and enforces UTF-16 limits
    /// (`<= CHANNEL_NAME_MAX_LEN = 80`, `<= CHANNEL_INSTRUCTIONS_MAX_LEN = 2000`, `UI-F17-01`).
    pub fn commit_channel_identity_draft(&mut self) -> bool {
        if self.channel_archived {
            return false;
        }
        let name = self.channel_draft_name.trim().to_string();
        let title = self.channel_draft_title.trim().to_string();
        let instructions = self.channel_draft_instructions.trim().to_string();
        self.openbot_text_focus = None;
        if name.is_empty() {
            self.channel_draft_name = self.profile.name.clone();
            let err = "Channel name cannot be blank".to_string();
            self.channel_validation_error = Some(err.clone());
            self.bot_autosave.save_error = Some(err);
            return false;
        }
        let name_u16 = name.encode_utf16().count();
        if name_u16 > crate::views::openbot::channel_editor::CHANNEL_NAME_MAX_LEN {
            let err = format!(
                "Channel name exceeds maximum limit of {} characters ({name_u16}).",
                crate::views::openbot::channel_editor::CHANNEL_NAME_MAX_LEN
            );
            self.channel_validation_error = Some(err.clone());
            self.bot_autosave.save_error = Some(err);
            return false;
        }
        let instr_u16 = instructions.encode_utf16().count();
        if instr_u16 > crate::views::openbot::channel_editor::CHANNEL_INSTRUCTIONS_MAX_LEN {
            let err = format!(
                "Channel instructions exceed maximum limit of {} characters ({instr_u16}).",
                crate::views::openbot::channel_editor::CHANNEL_INSTRUCTIONS_MAX_LEN
            );
            self.channel_validation_error = Some(err.clone());
            self.bot_autosave.save_error = Some(err);
            return false;
        }
        self.channel_validation_error = None;
        self.bot_autosave.save_error = None;
        let about = if !title.is_empty() && !instructions.is_empty() {
            format!("{title}\n{instructions}")
        } else if !instructions.is_empty() {
            instructions
        } else {
            title
        };
        self.profile.name = name.clone();
        self.profile.bio = about.clone();
        self.channel_draft_name = name.clone();
        self.pending_openbot_action = Some(OpenBotInspectorAction::UpdateChatInfo {
            title: name,
            about,
        });
        true
    }

    pub fn add_bot_backup_model_from_draft(&mut self) -> bool {
        let model = self.bot_backup_model_draft.trim().to_string();
        if model.is_empty() || self.bot_backup_models.len() >= 8 {
            return false;
        }
        if !self.bot_backup_models.iter().any(|m| m.eq_ignore_ascii_case(&model)) {
            self.bot_backup_models.push(model);
        }
        self.bot_backup_model_draft.clear();
        self.openbot_text_focus = None;
        true
    }

    pub fn add_bot_folder_from_draft(&mut self) -> bool {
        let path = self.bot_folder_draft.trim().to_string();
        if path.is_empty() {
            return false;
        }
        let target = if self.bot_folder_draft_excluded {
            &mut self.bot_excluded_folders
        } else {
            &mut self.bot_additional_folders
        };
        if target.len() >= 8 {
            return false;
        }
        if !target.iter().any(|p| p == &path) {
            target.push(path);
        }
        self.bot_folder_draft.clear();
        self.openbot_text_focus = None;
        true
    }

    pub fn add_channel_memory_from_draft(&mut self) -> bool {
        if self.channel_archived {
            return false;
        }
        let text = self.channel_memory_draft.trim().to_string();
        if text.is_empty() || self.channel_memories.len() >= 32 {
            return false;
        }
        self.channel_memories.insert(0, text);
        self.channel_memory_draft.clear();
        self.openbot_text_focus = None;
        true
    }

    pub fn add_channel_folder_from_draft(&mut self) -> bool {
        if self.channel_archived {
            return false;
        }
        let path = self.channel_folder_draft.trim().to_string();
        if path.is_empty() || self.channel_folders.len() >= 8 {
            return false;
        }
        if !self.channel_folders.iter().any(|p| p == &path) {
            self.channel_folders.push(path);
        }
        self.channel_folder_draft.clear();
        self.openbot_text_focus = None;
        true
    }

    pub fn clear_copied_feedback(&mut self) {
        self.copied_feedback_gen = self.copied_feedback_gen.wrapping_add(1);
        self.copied_feedback = None;
    }

    pub fn show_copied_feedback(&mut self, msg: impl Into<String>, cx: &mut Context<Self>) {
        self.show_copied_feedback_with_duration(
            msg,
            Duration::from_millis(INSPECTOR_COPIED_FEEDBACK_DURATION_MS),
            cx,
        );
    }

    pub fn show_copied_feedback_with_duration(
        &mut self,
        msg: impl Into<String>,
        duration: Duration,
        cx: &mut Context<Self>,
    ) {
        let generation = self.copied_feedback_gen.wrapping_add(1);
        self.copied_feedback_gen = generation;
        self.copied_feedback = Some(msg.into());
        cx.notify();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            app.background_executor().timer(duration).await;
            let _ = weak.update(app, |this, cx| {
                if this.copied_feedback_gen == generation {
                    this.copied_feedback = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    /// Flushes any dirty bot identity edits (`name` / `bio`) to the outgoing `active_agent_id`
    /// BEFORE switching peers (Bot -> Group, Bot -> Human, or Bot A -> Bot B), queuing a
    /// `PatchFabushiBot` action that survives `clear_peer_facts()` (`PAIR-22`).
    pub fn flush_dirty_bot_identity_before_peer_switch(
        &mut self,
        next_id: impl Into<String>,
        next_name: impl Into<String>,
        next_bio: impl Into<String>,
    ) -> Option<InspectorAction> {
        let prev_bot_id = self
            .active_fabushi_bot
            .as_ref()
            .map(|b| b.bot_user_id.max(0) as u64)
            .filter(|&id| id > 0)
            .or_else(|| (self.profile.bot_id > 0).then_some(self.profile.bot_id))
            .unwrap_or(0);
        let flushed = self
            .bot_autosave
            .flush_before_agent_switch(next_id, next_name, next_bio);
        if flushed && prev_bot_id > 0 {
            let act = InspectorAction::PatchFabushiBot {
                bot_id: prev_bot_id,
                req: gpui_telegram_proto::miniapp_contracts::PatchFabushiBotRequest {
                    display_name: self
                        .bot_autosave
                        .last_flushed_name
                        .clone()
                        .filter(|s| !s.trim().is_empty()),
                    description: self.bot_autosave.last_flushed_bio.clone(),
                    ..Default::default()
                },
            };
            self.pending_flushed_bot_patch = Some(act.clone());
            Some(act)
        } else {
            None
        }
    }

    /// Drains any queued `PatchFabushiBot` produced during a peer-switch flush (`PAIR-22`).
    pub fn take_pending_flushed_bot_patch(&mut self) -> Option<InspectorAction> {
        self.pending_flushed_bot_patch.take()
    }

    /// Queues `OpenBotInspectorAction::OpenPublishedCreation` for the creation at `index` (`PAIR-23`).
    pub fn open_published_creation_at(&mut self, index: usize) -> Option<OpenBotInspectorAction> {
        let item = self.published_creations.get(index)?;
        let act = OpenBotInspectorAction::OpenPublishedCreation {
            creation_id: item.creation_id.clone(),
            kind_code: item.kind_code,
        };
        self.pending_openbot_action = Some(act.clone());
        Some(act)
    }

    /// Maps the currently armed inline `OpenBotConfirmGuard` into the canonical OpenBot
    /// `ConfirmDialogState` (`460px`, cancel-first on destructive actions) (`PAIR-24`).
    pub fn active_confirm_dialog_state(
        &self,
    ) -> Option<crate::views::boxes::modal_theme::ConfirmDialogState> {
        self.pending_confirm_guard
            .as_ref()
            .map(|guard| {
                crate::views::boxes::modal_theme::ConfirmDialogState::open_dialog(
                    guard.is_destructive(),
                )
            })
    }

    pub fn set_active_fabushi_bot(&mut self, bot: Option<gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto>) {
        let editing_coding = self.subpage == InspectorSubpage::Main
            && matches!(
                self.bot_config_focus,
                BotConfigFocusField::CodingWorkspace | BotConfigFocusField::CodingModel
            );
        if let Some(ref b) = bot {
            let next_id = if !b.agent_id.trim().is_empty() {
                b.agent_id.clone()
            } else {
                format!("bot-{}", b.bot_user_id)
            };
            let next_name = if b.display_name.trim().is_empty() {
                self.profile.name.clone()
            } else {
                b.display_name.clone()
            };
            let next_bio = if b.description.trim().is_empty() {
                self.profile.bio.clone()
            } else {
                b.description.clone()
            };
            let _ = self.flush_dirty_bot_identity_before_peer_switch(
                next_id,
                next_name.clone(),
                next_bio.clone(),
            );
            self.active_fabushi_bot = bot.clone();
            if self.openbot_text_focus != Some(OpenBotInspectorTextField::BotName) {
                self.bot_draft_name = next_name;
            }
            if self.openbot_text_focus != Some(OpenBotInspectorTextField::BotTitle) {
                self.bot_draft_title = b.title.clone();
            }
            if self.openbot_text_focus != Some(OpenBotInspectorTextField::BotDescription) {
                self.bot_draft_description = next_bio;
            }
            self.bot_access_level = match b.approval_mode.trim().to_ascii_lowercase().as_str() {
                "autonomous" | "full" => "full".to_string(),
                "strict" | "workspace" => "workspace".to_string(),
                _ => "ask".to_string(),
            };
            if let Some(ref cfg) = b.coding_agent {
                if self.coding_draft_bot_id != b.bot_user_id || !editing_coding {
                    self.coding_draft_bot_id = b.bot_user_id;
                    self.coding_draft_workspace = cfg.workspace_path.clone();
                    self.coding_draft_use_worktree = cfg.use_worktree;
                    self.coding_draft_permission_mode = cfg.permission_mode;
                    self.coding_draft_allow_group_members = cfg.allow_group_members;
                    self.coding_draft_model_override =
                        cfg.model_override.clone().unwrap_or_default();
                }
                return;
            }
        } else {
            if self.bot_autosave.dirty {
                let next_name = self.profile.name.clone();
                let next_bio = self.profile.bio.clone();
                let _ = self.flush_dirty_bot_identity_before_peer_switch("", next_name, next_bio);
            }
            self.active_fabushi_bot = None;
        }
        self.coding_draft_bot_id = 0;
    }

    /// Commits the current Coding Agent inspector draft fields into `InspectorAction::PatchFabushiBot`.
    pub fn save_coding_agent_config_action(&mut self) -> bool {
        let Some(ref fab) = self.active_fabushi_bot else {
            return false;
        };
        let Some(mut next_cfg) = fab.coding_agent.clone() else {
            return false;
        };
        let target_bot_id = fab.bot_user_id.max(0) as u64;
        if target_bot_id == 0 {
            return false;
        }
        self.bot_config_focus = BotConfigFocusField::None;
        let ws = self.coding_draft_workspace.trim().to_string();
        if !ws.is_empty() {
            next_cfg.workspace_path = ws;
        }
        next_cfg.use_worktree = self.coding_draft_use_worktree;
        next_cfg.permission_mode = self.coding_draft_permission_mode;
        next_cfg.allow_group_members = self.coding_draft_allow_group_members;
        let model_trimmed = self.coding_draft_model_override.trim().to_string();
        next_cfg.model_override = (!model_trimmed.is_empty()).then_some(model_trimmed);
        let preferred_model = next_cfg.model_override.clone();
        self.pending_action = Some(InspectorAction::PatchFabushiBot {
            bot_id: target_bot_id,
            req: gpui_telegram_proto::miniapp_contracts::PatchFabushiBotRequest {
                preferred_model,
                coding_agent: Some(next_cfg),
                ..Default::default()
            },
        });
        true
    }
    pub fn set_tab(&mut self, tab: InspectorTab) { self.active_tab = tab; }
    pub fn toggle_open(&mut self) {
        if self.is_open {
            self.close();
        } else {
            self.open();
        }
    }
    pub fn open(&mut self) { self.is_open = true; }
    pub fn close(&mut self) {
        self.is_open = false;
        self.pending_leave_chat = false;
        self.clear_copied_feedback();
    }
    pub fn set_auto_delete_period(&mut self, period: impl Into<String>) { self.settings.auto_delete_period = period.into(); }

    pub fn set_peer(&mut self, name: &str, subtitle: &str, initials: &str, color: u32) {
        if name.trim().is_empty() { self.clear_peer(); return; }
        let same = !self.profile.name.is_empty() && self.profile.name == name && self.profile.avatar_initials == initials && self.profile.avatar_color == color;
        if !same {
            self.bot_autosave.flush_before_agent_switch(
                format!("peer:{name}"),
                name.to_string(),
                subtitle.to_string(),
            );
            self.clear_peer_facts();
        }
        self.profile.name = name.to_string();
        self.profile.bio = subtitle.to_string();
        self.profile.avatar_initials = initials.to_string();
        self.profile.avatar_color = color;
        if self.openbot_text_focus != Some(OpenBotInspectorTextField::ChannelName)
            && self.channel_draft_name.is_empty()
        {
            self.channel_draft_name = name.to_string();
        }
        if self.openbot_text_focus != Some(OpenBotInspectorTextField::ChannelInstructions)
            && self.channel_draft_instructions.is_empty()
        {
            self.channel_draft_instructions = subtitle.to_string();
        }
        if self.openbot_text_focus != Some(OpenBotInspectorTextField::BotName)
            && self.bot_draft_name.is_empty()
        {
            self.bot_draft_name = name.to_string();
        }
        if self.openbot_text_focus != Some(OpenBotInspectorTextField::BotDescription)
            && self.bot_draft_description.is_empty()
        {
            self.bot_draft_description = subtitle.to_string();
        }
    }

    pub fn sync_profile_details(&mut self, username: String, phone: String, status: String, avatar_path: Option<String>, facts: Vec<(String, String)>) {
        self.profile.username = username; self.profile.phone = phone; self.profile.status = status;
        self.profile.avatar_path = avatar_path; self.profile.facts = facts;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sync_bot_details(
        &mut self,
        is_bot: bool,
        bot_id: u64,
        persona_prompt: String,
        commands: Vec<(String, String)>,
        miniapp_title: Option<String>,
        miniapp_url: Option<String>,
        in_attach_menu: bool,
    ) {
        self.profile.is_bot = is_bot;
        self.profile.bot_id = bot_id;
        self.profile.bot_persona_prompt = persona_prompt;
        self.profile.bot_commands = commands;
        self.profile.miniapp_title = miniapp_title;
        self.profile.miniapp_url = miniapp_url;
        self.profile.in_attach_menu = in_attach_menu;
        if !is_bot {
            self.profile.bot_mau_count = 0;
            self.profile.bot_has_main_app = false;
            self.profile.bot_privacy_policy_url.clear();
            self.profile.bot_has_settings = false;
            self.profile.bot_can_join_groups = false;
            self.profile.is_viewer_owner = false;
            self.profile.is_verified = false;
            self.profile.is_blocked = false;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn sync_bot_profile_extra(
        &mut self,
        mau_count: i64,
        has_main_app: bool,
        privacy_policy_url: String,
        has_settings: bool,
        can_join_groups: bool,
        is_viewer_owner: bool,
        is_verified: bool,
        is_blocked: bool,
    ) {
        self.profile.bot_mau_count = mau_count;
        self.profile.bot_has_main_app = has_main_app;
        self.profile.bot_privacy_policy_url = privacy_policy_url;
        self.profile.bot_has_settings = has_settings;
        self.profile.bot_can_join_groups = can_join_groups;
        self.profile.is_viewer_owner = is_viewer_owner;
        self.profile.is_verified = is_verified;
        self.profile.is_blocked = is_blocked;
        self.is_contact_blocked = is_blocked;
    }

    pub fn sync_shared_content(&mut self, media: Vec<SharedMediaItem>, files: Vec<SharedFileItem>, links: Vec<SharedLinkItem>, voice_notes: Vec<SharedVoiceItem>, music_items: Vec<SharedFileItem>) {
        self.shared_media.media_count = media.len() as u32;
        self.media_items = media.clone();
        self.shared_media.media_items = media;
        self.shared_media.files = files; self.shared_media.links = links;
        self.shared_media.voice_notes = voice_notes; self.shared_media.music_items = music_items;
    }

    pub fn sync_members(&mut self, members: Vec<MemberItem>, invite_link: String, is_group_or_channel: bool) {
        if self.channel_lead_user_id.is_none() {
            self.channel_lead_user_id = members
                .iter()
                .find(|m| {
                    m.role.eq_ignore_ascii_case("owner")
                        || m.role.eq_ignore_ascii_case("admin")
                        || m.role.eq_ignore_ascii_case("lead")
                        || m.role == "群主"
                        || m.role == "主理人"
                })
                .or_else(|| members.first())
                .map(|m| m.id);
        }
        self.members.members = members;
        self.members.invite_link = invite_link;
        self.members.is_group_or_channel = is_group_or_channel;
    }

    pub fn sync_channel_statistics(&mut self, boost_level: u32, current: u32, target: u32, bars: Vec<ChartDataPoint>) {
        self.channel_boost_level = boost_level; self.channel_boosts_current = current; self.channel_boosts_target = target; self.channel_stats = bars;
    }

    pub fn ensure_bot_inputs(&mut self, cx: &mut Context<Self>) {
        if self.persona_input.is_none() {
            self.persona_input = Some(cx.new(|_| TextInputController::new()));
        }
        if self.model_input.is_none() {
            self.model_input = Some(cx.new(|_| TextInputController::new()));
        }
        if self.coding_workspace_input.is_none() {
            self.coding_workspace_input = Some(cx.new(|_| TextInputController::new()));
        }
        if self.coding_model_input.is_none() {
            self.coding_model_input = Some(cx.new(|_| TextInputController::new()));
        }
        if self.add_member_search_input.is_none() {
            self.add_member_search_input = Some(cx.new(|_| TextInputController::new()));
        }
        if self.memory_input.is_none() {
            self.memory_input = Some(cx.new(|_| TextInputController::new()));
        }
        if self.openbot_text_input.is_none() {
            self.openbot_text_input = Some(cx.new(|_| TextInputController::new()));
        }
    }

    /// Synchronizes the live Native Bot Fusion snapshot (Memory, Skills, MCP, Routines, Workflows, Queue).
    pub fn set_bot_fusion_snapshot(&mut self, snap: BotFusionSnapshot) {
        let same_agent = !snap.fabushi_agent_id.is_empty()
            && self.bot_fusion.fabushi_agent_id == snap.fabushi_agent_id;
        if !same_agent {
            self.memory_draft.clear();
            if self.bot_config_focus == BotConfigFocusField::MemoryDraft {
                self.bot_config_focus = BotConfigFocusField::None;
            }
        }
        if let Some(ref mut fab) = self.active_fabushi_bot {
            if !snap.fabushi_agent_id.is_empty() && fab.agent_id == snap.fabushi_agent_id {
                fab.memory_entries_count = snap.memories.len();
                fab.skills_count = snap.skills.iter().filter(|s| s.enabled).count();
            }
        }
        self.bot_fusion = snap;
    }

    /// Emits `InspectorAction::AddBotMemory` when `memory_draft` is non-empty and resets the draft.
    pub fn submit_memory_draft(&mut self) -> bool {
        let text = self.memory_draft.trim().to_string();
        if text.is_empty() {
            return false;
        }
        self.memory_draft.clear();
        self.bot_config_focus = BotConfigFocusField::None;
        self.pending_action = Some(InspectorAction::AddBotMemory(text));
        true
    }

    pub fn sync_computers(&mut self, computers: Vec<ComputerNodeDto>) {
        self.computers = computers;
    }

    /// Resolves the bot's explicit per-bot preferred model override (empty/`None` means follow Settings).
    pub fn effective_bot_preferred_model(&self) -> Option<String> {
        if let Some(ref p) = self.bot_computer_profile {
            let trimmed = p.preferred_model.trim();
            if !trimmed.is_empty() {
                return (!InspectorAiModelCatalog::is_placeholder_model(trimmed))
                    .then(|| trimmed.to_string());
            }
            // Non-zero updated_at_ms means the profile was explicitly saved with an empty override
            // ("Follow Settings"), so do not resurrect a stale value from a cached summary.
            if p.updated_at_ms > 0 {
                return None;
            }
        }
        self.active_fabushi_bot
            .as_ref()
            .map(|b| b.preferred_model.trim())
            .filter(|s| !InspectorAiModelCatalog::is_placeholder_model(s))
            .map(ToString::to_string)
    }

    pub fn sync_bot_computer_profile(&mut self, profile: Option<BotComputerProfileDto>) {
        let same_bot = self
            .bot_computer_profile
            .as_ref()
            .zip(profile.as_ref())
            .is_some_and(|(a, b)| a.bot_id == b.bot_id);
        let editing_now = self.subpage == InspectorSubpage::BotConfigEditor && same_bot;
        self.bot_computer_profile = profile.clone();
        if !editing_now {
            if let Some(p) = profile {
                self.bot_draft_persona = p.persona_prompt;
                self.bot_draft_model = self.effective_bot_preferred_model().unwrap_or_default();
                self.bot_draft_approval_mode = p.approval_mode;
                self.bot_draft_allow_cross_device = p.allow_cross_device_control;
                self.bot_draft_allow_cloud_self_config = p.allow_cloud_self_config;
            } else {
                self.bot_draft_persona = self.profile.bot_persona_prompt.clone();
                self.bot_draft_model = self.effective_bot_preferred_model().unwrap_or_default();
            }
        }
    }

    pub fn open_bot_config_editor(&mut self, cx: &mut Context<Self>) {
        self.ensure_bot_inputs(cx);
        if let Some(ref ctrl) = self.persona_input {
            ctrl.update(cx, |c, _| c.clear_preedit());
        }
        if let Some(ref ctrl) = self.model_input {
            ctrl.update(cx, |c, _| c.clear_preedit());
        }
        if let Some(ref p) = self.bot_computer_profile {
            self.bot_draft_persona = p.persona_prompt.clone();
            self.bot_draft_model = self.effective_bot_preferred_model().unwrap_or_default();
            self.bot_draft_approval_mode = p.approval_mode;
            self.bot_draft_allow_cross_device = p.allow_cross_device_control;
            self.bot_draft_allow_cloud_self_config = p.allow_cloud_self_config;
        } else {
            self.bot_draft_persona = self.profile.bot_persona_prompt.clone();
            self.bot_draft_model = self.effective_bot_preferred_model().unwrap_or_default();
        }
        self.subpage = InspectorSubpage::BotConfigEditor;
        self.bot_config_focus = BotConfigFocusField::Persona;
        cx.notify();
    }

    pub fn is_add_member_search_active(&self) -> bool {
        self.is_open
            && !self.profile.is_bot
            && (self.active_tab == InspectorTab::Members || self.embedded_in_side_panel)
            && self.add_member_picker_open
            && self.add_member_search_focused
    }

    pub fn is_bot_input_active(&self) -> bool {
        if self.is_add_member_search_active() {
            return true;
        }
        if self.is_open && self.openbot_text_focus.is_some() {
            return true;
        }
        self.is_open
            && matches!(self.active_tab, InspectorTab::Profile | InspectorTab::Members)
            && self.profile.is_bot
            && ((self.subpage == InspectorSubpage::BotConfigEditor
                && matches!(
                    self.bot_config_focus,
                    BotConfigFocusField::Persona | BotConfigFocusField::Model
                ))
                || ((self.subpage == InspectorSubpage::Main
                    || self.openbot_subpage == Some(OpenBotSettingsSubpage::Memories))
                    && matches!(
                        self.bot_config_focus,
                        BotConfigFocusField::CodingWorkspace
                            | BotConfigFocusField::CodingModel
                            | BotConfigFocusField::MemoryDraft
                    )))
    }

    fn active_bot_controller(&self) -> Option<&Entity<TextInputController>> {
        if self.is_add_member_search_active() {
            return self.add_member_search_input.as_ref();
        }
        if self.openbot_text_focus.is_some() {
            return self.openbot_text_input.as_ref();
        }
        match self.bot_config_focus {
            BotConfigFocusField::Persona => self.persona_input.as_ref(),
            BotConfigFocusField::Model => self.model_input.as_ref(),
            BotConfigFocusField::CodingWorkspace => self.coding_workspace_input.as_ref(),
            BotConfigFocusField::CodingModel => self.coding_model_input.as_ref(),
            BotConfigFocusField::MemoryDraft => self.memory_input.as_ref(),
            BotConfigFocusField::None => None,
        }
    }

    fn active_bot_draft_text(&self) -> &str {
        if self.is_add_member_search_active() {
            return &self.add_member_query;
        }
        if let Some(field) = self.openbot_text_focus {
            return match field {
                OpenBotInspectorTextField::BotName => &self.bot_draft_name,
                OpenBotInspectorTextField::BotTitle => &self.bot_draft_title,
                OpenBotInspectorTextField::BotDescription => &self.bot_draft_description,
                OpenBotInspectorTextField::BotInstructions => &self.bot_draft_persona,
                OpenBotInspectorTextField::BotBackupModelDraft => &self.bot_backup_model_draft,
                OpenBotInspectorTextField::BotFolderDraft => &self.bot_folder_draft,
                OpenBotInspectorTextField::ChannelName => &self.channel_draft_name,
                OpenBotInspectorTextField::ChannelTitle => &self.channel_draft_title,
                OpenBotInspectorTextField::ChannelInstructions => &self.channel_draft_instructions,
                OpenBotInspectorTextField::ChannelMemoryDraft => &self.channel_memory_draft,
                OpenBotInspectorTextField::ChannelFolderDraft => &self.channel_folder_draft,
            };
        }
        match self.bot_config_focus {
            BotConfigFocusField::Persona => &self.bot_draft_persona,
            BotConfigFocusField::Model => &self.bot_draft_model,
            BotConfigFocusField::CodingWorkspace => &self.coding_draft_workspace,
            BotConfigFocusField::CodingModel => &self.coding_draft_model_override,
            BotConfigFocusField::MemoryDraft => &self.memory_draft,
            BotConfigFocusField::None => "",
        }
    }

    fn active_bot_draft_mut(&mut self) -> Option<&mut String> {
        if self.is_add_member_search_active() {
            return Some(&mut self.add_member_query);
        }
        if let Some(field) = self.openbot_text_focus {
            return Some(match field {
                OpenBotInspectorTextField::BotName => &mut self.bot_draft_name,
                OpenBotInspectorTextField::BotTitle => &mut self.bot_draft_title,
                OpenBotInspectorTextField::BotDescription => &mut self.bot_draft_description,
                OpenBotInspectorTextField::BotInstructions => &mut self.bot_draft_persona,
                OpenBotInspectorTextField::BotBackupModelDraft => &mut self.bot_backup_model_draft,
                OpenBotInspectorTextField::BotFolderDraft => &mut self.bot_folder_draft,
                OpenBotInspectorTextField::ChannelName => &mut self.channel_draft_name,
                OpenBotInspectorTextField::ChannelTitle => &mut self.channel_draft_title,
                OpenBotInspectorTextField::ChannelInstructions => {
                    &mut self.channel_draft_instructions
                }
                OpenBotInspectorTextField::ChannelMemoryDraft => &mut self.channel_memory_draft,
                OpenBotInspectorTextField::ChannelFolderDraft => &mut self.channel_folder_draft,
            });
        }
        match self.bot_config_focus {
            BotConfigFocusField::Persona => Some(&mut self.bot_draft_persona),
            BotConfigFocusField::Model => Some(&mut self.bot_draft_model),
            BotConfigFocusField::CodingWorkspace => Some(&mut self.coding_draft_workspace),
            BotConfigFocusField::CodingModel => Some(&mut self.coding_draft_model_override),
            BotConfigFocusField::MemoryDraft => Some(&mut self.memory_draft),
            BotConfigFocusField::None => None,
        }
    }

    pub fn active_bot_input_text_and_preedit(&self, cx: &App) -> (String, Option<String>) {
        let committed = self.active_bot_draft_text().to_string();
        if let Some(ctrl) = self.active_bot_controller() {
            ctrl.read(cx).text_and_preedit(committed)
        } else {
            (committed, None)
        }
    }

    fn now_epoch_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn sync_bot_autosave_from_active_field(&mut self) {
        let now = Self::now_epoch_ms();
        match self.openbot_text_focus {
            Some(OpenBotInspectorTextField::BotName) => {
                let draft = self.bot_draft_name.clone();
                self.bot_autosave.edit_name(draft, now);
            }
            Some(OpenBotInspectorTextField::BotDescription) => {
                let draft = self.bot_draft_description.clone();
                self.bot_autosave.edit_bio(draft, now);
            }
            _ => {}
        }
    }

    pub fn push_bot_input_text(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if !self.is_bot_input_active() {
            return false;
        }
        if let Some(ctrl) = self.active_bot_controller().cloned() {
            ctrl.update(cx, |c, _| c.clear_preedit());
        }
        if let Some(dst) = self.active_bot_draft_mut() {
            dst.push_str(text);
            self.sync_bot_autosave_from_active_field();
            cx.notify();
            return true;
        }
        false
    }

    pub fn clear_active_bot_input(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.is_bot_input_active() {
            return false;
        }
        if let Some(ctrl) = self.active_bot_controller().cloned() {
            ctrl.update(cx, |c, _| c.clear_preedit());
        }
        if let Some(dst) = self.active_bot_draft_mut() {
            dst.clear();
            self.sync_bot_autosave_from_active_field();
            cx.notify();
            return true;
        }
        false
    }

    pub fn handle_bot_non_char_key(
        &mut self,
        key: &str,
        is_alt: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.is_bot_input_active() {
            return false;
        }
        match key {
            "backspace" | "delete" => {
                let changed = if let Some(dst) = self.active_bot_draft_mut() {
                    if is_alt {
                        delete_prev_word(dst)
                    } else {
                        pop_last_char(dst)
                    }
                } else {
                    false
                };
                if changed {
                    self.sync_bot_autosave_from_active_field();
                    cx.notify();
                }
                true
            }
            "tab" => {
                self.bot_config_focus = match self.bot_config_focus {
                    BotConfigFocusField::Persona => BotConfigFocusField::Model,
                    BotConfigFocusField::Model => BotConfigFocusField::Persona,
                    BotConfigFocusField::CodingWorkspace => BotConfigFocusField::CodingModel,
                    BotConfigFocusField::CodingModel => BotConfigFocusField::CodingWorkspace,
                    BotConfigFocusField::MemoryDraft => BotConfigFocusField::None,
                    BotConfigFocusField::None => BotConfigFocusField::None,
                };
                cx.notify();
                true
            }
            "enter" => {
                if self.is_add_member_search_active() {
                    if let Some(first) =
                        filter_add_member_candidates(&self.add_member_candidates, &self.add_member_query)
                            .first()
                            .map(|c| c.user_id)
                    {
                        self.request_add_member_to_active_group(first);
                        cx.notify();
                    }
                    return true;
                }
                if let Some(field) = self.openbot_text_focus {
                    match field {
                        OpenBotInspectorTextField::BotDescription => {
                            self.bot_draft_description.push('\n');
                            self.sync_bot_autosave_from_active_field();
                        }
                        OpenBotInspectorTextField::BotInstructions => {
                            self.bot_draft_persona.push('\n');
                        }
                        OpenBotInspectorTextField::ChannelInstructions => {
                            self.channel_draft_instructions.push('\n');
                        }
                        OpenBotInspectorTextField::BotName
                        | OpenBotInspectorTextField::BotTitle => {
                            self.commit_bot_identity_draft();
                        }
                        OpenBotInspectorTextField::BotBackupModelDraft => {
                            self.add_bot_backup_model_from_draft();
                        }
                        OpenBotInspectorTextField::BotFolderDraft => {
                            self.add_bot_folder_from_draft();
                        }
                        OpenBotInspectorTextField::ChannelName
                        | OpenBotInspectorTextField::ChannelTitle => {
                            self.commit_channel_identity_draft();
                        }
                        OpenBotInspectorTextField::ChannelMemoryDraft => {
                            self.add_channel_memory_from_draft();
                        }
                        OpenBotInspectorTextField::ChannelFolderDraft => {
                            self.add_channel_folder_from_draft();
                        }
                    }
                    cx.notify();
                    return true;
                }
                if self.bot_config_focus == BotConfigFocusField::MemoryDraft {
                    if !self.submit_memory_draft() {
                        self.bot_config_focus = BotConfigFocusField::None;
                    }
                    cx.notify();
                    return true;
                }
                if self.bot_config_focus == BotConfigFocusField::Persona {
                    self.bot_draft_persona.push('\n');
                    cx.notify();
                } else {
                    self.bot_config_focus = BotConfigFocusField::None;
                    cx.notify();
                }
                true
            }
            "escape" if self.openbot_text_focus.is_some() => {
                self.openbot_text_focus = None;
                cx.notify();
                true
            }
            "escape" if self.bot_config_focus == BotConfigFocusField::MemoryDraft => {
                self.bot_config_focus = BotConfigFocusField::None;
                cx.notify();
                true
            }
            "escape" if self.is_add_member_search_active() => {
                if !self.add_member_query.is_empty() {
                    self.add_member_query.clear();
                } else {
                    self.close_add_member_picker();
                }
                cx.notify();
                true
            }
            _ => false,
        }
    }

    pub fn replace_and_mark_bot_input(
        &mut self,
        new_text: &str,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.ensure_bot_inputs(cx);
        let commit = self
            .active_bot_controller()
            .cloned()
            .and_then(|c| c.update(cx, |ctrl, _| ctrl.handle_replace_and_mark(new_text)));
        if let Some(txt) = commit.as_ref() {
            if let Some(dst) = self.active_bot_draft_mut() {
                dst.push_str(txt);
            }
            self.sync_bot_autosave_from_active_field();
        }
        cx.notify();
        commit
    }

    pub fn set_bot_ime_preedit(&mut self, preedit: Option<&str>, cx: &mut Context<Self>) {
        self.ensure_bot_inputs(cx);
        if let Some(ctrl) = self.active_bot_controller().cloned() {
            ctrl.update(cx, |c, _| {
                c.ime_preedit = preedit.filter(|s| !s.is_empty()).map(str::to_string);
            });
            cx.notify();
        }
    }

    pub fn clear_all_bot_preedits(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let controllers = [
            self.add_member_search_input.clone(),
            self.persona_input.clone(),
            self.model_input.clone(),
            self.coding_workspace_input.clone(),
            self.coding_model_input.clone(),
            self.memory_input.clone(),
            self.openbot_text_input.clone(),
        ];
        for ctrl in controllers.into_iter().flatten() {
            if ctrl.read(cx).ime_preedit.is_some() {
                ctrl.update(cx, |c, _| {
                    c.ime_preedit = None;
                });
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    pub fn blur_bot_input(&mut self) {
        self.bot_config_focus = BotConfigFocusField::None;
        self.openbot_text_focus = None;
        self.add_member_search_focused = false;
    }
}

pub const BOT_IDENTITY_AUTOSAVE_DEBOUNCE_MS: u64 = 400;
pub const AGENT_SETTINGS_AUTOSAVE_DEBOUNCE_MS: u64 = BOT_IDENTITY_AUTOSAVE_DEBOUNCE_MS;
pub const BOT_PROFILE_NAME_MAX_CHARS: usize = 80;

/// Validates an OpenBot Agent/Bot profile display name (`1..=80` non-whitespace chars, `UI-F16`).
pub fn validate_bot_profile_name(name: &str) -> Result<(), &'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Agent name cannot be empty");
    }
    if trimmed.chars().count() > BOT_PROFILE_NAME_MAX_CHARS {
        return Err("Agent name exceeds 80 character limit");
    }
    Ok(())
}

/// Outcome returned by [`ContextInspector::try_start_new_chat_from_settings`] (`UI-F16-03`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewChatFromSettingsOutcome {
    pub new_chat_busy_toast: bool,
    pub chat_reset: bool,
    pub toast_message: Option<&'static str>,
}

/// Single-source-of-truth `400ms` debounced autosave state for Bot Settings Name/Title/Description (`UI-F16`),
/// supporting dirty-flush to the previous agent before switching and optimistic rollback on save error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotIdentityAutosaveState {
    pub active_agent_id: String,
    pub dirty: bool,
    pub last_edit_ms: u64,
    pub debounce_ms: u64,
    pub committed_name: String,
    pub committed_bio: String,
    pub draft_name: String,
    pub draft_bio: String,
    pub save_error: Option<String>,
    pub last_flushed_ms: Option<u64>,
    pub last_flushed_agent_id: Option<String>,
    pub last_flushed_name: Option<String>,
    pub last_flushed_bio: Option<String>,
    pub flush_count: usize,
}

impl Default for BotIdentityAutosaveState {
    fn default() -> Self {
        Self::new("", "", "")
    }
}

impl BotIdentityAutosaveState {
    pub fn new(
        agent_id: impl Into<String>,
        name: impl Into<String>,
        bio: impl Into<String>,
    ) -> Self {
        let name_s = name.into();
        let bio_s = bio.into();
        Self {
            active_agent_id: agent_id.into(),
            dirty: false,
            last_edit_ms: 0,
            debounce_ms: BOT_IDENTITY_AUTOSAVE_DEBOUNCE_MS,
            committed_name: name_s.clone(),
            committed_bio: bio_s.clone(),
            draft_name: name_s,
            draft_bio: bio_s,
            save_error: None,
            last_flushed_ms: None,
            last_flushed_agent_id: None,
            last_flushed_name: None,
            last_flushed_bio: None,
            flush_count: 0,
        }
    }

    pub fn edit_name(&mut self, name: impl Into<String>, now_ms: u64) {
        self.draft_name = name.into();
        self.save_error = validate_bot_profile_name(&self.draft_name)
            .err()
            .map(str::to_string);
        self.record_edit(now_ms);
    }

    pub fn edit_bio(&mut self, bio: impl Into<String>, now_ms: u64) {
        self.draft_bio = bio.into();
        self.save_error = None;
        self.record_edit(now_ms);
    }

    pub fn record_edit(&mut self, now_ms: u64) {
        self.dirty = true;
        self.last_edit_ms = now_ms;
    }

    pub fn should_flush(&self, now_ms: u64) -> bool {
        self.dirty && now_ms.saturating_sub(self.last_edit_ms) >= self.debounce_ms
    }

    pub fn flush_now(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        if let Err(err) = validate_bot_profile_name(&self.draft_name) {
            let err_s = err.to_string();
            self.on_save_failed(err_s.clone());
            return Err(err_s);
        }
        self.committed_name = self.draft_name.clone();
        self.committed_bio = self.draft_bio.clone();
        self.dirty = false;
        self.save_error = None;
        self.last_flushed_agent_id = Some(self.active_agent_id.clone());
        self.last_flushed_name = Some(self.committed_name.clone());
        self.last_flushed_bio = Some(self.committed_bio.clone());
        self.flush_count = self.flush_count.saturating_add(1);
        Ok(())
    }

    pub fn flush_if_due(&mut self, now_ms: u64) -> bool {
        if self.should_flush(now_ms) && self.flush_now().is_ok() {
            self.last_flushed_ms = Some(now_ms);
            true
        } else {
            false
        }
    }

    /// Strictly flushes any pending edits to the PREVIOUS `active_agent_id` BEFORE replacing
    /// the active agent state with `(next_id, next_name, next_bio)` (`T2-F16-01`).
    pub fn flush_before_agent_switch(
        &mut self,
        next_id: impl Into<String>,
        next_name: impl Into<String>,
        next_bio: impl Into<String>,
    ) -> bool {
        let next_id_s = next_id.into();
        let next_name_s = next_name.into();
        let next_bio_s = next_bio.into();
        if self.active_agent_id == next_id_s && !next_id_s.is_empty() {
            if !self.dirty {
                self.committed_name = next_name_s.clone();
                self.committed_bio = next_bio_s.clone();
                self.draft_name = next_name_s;
                self.draft_bio = next_bio_s;
            }
            return false;
        }
        let flushed_prev = if self.dirty && !self.active_agent_id.is_empty() {
            self.flush_now().is_ok()
        } else {
            false
        };
        self.active_agent_id = next_id_s;
        self.committed_name = next_name_s.clone();
        self.committed_bio = next_bio_s.clone();
        self.draft_name = next_name_s;
        self.draft_bio = next_bio_s;
        self.dirty = false;
        self.save_error = None;
        flushed_prev
    }

    /// Rolls draft fields back to the last committed values and records `save_error` (`T2-F16-04`).
    pub fn on_save_failed(&mut self, err: impl Into<String>) {
        self.draft_name = self.committed_name.clone();
        self.draft_bio = self.committed_bio.clone();
        self.dirty = false;
        self.save_error = Some(err.into());
    }
}

/// Pure render route resolved by `ContextInspector::render` when hosted inside the unified `SidePanel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextInspectorRenderRoute {
    Closed,
    ClassicDrawer,
    OpenBotBotSettingsRoot,
    OpenBotBotSubpage(OpenBotSettingsSubpage),
    OpenBotChannelSettingsRoot,
    OpenBotChannelSubpage(OpenBotSettingsSubpage),
    OpenBotContactProfileRoot,
    OpenBotSharedMediaSubpage,
}

impl ContextInspector {
    /// Syncs `ContextInspector` open/embedded state for the unified OpenBot Right `SidePanel` (`UI-F15-01`).
    pub fn sync_inspector_for_unified_side_panel(&mut self, settings_visible: bool) {
        self.embedded_in_side_panel = true;
        if settings_visible {
            self.open();
        } else {
            self.close();
        }
    }

    /// Resolves the live OpenBot settings/profile view route (consumed directly by `ContextInspector::render`).
    pub fn resolve_context_inspector_render_route(&self) -> ContextInspectorRenderRoute {
        if !self.is_open {
            return ContextInspectorRenderRoute::Closed;
        }
        if !self.embedded_in_side_panel {
            return ContextInspectorRenderRoute::ClassicDrawer;
        }
        if let Some(ob_s) = self.openbot_subpage {
            if ob_s == OpenBotSettingsSubpage::SharedMedia {
                return ContextInspectorRenderRoute::OpenBotSharedMediaSubpage;
            }
            if self.profile.is_bot {
                return ContextInspectorRenderRoute::OpenBotBotSubpage(ob_s);
            }
            if self.members.is_group_or_channel {
                return ContextInspectorRenderRoute::OpenBotChannelSubpage(ob_s);
            }
            return ContextInspectorRenderRoute::OpenBotSharedMediaSubpage;
        }
        if self.profile.is_bot {
            ContextInspectorRenderRoute::OpenBotBotSettingsRoot
        } else if self.members.is_group_or_channel {
            ContextInspectorRenderRoute::OpenBotChannelSettingsRoot
        } else {
            ContextInspectorRenderRoute::OpenBotContactProfileRoot
        }
    }

    /// Triggers one-click `@handle` copy feedback toast & clipboard action (`UI-F16-02`).
    pub fn trigger_copy_bot_handle(&mut self) -> Option<String> {
        let handle = self.profile.username.trim().trim_start_matches('@');
        if handle.is_empty() {
            return None;
        }
        let full_handle = format!("@{handle}");
        let msg = format!(
            "{}: {full_handle}",
            tr_str(&self.language_id, "已复制", "Copied")
        );
        self.copied_feedback = Some(msg.clone());
        self.pending_action = Some(InspectorAction::CopyText(full_handle));
        Some(msg)
    }

    /// Navigates into a bot settings drill-down subpage and returns the updated breadcrumb label (`UI-F16-04`).
    pub fn open_bot_subpage(&mut self, subpage: OpenBotSettingsSubpage) -> &'static str {
        self.openbot_subpage = Some(subpage);
        subpage.title(&self.language_id)
    }
}

impl Default for ContextInspector { fn default() -> Self { Self::new() } }

impl Render for ContextInspector {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_open {
            if self.copied_feedback.is_some() {
                self.clear_copied_feedback();
            }
            return div().w(px(0.0)).h_full().overflow_hidden().into_any_element();
        }
        let pal = self.palette;
        if !self.profile.is_bot && !self.members.is_group_or_channel && self.active_tab == InspectorTab::Members {
            self.active_tab = InspectorTab::Profile;
        }
        let active_tab = self.active_tab;
        let allow_owner_subpages =
            self.profile.is_viewer_owner && self.active_fabushi_bot.is_some();
        let subpage = if self.profile.is_bot
            && allow_owner_subpages
            && matches!(active_tab, InspectorTab::Profile | InspectorTab::Members)
        {
            self.subpage
        } else {
            InspectorSubpage::Main
        };
        if self.embedded_in_side_panel {
            self.ensure_bot_inputs(cx);
            let lang = self.language_id.as_str();
            let ob = pal.openbot_palette();
            let subpage = if self.profile.is_bot && allow_owner_subpages {
                self.subpage
            } else {
                InspectorSubpage::Main
            };
            let ob_sub = self.openbot_subpage;
            let in_subpage = subpage != InspectorSubpage::Main || ob_sub.is_some();

            let subpage_title: Option<&str> = if let Some(s) = ob_sub {
                Some(s.title(lang))
            } else {
                match subpage {
                    InspectorSubpage::ComputersList => Some(tr_str(lang, "远程电脑", "Computers")),
                    InspectorSubpage::BotConfigEditor => Some(tr_str(lang, "模型与配置", "Model & Config")),
                    InspectorSubpage::Main => None,
                }
            };

            let route = self.resolve_context_inspector_render_route();
            let body: AnyElement = match route {
                ContextInspectorRenderRoute::OpenBotSharedMediaSubpage => {
                    render_shared_media_panel(&self.shared_media, pal, lang, cx)
                }
                ContextInspectorRenderRoute::OpenBotBotSubpage(ob_s) => {
                    render_openbot_bot_subpage(self, ob_s, pal, lang, cx)
                }
                ContextInspectorRenderRoute::OpenBotChannelSubpage(ob_s) => {
                    render_openbot_channel_subpage(self, ob_s, pal, lang, cx)
                }
                ContextInspectorRenderRoute::OpenBotBotSettingsRoot => match subpage {
                    InspectorSubpage::Main => render_openbot_bot_settings_root(self, pal, lang, cx),
                    InspectorSubpage::ComputersList => {
                        render_bot_computers_subpage(self, pal, lang, cx)
                    }
                    InspectorSubpage::BotConfigEditor => {
                        render_bot_config_editor(self, pal, lang, cx)
                    }
                },
                ContextInspectorRenderRoute::OpenBotChannelSettingsRoot => {
                    render_openbot_channel_settings_root(self, pal, lang, cx)
                }
                ContextInspectorRenderRoute::OpenBotContactProfileRoot
                | ContextInspectorRenderRoute::Closed
                | ContextInspectorRenderRoute::ClassicDrawer => {
                    render_openbot_contact_profile_root(self, pal, lang, cx)
                }
            };

            return div()
                .id("openbot-embedded-settings-root")
                .w_full()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .h_full()
                .bg(ob.bg_canvas())
                .flex()
                .flex_col()
                .when(in_subpage, |col| {
                    let title = subpage_title.unwrap_or("").to_string();
                    col.child(
                        div()
                            .id("openbot-settings-subpage-header")
                            .h(px(39.0))
                            .flex_shrink_0()
                            .px(px(10.0))
                            .border_b_1()
                            .border_color(ob.border_hairline())
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .id("inspector-breadcrumb-root")
                                    .cursor_pointer()
                                    .h(px(28.0))
                                    .px(px(6.0))
                                    .rounded(px(6.0))
                                    .hover(|s| s.bg(ob.hover()))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .text_size(px(13.0))
                                    .text_color(ob.text_secondary())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.navigate_back_subpage();
                                            cx.notify();
                                        }),
                                    )
                                    .child(crate::theme::tg_icon_rgba(
                                        IconName::BackArrow,
                                        14.0,
                                        ob.text_muted(),
                                    ))
                                    .child(tr_str(lang, "返回", "Back")),
                            )
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ob.text_primary())
                                    .truncate()
                                    .child(title),
                            )
                            .child(div().w(px(48.0))),
                    )
                })
                .child(
                    div()
                        .id("inspector-tab-scroll")
                        .flex_1()
                        .min_h(px(0.0))
                        .px(px(15.0))
                        .pt(px(16.0))
                        .pb(px(28.0))
                        .overflow_y_scroll()
                        .child(body),
                )
                .into_any_element();
        }

        if (self.profile.is_bot
            && (subpage == InspectorSubpage::BotConfigEditor
                || (subpage == InspectorSubpage::Main
                    && active_tab == InspectorTab::Members
                    && self
                        .active_fabushi_bot
                        .as_ref()
                        .and_then(|b| b.coding_agent.as_ref())
                        .is_some())))
            || (!self.profile.is_bot
                && active_tab == InspectorTab::Members
                && self.add_member_picker_open)
        {
            self.ensure_bot_inputs(cx);
        }
        let lang = self.language_id.as_str();

        let body = match active_tab {
            InspectorTab::Profile => {
                if self.profile.is_bot {
                    match subpage {
                        InspectorSubpage::Main => render_bot_profile_main(self, pal, lang, cx),
                        InspectorSubpage::ComputersList => {
                            render_bot_computers_subpage(self, pal, lang, cx)
                        }
                        InspectorSubpage::BotConfigEditor => {
                            render_bot_config_editor(self, pal, lang, cx)
                        }
                    }
                } else {
                    render_contact_profile_panel(
                        &self.profile,
                        &self.members,
                        &self.shared_media,
                        &self.collapsed_sections,
                        self.is_secret_chat_active,
                        self.is_contact_blocked,
                        self.copied_feedback.as_ref(),
                        pal,
                        lang,
                        cx,
                    )
                }
            }
            InspectorTab::Members => {
                if self.profile.is_bot {
                    match subpage {
                        InspectorSubpage::Main => {
                            render_bot_agent_and_computer_tab(self, pal, lang, cx)
                        }
                        InspectorSubpage::ComputersList => {
                            render_bot_computers_subpage(self, pal, lang, cx)
                        }
                        InspectorSubpage::BotConfigEditor => {
                            render_bot_config_editor(self, pal, lang, cx)
                        }
                    }
                } else {
                    render_members_panel_with_picker(self, pal, lang, cx)
                }
            }
            InspectorTab::SharedMedia => render_shared_media_panel(&self.shared_media, pal, lang, cx),
            InspectorTab::Settings => {
                if self.profile.is_bot {
                    render_bot_settings_tab(self, pal, lang, cx)
                } else {
                    render_settings_and_stats_panel(
                        &self.settings,
                        self.channel_boost_level,
                        self.channel_boosts_current,
                        self.channel_boosts_target,
                        &self.channel_stats,
                        pal,
                        lang,
                        cx,
                    )
                }
            }
        };

        let is_dark = pal.mode.is_dark();
        let header_height = if subpage == InspectorSubpage::Main {
            44.0
        } else {
            48.0
        };
        let drawer_bg = if pal.is_apple_style() {
            pal.stage_bg
        } else {
            pal.dialogs_bg
        };

        let header = if subpage != InspectorSubpage::Main {
            let sub_label = match subpage {
                InspectorSubpage::ComputersList => tr_str(lang, "电脑", "Computers"),
                InspectorSubpage::BotConfigEditor => tr_str(lang, "AI Bot 配置", "Bot Config"),
                InspectorSubpage::Main => "",
            };
            div()
                .h(px(header_height))
                .flex_shrink_0()
                .px(px(14.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("inspector-breadcrumb-root")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded_full()
                        .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.text_secondary))
                        .hover(|s| s.text_color(rgb(pal.text_primary)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.subpage = InspectorSubpage::Main;
                                this.bot_config_focus = BotConfigFocusField::None;
                                this.pending_action =
                                    Some(InspectorAction::CloseBotComputersSubpage);
                                cx.notify();
                            }),
                        )
                        .child(tg_icon(IconName::BackArrow, 13.0, pal.text_secondary))
                        .child(tr_str(lang, "返回", "Back")),
                )
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(sub_label.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .window_control_area(WindowControlArea::Drag),
                )
                .child(Self::close_icon_btn(pal, cx))
        } else {
            div()
                .h(px(header_height))
                .flex_shrink_0()
                .px(px(14.0))
                .flex()
                .items_center()
                .justify_end()
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .window_control_area(WindowControlArea::Drag),
                )
                .child(Self::close_icon_btn(pal, cx))
        };

        let mut root = div()
            .w(px(DRAWER_WIDTH))
            .flex_shrink_0()
            .h_full()
            .bg(rgb(drawer_bg))
            .border_l_1()
            .border_color(pal.separator_rgba())
            .flex()
            .flex_col()
            .child(header);

        if subpage == InspectorSubpage::Main {
            let model = &self.profile;
            let is_owner_fabushi = model.is_viewer_owner && self.active_fabushi_bot.is_some();
            let can_edit_hero = is_owner_fabushi || !model.is_bot;
            let is_verified = model.is_verified
                || model
                    .username
                    .trim()
                    .trim_start_matches('@')
                    .eq_ignore_ascii_case("BotFather");
            let avatar_px = 84.0;
            let avatar_color = if model.avatar_color != 0 {
                model.avatar_color
            } else {
                pal.accent
            };
            let fallback_initials = {
                let trimmed = model.avatar_initials.trim();
                if !trimmed.is_empty() && trimmed != "?" {
                    trimmed.to_string()
                } else {
                    let derived = crate::views::app_shell::shell_truth::derive_initials(&model.name);
                    if derived.is_empty() {
                        if model.is_bot { "BOT".to_string() } else { "?".to_string() }
                    } else {
                        derived
                    }
                }
            };
            let avatar_shadow = vec![BoxShadow {
                color: hsla(0.0, 0.0, 0.0, if is_dark { 0.34 } else { 0.12 }),
                offset: point(px(0.0), px(4.0)),
                blur_radius: px(14.0),
                spread_radius: px(0.0),
            }];
            let core_avatar = if let Some(ref path) = model.avatar_path {
                div()
                    .w(px(avatar_px))
                    .h(px(avatar_px))
                    .rounded_full()
                    .overflow_hidden()
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .shadow(avatar_shadow)
                    .child(
                        img(path.clone())
                            .w(px(avatar_px))
                            .h(px(avatar_px))
                            .object_fit(ObjectFit::Cover),
                    )
                    .into_any_element()
            } else if is_owner_fabushi {
                if let Some(ref fab) = self.active_fabushi_bot {
                    let spin_id = fab.agent_id.clone();
                    let state = GrokPersonaState::from_activity_str(&fab.execution_state, false);
                    let avatar_el = if fab
                        .avatar_shape
                        .eq_ignore_ascii_case(CLASSIC_INITIALS_AVATAR_SHAPE)
                    {
                        render_grok_persona_avatar(
                            &fab.agent_id,
                            &fab.avatar_shape,
                            &fab.avatar_color,
                            &fallback_initials,
                            avatar_px,
                            state,
                            drawer_bg,
                            false,
                        )
                    } else {
                        render_grok_bot_avatar(
                            GrokBotAvatarProps::new(
                                &fab.agent_id,
                                &fab.avatar_shape,
                                &fab.avatar_color,
                                avatar_px,
                            )
                            .with_state(state)
                            .with_follow_pointer(true)
                            .with_surface_bg(drawer_bg),
                        )
                    };
                    div()
                        .id("bot-inspector-grok-avatar")
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _, _, cx| {
                                trigger_grok_avatar_spin(&spin_id);
                                cx.notify();
                            }),
                        )
                        .child(avatar_el)
                        .into_any_element()
                } else {
                    div().into_any_element()
                }
            } else {
                div()
                    .w(px(avatar_px))
                    .h(px(avatar_px))
                    .rounded_full()
                    .bg(rgb(avatar_color))
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .shadow(avatar_shadow)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(26.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xFFFFFF))
                    .child(fallback_initials)
                    .into_any_element()
            };

            let status_trimmed = model.status.trim();
            let clean_handle = model.username.trim();
            let (show_green_dot, status_label) = if model.is_bot {
                (true, tr_str(lang, "已连接", "Connected").to_string())
            } else if status_trimmed.eq_ignore_ascii_case("online") || status_trimmed == "在线" {
                (true, tr_str(lang, "在线", "Online").to_string())
            } else if !status_trimmed.is_empty() {
                (false, status_trimmed.to_string())
            } else {
                (false, clean_handle.to_string())
            };

            let edit_badge = can_edit_hero.then(|| {
                div()
                    .id("inspector-hero-edit-btn")
                    .cursor_pointer()
                    .absolute()
                    .bottom(px(0.0))
                    .right(px(0.0))
                    .w(px(26.0))
                    .h(px(26.0))
                    .rounded_full()
                    .bg(rgb(pal.modal_bg))
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .shadow(segmented_pill_shadow(pal))
                    .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            if is_owner_fabushi {
                                this.avatar_editor_open = !this.avatar_editor_open;
                            } else {
                                this.pending_action = Some(InspectorAction::OpenEditPeer);
                            }
                            cx.notify();
                        }),
                    )
                    .child(tg_icon(
                        IconName::Edit,
                        12.0,
                        if self.avatar_editor_open { pal.accent } else { pal.icon_fg },
                    ))
            });

            let hero = div()
                .flex_shrink_0()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .pb(px(6.0))
                .child(
                    div()
                        .relative()
                        .w(px(avatar_px))
                        .h(px(avatar_px))
                        .child(core_avatar)
                        .when_some(edit_badge, |wrap, badge| wrap.child(badge)),
                )
                .child(
                    div()
                        .pt(px(10.0))
                        .px(px(16.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(17.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .text_center()
                                .child(if model.name.is_empty() {
                                    tr_str(lang, "未选择对话", "No chat selected").to_string()
                                } else {
                                    model.name.clone()
                                }),
                        )
                        .when(is_verified, |row| {
                            row.child(tg_icon(IconName::Verified, 15.0, pal.accent))
                        }),
                )
                .when(!status_label.is_empty(), |col| {
                    col.child(
                        div()
                            .pt(px(2.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(5.0))
                            .when(show_green_dot, |row| {
                                row.child(
                                    div()
                                        .w(px(8.0))
                                        .h(px(8.0))
                                        .rounded_full()
                                        .bg(rgb(pal.online_dot_fg)),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(status_label),
                            ),
                    )
                })
                .when(self.avatar_editor_open && is_owner_fabushi, |col| {
                    col.child(render_bot_avatar_editor_popover(self, pal, lang, cx))
                });

            let track_bg = TdesktopThemePalette::rgba_with_alpha(
                if is_dark { 0xFFFFFF } else { 0x000000 },
                if is_dark { 0.06 } else { 0.045 },
            );
            let mut seg_track = div()
                .w_full()
                .h(px(34.0))
                .p(px(3.0))
                .rounded_full()
                .bg(track_bg)
                .flex()
                .items_center()
                .gap(px(2.0));

            if self.profile.is_bot {
                seg_track = seg_track
                    .child(Self::icon_tab_btn("inspector-tab-profile", IconName::FolderAll, InspectorTab::Profile, active_tab, pal, cx))
                    .child(Self::icon_tab_btn("inspector-tab-agent", IconName::BotRobot, InspectorTab::Members, active_tab, pal, cx))
                    .child(Self::icon_tab_btn("inspector-tab-media", IconName::Camera, InspectorTab::SharedMedia, active_tab, pal, cx))
                    .child(Self::icon_tab_btn("inspector-tab-settings", IconName::SettingsGear, InspectorTab::Settings, active_tab, pal, cx));
            } else {
                seg_track = seg_track.child(Self::icon_tab_btn(
                    "inspector-tab-profile",
                    IconName::ContactsUsers,
                    InspectorTab::Profile,
                    active_tab,
                    pal,
                    cx,
                ));
                if self.members.is_group_or_channel {
                    seg_track = seg_track.child(Self::icon_tab_btn(
                        "inspector-tab-members",
                        IconName::GroupUsers,
                        InspectorTab::Members,
                        active_tab,
                        pal,
                        cx,
                    ));
                }
                seg_track = seg_track
                    .child(Self::icon_tab_btn(
                        "inspector-tab-media",
                        IconName::Camera,
                        InspectorTab::SharedMedia,
                        active_tab,
                        pal,
                        cx,
                    ))
                    .child(Self::icon_tab_btn(
                        "inspector-tab-settings",
                        IconName::SettingsGear,
                        InspectorTab::Settings,
                        active_tab,
                        pal,
                        cx,
                    ));
            }

            root = root
                .child(hero)
                .child(
                    div()
                        .flex_shrink_0()
                        .px(px(14.0))
                        .pt(px(8.0))
                        .pb(px(10.0))
                        .child(seg_track),
                );
        }

        root.child(
            div()
                .id("inspector-tab-scroll")
                .flex_1()
                .min_h(px(0.0))
                .px(px(12.0))
                .pb(px(18.0))
                .pt(px(4.0))
                .overflow_y_scroll()
                .child(body),
        )
        .into_any_element()
    }
}

impl ContextInspector {
    fn close_icon_btn(pal: TdesktopThemePalette, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("inspector-header-close")
            .occlude()
            .cursor_pointer()
            .w(px(28.0))
            .h(px(28.0))
            .rounded_full()
            .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _e, _w, cx| {
                    cx.stop_propagation();
                    this.bot_config_focus = BotConfigFocusField::None;
                    this.avatar_editor_open = false;
                    this.close_add_member_picker();
                    this.close();
                    cx.notify();
                }),
            )
            .child(tg_icon(IconName::Close, 14.0, pal.icon_fg))
    }

    fn icon_tab_btn(
        id: &'static str,
        icon: IconName,
        tab: InspectorTab,
        active: InspectorTab,
        pal: TdesktopThemePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sel = tab == active;
        div()
            .id(id)
            .cursor_pointer()
            .flex_1()
            .h_full()
            .rounded_full()
            .when(sel, |el| {
                el.bg(segmented_active_bg(pal))
                    .shadow(segmented_pill_shadow(pal))
            })
            .when(!sel, |el| {
                el.hover(move |s| {
                    s.bg(TdesktopThemePalette::rgba_with_alpha(
                        if pal.mode.is_dark() { 0xFFFFFF } else { 0x000000 },
                        0.04,
                    ))
                })
            })
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _e, _w, cx| {
                    this.set_tab(tab);
                    cx.notify();
                }),
            )
            .child(tg_icon(
                icon,
                15.0,
                if sel {
                    pal.text_primary
                } else {
                    pal.text_secondary
                },
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn context_inspector_preserves_legacy_consts_and_resets_openbot_m4_state() {
        assert_eq!(DRAWER_WIDTH, 320.0);
        assert_eq!(CONTEXT_INSPECTOR_WIDTH, 320.0);
        assert_eq!(HEADER_HEIGHT, 54.0);
        assert_eq!(CONTEXT_INSPECTOR_HEADER_HEIGHT, 54.0);
        assert_eq!(AVATAR_SIZE, 80.0);
        assert_eq!(CONTEXT_INSPECTOR_AVATAR_SIZE, 80.0);
        assert_eq!(OPENBOT_SIDE_PANEL_DEFAULT_WIDTH, 360.0);

        let mut insp = ContextInspector::new();
        assert!(!insp.embedded_in_side_panel);
        assert!(insp.openbot_subpage.is_none());

        insp.embedded_in_side_panel = true;
        insp.openbot_subpage = Some(OpenBotSettingsSubpage::Permissions);
        insp.pending_confirm_guard = Some(OpenBotConfirmGuard::WidenPermissionsFull);
        insp.openbot_text_focus = Some(OpenBotInspectorTextField::BotName);
        insp.avatar_editor_open = true;
        insp.channel_picture_editor_open = true;
        assert!(insp.close_openbot_popovers());
        assert!(!insp.avatar_editor_open);
        assert!(!insp.channel_picture_editor_open);
        assert!(insp.pending_confirm_guard.is_none());

        insp.clear_peer_facts();
        assert!(insp.embedded_in_side_panel);
        assert!(insp.openbot_subpage.is_none());
        assert!(insp.openbot_text_focus.is_none());
    }

    #[std::prelude::v1::test]
    fn context_inspector_backup_models_and_channel_memory_caps() {
        let mut insp = ContextInspector::new();
        for idx in 0..10 {
            insp.bot_backup_model_draft = format!("gpt-5-backup-{idx}");
            let added = insp.add_bot_backup_model_from_draft();
            if idx < 8 {
                assert!(added);
            } else {
                assert!(!added);
            }
        }
        assert_eq!(insp.bot_backup_models.len(), 8);

        for idx in 0..35 {
            insp.channel_memory_draft = format!("rule-{idx}");
            let added = insp.add_channel_memory_from_draft();
            if idx < 32 {
                assert!(added);
            } else {
                assert!(!added);
            }
        }
        assert_eq!(insp.channel_memories.len(), 32);

        insp.channel_archived = true;
        insp.channel_draft_name = "Archived Group".to_string();
        assert!(!insp.commit_channel_identity_draft());
    }

    #[std::prelude::v1::test]
    fn commit_channel_identity_draft_reverts_blank_name_and_blocks_over_limit_fields() {
        let mut insp = ContextInspector::new();
        insp.profile.name = "Launch War Room".to_string();
        insp.channel_draft_name = "   ".to_string();
        insp.channel_draft_instructions = "Keep it crisp".to_string();
        assert!(!insp.commit_channel_identity_draft());
        // Blank name blur must revert draft to committed profile.name and never queue a save
        assert_eq!(insp.channel_draft_name, "Launch War Room");
        assert!(insp.pending_openbot_action.is_none());
        assert!(insp.channel_validation_error.is_some());

        // > 80 UTF-16 code units name blocked
        insp.channel_draft_name = "x".repeat(81);
        assert!(!insp.commit_channel_identity_draft());
        assert!(insp.pending_openbot_action.is_none());
        assert!(insp.channel_validation_error.is_some());

        // > 2000 UTF-16 code units instructions blocked
        insp.channel_draft_name = "Launch War Room".to_string();
        insp.channel_draft_instructions = "z".repeat(2001);
        assert!(!insp.commit_channel_identity_draft());
        assert!(insp.pending_openbot_action.is_none());
        assert!(insp.channel_validation_error.is_some());

        // Valid commit succeeds and clears validation error
        insp.channel_draft_instructions = "z".repeat(2000);
        assert!(insp.commit_channel_identity_draft());
        assert!(insp.pending_openbot_action.is_some());
        assert!(insp.channel_validation_error.is_none());
    }
}
