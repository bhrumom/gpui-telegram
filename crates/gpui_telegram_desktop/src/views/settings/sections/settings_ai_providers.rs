//! Pure-Rust GPUI Settings View for Magpie AI Subscriptions, Presets, Routing & Privacy Masking.
//! Backend contract: `crates/gpui_telegram_client/src/net/hermes_client.rs` (`Magpie*Dto`).

use std::collections::{BTreeMap, HashMap};

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_client::net::hermes_client::{
    MagpieImportRequest, MagpieMaskingConfigDto, MagpieMaskingPreviewRequest,
    MagpieMaskingPreviewResponse, MagpiePresetDto, MagpieProbeResponse, MagpieProviderDto,
    MagpieRoutingGroupDto, MagpieRoutingGroupMemberDto, MagpieSaveProviderRequest, MagpieStateDto,
    MagpieSubscriptionDto,
};
use serde::{Deserialize, Serialize};

fn default_sub_usable() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionAccountDto {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub kind: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default = "default_sub_usable")]
    pub usable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

impl Default for SubscriptionAccountDto {
    fn default() -> Self {
        Self {
            id: String::new(),
            account_id: None,
            kind: String::new(),
            label: String::new(),
            plan: None,
            models: Vec::new(),
            active: false,
            usable: true,
            unavailable_code: None,
            unavailable_reason: None,
        }
    }
}

impl SubscriptionAccountDto {
    /// Returns `true` when this subscription has verified usable credentials and may be selected.
    pub fn is_selectable(&self) -> bool {
        self.usable
            && self
                .unavailable_reason
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionActivateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionActivateResponse {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub active_provider_id: String,
    #[serde(default)]
    pub active_model: String,
}

pub fn sub_dto_to_account_dto(s: &MagpieSubscriptionDto) -> SubscriptionAccountDto {
    let mut models: Vec<String> = match s.kind.as_str() {
        "google_vertex" | "google-vertex" => magpie_gateway::default_models_for_subscription_kind(
            magpie_gateway::SubscriptionKind::GoogleVertex,
        )
        .iter()
        .map(|m| (*m).to_string())
        .collect(),
        "codex_chatgpt" | "codex" => magpie_gateway::default_models_for_subscription_kind(
            magpie_gateway::SubscriptionKind::CodexChatGpt,
        )
        .iter()
        .map(|m| (*m).to_string())
        .collect(),
        "claude_code" | "claude" => magpie_gateway::default_models_for_subscription_kind(
            magpie_gateway::SubscriptionKind::ClaudeCode,
        )
        .iter()
        .map(|m| (*m).to_string())
        .collect(),
        "gemini_code_assist" | "gemini" => magpie_gateway::default_models_for_subscription_kind(
            magpie_gateway::SubscriptionKind::GeminiCodeAssist,
        )
        .iter()
        .map(|m| (*m).to_string())
        .collect(),
        "github_copilot" => magpie_gateway::default_models_for_subscription_kind(
            magpie_gateway::SubscriptionKind::GitHubCopilot,
        )
        .iter()
        .map(|m| (*m).to_string())
        .collect(),
        _ => Vec::new(),
    };
    if let Some(ref dm) = s.default_model {
        let trimmed = dm.trim();
        if !trimmed.is_empty() && !models.iter().any(|m| m == trimmed) {
            models.insert(0, trimmed.to_string());
        }
    }
    let usable = s.is_selectable();
    SubscriptionAccountDto {
        id: s.id.clone(),
        account_id: s.account_id.clone().or_else(|| s.email.clone()),
        kind: s.kind.clone(),
        label: s.label.clone(),
        plan: s.plan.clone(),
        models,
        active: usable && s.active,
        usable,
        unavailable_code: s.unavailable_code.clone(),
        unavailable_reason: s.unavailable_reason.clone(),
    }
}

use crate::components::openbot_model_picker::{
    canonical_provider_icon_key, format_model_display_label, friendly_provider_name,
    render_chevrons_up_down, render_provider_icon,
};
use crate::components::ui::{
    TextInputController, delete_prev_word, pop_last_char, sanitize_paste_text,
};
use crate::i18n::tr_str;
use crate::theme::assets::IconName;
use crate::theme::palette::TdesktopThemePalette;
use crate::views::app_shell::provider_reauth::{
    classify_provider_recovery, UnavailableBadgeTone,
};
use crate::views::computer_workbench::render_ime_text_preview;
use crate::views::settings::settings_theme_ui::{
    SETTINGS_CARD_BG_DARK, SETTINGS_CARD_BG_LIGHT, SETTINGS_CARD_RADIUS_PX, settings_card,
    settings_container, settings_empty_state, settings_subsection_title, settings_toggle_row,
};
use crate::views::stage::openbot_stage_chrome::{openbot_stage_icon, OpenBotStageIcon};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AiProvidersTab {
    #[default]
    Overview,
    Routing,
    PrivacyImport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AiPresetCategory {
    #[default]
    All,
    Vendor,
    Relay,
    Local,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AiProviderField {
    #[default]
    None,
    PresetSearch,
    ProviderId,
    DisplayName,
    BaseUrl,
    ApiKey,
    Model,
    GroupId,
    GroupName,
    GroupTargets,
    GroupFastModel,
    GroupDeepModel,
    MaskCustomWords,
    MaskAllowList,
    MaskPreviewText,
    ImportText,
}

#[derive(Debug, Clone)]
pub enum SettingsAiProvidersEvent {
    RefreshState,
    ScanSubscriptions,
    FetchSubscriptionModels(String),
    ActivateSubscription {
        subscription_id: String,
        model: Option<String>,
    },
    /// Launches interactive browser OAuth PKCE or managed CLI re-login and auto-activates upon completion.
    ReauthenticateProvider {
        provider_id: String,
        kind_hint: Option<String>,
    },
    /// Cancels any currently running interactive browser/CLI re-authentication flow.
    CancelReauthenticate,
    /// Force-refreshes credentials/connectivity for a provider or subscription and re-activates when usable.
    RefreshAndActivateProvider {
        provider_id: String,
        model: Option<String>,
    },
    /// Opens an `http://` or `https://` URL (e.g. API key console) in the system browser.
    OpenExternalUrl(String),
    SaveProvider(MagpieSaveProviderRequest),
    DeleteProvider(String),
    ProbeProvider {
        provider_id: String,
        model: Option<String>,
    },
    ToggleAllowLocalFallback(bool),
    SetRoutingStrategy(String),
    UpsertRoutingGroup(MagpieRoutingGroupDto),
    DeleteRoutingGroup(String),
    UpdateMasking(MagpieMaskingConfigDto),
    PreviewMasking(MagpieMaskingPreviewRequest),
    ImportConfig(MagpieImportRequest),
    CopyText(String),
}

#[derive(Debug, Clone)]
struct ConnectedProviderEntry {
    id: String,
    kind: String,
    icon_key: String,
    display_name: String,
    subtitle: String,
    is_subscription: bool,
    is_active: bool,
    active_model: Option<String>,
    model_count: usize,
    usable: bool,
    unavailable_code: Option<String>,
    unavailable_reason: Option<String>,
}

pub struct SettingsAiProvidersSections {
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub tab: AiProvidersTab,
    pub category: AiPresetCategory,
    pub state: Option<MagpieStateDto>,
    pub subscription_accounts: Vec<SubscriptionAccountDto>,
    pub local_discovered_subscriptions: Vec<SubscriptionAccountDto>,
    pub selected_subscription_models: HashMap<String, String>,
    pub loading: bool,
    pub status_message: Option<String>,
    pub error_message: Option<String>,
    pub probe_result: Option<MagpieProbeResponse>,
    pub masking_preview_result: Option<MagpieMaskingPreviewResponse>,
    /// Currently running interactive browser/CLI re-login provider ID (e.g., `codex:...`, `google_vertex:...`).
    pub reauth_provider_id: Option<String>,
    /// Live status banner text for an active interactive browser/CLI re-login session.
    pub reauth_status_message: Option<String>,
    /// Provider/subscription IDs currently executing a force-refresh & re-activate request.
    pub refreshing_provider_ids: std::collections::HashSet<String>,

    // OpenBot Providers list & drill-down UI state
    pub default_provider_dropdown_open: bool,
    pub selected_subscription_detail_id: Option<String>,
    pub editing_existing_provider_id: Option<String>,

    // Provider preset / custom editor state
    pub editor_open: bool,
    pub selected_preset_id: Option<String>,
    pub selected_region_id: Option<String>,
    pub draft_provider_id: String,
    pub draft_name: String,
    pub draft_api_mode: String,
    pub draft_base_url: String,
    pub draft_api_key: String,
    pub draft_model: String,
    pub draft_no_key: bool,
    pub draft_editing_has_key: bool,
    pub draft_allow_local_fallback: bool,
    pub preset_search: String,

    // Routing group form drafts
    pub group_id_draft: String,
    pub group_name_draft: String,
    pub group_strategy_draft: String,
    pub group_targets_draft: String,
    pub group_fast_model_draft: String,
    pub group_deep_model_draft: String,

    // Masking & Import drafts
    pub masking_draft: MagpieMaskingConfigDto,
    pub mask_custom_words_draft: String,
    pub mask_allow_list_draft: String,
    pub mask_preview_draft: String,
    pub import_raw_draft: String,
    pub import_activate: bool,

    pub active_field: AiProviderField,
    pub editor_session_gen: u64,
    pub ime_controller: Option<Entity<TextInputController>>,
    pub pending_event: Option<SettingsAiProvidersEvent>,
}

impl SettingsAiProvidersSections {
    /// Creates a new live AI Providers settings section view initialized with `palette` and `lang`.
    pub fn new_live(palette: TdesktopThemePalette, lang: &str) -> Self {
        Self {
            palette,
            language_id: lang.to_string(),
            tab: AiProvidersTab::Overview,
            category: AiPresetCategory::All,
            state: None,
            subscription_accounts: Vec::new(),
            local_discovered_subscriptions: Vec::new(),
            selected_subscription_models: HashMap::new(),
            loading: true,
            status_message: None,
            error_message: None,
            probe_result: None,
            masking_preview_result: None,
            reauth_provider_id: None,
            reauth_status_message: None,
            refreshing_provider_ids: std::collections::HashSet::new(),
            default_provider_dropdown_open: false,
            selected_subscription_detail_id: None,
            editing_existing_provider_id: None,
            editor_open: false,
            selected_preset_id: None,
            selected_region_id: None,
            draft_provider_id: String::new(),
            draft_name: String::new(),
            draft_api_mode: "chat_completions".to_string(),
            draft_base_url: String::new(),
            draft_api_key: String::new(),
            draft_model: String::new(),
            draft_no_key: false,
            draft_editing_has_key: false,
            draft_allow_local_fallback: false,
            preset_search: String::new(),
            group_id_draft: String::new(),
            group_name_draft: String::new(),
            group_strategy_draft: "smart".to_string(),
            group_targets_draft: String::new(),
            group_fast_model_draft: String::new(),
            group_deep_model_draft: String::new(),
            masking_draft: MagpieMaskingConfigDto::default(),
            mask_custom_words_draft: String::new(),
            mask_allow_list_draft: String::new(),
            mask_preview_draft: String::new(),
            import_raw_draft: String::new(),
            import_activate: true,
            active_field: AiProviderField::None,
            editor_session_gen: 0,
            ime_controller: None,
            pending_event: None,
        }
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.palette = palette;
    }

    /// Clears all user-scoped Magpie provider/subscription state and form drafts on logout or account switch.
    pub fn reset_on_logout(&mut self) {
        let palette = self.palette;
        let lang = self.language_id.clone();
        let ime_controller = self.ime_controller.take();
        *self = Self::new_live(palette, &lang);
        self.loading = false;
        self.ime_controller = ime_controller;
    }

    pub fn set_language(&mut self, lang: &str) {
        self.language_id = lang.to_string();
    }

    pub fn take_event(&mut self) -> Option<SettingsAiProvidersEvent> {
        self.pending_event.take()
    }

    /// Synchronizes live interactive re-authentication and per-row refresh progress from `AppShell`.
    pub fn sync_recovery_progress(
        &mut self,
        reauth_provider_id: Option<String>,
        reauth_status_message: Option<String>,
        refreshing_ids: std::collections::HashSet<String>,
    ) {
        self.reauth_provider_id = reauth_provider_id;
        self.reauth_status_message = reauth_status_message;
        self.refreshing_provider_ids = refreshing_ids;
    }

    pub fn is_provider_reauthing(&self, provider_id: &str) -> bool {
        self.reauth_provider_id.as_deref() == Some(provider_id)
    }

    pub fn is_provider_refreshing(&self, provider_id: &str) -> bool {
        self.refreshing_provider_ids.contains(provider_id)
    }

    /// Emits `ReauthenticateProvider` for a subscription or OAuth provider.
    pub fn trigger_reauthenticate_provider(&mut self, provider_id: &str, kind_hint: Option<&str>) {
        self.error_message = None;
        self.reauth_provider_id = Some(provider_id.to_string());
        self.pending_event = Some(SettingsAiProvidersEvent::ReauthenticateProvider {
            provider_id: provider_id.to_string(),
            kind_hint: kind_hint.map(str::to_string),
        });
    }

    /// Emits `CancelReauthenticate` to abort any in-flight browser/CLI login flow.
    pub fn trigger_cancel_reauthenticate(&mut self) {
        self.reauth_provider_id = None;
        self.reauth_status_message = None;
        self.pending_event = Some(SettingsAiProvidersEvent::CancelReauthenticate);
    }

    /// Emits `RefreshAndActivateProvider` for a provider or subscription.
    pub fn trigger_refresh_and_activate(&mut self, provider_id: &str, model: Option<String>) {
        self.error_message = None;
        self.refreshing_provider_ids.insert(provider_id.to_string());
        self.pending_event = Some(SettingsAiProvidersEvent::RefreshAndActivateProvider {
            provider_id: provider_id.to_string(),
            model,
        });
    }

    /// Opens the matching subscription detail or provider editor for direct recovery (e.g. from Model Picker or Chat error card).
    pub fn open_provider_for_recovery(&mut self, provider_id: &str) {
        let pid = provider_id.trim();
        if pid.is_empty() {
            self.close_detail_view();
            return;
        }
        if self
            .effective_subscriptions()
            .iter()
            .any(|s| s.id == pid)
        {
            self.open_subscription_detail(pid);
            return;
        }
        if let Some(prov) = self
            .state
            .as_ref()
            .and_then(|st| st.providers.iter().find(|p| p.provider_id == pid))
            .cloned()
        {
            self.open_provider_editor(&prov);
            if !prov.no_key_required {
                self.active_field = AiProviderField::ApiKey;
            }
            return;
        }
        self.close_detail_view();
    }

    /// Selects a target model for the specified local subscription card.
    pub fn select_subscription_model(&mut self, sub_id: &str, model: &str) {
        self.selected_subscription_models
            .insert(sub_id.to_string(), model.to_string());
    }

    /// Dispatches an event to activate the specified local subscription agent as primary,
    /// refusing activation when the subscription is verified unavailable.
    pub fn activate_subscription_action(&mut self, sub_id: &str) {
        if let Some(sub) = self
            .effective_subscriptions()
            .into_iter()
            .find(|s| s.id == sub_id)
        {
            if !sub.is_selectable() {
                self.loading = false;
                let reason = sub.unavailable_reason.unwrap_or_else(|| {
                    tr_str(
                        &self.language_id,
                        "该服务商凭据当前不可用，请点击「重新登录」或「刷新激活」后再试",
                        "This provider is currently unavailable — please re-login or refresh first",
                    )
                    .to_string()
                });
                self.set_status(None, Some(reason));
                return;
            }
        }
        let chosen_model = self.selected_subscription_models.get(sub_id).cloned();
        self.loading = true;
        self.pending_event = Some(SettingsAiProvidersEvent::ActivateSubscription {
            subscription_id: sub_id.to_string(),
            model: chosen_model,
        });
    }

    /// Resolves effective discovered subscription accounts, merging server DTOs with locally
    /// discovered subscriptions (deduplicated by `id`) and preserving verified unavailability
    /// conservatively from either source.
    pub fn effective_subscriptions(&self) -> Vec<SubscriptionAccountDto> {
        let mut list = if !self.subscription_accounts.is_empty() {
            self.subscription_accounts.clone()
        } else if let Some(ref st) = self.state {
            st.subscriptions
                .iter()
                .map(sub_dto_to_account_dto)
                .collect()
        } else {
            Vec::new()
        };

        if !self.local_discovered_subscriptions.is_empty() {
            for local in &self.local_discovered_subscriptions {
                if let Some(existing) = list.iter_mut().find(|s| s.id == local.id) {
                    if !local.is_selectable() {
                        existing.usable = false;
                        if local.unavailable_code.is_some() {
                            existing.unavailable_code = local.unavailable_code.clone();
                        }
                        if local.unavailable_reason.is_some() {
                            existing.unavailable_reason = local.unavailable_reason.clone();
                        }
                        existing.active = false;
                    }
                } else {
                    list.push(local.clone());
                }
            }
        }

        if let Some(ref st) = self.state {
            for sub in &mut list {
                if let Some(st_sub) = st.subscriptions.iter().find(|s| s.id == sub.id) {
                    if !st_sub.is_selectable() {
                        sub.usable = false;
                        if st_sub.unavailable_code.is_some() {
                            sub.unavailable_code = st_sub.unavailable_code.clone();
                        }
                        if st_sub.unavailable_reason.is_some() {
                            sub.unavailable_reason = st_sub.unavailable_reason.clone();
                        }
                    }
                }
                if let Some(st_prov) = st.providers.iter().find(|p| p.provider_id == sub.id) {
                    if !st_prov.is_selectable() {
                        sub.usable = false;
                        if st_prov.unavailable_code.is_some() {
                            sub.unavailable_code = st_prov.unavailable_code.clone();
                        }
                        if st_prov.unavailable_reason.is_some() {
                            sub.unavailable_reason = st_prov.unavailable_reason.clone();
                        }
                    }
                }
                sub.active = sub.is_selectable()
                    && (sub.id == st.active_provider
                        || (sub.kind == "google_vertex"
                            && st.active_provider == "magpie_provider:google-vertex"));
            }
        }
        list
    }

    /// Synchronizes client-side locally discovered subscription records into the view.
    pub fn sync_local_subscriptions(&mut self, local_subs: Vec<SubscriptionAccountDto>) {
        if let Some(ref st) = self.state {
            let mut updated = local_subs;
            for sa in &mut updated {
                sa.active = sa.is_selectable() && sa.id == st.active_provider;
            }
            self.local_discovered_subscriptions = updated;
        } else {
            self.local_discovered_subscriptions = local_subs;
        }
    }

    /// Synchronizes explicit `SubscriptionAccountDto` records fetched from the server.
    pub fn sync_subscriptions(&mut self, subs: Vec<SubscriptionAccountDto>) {
        self.loading = false;
        if let Some(ref st) = self.state {
            let mut updated = subs;
            for sa in &mut updated {
                sa.active = sa.is_selectable() && sa.id == st.active_provider;
            }
            self.subscription_accounts = updated;
        } else {
            self.subscription_accounts = subs;
        }
    }

    /// Hydrates live Magpie backend state into the view while preserving unsaved input drafts.
    pub fn sync_state(&mut self, state: MagpieStateDto) {
        self.loading = false;
        self.draft_allow_local_fallback = state.allow_local_fallback;
        self.masking_draft = state.masking.clone();
        if self.active_field != AiProviderField::MaskCustomWords {
            self.mask_custom_words_draft = state.masking.custom_words.join(", ");
        }
        if self.active_field != AiProviderField::MaskAllowList {
            self.mask_allow_list_draft = state.masking.allow_list.join(", ");
        }
        if self.group_strategy_draft.trim().is_empty() && !state.routing_strategy.is_empty() {
            self.group_strategy_draft = state.routing_strategy.clone();
        }

        // Reconcile subscription_accounts against state.subscriptions:
        // retain existing server-fetched DTOs still present, prune removed IDs, append newly discovered ones.
        let state_ids: std::collections::HashSet<&str> = state
            .subscriptions
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        self.subscription_accounts
            .retain(|sa| state_ids.contains(sa.id.as_str()));

        for sub in &state.subscriptions {
            if let Some(existing) = self
                .subscription_accounts
                .iter_mut()
                .find(|sa| sa.id == sub.id)
            {
                existing.usable = sub.is_selectable();
                existing.unavailable_code = sub.unavailable_code.clone();
                existing.unavailable_reason = sub.unavailable_reason.clone();
            } else {
                self.subscription_accounts
                    .push(sub_dto_to_account_dto(sub));
            }
        }

        // Always synchronize active flag and selected model according to current active provider
        for sa in &mut self.subscription_accounts {
            if let Some(st_prov) = state.providers.iter().find(|p| p.provider_id == sa.id) {
                if !st_prov.is_selectable() {
                    sa.usable = false;
                    if st_prov.unavailable_code.is_some() {
                        sa.unavailable_code = st_prov.unavailable_code.clone();
                    }
                    if st_prov.unavailable_reason.is_some() {
                        sa.unavailable_reason = st_prov.unavailable_reason.clone();
                    }
                }
            }
            sa.active = sa.is_selectable()
                && (sa.id == state.active_provider
                    || (sa.kind == "google_vertex"
                        && state.active_provider == "magpie_provider:google-vertex"));
            if sa.active && !state.active_model.trim().is_empty() {
                self.selected_subscription_models
                    .insert(sa.id.clone(), state.active_model.trim().to_string());
            }
        }
        for sa in &mut self.local_discovered_subscriptions {
            sa.active = sa.is_selectable()
                && (sa.id == state.active_provider
                    || (sa.kind == "google_vertex"
                        && state.active_provider == "magpie_provider:google-vertex"));
            if sa.active && !state.active_model.trim().is_empty() {
                self.selected_subscription_models
                    .insert(sa.id.clone(), state.active_model.trim().to_string());
            }
        }

        self.state = Some(state);
    }

    pub fn set_status(&mut self, status: Option<String>, error: Option<String>) {
        self.loading = false;
        self.status_message = status;
        self.error_message = error;
    }

    fn current_preset(&self) -> Option<&MagpiePresetDto> {
        let pid = self.selected_preset_id.as_deref()?;
        self.state
            .as_ref()
            .and_then(|st| st.presets.iter().find(|p| p.id == pid))
    }

    fn resolve_preset_url(
        preset: &MagpiePresetDto,
        region_id: Option<&str>,
        proto: &str,
    ) -> (Option<String>, Vec<String>) {
        let region = region_id
            .and_then(|rid| preset.regions.iter().find(|r| r.id == rid))
            .or_else(|| preset.regions.first());
        let pick_url = |chat: &Option<String>,
                        resp: &Option<String>,
                        anth: &Option<String>,
                        gem: &Option<String>| {
            match proto {
                "codex_responses" => resp.clone().or_else(|| chat.clone()),
                "anthropic_messages" => anth.clone().or_else(|| chat.clone()),
                "gemini_generate_content" => gem.clone().or_else(|| chat.clone()),
                _ => chat
                    .clone()
                    .or_else(|| resp.clone())
                    .or_else(|| anth.clone())
                    .or_else(|| gem.clone()),
            }
        };
        if let Some(reg) = region {
            let url = pick_url(&reg.chat_url, &reg.responses_url, &reg.anthropic_url, &reg.gemini_url)
                .or_else(|| {
                    pick_url(
                        &preset.chat_url,
                        &preset.responses_url,
                        &preset.anthropic_url,
                        &preset.gemini_url,
                    )
                });
            let models = if reg.models.is_empty() {
                preset.suggested_models.clone()
            } else {
                reg.models.clone()
            };
            (url, models)
        } else {
            let url = pick_url(
                &preset.chat_url,
                &preset.responses_url,
                &preset.anthropic_url,
                &preset.gemini_url,
            );
            (url, preset.suggested_models.clone())
        }
    }

    /// Populates the provider configuration editor from a catalog preset ID.
    pub fn open_preset_editor(&mut self, preset_id: &str) {
        let preset_opt = self
            .state
            .as_ref()
            .and_then(|st| st.presets.iter().find(|p| p.id == preset_id))
            .cloned();
        self.tab = AiProvidersTab::Overview;
        self.default_provider_dropdown_open = false;
        self.selected_subscription_detail_id = None;
        self.editing_existing_provider_id = None;
        self.editor_open = true;
        self.editor_session_gen = self.editor_session_gen.wrapping_add(1);
        self.error_message = None;
        self.probe_result = None;
        self.draft_api_key.clear();

        if let Some(preset) = preset_opt {
            let reg_id = preset.regions.first().map(|r| r.id.clone());
            let proto = if preset.default_protocol.trim().is_empty() {
                "chat_completions".to_string()
            } else {
                preset.default_protocol.clone()
            };
            let (url, models) = Self::resolve_preset_url(&preset, reg_id.as_deref(), &proto);
            self.selected_preset_id = Some(preset.id.clone());
            self.selected_region_id = reg_id;
            self.draft_provider_id = preset.id.clone();
            self.draft_name = preset.name.clone();
            self.draft_api_mode = proto;
            self.draft_base_url = url.unwrap_or_default();
            self.draft_model = if !preset.default_model.trim().is_empty() {
                preset.default_model.clone()
            } else {
                models.first().cloned().unwrap_or_default()
            };
            self.draft_no_key = preset.no_key;
            self.draft_editing_has_key = false;
            self.active_field = if preset.no_key {
                AiProviderField::Model
            } else {
                AiProviderField::ApiKey
            };
        } else {
            self.selected_preset_id = None;
            self.selected_region_id = None;
            self.draft_provider_id = "custom".to_string();
            self.draft_name = "Custom Provider".to_string();
            self.draft_api_mode = "chat_completions".to_string();
            self.draft_base_url.clear();
            self.draft_model.clear();
            self.draft_no_key = false;
            self.draft_editing_has_key = false;
            self.active_field = AiProviderField::BaseUrl;
        }
    }

    /// Opens an OpenBot "其他服务商" row: prefers a matching Magpie preset, otherwise pre-fills
    /// a clean custom provider editor with the selected provider's ID & display name.
    pub fn open_other_provider_editor(
        &mut self,
        provider_key: &str,
        display_name: &str,
        preset_id_hint: Option<&str>,
    ) {
        if let Some(pid) = preset_id_hint {
            let has_preset = self
                .state
                .as_ref()
                .is_some_and(|st| st.presets.iter().any(|p| p.id == pid));
            if has_preset {
                self.open_preset_editor(pid);
                return;
            }
        }
        self.open_preset_editor(provider_key);
        if self.selected_preset_id.is_none() {
            self.draft_provider_id = provider_key.to_string();
            self.draft_name = display_name.to_string();
        }
    }

    /// Opens the drill-down detail view for a connected local/server subscription.
    pub fn open_subscription_detail(&mut self, sub_id: &str) {
        self.tab = AiProvidersTab::Overview;
        self.default_provider_dropdown_open = false;
        self.editor_open = false;
        self.editing_existing_provider_id = None;
        self.selected_subscription_detail_id = Some(sub_id.to_string());
        self.active_field = AiProviderField::None;
    }

    /// Returns from any provider detail or editor sub-view back to the main OpenBot Providers list.
    pub fn close_detail_view(&mut self) {
        self.editor_open = false;
        self.editing_existing_provider_id = None;
        self.selected_subscription_detail_id = None;
        self.default_provider_dropdown_open = false;
        self.active_field = AiProviderField::None;
    }

    /// Progressive Escape handler for OpenBot Providers: closes dropdown -> exits detail/editor -> clears search.
    pub fn handle_escape_key(&mut self) -> bool {
        if self.default_provider_dropdown_open {
            self.default_provider_dropdown_open = false;
            return true;
        }
        if self.selected_subscription_detail_id.is_some() || self.editor_open {
            self.close_detail_view();
            return true;
        }
        if !self.preset_search.is_empty() {
            self.preset_search.clear();
            return true;
        }
        if self.active_field != AiProviderField::None {
            self.active_field = AiProviderField::None;
            return true;
        }
        false
    }

    /// Populates the provider configuration editor from an existing saved provider.
    pub fn open_provider_editor(&mut self, prov: &MagpieProviderDto) {
        self.tab = AiProvidersTab::Overview;
        self.default_provider_dropdown_open = false;
        self.selected_subscription_detail_id = None;
        self.editing_existing_provider_id = Some(prov.provider_id.clone());
        self.editor_open = true;
        self.editor_session_gen = self.editor_session_gen.wrapping_add(1);
        self.error_message = None;
        self.probe_result = None;
        self.selected_preset_id = prov.preset_id.clone();
        self.selected_region_id = prov.region_id.clone();
        self.draft_provider_id = prov.provider_id.clone();
        self.draft_name = prov.name.clone();
        self.draft_api_mode = if prov.api_mode.trim().is_empty() {
            "chat_completions".to_string()
        } else {
            prov.api_mode.clone()
        };
        self.draft_base_url = prov.base_url.clone().unwrap_or_default();
        self.draft_api_key.clear();
        self.draft_model = prov.default_model.clone();
        self.draft_no_key = prov.no_key_required;
        self.draft_editing_has_key = prov.has_api_key;
        self.active_field = if prov.no_key_required {
            AiProviderField::Model
        } else {
            AiProviderField::ApiKey
        };
    }

    pub fn select_region(&mut self, region_id: &str) {
        self.selected_region_id = Some(region_id.to_string());
        if let Some(preset) = self.current_preset().cloned() {
            let proto = self.draft_api_mode.clone();
            let (url, models) = Self::resolve_preset_url(&preset, Some(region_id), &proto);
            if let Some(u) = url {
                self.draft_base_url = u;
            }
            if !models.is_empty() && !models.contains(&self.draft_model) {
                self.draft_model = models[0].clone();
            }
        }
    }

    pub fn select_protocol(&mut self, proto: &str) {
        self.draft_api_mode = proto.to_string();
        if let Some(preset) = self.current_preset().cloned() {
            let rid = self.selected_region_id.clone();
            let (url, _) = Self::resolve_preset_url(&preset, rid.as_deref(), proto);
            if let Some(u) = url {
                self.draft_base_url = u;
            }
        }
    }

    /// Validates and emits `SettingsAiProvidersEvent::SaveProvider` for the active form.
    pub fn submit_provider_form(&mut self) -> bool {
        let pid = self.draft_provider_id.trim().to_string();
        if pid.is_empty() {
            self.error_message = Some(
                tr_str(
                    &self.language_id,
                    "请填写供应商 ID (例如 deepseek / openai / ollama)",
                    "Please enter a Provider ID (e.g. deepseek / openai / ollama)",
                )
                .to_string(),
            );
            return false;
        }
        let key_trimmed = self.draft_api_key.trim().to_string();
        if !self.draft_no_key
            && self.draft_api_mode != "local_hermes"
            && key_trimmed.is_empty()
            && !self.draft_editing_has_key
        {
            self.error_message = Some(
                tr_str(
                    &self.language_id,
                    "请输入该供应商的 API Key（或选择无需 Key 的本地 Ollama）",
                    "Please enter an API Key for this provider (or choose keyless local Ollama)",
                )
                .to_string(),
            );
            return false;
        }
        self.error_message = None;
        self.loading = true;
        self.pending_event = Some(SettingsAiProvidersEvent::SaveProvider(
            MagpieSaveProviderRequest {
                preset_id: self.selected_preset_id.clone(),
                provider_id: Some(pid),
                name: opt_non_empty(&self.draft_name),
                region_id: self.selected_region_id.clone(),
                api_key: if key_trimmed.is_empty() {
                    None
                } else {
                    Some(key_trimmed)
                },
                api_mode: opt_non_empty(&self.draft_api_mode),
                base_url: opt_non_empty(&self.draft_base_url),
                default_model: opt_non_empty(&self.draft_model),
                custom_headers: HashMap::new(),
                enabled: Some(true),
                no_key_required: Some(self.draft_no_key),
                activate: true,
                allow_local_fallback: None,
            },
        ));
        true
    }

    /// Parses group drafts into `MagpieRoutingGroupDto` and emits `UpsertRoutingGroup`.
    pub fn submit_group_form(&mut self) -> bool {
        let id = self.group_id_draft.trim().to_string();
        if id.is_empty() {
            self.error_message = Some(
                tr_str(
                    &self.language_id,
                    "请填写路由组 ID (如 coding / daily)",
                    "Please enter a group ID (e.g. coding / daily)",
                )
                .to_string(),
            );
            return false;
        }
        let members: Vec<MagpieRoutingGroupMemberDto> = self
            .group_targets_draft
            .split([',', '\n'])
            .filter_map(|raw| {
                let token = raw.trim();
                if token.is_empty() {
                    return None;
                }
                let (target, weight) = match token.split_once('@') {
                    Some((t, w)) => (t.trim().to_string(), w.trim().parse::<u32>().unwrap_or(1).max(1)),
                    None => (token.to_string(), 1),
                };
                if target.is_empty() {
                    None
                } else {
                    Some(MagpieRoutingGroupMemberDto {
                        target,
                        weight,
                        enabled: true,
                        input_cost_per_1m: None,
                        output_cost_per_1m: None,
                        free: false,
                    })
                }
            })
            .collect();
        if members.is_empty() {
            self.error_message = Some(
                tr_str(
                    &self.language_id,
                    "请至少填写一个目标模型 (如 deepseek/deepseek-chat, openai/gpt-4o-mini)",
                    "Enter at least one target (e.g. deepseek/deepseek-chat, openai/gpt-4o-mini)",
                )
                .to_string(),
            );
            return false;
        }
        self.error_message = None;
        let name = if self.group_name_draft.trim().is_empty() {
            id.clone()
        } else {
            self.group_name_draft.trim().to_string()
        };
        let strategy = if self.group_strategy_draft.trim().is_empty() {
            "smart".to_string()
        } else {
            self.group_strategy_draft.trim().to_string()
        };
        self.pending_event = Some(SettingsAiProvidersEvent::UpsertRoutingGroup(
            MagpieRoutingGroupDto {
                id,
                name,
                strategy,
                members,
                intent_fast_model: opt_non_empty(&self.group_fast_model_draft),
                intent_deep_model: opt_non_empty(&self.group_deep_model_draft),
                sticky_ttl_ms: 300_000,
            },
        ));
        true
    }

    pub fn submit_masking_form(&mut self) {
        let mut cfg = self.masking_draft.clone();
        cfg.custom_words = split_csv(&self.mask_custom_words_draft);
        cfg.allow_list = split_csv(&self.mask_allow_list_draft);
        if cfg.kind_actions.is_empty() {
            cfg.kind_actions = BTreeMap::new();
        }
        self.masking_draft = cfg.clone();
        self.pending_event = Some(SettingsAiProvidersEvent::UpdateMasking(cfg));
    }

    fn ensure_ime_controller(&mut self, cx: &mut Context<Self>) {
        if self.ime_controller.is_none() {
            self.ime_controller = Some(cx.new(|_| TextInputController::new()));
        }
    }

    pub fn is_input_active(&self) -> bool {
        self.active_field != AiProviderField::None
    }

    pub fn active_field_text(&self) -> &str {
        match self.active_field {
            AiProviderField::None => "",
            AiProviderField::PresetSearch => &self.preset_search,
            AiProviderField::ProviderId => &self.draft_provider_id,
            AiProviderField::DisplayName => &self.draft_name,
            AiProviderField::BaseUrl => &self.draft_base_url,
            AiProviderField::ApiKey => &self.draft_api_key,
            AiProviderField::Model => &self.draft_model,
            AiProviderField::GroupId => &self.group_id_draft,
            AiProviderField::GroupName => &self.group_name_draft,
            AiProviderField::GroupTargets => &self.group_targets_draft,
            AiProviderField::GroupFastModel => &self.group_fast_model_draft,
            AiProviderField::GroupDeepModel => &self.group_deep_model_draft,
            AiProviderField::MaskCustomWords => &self.mask_custom_words_draft,
            AiProviderField::MaskAllowList => &self.mask_allow_list_draft,
            AiProviderField::MaskPreviewText => &self.mask_preview_draft,
            AiProviderField::ImportText => &self.import_raw_draft,
        }
    }

    fn active_field_mut(&mut self) -> Option<&mut String> {
        match self.active_field {
            AiProviderField::None => None,
            AiProviderField::PresetSearch => Some(&mut self.preset_search),
            AiProviderField::ProviderId => Some(&mut self.draft_provider_id),
            AiProviderField::DisplayName => Some(&mut self.draft_name),
            AiProviderField::BaseUrl => Some(&mut self.draft_base_url),
            AiProviderField::ApiKey => Some(&mut self.draft_api_key),
            AiProviderField::Model => Some(&mut self.draft_model),
            AiProviderField::GroupId => Some(&mut self.group_id_draft),
            AiProviderField::GroupName => Some(&mut self.group_name_draft),
            AiProviderField::GroupTargets => Some(&mut self.group_targets_draft),
            AiProviderField::GroupFastModel => Some(&mut self.group_fast_model_draft),
            AiProviderField::GroupDeepModel => Some(&mut self.group_deep_model_draft),
            AiProviderField::MaskCustomWords => Some(&mut self.mask_custom_words_draft),
            AiProviderField::MaskAllowList => Some(&mut self.mask_allow_list_draft),
            AiProviderField::MaskPreviewText => Some(&mut self.mask_preview_draft),
            AiProviderField::ImportText => Some(&mut self.import_raw_draft),
        }
    }

    pub fn active_input_text_and_preedit(&self, cx: &App) -> (String, Option<String>) {
        let committed = self.active_field_text().to_string();
        if let Some(ctrl) = self.ime_controller.as_ref() {
            ctrl.read(cx).text_and_preedit(committed)
        } else {
            (committed, None)
        }
    }

    pub fn clear_active_input(&mut self, cx: &mut Context<Self>) {
        if let Some(ctrl) = self.ime_controller.clone() {
            ctrl.update(cx, |c, _| c.clear_preedit());
        }
        if let Some(dst) = self.active_field_mut() {
            dst.clear();
        }
        cx.notify();
    }

    pub fn push_input_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let allow_nl = matches!(
            self.active_field,
            AiProviderField::GroupTargets | AiProviderField::ImportText | AiProviderField::MaskPreviewText
        );
        let clean = sanitize_paste_text(text, allow_nl);
        if clean.is_empty() {
            return;
        }
        if let Some(ctrl) = self.ime_controller.clone() {
            ctrl.update(cx, |c, _| c.clear_preedit());
        }
        if let Some(dst) = self.active_field_mut() {
            dst.push_str(&clean);
        }
        cx.notify();
    }

    pub fn set_ime_preedit(&mut self, preedit: Option<&str>, cx: &mut Context<Self>) {
        self.ensure_ime_controller(cx);
        if let Some(ctrl) = self.ime_controller.clone() {
            ctrl.update(cx, |c, _| {
                c.ime_preedit = preedit.filter(|s| !s.is_empty()).map(str::to_string);
            });
            cx.notify();
        }
    }

    fn tab_order_for_context(&self) -> &'static [AiProviderField] {
        if self.tab == AiProvidersTab::Overview && self.editor_open {
            &[
                AiProviderField::ProviderId,
                AiProviderField::DisplayName,
                AiProviderField::BaseUrl,
                AiProviderField::Model,
                AiProviderField::ApiKey,
            ]
        } else if self.tab == AiProvidersTab::Routing {
            &[
                AiProviderField::GroupId,
                AiProviderField::GroupName,
                AiProviderField::GroupTargets,
                AiProviderField::GroupFastModel,
                AiProviderField::GroupDeepModel,
            ]
        } else if self.tab == AiProvidersTab::PrivacyImport {
            &[
                AiProviderField::MaskCustomWords,
                AiProviderField::MaskAllowList,
                AiProviderField::MaskPreviewText,
                AiProviderField::ImportText,
            ]
        } else {
            &[AiProviderField::PresetSearch]
        }
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
                let order = self.tab_order_for_context();
                if let Some(pos) = order.iter().position(|f| *f == self.active_field) {
                    let next = if is_shift {
                        (pos + order.len() - 1) % order.len()
                    } else {
                        (pos + 1) % order.len()
                    };
                    self.active_field = order[next];
                    cx.notify();
                }
                true
            }
            "enter" => {
                match self.active_field {
                    AiProviderField::ProviderId
                    | AiProviderField::DisplayName
                    | AiProviderField::BaseUrl
                    | AiProviderField::ApiKey
                    | AiProviderField::Model => {
                        self.submit_provider_form();
                    }
                    AiProviderField::GroupId
                    | AiProviderField::GroupName
                    | AiProviderField::GroupTargets
                    | AiProviderField::GroupFastModel
                    | AiProviderField::GroupDeepModel => {
                        self.submit_group_form();
                    }
                    AiProviderField::MaskPreviewText => {
                        self.pending_event = Some(SettingsAiProvidersEvent::PreviewMasking(
                            MagpieMaskingPreviewRequest {
                                text: self.mask_preview_draft.clone(),
                                config: Some(self.masking_draft.clone()),
                            },
                        ));
                    }
                    AiProviderField::ImportText => {
                        if !self.import_raw_draft.trim().is_empty() {
                            self.pending_event = Some(SettingsAiProvidersEvent::ImportConfig(
                                MagpieImportRequest {
                                    raw: self.import_raw_draft.clone(),
                                    activate: self.import_activate,
                                },
                            ));
                        }
                    }
                    _ => {}
                }
                cx.notify();
                true
            }
            _ => false,
        }
    }

    fn pill_btn(
        id: impl Into<ElementId>,
        label: &str,
        bg_color: u32,
        fg_color: u32,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .cursor_pointer()
            .px(px(10.0))
            .py(px(5.0))
            .rounded(px(7.0))
            .bg(rgb(bg_color))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(fg_color))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    on_click(this, cx);
                    cx.notify();
                }),
            )
            .child(label.to_string())
    }

    fn render_input_field(
        &self,
        id: &'static str,
        field: AiProviderField,
        label: &str,
        value: &str,
        placeholder: &str,
        masked: bool,
        pal: TdesktopThemePalette,
        cx: &mut Context<Self>,
    ) -> Div {
        let ob = pal.openbot_palette();
        let focused = self.active_field == field;
        let preedit = if focused {
            self.ime_controller
                .as_ref()
                .and_then(|c| c.read(cx).ime_preedit.clone())
        } else {
            None
        };
        div()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ob.text_secondary())
                    .child(label.to_string()),
            )
            .child(
                div()
                    .id(id)
                    .cursor_text()
                    .min_h(px(36.0))
                    .px(px(11.0))
                    .py(px(7.0))
                    .rounded(px(8.0))
                    .bg(ob.bg_control())
                    .border_1()
                    .border_color(if focused {
                        ob.text_primary()
                    } else {
                        ob.border_strong_hairline()
                    })
                    .flex()
                    .items_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.ensure_ime_controller(cx);
                            this.active_field = field;
                            cx.notify();
                        }),
                    )
                    .child(render_ime_text_preview(
                        value,
                        preedit.as_deref(),
                        placeholder,
                        focused,
                        masked,
                        pal.text_primary,
                        pal,
                    )),
            )
    }

    fn render_status_banners(&self, pal: TdesktopThemePalette) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .when_some(self.error_message.clone(), |el, err| {
                el.child(
                    div()
                        .px(px(10.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                        .border_1()
                        .border_color(rgb(pal.danger))
                        .text_xs()
                        .text_color(rgb(pal.danger))
                        .child(err),
                )
            })
            .when_some(self.status_message.clone(), |el, msg| {
                el.child(
                    div()
                        .px(px(10.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.14))
                        .border_1()
                        .border_color(rgb(pal.success))
                        .text_xs()
                        .text_color(rgb(pal.success))
                        .child(msg),
                )
            })
            .when_some(self.probe_result.clone(), |el, p| {
                let ok = p.ok;
                let col = if ok { pal.success } else { pal.danger };
                let text = if ok {
                    format!(
                        "Probe OK: {} ({}) · {} ms — {}",
                        p.provider_id,
                        p.model,
                        p.latency_ms,
                        p.reply_preview.unwrap_or_default()
                    )
                } else {
                    format!(
                        "Probe Failed: {} ({}) · {}",
                        p.provider_id,
                        p.model,
                        p.error.unwrap_or_else(|| "error".to_string())
                    )
                };
                el.child(
                    div()
                        .px(px(10.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(col, 0.14))
                        .border_1()
                        .border_color(rgb(col))
                        .text_xs()
                        .text_color(rgb(col))
                        .child(text),
                )
            })
            .into_any_element()
    }

    fn openbot_card_bg(pal: TdesktopThemePalette) -> Rgba {
        let ob = pal.openbot_palette();
        rgb(if ob.scheme.is_dark() {
            SETTINGS_CARD_BG_DARK
        } else {
            SETTINGS_CARD_BG_LIGHT
        })
    }

    /// Resolves all connected providers (local/server subscriptions + configured API/local providers)
    /// for both the `默认服务商` dropdown and the `已连接` grouped list, carrying verified usability
    /// so unusable entries can never be selected as default.
    fn connected_provider_entries(&self, lang: &str) -> Vec<ConnectedProviderEntry> {
        let mut out: Vec<ConnectedProviderEntry> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let subs = self.effective_subscriptions();
        let st_opt = self.state.as_ref();

        for sub in &subs {
            if !seen.insert(sub.id.clone()) {
                continue;
            }
            let icon_key = canonical_provider_icon_key(&sub.kind).to_string();
            let display_name = friendly_provider_name(&sub.kind, &sub.label);
            let usable = sub.is_selectable();
            let email_like = sub
                .account_id
                .as_deref()
                .map(str::trim)
                .filter(|s| s.contains('@'));
            let subtitle = if !usable {
                sub.unavailable_reason
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| {
                        tr_str(
                            lang,
                            "凭据已失效或不可用，请重新登录授权",
                            "Credentials expired or unavailable — re-authenticate",
                        )
                        .to_string()
                    })
            } else if let Some(email) = email_like {
                email.to_string()
            } else {
                tr_str(lang, "在这台电脑上可用", "Available on this computer").to_string()
            };
            let active_model = self
                .selected_subscription_models
                .get(&sub.id)
                .cloned()
                .or_else(|| {
                    if sub.active && usable {
                        st_opt
                            .map(|s| s.active_model.trim().to_string())
                            .filter(|m| !m.is_empty())
                    } else {
                        None
                    }
                })
                .or_else(|| sub.models.first().cloned());
            out.push(ConnectedProviderEntry {
                id: sub.id.clone(),
                kind: sub.kind.clone(),
                icon_key,
                display_name,
                subtitle,
                is_subscription: true,
                is_active: usable && sub.active,
                active_model,
                model_count: sub.models.len(),
                usable,
                unavailable_code: sub.unavailable_code.clone(),
                unavailable_reason: sub.unavailable_reason.clone(),
            });
        }

        if let Some(st) = st_opt {
            for prov in &st.providers {
                if prov.provider_id == "local_hermes"
                    && (!st.allow_local_fallback || !out.is_empty())
                {
                    continue;
                }
                if !seen.insert(prov.provider_id.clone()) {
                    continue;
                }
                let icon_key = canonical_provider_icon_key(
                    prov.preset_id.as_deref().unwrap_or(&prov.provider_id),
                )
                .to_string();
                let display_name = if prov.name.trim().is_empty() {
                    friendly_provider_name(&prov.provider_id, &prov.provider_id)
                } else {
                    prov.name.trim().to_string()
                };
                let usable = prov.is_selectable();
                let key_part = if prov.no_key_required {
                    tr_str(lang, "免 Key · 本地", "No key · Local").to_string()
                } else {
                    prov.masked_key
                        .clone()
                        .unwrap_or_else(|| tr_str(lang, "已配置 API Key", "API Key configured").to_string())
                };
                let subtitle = if !usable {
                    prov.unavailable_reason
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| {
                            tr_str(
                                lang,
                                "服务商当前不可用，请检查 API Key 或网络",
                                "Provider unavailable — check API key or network",
                            )
                            .to_string()
                        })
                } else if prov.default_model.trim().is_empty() {
                    key_part
                } else {
                    format!("{} · {}", prov.default_model.trim(), key_part)
                };
                let is_active = usable
                    && !st.needs_provider_selection
                    && (prov.is_active || prov.provider_id == st.active_provider);
                let model_count = prov
                    .models
                    .len()
                    .max(usize::from(!prov.default_model.trim().is_empty()));
                out.push(ConnectedProviderEntry {
                    id: prov.provider_id.clone(),
                    kind: prov
                        .preset_id
                        .clone()
                        .unwrap_or_else(|| prov.api_mode.clone()),
                    icon_key,
                    display_name,
                    subtitle,
                    is_subscription: prov.is_subscription,
                    is_active,
                    active_model: opt_non_empty(&prov.default_model),
                    model_count,
                    usable,
                    unavailable_code: prov.unavailable_code.clone(),
                    unavailable_reason: prov.unavailable_reason.clone(),
                });
            }
        }
        // Keep verified usable providers ahead of unavailable ones while preserving relative order.
        out.sort_by_key(|e| !e.usable);
        out
    }

    fn render_subscription_detail(
        &self,
        sub_id: &str,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ob = pal.openbot_palette();
        let card_bg = Self::openbot_card_bg(pal);
        let subs = self.effective_subscriptions();
        let Some(sub) = subs.into_iter().find(|s| s.id == sub_id) else {
            return div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .id("magpie-sub-detail-back-missing")
                        .cursor_pointer()
                        .text_size(px(13.0))
                        .text_color(ob.text_muted())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.close_detail_view();
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "← 返回服务商", "← Back to Providers")),
                )
                .into_any_element();
        };

        let icon_key = canonical_provider_icon_key(&sub.kind);
        let display_name = friendly_provider_name(&sub.kind, &sub.label);
        let is_usable = sub.is_selectable();
        let unavail_reason = sub.unavailable_reason.clone().unwrap_or_else(|| {
            tr_str(
                lang,
                "该订阅凭据已过期或认证失败，当前无法使用。重新登录后点击「全部更新」即可恢复。",
                "This subscription's credentials expired or failed auth. Re-authenticate and click Update all to restore.",
            )
            .to_string()
        });
        let account_sub = sub
            .account_id
            .clone()
            .or_else(|| sub.plan.clone())
            .unwrap_or_else(|| {
                if is_usable {
                    tr_str(lang, "在这台电脑上可用", "Available on this computer").to_string()
                } else {
                    tr_str(lang, "凭据不可用", "Credentials unavailable").to_string()
                }
            });
        let is_active = is_usable && sub.active;
        let current_chosen = if is_usable {
            self.selected_subscription_models
                .get(&sub.id)
                .cloned()
                .or_else(|| {
                    if is_active {
                        self.state
                            .as_ref()
                            .map(|st| st.active_model.trim().to_string())
                            .filter(|m| !m.is_empty())
                    } else {
                        None
                    }
                })
                .or_else(|| sub.models.first().cloned())
        } else {
            None
        };

        let sub_id_for_act = sub.id.clone();
        let sub_id_for_fetch = sub.id.clone();
        let hover_bg = ob.hover();
        let border_hairline = ob.border_hairline();
        let border_strong = ob.border_strong_hairline();
        let radio_bg = ob.surface_light();
        let radio_fg = ob.text_on_light();
        let radio_border = ob.border_strong();
        let text_pri = ob.text_primary();
        let text_sec = ob.text_secondary();
        let text_muted = ob.text_muted();
        let danger_bg = TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14);
        let danger_border = TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.36);

        let mut models_card = div()
            .w_full()
            .rounded(px(SETTINGS_CARD_RADIUS_PX))
            .bg(card_bg)
            .border_1()
            .border_color(ob.frame_card_border())
            .overflow_hidden()
            .flex()
            .flex_col();

        if sub.models.is_empty() {
            models_card = models_card.child(
                div()
                    .px(px(16.0))
                    .py(px(18.0))
                    .text_size(px(13.0))
                    .text_color(text_muted)
                    .child(tr_str(
                        lang,
                        "尚未加载模型列表，点击上方「获取最新模型」拉取实时目录。",
                        "No models loaded yet. Click Fetch Models above to sync the live catalog.",
                    )),
            );
        } else {
            for (idx, model_name) in sub.models.iter().enumerate() {
                let m_id = model_name.clone();
                let m_click = model_name.clone();
                let sid = sub.id.clone();
                let is_sel = is_usable
                    && current_chosen
                        .as_deref()
                        .is_some_and(|c| c.eq_ignore_ascii_case(&m_id));
                let m_display = format_model_display_label(&m_id);
                let base_row = div()
                    .id(SharedString::from(format!("magpie-sub-{}-m-{}", sid, m_id)))
                    .min_h(px(44.0))
                    .px(px(16.0))
                    .py(px(9.0))
                    .when(idx > 0, |el| el.border_t_1().border_color(border_hairline))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0));
                let row = if is_usable {
                    base_row
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover_bg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                let already_active_model = this
                                    .state
                                    .as_ref()
                                    .is_some_and(|st| st.active_model == m_click);
                                this.select_subscription_model(&sid, &m_click);
                                if is_active && !already_active_model {
                                    this.activate_subscription_action(&sid);
                                }
                                cx.notify();
                            }),
                        )
                } else {
                    base_row.cursor_not_allowed().opacity(0.45)
                };
                models_card = models_card.child(
                    row.child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(if is_sel {
                                div()
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .rounded_full()
                                    .flex_shrink_0()
                                    .bg(radio_bg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(openbot_stage_icon(
                                        OpenBotStageIcon::Check,
                                        10.0,
                                        radio_fg,
                                    ))
                            } else {
                                div()
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .rounded_full()
                                    .flex_shrink_0()
                                    .border_1()
                                    .border_color(radio_border)
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .min_w(px(0.0))
                                    .child(
                                        div()
                                            .text_size(px(13.5))
                                            .font_weight(if is_sel {
                                                FontWeight::MEDIUM
                                            } else {
                                                FontWeight::NORMAL
                                            })
                                            .text_color(if !is_usable {
                                                text_muted
                                            } else if is_sel {
                                                text_pri
                                            } else {
                                                text_sec
                                            })
                                            .child(m_display.clone()),
                                    )
                                    .when(m_display != m_id, |row| {
                                        row.child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(text_muted)
                                                .child(m_id.clone()),
                                        )
                                    }),
                            ),
                    )
                    .when(idx == 0 && is_usable, |row| {
                        row.child(
                            div()
                                .px(px(6.0))
                                .py(px(1.5))
                                .rounded(px(5.0))
                                .bg(ob.bg_control_active())
                                .text_size(px(11.0))
                                .text_color(text_muted)
                                .child(tr_str(lang, "默认", "Default")),
                        )
                    })
                    .when(!is_usable, |row| {
                        row.child(
                            div()
                                .px(px(6.0))
                                .py(px(1.5))
                                .rounded(px(5.0))
                                .bg(danger_bg)
                                .text_size(px(10.5))
                                .text_color(rgb(pal.danger))
                                .child(tr_str(lang, "不可用", "Unavailable")),
                        )
                    }),
                );
            }
        }

        let recovery = classify_provider_recovery(
            &sub.id,
            Some(&sub.kind),
            true,
            sub.unavailable_code.as_deref(),
            sub.unavailable_reason.as_deref(),
        );
        let is_reauthing_this = self.reauth_provider_id.as_deref() == Some(sub.id.as_str());
        let is_refreshing_this = self.refreshing_provider_ids.contains(&sub.id);
        let badge_label = tr_str(lang, recovery.badge_zh, recovery.badge_en);
        let sub_id_reauth = sub.id.clone();
        let sub_kind_reauth = sub.kind.clone();
        let sub_id_reauth_active = sub.id.clone();
        let sub_kind_reauth_active = sub.kind.clone();
        let sub_id_refresh = sub.id.clone();
        let sub_id_refresh_card = sub.id.clone();
        let cli_hint_opt = recovery.cli_command_hint;
        let selected_model_for_refresh = self.selected_subscription_models.get(&sub.id).cloned();
        let selected_model_for_card_refresh = selected_model_for_refresh.clone();
        let amber_bg = TdesktopThemePalette::rgba_with_alpha(0xD97706, 0.16);
        let amber_border = TdesktopThemePalette::rgba_with_alpha(0xF59E0B, 0.36);
        let amber_fg = rgb(if ob.scheme.is_dark() { 0xF5A524 } else { 0xB45309 });
        let tone_bg = match recovery.tone {
            UnavailableBadgeTone::Network => amber_bg,
            _ => danger_bg,
        };
        let tone_border = match recovery.tone {
            UnavailableBadgeTone::Network => amber_border,
            _ => danger_border,
        };
        let tone_fg = match recovery.tone {
            UnavailableBadgeTone::Network => amber_fg,
            _ => rgb(pal.danger),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            // Back bar
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("magpie-sub-detail-back-btn")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .hover(move |s| s.bg(hover_bg))
                            .text_size(px(13.0))
                            .text_color(text_muted)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.close_detail_view();
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "← 服务商", "← Providers")),
                    ),
            )
            // OpenBot Recovery Card when credentials expired or failed verification
            .when(!is_usable, |col| {
                col.child(
                    div()
                        .id("magpie-sub-unavailable-banner")
                        .w_full()
                        .px(px(15.0))
                        .py(px(13.0))
                        .rounded(px(10.0))
                        .bg(tone_bg)
                        .border_1()
                        .border_color(tone_border)
                        .flex()
                        .flex_col()
                        .gap(px(9.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .text_size(px(13.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(tone_fg)
                                                .child(if lang.starts_with("zh") {
                                                    format!("{} · {}", display_name, badge_label)
                                                } else {
                                                    format!("{} · {}", display_name, badge_label)
                                                }),
                                        )
                                        .child(
                                            div()
                                                .px(px(6.0))
                                                .py(px(1.5))
                                                .rounded(px(5.0))
                                                .bg(tone_bg)
                                                .border_1()
                                                .border_color(tone_border)
                                                .text_size(px(10.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(tone_fg)
                                                .child(tr_str(lang, "暂不可选", "Disabled")),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(text_sec)
                                        .child(if is_reauthing_this {
                                            self.reauth_status_message.clone().unwrap_or_else(|| {
                                                tr_str(
                                                    lang,
                                                    "正在等待浏览器完成授权，完成后将自动验证并激活…",
                                                    "Waiting for browser sign-in — will auto-verify and activate once completed…",
                                                )
                                                .to_string()
                                            })
                                        } else {
                                            unavail_reason.clone()
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.0))
                                .when(recovery.supports_browser_relogin && !is_reauthing_this, |row| {
                                    let pid = sub_id_reauth.clone();
                                    let kind = sub_kind_reauth.clone();
                                    row.child(
                                        div()
                                            .id("magpie-sub-detail-reauth-btn")
                                            .cursor_pointer()
                                            .h(px(30.0))
                                            .px(px(12.0))
                                            .rounded(px(7.0))
                                            .bg(radio_bg)
                                            .flex()
                                            .items_center()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(radio_fg)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.trigger_reauthenticate_provider(
                                                        &pid,
                                                        Some(&kind),
                                                    );
                                                    cx.notify();
                                                }),
                                            )
                                            .child(tr_str(
                                                lang,
                                                "重新登录并激活",
                                                "Re-login & Activate",
                                            )),
                                    )
                                })
                                .when(is_reauthing_this, |row| {
                                    row.child(
                                        div()
                                            .id("magpie-sub-detail-cancel-reauth-btn")
                                            .cursor_pointer()
                                            .h(px(30.0))
                                            .px(px(11.0))
                                            .rounded(px(7.0))
                                            .bg(ob.bg_control())
                                            .border_1()
                                            .border_color(border_strong)
                                            .hover(move |s| s.bg(hover_bg))
                                            .flex()
                                            .items_center()
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(text_pri)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.trigger_cancel_reauthenticate();
                                                    cx.notify();
                                                }),
                                            )
                                            .child(tr_str(lang, "取消登录", "Cancel Sign-in")),
                                    )
                                })
                                .child({
                                    let pid = sub_id_refresh.clone();
                                    let mdl = selected_model_for_refresh.clone();
                                    let primary_refresh = !recovery.supports_browser_relogin;
                                    div()
                                        .id("magpie-sub-detail-refresh-activate-btn")
                                        .cursor_pointer()
                                        .h(px(30.0))
                                        .px(px(11.0))
                                        .rounded(px(7.0))
                                        .bg(if primary_refresh { radio_bg } else { ob.bg_control() })
                                        .when(!primary_refresh, |el| {
                                            el.border_1()
                                                .border_color(border_strong)
                                                .hover(move |s| s.bg(hover_bg))
                                        })
                                        .flex()
                                        .items_center()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(if primary_refresh { radio_fg } else { text_pri })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.trigger_refresh_and_activate(&pid, mdl.clone());
                                                cx.notify();
                                            }),
                                        )
                                        .child(if is_refreshing_this {
                                            tr_str(lang, "正在验证激活…", "Verifying…")
                                        } else {
                                            tr_str(lang, "刷新并重新激活", "Refresh & Activate")
                                        })
                                })
                                .when_some(cli_hint_opt, |row, cmd_hint| {
                                    row.child(
                                        div()
                                            .id("magpie-sub-detail-copy-cli-btn")
                                            .cursor_pointer()
                                            .h(px(30.0))
                                            .px(px(10.0))
                                            .rounded(px(7.0))
                                            .bg(ob.bg_control())
                                            .border_1()
                                            .border_color(border_hairline)
                                            .hover(move |s| s.bg(hover_bg))
                                            .flex()
                                            .items_center()
                                            .gap(px(5.0))
                                            .text_size(px(11.5))
                                            .text_color(text_muted)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.pending_event = Some(
                                                        SettingsAiProvidersEvent::CopyText(
                                                            cmd_hint.to_string(),
                                                        ),
                                                    );
                                                    cx.notify();
                                                }),
                                            )
                                            .child(openbot_stage_icon(
                                                OpenBotStageIcon::Copy,
                                                11.5,
                                                text_muted,
                                            ))
                                            .child(cmd_hint),
                                    )
                                }),
                        ),
                )
            })
            // Provider summary card
            .child(
                div()
                    .id(SharedString::from(format!("magpie-sub-card-{}", sub.id)))
                    .w_full()
                    .px(px(16.0))
                    .py(px(14.0))
                    .rounded(px(SETTINGS_CARD_RADIUS_PX))
                    .bg(card_bg)
                    .border_1()
                    .border_color(ob.frame_card_border())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .w(px(36.0))
                                    .h(px(36.0))
                                    .rounded(px(9.0))
                                    .bg(ob.bg_control())
                                    .border_1()
                                    .border_color(border_hairline)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(!is_usable, |el| el.opacity(0.5))
                                    .child(render_provider_icon(icon_key, 20.0, text_pri, true)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .min_w(px(0.0))
                                    .child(
                                        div()
                                            .text_size(px(15.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(text_pri)
                                            .child(display_name),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(text_muted)
                                            .child(format!("{} · {}", account_sub, sub.kind)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .flex_shrink_0()
                            .when(is_usable && recovery.supports_browser_relogin, |row| {
                                let pid = sub_id_reauth_active.clone();
                                let kind = sub_kind_reauth_active.clone();
                                row.child(
                                    div()
                                        .id("magpie-sub-detail-switch-account-btn")
                                        .cursor_pointer()
                                        .h(px(32.0))
                                        .px(px(11.0))
                                        .rounded(px(8.0))
                                        .bg(ob.bg_control())
                                        .border_1()
                                        .border_color(border_strong)
                                        .hover(move |s| s.bg(hover_bg))
                                        .flex()
                                        .items_center()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(text_sec)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.trigger_reauthenticate_provider(
                                                    &pid,
                                                    Some(&kind),
                                                );
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "重新登录", "Re-login")),
                                )
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "fetch-sub-models-{}",
                                        sub_id_for_fetch
                                    )))
                                    .cursor_pointer()
                                    .h(px(32.0))
                                    .px(px(12.0))
                                    .rounded(px(8.0))
                                    .bg(ob.bg_control())
                                    .border_1()
                                    .border_color(border_strong)
                                    .hover(move |s| s.bg(hover_bg))
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(text_pri)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            if is_usable {
                                                this.loading = true;
                                                this.pending_event = Some(
                                                    SettingsAiProvidersEvent::FetchSubscriptionModels(
                                                        sub_id_for_fetch.clone(),
                                                    ),
                                                );
                                            } else {
                                                this.trigger_refresh_and_activate(
                                                    &sub_id_refresh_card,
                                                    selected_model_for_card_refresh.clone(),
                                                );
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child(if is_usable {
                                        tr_str(lang, "获取最新模型", "Fetch Models")
                                    } else if is_refreshing_this {
                                        tr_str(lang, "刷新中…", "Refreshing…")
                                    } else {
                                        tr_str(lang, "刷新检测", "Refresh")
                                    }),
                            )
                            .child(if !is_usable {
                                div()
                                    .id(SharedString::from(format!(
                                        "magpie-sub-unusable-btn-{}",
                                        sub.id
                                    )))
                                    .cursor_not_allowed()
                                    .opacity(0.7)
                                    .h(px(32.0))
                                    .px(px(12.0))
                                    .rounded(px(8.0))
                                    .bg(tone_bg)
                                    .border_1()
                                    .border_color(tone_border)
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(tone_fg)
                                    .child(badge_label)
                            } else if is_active {
                                div()
                                    .id(SharedString::from(format!(
                                        "magpie-sub-active-btn-{}",
                                        sub.id
                                    )))
                                    .h(px(32.0))
                                    .px(px(12.0))
                                    .rounded(px(8.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.14))
                                    .border_1()
                                    .border_color(TdesktopThemePalette::rgba_with_alpha(
                                        pal.success, 0.36,
                                    ))
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.success())
                                    .child(tr_str(lang, "当前默认", "Default"))
                            } else {
                                div()
                                    .id(SharedString::from(format!(
                                        "magpie-activate-sub-btn-{}",
                                        sub_id_for_act
                                    )))
                                    .cursor_pointer()
                                    .h(px(32.0))
                                    .px(px(12.0))
                                    .rounded(px(8.0))
                                    .bg(radio_bg)
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(radio_fg)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.activate_subscription_action(&sub_id_for_act);
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "设为默认服务商", "Set as Default"))
                            }),
                    ),
            )
            // Models list section header
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_sec)
                    .child(if !is_usable {
                        tr_str(
                            lang,
                            "模型列表（凭据失效，暂不可选）",
                            "Models (credentials invalid — unavailable)",
                        )
                        .to_string()
                    } else if lang.starts_with("zh") {
                        format!("可用模型 ({})", sub.models.len())
                    } else {
                        format!("Available Models ({})", sub.models.len())
                    }),
            )
            .child(models_card)
            .into_any_element()
    }

    fn render_overview_tab(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.editor_open {
            return self.render_provider_editor(pal, lang, cx);
        }
        if let Some(ref detail_sub_id) = self.selected_subscription_detail_id {
            return self.render_subscription_detail(detail_sub_id, pal, lang, cx);
        }

        let ob = pal.openbot_palette();
        let card_bg = Self::openbot_card_bg(pal);
        let bg_control = ob.bg_control();
        let bg_control_hover = ob.bg_control_hover();
        let bg_active = ob.bg_control_active();
        let hover_bg = ob.hover();
        let border_hairline = ob.border_hairline();
        let border_strong = ob.border_strong_hairline();
        let frame_border = ob.frame_card_border();
        let text_pri = ob.text_primary();
        let text_sec = ob.text_secondary();
        let text_muted = ob.text_muted();
        let amber_bg = TdesktopThemePalette::rgba_with_alpha(0xD97706, 0.16);
        let amber_border = TdesktopThemePalette::rgba_with_alpha(0xF59E0B, 0.34);
        let amber_fg = rgb(if ob.scheme.is_dark() { 0xF5A524 } else { 0xB45309 });
        let green_bg = TdesktopThemePalette::rgba_with_alpha(pal.success, 0.14);
        let green_border = TdesktopThemePalette::rgba_with_alpha(pal.success, 0.32);
        let danger_bg = TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14);
        let danger_border = TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.34);

        let st_opt = self.state.as_ref();
        let connected = self.connected_provider_entries(lang);
        let active_default = connected
            .iter()
            .find(|e| e.usable && e.is_active)
            .or_else(|| connected.iter().find(|e| e.usable))
            .cloned();
        let presets = st_opt.map(|s| s.presets.clone()).unwrap_or_default();
        let q = self.preset_search.trim().to_ascii_lowercase();

        // Filter connected rows by search query
        let filtered_connected: Vec<ConnectedProviderEntry> = connected
            .iter()
            .filter(|entry| {
                q.is_empty()
                    || entry.id.to_ascii_lowercase().contains(&q)
                    || entry.icon_key.to_ascii_lowercase().contains(&q)
                    || entry.display_name.to_ascii_lowercase().contains(&q)
                    || entry.subtitle.to_ascii_lowercase().contains(&q)
                    || entry
                        .active_model
                        .as_deref()
                        .is_some_and(|m| m.to_ascii_lowercase().contains(&q))
            })
            .cloned()
            .collect();

        // Track connected canonical icon keys & IDs so `其他服务商` stays clean & deduplicated
        let connected_icon_keys: std::collections::HashSet<&str> = connected
            .iter()
            .map(|e| e.icon_key.as_str())
            .collect();
        let connected_ids: std::collections::HashSet<String> = connected
            .iter()
            .map(|e| e.id.to_ascii_lowercase())
            .collect();

        // OpenBot core providers shown at top of `其他服务商` when not yet connected
        const OPENBOT_CORE_OTHERS: [(&str, &str, Option<&str>, &str, &str); 8] = [
            ("grok", "Grok", Some("xai"), "未下载", "Not downloaded"),
            ("gemini", "Gemini", Some("google"), "未下载", "Not downloaded"),
            ("cursor", "Cursor", None, "未下载", "Not downloaded"),
            ("cline", "Cline", None, "未下载", "Not downloaded"),
            ("pi", "Pi", None, "未下载", "Not downloaded"),
            ("codex", "ChatGPT", Some("openai"), "未连接", "Not connected"),
            ("claude", "Claude", Some("anthropic"), "未连接", "Not connected"),
            ("opencode", "OpenCode", None, "未下载", "Not downloaded"),
        ];

        let mut covered_preset_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut core_other_rows: Vec<(String, String, String, String, Option<String>)> = Vec::new();
        if self.category == AiPresetCategory::All {
            for (icon_key, name, preset_hint, sub_zh, sub_en) in OPENBOT_CORE_OTHERS {
                if connected_icon_keys.contains(icon_key) {
                    if let Some(pid) = preset_hint {
                        covered_preset_ids.insert(pid.to_string());
                    }
                    continue;
                }
                let matches_q = q.is_empty()
                    || icon_key.contains(&q)
                    || name.to_ascii_lowercase().contains(&q)
                    || preset_hint.is_some_and(|p| p.contains(&q));
                if !matches_q {
                    continue;
                }
                if let Some(pid) = preset_hint {
                    covered_preset_ids.insert(pid.to_string());
                }
                core_other_rows.push((
                    icon_key.to_string(),
                    icon_key.to_string(),
                    name.to_string(),
                    tr_str(lang, sub_zh, sub_en).to_string(),
                    preset_hint.map(str::to_string),
                ));
            }
        }

        let filtered_presets: Vec<MagpiePresetDto> = presets
            .into_iter()
            .filter(|p| {
                if covered_preset_ids.contains(&p.id) || connected_ids.contains(&p.id.to_ascii_lowercase()) {
                    return false;
                }
                let cat_ok = match self.category {
                    AiPresetCategory::All => true,
                    AiPresetCategory::Vendor => p.kind == "vendor",
                    AiPresetCategory::Relay => p.kind == "relay",
                    AiPresetCategory::Local => p.kind == "local",
                    AiPresetCategory::Custom => p.kind == "remote" || p.kind == "custom",
                };
                if !cat_ok {
                    return false;
                }
                if q.is_empty() {
                    return true;
                }
                p.id.to_ascii_lowercase().contains(&q)
                    || p.name.to_ascii_lowercase().contains(&q)
                    || p.default_model.to_ascii_lowercase().contains(&q)
            })
            .collect();

        // 1. Default Provider Card (`默认服务商`)
        let default_dropdown_list = self.default_provider_dropdown_open.then(|| {
            let mut list = div()
                .id("openbot-default-provider-menu")
                .mt(px(12.0))
                .pt(px(8.0))
                .border_t_1()
                .border_color(border_hairline)
                .flex()
                .flex_col()
                .gap(px(2.0));
            if connected.is_empty() {
                list = list.child(
                    div()
                        .px(px(8.0))
                        .py(px(8.0))
                        .text_size(px(12.5))
                        .text_color(text_muted)
                        .child(tr_str(
                            lang,
                            "尚未连接服务商，请先在下方连接订阅或配置 API Key。",
                            "No connected providers yet. Connect one below first.",
                        )),
                );
            } else {
                for (idx, entry) in connected.iter().enumerate() {
                    let id_click = entry.id.clone();
                    let id_fix = entry.id.clone();
                    let kind_fix = entry.kind.clone();
                    let is_sub_click = entry.is_subscription;
                    let is_act = entry.usable && entry.is_active;
                    let is_usable = entry.usable;
                    let recovery = classify_provider_recovery(
                        &entry.id,
                        Some(&entry.kind),
                        entry.is_subscription,
                        entry.unavailable_code.as_deref(),
                        entry.unavailable_reason.as_deref(),
                    );
                    let badge_txt = tr_str(lang, recovery.badge_zh, recovery.badge_en);
                    let fix_cta = tr_str(lang, recovery.primary_cta_zh, recovery.primary_cta_en);
                    let supports_relogin = recovery.supports_browser_relogin;
                    let prefer_editor = recovery.prefer_open_editor;
                    let tone_bg = match recovery.tone {
                        UnavailableBadgeTone::Network => amber_bg,
                        _ => danger_bg,
                    };
                    let tone_border = match recovery.tone {
                        UnavailableBadgeTone::Network => amber_border,
                        _ => danger_border,
                    };
                    let tone_fg = match recovery.tone {
                        UnavailableBadgeTone::Network => amber_fg,
                        _ => rgb(pal.danger),
                    };
                    let base_row = div()
                        .id(SharedString::from(format!("default-prov-opt-{idx}")))
                        .h(px(38.0))
                        .px(px(10.0))
                        .rounded(px(8.0))
                        .when(is_act, |el| el.bg(bg_active))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0));
                    let row = if is_usable {
                        base_row
                            .cursor_pointer()
                            .hover(move |s| if is_act { s } else { s.bg(hover_bg) })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.default_provider_dropdown_open = false;
                                    if is_sub_click {
                                        this.activate_subscription_action(&id_click);
                                    } else {
                                        this.pending_event =
                                            Some(SettingsAiProvidersEvent::SaveProvider(
                                                MagpieSaveProviderRequest {
                                                    provider_id: Some(id_click.clone()),
                                                    enabled: Some(true),
                                                    activate: true,
                                                    ..Default::default()
                                                },
                                            ));
                                    }
                                    cx.notify();
                                }),
                            )
                    } else {
                        base_row.cursor_not_allowed()
                    };
                    list = list.child(
                        row.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .min_w(px(0.0))
                                .when(!is_usable, |el| el.opacity(0.55))
                                .child(render_provider_icon(&entry.icon_key, 16.0, text_pri, true))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(if is_usable { text_pri } else { text_muted })
                                        .child(entry.display_name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(text_muted)
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(entry.subtitle.clone()),
                                ),
                        )
                        .when(is_act, |row| {
                            row.child(openbot_stage_icon(
                                OpenBotStageIcon::Check,
                                13.0,
                                text_pri,
                            ))
                        })
                        .when(!is_usable, |row| {
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .px(px(6.0))
                                            .py(px(1.5))
                                            .rounded(px(5.0))
                                            .bg(tone_bg)
                                            .border_1()
                                            .border_color(tone_border)
                                            .text_size(px(10.5))
                                            .text_color(tone_fg)
                                            .child(badge_txt),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "default-prov-fix-{idx}"
                                            )))
                                            .cursor_pointer()
                                            .px(px(7.0))
                                            .py(px(2.0))
                                            .rounded(px(5.0))
                                            .bg(bg_control)
                                            .border_1()
                                            .border_color(border_strong)
                                            .hover(move |s| s.bg(bg_control_hover))
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(text_pri)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.default_provider_dropdown_open = false;
                                                    if supports_relogin {
                                                        this.trigger_reauthenticate_provider(
                                                            &id_fix,
                                                            Some(&kind_fix),
                                                        );
                                                    } else if prefer_editor {
                                                        this.open_provider_for_recovery(&id_fix);
                                                    } else {
                                                        this.trigger_refresh_and_activate(
                                                            &id_fix, None,
                                                        );
                                                    }
                                                    cx.notify();
                                                }),
                                            )
                                            .child(fix_cta),
                                    ),
                            )
                        }),
                    );
                }
            }
            list
        });

        let (def_icon, def_name) = match &active_default {
            Some(entry) => (entry.icon_key.as_str(), entry.display_name.clone()),
            None => ("generic", tr_str(lang, "未选择", "None").to_string()),
        };

        let default_provider_card = div()
            .id("openbot-default-provider-card")
            .w_full()
            .px(px(16.0))
            .py(px(14.0))
            .rounded(px(SETTINGS_CARD_RADIUS_PX))
            .bg(card_bg)
            .border_1()
            .border_color(frame_border)
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(text_pri)
                                    .child(tr_str(lang, "默认服务商", "Default provider")),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(text_muted)
                                    .child(tr_str(
                                        lang,
                                        "新助手默认使用该服务商。",
                                        "New agents use this provider by default.",
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id("openbot-default-provider-select")
                            .cursor_pointer()
                            .min_w(px(176.0))
                            .h(px(34.0))
                            .px(px(11.0))
                            .rounded(px(8.0))
                            .bg(bg_control)
                            .border_1()
                            .border_color(if self.default_provider_dropdown_open {
                                text_muted
                            } else {
                                border_strong
                            })
                            .hover(move |s| s.bg(bg_control_hover))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(10.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.default_provider_dropdown_open =
                                        !this.default_provider_dropdown_open;
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.0))
                                    .child(render_provider_icon(def_icon, 15.0, text_pri, true))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(text_pri)
                                            .child(def_name),
                                    ),
                            )
                            .child(render_chevrons_up_down(12.0, text_muted)),
                    ),
            )
            .when_some(default_dropdown_list, |card, menu| card.child(menu));

        // 2. Connected Section (`已连接`) with `全部更新` + `🔍 搜索服务商`
        let search_focused = self.active_field == AiProviderField::PresetSearch;
        let search_preedit = if search_focused {
            self.ime_controller
                .as_ref()
                .and_then(|c| c.read(cx).ime_preedit.clone())
        } else {
            None
        };

        let connected_toolbar = div()
            .w_full()
            .pt(px(6.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_sec)
                    .child(tr_str(lang, "已连接", "Connected")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("magpie-scan-subs-btn")
                            .cursor_pointer()
                            .h(px(30.0))
                            .px(px(11.0))
                            .rounded(px(8.0))
                            .bg(bg_control)
                            .border_1()
                            .border_color(border_strong)
                            .hover(move |s| s.bg(bg_control_hover))
                            .flex()
                            .items_center()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_pri)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.loading = true;
                                    this.pending_event =
                                        Some(SettingsAiProvidersEvent::ScanSubscriptions);
                                    cx.notify();
                                }),
                            )
                            .child(if self.loading {
                                tr_str(lang, "同步中...", "Syncing...")
                            } else {
                                tr_str(lang, "全部更新", "Update all")
                            }),
                    )
                    .child(
                        div()
                            .id("magpie-preset-search-input")
                            .cursor_text()
                            .w(px(196.0))
                            .h(px(30.0))
                            .px(px(10.0))
                            .rounded(px(8.0))
                            .bg(bg_control)
                            .border_1()
                            .border_color(if search_focused {
                                text_muted
                            } else {
                                border_hairline
                            })
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.ensure_ime_controller(cx);
                                    this.active_field = AiProviderField::PresetSearch;
                                    cx.notify();
                                }),
                            )
                            .child(openbot_stage_icon(
                                OpenBotStageIcon::Search,
                                13.0,
                                text_muted,
                            ))
                            .child(div().flex_1().min_w(px(0.0)).overflow_hidden().child(
                                render_ime_text_preview(
                                    &self.preset_search,
                                    search_preedit.as_deref(),
                                    tr_str(lang, "搜索服务商", "Search providers"),
                                    search_focused,
                                    false,
                                    pal.text_primary,
                                    pal,
                                ),
                            )),
                    ),
            );

        let unavailable_entries: Vec<ConnectedProviderEntry> = connected
            .iter()
            .filter(|e| !e.usable)
            .cloned()
            .collect();
        let cta_bg = ob.surface_light();
        let cta_fg = ob.text_on_light();
        let accent_bg = TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14);
        let accent_border = TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.34);

        let active_reauth_banner: Option<AnyElement> =
            self.reauth_provider_id.as_ref().map(|reauth_pid| {
                let target_pid = reauth_pid.clone();
                let status_txt = self.reauth_status_message.clone().unwrap_or_else(|| {
                    tr_str(
                        lang,
                        "已在浏览器中打开登录页，完成授权后将自动刷新并激活服务商…",
                        "Browser sign-in opened — will automatically refresh and activate once authorized…",
                    )
                    .to_string()
                });
                div()
                    .id("openbot-providers-reauth-live-banner")
                    .w_full()
                    .px(px(14.0))
                    .py(px(12.0))
                    .rounded(px(10.0))
                    .bg(accent_bg)
                    .border_1()
                    .border_color(accent_border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(text_pri)
                                    .child(tr_str(
                                        lang,
                                        "正在等待浏览器完成授权…",
                                        "Waiting for browser authentication…",
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(text_sec)
                                    .child(status_txt),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id("openbot-reauth-done-refresh-btn")
                                    .cursor_pointer()
                                    .h(px(28.0))
                                    .px(px(10.0))
                                    .rounded(px(7.0))
                                    .bg(cta_bg)
                                    .flex()
                                    .items_center()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(cta_fg)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.trigger_refresh_and_activate(&target_pid, None);
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "已完成，立即刷新", "Done, Refresh")),
                            )
                            .child(
                                div()
                                    .id("openbot-reauth-cancel-btn")
                                    .cursor_pointer()
                                    .h(px(28.0))
                                    .px(px(9.0))
                                    .rounded(px(7.0))
                                    .bg(bg_control)
                                    .border_1()
                                    .border_color(border_strong)
                                    .hover(move |s| s.bg(bg_control_hover))
                                    .flex()
                                    .items_center()
                                    .text_size(px(11.5))
                                    .text_color(text_sec)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.trigger_cancel_reauthenticate();
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "取消", "Cancel")),
                            ),
                    )
                    .into_any_element()
            });

        let unavailable_alert_banner: Option<AnyElement> =
            if active_reauth_banner.is_none() && !unavailable_entries.is_empty() {
                let first = unavailable_entries[0].clone();
                let count = unavailable_entries.len();
                let first_recovery = classify_provider_recovery(
                    &first.id,
                    Some(&first.kind),
                    first.is_subscription,
                    first.unavailable_code.as_deref(),
                    first.unavailable_reason.as_deref(),
                );
                let title = if count == 1 {
                    let b = tr_str(lang, first_recovery.badge_zh, first_recovery.badge_en);
                    if lang.starts_with("zh") {
                        format!("{} {}", first.display_name, b)
                    } else {
                        format!("{} {}", first.display_name, b)
                    }
                } else if lang.starts_with("zh") {
                    format!("{count} 个服务商已过期或不可用")
                } else {
                    format!("{count} providers expired or unavailable")
                };
                let detail = if count == 1 {
                    first.subtitle.clone()
                } else {
                    let names: Vec<String> = unavailable_entries
                        .iter()
                        .take(3)
                        .map(|e| e.display_name.clone())
                        .collect();
                    if lang.starts_with("zh") {
                        format!("{} · 重新登录或刷新检测后可立即重新激活使用", names.join("、"))
                    } else {
                        format!("{} · Re-login or refresh to re-activate", names.join(", "))
                    }
                };
                let first_id_cta = first.id.clone();
                let first_kind_cta = first.kind.clone();
                let first_id_refresh = first.id.clone();
                let first_supports_relogin = first_recovery.supports_browser_relogin;
                let first_prefer_editor = first_recovery.prefer_open_editor;
                let single_cta_lbl = tr_str(
                    lang,
                    first_recovery.primary_cta_zh,
                    first_recovery.primary_cta_en,
                );
                Some(
                    div()
                        .id("openbot-providers-expired-banner")
                        .w_full()
                        .px(px(14.0))
                        .py(px(11.0))
                        .rounded(px(10.0))
                        .bg(danger_bg)
                        .border_1()
                        .border_color(danger_border)
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.danger))
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(text_sec)
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(detail),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .when(count == 1, |row| {
                                    let pid = first_id_cta.clone();
                                    let kind = first_kind_cta.clone();
                                    let pid_ref = first_id_refresh.clone();
                                    row.child(
                                        div()
                                            .id("openbot-expired-banner-primary-cta")
                                            .cursor_pointer()
                                            .h(px(28.0))
                                            .px(px(10.0))
                                            .rounded(px(7.0))
                                            .bg(cta_bg)
                                            .flex()
                                            .items_center()
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(cta_fg)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    if first_supports_relogin {
                                                        this.trigger_reauthenticate_provider(
                                                            &pid,
                                                            Some(&kind),
                                                        );
                                                    } else if first_prefer_editor {
                                                        this.open_provider_for_recovery(&pid);
                                                    } else {
                                                        this.trigger_refresh_and_activate(
                                                            &pid, None,
                                                        );
                                                    }
                                                    cx.notify();
                                                }),
                                            )
                                            .child(single_cta_lbl),
                                    )
                                    .when(first_supports_relogin || first_prefer_editor, |r| {
                                        r.child(
                                            div()
                                                .id("openbot-expired-banner-refresh-cta")
                                                .cursor_pointer()
                                                .h(px(28.0))
                                                .px(px(9.0))
                                                .rounded(px(7.0))
                                                .bg(bg_control)
                                                .border_1()
                                                .border_color(border_strong)
                                                .hover(move |s| s.bg(bg_control_hover))
                                                .flex()
                                                .items_center()
                                                .text_size(px(11.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(text_pri)
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.trigger_refresh_and_activate(
                                                            &pid_ref, None,
                                                        );
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(tr_str(lang, "刷新激活", "Refresh")),
                                        )
                                    })
                                })
                                .when(count > 1, |row| {
                                    row.child(
                                        div()
                                            .id("openbot-expired-banner-rescan-all")
                                            .cursor_pointer()
                                            .h(px(28.0))
                                            .px(px(10.0))
                                            .rounded(px(7.0))
                                            .bg(cta_bg)
                                            .flex()
                                            .items_center()
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(cta_fg)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.loading = true;
                                                    this.pending_event = Some(
                                                        SettingsAiProvidersEvent::ScanSubscriptions,
                                                    );
                                                    cx.notify();
                                                }),
                                            )
                                            .child(tr_str(lang, "全部刷新检测", "Refresh All")),
                                    )
                                }),
                        )
                        .into_any_element(),
                )
            } else {
                None
            };

        let mut connected_card = div()
            .w_full()
            .rounded(px(SETTINGS_CARD_RADIUS_PX))
            .bg(card_bg)
            .border_1()
            .border_color(frame_border)
            .overflow_hidden()
            .flex()
            .flex_col();

        if filtered_connected.is_empty() {
            connected_card = connected_card.child(
                div()
                    .px(px(16.0))
                    .py(px(16.0))
                    .text_size(px(13.0))
                    .text_color(text_muted)
                    .child(if q.is_empty() {
                        tr_str(
                            lang,
                            "未检测到已连接的服务商。点击「全部更新」扫描本机凭证，或从下方「其他服务商」添加。",
                            "No connected providers yet. Click Update all to scan local credentials, or add one below.",
                        )
                    } else {
                        tr_str(lang, "已连接列表中无匹配服务商", "No matching connected providers")
                    }),
            );
        } else {
            let saved_providers = st_opt.map(|s| s.providers.clone()).unwrap_or_default();
            for (idx, entry) in filtered_connected.into_iter().enumerate() {
                let id_for_click = entry.id.clone();
                let id_for_primary = entry.id.clone();
                let kind_for_primary = entry.kind.clone();
                let id_for_refresh = entry.id.clone();
                let is_sub = entry.is_subscription;
                let is_usable = entry.usable;
                let is_reauthing_row =
                    self.reauth_provider_id.as_deref() == Some(entry.id.as_str());
                let is_refreshing_row = self.refreshing_provider_ids.contains(&entry.id);
                let prov_dto_opt = if !is_sub {
                    saved_providers
                        .iter()
                        .find(|p| p.provider_id == entry.id)
                        .cloned()
                } else {
                    None
                };
                let prov_dto_for_edit = prov_dto_opt.clone();
                let recovery = classify_provider_recovery(
                    &entry.id,
                    Some(&entry.kind),
                    is_sub,
                    entry.unavailable_code.as_deref(),
                    entry.unavailable_reason.as_deref(),
                );
                let row_tone_bg = match recovery.tone {
                    UnavailableBadgeTone::Network => amber_bg,
                    _ => danger_bg,
                };
                let row_tone_border = match recovery.tone {
                    UnavailableBadgeTone::Network => amber_border,
                    _ => danger_border,
                };
                let row_tone_fg = match recovery.tone {
                    UnavailableBadgeTone::Network => amber_fg,
                    _ => rgb(pal.danger),
                };

                let badge_el: Option<AnyElement> = if !is_usable {
                    let badge_txt = tr_str(lang, recovery.badge_zh, recovery.badge_en);
                    let cta_txt = tr_str(lang, recovery.primary_cta_zh, recovery.primary_cta_en);
                    let supports_relogin = recovery.supports_browser_relogin;
                    let prefer_editor = recovery.prefer_open_editor;
                    let show_secondary_refresh =
                        !is_reauthing_row && (supports_relogin || prefer_editor);
                    Some(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .px(px(7.0))
                                    .py(px(2.0))
                                    .rounded(px(6.0))
                                    .bg(row_tone_bg)
                                    .border_1()
                                    .border_color(row_tone_border)
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(row_tone_fg)
                                    .child(badge_txt),
                            )
                            .when(is_reauthing_row, |r| {
                                r.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "magpie-row-cancel-reauth-{}",
                                            entry.id
                                        )))
                                        .cursor_pointer()
                                        .h(px(26.0))
                                        .px(px(8.0))
                                        .rounded(px(6.0))
                                        .bg(bg_control)
                                        .border_1()
                                        .border_color(border_strong)
                                        .hover(move |s| s.bg(bg_control_hover))
                                        .flex()
                                        .items_center()
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(text_pri)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.trigger_cancel_reauthenticate();
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "登录中 · 取消", "Signing in · Cancel")),
                                )
                            })
                            .when(!is_reauthing_row, |r| {
                                let pid = id_for_primary.clone();
                                let kind = kind_for_primary.clone();
                                let dto_edit = prov_dto_for_edit.clone();
                                r.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "magpie-row-recover-btn-{}",
                                            entry.id
                                        )))
                                        .cursor_pointer()
                                        .h(px(26.0))
                                        .px(px(9.0))
                                        .rounded(px(6.0))
                                        .bg(cta_bg)
                                        .flex()
                                        .items_center()
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(cta_fg)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                if supports_relogin {
                                                    this.trigger_reauthenticate_provider(
                                                        &pid,
                                                        Some(&kind),
                                                    );
                                                } else if prefer_editor {
                                                    if let Some(ref dto) = dto_edit {
                                                        this.open_provider_editor(dto);
                                                        if !dto.no_key_required {
                                                            this.active_field =
                                                                AiProviderField::ApiKey;
                                                        }
                                                    } else {
                                                        this.open_provider_for_recovery(&pid);
                                                    }
                                                } else {
                                                    this.trigger_refresh_and_activate(&pid, None);
                                                }
                                                cx.notify();
                                            }),
                                        )
                                        .child(if is_refreshing_row && !supports_relogin && !prefer_editor {
                                            tr_str(lang, "刷新中…", "Refreshing…")
                                        } else {
                                            cta_txt
                                        }),
                                )
                            })
                            .when(show_secondary_refresh, |r| {
                                let pid = id_for_refresh.clone();
                                r.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "magpie-row-refresh-btn-{}",
                                            entry.id
                                        )))
                                        .cursor_pointer()
                                        .h(px(26.0))
                                        .px(px(8.0))
                                        .rounded(px(6.0))
                                        .bg(bg_control)
                                        .border_1()
                                        .border_color(border_strong)
                                        .hover(move |s| s.bg(bg_control_hover))
                                        .flex()
                                        .items_center()
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(text_sec)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.trigger_refresh_and_activate(&pid, None);
                                                cx.notify();
                                            }),
                                        )
                                        .child(if is_refreshing_row {
                                            tr_str(lang, "刷新中…", "Refreshing…")
                                        } else {
                                            tr_str(lang, "刷新", "Refresh")
                                        }),
                                )
                            })
                            .into_any_element(),
                    )
                } else if entry.is_active {
                    let badge_txt = entry
                        .active_model
                        .as_deref()
                        .map(format_model_display_label)
                        .unwrap_or_else(|| tr_str(lang, "当前默认", "Default").to_string());
                    Some(
                        div()
                            .px(px(8.0))
                            .py(px(2.5))
                            .rounded(px(6.0))
                            .bg(green_bg)
                            .border_1()
                            .border_color(green_border)
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.success())
                            .child(badge_txt)
                            .into_any_element(),
                    )
                } else if is_sub && entry.model_count == 0 {
                    Some(
                        div()
                            .px(px(8.0))
                            .py(px(2.5))
                            .rounded(px(6.0))
                            .bg(amber_bg)
                            .border_1()
                            .border_color(amber_border)
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(amber_fg)
                            .child(tr_str(lang, "可获取模型", "Fetch models"))
                            .into_any_element(),
                    )
                } else if entry.model_count > 0 {
                    Some(
                        div()
                            .px(px(8.0))
                            .py(px(2.5))
                            .rounded(px(6.0))
                            .bg(amber_bg)
                            .border_1()
                            .border_color(amber_border)
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(amber_fg)
                            .child(if lang.starts_with("zh") {
                                format!("{} 个模型", entry.model_count)
                            } else {
                                format!("{} models", entry.model_count)
                            })
                            .into_any_element(),
                    )
                } else {
                    None
                };

                connected_card = connected_card.child(
                    div()
                        .id(SharedString::from(format!("magpie-sub-card-{}", entry.id)))
                        .cursor_pointer()
                        .w_full()
                        .min_h(px(58.0))
                        .px(px(16.0))
                        .py(px(11.0))
                        .when(idx > 0, |el| el.border_t_1().border_color(border_hairline))
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                if is_sub {
                                    this.open_subscription_detail(&id_for_click);
                                } else if let Some(ref dto) = prov_dto_opt {
                                    this.open_provider_editor(dto);
                                }
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .w(px(28.0))
                                        .h(px(28.0))
                                        .flex_shrink_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .when(!is_usable, |el| el.opacity(0.45))
                                        .child(render_provider_icon(
                                            &entry.icon_key,
                                            20.0,
                                            text_pri,
                                            true,
                                        )),
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
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(if is_usable {
                                                    text_pri
                                                } else {
                                                    text_sec
                                                })
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(entry.display_name),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.5))
                                                .text_color(if is_usable {
                                                    text_muted
                                                } else {
                                                    row_tone_fg
                                                })
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .child(entry.subtitle),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .when_some(badge_el, |row, b| row.child(b))
                                .child(openbot_stage_icon(
                                    OpenBotStageIcon::ChevronRight,
                                    14.0,
                                    text_muted,
                                )),
                        ),
                );
            }
        }

        // 3. Other Providers Section (`其他服务商`)
        let categories = [
            (AiPresetCategory::All, tr_str(lang, "全部", "All")),
            (AiPresetCategory::Vendor, tr_str(lang, "官方", "Vendors")),
            (AiPresetCategory::Relay, tr_str(lang, "中转", "Relays")),
            (AiPresetCategory::Local, tr_str(lang, "本地", "Local")),
            (AiPresetCategory::Custom, tr_str(lang, "自定义", "Custom")),
        ];

        let others_header = div()
            .w_full()
            .pt(px(6.0))
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_sec)
                    .child(tr_str(lang, "其他服务商", "Other providers")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .children(categories.into_iter().enumerate().map(|(cidx, (cat, lbl))| {
                        let active = self.category == cat;
                        div()
                            .id(SharedString::from(format!("magpie-cat-filter-btn-{cidx}")))
                            .cursor_pointer()
                            .h(px(24.0))
                            .px(px(8.0))
                            .rounded(px(6.0))
                            .when(active, |el| el.bg(bg_active))
                            .hover(move |s| if active { s } else { s.bg(hover_bg) })
                            .flex()
                            .items_center()
                            .text_size(px(11.5))
                            .font_weight(if active {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(if active { text_pri } else { text_muted })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.category = cat;
                                    cx.notify();
                                }),
                            )
                            .child(lbl)
                    }))
                    .child(
                        div()
                            .id("magpie-add-custom-btn")
                            .cursor_pointer()
                            .ml(px(4.0))
                            .h(px(26.0))
                            .px(px(9.0))
                            .rounded(px(7.0))
                            .bg(bg_control)
                            .border_1()
                            .border_color(border_strong)
                            .hover(move |s| s.bg(bg_control_hover))
                            .flex()
                            .items_center()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_pri)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.open_preset_editor("custom-openai");
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "+ 自定义", "+ Custom")),
                    ),
            );

        let mut others_card = div()
            .w_full()
            .rounded(px(SETTINGS_CARD_RADIUS_PX))
            .bg(card_bg)
            .border_1()
            .border_color(frame_border)
            .overflow_hidden()
            .flex()
            .flex_col();

        let mut other_row_idx = 0usize;
        for (prov_key, icon_key, name, subtitle, preset_hint) in core_other_rows {
            let idx = other_row_idx;
            other_row_idx += 1;
            let key_click = prov_key.clone();
            let name_click = name.clone();
            let hint_click = preset_hint.clone();
            others_card = others_card.child(
                div()
                    .id(SharedString::from(format!("openbot-other-prov-{}", prov_key)))
                    .cursor_pointer()
                    .w_full()
                    .min_h(px(56.0))
                    .px(px(16.0))
                    .py(px(10.0))
                    .when(idx > 0, |el| el.border_t_1().border_color(border_hairline))
                    .hover(move |s| s.bg(hover_bg))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.open_other_provider_editor(
                                &key_click,
                                &name_click,
                                hint_click.as_deref(),
                            );
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .w(px(28.0))
                                    .h(px(28.0))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(render_provider_icon(&icon_key, 20.0, text_pri, true)),
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
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(text_pri)
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(text_muted)
                                            .child(subtitle),
                                    ),
                            ),
                    )
                    .child(openbot_stage_icon(
                        OpenBotStageIcon::ChevronRight,
                        14.0,
                        text_muted,
                    )),
            );
        }

        for p in filtered_presets {
            let idx = other_row_idx;
            other_row_idx += 1;
            let pid = p.id.clone();
            let icon_key = canonical_provider_icon_key(&p.id);
            let title = p.short_name.unwrap_or(p.name);
            let subtitle = if p.no_key {
                if p.default_model.trim().is_empty() {
                    tr_str(lang, "免 Key · 本地", "No Key · Local").to_string()
                } else {
                    format!("{} · {}", tr_str(lang, "免 Key", "No Key"), p.default_model)
                }
            } else if p.default_model.trim().is_empty() {
                tr_str(lang, "未配置 · API Key", "Not configured · API Key").to_string()
            } else {
                format!("API Key · {}", p.default_model)
            };
            others_card = others_card.child(
                div()
                    .id(SharedString::from(format!("magpie-preset-card-{}", p.id)))
                    .cursor_pointer()
                    .w_full()
                    .min_h(px(56.0))
                    .px(px(16.0))
                    .py(px(10.0))
                    .when(idx > 0, |el| el.border_t_1().border_color(border_hairline))
                    .hover(move |s| s.bg(hover_bg))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.open_preset_editor(&pid);
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .w(px(28.0))
                                    .h(px(28.0))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(render_provider_icon(icon_key, 20.0, text_pri, true)),
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
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(text_pri)
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(text_muted)
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(subtitle),
                                    ),
                            ),
                    )
                    .child(openbot_stage_icon(
                        OpenBotStageIcon::ChevronRight,
                        14.0,
                        text_muted,
                    )),
            );
        }

        if other_row_idx == 0 {
            others_card = others_card.child(
                div()
                    .px(px(16.0))
                    .py(px(16.0))
                    .text_size(px(13.0))
                    .text_color(text_muted)
                    .child(tr_str(lang, "没有匹配的其他服务商", "No matching providers")),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(default_provider_card)
            .when_some(active_reauth_banner, |col, b| col.child(b))
            .when_some(unavailable_alert_banner, |col, b| col.child(b))
            .child(connected_toolbar)
            .child(connected_card)
            .child(others_header)
            .child(others_card)
            .child(settings_card(pal).child(settings_toggle_row(
                IconName::BotRobot,
                tr_str(
                    lang,
                    "无服务商时允许本地回退 (fabushi-local)",
                    "Allow local fallback when unconfigured",
                ),
                None,
                self.draft_allow_local_fallback,
                pal,
                |this| {
                    this.draft_allow_local_fallback = !this.draft_allow_local_fallback;
                    this.pending_event = Some(SettingsAiProvidersEvent::ToggleAllowLocalFallback(
                        this.draft_allow_local_fallback,
                    ));
                },
                cx,
            )))
            .into_any_element()
    }

    fn render_provider_editor(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ob = pal.openbot_palette();
        let hover_bg = ob.hover();
        let bg_control = ob.bg_control();
        let bg_active = ob.bg_control_active();
        let border_strong = ob.border_strong_hairline();
        let text_pri = ob.text_primary();
        let text_sec = ob.text_secondary();
        let text_muted = ob.text_muted();
        let cta_bg = ob.surface_light();
        let cta_fg = ob.text_on_light();

        let preset_opt = self.current_preset().cloned();
        let regions = preset_opt.as_ref().map(|p| p.regions.clone()).unwrap_or_default();
        let protocols = preset_opt
            .as_ref()
            .filter(|p| !p.supported_protocols.is_empty())
            .map(|p| p.supported_protocols.clone())
            .unwrap_or_else(|| {
                vec![
                    "chat_completions".to_string(),
                    "codex_responses".to_string(),
                    "anthropic_messages".to_string(),
                    "gemini_generate_content".to_string(),
                ]
            });
        let suggested_models = preset_opt
            .as_ref()
            .map(|p| {
                Self::resolve_preset_url(p, self.selected_region_id.as_deref(), &self.draft_api_mode).1
            })
            .unwrap_or_default();
        let keys_url = preset_opt.as_ref().and_then(|p| {
            self.selected_region_id
                .as_deref()
                .and_then(|rid| p.regions.iter().find(|r| r.id == rid))
                .and_then(|r| r.keys_url.clone())
                .or_else(|| p.keys_url.clone())
        });
        let note = preset_opt.as_ref().and_then(|p| p.note.clone());
        let key_hint = preset_opt
            .as_ref()
            .and_then(|p| p.key_hint.clone())
            .unwrap_or_else(|| "sk-...".to_string());
        let icon_key = canonical_provider_icon_key(
            self.selected_preset_id
                .as_deref()
                .unwrap_or(&self.draft_provider_id),
        );
        let existing_pid = self.editing_existing_provider_id.clone();
        let existing_prov_opt = existing_pid.as_deref().and_then(|pid| {
            self.state
                .as_ref()
                .and_then(|st| st.providers.iter().find(|p| p.provider_id == pid))
        });
        let existing_unavailable = existing_prov_opt
            .filter(|p| !p.is_selectable())
            .map(|p| {
                let recovery = classify_provider_recovery(
                    &p.provider_id,
                    p.preset_id.as_deref(),
                    p.is_subscription,
                    p.unavailable_code.as_deref(),
                    p.unavailable_reason.as_deref(),
                );
                let reason = p.unavailable_reason.clone().unwrap_or_else(|| {
                    tr_str(
                        lang,
                        "凭据校验失败，当前不可用，请更新配置或点击刷新激活。",
                        "Credential check failed. Update configuration or refresh to reactivate.",
                    )
                    .to_string()
                });
                (p.provider_id.clone(), recovery, reason)
            });

        // OpenBot Subscription Quick-Login strip for major subscription-backed vendors
        let quick_sub_login: Option<(&'static str, &'static str, &'static str, &'static str, &'static str, &'static str)> =
            match icon_key {
                "codex" => Some((
                    "codex",
                    "codex_chatgpt",
                    "已有 ChatGPT Plus / Pro 订阅？",
                    "Have a ChatGPT Plus / Pro subscription?",
                    "使用浏览器登录 Codex",
                    "Sign in with ChatGPT (Codex)",
                )),
                "gemini" => Some((
                    "google_vertex",
                    "google_vertex",
                    "已有 Google Cloud / Vertex AI 授权？",
                    "Using Google Cloud / Vertex AI ADC?",
                    "浏览器登录 Google Vertex AI",
                    "Sign in to Google Vertex AI",
                )),
                "claude" => Some((
                    "claude_code",
                    "claude_code",
                    "已有 Claude Code 订阅？",
                    "Have a Claude Code subscription?",
                    "登录 Claude Code",
                    "Sign in to Claude Code",
                )),
                _ => None,
            };

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("magpie-editor-back-btn")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .hover(move |s| s.bg(hover_bg))
                            .text_size(px(13.0))
                            .text_color(text_muted)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.close_detail_view();
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "← 服务商", "← Providers")),
                    )
                    .when_some(existing_pid.clone(), |row, pid| {
                        let pid_probe = pid.clone();
                        let pid_del = pid.clone();
                        let model_probe = opt_non_empty(&self.draft_model);
                        row.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .id("magpie-probe-prov-btn")
                                        .cursor_pointer()
                                        .h(px(28.0))
                                        .px(px(10.0))
                                        .rounded(px(7.0))
                                        .bg(bg_control)
                                        .border_1()
                                        .border_color(border_strong)
                                        .hover(move |s| s.bg(hover_bg))
                                        .flex()
                                        .items_center()
                                        .text_size(px(12.0))
                                        .text_color(text_pri)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.pending_event = Some(
                                                    SettingsAiProvidersEvent::ProbeProvider {
                                                        provider_id: pid_probe.clone(),
                                                        model: model_probe.clone(),
                                                    },
                                                );
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "测试连通", "Probe")),
                                )
                                .when(pid != "local_hermes", |el| {
                                    el.child(
                                        div()
                                            .id("magpie-del-prov-btn")
                                            .cursor_pointer()
                                            .h(px(28.0))
                                            .px(px(10.0))
                                            .rounded(px(7.0))
                                            .bg(bg_control)
                                            .border_1()
                                            .border_color(border_strong)
                                            .hover(move |s| s.bg(hover_bg))
                                            .flex()
                                            .items_center()
                                            .text_size(px(12.0))
                                            .text_color(ob.danger())
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.pending_event = Some(
                                                        SettingsAiProvidersEvent::DeleteProvider(
                                                            pid_del.clone(),
                                                        ),
                                                    );
                                                    this.close_detail_view();
                                                    cx.notify();
                                                }),
                                            )
                                            .child(tr_str(lang, "删除", "Delete")),
                                    )
                                }),
                        )
                    }),
            )
            .child(
                settings_card(pal)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .pb(px(4.0))
                            .child(
                                div()
                                    .w(px(32.0))
                                    .h(px(32.0))
                                    .rounded(px(8.0))
                                    .bg(bg_control)
                                    .border_1()
                                    .border_color(ob.border_hairline())
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(render_provider_icon(icon_key, 18.0, text_pri, true)),
                            )
                            .child(
                                div()
                                    .text_size(px(15.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(text_pri)
                                    .child(if self.draft_name.is_empty() {
                                        self.draft_provider_id.clone()
                                    } else {
                                        self.draft_name.clone()
                                    }),
                            ),
                    )
                    .when_some(existing_unavailable, |el, (pid, recovery, reason)| {
                        let is_refreshing = self.is_provider_refreshing(&pid);
                        let is_reauthing = self.is_provider_reauthing(&pid);
                        let pid_refresh = pid.clone();
                        let pid_reauth = pid.clone();
                        let model_refresh = opt_non_empty(&self.draft_model);
                        let kind_hint = recovery.subscription_kind.map(|k| k.as_str().to_string());
                        let (badge_bg, badge_fg, border_col) = match recovery.tone {
                            UnavailableBadgeTone::Expired => (
                                gpui::hsla(0.09, 0.85, 0.50, 0.14),
                                gpui::rgb(0xF59E0B),
                                gpui::hsla(0.09, 0.85, 0.50, 0.40),
                            ),
                            UnavailableBadgeTone::Network => (
                                gpui::hsla(0.58, 0.72, 0.52, 0.14),
                                gpui::rgb(0x38BDF8),
                                gpui::hsla(0.58, 0.72, 0.52, 0.38),
                            ),
                            _ => (
                                gpui::hsla(0.0, 0.72, 0.50, 0.14),
                                ob.danger(),
                                gpui::hsla(0.0, 0.72, 0.54, 0.42),
                            ),
                        };
                        el.child(
                            div()
                                .w_full()
                                .px(px(10.0))
                                .py(px(9.0))
                                .rounded(px(8.0))
                                .bg(badge_bg)
                                .border_1()
                                .border_color(border_col)
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .px(px(6.0))
                                                .py(px(1.5))
                                                .rounded(px(4.0))
                                                .bg(bg_control)
                                                .text_size(px(10.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(badge_fg)
                                                .child(tr_str(lang, recovery.badge_zh, recovery.badge_en)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(badge_fg)
                                                .child(tr_str(
                                                    lang,
                                                    "当前服务商不可用，已被禁止选择",
                                                    "Provider unavailable and disabled from selection",
                                                )),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(text_sec)
                                        .child(reason),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap(px(8.0))
                                        .when(recovery.supports_browser_relogin, |row| {
                                            let kh = kind_hint.clone();
                                            let pid_r = pid_reauth.clone();
                                            row.child(
                                                div()
                                                    .id("magpie-editor-reauth-btn")
                                                    .cursor_pointer()
                                                    .h(px(28.0))
                                                    .px(px(11.0))
                                                    .rounded(px(7.0))
                                                    .bg(cta_bg)
                                                    .flex()
                                                    .items_center()
                                                    .text_size(px(12.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(cta_fg)
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            if is_reauthing {
                                                                this.trigger_cancel_reauthenticate();
                                                            } else {
                                                                this.trigger_reauthenticate_provider(
                                                                    &pid_r,
                                                                    kh.as_deref(),
                                                                );
                                                            }
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child(if is_reauthing {
                                                        tr_str(lang, "取消登录", "Cancel Login")
                                                    } else {
                                                        tr_str(lang, "重新登录并激活", "Re-login & Activate")
                                                    }),
                                            )
                                        })
                                        .child(
                                            div()
                                                .id("magpie-editor-refresh-activate-btn")
                                                .cursor_pointer()
                                                .h(px(28.0))
                                                .px(px(11.0))
                                                .rounded(px(7.0))
                                                .bg(bg_control)
                                                .border_1()
                                                .border_color(border_strong)
                                                .hover(move |s| s.bg(hover_bg))
                                                .flex()
                                                .items_center()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(text_pri)
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.trigger_refresh_and_activate(
                                                            &pid_refresh,
                                                            model_refresh.clone(),
                                                        );
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(if is_refreshing {
                                                    tr_str(lang, "正在校验...", "Verifying...")
                                                } else {
                                                    tr_str(lang, "刷新并重新激活", "Refresh & Reactivate")
                                                }),
                                        ),
                                ),
                        )
                    })
                    .when_some(quick_sub_login, |el, (sub_pid, sub_kind, hint_zh, hint_en, btn_zh, btn_en)| {
                        let is_reauthing_sub = self.is_provider_reauthing(sub_pid)
                            || self.reauth_provider_id.as_deref().is_some_and(|r| {
                                r.to_ascii_lowercase().contains(sub_pid)
                            });
                        el.child(
                            div()
                                .w_full()
                                .px(px(10.0))
                                .py(px(8.0))
                                .rounded(px(8.0))
                                .bg(bg_control)
                                .border_1()
                                .border_color(ob.border_hairline())
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(text_sec)
                                        .child(tr_str(lang, hint_zh, hint_en)),
                                )
                                .child(
                                    div()
                                        .id("magpie-editor-quick-sub-login-btn")
                                        .cursor_pointer()
                                        .h(px(28.0))
                                        .px(px(11.0))
                                        .rounded(px(7.0))
                                        .bg(cta_bg)
                                        .flex()
                                        .items_center()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(cta_fg)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                if is_reauthing_sub {
                                                    this.trigger_cancel_reauthenticate();
                                                } else {
                                                    this.trigger_reauthenticate_provider(
                                                        sub_pid,
                                                        Some(sub_kind),
                                                    );
                                                }
                                                cx.notify();
                                            }),
                                        )
                                        .child(if is_reauthing_sub {
                                            tr_str(lang, "登录中 · 取消", "Signing in · Cancel")
                                        } else {
                                            tr_str(lang, btn_zh, btn_en)
                                        }),
                                ),
                        )
                    })
                    .when_some(note, |el, n| {
                        el.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(text_muted)
                                .child(n),
                        )
                    })
                    .when(!regions.is_empty(), |el| {
                        el.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(text_sec)
                                        .child(tr_str(lang, "区域 / 计费通道", "Region / Plan")),
                                )
                                .child(div().flex().flex_wrap().gap(px(6.0)).children(
                                    regions.into_iter().enumerate().map(|(idx, r)| {
                                        let active =
                                            self.selected_region_id.as_deref() == Some(r.id.as_str());
                                        let rid = r.id.clone();
                                        div()
                                            .id(SharedString::from(format!("magpie-region-pill-{idx}")))
                                            .cursor_pointer()
                                            .px(px(10.0))
                                            .py(px(5.0))
                                            .rounded(px(7.0))
                                            .bg(if active { cta_bg } else { bg_control })
                                            .border_1()
                                            .border_color(if active { cta_bg } else { border_strong })
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(if active { cta_fg } else { text_pri })
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.select_region(&rid);
                                                    cx.notify();
                                                }),
                                            )
                                            .child(r.name)
                                    }),
                                )),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(text_sec)
                                    .child(tr_str(lang, "协议格式", "Protocol")),
                            )
                            .child(div().flex().flex_wrap().gap(px(6.0)).children(
                                protocols.into_iter().enumerate().map(|(idx, proto)| {
                                    let active = self.draft_api_mode == proto;
                                    let p_clone = proto.clone();
                                    div()
                                        .id(SharedString::from(format!("magpie-proto-pill-{idx}")))
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(5.0))
                                        .rounded(px(7.0))
                                        .bg(if active { cta_bg } else { bg_control })
                                        .border_1()
                                        .border_color(if active { cta_bg } else { border_strong })
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(if active { cta_fg } else { text_pri })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.select_protocol(&p_clone);
                                                cx.notify();
                                            }),
                                        )
                                        .child(proto)
                                }),
                            )),
                    )
                    .child(self.render_input_field(
                        "magpie-field-provider-id",
                        AiProviderField::ProviderId,
                        tr_str(lang, "服务商 ID", "Provider ID"),
                        &self.draft_provider_id,
                        "deepseek",
                        false,
                        pal,
                        cx,
                    ))
                    .child(self.render_input_field(
                        "magpie-field-display-name",
                        AiProviderField::DisplayName,
                        tr_str(lang, "显示名称", "Display Name"),
                        &self.draft_name,
                        "DeepSeek",
                        false,
                        pal,
                        cx,
                    ))
                    .child(self.render_input_field(
                        "magpie-field-base-url",
                        AiProviderField::BaseUrl,
                        "Base URL",
                        &self.draft_base_url,
                        "https://api.deepseek.com/v1",
                        false,
                        pal,
                        cx,
                    ))
                    .when(!suggested_models.is_empty(), |el| {
                        el.child(div().flex().flex_wrap().gap(px(5.0)).children(
                            suggested_models.into_iter().enumerate().map(|(idx, m)| {
                                let active = self.draft_model == m;
                                let m_val = m.clone();
                                div()
                                    .id(SharedString::from(format!("magpie-model-chip-{idx}")))
                                    .cursor_pointer()
                                    .px(px(9.0))
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .bg(if active { bg_active } else { bg_control })
                                    .border_1()
                                    .border_color(if active { text_muted } else { border_strong })
                                    .text_xs()
                                    .text_color(if active { text_pri } else { text_sec })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.draft_model = m_val.clone();
                                            cx.notify();
                                        }),
                                    )
                                    .child(m)
                            }),
                        ))
                    })
                    .child(self.render_input_field(
                        "magpie-field-model",
                        AiProviderField::Model,
                        tr_str(lang, "默认模型", "Default Model"),
                        &self.draft_model,
                        "deepseek-chat",
                        false,
                        pal,
                        cx,
                    ))
                    .when(!self.draft_no_key, |el| {
                        let ph = if self.draft_editing_has_key {
                            tr_str(lang, "已配置 Key · 留空保持不变", "Key saved · leave blank to keep")
                                .to_string()
                        } else {
                            key_hint.clone()
                        };
                        el.child(self.render_input_field(
                            "magpie-field-api-key",
                            AiProviderField::ApiKey,
                            "API Key",
                            &self.draft_api_key,
                            &ph,
                            true,
                            pal,
                            cx,
                        ))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .pt(px(6.0))
                            .child(
                                div()
                                    .id("magpie-save-provider-btn")
                                    .cursor_pointer()
                                    .h(px(34.0))
                                    .px(px(16.0))
                                    .rounded(px(8.0))
                                    .bg(cta_bg)
                                    .flex()
                                    .items_center()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(cta_fg)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.submit_provider_form();
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "保存并启用", "Save & Activate")),
                            )
                            .when_some(keys_url, |row, kurl| {
                                let kurl_open = kurl.clone();
                                row.child(
                                    div()
                                        .id("magpie-open-keys-url-btn")
                                        .cursor_pointer()
                                        .h(px(34.0))
                                        .px(px(12.0))
                                        .rounded(px(8.0))
                                        .bg(bg_control)
                                        .border_1()
                                        .border_color(border_strong)
                                        .hover(move |s| s.bg(hover_bg))
                                        .flex()
                                        .items_center()
                                        .text_size(px(12.5))
                                        .text_color(text_pri)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.pending_event = Some(
                                                    SettingsAiProvidersEvent::OpenExternalUrl(
                                                        kurl_open.clone(),
                                                    ),
                                                );
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "打开密钥页面", "Open Key Console")),
                                )
                                .child(
                                    div()
                                        .id("magpie-copy-keys-url-btn")
                                        .cursor_pointer()
                                        .h(px(34.0))
                                        .px(px(12.0))
                                        .rounded(px(8.0))
                                        .bg(bg_control)
                                        .border_1()
                                        .border_color(border_strong)
                                        .hover(move |s| s.bg(hover_bg))
                                        .flex()
                                        .items_center()
                                        .text_size(px(12.5))
                                        .text_color(text_sec)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.pending_event =
                                                    Some(SettingsAiProvidersEvent::CopyText(
                                                        kurl.clone(),
                                                    ));
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "复制链接", "Copy URL")),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_routing_tab(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let strategies = ["smart", "order", "rotate", "usage", "pace", "cheapest", "drain"];
        let active_strat = self
            .state
            .as_ref()
            .map(|s| s.routing_strategy.as_str())
            .unwrap_or("smart");
        let groups = self
            .state
            .as_ref()
            .map(|s| s.routing_groups.clone())
            .unwrap_or_default();
        let traces = self
            .state
            .as_ref()
            .map(|s| s.recent_route_traces.clone())
            .unwrap_or_default();

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                settings_card(pal)
                    .child(settings_subsection_title(
                        tr_str(lang, "全局智能路由策略", "Global Routing Strategy"),
                        pal,
                    ))
                    .child(div().flex().flex_wrap().gap(px(6.0)).children(strategies.into_iter().map(
                        |s| {
                            let act = active_strat == s;
                            let strat = s.to_string();
                            Self::pill_btn(
                                "magpie-global-strategy-btn",
                                s,
                                if act { pal.accent } else { pal.input_bg },
                                if act { pal.active_button_fg } else { pal.text_primary },
                                move |this, _| {
                                    this.pending_event = Some(
                                        SettingsAiProvidersEvent::SetRoutingStrategy(strat.clone()),
                                    );
                                },
                                cx,
                            )
                        },
                    ))),
            )
            .child(
                settings_card(pal)
                    .child(settings_subsection_title(
                        tr_str(lang, "模型聚合组 (group/<id>)", "Model Routing Groups (group/<id>)"),
                        pal,
                    ))
                    .children(groups.into_iter().map(|g| {
                        let gid = g.id.clone();
                        let targets = g
                            .members
                            .iter()
                            .map(|m| format!("{}@{}", m.target, m.weight))
                            .collect::<Vec<_>>()
                            .join(", ");
                        div()
                            .p(px(8.0))
                            .rounded(px(8.0))
                            .bg(rgb(pal.input_bg))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(format!("group/{} · {} ({})", g.id, g.name, g.strategy)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(pal.text_secondary))
                                            .child(targets),
                                    ),
                            )
                            .child(Self::pill_btn(
                                "magpie-del-group-btn",
                                tr_str(lang, "删除", "Delete"),
                                pal.card_bg,
                                pal.danger,
                                move |this, _| {
                                    this.pending_event = Some(
                                        SettingsAiProvidersEvent::DeleteRoutingGroup(gid.clone()),
                                    );
                                },
                                cx,
                            ))
                    }))
                    .child(self.render_input_field(
                        "magpie-group-id-input",
                        AiProviderField::GroupId,
                        tr_str(lang, "分组 ID", "Group ID"),
                        &self.group_id_draft,
                        "coding",
                        false,
                        pal,
                        cx,
                    ))
                    .child(self.render_input_field(
                        "magpie-group-name-input",
                        AiProviderField::GroupName,
                        tr_str(lang, "分组名称", "Group Name"),
                        &self.group_name_draft,
                        "Coding Pool",
                        false,
                        pal,
                        cx,
                    ))
                    .child(self.render_input_field(
                        "magpie-group-targets-input",
                        AiProviderField::GroupTargets,
                        tr_str(lang, "成员目标 (逗号分隔)", "Targets (comma-separated, optional @weight)"),
                        &self.group_targets_draft,
                        "deepseek/deepseek-chat@2, openai/gpt-4o-mini@1",
                        false,
                        pal,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.0))
                            .children(["smart", "order", "rotate", "cheapest"].into_iter().map(|s| {
                                let act = self.group_strategy_draft == s;
                                let val = s.to_string();
                                Self::pill_btn(
                                    "magpie-group-strat-btn",
                                    s,
                                    if act { pal.accent } else { pal.input_bg },
                                    if act { pal.active_button_fg } else { pal.text_primary },
                                    move |this, _| this.group_strategy_draft = val.clone(),
                                    cx,
                                )
                            })),
                    )
                    .child(self.render_input_field(
                        "magpie-group-fast-input",
                        AiProviderField::GroupFastModel,
                        tr_str(lang, "快速意图模型 (可选)", "Fast Intent Model (optional)"),
                        &self.group_fast_model_draft,
                        "deepseek/deepseek-chat",
                        false,
                        pal,
                        cx,
                    ))
                    .child(self.render_input_field(
                        "magpie-group-deep-input",
                        AiProviderField::GroupDeepModel,
                        tr_str(lang, "深度推理模型 (可选)", "Deep Intent Model (optional)"),
                        &self.group_deep_model_draft,
                        "anthropic/claude-sonnet-4-6",
                        false,
                        pal,
                        cx,
                    ))
                    .child(Self::pill_btn(
                        "magpie-save-group-btn",
                        tr_str(lang, "保存路由组", "Save Routing Group"),
                        pal.accent,
                        pal.active_button_fg,
                        |this, _| {
                            this.submit_group_form();
                        },
                        cx,
                    )),
            )
            .child(
                settings_card(pal)
                    .child(settings_subsection_title(
                        tr_str(lang, "最近路由决策轨迹", "Recent Route Decision Traces"),
                        pal,
                    ))
                    .when(traces.is_empty(), |el| {
                        el.child(settings_empty_state(
                            IconName::TerminalMcp,
                            tr_str(lang, "暂无路由记录", "No route traces recorded yet"),
                            pal,
                        ))
                    })
                    .children(traces.into_iter().rev().take(6).map(|tr| {
                        div()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .bg(rgb(pal.input_bg))
                            .text_size(px(11.0))
                            .text_color(rgb(pal.text_secondary))
                            .child(format!(
                                "{} -> {}/{} [{}] ({}) · mask={}",
                                tr.requested_model,
                                tr.resolved_provider_id,
                                tr.resolved_model,
                                tr.strategy,
                                tr.reason,
                                tr.masking_hits
                            ))
                    })),
            )
            .into_any_element()
    }

    fn render_privacy_import_tab(
        &self,
        pal: TdesktopThemePalette,
        lang: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                settings_card(pal)
                    .child(settings_subsection_title(
                        tr_str(lang, "请求隐私脱敏 (MaskingEngine)", "Prompt Privacy Masking"),
                        pal,
                    ))
                    .child(settings_toggle_row(
                        IconName::Lock,
                        tr_str(lang, "启用请求敏感信息自动脱敏", "Enable Automatic Prompt Masking"),
                        Some(tr_str(
                            lang,
                            "外发给上游 LLM 前替换密钥、JWT、邮箱与银行卡并在回包恢复",
                            "Masks secrets/PII before upstream calls and unmasks responses locally",
                        )),
                        self.masking_draft.enabled,
                        pal,
                        |this| this.masking_draft.enabled = !this.masking_draft.enabled,
                        cx,
                    ))
                    .child(settings_toggle_row(
                        IconName::Lock,
                        tr_str(lang, "脱敏 API Key / Bearer / PEM", "Mask Secrets (API Keys / JWT / PEM)"),
                        None,
                        self.masking_draft.mask_secrets,
                        pal,
                        |this| this.masking_draft.mask_secrets = !this.masking_draft.mask_secrets,
                        cx,
                    ))
                    .child(settings_toggle_row(
                        IconName::ContactsUsers,
                        tr_str(lang, "脱敏个人邮箱与银行卡", "Mask Personal Info (Emails & Luhn Cards)"),
                        None,
                        self.masking_draft.mask_personal,
                        pal,
                        |this| this.masking_draft.mask_personal = !this.masking_draft.mask_personal,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .children(["mask", "warn", "block"].into_iter().map(|act| {
                                let is_sel = self.masking_draft.default_action == act;
                                let val = act.to_string();
                                Self::pill_btn(
                                    "magpie-mask-action-btn",
                                    act,
                                    if is_sel { pal.accent } else { pal.input_bg },
                                    if is_sel { pal.active_button_fg } else { pal.text_primary },
                                    move |this, _| this.masking_draft.default_action = val.clone(),
                                    cx,
                                )
                            })),
                    )
                    .child(self.render_input_field(
                        "magpie-mask-custom-words",
                        AiProviderField::MaskCustomWords,
                        tr_str(lang, "自定义脱敏词 (逗号分隔)", "Custom Words (comma-separated)"),
                        &self.mask_custom_words_draft,
                        "INTERNAL_CODENAME, ACME_CORP",
                        false,
                        pal,
                        cx,
                    ))
                    .child(self.render_input_field(
                        "magpie-mask-allow-list",
                        AiProviderField::MaskAllowList,
                        tr_str(lang, "白名单豁免 (逗号分隔)", "Allow List (comma-separated)"),
                        &self.mask_allow_list_draft,
                        "support@example.com",
                        false,
                        pal,
                        cx,
                    ))
                    .child(Self::pill_btn(
                        "magpie-save-masking-btn",
                        tr_str(lang, "保存脱敏规则", "Save Masking Settings"),
                        pal.accent,
                        pal.active_button_fg,
                        |this, _| this.submit_masking_form(),
                        cx,
                    ))
                    .child(self.render_input_field(
                        "magpie-mask-preview-input",
                        AiProviderField::MaskPreviewText,
                        tr_str(lang, "脱敏效果实时试测", "Test Masking Preview"),
                        &self.mask_preview_draft,
                        "My key is sk-live-secret1234567890",
                        false,
                        pal,
                        cx,
                    ))
                    .child(Self::pill_btn(
                        "magpie-run-mask-preview-btn",
                        tr_str(lang, "预览脱敏结果", "Preview Masking"),
                        pal.input_bg,
                        pal.accent,
                        |this, _| {
                            let mut cfg = this.masking_draft.clone();
                            cfg.enabled = true;
                            cfg.custom_words = split_csv(&this.mask_custom_words_draft);
                            cfg.allow_list = split_csv(&this.mask_allow_list_draft);
                            this.pending_event = Some(SettingsAiProvidersEvent::PreviewMasking(
                                MagpieMaskingPreviewRequest {
                                    text: this.mask_preview_draft.clone(),
                                    config: Some(cfg),
                                },
                            ));
                        },
                        cx,
                    ))
                    .when_some(self.masking_preview_result.clone(), |el, res| {
                        el.child(
                            div()
                                .p(px(8.0))
                                .rounded(px(8.0))
                                .bg(rgb(pal.input_bg))
                                .text_xs()
                                .text_color(rgb(pal.text_primary))
                                .child(format!(
                                    "hits={} blocked={} => {}",
                                    res.hit_count, res.blocked, res.masked_text
                                )),
                        )
                    }),
            )
            .child(
                settings_card(pal)
                    .child(settings_subsection_title(
                        tr_str(lang, "一键导入配置 (.env / 导入链接)", "Import Config (.env / Import Link)"),
                        pal,
                    ))
                    .child(self.render_input_field(
                        "magpie-import-raw-input",
                        AiProviderField::ImportText,
                        tr_str(lang, "粘贴 .env 密钥或兼容导入链接", "Paste .env keys or import link"),
                        &self.import_raw_draft,
                        "DEEPSEEK_API_KEY=sk-... / OPENAI_API_KEY=sk-...",
                        false,
                        pal,
                        cx,
                    ))
                    .child(Self::pill_btn(
                        "magpie-import-submit-btn",
                        tr_str(lang, "立即导入并激活", "Import & Activate"),
                        pal.accent,
                        pal.active_button_fg,
                        |this, _| {
                            if !this.import_raw_draft.trim().is_empty() {
                                this.pending_event = Some(SettingsAiProvidersEvent::ImportConfig(
                                    MagpieImportRequest {
                                        raw: this.import_raw_draft.clone(),
                                        activate: this.import_activate,
                                    },
                                ));
                            }
                        },
                        cx,
                    )),
            )
            .into_any_element()
    }
}

#[allow(dead_code)]
fn subs_is_empty(st: Option<&MagpieStateDto>) -> bool {
    st.is_none_or(|s| s.subscriptions.is_empty())
}

fn opt_non_empty(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn split_csv(raw: &str) -> Vec<String> {
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

impl Render for SettingsAiProvidersSections {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_ime_controller(cx);
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.language_id.clone();

        let text_pri = ob.text_primary();
        let text_muted = ob.text_muted();
        let bg_control = ob.bg_control();
        let bg_surface = ob.bg_surface();
        let border_hairline = ob.border_hairline();
        let border_strong = ob.border_strong_hairline();
        let hover_bg = ob.hover();

        let sub_tabs = [
            (
                0usize,
                AiProvidersTab::Overview,
                tr_str(&lang, "服务商", "Providers"),
            ),
            (
                1usize,
                AiProvidersTab::Routing,
                tr_str(&lang, "路由组", "Routing"),
            ),
            (
                2usize,
                AiProvidersTab::PrivacyImport,
                tr_str(&lang, "脱敏与导入", "Privacy & Import"),
            ),
        ];
        let has_banners = self.error_message.is_some()
            || self.status_message.is_some()
            || self.probe_result.is_some();

        settings_container(pal)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .pb(px(2.0))
                    .child(
                        div()
                            .text_size(px(24.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_pri)
                            .child(tr_str(&lang, "服务商", "Providers")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .p(px(2.0))
                                    .rounded(px(9.0))
                                    .bg(bg_control)
                                    .border_1()
                                    .border_color(border_hairline)
                                    .flex()
                                    .items_center()
                                    .gap(px(2.0))
                                    .children(sub_tabs.into_iter().map(|(idx, t, label)| {
                                        let active = self.tab == t;
                                        div()
                                            .id(SharedString::from(format!("magpie-subtab-{}", idx)))
                                            .cursor_pointer()
                                            .h(px(26.0))
                                            .px(px(10.0))
                                            .rounded(px(7.0))
                                            .when(active, |el| {
                                                el.bg(bg_surface)
                                                    .border_1()
                                                    .border_color(border_strong)
                                            })
                                            .when(!active, |el| el.hover(move |s| s.bg(hover_bg)))
                                            .flex()
                                            .items_center()
                                            .text_size(px(12.0))
                                            .font_weight(if active {
                                                FontWeight::MEDIUM
                                            } else {
                                                FontWeight::NORMAL
                                            })
                                            .text_color(if active { text_pri } else { text_muted })
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.tab = t;
                                                    this.close_detail_view();
                                                    cx.notify();
                                                }),
                                            )
                                            .child(label.to_string())
                                    })),
                            )
                            .when(self.tab != AiProvidersTab::Overview, |row| {
                                row.child(
                                    div()
                                        .id("magpie-refresh-state-btn")
                                        .cursor_pointer()
                                        .h(px(30.0))
                                        .px(px(10.0))
                                        .rounded(px(8.0))
                                        .bg(bg_control)
                                        .border_1()
                                        .border_color(border_strong)
                                        .hover(move |s| s.bg(hover_bg))
                                        .flex()
                                        .items_center()
                                        .text_size(px(12.0))
                                        .text_color(text_pri)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.loading = true;
                                                this.pending_event =
                                                    Some(SettingsAiProvidersEvent::RefreshState);
                                                cx.notify();
                                            }),
                                        )
                                        .child(if self.loading {
                                            tr_str(&lang, "同步中...", "Syncing...")
                                        } else {
                                            tr_str(&lang, "刷新", "Refresh")
                                        }),
                                )
                            }),
                    ),
            )
            .when(has_banners, |col| col.child(self.render_status_banners(pal)))
            .child(match self.tab {
                AiProvidersTab::Overview => self.render_overview_tab(pal, &lang, cx),
                AiProvidersTab::Routing => self.render_routing_tab(pal, &lang, cx),
                AiProvidersTab::PrivacyImport => self.render_privacy_import_tab(pal, &lang, cx),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::TdesktopThemeMode;
    use gpui_telegram_client::net::hermes_client::MagpiePresetRegionDto;

    fn sample_state() -> MagpieStateDto {
        MagpieStateDto {
            presets: vec![
                MagpiePresetDto {
                    id: "deepseek".to_string(),
                    name: "DeepSeek".to_string(),
                    kind: "vendor".to_string(),
                    chat_url: Some("https://api.deepseek.com/v1".to_string()),
                    default_protocol: "chat_completions".to_string(),
                    supported_protocols: vec![
                        "chat_completions".to_string(),
                        "anthropic_messages".to_string(),
                    ],
                    anthropic_url: Some("https://api.deepseek.com/anthropic".to_string()),
                    default_model: "deepseek-chat".to_string(),
                    suggested_models: vec![
                        "deepseek-chat".to_string(),
                        "deepseek-reasoner".to_string(),
                    ],
                    no_key: false,
                    ..Default::default()
                },
                MagpiePresetDto {
                    id: "zhipu".to_string(),
                    name: "Zhipu GLM".to_string(),
                    kind: "vendor".to_string(),
                    default_protocol: "chat_completions".to_string(),
                    default_model: "glm-5".to_string(),
                    regions: vec![
                        MagpiePresetRegionDto {
                            id: "payg".to_string(),
                            name: "Pay as you go".to_string(),
                            chat_url: Some("https://open.bigmodel.cn/api/paas/v4".to_string()),
                            models: vec!["glm-5".to_string()],
                            ..Default::default()
                        },
                        MagpiePresetRegionDto {
                            id: "coding".to_string(),
                            name: "Coding Plan".to_string(),
                            chat_url: Some("https://open.bigmodel.cn/api/coding/paas/v4".to_string()),
                            models: vec!["glm-5-code".to_string()],
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
                MagpiePresetDto {
                    id: "ollama".to_string(),
                    name: "Ollama".to_string(),
                    kind: "local".to_string(),
                    chat_url: Some("http://127.0.0.1:11434/v1".to_string()),
                    default_protocol: "chat_completions".to_string(),
                    default_model: "qwen2.5-coder".to_string(),
                    no_key: true,
                    ..Default::default()
                },
            ],
            needs_provider_selection: true,
            routing_strategy: "smart".to_string(),
            ..Default::default()
        }
    }

    #[std::prelude::v1::test]
    fn preset_selection_region_protocol_and_validation_work() {
        let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12);
        let mut view = SettingsAiProvidersSections::new_live(pal, "zh-CN");
        view.sync_state(sample_state());

        view.open_preset_editor("deepseek");
        assert!(view.editor_open);
        assert_eq!(view.draft_provider_id, "deepseek");
        assert_eq!(view.draft_base_url, "https://api.deepseek.com/v1");
        // Missing key fails cleanly for non-keyless preset
        assert!(!view.submit_provider_form());
        assert!(view.error_message.is_some());

        view.select_protocol("anthropic_messages");
        assert_eq!(view.draft_base_url, "https://api.deepseek.com/anthropic");

        view.draft_api_key = "sk-ds-test-123".to_string();
        assert!(view.submit_provider_form());
        match view.take_event() {
            Some(SettingsAiProvidersEvent::SaveProvider(req)) => {
                assert_eq!(req.preset_id.as_deref(), Some("deepseek"));
                assert_eq!(req.api_key.as_deref(), Some("sk-ds-test-123"));
                assert!(req.activate);
            }
            other => panic!("expected SaveProvider, got {other:?}"),
        }

        // Region switching updates URL & model
        view.open_preset_editor("zhipu");
        assert_eq!(view.selected_region_id.as_deref(), Some("payg"));
        view.select_region("coding");
        assert_eq!(
            view.draft_base_url,
            "https://open.bigmodel.cn/api/coding/paas/v4"
        );
        assert_eq!(view.draft_model, "glm-5-code");

        // Keyless preset succeeds without API key
        view.open_preset_editor("ollama");
        assert!(view.draft_no_key);
        assert!(view.submit_provider_form());
        match view.take_event() {
            Some(SettingsAiProvidersEvent::SaveProvider(req)) => {
                assert_eq!(req.provider_id.as_deref(), Some("ollama"));
                assert_eq!(req.no_key_required, Some(true));
                assert!(req.api_key.is_none());
            }
            other => panic!("expected keyless SaveProvider, got {other:?}"),
        }
    }

    #[std::prelude::v1::test]
    fn routing_group_and_masking_forms_parse_cleanly() {
        let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12);
        let mut view = SettingsAiProvidersSections::new_live(pal, "en");
        view.sync_state(sample_state());

        view.group_id_draft = "coding".to_string();
        view.group_name_draft = "Code Pool".to_string();
        view.group_targets_draft = "deepseek/deepseek-chat@3, openai/gpt-4o-mini".to_string();
        view.group_fast_model_draft = "deepseek/deepseek-chat".to_string();
        assert!(view.submit_group_form());
        match view.take_event() {
            Some(SettingsAiProvidersEvent::UpsertRoutingGroup(g)) => {
                assert_eq!(g.id, "coding");
                assert_eq!(g.members.len(), 2);
                assert_eq!(g.members[0].weight, 3);
                assert_eq!(g.members[1].weight, 1);
                assert_eq!(g.intent_fast_model.as_deref(), Some("deepseek/deepseek-chat"));
            }
            other => panic!("expected UpsertRoutingGroup, got {other:?}"),
        }

        view.masking_draft.enabled = true;
        view.mask_custom_words_draft = "PROJECT_ZEUS, SECRET_TOKEN".to_string();
        view.mask_allow_list_draft = "help@example.com".to_string();
        view.submit_masking_form();
        match view.take_event() {
            Some(SettingsAiProvidersEvent::UpdateMasking(cfg)) => {
                assert!(cfg.enabled);
                assert_eq!(cfg.custom_words, vec!["PROJECT_ZEUS", "SECRET_TOKEN"]);
                assert_eq!(cfg.allow_list, vec!["help@example.com"]);
            }
            other => panic!("expected UpdateMasking, got {other:?}"),
        }
    }

    #[std::prelude::v1::test]
    fn discovered_ai_agents_cards_and_activation_event_work() {
        let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12);
        let mut view = SettingsAiProvidersSections::new_live(pal, "zh-CN");
        let mut st = sample_state();
        st.subscriptions = vec![
            MagpieSubscriptionDto {
                id: "google-vertex".to_string(),
                kind: "google_vertex".to_string(),
                label: "Google Vertex AI (test@vertex.com)".to_string(),
                email: Some("test@vertex.com".to_string()),
                account_id: Some("test-project-123".to_string()),
                plan: Some("vertex_adc".to_string()),
                active: false,
                default_model: Some("gemini-2.5-flash".to_string()),
                ..Default::default()
            },
            MagpieSubscriptionDto {
                id: "codex".to_string(),
                kind: "codex_chatgpt".to_string(),
                label: "Codex / ChatGPT (Plus)".to_string(),
                email: Some("user@openai.com".to_string()),
                account_id: Some("chatgpt-sub-123".to_string()),
                plan: Some("plus".to_string()),
                active: false,
                default_model: Some("gpt-4o".to_string()),
                ..Default::default()
            },
        ];
        view.sync_state(st);

        let subs = view.effective_subscriptions();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].id, "google-vertex");
        assert_eq!(subs[0].plan.as_deref(), Some("vertex_adc"));
        assert!(subs[0].models.contains(&"gemini-2.5-flash".to_string()));
        assert!(subs[0].models.contains(&"gemini-2.5-pro".to_string()));
        assert!(!subs[0].active);

        // Model selection
        view.select_subscription_model("google-vertex", "gemini-2.5-pro");
        assert_eq!(
            view.selected_subscription_models
                .get("google-vertex")
                .map(String::as_str),
            Some("gemini-2.5-pro")
        );

        // Click activate
        view.activate_subscription_action("google-vertex");
        match view.take_event() {
            Some(SettingsAiProvidersEvent::ActivateSubscription {
                subscription_id,
                model,
            }) => {
                assert_eq!(subscription_id, "google-vertex");
                assert_eq!(model.as_deref(), Some("gemini-2.5-pro"));
            }
            other => panic!("expected ActivateSubscription event, got {other:?}"),
        }

        // Test sync_subscriptions with server DTOs and active state reflection
        // When google-vertex is activated, server state active_provider is updated
        let mut st_act = view.state.clone().unwrap();
        st_act.active_provider = "google-vertex".to_string();
        view.sync_state(st_act);

        let server_subs = vec![
            SubscriptionAccountDto {
                id: "google-vertex".to_string(),
                account_id: Some("test-project-123".to_string()),
                kind: "google_vertex".to_string(),
                label: "Google Vertex AI (ADC)".to_string(),
                plan: Some("vertex_adc".to_string()),
                models: vec![
                    "gemini-2.5-flash".to_string(),
                    "gemini-2.5-pro".to_string(),
                ],
                active: true,
                ..Default::default()
            },
            SubscriptionAccountDto {
                id: "codex".to_string(),
                account_id: Some("user@openai.com".to_string()),
                kind: "codex_chatgpt".to_string(),
                label: "Codex / ChatGPT".to_string(),
                plan: Some("plus".to_string()),
                models: vec!["gpt-4o".to_string(), "codex-mini-latest".to_string()],
                active: false,
                ..Default::default()
            },
        ];
        view.sync_subscriptions(server_subs);
        let eff = view.effective_subscriptions();
        assert_eq!(eff.len(), 2);
        assert!(eff[0].active);
        assert!(!eff[1].active);

        // Switch active provider to codex: codex becomes active, google-vertex becomes inactive
        let mut st_codex = view.state.clone().unwrap();
        st_codex.active_provider = "codex".to_string();
        view.sync_state(st_codex);
        let eff_switched = view.effective_subscriptions();
        assert_eq!(eff_switched.len(), 2);
        assert!(!eff_switched[0].active);
        assert!(eff_switched[1].active);
    }

    #[std::prelude::v1::test]
    fn sync_state_reconciles_added_and_removed_subscriptions_while_preserving_server_metadata() {
        let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12);
        let mut view = SettingsAiProvidersSections::new_live(pal, "en");

        let sub_vertex = MagpieSubscriptionDto {
            id: "google_vertex:a".to_string(),
            kind: "google_vertex".to_string(),
            label: "Google Vertex A".to_string(),
            email: Some("a@vertex.internal".to_string()),
            account_id: Some("acct-a".to_string()),
            plan: Some("vertex_adc".to_string()),
            active: true,
            default_model: Some("gemini-2.5-flash".to_string()),
            ..Default::default()
        };
        let sub_codex = MagpieSubscriptionDto {
            id: "codex:b".to_string(),
            kind: "codex_chatgpt".to_string(),
            label: "Codex B".to_string(),
            email: Some("b@example.com".to_string()),
            account_id: Some("acct-b".to_string()),
            plan: Some("plus".to_string()),
            active: false,
            default_model: Some("gpt-4o".to_string()),
            ..Default::default()
        };

        // 1. Initial sync_state + sync_subscriptions with server-enriched metadata
        let mut st1 = sample_state();
        st1.active_provider = "google_vertex:a".to_string();
        st1.subscriptions = vec![sub_vertex.clone()];
        view.sync_state(st1.clone());
        view.sync_subscriptions(vec![SubscriptionAccountDto {
            id: "google_vertex:a".to_string(),
            account_id: Some("acct-a".to_string()),
            kind: "google_vertex".to_string(),
            label: "Server Label Vertex A".to_string(),
            plan: Some("vertex_adc".to_string()),
            models: vec!["gemini-2.5-pro-server-custom".to_string()],
            active: true,
            ..Default::default()
        }]);
        assert_eq!(view.effective_subscriptions().len(), 1);

        // 2. Subsequent sync_state adds codex:b and switches active_provider to codex:b (without sync_subscriptions)
        let mut st2 = sample_state();
        st2.active_provider = "codex:b".to_string();
        st2.subscriptions = vec![sub_vertex.clone(), sub_codex.clone()];
        view.sync_state(st2);

        let eff2 = view.effective_subscriptions();
        assert_eq!(eff2.len(), 2, "newly added subscription must appear after sync_state");
        let v_after_add = eff2.iter().find(|s| s.id == "google_vertex:a").unwrap();
        let c_after_add = eff2.iter().find(|s| s.id == "codex:b").unwrap();
        assert_eq!(
            v_after_add.label, "Server Label Vertex A",
            "existing server-fetched metadata must be preserved"
        );
        assert_eq!(
            v_after_add.models,
            vec!["gemini-2.5-pro-server-custom".to_string()]
        );
        assert!(!v_after_add.active);
        assert!(c_after_add.active);

        // 3. Subsequent sync_state removes google_vertex:a
        let mut st3 = sample_state();
        st3.active_provider = "codex:b".to_string();
        st3.subscriptions = vec![sub_codex];
        view.sync_state(st3);

        let eff3 = view.effective_subscriptions();
        assert_eq!(eff3.len(), 1, "removed subscription must be pruned");
        assert_eq!(eff3[0].id, "codex:b");
        assert!(eff3[0].active);

        // 4. Subsequent sync_state clears all subscriptions
        let mut st4 = sample_state();
        st4.active_provider = "local_hermes".to_string();
        st4.subscriptions = Vec::new();
        view.sync_state(st4);

        assert!(
            view.effective_subscriptions().is_empty(),
            "all subscriptions must be pruned when state.subscriptions becomes empty"
        );

        // 5. Client-side locally discovered subscriptions remain visible when remote server
        // returns empty state.subscriptions (legacy v1.3.0 server compatibility)
        view.sync_local_subscriptions(vec![SubscriptionAccountDto {
            id: "google_vertex:local@mac".to_string(),
            account_id: Some("proj-local".to_string()),
            kind: "google_vertex".to_string(),
            label: "Google Vertex AI (local@mac)".to_string(),
            plan: Some("vertex_adc".to_string()),
            models: vec!["gemini-2.5-flash".to_string(), "gemini-2.5-pro".to_string()],
            active: false,
            ..Default::default()
        }]);
        let mut st5 = sample_state();
        st5.active_provider = "google_vertex:local@mac".to_string();
        st5.subscriptions = Vec::new();
        view.sync_state(st5);

        let eff5 = view.effective_subscriptions();
        assert_eq!(eff5.len(), 1);
        assert_eq!(eff5[0].id, "google_vertex:local@mac");
        assert!(eff5[0].active, "local subscription must reflect active_provider from server state");
    }

    #[std::prelude::v1::test]
    fn openbot_providers_connected_entries_detail_and_escape_flow_work() {
        let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12);
        let mut view = SettingsAiProvidersSections::new_live(pal, "zh-CN");
        let mut st = sample_state();
        st.active_provider = "codex:1".to_string();
        st.needs_provider_selection = false;
        st.subscriptions = vec![
            MagpieSubscriptionDto {
                id: "codex:1".to_string(),
                kind: "codex_chatgpt".to_string(),
                label: "Codex / ChatGPT".to_string(),
                email: Some("dev@openai.com".to_string()),
                account_id: Some("dev@openai.com".to_string()),
                active: true,
                default_model: Some("gpt-6-luna".to_string()),
                ..Default::default()
            },
            MagpieSubscriptionDto {
                id: "claude:1".to_string(),
                kind: "claude_code".to_string(),
                label: "Claude Code".to_string(),
                email: None,
                account_id: None,
                active: false,
                default_model: Some("claude-sonnet-4-6".to_string()),
                ..Default::default()
            },
        ];
        st.providers = vec![MagpieProviderDto {
            provider_id: "deepseek".to_string(),
            preset_id: Some("deepseek".to_string()),
            name: "DeepSeek".to_string(),
            default_model: "deepseek-chat".to_string(),
            masked_key: Some("sk-***1234".to_string()),
            is_active: false,
            ..Default::default()
        }];
        view.sync_state(st);

        let entries = view.connected_provider_entries("zh-CN");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, "codex:1");
        assert_eq!(entries[0].display_name, "ChatGPT");
        assert_eq!(entries[0].subtitle, "dev@openai.com");
        assert!(entries[0].is_active);
        assert!(entries[0].usable);
        assert_eq!(entries[1].display_name, "Claude");
        assert_eq!(entries[1].subtitle, "在这台电脑上可用");
        assert!(entries[1].usable);
        assert_eq!(entries[2].id, "deepseek");
        assert_eq!(entries[2].display_name, "DeepSeek");
        assert!(entries[2].usable);

        // Progressive Escape cascade: dropdown -> subscription detail -> search
        view.default_provider_dropdown_open = true;
        assert!(view.handle_escape_key());
        assert!(!view.default_provider_dropdown_open);

        view.open_subscription_detail("codex:1");
        assert_eq!(view.selected_subscription_detail_id.as_deref(), Some("codex:1"));
        assert!(view.handle_escape_key());
        assert!(view.selected_subscription_detail_id.is_none());

        // Clicking an OpenBot "其他服务商" row opens matching preset or prefilled custom editor
        view.open_other_provider_editor("deepseek", "DeepSeek", Some("deepseek"));
        assert!(view.editor_open);
        assert_eq!(view.selected_preset_id.as_deref(), Some("deepseek"));
        assert!(view.handle_escape_key());
        assert!(!view.editor_open);

        view.open_other_provider_editor("grok", "Grok", Some("xai"));
        assert!(view.editor_open);
        assert_eq!(view.draft_provider_id, "grok");
        assert_eq!(view.draft_name, "Grok");
        view.close_detail_view();
        assert!(!view.editor_open);

        view.preset_search = "claude".to_string();
        assert!(view.handle_escape_key());
        assert!(view.preset_search.is_empty());
        assert!(!view.handle_escape_key());
    }

    #[std::prelude::v1::test]
    fn unavailable_vertex_subscription_cannot_be_activated_and_shows_reason() {
        let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12);
        let mut view = SettingsAiProvidersSections::new_live(pal, "zh-CN");
        let mut st = sample_state();
        st.active_provider = "deepseek".to_string();
        st.needs_provider_selection = false;
        st.subscriptions = vec![MagpieSubscriptionDto {
            id: "google_vertex:dead@example.com".to_string(),
            kind: "google_vertex".to_string(),
            label: "Google Vertex AI (dead@example.com)".to_string(),
            email: Some("dead@example.com".to_string()),
            account_id: Some("proj-dead".to_string()),
            plan: Some("vertex_adc".to_string()),
            active: false,
            default_model: Some("gemini-2.5-flash".to_string()),
            usable: false,
            unavailable_reason: Some(
                "Google Vertex AI OAuth 授权已失效 (invalid_grant)，请重新登录".to_string(),
            ),
            ..Default::default()
        }];
        st.providers = vec![MagpieProviderDto {
            provider_id: "deepseek".to_string(),
            preset_id: Some("deepseek".to_string()),
            name: "DeepSeek".to_string(),
            default_model: "deepseek-chat".to_string(),
            masked_key: Some("sk-***1234".to_string()),
            is_active: true,
            usable: true,
            unavailable_reason: None,
            ..Default::default()
        }];
        view.sync_state(st);

        // Usable providers sort before unavailable entries, and unavailable displays reason
        let entries = view.connected_provider_entries("zh-CN");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "deepseek");
        assert!(entries[0].usable);
        assert_eq!(entries[1].id, "google_vertex:dead@example.com");
        assert!(!entries[1].usable);
        assert!(!entries[1].is_active);
        assert!(entries[1].subtitle.contains("invalid_grant"));

        // Attempting to activate the unavailable Vertex subscription MUST NOT emit an event
        view.activate_subscription_action("google_vertex:dead@example.com");
        assert!(view.take_event().is_none());
        assert!(view.error_message.as_deref().is_some_and(|m| m.contains("invalid_grant")));
    }

    #[std::prelude::v1::test]
    fn expired_subscription_emits_reauth_refresh_and_cancel_events_while_staying_unselectable() {
        let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12);
        let mut view = SettingsAiProvidersSections::new_live(pal, "zh-CN");
        let mut st = sample_state();
        st.active_provider = "local_hermes".to_string();
        st.needs_provider_selection = true;
        st.subscriptions = vec![MagpieSubscriptionDto {
            id: "codex:expired@example.com".to_string(),
            kind: "codex_chatgpt".to_string(),
            label: "Codex / ChatGPT (expired@example.com)".to_string(),
            email: Some("expired@example.com".to_string()),
            active: false,
            default_model: Some("gpt-6-luna".to_string()),
            usable: false,
            unavailable_code: Some("oauth_invalid_grant".to_string()),
            unavailable_reason: Some("Codex / ChatGPT 授权已过期 (invalid_grant)".to_string()),
            ..Default::default()
        }];
        view.sync_state(st);

        // Selecting expired subscription for inference stays strictly blocked
        view.activate_subscription_action("codex:expired@example.com");
        assert!(view.take_event().is_none());

        // Clicking Re-login emits ReauthenticateProvider
        view.trigger_reauthenticate_provider("codex:expired@example.com", Some("codex_chatgpt"));
        assert!(matches!(
            view.take_event(),
            Some(SettingsAiProvidersEvent::ReauthenticateProvider {
                ref provider_id,
                ref kind_hint,
            }) if provider_id == "codex:expired@example.com"
                && kind_hint.as_deref() == Some("codex_chatgpt")
        ));

        // Live progress state syncs cleanly
        let mut refreshing = std::collections::HashSet::new();
        refreshing.insert("codex:expired@example.com".to_string());
        view.sync_recovery_progress(
            Some("codex:expired@example.com".to_string()),
            Some("正在浏览器中登录…".to_string()),
            refreshing,
        );
        assert!(view.is_provider_reauthing("codex:expired@example.com"));
        assert!(view.is_provider_refreshing("codex:expired@example.com"));

        // Clicking Cancel emits CancelReauthenticate
        view.trigger_cancel_reauthenticate();
        assert!(matches!(
            view.take_event(),
            Some(SettingsAiProvidersEvent::CancelReauthenticate)
        ));

        // Clicking Refresh & Reactivate emits RefreshAndActivateProvider
        view.trigger_refresh_and_activate("codex:expired@example.com", Some("gpt-6-luna".to_string()));
        assert!(matches!(
            view.take_event(),
            Some(SettingsAiProvidersEvent::RefreshAndActivateProvider {
                ref provider_id,
                ref model,
            }) if provider_id == "codex:expired@example.com"
                && model.as_deref() == Some("gpt-6-luna")
        ));

        // open_provider_for_recovery focuses directly on the subscription detail card
        view.open_provider_for_recovery("codex:expired@example.com");
        assert_eq!(
            view.selected_subscription_detail_id.as_deref(),
            Some("codex:expired@example.com")
        );
    }
}
