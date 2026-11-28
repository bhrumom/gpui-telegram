//! Synchronization and action dispatch between `AppShell`, `SettingsAiProvidersSections`,
//! `GlobalTasksWorkspace`, and the Magpie backend routes (`/api/v1/hermes/magpie/*`).

use std::future::Future;
use std::time::Duration;

use gpui::{AsyncApp, ClipboardItem, Context, WeakEntity};
use gpui_telegram_agent::hermes::providers::classify_provider_auth_error_code;
use gpui_telegram_client::net::hermes_client::{
    MagpieGroupsRequest, MagpieProbeRequest, MagpieSaveProviderRequest, MagpieStateDto,
};
use magpie_gateway::SubscriptionKind;

use crate::i18n::tr_str;
use crate::views::app_shell::local_subscription_bridge::{
    activate_subscription_with_local_fallback_pinned,
    apply_local_subscription_health_to_fabushi_for_user,
    discover_all_local_subscriptions_in_home, discover_local_subscriptions_for_server,
    enrich_items_with_live_catalog, fetch_live_models_catalog_pinned,
    fetch_models_for_subscription_live_pinned, hydrate_local_subscriptions_into_fabushi_for_user,
    refresh_and_activate_provider_or_subscription_pinned,
    refresh_legacy_provider_token_once_pinned, resolve_local_home_dir,
    sync_local_subscriptions_to_server_pinned, verify_discovered_subscriptions,
    ActiveProviderReauth, LocalSubscriptionHealth,
};
use crate::views::app_shell::provider_reauth::{
    cli_relogin_command_for_kind, infer_subscription_kind, open_external_url_in_system_browser,
    run_interactive_subscription_reauth, ReauthError,
};
use crate::views::app_shell::shell_truth::ClientCallFence;
use crate::views::drawer::InspectorAiModelCatalog;
use crate::views::settings::sections::settings_ai_providers::SubscriptionAccountDto;
use crate::views::settings::sections::SettingsAiProvidersEvent;
use crate::views::settings::settings_type::SettingsSectionType;
use crate::views::shell::AppShell;

async fn http_fetch_subscriptions(
    client: &gpui_telegram_client::TelegramClient,
    expected_epoch: u64,
    base_url: &str,
    token: &str,
) -> Result<Vec<SubscriptionAccountDto>, String> {
    client
        .ensure_pinned_session(expected_epoch, token)
        .map_err(|e| format!("Session changed before subscriptions fetch: {e}"))?;
    let url = format!("{}/api/v1/hermes/magpie/subscriptions", base_url.trim_end_matches('/'));
    let http = magpie_gateway::build_proxy_aware_http_client(
        Some(std::time::Duration::from_secs(10)),
        Some(std::time::Duration::from_secs(15)),
    );
    let resp = http
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("HTTP GET subscriptions failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Server returned status {}", resp.status()));
    }
    resp.json::<Vec<SubscriptionAccountDto>>()
        .await
        .map_err(|e| format!("Failed to parse subscriptions JSON: {e}"))
}

fn magpie_fence(shell: &AppShell) -> Option<ClientCallFence> {
    if shell.session_sync.user_id.is_none() {
        return None;
    }
    shell.capture_client_fence()
}

fn summarize_magpie_for_main(state: &MagpieStateDto, lang: &str) -> (Option<String>, bool) {
    let active_pid = state.active_provider.trim();
    let active_unusable = !active_pid.is_empty()
        && active_pid != "local_hermes"
        && (state
            .providers
            .iter()
            .find(|p| p.provider_id == active_pid)
            .is_some_and(|p| !p.is_selectable())
            || state
                .subscriptions
                .iter()
                .find(|s| s.id == active_pid)
                .is_some_and(|s| !s.is_selectable()));
    if state.needs_provider_selection || active_unusable {
        return (
            Some(tr_str(lang, "请选择供应商", "Choose Provider").to_string()),
            true,
        );
    }
    let active = if !active_pid.is_empty() && !state.active_model.is_empty() {
        format!("{} · {}", active_pid, state.active_model)
    } else if !active_pid.is_empty() {
        active_pid.to_string()
    } else if let Some(sub) = state
        .subscriptions
        .iter()
        .find(|s| s.active && s.is_selectable())
    {
        sub.label.clone()
    } else {
        tr_str(lang, "已配置", "Ready").to_string()
    };
    (Some(active), false)
}

impl AppShell {
    /// Opens Settings directly to `SettingsSectionType::AiProviders` and fetches fresh Magpie state.
    pub fn open_ai_providers_settings(&mut self, cx: &mut Context<Self>) {
        self.navigate_settings_to(SettingsSectionType::AiProviders, cx);
        self.refresh_magpie_state(cx);
    }

    /// Returns `true` when `provider_id` is NOT marked unavailable by either local verification
    /// or server state.
    pub(crate) fn is_provider_selectable_in_current_state(&self, provider_id: &str) -> bool {
        let pid = provider_id.trim();
        if pid.is_empty() {
            return false;
        }
        if !self.magpie_local.is_local_item_usable(pid) {
            return false;
        }
        if let Some(sub) = self
            .magpie_local
            .server_subscriptions
            .iter()
            .find(|s| s.id == pid)
        {
            if !sub.is_selectable() {
                return false;
            }
        }
        if let Some(ref st) = self.magpie_local.latest_state {
            if let Some(sub) = st.subscriptions.iter().find(|s| s.id == pid) {
                if !sub.is_selectable() {
                    return false;
                }
            }
            if let Some(prov) = st.providers.iter().find(|p| p.provider_id == pid) {
                if !prov.is_selectable() {
                    return false;
                }
            }
        }
        true
    }

    pub(crate) fn sync_local_health_to_fabushi(&self) {
        if let Some(ref host) = self.bot_browser.local_fabushi {
            let uid_str = self.session_sync.user_id.map(|u| u.to_string());
            apply_local_subscription_health_to_fabushi_for_user(
                host,
                &self.magpie_local.discovered_health,
                uid_str.as_deref(),
            );
        }
    }

    /// Returns the live `InspectorAiModelCatalog` from cached Magpie state (or `ContextInspector` fallback).
    pub(crate) fn current_ai_model_catalog(&self, cx: &gpui::App) -> InspectorAiModelCatalog {
        if let Some(ref state) = self.magpie_local.latest_state {
            let local_dtos = self
                .magpie_local
                .cached_dtos(Some(state.active_provider.as_str()));
            InspectorAiModelCatalog::from_magpie_state(
                state,
                &self.magpie_local.server_subscriptions,
                &local_dtos,
            )
        } else {
            self.inspector.read(cx).ai_models.clone()
        }
    }

    /// Synchronizes the conversation header model picker with live Settings AI models and the active bot's
    /// real `preferred_model` / `thinking_level` (never falling back to hardcoded fake models).
    pub(crate) fn sync_model_picker_to_stage(&mut self, cx: &mut Context<Self>) {
        let catalog = self.current_ai_model_catalog(cx);
        let viewer_uid = self.session_sync.user_id;
        let active_fab = self
            .stage
            .read(cx)
            .active_fabushi_bot
            .clone()
            .or_else(|| self.inspector.read(cx).active_fabushi_bot.clone());
        let bot_id = active_fab
            .as_ref()
            .map(|b| b.bot_user_id.max(0) as u64)
            .filter(|&id| id > 0)
            .or_else(|| self.active_chat_bot_id(cx));
        let comp_override = bot_id
            .and_then(|bid| self.computers.bot_profiles.get(&bid))
            .map(|p| p.preferred_model.trim().to_string())
            .filter(|m| !InspectorAiModelCatalog::is_placeholder_model(m))
            .or_else(|| {
                let insp = self.inspector.read(cx);
                let matches_active = bot_id.is_none()
                    || insp.profile.bot_id == 0
                    || Some(insp.profile.bot_id) == bot_id;
                matches_active
                    .then(|| insp.effective_bot_preferred_model())
                    .flatten()
            });
        let fab_override = active_fab
            .as_ref()
            .map(|f| f.preferred_model.trim().to_string())
            .filter(|m| !InspectorAiModelCatalog::is_placeholder_model(m));
        let bot_override_model = comp_override.or(fab_override);

        let is_strict_fabushi_owner = active_fab.as_ref().is_some_and(|fab| {
            fab.owner_user_id > 0 && viewer_uid.is_some_and(|u| fab.owner_user_id as u64 == u)
        });
        let is_non_fabushi_owner = active_fab.is_none()
            && bot_id.is_some()
            && self.inspector.read(cx).profile.is_viewer_owner;
        let can_override_bot_model = is_strict_fabushi_owner || is_non_fabushi_owner;
        let can_edit_thinking_effort = active_fab.is_some();

        let thinking_effort = active_fab
            .as_ref()
            .map(|f| f.thinking_level.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_default();

        self.stage.update(cx, |st, st_cx| {
            st.sync_model_picker_context(
                &catalog,
                bot_override_model,
                thinking_effort,
                can_override_bot_model,
                can_edit_thinking_effort,
            );
            st_cx.notify();
        });
    }

    /// Switches the viewer's active Settings/Magpie model on the specified (or currently active)
    /// usable provider or subscription. Refuses selection if the target provider is unavailable.
    pub(crate) fn switch_active_settings_model(
        &mut self,
        provider_id: Option<String>,
        model: String,
        chat_id: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let trimmed = model.trim().to_string();
        if trimmed.is_empty() {
            self.sync_model_picker_to_stage(cx);
            return;
        }
        let lang = self.active_lang_id().to_string();
        let Some(state) = self.magpie_local.latest_state.clone() else {
            self.refresh_magpie_state(cx);
            if let Some(cid) = chat_id {
                self.show_chat_toast(
                    cid,
                    tr_str(
                        &lang,
                        "正在同步模型设置，请稍后再试",
                        "Syncing model settings, please retry shortly",
                    )
                    .to_string(),
                    2000,
                    cx,
                );
            }
            return;
        };

        let target_pid = provider_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| state.active_provider.trim().to_string());

        if !target_pid.is_empty()
            && !trimmed.starts_with("group/")
            && !self.is_provider_selectable_in_current_state(&target_pid)
        {
            let msg = tr_str(
                &lang,
                "该服务商当前不可用，请检查凭据或重新登录后再试",
                "This provider is currently unavailable — check credentials or re-login",
            )
            .to_string();
            if let Some(cid) = chat_id {
                self.show_chat_toast(cid, msg, 2800, cx);
            } else {
                self.show_non_chat_toast(msg, 2800, cx);
            }
            self.sync_model_picker_to_stage(cx);
            return;
        }

        let matching_sub_id: Option<String> = if target_pid.is_empty() || trimmed.starts_with("group/") {
            None
        } else {
            self.magpie_local
                .discovered_items
                .iter()
                .find(|i| {
                    i.account.id == target_pid
                        || (i.account.kind == magpie_gateway::SubscriptionKind::GoogleVertex
                            && target_pid == "magpie_provider:google-vertex")
                })
                .map(|i| i.account.id.clone())
                .or_else(|| {
                    self.magpie_local
                        .server_subscriptions
                        .iter()
                        .find(|s| {
                            s.id == target_pid
                                || (s.kind == "google_vertex"
                                    && target_pid == "magpie_provider:google-vertex")
                        })
                        .map(|s| s.id.clone())
                })
                .or_else(|| {
                    state
                        .subscriptions
                        .iter()
                        .find(|s| {
                            s.id == target_pid
                                || (s.kind == "google_vertex"
                                    && target_pid == "magpie_provider:google-vertex")
                        })
                        .map(|s| s.id.clone())
                })
        };

        if let Some(sub_id) = matching_sub_id {
            self.activate_subscription(sub_id, Some(trimmed.clone()), cx);
            if let Some(cid) = chat_id {
                self.show_chat_toast(
                    cid,
                    format!("{}: {trimmed}", tr_str(&lang, "正在切换模型", "Switching model")),
                    1800,
                    cx,
                );
            }
            return;
        }

        let target_provider_id = if !target_pid.is_empty() && target_pid != "local_hermes" {
            Some(target_pid)
        } else {
            state
                .providers
                .iter()
                .find(|p| p.is_active && p.is_selectable())
                .or_else(|| {
                    state
                        .providers
                        .iter()
                        .find(|p| p.provider_id != "local_hermes" && p.is_selectable())
                })
                .map(|p| p.provider_id.clone())
        };

        if let Some(pid) = target_provider_id {
            let req = MagpieSaveProviderRequest {
                provider_id: Some(pid),
                default_model: Some(trimmed.clone()),
                activate: true,
                ..Default::default()
            };
            self.spawn_magpie_state_mutation(
                "已切换默认模型",
                "Active model updated",
                false,
                move |c, tok, epoch| async move {
                    c.hermes_magpie_save_provider_with_token(&tok, epoch, &req)
                        .await
                        .map(|r| r.state)
                        .map_err(|e| e.to_string())
                },
                cx,
            );
            if let Some(cid) = chat_id {
                self.show_chat_toast(
                    cid,
                    format!("{}: {trimmed}", tr_str(&lang, "已切换模型", "Model")),
                    1800,
                    cx,
                );
            }
        } else {
            self.sync_model_picker_to_stage(cx);
            self.open_ai_providers_settings(cx);
        }
    }

    /// Applies fetched `MagpieStateDto` and optional explicit `SubscriptionAccountDto` records to desktop views.
    pub(crate) fn apply_magpie_state_and_subscriptions_to_views(
        &mut self,
        state: MagpieStateDto,
        subs: Option<Vec<SubscriptionAccountDto>>,
        cx: &mut Context<Self>,
    ) {
        for s in &state.subscriptions {
            if !s.is_selectable() {
                let reason = s.unavailable_reason.clone().unwrap_or_default();
                let code = s
                    .unavailable_code
                    .clone()
                    .unwrap_or_else(|| classify_provider_auth_error_code(&reason).to_string());
                self.magpie_local
                    .discovered_health
                    .insert(s.id.clone(), LocalSubscriptionHealth::unavailable(code, reason));
            } else if self
                .magpie_local
                .discovered_health
                .get(&s.id)
                .is_some_and(|h| !h.usable)
            {
                self.magpie_local
                    .discovered_health
                    .insert(s.id.clone(), LocalSubscriptionHealth::usable());
            }
        }
        if let Some(ref s_list) = subs {
            for s in s_list {
                if !s.is_selectable() {
                    let reason = s.unavailable_reason.clone().unwrap_or_default();
                    let code = s
                        .unavailable_code
                        .clone()
                        .unwrap_or_else(|| classify_provider_auth_error_code(&reason).to_string());
                    self.magpie_local
                        .discovered_health
                        .insert(s.id.clone(), LocalSubscriptionHealth::unavailable(code, reason));
                } else if self
                    .magpie_local
                    .discovered_health
                    .get(&s.id)
                    .is_some_and(|h| !h.usable)
                {
                    self.magpie_local
                        .discovered_health
                        .insert(s.id.clone(), LocalSubscriptionHealth::usable());
                }
            }
        }
        self.sync_local_health_to_fabushi();
        let lang = self.active_lang_id().to_string();
        let (summary, needs_sel) = summarize_magpie_for_main(&state, &lang);
        let local_dtos = self
            .magpie_local
            .cached_dtos(Some(state.active_provider.as_str()));
        self.magpie_local.latest_state = Some(state.clone());
        if let Some(ref s) = subs {
            self.magpie_local.server_subscriptions = s.clone();
        }
        let inspector_catalog = InspectorAiModelCatalog::from_magpie_state(
            &state,
            &self.magpie_local.server_subscriptions,
            &local_dtos,
        );
        let catalog_for_creator = inspector_catalog.clone();
        let state_for_ws = state.clone();
        let state_for_pickers = state.clone();
        let server_subs_for_pickers = self.magpie_local.server_subscriptions.clone();
        let local_dtos_for_pickers = local_dtos.clone();
        let detected_coding = self.coding_host.detected_providers.clone();
        self.settings_router.update(cx, |sr, scx| {
            if let Some(ref ap) = sr.settings_ai_providers {
                ap.update(scx, |view, vcx| {
                    if let Some(s) = subs {
                        view.sync_subscriptions(s);
                    }
                    view.sync_local_subscriptions(local_dtos);
                    view.sync_state(state.clone());
                    vcx.notify();
                });
            }
            if let Some(ref sm) = sr.settings_main {
                sm.update(scx, |main, mcx| {
                    main.sync_ai_providers_status(summary.clone(), needs_sel);
                    mcx.notify();
                });
            }
        });
        self.tasks_workspace.update(cx, |ws, wcx| {
            ws.sync_magpie_state(state_for_ws);
            wcx.notify();
        });
        self.inspector.update(cx, |insp, icx| {
            insp.sync_ai_models(inspector_catalog);
            icx.notify();
        });
        if let Some(ref creator) = self.miniapps.create_bot_box.clone() {
            creator.update(cx, |b, bcx| {
                b.sync_ai_models(catalog_for_creator);
                b.sync_magpie_catalog(
                    &state_for_pickers,
                    &server_subs_for_pickers,
                    &local_dtos_for_pickers,
                );
                bcx.notify();
            });
        }
        self.sync_model_picker_to_stage(cx);
        self.stage.update(cx, |st, stcx| {
            st.sync_magpie_model_catalog(
                &state_for_pickers,
                &server_subs_for_pickers,
                &local_dtos_for_pickers,
                &detected_coding,
            );
            stcx.notify();
        });
        self.sync_provider_recovery_progress_to_views(cx);
        cx.notify();
    }

    /// Pushes cached Settings/Magpie AI model catalog into `ContextInspector`, `ConversationStage`,
    /// and `CreateManagedBotBoxPeers`, or fetches it if absent.
    pub fn sync_cached_ai_models_to_inspector(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = self.magpie_local.latest_state.clone() {
            let local_dtos = self
                .magpie_local
                .cached_dtos(Some(state.active_provider.as_str()));
            let catalog = InspectorAiModelCatalog::from_magpie_state(
                &state,
                &self.magpie_local.server_subscriptions,
                &local_dtos,
            );
            let catalog_for_creator = catalog.clone();
            self.inspector.update(cx, |insp, icx| {
                insp.sync_ai_models(catalog);
                icx.notify();
            });
            if let Some(ref creator) = self.miniapps.create_bot_box {
                creator.update(cx, |b, bcx| {
                    b.sync_ai_models(catalog_for_creator);
                    bcx.notify();
                });
            }
            self.sync_model_picker_to_stage(cx);
        } else if self.is_authenticated {
            self.sync_model_picker_to_stage(cx);
            self.refresh_magpie_state(cx);
        } else {
            self.sync_model_picker_to_stage(cx);
        }
    }

    /// Applies fetched `MagpieStateDto` to `SettingsAiProvidersSections`, `SettingsMainSections`, and `GlobalTasksWorkspace`.
    pub(crate) fn apply_magpie_state_to_views(
        &mut self,
        state: MagpieStateDto,
        cx: &mut Context<Self>,
    ) {
        self.apply_magpie_state_and_subscriptions_to_views(state, None, cx);
    }

    /// Fetches `/api/v1/hermes/magpie/state` and `/api/v1/hermes/magpie/subscriptions`, while
    /// refreshing client-side local AI subscription discovery on the desktop machine.
    pub fn refresh_magpie_state(&mut self, cx: &mut Context<Self>) {
        let Some(fence) = magpie_fence(self) else {
            return;
        };
        let token = fence.token.clone();
        let expected_epoch = fence.client_epoch;
        let base_url = fence.client.http_client.base_url.clone();
        let client = fence.client.clone();
        let fence_pre = fence.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence_pre.preflight_ui(&weak, app).await {
                return;
            }
            let scan_base = base_url.clone();
            let base_tok = base_url.clone();
            let tok_task = token.clone();
            let client_task = client.clone();
            let Ok(Some((local_items, local_health, res, subs_res))) = client
                .runtime()
                .spawn(async move {
                    let mut local_items = tokio::task::spawn_blocking(move || {
                        discover_local_subscriptions_for_server(&scan_base)
                    })
                    .await
                    .unwrap_or_default();
                    if client_task
                        .ensure_pinned_session(expected_epoch, &tok_task)
                        .is_err()
                    {
                        return None;
                    }
                    if let Ok(cat) = fetch_live_models_catalog_pinned(
                        &client_task,
                        expected_epoch,
                        &base_tok,
                        &tok_task,
                        false,
                    )
                    .await
                    {
                        enrich_items_with_live_catalog(&mut local_items, &cat);
                    }
                    let local_health = verify_discovered_subscriptions(&local_items).await;
                    if client_task
                        .ensure_pinned_session(expected_epoch, &tok_task)
                        .is_err()
                    {
                        return None;
                    }
                    let res = client_task
                        .hermes_magpie_state_with_token(&tok_task, expected_epoch)
                        .await;
                    let subs_res =
                        http_fetch_subscriptions(&client_task, expected_epoch, &base_tok, &tok_task)
                            .await;
                    Some((local_items, local_health, res, subs_res))
                })
                .await
            else {
                return;
            };
            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                shell.magpie_local.discovered_items = local_items;
                shell.magpie_local.discovered_health = local_health;
                match res {
                    Ok(state) => {
                        let subs = subs_res.ok();
                        shell.apply_magpie_state_and_subscriptions_to_views(state, subs, cx);
                    }
                    Err(e) => {
                        let msg = format!("AI provider sync error: {e}");
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    view.set_status(None, Some(msg.clone()));
                                    vcx.notify();
                                });
                            }
                        });
                    }
                }
            });
        })
        .detach();
    }

    /// Automatically runs once per authenticated session after startup/login: discovers local AI
    /// subscriptions on this desktop machine, syncs them to the backend server, and auto-activates
    /// the best available subscription if no real provider is active yet so the app works out-of-the-box.
    pub fn bootstrap_local_subscriptions_on_startup(&mut self, cx: &mut Context<Self>) {
        let Some(fence) = magpie_fence(self) else {
            return;
        };
        if self.magpie_local.bootstrapped_auth_generation == Some(fence.auth_generation) {
            return;
        }
        self.magpie_local.bootstrapped_auth_generation = Some(fence.auth_generation);

        let token = fence.token.clone();
        let expected_epoch = fence.client_epoch;
        let base_url = fence.client.http_client.base_url.clone();
        let client = fence.client.clone();

        let fence_pre = fence.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence_pre.preflight_ui(&weak, app).await {
                return;
            }
            let scan_base = base_url.clone();
            let base_tok = base_url.clone();
            let tok_task = token.clone();
            let client_task = client.clone();
            let Ok(Some((outcome, subs_res))) = client
                .runtime()
                .spawn(async move {
                    let mut local_items = tokio::task::spawn_blocking(move || {
                        discover_local_subscriptions_for_server(&scan_base)
                    })
                    .await
                    .unwrap_or_default();
                    if client_task
                        .ensure_pinned_session(expected_epoch, &tok_task)
                        .is_err()
                    {
                        return None;
                    }
                    if let Ok(cat) = fetch_live_models_catalog_pinned(
                        &client_task,
                        expected_epoch,
                        &base_tok,
                        &tok_task,
                        false,
                    )
                    .await
                    {
                        enrich_items_with_live_catalog(&mut local_items, &cat);
                    }

                    let outcome = sync_local_subscriptions_to_server_pinned(
                        &client_task,
                        expected_epoch,
                        &base_tok,
                        &tok_task,
                        local_items,
                        true,
                    )
                    .await
                    .ok()?;
                    let subs_res =
                        http_fetch_subscriptions(&client_task, expected_epoch, &base_tok, &tok_task)
                            .await;
                    Some((outcome, subs_res))
                })
                .await
            else {
                return;
            };

            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                shell.magpie_local.discovered_items = outcome.local_items;
                shell.magpie_local.discovered_health = outcome.local_health;
                if let Some(state) = outcome.state {
                    let subs = subs_res.ok();
                    shell.apply_magpie_state_and_subscriptions_to_views(state, subs, cx);
                } else {
                    shell.sync_local_health_to_fabushi();
                }
                if let Some(act_id) = outcome.auto_activated_id {
                    if outcome.used_legacy_fallback {
                        shell.schedule_legacy_token_keepalive(
                            act_id.clone(),
                            outcome.auto_activated_model.clone(),
                            cx,
                        );
                    }
                    let lang = shell.active_lang_id().to_string();
                    let display = outcome.auto_activated_label.unwrap_or(act_id);
                    let toast = if lang.starts_with("zh") {
                        format!("已自动发现并接入本机 AI 订阅：{display}")
                    } else {
                        format!("Auto-connected local AI subscription: {display}")
                    };
                    shell.show_non_chat_toast(toast, 3000, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Starts a lightweight background keep-alive refresher when an OAuth-backed local subscription
    /// is synced to a legacy v1.3.0 server (which does not store refresh tokens server-side).
    pub(crate) fn schedule_legacy_token_keepalive(
        &mut self,
        sub_id: String,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(fence) = magpie_fence(self) else {
            return;
        };
        self.magpie_local.legacy_keepalive_generation = self
            .magpie_local
            .legacy_keepalive_generation
            .wrapping_add(1);
        let keepalive_gen = self.magpie_local.legacy_keepalive_generation;
        self.magpie_local.active_legacy_subscription_id = Some(sub_id.clone());
        self.magpie_local.active_legacy_model = model.clone();

        let token = fence.token.clone();
        let expected_epoch = fence.client_epoch;
        let base_url = fence.client.http_client.base_url.clone();
        let client = fence.client.clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            loop {
                app.background_executor()
                    .timer(Duration::from_secs(40 * 60))
                    .await;
                let still_valid = weak
                    .update(app, |shell, _| {
                        fence.is_valid(shell)
                            && shell.magpie_local.legacy_keepalive_generation == keepalive_gen
                            && shell
                                .magpie_local
                                .active_legacy_subscription_id
                                .as_deref()
                                == Some(sub_id.as_str())
                    })
                    .unwrap_or(false);
                if !still_valid {
                    break;
                }
                let client_task = client.clone();
                let base_tok = base_url.clone();
                let tok_task = token.clone();
                let sub_task = sub_id.clone();
                let model_task = model.clone();
                let _ = client
                    .runtime()
                    .spawn(async move {
                        refresh_legacy_provider_token_once_pinned(
                            &client_task,
                            expected_epoch,
                            &base_tok,
                            &tok_task,
                            &sub_task,
                            model_task.as_deref(),
                        )
                        .await
                    })
                    .await;
            }
        })
        .detach();
    }

    fn spawn_magpie_state_mutation<F>(
        &mut self,
        zh_ok: &'static str,
        en_ok: &'static str,
        close_editor_on_ok: bool,
        fut_fn: impl FnOnce(std::sync::Arc<gpui_telegram_client::TelegramClient>, String, u64) -> F
            + 'static,
        cx: &mut Context<Self>,
    ) where
        F: Future<Output = Result<MagpieStateDto, String>> + 'static,
    {
        let Some(fence) = magpie_fence(self) else {
            return;
        };
        let token = fence.token.clone();
        let expected_epoch = fence.client_epoch;
        let client = fence.client.clone();
        let fence_pre = fence.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence_pre.preflight_ui(&weak, app).await {
                return;
            }
            let res = fut_fn(client, token, expected_epoch).await;
            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                let lang = shell.active_lang_id().to_string();
                match res {
                    Ok(state) => {
                        let status_msg = tr_str(&lang, zh_ok, en_ok).to_string();
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    if close_editor_on_ok {
                                        view.editor_open = false;
                                        view.active_field = crate::views::settings::sections::AiProviderField::None;
                                    }
                                    view.set_status(Some(status_msg.clone()), None);
                                    vcx.notify();
                                });
                            }
                        });
                        shell.apply_magpie_state_to_views(state, cx);
                        shell.show_non_chat_toast(status_msg, 2400, cx);
                        shell.refresh_tasks_and_mcp(cx);
                    }
                    Err(err) => {
                        let msg = format!("{}: {err}", tr_str(&lang, "操作失败", "Action failed"));
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    view.set_status(None, Some(msg.clone()));
                                    vcx.notify();
                                });
                            }
                        });
                        shell.refresh_magpie_state(cx);
                        shell.sync_model_picker_to_stage(cx);
                        shell.show_non_chat_toast(msg, 2800, cx);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(crate) fn run_magpie_subscription_scan(&mut self, cx: &mut Context<Self>) {
        let Some(fence) = magpie_fence(self) else {
            return;
        };
        let token = fence.token.clone();
        let expected_epoch = fence.client_epoch;
        let base_url = fence.client.http_client.base_url.clone();
        let client = fence.client.clone();
        let fence_pre = fence.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence_pre.preflight_ui(&weak, app).await {
                return;
            }
            let scan_base = base_url.clone();
            let base_tok = base_url.clone();
            let tok_task = token.clone();
            let client_task = client.clone();
            let Ok(Some((server_scan_count, outcome, subs_res, state_opt))) = client
                .runtime()
                .spawn(async move {
                    let mut local_items = tokio::task::spawn_blocking(move || {
                        discover_local_subscriptions_for_server(&scan_base)
                    })
                    .await
                    .unwrap_or_default();
                    if client_task
                        .ensure_pinned_session(expected_epoch, &tok_task)
                        .is_err()
                    {
                        return None;
                    }
                    if let Ok(cat) = fetch_live_models_catalog_pinned(
                        &client_task,
                        expected_epoch,
                        &base_tok,
                        &tok_task,
                        true,
                    )
                    .await
                    {
                        enrich_items_with_live_catalog(&mut local_items, &cat);
                    }

                    if client_task
                        .ensure_pinned_session(expected_epoch, &tok_task)
                        .is_err()
                    {
                        return None;
                    }
                    let is_local_server = magpie_gateway::is_loopback_server_url(&base_tok);
                    let server_scan_count = if is_local_server {
                        client_task
                            .hermes_magpie_scan_subscriptions_with_token(&tok_task, expected_epoch)
                            .await
                            .map(|r| {
                                r.subscriptions
                                    .iter()
                                    .filter(|s| s.active && s.is_selectable())
                                    .count()
                            })
                            .unwrap_or(0)
                    } else {
                        0
                    };

                    let outcome = sync_local_subscriptions_to_server_pinned(
                        &client_task,
                        expected_epoch,
                        &base_tok,
                        &tok_task,
                        local_items,
                        true,
                    )
                    .await
                    .ok()?;
                    let subs_res =
                        http_fetch_subscriptions(&client_task, expected_epoch, &base_tok, &tok_task)
                            .await;
                    let state_opt = match outcome.state {
                        Some(ref st) => Some(st.clone()),
                        None => client_task
                            .hermes_magpie_state_with_token(&tok_task, expected_epoch)
                            .await
                            .ok(),
                    };
                    Some((server_scan_count, outcome, subs_res, state_opt))
                })
                .await
            else {
                return;
            };

            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                let local_usable = outcome
                    .local_items
                    .iter()
                    .filter(|s| {
                        s.account.active
                            && outcome
                                .local_health
                                .get(&s.account.id)
                                .is_none_or(|h| h.usable)
                    })
                    .count();
                let local_unavailable = outcome
                    .local_items
                    .iter()
                    .filter(|s| {
                        outcome
                            .local_health
                            .get(&s.account.id)
                            .is_some_and(|h| !h.usable)
                    })
                    .count();
                let total_found = local_usable.max(server_scan_count);
                shell.magpie_local.discovered_items = outcome.local_items;
                shell.magpie_local.discovered_health = outcome.local_health;

                let lang = shell.active_lang_id().to_string();
                let toast = if let Some(ref act_id) = outcome.auto_activated_id {
                    let display = outcome
                        .auto_activated_label
                        .as_deref()
                        .unwrap_or(act_id.as_str());
                    if lang.starts_with("zh") {
                        format!("扫描完成：发现 {total_found} 个可用本机凭据，已自动激活「{display}」")
                    } else {
                        format!("Scan complete: found {total_found} usable local credential(s), auto-activated \"{display}\"")
                    }
                } else if total_found > 0 {
                    if lang.starts_with("zh") {
                        format!("扫描完成：检测到 {total_found} 个可用本机 AI 订阅凭据")
                    } else {
                        format!("Scan complete: detected {total_found} usable local AI credential(s)")
                    }
                } else if local_unavailable > 0 {
                    if lang.starts_with("zh") {
                        format!(
                            "扫描完成：检测到 {local_unavailable} 个本机凭据但均已失效/不可用，请重新登录或切换其他服务商"
                        )
                    } else {
                        format!(
                            "Scan complete: {local_unavailable} local credential(s) found but expired/invalid — please re-login or choose another provider"
                        )
                    }
                } else if let Some(ref warn) = outcome.warning {
                    format!("Subscription sync warning: {warn}")
                } else {
                    tr_str(
                        &lang,
                        "扫描完成：未发现已登录的本地凭据，请在下方选择供应商预设",
                        "Scan complete: no active local credentials found — select a provider preset below",
                    )
                    .to_string()
                };

                if let Some(state) = state_opt {
                    let subs = subs_res.ok();
                    shell.apply_magpie_state_and_subscriptions_to_views(state, subs, cx);
                } else {
                    shell.sync_local_health_to_fabushi();
                }
                if let Some(act_id) = outcome.auto_activated_id {
                    if outcome.used_legacy_fallback {
                        shell.schedule_legacy_token_keepalive(
                            act_id,
                            outcome.auto_activated_model,
                            cx,
                        );
                    }
                }
                shell.settings_router.update(cx, |sr, scx| {
                    if let Some(ref ap) = sr.settings_ai_providers {
                        ap.update(scx, |view, vcx| {
                            if total_found == 0 && local_unavailable > 0 {
                                view.set_status(None, Some(toast.clone()));
                            } else {
                                view.set_status(Some(toast.clone()), None);
                            }
                            vcx.notify();
                        });
                    }
                });
                shell.show_non_chat_toast(toast, 3000, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Activates a discovered local AI subscription as the primary provider.
    pub(crate) fn activate_subscription(
        &mut self,
        subscription_id: String,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(fence) = magpie_fence(self) else {
            return;
        };
        let token = fence.token.clone();
        let expected_epoch = fence.client_epoch;
        let base_url = fence.client.http_client.base_url.clone();
        let client = fence.client.clone();
        let sub_id = subscription_id.clone();
        let req_model = model.clone();
        let cached_local = self.magpie_local.discovered_items.clone();

        let fence_pre = fence.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence_pre.preflight_ui(&weak, app).await {
                return;
            }
            let client_task = client.clone();
            let base_tok = base_url.clone();
            let tok_task = token.clone();
            let sub_task = sub_id.clone();
            let Ok(Some((act_res, subs_res))) = client
                .runtime()
                .spawn(async move {
                    if client_task
                        .ensure_pinned_session(expected_epoch, &tok_task)
                        .is_err()
                    {
                        return None;
                    }
                    let act_res = activate_subscription_with_local_fallback_pinned(
                        &client_task,
                        expected_epoch,
                        &base_tok,
                        &tok_task,
                        &sub_task,
                        req_model.as_deref(),
                        &cached_local,
                    )
                    .await;
                    let subs_res =
                        http_fetch_subscriptions(&client_task, expected_epoch, &base_tok, &tok_task)
                            .await;
                    Some((act_res, subs_res))
                })
                .await
            else {
                return;
            };

            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                let lang = shell.active_lang_id().to_string();
                match act_res {
                    Ok(resp) => {
                        let display_name = shell
                            .magpie_local
                            .discovered_items
                            .iter()
                            .find(|i| i.account.id == resp.active_provider_id)
                            .map(|i| i.account.label.clone())
                            .or_else(|| {
                                resp.state.as_ref().and_then(|st| {
                                    st.providers
                                        .iter()
                                        .find(|p| p.provider_id == resp.active_provider_id)
                                        .map(|p| p.name.clone())
                                })
                            })
                            .unwrap_or_else(|| resp.active_provider_id.clone());
                        let toast = if lang.starts_with("zh") {
                            format!(
                                "已成功激活「{}」为主要模型供应商 (模型: {})",
                                display_name, resp.active_model
                            )
                        } else {
                            format!(
                                "Activated \"{}\" as primary provider ({})",
                                display_name, resp.active_model
                            )
                        };
                        if let Some(state) = resp.state {
                            let subs = subs_res.ok();
                            shell.apply_magpie_state_and_subscriptions_to_views(state, subs, cx);
                        }
                        if resp.used_legacy_fallback {
                            shell.schedule_legacy_token_keepalive(
                                resp.active_provider_id,
                                Some(resp.active_model),
                                cx,
                            );
                        }
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    view.set_status(Some(toast.clone()), None);
                                    vcx.notify();
                                });
                            }
                        });
                        shell.show_non_chat_toast(toast, 2800, cx);
                        shell.refresh_tasks_and_mcp(cx);
                    }
                    Err(err) => {
                        let code = classify_provider_auth_error_code(&err);
                        shell.magpie_local.discovered_health.insert(
                            sub_id.clone(),
                            LocalSubscriptionHealth::unavailable(code, err.clone()),
                        );
                        shell.sync_local_health_to_fabushi();
                        let msg = format!(
                            "{}: {err}",
                            tr_str(&lang, "激活智能体失败", "Failed to activate AI agent")
                        );
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    view.set_status(None, Some(msg.clone()));
                                    vcx.notify();
                                });
                            }
                        });
                        shell.refresh_magpie_state(cx);
                        shell.sync_model_picker_to_stage(cx);
                        shell.show_non_chat_toast(msg, 3000, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn run_magpie_probe(
        &mut self,
        provider_id: String,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(fence) = magpie_fence(self) else {
            return;
        };
        let token = fence.token.clone();
        let expected_epoch = fence.client_epoch;
        let client = fence.client.clone();
        let fence_pre = fence.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence_pre.preflight_ui(&weak, app).await {
                return;
            }
            let req = MagpieProbeRequest {
                model,
                prompt: Some("Reply with OK".to_string()),
            };
            let res = client
                .hermes_magpie_probe_provider_with_token(
                    &token,
                    expected_epoch,
                    &provider_id,
                    Some(&req),
                )
                .await;
            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                let lang = shell.active_lang_id().to_string();
                match res {
                    Ok(probe) => {
                        let toast = if probe.ok {
                            format!(
                                "Probe OK ({} · {}ms)",
                                probe.provider_id, probe.latency_ms
                            )
                        } else {
                            format!(
                                "{}: {}",
                                tr_str(&lang, "连通性测试失败", "Probe failed"),
                                probe.error.as_deref().unwrap_or("error")
                            )
                        };
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    view.loading = false;
                                    view.probe_result = Some(probe.clone());
                                    vcx.notify();
                                });
                            }
                        });
                        shell.refresh_magpie_state(cx);
                        shell.show_non_chat_toast(toast, 2800, cx);
                    }
                    Err(e) => {
                        let msg = format!("Probe error: {e}");
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    view.set_status(None, Some(msg.clone()));
                                    vcx.notify();
                                });
                            }
                        });
                        shell.refresh_magpie_state(cx);
                        shell.show_non_chat_toast(msg, 2800, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Live-fetches the latest models for a specific discovered subscription card from the
    /// upstream provider API + `models.dev`, updates the card immediately, and notifies the user.
    pub(crate) fn fetch_subscription_models_live(
        &mut self,
        subscription_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(fence) = magpie_fence(self) else {
            return;
        };
        let token = fence.token.clone();
        let expected_epoch = fence.client_epoch;
        let base_url = fence.client.http_client.base_url.clone();
        let cached = self.magpie_local.discovered_items.clone();
        let client = fence.client.clone();
        let target_id = subscription_id.clone();

        let fence_pre = fence.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence_pre.preflight_ui(&weak, app).await {
                return;
            }
            let tid_task = target_id.clone();
            let base_tok = base_url.clone();
            let tok_task = token.clone();
            let client_task = client.clone();
            let fetch_res = client
                .runtime()
                .spawn(async move {
                    let item_opt = cached
                        .into_iter()
                        .find(|i| i.account.id == tid_task)
                        .or_else(|| {
                            discover_local_subscriptions_for_server(&base_tok)
                                .into_iter()
                                .find(|i| i.account.id == tid_task)
                        });
                    if let Err(e) = client_task.ensure_pinned_session(expected_epoch, &tok_task) {
                        return Err(format!("Session changed before fetching models: {e}"));
                    }
                    match item_opt {
                        Some(item) => {
                            fetch_models_for_subscription_live_pinned(
                                &client_task,
                                expected_epoch,
                                &base_tok,
                                &tok_task,
                                &item,
                            )
                            .await
                        }
                        None => Err(format!("Subscription '{tid_task}' not found locally")),
                    }
                })
                .await
                .unwrap_or_else(|e| Err(format!("tokio join error: {e}")));

            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                let lang = shell.active_lang_id().to_string();
                match fetch_res {
                    Ok(outcome) => {
                        let count = outcome.models.len();
                        if let Some(entry) = shell
                            .magpie_local
                            .discovered_items
                            .iter_mut()
                            .find(|i| i.account.id == target_id)
                        {
                            entry.available_models = outcome.models.clone();
                        }
                        let active_pid = shell
                            .settings_router
                            .read(cx)
                            .settings_ai_providers
                            .as_ref()
                            .and_then(|ap| {
                                ap.read(cx)
                                    .state
                                    .as_ref()
                                    .map(|s| s.active_provider.clone())
                            });
                        let local_dtos = shell
                            .magpie_local
                            .cached_dtos(active_pid.as_deref());
                        let toast = if lang.starts_with("zh") {
                            format!(
                                "已从 {} 获取 {count} 个最新模型",
                                outcome.source_label_zh
                            )
                        } else {
                            format!(
                                "Fetched {count} live models from {}",
                                outcome.source_label_en
                            )
                        };
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    view.sync_local_subscriptions(local_dtos);
                                    view.set_status(Some(toast.clone()), None);
                                    vcx.notify();
                                });
                            }
                        });
                        shell.sync_cached_ai_models_to_inspector(cx);
                        shell.show_non_chat_toast(toast, 2800, cx);
                    }
                    Err(err) => {
                        let msg = if lang.starts_with("zh") {
                            format!("获取模型失败: {err}")
                        } else {
                            format!("Failed to fetch models: {err}")
                        };
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    view.set_status(None, Some(msg.clone()));
                                    vcx.notify();
                                });
                            }
                        });
                        shell.show_non_chat_toast(msg, 3000, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Pushes active re-authentication and refresh-in-progress state to Settings, ConversationStage,
    /// and CreateManagedBotBoxPeers so every UI surface reflects live recovery progress.
    pub(crate) fn sync_provider_recovery_progress_to_views(&mut self, cx: &mut Context<Self>) {
        let is_zh = self.active_lang_id().starts_with("zh");
        let reauth_pid = self
            .magpie_local
            .reauth_in_progress
            .as_ref()
            .map(|r| r.provider_id.clone());
        let reauth_msg = self.magpie_local.reauth_in_progress.as_ref().map(|r| {
            if is_zh {
                r.status_zh.clone()
            } else {
                r.status_en.clone()
            }
        });
        let refreshing_set = self.magpie_local.refreshing_provider_ids.clone();
        let refreshing_vec: Vec<String> = refreshing_set.iter().cloned().collect();

        let reauth_pid_settings = reauth_pid.clone();
        let reauth_msg_settings = reauth_msg.clone();
        self.settings_router.update(cx, |sr, scx| {
            if let Some(ref ap) = sr.settings_ai_providers {
                ap.update(scx, |view, vcx| {
                    view.sync_recovery_progress(
                        reauth_pid_settings,
                        reauth_msg_settings,
                        refreshing_set,
                    );
                    vcx.notify();
                });
            }
        });

        let reauth_pid_stage = reauth_pid.clone();
        let refreshing_stage = refreshing_vec.clone();
        self.stage.update(cx, |st, stcx| {
            st.sync_provider_recovery_progress(reauth_pid_stage, refreshing_stage);
            stcx.notify();
        });

        if let Some(ref creator) = self.miniapps.create_bot_box.clone() {
            creator.update(cx, |b, bcx| {
                b.sync_provider_recovery_progress(reauth_pid, refreshing_vec);
                bcx.notify();
            });
        }
    }

    /// Opens Settings directly to `SettingsSectionType::AiProviders` focused on recovering `provider_id`.
    pub fn open_ai_provider_for_recovery(&mut self, provider_id: String, cx: &mut Context<Self>) {
        self.navigate_settings_to(SettingsSectionType::AiProviders, cx);
        let pid = provider_id.trim().to_string();
        if !pid.is_empty() {
            self.settings_router.update(cx, |sr, scx| {
                if let Some(ref ap) = sr.settings_ai_providers {
                    ap.update(scx, |view, vcx| {
                        view.open_provider_for_recovery(&pid);
                        vcx.notify();
                    });
                }
            });
        }
        self.refresh_magpie_state(cx);
    }

    /// Cancels any active interactive browser/CLI re-authentication session.
    pub fn cancel_provider_reauth(&mut self, cx: &mut Context<Self>) {
        let cancelled = self.magpie_local.cancel_active_reauth();
        self.sync_provider_recovery_progress_to_views(cx);
        if cancelled {
            let lang = self.active_lang_id().to_string();
            let msg = tr_str(&lang, "已取消重新登录", "Sign-in cancelled").to_string();
            self.settings_router.update(cx, |sr, scx| {
                if let Some(ref ap) = sr.settings_ai_providers {
                    ap.update(scx, |view, vcx| {
                        view.set_status(Some(msg.clone()), None);
                        vcx.notify();
                    });
                }
            });
            self.show_non_chat_toast(msg, 2200, cx);
        }
        cx.notify();
    }

    /// Launches interactive browser OAuth2 PKCE or managed CLI re-login for a subscription provider
    /// (Codex/ChatGPT, Google Vertex AI, Claude Code, GitHub Copilot, Gemini), then automatically
    /// re-discovers credentials, verifies live availability, hydrates local Fabushi, and activates it.
    pub fn reauthenticate_provider_or_subscription(
        &mut self,
        provider_id: String,
        kind_hint: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = infer_subscription_kind(&provider_id, kind_hint.as_deref()) else {
            self.open_ai_provider_for_recovery(provider_id, cx);
            return;
        };
        let Some(fence) = magpie_fence(self) else {
            return;
        };
        let Some(home) = resolve_local_home_dir() else {
            let lang = self.active_lang_id().to_string();
            self.show_non_chat_toast(
                tr_str(
                    &lang,
                    "无法定位用户主目录，请在终端完成登录后点击「刷新激活」",
                    "Cannot resolve home directory — sign in via terminal then click Refresh",
                )
                .to_string(),
                3000,
                cx,
            );
            return;
        };

        let display_name = match kind {
            SubscriptionKind::CodexChatGpt => "ChatGPT / Codex",
            SubscriptionKind::GoogleVertex => "Google Vertex AI",
            SubscriptionKind::ClaudeCode => "Claude Code",
            SubscriptionKind::GitHubCopilot => "GitHub Copilot",
            SubscriptionKind::GeminiCodeAssist => "Gemini Code Assist",
            SubscriptionKind::Antigravity => "Antigravity",
            SubscriptionKind::GrokSuperGrok => "SuperGrok",
        }
        .to_string();

        let _ = self.magpie_local.cancel_active_reauth();
        self.magpie_local.reauth_generation = self.magpie_local.reauth_generation.wrapping_add(1);
        let reauth_gen = self.magpie_local.reauth_generation;
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);

        let status_zh = format!("正在浏览器中登录 {display_name}，完成授权后将自动刷新并激活…");
        let status_en = format!(
            "Waiting for {display_name} sign-in in your browser — will auto-verify and activate…"
        );
        self.magpie_local.reauth_in_progress = Some(ActiveProviderReauth {
            provider_id: provider_id.clone(),
            display_name: display_name.clone(),
            status_zh: status_zh.clone(),
            status_en: status_en.clone(),
            generation: reauth_gen,
            cancel_tx,
        });
        self.sync_provider_recovery_progress_to_views(cx);

        let lang = self.active_lang_id().to_string();
        let start_toast = if lang.starts_with("zh") {
            status_zh.clone()
        } else {
            status_en.clone()
        };
        self.settings_router.update(cx, |sr, scx| {
            if let Some(ref ap) = sr.settings_ai_providers {
                ap.update(scx, |view, vcx| {
                    view.set_status(Some(start_toast.clone()), None);
                    vcx.notify();
                });
            }
        });
        self.show_non_chat_toast(start_toast, 3000, cx);

        let token = fence.token.clone();
        let expected_epoch = fence.client_epoch;
        let base_url = fence.client.http_client.base_url.clone();
        let client = fence.client.clone();
        let target_pid = provider_id.clone();
        let fence_pre = fence.clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence_pre.preflight_ui(&weak, app).await {
                return;
            }
            let client_task = client.clone();
            let base_tok = base_url.clone();
            let tok_task = token.clone();
            let pid_task = target_pid.clone();
            let Ok(reauth_res) = client
                .runtime()
                .spawn(async move {
                    let login_ok =
                        run_interactive_subscription_reauth(kind, &home, cancel_rx).await?;
                    // Discover ALL local subscriptions (including non-cross-device Codex/Claude)
                    // so local Fabushi immediately picks up the freshly persisted auth file.
                    let all_local_items = tokio::task::spawn_blocking(
                        discover_all_local_subscriptions_in_home,
                    )
                    .await
                    .unwrap_or_default();
                    let all_local_health = verify_discovered_subscriptions(&all_local_items).await;
                    let refresh_res = refresh_and_activate_provider_or_subscription_pinned(
                        &client_task,
                        expected_epoch,
                        &base_tok,
                        &tok_task,
                        &pid_task,
                        Some(kind),
                        None,
                    )
                    .await;
                    let subs_res = http_fetch_subscriptions(
                        &client_task,
                        expected_epoch,
                        &base_tok,
                        &tok_task,
                    )
                    .await
                    .ok();
                    let fallback_state = if refresh_res
                        .as_ref()
                        .ok()
                        .and_then(|o| o.state.as_ref())
                        .is_none()
                    {
                        client_task
                            .hermes_magpie_state_with_token(&tok_task, expected_epoch)
                            .await
                            .ok()
                    } else {
                        None
                    };
                    Ok::<_, ReauthError>((
                        login_ok,
                        all_local_items,
                        all_local_health,
                        refresh_res,
                        subs_res,
                        fallback_state,
                    ))
                })
                .await
            else {
                let _ = weak.update(app, |shell, cx| {
                    if shell
                        .magpie_local
                        .reauth_in_progress
                        .as_ref()
                        .is_some_and(|r| r.generation == reauth_gen)
                    {
                        shell.magpie_local.reauth_in_progress = None;
                        shell.sync_provider_recovery_progress_to_views(cx);
                    }
                });
                return;
            };

            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                let is_current_gen = shell
                    .magpie_local
                    .reauth_in_progress
                    .as_ref()
                    .is_some_and(|r| r.generation == reauth_gen);
                if is_current_gen {
                    shell.magpie_local.reauth_in_progress = None;
                }

                let lang = shell.active_lang_id().to_string();
                let is_zh = lang.starts_with("zh");
                match reauth_res {
                    Ok((
                        login_ok,
                        all_local_items,
                        all_local_health,
                        refresh_res,
                        subs_opt,
                        fallback_state,
                    )) => {
                        // Immediately hydrate local Fabushi with fresh local credentials (including Codex/Claude).
                        if let Some(ref host) = shell.bot_browser.local_fabushi {
                            let uid_str = shell.session_sync.user_id.map(|u| u.to_string());
                            hydrate_local_subscriptions_into_fabushi_for_user(
                                host,
                                &all_local_items,
                                uid_str.as_deref(),
                            );
                            apply_local_subscription_health_to_fabushi_for_user(
                                host,
                                &all_local_health,
                                uid_str.as_deref(),
                            );
                            host.sync_all_providers();
                        }

                        // Check if local login produced a verified usable subscription matching `kind`
                        // (important when connected to a remote server that doesn't accept Codex OAuth sync).
                        let local_matched_usable = all_local_items.iter().find(|item| {
                            item.account.kind == kind
                                && all_local_health
                                    .get(&item.account.id)
                                    .is_none_or(|h| h.usable)
                        });

                        let (status_ok, status_err) = match refresh_res {
                            Ok(outcome) => {
                                shell.magpie_local.discovered_items = outcome.local_items;
                                shell.magpie_local.discovered_health = outcome.local_health;
                                for (k, v) in &all_local_health {
                                    if v.usable {
                                        shell.magpie_local.discovered_health.insert(k.clone(), v.clone());
                                    }
                                }
                                if let Some(state) = outcome.state.or(fallback_state) {
                                    shell.apply_magpie_state_and_subscriptions_to_views(
                                        state, subs_opt, cx,
                                    );
                                } else {
                                    shell.sync_local_health_to_fabushi();
                                }
                                if outcome.usable && outcome.activated {
                                    if outcome.used_legacy_fallback {
                                        shell.schedule_legacy_token_keepalive(
                                            outcome.provider_id.clone(),
                                            outcome.active_model.clone(),
                                            cx,
                                        );
                                    }
                                    let mdl_suffix = outcome
                                        .active_model
                                        .as_deref()
                                        .map(|m| format!(" ({m})"))
                                        .unwrap_or_default();
                                    let ok_msg = if is_zh {
                                        format!("重新登录成功，已自动激活「{display_name}」{mdl_suffix}")
                                    } else {
                                        format!("Signed in & activated \"{display_name}\"{mdl_suffix}")
                                    };
                                    (Some(ok_msg), None)
                                } else if local_matched_usable.is_some() {
                                    let ok_msg = if is_zh {
                                        format!("{}，本机服务已就绪", login_ok.detail_zh)
                                    } else {
                                        format!("{}, ready locally", login_ok.detail_en)
                                    };
                                    (Some(ok_msg), None)
                                } else {
                                    let reason = outcome.unavailable_reason.unwrap_or_else(|| {
                                        tr_str(
                                            &lang,
                                            "重新登录后校验仍未通过，请点击「刷新激活」重试",
                                            "Verification still failed after sign-in — click Refresh to retry",
                                        )
                                        .to_string()
                                    });
                                    (None, Some(reason))
                                }
                            }
                            Err(err) => {
                                if let Some(state) = fallback_state {
                                    shell.apply_magpie_state_and_subscriptions_to_views(
                                        state, subs_opt, cx,
                                    );
                                }
                                if local_matched_usable.is_some() {
                                    let ok_msg = if is_zh {
                                        format!("{}，本机凭据已更新并激活", login_ok.detail_zh)
                                    } else {
                                        format!("{}, local credentials updated", login_ok.detail_en)
                                    };
                                    (Some(ok_msg), None)
                                } else {
                                    let err_msg = if is_zh {
                                        format!("登录已完成，但同步激活失败：{err}")
                                    } else {
                                        format!("Sign-in succeeded, but activation failed: {err}")
                                    };
                                    (None, Some(err_msg))
                                }
                            }
                        };

                        let toast_msg = status_ok
                            .clone()
                            .or_else(|| status_err.clone())
                            .unwrap_or_default();
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    view.set_status(status_ok.clone(), status_err.clone());
                                    vcx.notify();
                                });
                            }
                        });
                        shell.sync_provider_recovery_progress_to_views(cx);
                        if !toast_msg.is_empty() {
                            shell.show_non_chat_toast(toast_msg, 3400, cx);
                        }
                        if status_ok.is_some() {
                            shell.refresh_tasks_and_mcp(cx);
                        }
                    }
                    Err(ReauthError::Cancelled) => {
                        let msg = tr_str(&lang, "已取消重新登录", "Sign-in cancelled").to_string();
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    view.set_status(Some(msg.clone()), None);
                                    vcx.notify();
                                });
                            }
                        });
                        shell.sync_provider_recovery_progress_to_views(cx);
                        shell.show_non_chat_toast(msg, 2000, cx);
                    }
                    Err(ReauthError::CliNotInstalled {
                        binary,
                        command_hint,
                    }) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(command_hint.clone()));
                        let msg = if is_zh {
                            format!(
                                "未检测到 `{binary}` CLI，已复制命令 `{command_hint}`，请安装并在终端登录后点击「刷新激活」"
                            )
                        } else {
                            format!(
                                "`{binary}` CLI not found. Copied `{command_hint}` — install, sign in, then click Refresh"
                            )
                        };
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    view.set_status(None, Some(msg.clone()));
                                    vcx.notify();
                                });
                            }
                        });
                        shell.sync_provider_recovery_progress_to_views(cx);
                        shell.show_non_chat_toast(msg, 4200, cx);
                    }
                    Err(other_err) => {
                        let cmd_suffix = cli_relogin_command_for_kind(kind)
                            .map(|c| format!(" (也可运行 `{c}` 后点击刷新)"))
                            .unwrap_or_default();
                        let msg = if is_zh {
                            format!("重新登录失败：{other_err}{cmd_suffix}")
                        } else {
                            format!("Sign-in failed: {other_err}")
                        };
                        shell.settings_router.update(cx, |sr, scx| {
                            if let Some(ref ap) = sr.settings_ai_providers {
                                ap.update(scx, |view, vcx| {
                                    view.set_status(None, Some(msg.clone()));
                                    vcx.notify();
                                });
                            }
                        });
                        shell.sync_provider_recovery_progress_to_views(cx);
                        shell.show_non_chat_toast(msg, 3800, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Force-refreshes credentials/connectivity for a provider or subscription, clears stale
    /// failure state when verified usable, and atomically activates it without app restart.
    pub fn refresh_and_activate_provider(
        &mut self,
        provider_id: String,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let pid = provider_id.trim().to_string();
        if pid.is_empty() {
            self.run_magpie_subscription_scan(cx);
            return;
        }
        let Some(fence) = magpie_fence(self) else {
            return;
        };
        let kind_hint = infer_subscription_kind(&pid, None);
        self.magpie_local.refreshing_provider_ids.insert(pid.clone());
        self.sync_provider_recovery_progress_to_views(cx);

        let token = fence.token.clone();
        let expected_epoch = fence.client_epoch;
        let base_url = fence.client.http_client.base_url.clone();
        let client = fence.client.clone();
        let req_model = model.clone();
        let fence_pre = fence.clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence_pre.preflight_ui(&weak, app).await {
                return;
            }
            let client_task = client.clone();
            let base_tok = base_url.clone();
            let tok_task = token.clone();
            let pid_task = pid.clone();
            let Ok(task_res) = client
                .runtime()
                .spawn(async move {
                    let all_local_items = tokio::task::spawn_blocking(
                        discover_all_local_subscriptions_in_home,
                    )
                    .await
                    .unwrap_or_default();
                    let all_local_health = verify_discovered_subscriptions(&all_local_items).await;
                    let refresh_res = refresh_and_activate_provider_or_subscription_pinned(
                        &client_task,
                        expected_epoch,
                        &base_tok,
                        &tok_task,
                        &pid_task,
                        kind_hint,
                        req_model.as_deref(),
                    )
                    .await;
                    let subs_res = http_fetch_subscriptions(
                        &client_task,
                        expected_epoch,
                        &base_tok,
                        &tok_task,
                    )
                    .await
                    .ok();
                    let fallback_state = if refresh_res
                        .as_ref()
                        .ok()
                        .and_then(|o| o.state.as_ref())
                        .is_none()
                    {
                        client_task
                            .hermes_magpie_state_with_token(&tok_task, expected_epoch)
                            .await
                            .ok()
                    } else {
                        None
                    };
                    (
                        all_local_items,
                        all_local_health,
                        refresh_res,
                        subs_res,
                        fallback_state,
                    )
                })
                .await
            else {
                let _ = weak.update(app, |shell, cx| {
                    shell.magpie_local.refreshing_provider_ids.remove(&pid);
                    shell.sync_provider_recovery_progress_to_views(cx);
                });
                return;
            };

            let _ = weak.update(app, |shell, cx| {
                shell.magpie_local.refreshing_provider_ids.remove(&pid);
                if !fence.is_valid(shell) {
                    return;
                }
                let (
                    all_local_items,
                    all_local_health,
                    refresh_res,
                    subs_opt,
                    fallback_state,
                ) = task_res;

                if let Some(ref host) = shell.bot_browser.local_fabushi {
                    let uid_str = shell.session_sync.user_id.map(|u| u.to_string());
                    hydrate_local_subscriptions_into_fabushi_for_user(
                        host,
                        &all_local_items,
                        uid_str.as_deref(),
                    );
                    apply_local_subscription_health_to_fabushi_for_user(
                        host,
                        &all_local_health,
                        uid_str.as_deref(),
                    );
                    host.sync_all_providers();
                }

                let lang = shell.active_lang_id().to_string();
                let is_zh = lang.starts_with("zh");
                let (status_ok, status_err) = match refresh_res {
                    Ok(outcome) => {
                        shell.magpie_local.discovered_items = outcome.local_items;
                        shell.magpie_local.discovered_health = outcome.local_health;
                        if let Some(state) = outcome.state.or(fallback_state) {
                            shell.apply_magpie_state_and_subscriptions_to_views(
                                state, subs_opt, cx,
                            );
                        } else {
                            shell.sync_local_health_to_fabushi();
                        }
                        if outcome.usable {
                            if outcome.used_legacy_fallback {
                                shell.schedule_legacy_token_keepalive(
                                    outcome.provider_id.clone(),
                                    outcome.active_model.clone(),
                                    cx,
                                );
                            }
                            let mdl = outcome
                                .active_model
                                .as_deref()
                                .map(|m| format!(" ({m})"))
                                .unwrap_or_default();
                            let msg = if outcome.activated {
                                if is_zh {
                                    format!("刷新成功，已重新激活「{}」{mdl}", outcome.provider_id)
                                } else {
                                    format!("Refreshed & activated \"{}\"{mdl}", outcome.provider_id)
                                }
                            } else if is_zh {
                                format!("「{}」校验通过，现已可用", outcome.provider_id)
                            } else {
                                format!("\"{}\" verified usable", outcome.provider_id)
                            };
                            (Some(msg), None)
                        } else {
                            let reason = outcome.unavailable_reason.unwrap_or_else(|| {
                                tr_str(
                                    &lang,
                                    "凭据或连接仍不可用，请重新登录或检查配置",
                                    "Credentials or connection still unavailable — please re-login or check config",
                                )
                                .to_string()
                            });
                            let msg = if is_zh {
                                format!("刷新失败：{reason}")
                            } else {
                                format!("Refresh failed: {reason}")
                            };
                            (None, Some(msg))
                        }
                    }
                    Err(err) => {
                        if let Some(state) = fallback_state {
                            shell.apply_magpie_state_and_subscriptions_to_views(
                                state, subs_opt, cx,
                            );
                        }
                        // If local subscription matching `pid` is usable in local Fabushi even on remote server
                        let local_ok = all_local_items.iter().any(|item| {
                            (item.account.id == pid || kind_hint == Some(item.account.kind))
                                && all_local_health
                                    .get(&item.account.id)
                                    .is_none_or(|h| h.usable)
                        });
                        if local_ok {
                            let msg = if is_zh {
                                format!("本机「{pid}」凭据已刷新并就绪")
                            } else {
                                format!("Local \"{pid}\" credentials refreshed and ready")
                            };
                            (Some(msg), None)
                        } else {
                            let msg = if is_zh {
                                format!("刷新激活失败：{err}")
                            } else {
                                format!("Refresh & activate failed: {err}")
                            };
                            (None, Some(msg))
                        }
                    }
                };

                let toast = status_ok
                    .clone()
                    .or_else(|| status_err.clone())
                    .unwrap_or_default();
                shell.settings_router.update(cx, |sr, scx| {
                    if let Some(ref ap) = sr.settings_ai_providers {
                        ap.update(scx, |view, vcx| {
                            view.set_status(status_ok.clone(), status_err.clone());
                            vcx.notify();
                        });
                    }
                });
                shell.sync_provider_recovery_progress_to_views(cx);
                if !toast.is_empty() {
                    shell.show_non_chat_toast(toast, 3200, cx);
                }
                if status_ok.is_some() {
                    shell.refresh_tasks_and_mcp(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Handles events emitted by `SettingsAiProvidersSections` via `SettingsRouter`.
    pub fn handle_ai_providers_settings_event(
        &mut self,
        ev: &SettingsAiProvidersEvent,
        cx: &mut Context<Self>,
    ) {
        match ev {
            SettingsAiProvidersEvent::RefreshState => self.refresh_magpie_state(cx),
            SettingsAiProvidersEvent::ScanSubscriptions => self.run_magpie_subscription_scan(cx),
            SettingsAiProvidersEvent::FetchSubscriptionModels(sub_id) => {
                self.fetch_subscription_models_live(sub_id.clone(), cx);
            }
            SettingsAiProvidersEvent::ActivateSubscription {
                subscription_id,
                model,
            } => {
                self.activate_subscription(subscription_id.clone(), model.clone(), cx);
            }
            SettingsAiProvidersEvent::ReauthenticateProvider {
                provider_id,
                kind_hint,
            } => {
                self.reauthenticate_provider_or_subscription(
                    provider_id.clone(),
                    kind_hint.clone(),
                    cx,
                );
            }
            SettingsAiProvidersEvent::CancelReauthenticate => {
                self.cancel_provider_reauth(cx);
            }
            SettingsAiProvidersEvent::RefreshAndActivateProvider { provider_id, model } => {
                self.refresh_and_activate_provider(provider_id.clone(), model.clone(), cx);
            }
            SettingsAiProvidersEvent::OpenExternalUrl(url) => {
                let lang = self.active_lang_id().to_string();
                if let Err(err) = open_external_url_in_system_browser(url) {
                    cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                    let msg = if lang.starts_with("zh") {
                        format!("打开浏览器失败（已复制链接）：{err}")
                    } else {
                        format!("Failed to open browser (URL copied): {err}")
                    };
                    self.show_non_chat_toast(msg, 3000, cx);
                }
            }
            SettingsAiProvidersEvent::CopyText(txt) => {
                cx.write_to_clipboard(ClipboardItem::new_string(txt.clone()));
                self.show_non_chat_toast(
                    format!("{}: {txt}", tr_str(self.active_lang_id(), "已复制", "Copied")),
                    2200,
                    cx,
                );
            }
            SettingsAiProvidersEvent::SaveProvider(req) => {
                let req = req.clone();
                self.spawn_magpie_state_mutation(
                    "AI 供应商已保存并激活",
                    "AI provider saved & activated",
                    true,
                    move |c, tok, epoch| async move {
                        c.hermes_magpie_save_provider_with_token(&tok, epoch, &req)
                            .await
                            .map(|r| r.state)
                            .map_err(|e| e.to_string())
                    },
                    cx,
                );
            }
            SettingsAiProvidersEvent::DeleteProvider(pid) => {
                let pid = pid.clone();
                self.spawn_magpie_state_mutation(
                    "已移除供应商",
                    "Provider removed",
                    false,
                    move |c, tok, epoch| async move {
                        c.hermes_magpie_delete_provider_with_token(&tok, epoch, &pid)
                            .await
                            .map(|r| r.state)
                            .map_err(|e| e.to_string())
                    },
                    cx,
                );
            }
            SettingsAiProvidersEvent::ProbeProvider { provider_id, model } => {
                self.run_magpie_probe(provider_id.clone(), model.clone(), cx);
            }
            SettingsAiProvidersEvent::ToggleAllowLocalFallback(allow) => {
                let target_pid = self
                    .settings_router
                    .read(cx)
                    .settings_ai_providers
                    .as_ref()
                    .and_then(|ap| ap.read(cx).state.as_ref().map(|s| s.active_provider.clone()))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "local_hermes".to_string());
                let req = MagpieSaveProviderRequest {
                    provider_id: Some(target_pid),
                    activate: false,
                    allow_local_fallback: Some(*allow),
                    ..Default::default()
                };
                self.spawn_magpie_state_mutation(
                    "本地回退策略已更新",
                    "Local fallback setting updated",
                    false,
                    move |c, tok, epoch| async move {
                        c.hermes_magpie_save_provider_with_token(&tok, epoch, &req)
                            .await
                            .map(|r| r.state)
                            .map_err(|e| e.to_string())
                    },
                    cx,
                );
            }
            SettingsAiProvidersEvent::SetRoutingStrategy(strategy) => {
                let req = MagpieGroupsRequest {
                    group: None,
                    routing_strategy: Some(strategy.clone()),
                };
                self.spawn_magpie_state_mutation(
                    "全局路由策略已更新",
                    "Routing strategy updated",
                    false,
                    move |c, tok, epoch| async move {
                        c.hermes_magpie_upsert_group_with_token(&tok, epoch, &req)
                            .await
                            .map(|r| r.state)
                            .map_err(|e| e.to_string())
                    },
                    cx,
                );
            }
            SettingsAiProvidersEvent::UpsertRoutingGroup(group) => {
                let req = MagpieGroupsRequest {
                    group: Some(group.clone()),
                    routing_strategy: None,
                };
                self.spawn_magpie_state_mutation(
                    "模型路由组已保存",
                    "Routing group saved",
                    false,
                    move |c, tok, epoch| async move {
                        c.hermes_magpie_upsert_group_with_token(&tok, epoch, &req)
                            .await
                            .map(|r| r.state)
                            .map_err(|e| e.to_string())
                    },
                    cx,
                );
            }
            SettingsAiProvidersEvent::DeleteRoutingGroup(gid) => {
                let gid = gid.clone();
                self.spawn_magpie_state_mutation(
                    "已删除模型路由组",
                    "Routing group deleted",
                    false,
                    move |c, tok, epoch| async move {
                        c.hermes_magpie_delete_group_with_token(&tok, epoch, &gid)
                            .await
                            .map(|r| r.state)
                            .map_err(|e| e.to_string())
                    },
                    cx,
                );
            }
            SettingsAiProvidersEvent::UpdateMasking(cfg) => {
                let cfg = cfg.clone();
                self.spawn_magpie_state_mutation(
                    "隐私脱敏规则已保存",
                    "Privacy masking updated",
                    false,
                    move |c, tok, epoch| async move {
                        c.hermes_magpie_update_masking_with_token(&tok, epoch, &cfg)
                            .await
                            .map(|r| r.state)
                            .map_err(|e| e.to_string())
                    },
                    cx,
                );
            }
            SettingsAiProvidersEvent::PreviewMasking(req) => {
                let Some(fence) = magpie_fence(self) else {
                    return;
                };
                let token = fence.token.clone();
                let expected_epoch = fence.client_epoch;
                let client = fence.client.clone();
                let req = req.clone();
                let fence_pre = fence.clone();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence_pre.preflight_ui(&weak, app).await {
                        return;
                    }
                    let res = client
                        .hermes_magpie_preview_masking_with_token(&token, expected_epoch, &req)
                        .await;
                    let _ = weak.update(app, |shell, cx| {
                        if !fence.is_valid(shell) {
                            return;
                        }
                        if let Ok(preview) = res {
                            shell.settings_router.update(cx, |sr, scx| {
                                if let Some(ref ap) = sr.settings_ai_providers {
                                    ap.update(scx, |view, vcx| {
                                        view.masking_preview_result = Some(preview);
                                        vcx.notify();
                                    });
                                }
                            });
                        }
                    });
                })
                .detach();
            }
            SettingsAiProvidersEvent::ImportConfig(req) => {
                let req = req.clone();
                self.spawn_magpie_state_mutation(
                    "配置导入成功",
                    "AI provider config imported",
                    false,
                    move |c, tok, epoch| async move {
                        c.hermes_magpie_import_with_token(&tok, epoch, &req)
                            .await
                            .map(|r| r.state)
                            .map_err(|e| e.to_string())
                    },
                    cx,
                );
            }
        }
    }
}
