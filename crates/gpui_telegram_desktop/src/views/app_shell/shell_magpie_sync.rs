//! Synchronization and action dispatch between `AppShell`, `SettingsAiProvidersSections`,
//! `GlobalTasksWorkspace`, and the Magpie backend routes (`/api/v1/hermes/magpie/*`).

use std::future::Future;
use std::time::Duration;

use gpui::{AsyncApp, ClipboardItem, Context, WeakEntity};
use gpui_telegram_client::net::hermes_client::{
    MagpieGroupsRequest, MagpieProbeRequest, MagpieSaveProviderRequest, MagpieStateDto,
};

use crate::i18n::tr_str;
use crate::views::app_shell::local_subscription_bridge::{
    activate_subscription_with_local_fallback_pinned, discover_local_subscriptions_for_server,
    enrich_items_with_live_catalog, fetch_live_models_catalog_pinned,
    fetch_models_for_subscription_live_pinned, refresh_legacy_provider_token_once_pinned,
    sync_local_subscriptions_to_server_pinned,
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
    let http = reqwest::Client::new();
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
    if state.needs_provider_selection {
        return (
            Some(tr_str(lang, "请选择供应商", "Choose Provider").to_string()),
            true,
        );
    }
    let active = if !state.active_provider.is_empty() && !state.active_model.is_empty() {
        format!("{} · {}", state.active_provider, state.active_model)
    } else if !state.active_provider.is_empty() {
        state.active_provider.clone()
    } else if let Some(sub) = state.subscriptions.iter().find(|s| s.active) {
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

    /// Switches the viewer's active Settings/Magpie model on the currently active provider or subscription.
    pub(crate) fn switch_active_settings_model(
        &mut self,
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

        let active_pid = state.active_provider.trim().to_string();
        let matching_sub_id: Option<String> = if active_pid.is_empty() || trimmed.starts_with("group/") {
            None
        } else {
            self.magpie_local
                .discovered_items
                .iter()
                .find(|i| {
                    i.account.id == active_pid
                        || (i.account.kind == magpie_gateway::SubscriptionKind::GoogleVertex
                            && active_pid == "magpie_provider:google-vertex")
                })
                .map(|i| i.account.id.clone())
                .or_else(|| {
                    self.magpie_local
                        .server_subscriptions
                        .iter()
                        .find(|s| {
                            s.id == active_pid
                                || (s.kind == "google_vertex"
                                    && active_pid == "magpie_provider:google-vertex")
                        })
                        .map(|s| s.id.clone())
                })
                .or_else(|| {
                    state
                        .subscriptions
                        .iter()
                        .find(|s| {
                            s.id == active_pid
                                || (s.kind == "google_vertex"
                                    && active_pid == "magpie_provider:google-vertex")
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

        let target_provider_id = if !active_pid.is_empty() {
            Some(active_pid)
        } else {
            state
                .providers
                .iter()
                .find(|p| p.is_active)
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
        if let Some(ref creator) = self.miniapps.create_bot_box {
            creator.update(cx, |b, bcx| {
                b.sync_ai_models(catalog_for_creator);
                bcx.notify();
            });
        }
        self.sync_model_picker_to_stage(cx);
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
            let Ok(Some((local_items, res, subs_res))) = client
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
                    Some((local_items, res, subs_res))
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
                if let Some(state) = outcome.state {
                    let subs = subs_res.ok();
                    shell.apply_magpie_state_and_subscriptions_to_views(state, subs, cx);
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
                            .map(|r| r.subscriptions.iter().filter(|s| s.active).count())
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
                let local_active = outcome
                    .local_items
                    .iter()
                    .filter(|s| s.account.active)
                    .count();
                let total_found = local_active.max(server_scan_count);
                shell.magpie_local.discovered_items = outcome.local_items;

                let lang = shell.active_lang_id().to_string();
                let toast = if let Some(ref act_id) = outcome.auto_activated_id {
                    let display = outcome
                        .auto_activated_label
                        .as_deref()
                        .unwrap_or(act_id.as_str());
                    if lang.starts_with("zh") {
                        format!("扫描完成：发现 {total_found} 个本机凭据，已自动激活「{display}」")
                    } else {
                        format!("Scan complete: found {total_found} local credential(s), auto-activated \"{display}\"")
                    }
                } else if total_found > 0 {
                    if lang.starts_with("zh") {
                        format!("扫描完成：检测到 {total_found} 个可用本机 AI 订阅凭据")
                    } else {
                        format!("Scan complete: detected {total_found} active local AI credential(s)")
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
                            view.set_status(Some(toast.clone()), None);
                            vcx.notify();
                        });
                    }
                });
                shell.show_non_chat_toast(toast, 2800, cx);
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
