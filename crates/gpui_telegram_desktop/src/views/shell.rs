//! AppShell: Root Desktop View Orchestration (`SPEC-02-GPUI-COMPONENT-HIERARCHY` § 2.1).
//!
//! Unauthenticated onboarding/login delegates to `AuthWizardView`; authenticated workspace mounts
//! Sidebar + Stage/Composer + Inspector, lazy `SettingsRouter`, and `shell_overlays_render` overlays.

pub use crate::views::app_shell::shell_state::{
    reconcile_stage_message_list, should_switch_to_chats_on_sidebar_selection,
    ShellCodingHostState, ShellComputersState, ShellMiniAppState, ShellOverlayState,
    ShellRemoteHostRuntime, ShellRtcTalkState, ShellSessionSyncState, ShellThemeVisualState,
};
pub use crate::views::app_shell::shell_truth::*;
pub use crate::views::auth::{
    AuthWizardEvent, AuthWizardTab, AuthWizardView, LoginInputField, LoginWizardStep,
    PendingAuthVaultBootstrap,
};

use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_client::storage::Localstorage;
use gpui_telegram_client::{AuthResponse, ClientConfig, StoredSession, TelegramClient};
use gpui_telegram_core::types::ChatId;

use crate::app::session::send_as_peers::SendAsKey;
use crate::app::{preferences::{AppPreferences, ThemeMode}, TelegramApp};
use crate::components::ui::TextInputController;
use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemeMode, TdesktopThemePalette};
use crate::views::boxes::modal_types::ModalDialogKind;
use crate::views::app_shell::{
    build_account_record, compute_shell_palette,
    local_subscription_bridge::ShellMagpieLocalState,
    resolve_active_target_chat_id,
    shell_theme::tdesktop_mode_for_window_appearance,
    window_connecting_widget::ConnectionStatus as BannerConnectionStatus, ModalContainer,
    ShellBotBrowserState, ShellContentHubState, ShellPanelState,
};
use crate::views::composer::BotKeyboard;
use crate::views::nav_rail::NavDestination;
use crate::views::settings::{
    sections::SettingsLocalPasscodeSections, settings_type::SettingsSectionType, SettingsRouter,
};
use crate::views::{
    AppleAppSidebar, BrowserWorkspace, CallOverlay, CanonicalComposer, ChatSidebar,
    CodeSceneWorkspace, CommunityWorkspace, ComputerWorkbench, ContentHubSection,
    ContentHubWorkspace, ContextInspector, ConversationStage, GlobalTasksWorkspace,
    MainMenuDrawer, MediaViewer, NavRail,
};
use crate::window::WindowSubsystemRuntime;

/// Top-level workspace panel state when authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveWorkspacePanel {
    Chats,
    Calls,
    TaskCenter,
    McpTools,
    Browser,
    CodeScene,
    Community,
    Settings,
    ContentHub(ContentHubSection),
}

/// Cycles the 4 built-in Telegram Desktop theme palettes (`Night -> Tinted -> Day -> Classic`).
pub fn next_tdesktop_theme_cycle(current: TdesktopThemeMode) -> TdesktopThemeMode {
    match current {
        TdesktopThemeMode::Night => TdesktopThemeMode::Tinted,
        TdesktopThemeMode::Tinted => TdesktopThemeMode::Day,
        TdesktopThemeMode::Day => TdesktopThemeMode::Classic,
        TdesktopThemeMode::Classic => TdesktopThemeMode::Night,
    }
}

/// Toggles the UI language code between `zh-CN` and `en-US`.
pub fn toggled_language_code(current: &str) -> &'static str {
    if current.starts_with("zh") { "en-US" } else { "zh-CN" }
}

/// Maps `Cmd+1..9` keystrokes to primary `NavDestination` tabs when authenticated.
pub fn nav_destination_for_shortcut_digit(key: &str) -> Option<NavDestination> {
    match key {
        "1" => Some(NavDestination::Chats),
        "2" => Some(NavDestination::Folders),
        "3" => Some(NavDestination::Calls),
        "4" => Some(NavDestination::TaskCenter),
        "5" => Some(NavDestination::McpTools),
        "6" => Some(NavDestination::TalkMode),
        "7" => Some(NavDestination::SavedMessages),
        "8" => Some(NavDestination::Settings),
        "9" => Some(NavDestination::Browser),
        _ => None,
    }
}

/// Snapshot of per-chat composer metadata taken when entering `+` / modal isolation so canceling
/// back to the same chat restores reply/edit/attachments/bot-keyboard cleanly.
#[derive(Debug, Clone, Default)]
pub(crate) struct ComposeIsolationSnapshot {
    pub chat_id: Option<u64>,
    pub replying_to: Option<(u64, String, String)>,
    pub editing_message: Option<(u64, String)>,
    pub attachments: Vec<PathBuf>,
    pub bot_keyboard: Option<BotKeyboard>,
    pub is_bot_keyboard_open: bool,
}

/// Root GPUI desktop view orchestrating workspace views, session lifecycle, routing, and overlays.
pub struct AppShell {
    pub nav_rail: Entity<NavRail>,
    pub apple_sidebar: Entity<AppleAppSidebar>,
    pub content_hub: Entity<ContentHubWorkspace>,
    pub sidebar: Entity<ChatSidebar>,
    pub stage: Entity<ConversationStage>,
    pub composer: Entity<CanonicalComposer>,
    pub inspector: Entity<ContextInspector>,
    pub modal_container: Entity<ModalContainer>,
    pub main_menu_drawer: Entity<MainMenuDrawer>,
    pub tasks_workspace: Entity<GlobalTasksWorkspace>,
    pub browser_workspace: Option<Entity<BrowserWorkspace>>,
    pub code_scene_workspace: Option<Entity<CodeSceneWorkspace>>,
    pub community_workspace: Option<Entity<CommunityWorkspace>>,
    pub community_ime_preedit_input_gen: Option<u64>,
    pub community_pending_share: Option<(String, String)>,
    pub auth_wizard: Entity<AuthWizardView>,
    pub settings_router: Entity<SettingsRouter>,
    pub client: Arc<TelegramClient>,
    pub preferences_dir: PathBuf,
    pub preferences: AppPreferences,
    pub local_storage: Localstorage,
    pub app: TelegramApp,
    pub window_runtime: WindowSubsystemRuntime,
    pub panel_state: ShellPanelState,
    pub content_hub_state: ShellContentHubState,
    pub is_authenticated: bool,
    pub active_panel: ActiveWorkspacePanel,
    pub last_handled_nav_dest: Option<NavDestination>,
    pub last_handled_nav_trigger: u64,
    pub focus_handle: FocusHandle,
    pub ime: TextInputController,
    pub session_sync: ShellSessionSyncState,
    pub miniapps: ShellMiniAppState,
    pub rtc_talk: ShellRtcTalkState,
    pub overlays: ShellOverlayState,
    pub computers: ShellComputersState,
    pub remote_host: ShellRemoteHostRuntime,
    pub coding_host: ShellCodingHostState,
    pub magpie_local: ShellMagpieLocalState,
    pub bot_browser: ShellBotBrowserState,
    pub theme_state: ShellThemeVisualState,
    pub last_window_appearance: Option<WindowAppearance>,
    pub scheduled_toast_id: Option<u64>,
    pub last_observed_toast_chat_id: Option<u64>,
    pub proxy_op_generation: u64,
    pub(crate) compose_isolation_snapshot: Option<ComposeIsolationSnapshot>,
    pub(crate) prev_composer_focused: std::cell::Cell<bool>,
    pub(crate) last_ime_modal_snapshot:
        Option<(crate::views::boxes::modal_types::ActiveTextField, Option<ModalDialogKind>)>,
}

impl AppShell {
    /// Creates an `AppShell` using the resolved default `ClientConfig`.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::new_with_client(Arc::new(TelegramClient::new(ClientConfig::resolve())), cx)
    }

    /// Creates an `AppShell` bound to `client`, loads persisted preferences, wires observers,
    /// and attempts startup session hydration if a stored session is present.
    pub fn new_with_client(client: Arc<TelegramClient>, cx: &mut Context<Self>) -> Self {
        client.ws_transport.set_max_reconnect_attempts(client.config.max_reconnect_attempts);
        let srv = client.config.server_url.clone();

        let nav_rail = cx.new(|_| NavRail::new());
        let sidebar = cx.new(|_| { let mut sb = ChatSidebar::new(); sb.configure_for_cloud_workspace(&srv); sb });
        let stage = cx.new(|_| { let mut st = ConversationStage::new(); st.configure_for_cloud_workspace(&srv, None); st });
        let composer = cx.new(|_| CanonicalComposer::new());
        let inspector = cx.new(|_| { let mut insp = ContextInspector::new(); insp.configure_for_cloud_workspace(&srv); insp });
        let modal_container = cx.new(|_| ModalContainer::new());
        let focus_handle = cx.focus_handle();

        let app_dir = if std::env::var_os("GPUI_TELEGRAM_CONFIG_DIR").is_some() {
            ClientConfig::get_app_dir()
        } else if let Some(cache_dir) = client.local_store.cache_dir() {
            if cache_dir.file_name().and_then(|s| s.to_str()) == Some("cache") {
                cache_dir.parent().unwrap_or(&cache_dir).to_path_buf()
            } else {
                cache_dir.to_path_buf()
            }
        } else {
            ClientConfig::get_app_dir()
        };
        let mut local_storage = Localstorage::new(&app_dir);
        let _ = local_storage.start();
        let preferences = AppPreferences::load(&app_dir, &local_storage);
        let is_chinese_locale = preferences.localization.language_code.starts_with("zh");
        let init_radius = preferences.appearance.bubble_radius;
        let (theme_mode, init_palette) =
            compute_shell_palette(&preferences, None, None, init_radius, None);
        let init_lang = if is_chinese_locale { "zh-CN" } else { "en" };

        composer.update(cx, |comp, _| {
            comp.send_on_enter = preferences.appearance.send_on_enter;
            comp.suggest_stickers_by_emoji = preferences.appearance.suggest_stickers_by_emoji;
            comp.loop_animated_stickers = preferences.appearance.loop_animated_stickers;
            comp.stickers_list.loop_animated = preferences.appearance.loop_animated_stickers;
        });
        stage.update(cx, |st, _| {
            st.loop_animated_stickers = preferences.appearance.loop_animated_stickers;
        });
        sidebar.update(cx, |sb, sb_cx| {
            sb.attach_nav_rail(nav_rail.clone(), sb_cx);
            sb.set_has_leading_app_sidebar(true);
            sb.set_collapsed(preferences.appearance.sidebar_collapsed);
            sb.folder_tags_enabled = preferences.folder_tags_enabled;
            sb.set_notification_badge_rules(
                preferences.notifications.badge_count_messages,
                preferences.notifications.include_muted_in_badge,
            );
        });

        let apple_sidebar = cx.new(|_| AppleAppSidebar::new(init_palette, init_lang));
        let content_hub = cx.new(|_| ContentHubWorkspace::new(init_palette, init_lang));
        let main_menu_drawer = cx.new(|_| {
            let mut m = MainMenuDrawer::new(Vec::new(), 0);
            m.set_palette(init_palette);
            m.set_language(init_lang);
            m
        });
        let tasks_workspace = cx.new(|_| GlobalTasksWorkspace::new_live(init_palette, init_lang));
        let auth_wizard = cx.new(|cx| AuthWizardView::new(client.clone(), init_palette, is_chinese_locale, cx));
        let settings_router = cx.new(|_| SettingsRouter::new(init_palette, is_chinese_locale, preferences.clone()));

        let mut init_theme_state =
            ShellThemeVisualState::new(init_palette, theme_mode, is_chinese_locale);
        init_theme_state.bubble_radius_px = init_radius;

        let mut shell_instance = Self {
            nav_rail,
            apple_sidebar,
            content_hub,
            sidebar,
            stage,
            composer,
            inspector,
            modal_container,
            main_menu_drawer,
            tasks_workspace,
            browser_workspace: None,
            code_scene_workspace: None,
            community_workspace: None,
            community_ime_preedit_input_gen: None,
            community_pending_share: None,
            auth_wizard,
            settings_router,
            client: client.clone(),
            preferences_dir: app_dir,
            preferences,
            local_storage,
            app: TelegramApp::new(),
            window_runtime: WindowSubsystemRuntime::new(),
            panel_state: ShellPanelState::default(),
            content_hub_state: ShellContentHubState::new(),
            is_authenticated: false,
            active_panel: ActiveWorkspacePanel::Chats,
            last_handled_nav_dest: Some(NavDestination::Chats),
            last_handled_nav_trigger: 0,
            focus_handle,
            ime: TextInputController::new(),
            session_sync: ShellSessionSyncState::new(),
            miniapps: ShellMiniAppState::new(),
            rtc_talk: ShellRtcTalkState::new(),
            overlays: ShellOverlayState::new(),
            computers: ShellComputersState::new(),
            remote_host: ShellRemoteHostRuntime::new(),
            coding_host: ShellCodingHostState::new(),
            magpie_local: ShellMagpieLocalState::new(),
            bot_browser: ShellBotBrowserState::new(),
            theme_state: init_theme_state,
            last_window_appearance: None,
            scheduled_toast_id: None,
            last_observed_toast_chat_id: None,
            proxy_op_generation: 0,
            compose_isolation_snapshot: None,
            prev_composer_focused: std::cell::Cell::new(false),
            last_ime_modal_snapshot: None,
        };

        shell_instance.overlays.is_passcode_locked = shell_instance.has_local_passcode_configured();
        shell_instance.wire_workspace_observers(cx);
        cx.observe(&shell_instance.tasks_workspace, |shell, _, cx| {
            shell.sync_discovered_subscriptions_to_local_fabushi_and_vault(cx);
        })
        .detach();
        shell_instance.sync_theme_and_locale_to_views(cx);
        shell_instance.hydrate_settings_from_preferences(cx);
        shell_instance.refresh_settings_truth(cx);
        shell_instance.ensure_remote_host_runtime_bootstrapped(cx);

        if let Some(stored) = shell_instance.load_stored_session_for_shell() {
            let provisional_auth = AuthResponse {
                token: stored.token.clone(),
                session_id: stored.session_id.clone(),
                user_id: stored.user_id,
                username: stored.username.clone(),
                first_name: None,
                created_at: None,
                expires_at: None,
            };
            shell_instance.apply_authenticated_session(provisional_auth, cx);

            let verify_auth_gen = shell_instance.session_sync.auth_generation;
            let verify_token = stored.token.clone();
            let verify_user_id = stored.user_id;
            let verify_session_id = stored.session_id.clone();
            let verify_username = stored.username.clone();

            cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
                let verify_res = client.http_client.get_me(&verify_token).await;
                let _ = weak_shell.update(app, |shell, cx| {
                    if !shell.is_authenticated
                        || shell.session_sync.auth_generation != verify_auth_gen
                        || shell.session_sync.session_token.as_deref() != Some(verify_token.as_str())
                        || shell.session_sync.user_id != Some(verify_user_id)
                        || !Arc::ptr_eq(&shell.client, &client)
                    {
                        return;
                    }
                    match verify_res {
                        Ok(me) => {
                            let resolved_username = me
                                .username
                                .clone()
                                .filter(|u| !u.trim().is_empty())
                                .unwrap_or_else(|| {
                                    shell
                                        .session_sync
                                        .username
                                        .clone()
                                        .unwrap_or_else(|| verify_username.clone())
                                });
                            if me.user_id > 0 && me.user_id != verify_user_id {
                                let auth = AuthResponse {
                                    token: verify_token,
                                    session_id: verify_session_id,
                                    user_id: me.user_id,
                                    username: resolved_username,
                                    first_name: me.first_name,
                                    created_at: me.created_at,
                                    expires_at: me.expires_at,
                                };
                                shell.apply_authenticated_session(auth, cx);
                                return;
                            }
                            if let Some(ref fname) = me.first_name {
                                if !fname.trim().is_empty() {
                                    shell
                                        .client
                                        .local_store
                                        .set_cached_user_display_name(Some(fname.trim().to_string()));
                                }
                            }
                            shell.client.update_auth_profile(
                                verify_user_id,
                                resolved_username.clone(),
                                me.first_name.clone(),
                                me.created_at,
                                me.expires_at,
                            );
                            shell.session_sync.username = Some(resolved_username.clone());
                            let full_name = me
                                .first_name
                                .as_deref()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .unwrap_or(resolved_username.as_str())
                                .to_string();
                            let acc = build_account_record(
                                verify_user_id,
                                &full_name,
                                &resolved_username,
                                "",
                                "",
                            );
                            shell.app.bind_authenticated_session(verify_user_id, &full_name, "");
                            shell.window_runtime.bind_session_user(verify_user_id, &full_name, "");
                            shell.nav_rail.update(cx, |r, _| {
                                r.set_account(&acc.avatar_initials, &acc.display_name);
                            });
                            shell.main_menu_drawer.update(cx, |m, _| {
                                m.accounts = vec![acc];
                                m.active_account_id = verify_user_id as u32;
                            });
                            cx.notify();
                        }
                        Err(
                            gpui_telegram_client::HttpClientError::Unauthorized(_)
                            | gpui_telegram_client::HttpClientError::Api {
                                status: 401 | 403,
                                ..
                            },
                        ) => {
                            shell.logout(cx);
                            shell.auth_wizard.update(cx, |w, wcx| {
                                w.set_login_error(Some(
                                    "Saved session expired. Please sign in again.".to_string(),
                                ));
                                wcx.notify();
                            });
                            cx.notify();
                        }
                        Err(_) => {
                            // Transient network/timeout/5xx: preserve cached workspace offline
                            // just like Telegram Desktop instead of wiping the user's session.
                            shell.window_runtime.set_connection_status(
                                BannerConnectionStatus::WaitingForNetwork,
                                None,
                            );
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
        }

        shell_instance
    }

    fn session_persistence_dir_for_client(client: &TelegramClient) -> Option<PathBuf> {
        let cache_dir = client.local_store.cache_dir()?;
        if cache_dir.file_name().and_then(|s| s.to_str()) == Some("cache") {
            Some(cache_dir.parent().unwrap_or(&cache_dir).to_path_buf())
        } else {
            Some(cache_dir)
        }
    }

    fn load_stored_session_for_shell(&self) -> Option<StoredSession> {
        let dir = Self::session_persistence_dir_for_client(&self.client)?;
        let stored = ClientConfig::load_session_in_dir(&dir)?;
        if stored.token.trim().is_empty() || stored.user_id == 0 {
            return None;
        }
        if !ClientConfig::server_urls_match(&stored.server_url, &self.client.config.server_url) {
            return None;
        }
        Some(stored)
    }

    fn save_stored_session_for_shell(&self, session: &StoredSession) {
        if let Some(dir) = Self::session_persistence_dir_for_client(&self.client) {
            let _ = ClientConfig::save_session_in_dir(&dir, session);
        }
    }

    fn clear_stored_session_for_shell(&self) {
        if let Some(dir) = Self::session_persistence_dir_for_client(&self.client) {
            ClientConfig::clear_session_in_dir(&dir);
        }
    }

    /// Triggers a single deferred redraw right after `CONNECTING_BANNER_GRACE_MS` so the
    /// floating connecting pill appears smoothly only if the connection is still pending.
    pub(crate) fn schedule_connecting_banner_grace_refresh(&self, cx: &mut Context<Self>) {
        let delay = std::time::Duration::from_millis(
            crate::views::app_shell::window_connecting_widget::CONNECTING_BANNER_GRACE_MS + 50,
        );
        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            app.background_executor().timer(delay).await;
            let _ = weak_shell.update(app, |shell, cx| {
                if shell.window_runtime.connecting_widget.is_visible {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Synchronously hydrates folders, contacts, cached dialogs, rich metadata, message histories,
    /// and optional last-active chat from SQLite-backed `LocalStore` for Frame-0 rendering.
    /// Returns `(hydrated_dialogs, restored_active_chat_id)`.
    fn hydrate_workspace_from_local_store(
        &mut self,
        user_id: u64,
        restore_last_chat: bool,
        cx: &mut Context<Self>,
    ) -> (bool, Option<u64>) {
        let cached_folders = self.client.local_store.get_folders();
        if !cached_folders.is_empty() {
            crate::views::app_shell::shell_settings_sync::apply_folders_to_shell(
                self,
                &cached_folders,
                cx,
            );
        }

        let cached_contacts = self.client.local_store.get_cached_contacts();
        if !cached_contacts.is_empty() {
            let rows =
                crate::views::app_shell::shell_actions::contacts_to_modal_rows(&cached_contacts);
            let contact_cids: std::collections::HashSet<u64> = cached_contacts
                .iter()
                .filter(|c| !c.is_bot)
                .filter_map(|c| c.chat_id.map(|cid| cid as u64))
                .collect();
            self.session_sync.contacts_list = cached_contacts;
            self.sidebar.update(cx, |sb, _| {
                sb.contact_chat_ids = contact_cids;
            });
            self.modal_container
                .update(cx, |m, _| m.dialog.sync_contacts(rows));
        }

        let cached_dialogs = self.client.local_store.get_cached_dialog_summaries();
        if cached_dialogs.is_empty() {
            return (false, None);
        }
        let cached_rich_map = self.client.local_store.get_cached_rich_chats();
        let cached_histories = self.client.local_store.list_cached_chat_histories();
        let last_active_chat_id = if restore_last_chat {
            self.client.local_store.get_last_active_chat_id()
        } else {
            None
        };
        let lang_for_hydrate = self.active_lang_id().to_string();

        for (&cid, rc) in &cached_rich_map {
            self.session_sync.rich_chats_by_id.insert(cid, rc.clone());
            if let Some(pid) = rc.peer_user_id.filter(|&pid| pid > 0) {
                self.session_sync.chat_peer_user_ids.insert(cid, pid);
            }
        }

        let mut restored_chat_id: Option<u64> = None;
        self.sidebar.update(cx, |sb, sb_cx| {
            self.stage.update(sb_cx, |st, st_cx| {
                self.inspector.update(st_cx, |insp, insp_cx| {
                    let _ = apply_authoritative_chat_list(sb, st, insp, &cached_dialogs);
                    for c in &cached_dialogs {
                        if c.is_bot.unwrap_or(false) {
                            sb.bot_ids.insert(c.chat_id);
                        }
                    }
                    for (&cid, rc) in &cached_rich_map {
                        if rc.is_bot {
                            sb.bot_ids.insert(cid);
                        }
                        if let Some(d) = sb.dialogs.iter_mut().find(|d| d.id == cid) {
                            d.is_pinned = rc.is_pinned;
                            d.is_muted = rc.is_muted;
                        }
                        if rc.is_archived {
                            if !sb.archived_ids.contains(&cid) {
                                sb.archived_ids.push(cid);
                            }
                        } else {
                            sb.archived_ids.retain(|&x| x != cid);
                        }
                    }
                    for d in &mut sb.dialogs {
                        if d.avatar_initials.is_empty() || d.avatar_initials == "?" {
                            let fixed = derive_initials(&d.peer_name);
                            if !fixed.is_empty() {
                                d.avatar_initials = fixed;
                            }
                        }
                        if let Some(snap) = cached_histories.get(&d.id) {
                            if !snap.messages.is_empty() {
                                let models = build_cached_history_models(
                                    &snap.messages,
                                    &snap.rich_messages,
                                    user_id,
                                    &d.peer_name,
                                    &d.avatar_initials,
                                    d.avatar_color,
                                );
                                st.chat_histories.insert(d.id, models);
                            }
                        }
                    }
                    sb.recompute_folder_badges();

                    if let Some(cid) = last_active_chat_id {
                        if select_authoritative_dialog(sb, st, insp, cid) {
                            restored_chat_id = Some(cid);
                            if let Some(d) = sb.dialogs.iter().find(|d| d.id == cid) {
                                st.is_muted_chat = d.is_muted;
                                st.is_pinned_chat = d.is_pinned;
                            }
                            if let Some(rc) = cached_rich_map.get(&cid) {
                                st.is_muted_chat = rc.is_muted;
                                st.is_pinned_chat = rc.is_pinned;
                                st.is_peer_blocked = rc.is_blocked;
                                st.is_bot_chat = rc.is_bot;
                                st.is_group_chat =
                                    rc.is_group || rc.is_channel || matches!(rc.chat_type, 1 | 2 | 3);
                                st.is_saved_messages = rc.chat_type == 5
                                    || rc.kind.as_deref() == Some("saved")
                                    || (rc.chat_type == 0
                                        && rc.peer_user_id.is_none()
                                        && rc.members_count <= 1
                                        && !rc.is_bot);
                                st.message_ttl_seconds = rc.message_ttl_seconds;
                                if st.is_group_chat
                                    && rc.members_count > 0
                                    && st.chat_subtitle.trim().is_empty()
                                {
                                    st.chat_subtitle = if lang_for_hydrate.starts_with("zh") {
                                        format!("{} 位成员", rc.members_count)
                                    } else {
                                        format!("{} members", rc.members_count)
                                    };
                                }
                            }
                            if st.messages.is_empty() && !st.show_bot_start_bar {
                                st.is_loading_history = true;
                            } else {
                                st.is_loading_history = false;
                            }
                            let (_, media, files, links, voices) =
                                crate::views::app_shell::shell_actions::map_stage_media_to_gallery(
                                    &st.messages,
                                );
                            insp.sync_shared_content(media, files, links, voices, Vec::new());
                            insp_cx.notify();
                        }
                    }
                });
                st_cx.notify();
            });
            sb_cx.notify();
        });
        self.overlays.last_sidebar_selection_revision = self.sidebar.read(cx).selection_revision;

        if let Some(cid) = restored_chat_id {
            let cached_draft = self
                .client
                .local_store
                .get_draft(ChatId(cid as i64))
                .map(|d| d.text)
                .unwrap_or_default();
            if !cached_draft.is_empty() {
                self.session_sync
                    .last_draft_text_by_chat
                    .insert(cid, cached_draft.clone());
                self.sidebar
                    .update(cx, |sb, _| sb.set_draft(cid, cached_draft.clone()));
                self.composer.update(cx, |c, _| {
                    if !c.is_compose_isolated {
                        c.set_text(cached_draft);
                    }
                });
            }
        }

        (true, restored_chat_id)
    }

    pub(crate) fn reset_conversation_views(&mut self, srv_url: &str, username: Option<&str>, cx: &mut Context<Self>) {
        self.sidebar.update(cx, |sb, sb_cx| {
            let had_sb_tracked = sb.search_focused
                || sb.is_plus_menu_open
                || sb.is_contacts_modal_open
                || sb.active_creation_modal.is_some()
                || sb.is_account_popover_open
                || !sb.compose_recipient_query.is_empty();
            self.stage.update(sb_cx, |st, st_cx| {
                let had_st_tracked = st.search_open || st.active_bot_card_field.is_some();
                self.inspector.update(st_cx, |insp, insp_cx| {
                    let had_insp_tracked = insp.is_bot_input_active();
                    reset_account_conversation_views(sb, st, insp, srv_url, username);
                    sb.contacts_search_query.clear();
                    sb.creation_primary_input.clear();
                    sb.creation_secondary_input.clear();
                    sb.creation_tertiary_input.clear();
                    sb.pending_add_contact_request = None;
                    sb.pending_create_group_request = None;
                    sb.pending_open_new_channel = false;
                    if had_insp_tracked {
                        insp_cx.notify();
                    }
                });
                if had_st_tracked {
                    st_cx.notify();
                }
            });
            if had_sb_tracked {
                sb_cx.notify();
            }
        });
        self.overlays.last_sidebar_selection_revision = self.sidebar.read(cx).selection_revision;
    }

    /// Resets all account/chat-scoped state on `CanonicalComposer` (draft, IME, undo/redo,
    /// reply/edit, attachments, bot menu/commands/keyboard, send-as, mentions, isolation, focus)
    /// across logout / account-switch boundaries while preserving theme/locale/editor prefs.
    pub(crate) fn reset_composer_for_account_boundary(&mut self, cx: &mut Context<Self>) {
        self.composer.update(cx, |c, ccx| {
            let had_state = c.is_input_focused
                || c.is_compose_isolated
                || c.ime_machine.is_composing_or_committed()
                || !c.text().is_empty()
                || c.replying_to.is_some()
                || c.editing_message.is_some()
                || !c.editor.attachments.is_empty()
                || !c.editor.undo_stack.is_empty()
                || !c.editor.redo_stack.is_empty()
                || c.bot_keyboard.has_markup()
                || c.is_bot_keyboard_open
                || !c.send_as_options.is_empty()
                || c.selected_send_as_peer_id.is_some()
                || c.bot_menu_controls.is_active
                || !c.bot_slash_commands.is_empty()
                || c.hermes_slash_enabled
                || !c.attach_mini_apps.is_empty()
                || !c.autocomplete.all_mention_candidates.is_empty()
                || c.compose_placeholder_override.is_some()
                || c.compose_send_disabled
                || c.is_emoji_picker_open
                || c.is_formatting_open
                || c.attachments_drawer.is_open
                || c.submitted_message.is_some()
                || c.submitted_attachment.is_some()
                || c.pending_attached_files.is_some()
                || c.pending_launch_miniapp.is_some()
                || c.pending_bot_keyboard_press.is_some();

            c.is_compose_isolated = false; // MUST precede set_send_as_options!
            c.is_input_focused = false;
            c.ime_machine.on_ime_cancel();
            c.clear();
            c.cancel_reply_or_edit();
            c.editor.selection_anchor = None;
            c.editor.undo_stack.clear();
            c.editor.redo_stack.clear();
            c.editor.attachments.clear();
            c.editor.send_options_open = false;
            c.editor.send_silent = false;
            c.editor.scheduled_at_secs = None;
            c.editor.selector_search_focused = false;
            c.submitted_message = None;
            c.submitted_attachment = None;
            c.pending_send_silent = None;
            c.pending_schedule_message = None;
            c.pending_edit_commit = None;
            c.pending_attached_files = None;
            c.pending_send_sticker_emoji = None;
            c.pending_send_dice = None;
            c.pending_send_gif = None;
            c.pending_open_poll_modal = false;
            c.pending_open_location_modal = false;
            c.pending_open_contact_modal = false;
            c.pending_launch_miniapp = None;
            c.pending_bot_keyboard_press = None;
            c.clear_bot_menu(Some(ccx));
            c.clear_bot_slash_commands();
            c.set_hermes_slash_enabled(false);
            c.hermes_ac_pinned_by_button = false;
            c.bot_ac_pinned_by_button = false;
            c.set_attach_mini_apps(Vec::new(), 0);
            c.clear_bot_reply_keyboard();
            c.set_send_as_options(Vec::new(), None);
            c.set_compose_placeholder_override(None);
            c.set_compose_send_disabled(false);
            c.autocomplete.mention_rows.clear();
            c.autocomplete.all_mention_candidates.clear();
            c.autocomplete.mention_role_hints.clear();
            c.tabbed_selector.search_query.clear();
            c.attachments_drawer.close();
            c.is_emoji_picker_open = false;
            c.is_formatting_open = false;
            c.formatting_bar.is_visible = false;
            c.context_ref_hint_open = false;
            c.voice_gesture.cancel();
            c.voice_gesture.waveform_samples.clear();
            c.sync_autocomplete();
            if had_state {
                ccx.notify();
            }
        });
        self.prev_composer_focused.set(false);
    }

    pub(crate) fn reset_tasks_workspace_for_account_boundary(&mut self, cx: &mut Context<Self>) {
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id().to_string();
        self.tasks_workspace.update(cx, |w, wcx| {
            let fh = w.focus_handle.take();
            *w = GlobalTasksWorkspace::new_live(pal, &lang);
            w.focus_handle = fh;
            wcx.notify();
        });
    }

    /// Resets all account-scoped state on `NavRail` (badges, archive badge/visibility,
    /// custom folders, connection status, identity labels, active destination, and pending
    /// action flags) without touching `nav_rail.rs`, while preserving theme palette and locale.
    pub(crate) fn reset_nav_rail_for_logout_or_account_switch(&mut self, cx: &mut Context<Self>) {
        let cur_trigger = self.nav_rail.update(cx, |r, rcx| {
            for item in &mut r.items {
                item.unread_badge = 0;
            }
            r.archive_unread_count = 0;
            r.show_archive_in_rail = false;
            r.is_connected = false;
            r.set_folders(Vec::new());
            r.active_folder_id = None;
            r.logout_requested = false;
            r.menu_open_requested = false;
            r.archive_open_requested = false;
            r.theme_toggle_requested = false;
            r.selected_folder_requested = None;
            r.set_account("?", "");
            r.set_active_destination(NavDestination::Chats);
            rcx.notify();
            r.trigger_counter
        });
        self.last_handled_nav_dest = Some(NavDestination::Chats);
        self.last_handled_nav_trigger = cur_trigger;
    }

    /// Stops any active Settings microphone test capture and silently resets `SettingsRouter`
    /// back to `Main` whenever transitioning `self.active_panel` away from `Settings`.
    pub(crate) fn cleanup_settings_panel_if_active(&mut self, cx: &mut Context<Self>) {
        if self.active_panel == ActiveWorkspacePanel::Settings {
            self.stop_settings_mic_test_capture(cx);
            self.settings_router.update(cx, |r, rcx| r.close_silent(rcx));
        }
    }

    /// Programmatically syncs `NavRail` to `dest` and updates BOTH `last_handled_nav_dest` and
    /// `last_handled_nav_trigger` so `observe_nav_rail_entity` never fires a stale panel bounce.
    pub(crate) fn sync_nav_rail_destination_silent(
        &mut self,
        dest: NavDestination,
        cx: &mut Context<Self>,
    ) {
        self.last_handled_nav_dest = Some(dest);
        let nav_trigger = self.nav_rail.update(cx, |r, rcx| {
            if r.active_destination != dest {
                r.set_active_destination(dest);
                rcx.notify();
            }
            r.trigger_counter
        });
        self.last_handled_nav_trigger = nav_trigger;
    }

    /// Binds an authenticated `AuthResponse`, opens WebSocket streaming, resets peer views,
    /// hydrates persisted cache synchronously for Frame-0 painting, triggers parallel server
    /// data syncs, and persists the session to disk.
    pub fn apply_authenticated_session(&mut self, auth: AuthResponse, cx: &mut Context<Self>) {
        self.apply_authenticated_session_with_vault(auth, None, cx);
    }

    /// Binds an authenticated `AuthResponse` together with optional in-memory Split-Key /
    /// Passwordless vault bootstrap credentials so the E2EE vault unlocks transparently.
    pub fn apply_authenticated_session_with_vault(
        &mut self,
        auth: AuthResponse,
        vault_bootstrap: Option<PendingAuthVaultBootstrap>,
        cx: &mut Context<Self>,
    ) {
        let prev_user_id = self.session_sync.user_id;
        let is_user_switch = prev_user_id.is_some_and(|p| p != auth.user_id);
        if prev_user_id != Some(auth.user_id) {
            self.session_sync.clear_peer_and_draft_caches();
            self.bot_browser.reset_on_logout();
            if let Some(ov) = self.miniapps.miniapp_overlay.take() {
                ov.update(cx, |w, _| w.close());
            }
            self.miniapps.reset();
            self.computers.reset_on_logout();
            self.magpie_local.discovered_items.clear();
            self.magpie_local.bootstrapped_auth_generation = None;
            self.magpie_local.legacy_keepalive_generation =
                self.magpie_local.legacy_keepalive_generation.wrapping_add(1);
            self.magpie_local.active_legacy_subscription_id = None;
            self.magpie_local.active_legacy_model = None;
            self.community_workspace = None;
            self.community_pending_share = None;
            self.community_ime_preedit_input_gen = None;
            self.reset_nav_rail_for_logout_or_account_switch(cx);
        }
        if is_user_switch {
            self.client.local_store.clear_all();
            self.rtc_talk.reset_on_logout();
            self.overlays.clear_overlays();
            self.sync_remote_tray_status(cx);
            self.active_panel = ActiveWorkspacePanel::Chats;
            self.last_handled_nav_dest = Some(NavDestination::Chats);
            self.reset_content_hub_for_logout_or_account_switch(cx);
            self.window_runtime.controller.clear_all_toasts();
            self.scheduled_toast_id = None;
            self.last_observed_toast_chat_id = None;
            self.compose_isolation_snapshot = None;
            self.reset_composer_for_account_boundary(cx);
            self.reset_tasks_workspace_for_account_boundary(cx);
            self.main_menu_drawer.update(cx, |m, _| m.close());
            self.modal_container.update(cx, |m, mcx| {
                let was_active = m.is_modal_active();
                m.reset_for_logout_or_account_switch();
                if was_active {
                    mcx.notify();
                }
            });
            self.stop_settings_mic_test_capture(cx);
            self.settings_router.update(cx, |r, rcx| {
                r.reset_for_logout_or_account_switch(rcx);
            });
        }
        let srv_url = self.client.config.server_url.clone();
        self.proxy_op_generation = self.proxy_op_generation.wrapping_add(1);
        self.sync_settings_advanced_to_active_proxy(cx);
        self.session_sync.begin_session(
            auth.token.clone(),
            auth.session_id.clone(),
            auth.user_id,
            auth.username.clone(),
        );
        self.is_authenticated = true;
        self.clear_all_ime_preedits(cx);
        self.ime.clear_commit_fence();
        self.panel_state.reset();
        self.auth_wizard.update(cx, |w, wcx| {
            w.is_authenticated = true;
            w.is_authenticating = false;
            w.set_login_error(None);
            wcx.notify();
        });
        self.client.set_auth_session(auth.clone());
        if let Some(ref handler) = self.remote_host.ipc_handler {
            handler.set_client(self.client.clone());
            if prev_user_id != Some(auth.user_id) {
                handler.set_local_node_id(None);
            }
        }
        self.client.connect_ws();
        self.start_ws_inbound_stream(cx);

        if let Some(ref fname) = auth.first_name {
            if !fname.trim().is_empty() {
                self.client
                    .local_store
                    .set_cached_user_display_name(Some(fname.trim().to_string()));
            }
        }
        let cached_display_name = self.client.local_store.get_cached_user_display_name();
        let full_name = auth
            .first_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| cached_display_name.as_deref())
            .unwrap_or(auth.username.as_str())
            .to_string();
        let acc = build_account_record(auth.user_id, &full_name, &auth.username, "", "");
        self.app.bind_authenticated_session(auth.user_id, &full_name, "");
        self.window_runtime.bind_session_user(auth.user_id, &full_name, "");
        self.window_runtime.set_connection_status(BannerConnectionStatus::Connecting, None);
        self.schedule_connecting_banner_grace_refresh(cx);
        self.nav_rail.update(cx, |r, _| r.set_account(&acc.avatar_initials, &acc.display_name));
        self.main_menu_drawer.update(cx, |m, _| {
            m.accounts = vec![acc];
            m.active_account_id = auth.user_id as u32;
        });
        self.session_sync.clear_message_tracking();
        self.reset_conversation_views(&srv_url, Some(&auth.username), cx);
        if self.overlays.is_passcode_locked {
            self.blur_composer_and_cancel_ime(cx);
        }
        let init_comp_focused = self.composer.read(cx).is_input_focused
            && self.is_chats_inline_unoccluded(cx)
            && !self.should_hide_composer(cx);
        self.prev_composer_focused.set(init_comp_focused);

        let (hydrated_dialogs, restored_chat_id) =
            self.hydrate_workspace_from_local_store(auth.user_id, prev_user_id.is_none(), cx);
        if !hydrated_dialogs {
            self.sidebar.update(cx, |sb, sb_cx| {
                sb.is_loading_dialogs = true;
                sb_cx.notify();
            });
            self.stage.update(cx, |st, st_cx| {
                st.is_loading_workspace = true;
                st_cx.notify();
            });
        }

        if prev_user_id != Some(auth.user_id) {
            if let Some(bw) = self.browser_workspace.take() {
                bw.update(cx, |w, wcx| {
                    w.set_workspace_visible(false, wcx);
                    w.set_occluded(true, wcx);
                });
            }
            if let Some(cs) = self.code_scene_workspace.take() {
                cs.update(cx, |w, wcx| {
                    w.teardown(wcx);
                });
            }
            self.community_workspace = None;
            self.community_pending_share = None;
            self.community_ime_preedit_input_gen = None;
            if matches!(
                self.active_panel,
                ActiveWorkspacePanel::Browser
                    | ActiveWorkspacePanel::CodeScene
                    | ActiveWorkspacePanel::Community
            ) {
                self.active_panel = ActiveWorkspacePanel::Chats;
                self.sync_nav_rail_destination_silent(NavDestination::Chats, cx);
                self.sync_apple_sidebar_to_active_panel(cx);
            }
            self.bot_browser.reset_on_logout();
            self.miniapps.reset();
            self.computers.reset_on_logout();
            self.magpie_local.reset_on_logout();
            if let Some(old_rt) = self.coding_host.reset_on_logout() {
                self.client.runtime().spawn(async move {
                    old_rt.shutdown().await;
                });
            }
            self.inspector.update(cx, |insp, _| insp.clear_ai_models());
            self.tasks_workspace.update(cx, |w, wcx| {
                w.reset_on_logout();
                wcx.notify();
            });
            self.settings_router.update(cx, |r, rcx| {
                if let Some(ap) = r.settings_ai_providers.clone() {
                    ap.update(rcx, |sec, sec_cx| {
                        sec.reset_on_logout();
                        sec_cx.notify();
                    });
                }
                if let Some(sm) = r.settings_main.clone() {
                    sm.update(rcx, |main, mcx| {
                        main.sync_ai_providers_status(None, false);
                        mcx.notify();
                    });
                }
            });
        }
        self.refresh_chats(cx);
        if let Some(cid) = restored_chat_id {
            self.load_historical_messages(cid, cx);
        }
        self.refresh_folders(cx);
        self.refresh_contacts(cx);
        self.refresh_wallet(cx);
        self.refresh_settings_truth(cx);
        self.refresh_tasks_and_mcp(cx);
        self.refresh_hermes_commands(cx);
        self.refresh_content_hub_on_login(cx);
        self.register_current_host_and_start_relay(cx);
        if let Some(prev_rt) = self.coding_host.runtime.as_ref() {
            if prev_rt.owner_user_id() != auth.user_id {
                if let Some(old_rt) = self.coding_host.reset_on_logout() {
                    self.client.runtime().spawn(async move {
                        old_rt.shutdown().await;
                    });
                }
            }
        }
        self.coding_host.registered_for_auth_gen = None;
        self.ensure_coding_host_runtime(cx);
        self.announce_coding_host_to_server(cx);
        self.bootstrap_local_bot_browser_and_vault_with_auth(vault_bootstrap, cx);

        self.save_stored_session_for_shell(&StoredSession {
            server_url: srv_url,
            user_id: auth.user_id,
            username: auth.username,
            token: auth.token,
            session_id: auth.session_id,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
        });
        cx.notify();
    }

    /// Clears session credentials, tears down calls/overlays/Mini Apps, resets views,
    /// returns to `AuthWizardView`, and revokes the remote session token asynchronously.
    pub fn logout(&mut self, cx: &mut Context<Self>) {
        let token = self.session_sync.session_token.clone();
        let session_id = self.session_sync.session_id.clone();
        let client = self.client.clone();
        let srv_url = self.client.config.server_url.clone();

        self.session_sync.reset_on_logout();
        self.proxy_op_generation = self.proxy_op_generation.wrapping_add(1);
        self.is_authenticated = false;
        self.clear_all_ime_preedits(cx);
        self.ime.clear_commit_fence();
        self.rtc_talk.reset_on_logout();
        self.overlays.clear_overlays();
        if let Some(ov) = self.miniapps.miniapp_overlay.take() {
            ov.update(cx, |w, _| w.close());
        }
        self.miniapps.reset();
        self.computers.reset_on_logout();
        if let Some(ref handler) = self.remote_host.ipc_handler {
            handler.set_local_node_id(None);
        }
        self.magpie_local.reset_on_logout();
        self.inspector.update(cx, |insp, _| insp.clear_ai_models());
        self.tasks_workspace.update(cx, |w, wcx| {
            w.reset_on_logout();
            wcx.notify();
        });
        self.sync_remote_tray_status(cx);
        if let Some(coding_rt) = self.coding_host.reset_on_logout() {
            client.runtime().spawn(async move {
                coding_rt.shutdown().await;
            });
        }
        if let Some(bw) = self.browser_workspace.take() {
            bw.update(cx, |w, wcx| {
                w.set_workspace_visible(false, wcx);
                w.set_occluded(true, wcx);
            });
        }
        if let Some(cs) = self.code_scene_workspace.take() {
            cs.update(cx, |w, wcx| {
                w.teardown(wcx);
            });
        }
        self.community_workspace = None;
        self.community_pending_share = None;
        self.community_ime_preedit_input_gen = None;
        self.bot_browser.reset_on_logout();
        self.active_panel = ActiveWorkspacePanel::Chats;
        self.last_handled_nav_dest = Some(NavDestination::Chats);
        self.reset_content_hub_for_logout_or_account_switch(cx);

        let reset_client = self.client.clone();
        self.auth_wizard.update(cx, |w, wcx| {
            w.reset_after_logout(reset_client);
            wcx.notify();
        });
        self.stop_settings_mic_test_capture(cx);
        self.settings_router.update(cx, |r, rcx| {
            if let Some(ap) = r.settings_ai_providers.clone() {
                ap.update(rcx, |sec, sec_cx| {
                    sec.reset_on_logout();
                    sec_cx.notify();
                });
            }
            r.reset_for_logout_or_account_switch(rcx);
        });
        self.sync_settings_advanced_to_active_proxy(cx);

        self.client.local_store.clear_all();
        self.client.clear_session();
        self.app.reset_session();
        self.window_runtime.clear_session_user();
        self.window_runtime.controller.clear_all_toasts();
        self.scheduled_toast_id = None;
        self.last_observed_toast_chat_id = None;
        self.compose_isolation_snapshot = None;
        self.window_runtime.set_connection_status(BannerConnectionStatus::Connecting, None);
        self.panel_state.reset();
        self.magpie_local.discovered_items.clear();
        self.magpie_local.bootstrapped_auth_generation = None;
        self.magpie_local.legacy_keepalive_generation =
            self.magpie_local.legacy_keepalive_generation.wrapping_add(1);
        self.magpie_local.active_legacy_subscription_id = None;
        self.magpie_local.active_legacy_model = None;
        self.reset_conversation_views(&srv_url, None, cx);
        self.reset_composer_for_account_boundary(cx);
        self.reset_tasks_workspace_for_account_boundary(cx);
        self.main_menu_drawer.update(cx, |m, _| { m.close(); m.accounts.clear(); m.active_account_id = 0; });
        self.modal_container.update(cx, |m, mcx| {
            let was_active = m.is_modal_active();
            m.reset_for_logout_or_account_switch();
            if was_active {
                mcx.notify();
            }
        });
        self.reset_nav_rail_for_logout_or_account_switch(cx);
        self.clear_stored_session_for_shell();

        if let (Some(token), Some(session_id)) = (token, session_id) {
            let http = client.http_client.clone();
            client.runtime().spawn(async move { let _ = http.revoke_session(&token, &session_id).await; });
        }
        cx.notify();
    }

    /// Returns `"zh-CN"` if Chinese locale is active, otherwise `"en"`.
    pub fn active_lang_id(&self) -> &'static str {
        if self.theme_state.is_chinese_locale || self.preferences.localization.language_code.starts_with("zh") {
            "zh-CN"
        } else {
            "en"
        }
    }

    pub fn nav_rail(&self) -> &Entity<NavRail> { &self.nav_rail }
    pub fn sidebar(&self) -> &Entity<ChatSidebar> { &self.sidebar }
    pub fn stage(&self) -> &Entity<ConversationStage> { &self.stage }
    pub fn composer(&self) -> &Entity<CanonicalComposer> { &self.composer }
    pub fn inspector(&self) -> &Entity<ContextInspector> { &self.inspector }

    fn attach_overlay_close_observer<V: 'static>(
        &self,
        overlay: &Entity<V>,
        is_closed: impl Fn(&V) -> bool + 'static,
        on_close: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.observe(overlay, move |shell: &mut AppShell, ov: Entity<V>, cx| {
            if is_closed(ov.read(cx)) {
                on_close(shell, cx);
                cx.notify();
            }
        })
        .detach();
    }

    pub(crate) fn nav_destination_for_active_panel(&self, cx: &App) -> NavDestination {
        match self.active_panel {
            ActiveWorkspacePanel::Chats | ActiveWorkspacePanel::ContentHub(_) => {
                NavDestination::Chats
            }
            ActiveWorkspacePanel::Calls => NavDestination::Calls,
            ActiveWorkspacePanel::TaskCenter => NavDestination::TaskCenter,
            ActiveWorkspacePanel::McpTools => NavDestination::McpTools,
            ActiveWorkspacePanel::Browser => NavDestination::Browser,
            ActiveWorkspacePanel::CodeScene => NavDestination::CodeScene,
            ActiveWorkspacePanel::Community => NavDestination::Community,
            ActiveWorkspacePanel::Settings => {
                let sr = self.settings_router.read(cx);
                if sr.standalone_computers_mode
                    && sr.settings_section == SettingsSectionType::Computers
                {
                    NavDestination::RemoteComputer
                } else {
                    NavDestination::Settings
                }
            }
        }
    }

    pub(crate) fn reset_nav_destination_if(
        &mut self,
        matching: &[NavDestination],
        cx: &mut Context<Self>,
    ) {
        let restored = self.nav_destination_for_active_panel(cx);
        let (did_reset, nav_trigger) = self.nav_rail.update(cx, |r, rcx| {
            if matching.contains(&r.active_destination) {
                r.set_active_destination(restored);
                rcx.notify();
                (true, r.trigger_counter)
            } else {
                (false, r.trigger_counter)
            }
        });
        if did_reset {
            self.last_handled_nav_dest = Some(restored);
            self.last_handled_nav_trigger = nav_trigger;
        }
    }

    pub fn show_talk_mode(&mut self, cx: &mut Context<Self>) {
        self.occlude_browser_for_overlay(cx);
        self.prepare_for_media_or_call_overlay_open(cx);
        let peer = self.stage.read(cx).chat_title.clone();
        let overlay = cx.new(|_| CallOverlay::new(peer));
        self.attach_overlay_close_observer(&overlay, |ov| ov.should_close, Self::hide_talk_mode, cx);
        self.rtc_talk.talk_mode_overlay = Some(overlay);
        self.reconcile_chat_scoped_toasts(cx);
    }

    pub fn hide_talk_mode(&mut self, cx: &mut Context<Self>) {
        self.rtc_talk.talk_mode_overlay = None;
        self.clear_ime_if_below_z5_unoccluded(cx);
        self.reset_nav_destination_if(&[NavDestination::TalkMode, NavDestination::Calls], cx);
    }

    pub fn show_remote_computer(&mut self, cx: &mut Context<Self>) {
        self.occlude_browser_for_overlay(cx);
        self.prepare_for_computer_overlay_open(cx);
        let overlay = cx.new(|_| ComputerWorkbench::new("Dev-Workstation-01".to_string()));
        self.attach_overlay_close_observer(&overlay, |ov| ov.should_close, Self::hide_remote_computer, cx);
        self.overlays.computer_overlay = Some(overlay);
        self.reconcile_chat_scoped_toasts(cx);
    }

    pub fn hide_remote_computer(&mut self, cx: &mut Context<Self>) {
        if !self.overlays.is_passcode_locked {
            self.clear_all_ime_preedits(cx);
        }
        self.overlays.computer_overlay = None;
        let hub_open = self.active_panel == ActiveWorkspacePanel::Settings
            && self.settings_router.read(cx).settings_section == SettingsSectionType::Computers
            && self.settings_router.read(cx).standalone_computers_mode;
        if !hub_open {
            self.reset_nav_destination_if(&[NavDestination::RemoteComputer], cx);
        }
    }

    pub fn show_media_viewer(&mut self, url: impl Into<String>, is_video: bool, cx: &mut Context<Self>) {
        let url = url.into();
        if self.try_handle_or_swallow_apple_deep_link(&url, false, cx) {
            return;
        }
        self.occlude_browser_for_overlay(cx);
        self.prepare_for_media_or_call_overlay_open(cx);
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();
        let overlay = cx.new(|_| {
            let mut m = MediaViewer::new(url, is_video);
            m.apply_theme(pal, lang);
            m
        });
        self.attach_overlay_close_observer(&overlay, |ov| ov.should_close, Self::close_media_viewer, cx);
        self.overlays.media_viewer_overlay = Some(overlay);
        self.reconcile_chat_scoped_toasts(cx);
    }

    pub fn hide_media_viewer(&mut self) {
        self.ime.clear_ime_state();
        self.overlays.media_viewer_overlay = None;
    }

    pub(crate) fn close_media_viewer(&mut self, cx: &mut Context<Self>) {
        self.overlays.media_viewer_overlay = None;
        self.clear_ime_if_below_z5_unoccluded(cx);
    }

    /// Returns the active section currently displayed inside `SettingsRouter`.
    pub fn current_settings_section(&self, cx: &App) -> SettingsSectionType {
        self.settings_router.read(cx).settings_section
    }

    /// Opens or navigates within the lazy `SettingsRouter` modal and dismisses conflicting popovers.
    pub fn navigate_settings_to(&mut self, target: SettingsSectionType, cx: &mut Context<Self>) {
        self.leave_browser_panel_if_active(cx);
        self.leave_community_panel_if_active(cx);
        self.clear_ime_and_blur_content_hub_for_transition(true, cx);
        let was_outside = self.active_panel != ActiveWorkspacePanel::Settings;
        self.active_panel = ActiveWorkspacePanel::Settings;
        self.sidebar.update(cx, |sb, sb_cx| {
            sb.cancel_compose_mode();
            sb.is_account_popover_open = false;
            sb_cx.notify();
        });
        if self.composer.read(cx).is_compose_isolated
            && !self.is_compose_or_creation_modal_active(cx)
        {
            self.exit_compose_or_modal_isolation(true, cx);
        } else {
            self.sync_compose_state_to_composer(true, cx);
        }
        self.stage.update(cx, |st, st_cx| { st.more_menu_open = false; st.ttl_submenu_open = false; st_cx.notify(); });

        self.settings_router.update(cx, |r, rcx| r.navigate_to(target, was_outside, rcx));
        self.sync_nav_rail_destination_silent(NavDestination::Settings, cx);
        self.sync_apple_sidebar_to_active_panel(cx);
        self.reconcile_chat_scoped_toasts(cx);
        if was_outside {
            self.refresh_settings_truth(cx);
        }
        cx.notify();
    }

    /// Navigates back one step inside `SettingsRouter`, closing Settings when already at root.
    pub fn navigate_settings_back(&mut self, cx: &mut Context<Self>) {
        if self.is_settings_unoccluded(cx) {
            self.clear_all_ime_preedits(cx);
        } else {
            self.settings_router.update(cx, |sr, scx| sr.clear_all_preedits(scx));
        }
        if !self.settings_router.update(cx, |r, rcx| r.navigate_back(rcx)) {
            self.close_settings(cx);
        } else {
            cx.notify();
        }
    }

    /// Closes the Settings modal, restores `ActiveWorkspacePanel::Chats`, and resets `SettingsRouter`.
    pub fn close_settings(&mut self, cx: &mut Context<Self>) {
        if self.is_settings_unoccluded(cx) {
            self.clear_all_ime_preedits(cx);
        } else {
            self.settings_router.update(cx, |sr, scx| sr.clear_all_preedits(scx));
        }
        self.stop_settings_mic_test_capture(cx);
        let was_in_settings = self.active_panel == ActiveWorkspacePanel::Settings;
        self.active_panel = ActiveWorkspacePanel::Chats;
        self.last_handled_nav_dest = Some(NavDestination::Chats);
        let nav_trigger = self.nav_rail.update(cx, |r, rcx| {
            if r.active_destination != NavDestination::Chats {
                r.set_active_destination(NavDestination::Chats);
                rcx.notify();
            }
            r.trigger_counter
        });
        self.last_handled_nav_trigger = nav_trigger;
        self.sync_apple_sidebar_to_active_panel(cx);
        let needs_router_close = {
            let r = self.settings_router.read(cx);
            r.settings_section != SettingsSectionType::Main || !r.settings_stack.is_empty()
        };
        if needs_router_close {
            self.settings_router.update(cx, |r, rcx| r.close_silent(rcx));
        }
        if was_in_settings || needs_router_close {
            cx.notify();
        }
    }

    /// Mutates `AppPreferences`, persists to disk + `Localstorage`, syncs `SettingsRouter`,
    /// updates locale flag, and propagates active theme/locale across child views.
    pub fn update_preferences<F>(&mut self, cx: &mut Context<Self>, f: F)
    where
        F: FnOnce(&mut AppPreferences),
    {
        f(&mut self.preferences);
        if let Err(e) = self.preferences.save_to_dir(&self.preferences_dir) {
            eprintln!("[AppShell] Failed to persist preferences.json: {e}");
        }
        if let Err(e) = self.preferences.sync_to_localstorage(&mut self.local_storage) {
            eprintln!("[AppShell] Failed to sync Localstorage TDF settings: {e}");
        }
        let prefs_snapshot = self.preferences.clone();
        self.theme_state.is_chinese_locale = prefs_snapshot.localization.language_code.starts_with("zh");
        self.theme_state.bubble_radius_px = prefs_snapshot.appearance.bubble_radius;
        self.settings_router.update(cx, |r, rcx| r.sync_preferences(prefs_snapshot, rcx));
        self.sync_theme_and_locale_to_views(cx);
        cx.notify();
    }

    /// Returns `true` when a non-empty local passcode hash is configured.
    pub fn has_local_passcode_configured(&self) -> bool {
        self.preferences
            .local_passcode_hash
            .as_deref()
            .is_some_and(|h| !h.is_empty())
    }

    /// Refreshes the user-activity timestamp used for idle auto-lock checks.
    pub fn record_user_activity(&mut self) {
        self.overlays.last_user_activity = std::time::Instant::now();
    }

    /// Activates the blocking passcode lockscreen if a local passcode is configured.
    pub fn lock_app_now(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.has_local_passcode_configured() {
            return false;
        }
        self.overlays.is_passcode_locked = true;
        self.overlays.passcode_unlock_input.clear();
        self.overlays.passcode_unlock_error = None;
        self.clear_ime_and_blur_content_hub_for_transition(false, cx);
        self.blur_composer_and_cancel_ime(cx);
        self.occlude_browser_for_overlay(cx);
        self.occlude_community_for_overlay(cx);
        self.reconcile_chat_scoped_toasts(cx);
        self.settings_router.update(cx, |sr, scx| {
            if let Some(ref lp) = sr.settings_local_passcode {
                lp.update(scx, |lp, lpcx| {
                    lp.is_locked = true;
                    lpcx.notify();
                });
            }
        });
        cx.notify();
        true
    }

    /// Verifies `pin` against `preferences.local_passcode_hash` and unlocks on match.
    pub fn try_unlock_with_passcode(&mut self, pin: &str, cx: &mut Context<Self>) -> bool {
        let Some(expected) = self.preferences.local_passcode_hash.as_deref() else {
            self.overlays.is_passcode_locked = false;
            self.overlays.passcode_unlock_input.clear();
            self.overlays.passcode_unlock_error = None;
            self.clear_all_ime_preedits(cx);
            cx.notify();
            return true;
        };
        if SettingsLocalPasscodeSections::verify_passcode_against_hash(pin, expected) {
            self.overlays.is_passcode_locked = false;
            self.overlays.passcode_unlock_input.clear();
            self.overlays.passcode_unlock_error = None;
            self.clear_all_ime_preedits(cx);
            self.record_user_activity();
            self.settings_router.update(cx, |sr, scx| {
                if let Some(ref lp) = sr.settings_local_passcode {
                    lp.update(scx, |lp, lpcx| {
                        lp.is_locked = false;
                        lp.bad_tries = 0;
                        lpcx.notify();
                    });
                }
            });
            cx.notify();
            true
        } else {
            let err = tr_str(
                self.active_lang_id(),
                "本地锁定密码不正确，请重试",
                "Incorrect passcode. Please try again.",
            )
            .to_string();
            self.overlays.passcode_unlock_input.clear();
            self.overlays.passcode_unlock_error = Some(err);
            self.settings_router.update(cx, |sr, scx| {
                if let Some(ref lp) = sr.settings_local_passcode {
                    lp.update(scx, |lp, lpcx| {
                        lp.bad_tries = lp.bad_tries.saturating_add(1);
                        lpcx.notify();
                    });
                }
            });
            cx.notify();
            false
        }
    }

    /// Submits the currently typed PIN on the passcode lockscreen.
    pub fn submit_passcode_unlock_input(&mut self, cx: &mut Context<Self>) -> bool {
        let pin = self.overlays.passcode_unlock_input.clone();
        self.try_unlock_with_passcode(&pin, cx)
    }

    /// Locks the app when `auto_lock_seconds > 0` and idle duration meets/exceeds it.
    pub fn check_idle_auto_lock(&mut self, elapsed_secs: u64, cx: &mut Context<Self>) -> bool {
        if self.is_authenticated
            && !self.overlays.is_passcode_locked
            && self.has_local_passcode_configured()
            && self.preferences.auto_lock_seconds > 0
            && elapsed_secs >= self.preferences.auto_lock_seconds
        {
            return self.lock_app_now(cx);
        }
        false
    }

    /// Returns `true` when `+` Apple compose picker or any creation/contacts modal is active.
    pub fn is_compose_or_creation_modal_active(&self, cx: &App) -> bool {
        let sb = self.sidebar.read(cx);
        let is_apple_compose = self.theme_state.palette.is_apple_style() && sb.is_plus_menu_open;
        let create_bot_modal_open = self
            .miniapps
            .create_bot_box
            .as_ref()
            .is_some_and(|b| b.read(cx).is_visible);
        let creation_dialog_open = {
            let mc = self.modal_container.read(cx);
            mc.is_modal_active()
                && matches!(
                    mc.dialog.kind,
                    Some(
                        ModalDialogKind::NewChannel
                            | ModalDialogKind::NewGroup
                            | ModalDialogKind::AddContact
                            | ModalDialogKind::Contacts
                    )
                )
        };
        is_apple_compose
            || sb.active_creation_modal.is_some()
            || sb.is_contacts_modal_open
            || create_bot_modal_open
            || creation_dialog_open
    }

    /// Returns `true` when the bottom `CanonicalComposer` must be hidden (empty `+` compose,
    /// creation in flight, creation/contacts modals, or unstarted bot `START` bar).
    pub fn should_hide_composer(&self, cx: &App) -> bool {
        let sb = self.sidebar.read(cx);
        let is_apple_compose_mode =
            self.theme_state.palette.is_apple_style() && sb.is_plus_menu_open;
        let compose_has_recipients = !sb.compose_draft.selected_recipients.is_empty();
        let compose_pending_creation = sb.compose_draft.pending_creation.is_some();
        let contacts_open = sb.is_contacts_modal_open;
        let creation_modal = sb.active_creation_modal.is_some();
        let create_bot_modal_open = self
            .miniapps
            .create_bot_box
            .as_ref()
            .is_some_and(|b| b.read(cx).is_visible);
        let creation_dialog_open = {
            let mc = self.modal_container.read(cx);
            mc.is_modal_active()
                && matches!(
                    mc.dialog.kind,
                    Some(
                        ModalDialogKind::NewChannel
                            | ModalDialogKind::NewGroup
                            | ModalDialogKind::AddContact
                            | ModalDialogKind::Contacts
                    )
                )
        };
        let show_bot_start_bar = !is_apple_compose_mode && self.stage.read(cx).show_bot_start_bar;
        (is_apple_compose_mode && (!compose_has_recipients || compose_pending_creation))
            || creation_modal
            || contacts_open
            || create_bot_modal_open
            || creation_dialog_open
            || show_bot_start_bar
            || matches!(
                self.active_panel,
                ActiveWorkspacePanel::Browser
                    | ActiveWorkspacePanel::CodeScene
                    | ActiveWorkspacePanel::Community
                    | ActiveWorkspacePanel::ContentHub(_)
            )
            || self.is_publish_review_modal_active(cx)
    }

    /// Flushes the active chat's unsent draft and isolates `CanonicalComposer` for `+` / creation.
    pub(crate) fn enter_compose_or_modal_isolation(&mut self, cx: &mut Context<Self>) {
        if self.composer.read(cx).is_compose_isolated {
            return;
        }
        let active_cid = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        );
        if let Some(cid) = active_cid {
            self.flush_pending_draft_for_chat(cid, cx);
        }
        let snapshot = {
            let c = self.composer.read(cx);
            ComposeIsolationSnapshot {
                chat_id: active_cid,
                replying_to: c.replying_to.clone(),
                editing_message: c.editing_message.clone(),
                attachments: c.editor.attachments.clone(),
                bot_keyboard: c.bot_keyboard.has_markup().then(|| c.bot_keyboard.clone()),
                is_bot_keyboard_open: c.is_bot_keyboard_open,
            }
        };
        self.compose_isolation_snapshot = Some(snapshot);
        self.composer.update(cx, |c, ccx| {
            c.reset_for_compose_picker(Some(ccx));
            ccx.notify();
        });
        self.sync_compose_state_to_composer(true, cx);
        self.reconcile_chat_scoped_toasts(cx);
    }

    pub(crate) fn clear_compose_isolation_snapshot(&mut self) {
        self.compose_isolation_snapshot = None;
    }

    pub(crate) fn has_blocking_fullscreen_overlay(&self, cx: &App) -> bool {
        self.overlays.computer_overlay.is_some()
            || self.rtc_talk.talk_mode_overlay.is_some()
            || self.overlays.media_viewer_overlay.is_some()
            || self.overlays.iv_viewer_overlay.is_some()
            || (self
                .miniapps
                .miniapp_overlay
                .as_ref()
                .is_some_and(|ov| ov.read(cx).is_open)
                && !self.miniapps.is_miniapp_minimized)
    }

    /// Returns `true` when no strictly higher-Z layer (Z #15..#17: `ComputerWorkbench`,
    /// `PasscodeLock`, `AuthWizard`) occludes `PublishReviewModal` (Z #14).
    pub(crate) fn is_publish_review_modal_unoccluded(&self, _cx: &App) -> bool {
        self.is_authenticated
            && !self.overlays.is_passcode_locked
            && self.overlays.computer_overlay.is_none()
    }

    /// Returns `true` when no strictly higher-Z overlay (Z #17..#14: `AuthWizard`,
    /// `PasscodeLock`, `ComputerWorkbench`, `PublishReviewModal`) occludes `ModalContainer` (Z #13).
    pub(crate) fn is_modal_container_unoccluded(&self, cx: &App) -> bool {
        self.is_publish_review_modal_unoccluded(cx) && !self.is_publish_review_modal_active(cx)
    }

    /// Returns `true` when no strictly higher-Z layer (Z #17..#13) occludes `AccountPopover` (Z #12).
    pub(crate) fn is_account_popover_unoccluded(&self, cx: &App) -> bool {
        self.is_modal_container_unoccluded(cx) && !self.modal_container.read(cx).is_modal_active()
    }

    /// Returns `true` when `TaskCenter` / `McpTools` (`StudioInput`, Z #10..#11) is active
    /// and NOT occluded by Z #12..#17 (`AccountPopover`, `ModalContainer`, `PublishReviewModal`,
    /// `ComputerWorkbench`, `PasscodeLock`, `AuthWizard`).
    pub(crate) fn is_tasks_workspace_unoccluded(&self, cx: &App) -> bool {
        self.is_account_popover_unoccluded(cx)
            && !self.sidebar.read(cx).is_account_popover_open
            && matches!(
                self.active_panel,
                ActiveWorkspacePanel::McpTools | ActiveWorkspacePanel::TaskCenter
            )
    }

    /// Returns `true` when `Settings` (Z #9) is the active workspace panel and NOT occluded
    /// by any strictly higher-Z layer (Z #17..#12: `AuthWizard`, `PasscodeLock`,
    /// `ComputerWorkbench`, `PublishReviewModal`, `ModalContainer`, `AccountPopover`).
    pub(crate) fn is_settings_unoccluded(&self, cx: &App) -> bool {
        self.active_panel == ActiveWorkspacePanel::Settings
            && self.is_account_popover_unoccluded(cx)
            && !self.sidebar.read(cx).is_account_popover_open
    }

    /// Returns `true` when no strictly higher-Z layer (Z #17..#8) occludes `CreationModal` (Z #7)
    /// or `ContactsModal` (Z #6).
    pub(crate) fn is_sidebar_modal_unoccluded(&self, cx: &App) -> bool {
        self.is_account_popover_unoccluded(cx)
            && !self.sidebar.read(cx).is_account_popover_open
            && !matches!(
                self.active_panel,
                ActiveWorkspacePanel::McpTools
                    | ActiveWorkspacePanel::TaskCenter
                    | ActiveWorkspacePanel::Settings
                    | ActiveWorkspacePanel::Calls
            )
    }

    /// Returns `true` when the Z #1 base workspace (`Chats` or `ContentHub`) is authenticated
    /// and NOT occluded by any higher-Z layer (Z #2..#17).
    pub(crate) fn is_base_workspace_unoccluded(&self, cx: &App) -> bool {
        if !self.is_sidebar_modal_unoccluded(cx) || self.has_blocking_fullscreen_overlay(cx) {
            return false;
        }
        let sb = self.sidebar.read(cx);
        !sb.is_contacts_modal_open
            && sb.active_creation_modal.is_none()
            && !self
                .miniapps
                .create_bot_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
            && self.miniapps.pending_miniapp_confirm.is_none()
            && !self.miniapps.miniapp_tos_modal_open
            && !self
                .miniapps
                .bot_profile_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
    }

    /// Returns `true` when the inline `Chats` workspace (Z #1) is authenticated, active, and
    /// NOT occluded by any higher-Z layer (Z #17..#2).
    pub(crate) fn is_chats_inline_unoccluded(&self, cx: &App) -> bool {
        self.active_panel == ActiveWorkspacePanel::Chats && self.is_base_workspace_unoccluded(cx)
    }

    /// Clears all shell/child IME preedits iff no higher-Z modal/panel/lockscreen (Z #6..#17)
    /// sits above the Z #5 MiniApp/Bot overlay stack.
    pub(crate) fn clear_ime_if_no_tier2_to_5_overlay(&mut self, cx: &mut Context<Self>) {
        let sb = self.sidebar.read(cx);
        let sidebar_modal_open = sb.is_contacts_modal_open || sb.active_creation_modal.is_some();
        if self.is_sidebar_modal_unoccluded(cx) && !sidebar_modal_open {
            self.clear_all_ime_preedits(cx);
        }
    }

    /// Returns `true` when no strictly higher-Z overlay (Z #5b..#17) occludes `MiniAppOverlay` (Z #5a).
    pub(crate) fn is_miniapp_overlay_unoccluded(&self, cx: &App) -> bool {
        let sb = self.sidebar.read(cx);
        let sidebar_modal_open = sb.is_contacts_modal_open || sb.active_creation_modal.is_some();
        let above_z5a = self
            .miniapps
            .create_bot_box
            .as_ref()
            .is_some_and(|b| b.read(cx).is_visible)
            || self.miniapps.pending_miniapp_confirm.is_some()
            || self.miniapps.miniapp_tos_modal_open
            || self
                .miniapps
                .bot_profile_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible);
        self.is_sidebar_modal_unoccluded(cx) && !sidebar_modal_open && !above_z5a
    }

    /// Clears IME preedits iff `MiniAppOverlay` (Z #5a) is unoccluded by Z #5b..#17.
    pub(crate) fn clear_ime_if_miniapp_unoccluded(&mut self, cx: &mut Context<Self>) {
        if self.is_miniapp_overlay_unoccluded(cx) {
            self.clear_all_ime_preedits(cx);
        }
    }

    /// Prepares shell input state when opening/restoring `MiniAppOverlay` (Z #5a) without
    /// wiping active higher-Z IME (`CreateManagedBotBox` Z #5b or Z #6..#17).
    pub(crate) fn prepare_for_miniapp_overlay_open(&mut self, cx: &mut Context<Self>) {
        self.clear_ime_if_miniapp_unoccluded(cx);
        self.blur_composer_and_cancel_ime(cx);
        self.blur_content_hub_if_focused(cx);
        self.blur_community_if_focused(cx);
    }

    /// Clears IME preedits iff `CreateManagedBotBox` (Z #5b) is unoccluded by Z #5c..#17.
    pub(crate) fn clear_ime_if_create_bot_box_unoccluded(&mut self, cx: &mut Context<Self>) {
        let above_z5b = self.miniapps.pending_miniapp_confirm.is_some()
            || self.miniapps.miniapp_tos_modal_open
            || self
                .miniapps
                .bot_profile_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible);
        if !above_z5b {
            self.clear_ime_if_no_tier2_to_5_overlay(cx);
        }
    }

    /// Clears all shell/child IME preedits iff no higher-Z modal/panel/MiniApp-input/lockscreen
    /// (Z #5a/5b..#17) sits above the Z #2..#4 (`TalkMode`/`MediaViewer`/`IvViewer`) layer.
    pub(crate) fn clear_ime_if_below_z5_unoccluded(&mut self, cx: &mut Context<Self>) {
        let create_bot_visible = self
            .miniapps
            .create_bot_box
            .as_ref()
            .is_some_and(|b| b.read(cx).is_visible);
        let miniapp_input_focused = !self.miniapps.is_miniapp_minimized
            && self
                .miniapps
                .miniapp_overlay
                .as_ref()
                .is_some_and(|w| {
                    let ov = w.read(cx);
                    ov.is_open && ov.has_focused_text_input()
                });
        if !create_bot_visible && !miniapp_input_focused {
            self.clear_ime_if_no_tier2_to_5_overlay(cx);
        }
    }

    /// Blurs `CanonicalComposer` and cancels any active `ime_machine` composition,
    /// notifying `self.composer` iff `is_input_focused` or `ime_machine.is_composing()` changed.
    pub(crate) fn blur_composer_and_cancel_ime(&mut self, cx: &mut Context<Self>) -> bool {
        self.composer.update(cx, |c, ccx| {
            let mut changed = false;
            if c.ime_machine.is_composing() {
                c.ime_machine.on_ime_cancel();
                c.sync_autocomplete();
                changed = true;
            }
            if c.is_input_focused {
                c.is_input_focused = false;
                changed = true;
            }
            if changed {
                ccx.notify();
            }
            changed
        })
    }

    fn blur_content_hub_if_focused(&mut self, cx: &mut Context<Self>) {
        self.content_hub.update(cx, |h, hcx| {
            if h.active_input != crate::views::content_hub::ContentHubInputTarget::None {
                h.blur_input();
                hcx.notify();
            }
        });
    }

    pub(crate) fn blur_community_if_focused(&mut self, cx: &mut Context<Self>) {
        self.community_ime_preedit_input_gen = None;
        if let Some(cw) = self.community_workspace.clone() {
            if cw.read(cx).has_focused_text_input() {
                cw.update(cx, |w, wcx| w.blur_active_input(wcx));
            }
        }
    }

    /// Prepares shell input state when opening `ComputerWorkbench` (Z #15, below `PasscodeLock`
    /// and above `PublishReviewModal`/`ModalContainer`).
    pub(crate) fn prepare_for_computer_overlay_open(&mut self, cx: &mut Context<Self>) {
        if !self.overlays.is_passcode_locked {
            self.clear_all_ime_preedits(cx);
        }
        self.blur_composer_and_cancel_ime(cx);
        self.blur_content_hub_if_focused(cx);
        self.blur_community_if_focused(cx);
    }

    /// Prepares shell input state when opening/restoring a Z #5 MiniApp/Bot overlay
    /// (`MiniApp`, `PendingMiniAppConfirm`, `BotProfileBox`, `MiniAppToS`).
    pub(crate) fn prepare_for_lower_z_overlay_open(&mut self, cx: &mut Context<Self>) {
        self.clear_ime_if_no_tier2_to_5_overlay(cx);
        self.blur_composer_and_cancel_ime(cx);
        self.blur_content_hub_if_focused(cx);
        self.blur_community_if_focused(cx);
    }

    /// Prepares shell input state when opening a Z #2..#4 overlay (`TalkMode`, `MediaViewer`, `IvViewer`).
    pub(crate) fn prepare_for_media_or_call_overlay_open(&mut self, cx: &mut Context<Self>) {
        self.clear_ime_if_below_z5_unoccluded(cx);
        self.blur_composer_and_cancel_ime(cx);
        self.blur_content_hub_if_focused(cx);
        self.blur_community_if_focused(cx);
    }

    pub(crate) fn can_focus_chat_composer(&self, cx: &App) -> bool {
        self.is_chats_inline_unoccluded(cx) && !self.is_non_composer_text_capturing(cx)
    }

    /// Finalizes exiting compose isolation after switching `stage`/`inspector` to `chat_id`,
    /// discarding any pre-switch snapshot for the prior chat and hydrating the new chat's
    /// send-as options, mention candidates, and miniapp/bot controls.
    pub(crate) fn finish_compose_isolation_for_switched_chat(
        &mut self,
        chat_id: u64,
        cx: &mut Context<Self>,
    ) {
        self.clear_compose_isolation_snapshot();
        if self.is_compose_or_creation_modal_active(cx) {
            return;
        }
        if !self.composer.read(cx).is_compose_isolated {
            return;
        }
        let saved_draft = self
            .session_sync
            .last_draft_text_by_chat
            .get(&chat_id)
            .cloned()
            .or_else(|| {
                self.client
                    .local_store
                    .get_draft(ChatId(chat_id as i64))
                    .map(|d| d.text)
            })
            .unwrap_or_default();
        let cached_send_as_list = self
            .session_sync
            .send_as_peers
            .list(&SendAsKey::message(chat_id))
            .to_vec();
        let cached_chosen_send_as = self.session_sync.send_as_peers.chosen(chat_id);
        let focus_composer = self.can_focus_chat_composer(cx);
        self.composer.update(cx, |c, ccx| {
            c.is_compose_isolated = false;
            if saved_draft.is_empty() {
                c.clear();
            } else {
                c.set_text(saved_draft);
            }
            c.cancel_reply_or_edit();
            c.editor.attachments.clear();
            c.submitted_attachment = None;
            c.pending_attached_files = None;
            c.clear_bot_reply_keyboard();
            c.set_send_as_options(cached_send_as_list, cached_chosen_send_as);
            c.is_input_focused = focus_composer;
            ccx.notify();
        });
        self.sync_composer_mention_candidates(chat_id, None, cx);
        self.sync_compose_state_to_composer(true, cx);
        self.sync_composer_and_inspector_miniapp_state(cx);
    }

    /// Restores the active chat's composer state (draft, reply/edit/attachments snapshot,
    /// send-as options, mentions, bot menu) when exiting `+` / creation modals without switching
    /// to a different chat.
    pub(crate) fn exit_compose_or_modal_isolation(
        &mut self,
        restore_same_chat_draft: bool,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self.compose_isolation_snapshot.take();
        if !self.composer.read(cx).is_compose_isolated {
            return;
        }
        let active_cid = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        );
        let saved_draft = active_cid.map(|cid| {
            self.session_sync
                .last_draft_text_by_chat
                .get(&cid)
                .cloned()
                .or_else(|| {
                    self.client
                        .local_store
                        .get_draft(ChatId(cid as i64))
                        .map(|d| d.text)
                })
                .unwrap_or_default()
        });
        let send_as_opts = active_cid
            .map(|cid| {
                (
                    self.session_sync
                        .send_as_peers
                        .list(&SendAsKey::message(cid))
                        .to_vec(),
                    self.session_sync.send_as_peers.chosen(cid),
                )
            })
            .unwrap_or_default();
        let focus_composer = self.can_focus_chat_composer(cx);

        self.composer.update(cx, |c, ccx| {
            c.is_compose_isolated = false;
            if restore_same_chat_draft {
                c.submitted_attachment = None;
                c.pending_attached_files = None;
                if let Some(ref snap) = snapshot.as_ref().filter(|snap| snap.chat_id == active_cid) {
                    c.replying_to = snap.replying_to.clone();
                    c.editing_message = snap.editing_message.clone();
                    c.editor.attachments = snap.attachments.clone();
                    if let Some(ref kb) = snap.bot_keyboard {
                        c.bot_keyboard = kb.clone();
                        c.is_bot_keyboard_open = snap.is_bot_keyboard_open;
                    }
                } else {
                    c.cancel_reply_or_edit();
                    c.editor.attachments.clear();
                    c.clear_bot_reply_keyboard();
                }
                let draft = saved_draft.unwrap_or_default();
                if draft.is_empty() {
                    c.clear();
                } else {
                    c.set_text(draft);
                }
            }
            c.set_send_as_options(send_as_opts.0, send_as_opts.1);
            c.is_input_focused = focus_composer;
            ccx.notify();
        });
        if let Some(cid) = active_cid {
            self.sync_composer_mention_candidates(cid, None, cx);
        }
        self.sync_compose_state_to_composer(true, cx);
        self.sync_composer_and_inspector_miniapp_state(cx);
    }

    /// Syncs dynamic compose placeholder and send-disabled state onto `CanonicalComposer`
    /// while Apple/Grok compose mode is active, resetting both when leaving compose mode.
    pub fn sync_compose_state_to_composer(
        &self,
        notify_composer: bool,
        cx: &mut Context<Self>,
    ) {
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id().to_string();
        let is_isolated_mode = self.is_compose_or_creation_modal_active(cx);
        let hide_composer = self.should_hide_composer(cx);
        let show_bot_start_bar = self.stage.read(cx).show_bot_start_bar;
        let chats_unoccluded = self.is_chats_inline_unoccluded(cx);
        let cannot_focus = hide_composer || show_bot_start_bar || !chats_unoccluded;
        let (placeholder_override, send_disabled) = {
            let sb = self.sidebar.read(cx);
            if pal.is_apple_style() && sb.is_plus_menu_open {
                (
                    Some(sb.compose_placeholder_text(&lang)),
                    sb.compose_draft.selected_recipients.is_empty()
                        || sb.compose_draft.pending_creation.is_some(),
                )
            } else {
                (None, false)
            }
        };
        let needs_defensive_reset =
            is_isolated_mode && !self.composer.read(cx).is_compose_isolated;
        self.composer.update(cx, |c, ccx| {
            if needs_defensive_reset {
                if notify_composer {
                    c.reset_for_compose_picker(Some(ccx));
                } else {
                    c.clear_bot_menu_without_notify(ccx);
                    c.clear_bot_slash_commands();
                    c.set_hermes_slash_enabled(false);
                    c.hermes_ac_pinned_by_button = false;
                    c.cancel_reply_or_edit();
                    c.set_send_as_options(Vec::new(), None);
                    c.editor.attachments.clear();
                    c.submitted_attachment = None;
                    c.pending_attached_files = None;
                    c.autocomplete.mention_rows.clear();
                    c.autocomplete.all_mention_candidates.clear();
                    c.autocomplete.mention_role_hints.clear();
                    if c.ime_machine.is_composing() {
                        c.ime_machine.on_ime_cancel();
                        c.sync_autocomplete();
                    }
                    c.clear();
                    c.clear_bot_reply_keyboard();
                    c.is_input_focused = false;
                    c.is_compose_isolated = true;
                    self.prev_composer_focused.set(false);
                }
            } else if is_isolated_mode {
                if c.bot_menu_controls.is_active {
                    if notify_composer {
                        c.clear_bot_menu(Some(ccx));
                    } else {
                        c.clear_bot_menu_without_notify(ccx);
                    }
                }
                if !c.bot_slash_commands.is_empty() {
                    c.clear_bot_slash_commands();
                }
                if c.hermes_slash_enabled {
                    c.set_hermes_slash_enabled(false);
                    c.hermes_ac_pinned_by_button = false;
                }
                if !c.send_as_options.is_empty() || c.selected_send_as_peer_id.is_some() {
                    c.send_as_options.clear();
                    c.selected_send_as_peer_id = None;
                }
                if c.bot_keyboard.has_markup() || c.is_bot_keyboard_open {
                    c.clear_bot_reply_keyboard();
                }
                if (hide_composer || send_disabled || !chats_unoccluded)
                    && (c.is_input_focused || c.ime_machine.is_composing())
                {
                    if c.ime_machine.is_composing() {
                        c.ime_machine.on_ime_cancel();
                        c.sync_autocomplete();
                    }
                    c.is_input_focused = false;
                    if !notify_composer {
                        self.prev_composer_focused.set(false);
                    }
                }
            } else if cannot_focus && (c.is_input_focused || c.ime_machine.is_composing()) {
                if c.ime_machine.is_composing() {
                    c.ime_machine.on_ime_cancel();
                    c.sync_autocomplete();
                }
                c.is_input_focused = false;
                if !notify_composer {
                    self.prev_composer_focused.set(false);
                }
            }
            c.set_compose_placeholder_override(placeholder_override);
            c.set_compose_send_disabled(send_disabled);
            if notify_composer {
                ccx.notify();
            }
        });
    }

    pub fn has_blocking_overlay(&self, cx: &gpui::App) -> bool {
        let sb = self.sidebar.read(cx);
        sb.is_contacts_modal_open
            || sb.active_creation_modal.is_some()
            || self.modal_container.read(cx).is_modal_active()
            || self
                .miniapps
                .create_bot_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
            || self.miniapps.pending_miniapp_confirm.is_some()
            || self.miniapps.miniapp_tos_modal_open
            || self
                .miniapps
                .bot_profile_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
            || self.active_panel != ActiveWorkspacePanel::Chats
            || self.overlays.is_passcode_locked
            || self.rtc_talk.talk_mode_overlay.is_some()
            || self.overlays.media_viewer_overlay.is_some()
            || self.overlays.iv_viewer_overlay.is_some()
            || self.overlays.computer_overlay.is_some()
            || self.is_publish_review_modal_active(cx)
            || (self
                .miniapps
                .miniapp_overlay
                .as_ref()
                .is_some_and(|ov| ov.read(cx).is_open)
                && !self.miniapps.is_miniapp_minimized)
    }

    pub fn is_chat_visible_for_toast(&self, chat_id: u64, cx: &gpui::App) -> bool {
        let sb = self.sidebar.read(cx);
        self.is_authenticated
            && !sb.is_plus_menu_open
            && !self.is_compose_or_creation_modal_active(cx)
            && !self.has_blocking_overlay(cx)
            && self.stage.read(cx).active_chat_id == Some(chat_id)
    }

    pub fn show_toast(
        &mut self,
        text: impl Into<String>,
        duration_ms: u64,
        cx: &mut Context<Self>,
    ) {
        if let Some(cid) = self
            .stage
            .read(cx)
            .active_chat_id
            .filter(|&cid| self.is_chat_visible_for_toast(cid, cx))
        {
            self.show_chat_toast(cid, text, duration_ms, cx);
            return;
        }
        self.show_non_chat_toast(text, duration_ms, cx);
    }

    pub fn show_chat_toast(
        &mut self,
        chat_id: u64,
        text: impl Into<String>,
        duration_ms: u64,
        cx: &mut Context<Self>,
    ) {
        if !self.is_chat_visible_for_toast(chat_id, cx) {
            return;
        }
        self.window_runtime
            .controller
            .show_chat_toast(chat_id, text, duration_ms);
        self.schedule_current_toast_dismiss(cx);
        cx.notify();
    }

    pub fn show_non_chat_toast(
        &mut self,
        text: impl Into<String>,
        duration_ms: u64,
        cx: &mut Context<Self>,
    ) {
        self.window_runtime
            .controller
            .show_non_chat_toast(text, duration_ms);
        self.schedule_current_toast_dismiss(cx);
        cx.notify();
    }

    /// Schedules an id-fenced async auto-dismiss timer for the current window toast if not
    /// already scheduled. Safe to call across re-renders.
    pub fn schedule_current_toast_dismiss(&mut self, cx: &mut Context<Self>) {
        let Some((target_id, duration_ms, is_unset_chat_id)) = self
            .window_runtime
            .controller
            .current_toast
            .as_ref()
            .map(|t| (t.id, t.duration_ms, t.chat_id.is_none()))
        else {
            self.scheduled_toast_id = None;
            return;
        };

        if self.scheduled_toast_id == Some(target_id) {
            return;
        }

        if is_unset_chat_id {
            if let Some(cid) = self
                .stage
                .read(cx)
                .active_chat_id
                .filter(|&cid| self.is_chat_visible_for_toast(cid, cx))
            {
                self.window_runtime
                    .controller
                    .stamp_current_toast_chat_id_if_unset(cid);
            }
        }

        self.scheduled_toast_id = Some(target_id);
        let delay = std::time::Duration::from_millis(duration_ms.max(80));
        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            app.background_executor().timer(delay).await;
            let _ = weak_shell.update(app, |shell, cx| {
                if shell.window_runtime.controller.dismiss_toast_if_id(target_id) {
                    shell.scheduled_toast_id = None;
                    shell.schedule_current_toast_dismiss(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn reconcile_chat_scoped_toasts(&mut self, cx: &mut Context<Self>) {
        let cur_chat_id = self.stage.read(cx).active_chat_id;
        let mut stale_chat_ids: Vec<u64> = Vec::new();
        if let Some(old_cid) = self.last_observed_toast_chat_id {
            if cur_chat_id != Some(old_cid) || !self.is_chat_visible_for_toast(old_cid, cx) {
                stale_chat_ids.push(old_cid);
            }
        }
        if let Some(cid) = self
            .window_runtime
            .controller
            .current_toast
            .as_ref()
            .and_then(|t| t.chat_id)
        {
            if !self.is_chat_visible_for_toast(cid, cx) && !stale_chat_ids.contains(&cid) {
                stale_chat_ids.push(cid);
            }
        }
        for t in &self.window_runtime.controller.toast_queue {
            if let Some(cid) = t.chat_id {
                if !self.is_chat_visible_for_toast(cid, cx) && !stale_chat_ids.contains(&cid) {
                    stale_chat_ids.push(cid);
                }
            }
        }
        let mut cleared = false;
        for cid in stale_chat_ids {
            if self.window_runtime.controller.clear_toasts_for_chat(cid) {
                cleared = true;
            }
        }
        self.last_observed_toast_chat_id = cur_chat_id;
        if cleared {
            self.schedule_current_toast_dismiss(cx);
        }
    }

    pub fn is_error_toast_text(text: &str) -> bool {
        let lower = text.to_ascii_lowercase();
        text.contains("PROVIDER_ERROR")
            || text.contains("Error:")
            || text.contains("失败")
            || text.contains("错误")
            || lower.contains("failed")
            || lower.contains("error")
    }

    pub fn render_toast_banner_slot(
        &self,
        text: String,
        bottom_gap: f32,
        pal: TdesktopThemePalette,
        cx: &mut Context<Self>,
    ) -> Div {
        let is_apple = pal.is_apple_style();
        let is_err = Self::is_error_toast_text(&text);
        let border_col: Rgba = if is_err {
            TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.55)
        } else if is_apple {
            pal.separator_rgba()
        } else {
            rgb(pal.border_fg).into()
        };
        let card_bg: Rgba = if is_apple {
            TdesktopThemePalette::rgba_with_alpha(pal.menu_bg, 0.96)
        } else {
            rgb(pal.card_bg).into()
        };

        let card = div()
            .id("window-toast-banner")
            .occlude()
            .cursor_pointer()
            .max_w(px(pal.scaled_px(480.0)))
            .min_w(px(0.0))
            .px(px(pal.scaled_px(14.0)))
            .py(px(pal.scaled_px(10.0)))
            .rounded(px(pal.scaled_px(14.0)))
            .bg(card_bg)
            .border_1()
            .border_color(border_col)
            .shadow(pal.popover_shadows())
            .flex()
            .items_start()
            .gap(px(pal.scaled_px(8.0)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.window_runtime.controller.dismiss_current_toast();
                this.schedule_current_toast_dismiss(cx);
                cx.notify();
            }))
            .when(is_err, |row| {
                row.child(
                    div()
                        .w(px(pal.scaled_px(18.0)))
                        .h(px(pal.scaled_px(18.0)))
                        .mt(px(pal.scaled_px(1.0)))
                        .rounded_full()
                        .flex_shrink_0()
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.16))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(pal.scaled_px(11.0)))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.danger))
                        .child("!"),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(pal.scaled_px(12.5)))
                    .line_height(px(pal.scaled_px(17.5)))
                    .font_weight(if is_apple { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                    .text_color(rgb(pal.text_primary))
                    .child(text),
            )
            .child(
                div()
                    .id("window-toast-banner-close")
                    .flex_shrink_0()
                    .w(px(pal.scaled_px(20.0)))
                    .h(px(pal.scaled_px(20.0)))
                    .rounded_full()
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(pal.menu_hover_bg)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.window_runtime.controller.dismiss_current_toast();
                        this.schedule_current_toast_dismiss(cx);
                        cx.notify();
                    }))
                    .child(tg_icon(IconName::Close, pal.scaled_px(11.0), pal.text_secondary)),
            );

        div()
            .absolute()
            .bottom(px(bottom_gap))
            .left(px(pal.scaled_px(16.0)))
            .right(px(pal.scaled_px(16.0)))
            .flex()
            .justify_end()
            .child(card)
    }

    fn render_toast_banner(&self, text: String, pal: TdesktopThemePalette, cx: &mut Context<Self>) -> Div {
        self.render_toast_banner_slot(text, pal.scaled_px(24.0), pal, cx)
    }

    /// Returns `true` only when an unoccluded, full interactive native macOS `WKWebView`
    /// (open Mini App host sheet or active Browser `WKWebView` viewport) should be allowed
    /// to retain `NSWindow.firstResponder`. Floating PiP Mini Apps and all GPUI inputs/modals
    /// return `false` so host `GPUIView` owns keyboard and IME routing.
    pub fn should_native_webview_own_keyboard(&self, cx: &App) -> bool {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return false;
        }
        let miniapp_owns = !self.has_miniapp_top_modal(cx)
            && !self.main_menu_drawer.read(cx).is_open
            && !self.sidebar.read(cx).search_focused
            && self
                .miniapps
                .miniapp_overlay
                .as_ref()
                .is_some_and(|ov| ov.read(cx).allows_native_webview_keyboard_focus());
        if miniapp_owns {
            return true;
        }
        self.active_panel == ActiveWorkspacePanel::Browser
            && !self.has_browser_shell_blocker(cx)
            && !self.is_browser_workspace_input_active(cx)
            && self.browser_workspace.as_ref().is_some_and(|bw| {
                let bw_read = bw.read(cx);
                bw_read.active_engine == crate::views::browser::BrowserEngineKind::WkWebView
                    && !bw_read.is_workspace_occluded()
                    && !bw_read.has_focused_text_input()
            })
    }

    /// Restores macOS `NSWindow.firstResponder` to the host `GPUIView` so GPUI keyboard/IME
    /// input works reliably after any native `WKWebView` interaction or Floating PiP activity.
    pub fn restore_native_host_focus(&self, window: &Window) {
        #[cfg(target_os = "macos")]
        crate::components::bots::restore_gpui_first_responder(window);
        #[cfg(not(target_os = "macos"))]
        let _ = window;
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale_factor = (self.preferences.interface_scale_pct.clamp(50, 300) as f32) / 100.0;
        window.set_rem_size(px(16.0 * scale_factor));

        let current_appearance = window.appearance();
        if self.last_window_appearance != Some(current_appearance) {
            self.last_window_appearance = Some(current_appearance);
        }
        if self.preferences.theme_mode == ThemeMode::System {
            let target_mode = tdesktop_mode_for_window_appearance(current_appearance);
            if self.theme_state.theme_mode != target_mode {
                self.theme_state.theme_mode = target_mode;
                self.sync_theme_and_locale_to_views(cx);
            }
        }

        let browser_occluded = !self.is_authenticated
            || self.active_panel != ActiveWorkspacePanel::Browser
            || self.has_browser_visual_occlusion_overlay(cx);
        if let Some(ref bw) = self.browser_workspace {
            let bw_read = bw.read(cx);
            let want_visible = self.is_authenticated && self.active_panel == ActiveWorkspacePanel::Browser;
            let entering_occlusion_or_leaving = (!want_visible && bw_read.is_workspace_visible())
                || (browser_occluded && !bw_read.is_externally_occluded())
                || (browser_occluded
                    && bw_read.active_input != crate::views::browser::ActiveBrowserInput::None);
            let needs_fabushi_sync = match (&bw_read.local_fabushi, &self.bot_browser.local_fabushi) {
                (Some(cur), Some(host)) => !Arc::ptr_eq(cur, host),
                (None, Some(_)) => true,
                _ => false,
            };
            if bw_read.is_externally_occluded() != browser_occluded
                || bw_read.is_workspace_visible() != want_visible
                || (browser_occluded && bw_read.is_native_webview_visible())
                || entering_occlusion_or_leaving
                || needs_fabushi_sync
            {
                if entering_occlusion_or_leaving {
                    self.ime.clear_preedit();
                    self.bot_browser.ime_preedit_input_gen = None;
                }
                let fabushi_opt = self.bot_browser.local_fabushi.clone();
                bw.update(cx, |w, wcx| {
                    if let Some(host) = fabushi_opt {
                        w.set_local_fabushi(host);
                    }
                    if entering_occlusion_or_leaving
                        && w.active_input != crate::views::browser::ActiveBrowserInput::None
                    {
                        w.set_active_input(
                            crate::views::browser::ActiveBrowserInput::None,
                            None,
                            wcx,
                        );
                    }
                    w.set_workspace_visible(want_visible, wcx);
                    w.set_occluded(browser_occluded, wcx);
                });
            }
        }

        if self.is_authenticated && self.active_panel == ActiveWorkspacePanel::Browser {
            let has_shell_blocker = self.has_browser_shell_blocker(cx);
            if !has_shell_blocker {
                if let Some(bw) = self.browser_workspace.clone() {
                    let has_native_claim = bw.read(cx).has_pending_webview_focus_claim();
                    if has_native_claim
                        && bw.update(cx, |w, wcx| w.consume_webview_focus_claim(wcx))
                    {
                        self.ime.clear_preedit();
                        self.bot_browser.ime_preedit_input_gen = None;
                    }
                    let (clear_for_cdp_focus, had_window_flag) = {
                        let bw_read = bw.read(cx);
                        let surf_focused = bw_read.active_engine
                            == crate::views::browser::BrowserEngineKind::RustCdp
                            && bw_read
                                .cdp_surface()
                                .is_some_and(|s| s.read(cx).focus_handle(cx).is_focused(window));
                        let newly_focused = !self.bot_browser.last_rendered_cdp_surface_focused
                            || bw_read.last_set_active_input_had_window();
                        let should_clear = surf_focused
                            && bw_read.active_input
                                != crate::views::browser::ActiveBrowserInput::None
                            && newly_focused;
                        (should_clear, bw_read.last_set_active_input_had_window())
                    };
                    if clear_for_cdp_focus {
                        self.ime.clear_preedit();
                        self.bot_browser.ime_preedit_input_gen = None;
                        bw.update(cx, |w, wcx| {
                            w.set_active_input(
                                crate::views::browser::ActiveBrowserInput::None,
                                None,
                                wcx,
                            );
                        });
                    } else if had_window_flag {
                        bw.update(cx, |w, _| {
                            w.clear_last_set_active_input_had_window();
                        });
                    }
                }
            }
        }

        if self.is_authenticated && self.active_panel == ActiveWorkspacePanel::Community {
            let sb_w = self.sidebar.read(cx).effective_width();
            let occluded = self.has_community_visual_occlusion_overlay(cx);
            if let Some(cw) = self.community_workspace.clone() {
                if occluded && cw.read(cx).has_focused_text_input() {
                    if self.community_ime_preedit_input_gen.is_some() {
                        self.ime.clear_preedit();
                        self.community_ime_preedit_input_gen = None;
                    }
                    cw.update(cx, |w, wcx| {
                        w.set_host_sidebar_width(sb_w);
                        w.blur_active_input(wcx);
                    });
                } else {
                    cw.update(cx, |w, _| {
                        w.set_host_sidebar_width(sb_w);
                    });
                }
            }
        }

        let cdp_surface_focused = self.is_authenticated && self.is_browser_cdp_viewport_focused(window, cx);
        if self.is_authenticated && !cdp_surface_focused && !self.focus_handle.is_focused(window) {
            self.focus_handle.focus(window);
        }
        if !self.should_native_webview_own_keyboard(cx) {
            self.restore_native_host_focus(window);
        }
        self.bot_browser.last_rendered_cdp_surface_focused = cdp_surface_focused;
        let vp = window.viewport_size();
        let (vp_w, vp_h) = (f32::from(vp.width), f32::from(vp.height));
        if (vp_w - self.overlays.last_viewport_width).abs() >= 1.0 && vp_w > 0.0 {
            self.overlays.last_viewport_width = vp_w;
            self.app.sync_viewport_size(vp_w, vp_h);
        }

        self.reconcile_chat_scoped_toasts(cx);
        self.schedule_current_toast_dismiss(cx);
        let pal = self.theme_state.palette;
        let current_toast = self
            .window_runtime
            .controller
            .current_toast
            .clone()
            .filter(|toast| toast.chat_id.is_none_or(|cid| self.is_chat_visible_for_toast(cid, cx)));
        let update_banner = self.render_in_app_update_banner(cx);

        if !self.is_authenticated {
            let login_bg = if pal.is_apple_style() && !pal.mode.is_dark() {
                pal.dialogs_bg
            } else {
                pal.window_bg
            };
            return div()
                .id("app-shell-login-root")
                .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, _| {
                    this.restore_native_host_focus(window);
                }))
                .size_full()
                .bg(rgb(login_bg))
                .flex()
                .flex_col()
                .relative()
                .when(cfg!(target_os = "macos"), |el| {
                    el.child(
                        div()
                            .id("login-titlebar-drag-strip")
                            .w_full()
                            .h(px(44.0))
                            .flex_shrink_0()
                            .window_control_area(WindowControlArea::Drag),
                    )
                })
                .when_some(update_banner, |el, banner| el.child(banner))
                .child(
                    div()
                        .flex_1()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(self.auth_wizard.clone()),
                )
                .when_some(current_toast, |el, toast| el.child(self.render_toast_banner(toast.text, pal, cx)))
                .into_any_element();
        }

        let focus_handle = self.focus_handle.clone();
        let view = cx.entity().clone();
        let ime_canvas = canvas(
            |bounds, _, _| bounds,
            move |bounds, _, window, cx| {
                window.handle_input(&focus_handle, ElementInputHandler::new(bounds, view.clone()), cx);
            },
        ).size_0();

        let show_conn_banner = self.window_runtime.connecting_widget.should_show_banner();
        let conn_banner_text = self
            .window_runtime
            .connecting_widget
            .localized_status_text(self.active_lang_id());
        let (contacts_open, creation_modal, account_popover_open, is_apple_compose_mode) = {
            let sb = self.sidebar.read(cx);
            (
                sb.is_contacts_modal_open,
                sb.active_creation_modal,
                sb.is_account_popover_open,
                pal.is_apple_style() && sb.is_plus_menu_open,
            )
        };
        let show_bot_start_bar = !is_apple_compose_mode && self.stage.read(cx).show_bot_start_bar;
        let hide_composer = self.should_hide_composer(cx);
        let in_content_hub = matches!(self.active_panel, ActiveWorkspacePanel::ContentHub(_));
        let quick_drawer_open = self.content_hub.read(cx).quick_drawer_open;
        let publish_review_modal_open = self.is_publish_review_modal_active(cx);
        let has_blocking_overlay = contacts_open
            || creation_modal.is_some()
            || self.modal_container.read(cx).is_modal_active()
            || publish_review_modal_open
            || self
                .miniapps
                .create_bot_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
            || self.miniapps.pending_miniapp_confirm.is_some()
            || self.miniapps.miniapp_tos_modal_open
            || self
                .miniapps
                .bot_profile_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
            || self.active_panel != ActiveWorkspacePanel::Chats
            || self.overlays.is_passcode_locked
            || self.rtc_talk.talk_mode_overlay.is_some()
            || self.overlays.media_viewer_overlay.is_some()
            || self.overlays.iv_viewer_overlay.is_some()
            || self.overlays.computer_overlay.is_some()
            || (self
                .miniapps
                .miniapp_overlay
                .as_ref()
                .is_some_and(|ov| ov.read(cx).is_open)
                && !self.miniapps.is_miniapp_minimized);
        if let Some(ref cs) = self.code_scene_workspace {
            let cs_occluded = self.has_code_scene_shell_blocker(cx);
            let want_vis =
                self.is_authenticated && self.active_panel == ActiveWorkspacePanel::CodeScene;
            cs.update(cx, |w, wcx| {
                w.set_occluded(cs_occluded, wcx);
                if !want_vis && w.is_visible {
                    w.set_visible(false, wcx);
                }
            });
        }
        self.sync_compose_state_to_composer(false, cx);

        div()
            .id("app-shell-authenticated-root")
            .track_focus(&self.focus_handle)
            .capture_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, _| {
                this.restore_native_host_focus(window);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_window_key_down(event, window, cx);
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _window, cx| {
                this.handle_window_key_up(event, cx);
            }))
            .relative().size_full().flex().flex_col()
            .bg(rgb(pal.window_bg))
            .child(ime_canvas)
            .when_some(update_banner, |el, banner| el.child(banner))
            .child(
                div().flex_1().w_full().min_h(px(0.0)).flex()
                    .child(self.apple_sidebar.clone())
                    .when(quick_drawer_open, |el| {
                        el.child(self.render_quick_creations_drawer_pane(cx))
                    })
                    .when(in_content_hub, |el| {
                        el.child(
                            div()
                                .id("app-shell-content-hub-slot")
                                .flex_1()
                                .min_w(px(0.0))
                                .h_full()
                                .overflow_hidden()
                                .child(self.content_hub.clone()),
                        )
                    })
                    .when(!in_content_hub, |el| {
                        el.child(self.sidebar.clone())
                        .when(self.active_panel == ActiveWorkspacePanel::Browser, |row| {
                            if let Some(ref bw) = self.browser_workspace {
                                row.child(bw.clone())
                            } else {
                                row
                            }
                        })
                        .when(self.active_panel == ActiveWorkspacePanel::CodeScene, |row| {
                            if let Some(ref cs) = self.code_scene_workspace {
                                row.child(cs.clone())
                            } else {
                                row
                            }
                        })
                        .when(self.active_panel == ActiveWorkspacePanel::Community, |row| {
                            if let Some(ref cw) = self.community_workspace {
                                row.child(cw.clone())
                            } else {
                                row
                            }
                        })
                        .when(
                            !matches!(
                                self.active_panel,
                                ActiveWorkspacePanel::Browser
                                    | ActiveWorkspacePanel::CodeScene
                                    | ActiveWorkspacePanel::Community
                            ),
                            |row| {
                            row.child(
                                div().flex().flex_col().flex_1().min_w(px(0.0)).h_full().overflow_hidden()
                                    .bg(rgb(pal.stage_bg))
                                    .child(
                                        div()
                                            .id("app-shell-stage-slot")
                                            .relative()
                                            .flex_1()
                                            .min_h(px(0.0))
                                            .w_full()
                                            .flex()
                                            .flex_col()
                                            .child(if is_apple_compose_mode {
                                                self.render_apple_compose_stage(cx)
                                            } else {
                                                self.stage.clone().into_any_element()
                                            })
                                            .when(!has_blocking_overlay, |el| {
                                                el.when_some(current_toast.clone(), |el, toast| {
                                                    let bottom_gap = if show_bot_start_bar {
                                                        pal.scaled_px(68.0)
                                                            .max(52.0 + pal.scaled_px(16.0))
                                                    } else {
                                                        pal.scaled_px(16.0)
                                                    };
                                                    el.child(self.render_toast_banner_slot(
                                                        toast.text,
                                                        bottom_gap,
                                                        pal,
                                                        cx,
                                                    ))
                                                })
                                            }),
                                    )
                                    .when(!hide_composer, |el| el.child(self.composer.clone()))
                                    .into_any_element(),
                            )
                            .when(!is_apple_compose_mode, |el| el.child(self.inspector.clone()))
                        })
                    }),
            )
            .when(show_conn_banner && !has_blocking_overlay, |el| {
                let pill_bg = if pal.mode.is_dark() {
                    hsla(0.0, 0.0, 0.14, 0.92)
                } else {
                    hsla(0.0, 0.0, 0.12, 0.84)
                };
                el.child(
                    div()
                        .id("app-shell-connecting-pill")
                        .absolute()
                        .bottom(px(pal.scaled_px(14.0)))
                        .left(px(pal.scaled_px(14.0)))
                        .px(px(pal.scaled_px(12.0)))
                        .py(px(pal.scaled_px(6.0)))
                        .rounded(px(999.0))
                        .bg(pill_bg)
                        .border_1()
                        .border_color(hsla(0.0, 0.0, 1.0, 0.10))
                        .shadow_md()
                        .flex()
                        .items_center()
                        .gap(px(pal.scaled_px(7.0)))
                        .child(
                            div()
                                .w(px(pal.scaled_px(7.0)))
                                .h(px(pal.scaled_px(7.0)))
                                .rounded(px(999.0))
                                .bg(rgb(pal.accent)),
                        )
                        .child(
                            div()
                                .text_size(px(pal.scaled_px(12.0)))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(0xFFFFFF))
                                .child(conn_banner_text),
                        ),
                )
            })
            .when_some(self.rtc_talk.talk_mode_overlay.clone(), |el, ov| el.child(self.render_talk_mode_overlay(ov, cx)))
            .when_some(self.overlays.media_viewer_overlay.clone(), |el, ov| el.child(self.render_media_viewer_overlay(ov)))
            .when_some(self.overlays.iv_viewer_overlay.clone(), |el, ov| el.child(self.render_iv_viewer_overlay(ov)))
            .children(self.render_miniapp_overlays(cx))
            .when(contacts_open, |el| el.child(self.render_contacts_modal_overlay(cx)))
            .when_some(creation_modal, |el, kind| el.child(self.render_sidebar_creation_modal_overlay(kind, cx)))
            .when(self.active_panel == ActiveWorkspacePanel::Calls, |el| el.child(self.render_calls_modal_overlay(cx)))
            .when(self.active_panel == ActiveWorkspacePanel::Settings, |el| el.child(self.settings_router.clone().into_any_element()))
            .when(self.active_panel == ActiveWorkspacePanel::TaskCenter, |el| el.child(self.render_task_center_modal_overlay(cx)))
            .when(self.active_panel == ActiveWorkspacePanel::McpTools, |el| el.child(self.render_mcp_tools_modal_overlay(cx)))
            .when(account_popover_open, |el| el.child(self.render_account_popover_overlay(cx)))
            .child(self.modal_container.clone())
            .when(publish_review_modal_open, |el| {
                el.child(self.render_publish_review_modal_overlay(cx))
            })
            .when_some(self.overlays.computer_overlay.clone(), |el, ov| el.child(self.render_remote_computer_overlay(ov)))
            .when(self.overlays.is_passcode_locked, |el| el.child(self.render_passcode_lock_overlay(cx)))
            .when_some(
                self.computers.pending_approvals.first().cloned(),
                |el, pending| el.child(self.render_pending_computer_approval_banner(&pending, cx)),
            )
            .when(
                self.bot_browser.first_creation_banner_open
                    && self.bot_browser.last_recovery_key_notice.is_some(),
                |el| el.child(self.render_vault_first_setup_notice_banner(cx)),
            )
            .when(has_blocking_overlay, |el| {
                el.when_some(current_toast, |el, toast| {
                    el.child(self.render_toast_banner(toast.text, pal, cx))
                })
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::stage::cards::{DeliveryStatus, MessageContent, MessageDirection, MessageItemModel};

    #[std::prelude::v1::test]
    fn test_theme_cycle_covers_all_four_tdesktop_modes() {
        let m1 = next_tdesktop_theme_cycle(TdesktopThemeMode::Night);
        let m2 = next_tdesktop_theme_cycle(m1);
        let m3 = next_tdesktop_theme_cycle(m2);
        let m4 = next_tdesktop_theme_cycle(m3);
        assert_eq!((m1, m2, m3, m4), (TdesktopThemeMode::Tinted, TdesktopThemeMode::Day, TdesktopThemeMode::Classic, TdesktopThemeMode::Night));
    }

    #[std::prelude::v1::test]
    fn test_language_toggle_and_nav_shortcut_mapping() {
        assert_eq!((toggled_language_code("zh-CN"), toggled_language_code("zh-Hans"), toggled_language_code("en-US")), ("en-US", "en-US", "zh-CN"));
        for (key, expected) in [
            ("1", Some(NavDestination::Chats)), ("2", Some(NavDestination::Folders)),
            ("3", Some(NavDestination::Calls)), ("4", Some(NavDestination::TaskCenter)),
            ("5", Some(NavDestination::McpTools)), ("6", Some(NavDestination::TalkMode)),
            ("7", Some(NavDestination::SavedMessages)), ("8", Some(NavDestination::Settings)),
            ("9", Some(NavDestination::Browser)), ("0", None),
        ] {
            assert_eq!(nav_destination_for_shortcut_digit(key), expected);
        }
    }

    #[std::prelude::v1::test]
    fn test_sidebar_selection_revision_gates_panel_switch_ignoring_hover() {
        let mut last_seen = 0u64;
        assert!(!should_switch_to_chats_on_sidebar_selection(0, &mut last_seen, true));
        assert_eq!(last_seen, 0);
        assert!(should_switch_to_chats_on_sidebar_selection(1, &mut last_seen, true));
        assert_eq!(last_seen, 1);
        assert!(!should_switch_to_chats_on_sidebar_selection(1, &mut last_seen, true));
    }

    #[std::prelude::v1::test]
    fn test_reconcile_stage_message_list_handles_ws_first_http_first_and_duplicates() {
        let mk = |id: u64, delivery_status: DeliveryStatus| MessageItemModel {
            id, sender_id: 10, sender_name: "Me".into(), avatar_initials: "ME".into(),
            avatar_color: 0x3498DB, direction: MessageDirection::Outgoing, timestamp: "13:45".into(),
            delivery_status, reply_to: None, reactions: vec![], bot_keyboard: None, is_pinned: false,
            content: MessageContent::Text { text: "Hello".into(), is_edited: false },
        };

        let mut ws_first = vec![mk(1_700_000_001, DeliveryStatus::Sending)];
        assert!(reconcile_stage_message_list(&mut ws_first, 1_700_000_001, 5, DeliveryStatus::Read));
        assert!(reconcile_stage_message_list(&mut ws_first, 1_700_000_001, 5, DeliveryStatus::Sent));
        assert_eq!((ws_first.len(), ws_first[0].id, ws_first[0].delivery_status), (1, 5, DeliveryStatus::Read));

        let mut http_first = vec![mk(1_700_000_002, DeliveryStatus::Sending)];
        assert!(reconcile_stage_message_list(&mut http_first, 1_700_000_002, 6, DeliveryStatus::Sent));
        assert!(reconcile_stage_message_list(&mut http_first, 1_700_000_002, 6, DeliveryStatus::Read));
        assert_eq!((http_first.len(), http_first[0].id, http_first[0].delivery_status), (1, 6, DeliveryStatus::Read));

        let mut both = vec![mk(7, DeliveryStatus::Read), mk(1_700_000_003, DeliveryStatus::Sending)];
        assert!(reconcile_stage_message_list(&mut both, 1_700_000_003, 7, DeliveryStatus::Sent));
        assert_eq!((both.len(), both[0].id, both[0].delivery_status), (1, 7, DeliveryStatus::Read));
    }
}
