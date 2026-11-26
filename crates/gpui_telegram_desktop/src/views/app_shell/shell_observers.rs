//! Entity observer bindings, WebSocket connection status monitoring, and event handlers
//! for `AuthWizardView`, `SettingsRouter`, and workspace panels in `AppShell`.

use std::future::Future;
use std::sync::Arc;

use gpui::{AsyncApp, ClipboardItem, Context, WeakEntity};
use gpui_telegram_client::{ConnectionStatus, TelegramClient};
use gpui_telegram_proto::contracts::auth::{
    TwoFactorSetRequest, UpdateProfileRequest as ProtoUpdateProfileRequest,
};
use gpui_telegram_proto::contracts::chats::ChatFolderDto;
use gpui_telegram_proto::miniapp_contracts::MiniAppLaunchMode;

use crate::i18n::tr_str;
use crate::views::app_shell::shell_state::should_switch_to_chats_on_sidebar_selection;
use crate::views::app_shell::shell_theme::toggle_desktop_theme_mode;
use crate::views::app_shell::shell_truth::ClientCallFence;
use crate::views::app_shell::window_connecting_widget::ConnectionStatus as BannerConnectionStatus;
use crate::views::auth::AuthWizardEvent;
use crate::views::boxes::modal_types::{ActiveTextField, ModalDialogKind};
use crate::views::chat_sidebar::{
    AddContactKind, CategorizedSearchResult, CreationInputField, SidebarCreationModal,
};
use crate::views::composer::bot_keyboard::BotKeyboardButtonKind;
use crate::views::content_hub::ContentHubInputTarget;
use crate::views::drawer::inspector_tabs::{
    BotConfigFocusField, InspectorAction, InspectorSubpage, InspectorTab,
};
use crate::views::global_tasks_workspace::StudioInputTarget;
use crate::views::nav_rail::NavDestination;
use crate::views::settings::{settings_type::SettingsSectionType, SettingsRouterEvent};
use crate::views::shell::{toggled_language_code, ActiveWorkspacePanel, AppShell};

fn other_input_owns_ime(shell: &AppShell, cx: &gpui::App) -> bool {
    shell.has_active_non_composer_ime_owner(cx)
}

impl AppShell {
    fn spawn_authed_shell_fut<R: 'static, Fut: Future<Output = R> + 'static>(
        &self,
        cx: &mut Context<Self>,
        build_fut: impl FnOnce(ClientCallFence) -> Fut + 'static,
        apply: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        let Some(fence) = self.capture_client_fence() else {
            return;
        };
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let out = build_fut(fence.clone()).await;
            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                apply(shell, out, cx);
            });
        })
        .detach();
    }

    fn sync_auth_wizard_visuals(&self, cx: &mut Context<Self>) {
        let (pal, is_zh) = (self.theme_state.palette, self.theme_state.is_chinese_locale);
        self.auth_wizard.update(cx, |w, wcx| {
            w.apply_theme_and_locale(pal, is_zh);
            wcx.notify();
        });
    }

    pub fn enqueue_localized_toast(
        &mut self,
        zh: &'static str,
        en: &'static str,
        duration_ms: u64,
        cx: &mut Context<Self>,
    ) {
        let msg = tr_str(self.active_lang_id(), zh, en);
        self.show_toast(msg, duration_ms, cx);
    }

    pub fn enqueue_localized_non_chat_toast(
        &mut self,
        zh: &'static str,
        en: &'static str,
        duration_ms: u64,
        cx: &mut Context<Self>,
    ) {
        let msg = tr_str(self.active_lang_id(), zh, en);
        self.show_non_chat_toast(msg, duration_ms, cx);
    }

    /// Wires all child view subscriptions, entity observers, and the WS status monitor.
    pub(crate) fn wire_workspace_observers(&self, cx: &mut Context<Self>) {
        cx.subscribe(&self.auth_wizard, |shell, _, ev, cx| shell.handle_auth_wizard_event(ev, cx))
            .detach();
        let mut prev_auth_target = {
            let w = self.auth_wizard.read(cx);
            (
                w.editing_server_url,
                w.login_step,
                w.auth_wizard_tab,
                w.active_login_field,
            )
        };
        cx.observe(&self.auth_wizard, move |shell, wizard, cx| {
            let cur_target = {
                let w = wizard.read(cx);
                (
                    w.editing_server_url,
                    w.login_step,
                    w.auth_wizard_tab,
                    w.active_login_field,
                )
            };
            if !shell.is_authenticated && cur_target != prev_auth_target {
                let wizard_dirty = {
                    let w = wizard.read(cx);
                    w.ime.ime_preedit.is_some()
                        || w.ime.just_committed_ime
                        || w.ime.last_ime_commit_at.is_some()
                };
                if wizard_dirty {
                    wizard.update(cx, |w, wcx| {
                        w.ime.clear_ime_state();
                        wcx.notify();
                    });
                }
                let shell_dirty = shell.ime.ime_preedit.is_some()
                    || shell.ime.just_committed_ime
                    || shell.ime.last_ime_commit_at.is_some();
                if shell_dirty {
                    shell.ime.clear_ime_state();
                    cx.notify();
                }
            }
            prev_auth_target = cur_target;
        })
        .detach();
        cx.subscribe(&self.settings_router, |shell, _, ev, cx| {
            shell.handle_settings_router_event(ev, cx);
        })
        .detach();
        cx.subscribe(&self.apple_sidebar, |shell, _, ev, cx| {
            shell.handle_apple_sidebar_event(ev, cx);
        })
        .detach();
        cx.subscribe(&self.content_hub, |shell, _, ev, cx| {
            shell.handle_content_hub_event(ev, cx);
        })
        .detach();
        let (mut prev_pub_open, mut prev_pub_field, mut prev_pub_queue, mut prev_hub_input) = {
            let h = self.content_hub.read(cx);
            (
                h.publish_modal.is_open,
                h.publish_modal.focused_field,
                h.publish_modal.is_showing_reviewer_queue(),
                h.active_input,
            )
        };
        cx.observe(&self.content_hub, move |shell, hub, cx| {
            let in_hub_panel = matches!(shell.active_panel, ActiveWorkspacePanel::ContentHub(_));
            let (
                cur_pub_open,
                cur_pub_field,
                cur_pub_queue,
                cur_hub_input,
                pub_close_pending_clear,
                cur_pub_owns_preedit,
                cur_hub_owns_preedit,
            ) = hub.update(cx, |h, _| {
                let eff_hub = if h.has_focused_input(in_hub_panel) {
                    h.active_input
                } else {
                    ContentHubInputTarget::None
                };
                let pending_clear = if prev_pub_open && !h.publish_modal.is_open {
                    std::mem::take(&mut h.publish_modal.pending_close_ime_clear)
                } else {
                    false
                };
                let pub_owns_pre = h.publish_modal.is_open
                    && h.publish_modal.focused_field.is_some_and(|f| {
                        h.publish_modal
                            .field(f)
                            .ime_preedit
                            .as_deref()
                            .is_some_and(|p| !p.is_empty())
                    });
                let hub_owns_pre = match eff_hub {
                    ContentHubInputTarget::CreationsSearch => h
                        .creations_search
                        .ime_preedit
                        .as_deref()
                        .is_some_and(|p| !p.is_empty()),
                    ContentHubInputTarget::CatalogSearch => h
                        .catalog_search
                        .ime_preedit
                        .as_deref()
                        .is_some_and(|p| !p.is_empty()),
                    ContentHubInputTarget::QuickDrawerSearch => h
                        .quick_drawer_search
                        .ime_preedit
                        .as_deref()
                        .is_some_and(|p| !p.is_empty()),
                    ContentHubInputTarget::None => false,
                };
                (
                    h.publish_modal.is_open,
                    h.publish_modal.focused_field,
                    h.publish_modal.is_showing_reviewer_queue(),
                    eff_hub,
                    pending_clear,
                    pub_owns_pre,
                    hub_owns_pre,
                )
            });
            let pub_opening_or_switched = (!prev_pub_open && cur_pub_open)
                || (cur_pub_open
                    && (cur_pub_field != prev_pub_field || cur_pub_queue != prev_pub_queue));
            let pub_closing_with_clear = prev_pub_open && !cur_pub_open && pub_close_pending_clear;
            let pub_changed = pub_opening_or_switched || pub_closing_with_clear;
            if pub_changed && shell.is_publish_review_modal_unoccluded(cx) {
                if pub_opening_or_switched {
                    shell.blur_community_if_focused(cx);
                }
                if !(pub_opening_or_switched && cur_pub_owns_preedit) {
                    shell.clear_all_ime_preedits(cx);
                }
            } else if !pub_changed
                && cur_hub_input != prev_hub_input
                && shell.is_base_workspace_unoccluded(cx)
            {
                if cur_hub_input != ContentHubInputTarget::None {
                    shell.blur_community_if_focused(cx);
                }
                if !cur_hub_owns_preedit {
                    let should_clear = shell.is_content_hub_input_active(cx)
                        || (cur_hub_input == ContentHubInputTarget::None
                            && prev_hub_input != ContentHubInputTarget::None
                            && !other_input_owns_ime(shell, cx));
                    if should_clear {
                        shell.clear_all_ime_preedits(cx);
                    }
                }
            }
            prev_pub_open = cur_pub_open;
            prev_pub_field = cur_pub_field;
            prev_pub_queue = cur_pub_queue;
            prev_hub_input = cur_hub_input;
            cx.notify();
        })
        .detach();

        self.spawn_ws_status_observer(cx);
        self.spawn_idle_auto_lock_observer(cx);
        self.spawn_ttl_purge_observer(cx);
        self.spawn_auto_update_loop(cx);
        self.observe_drawer_and_modal_entities(cx);
        self.observe_nav_rail_entity(cx);
        self.observe_sidebar_entity(cx);
        self.observe_stage_composer_inspector_entities(cx);
    }

    fn spawn_idle_auto_lock_observer(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            loop {
                app.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                let keep = weak_shell
                    .update(app, |shell, cx| {
                        let elapsed = shell.overlays.last_user_activity.elapsed().as_secs();
                        let _ = shell.check_idle_auto_lock(elapsed, cx);
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn spawn_ttl_purge_observer(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            loop {
                app.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
                let keep = weak_shell
                    .update(app, |shell, cx| {
                        if !shell.is_authenticated {
                            return true;
                        }
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let purged_store = shell.client.local_store.purge_expired_messages(now_ms);
                        let mut purged_stage_count = 0usize;
                        shell.stage.update(cx, |st, st_cx| {
                            for (cid, mids) in &purged_store {
                                let cid_u64 = u64::try_from(cid.0).unwrap_or(0);
                                let id_set: std::collections::HashSet<u64> = mids
                                    .iter()
                                    .filter_map(|m| u64::try_from(m.0).ok())
                                    .collect();
                                if st.active_chat_id == Some(cid_u64) {
                                    let b = st.messages.len();
                                    st.messages.retain(|m| !id_set.contains(&m.id));
                                    purged_stage_count += b.saturating_sub(st.messages.len());
                                }
                                if let Some(h) = st.chat_histories.get_mut(&cid_u64) {
                                    let b = h.len();
                                    h.retain(|m| !id_set.contains(&m.id));
                                    purged_stage_count += b.saturating_sub(h.len());
                                }
                            }
                            purged_stage_count += st.purge_expired_messages_before(now_ms);
                            if purged_stage_count > 0 {
                                st_cx.notify();
                            }
                        });
                        if !purged_store.is_empty() || purged_stage_count > 0 {
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn stop_settings_mic_test_capture(&mut self, cx: &mut Context<Self>) {
        self.overlays.mic_test_loop_gen = self.overlays.mic_test_loop_gen.wrapping_add(1);
        if self.rtc_talk.active_call_id.is_none()
            && self.rtc_talk.talk_mode_overlay.is_none()
            && self.rtc_talk.voice_recorder.is_recording()
        {
            let _ = self.rtc_talk.voice_recorder.stop();
        }
        self.settings_router.update(cx, |sr, scx| {
            if let Some(ref sc) = sr.settings_calls {
                sc.update(scx, |sc, sc_cx| {
                    sc.stop_mic_test_silent();
                    sc_cx.notify();
                });
            }
        });
    }

    /// Spawns a background task monitoring `WsTransport` status changes for the current `self.client`.
    pub(crate) fn spawn_ws_status_observer(&self, cx: &mut Context<Self>) {
        let watched_client = self.client.clone();
        let mut status_rx = watched_client.ws_transport.subscribe_status();
        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            while status_rx.changed().await.is_ok() {
                let status = status_rx.borrow().clone();
                let is_connected = status.is_connected();
                let keep = weak_shell
                    .update(app, |shell, cx| {
                        if !Arc::ptr_eq(&shell.client, &watched_client) {
                            return false;
                        }
                        let was_connected = shell.nav_rail.read(cx).is_connected;
                        shell.nav_rail.update(cx, |r, _| r.set_connected(is_connected));
                        let banner = match &status {
                            ConnectionStatus::Connected => BannerConnectionStatus::Connected,
                            ConnectionStatus::Connecting => BannerConnectionStatus::Connecting,
                            ConnectionStatus::Reconnecting { .. } => BannerConnectionStatus::Updating,
                            ConnectionStatus::Offline { .. } => BannerConnectionStatus::WaitingForNetwork,
                        };
                        shell.window_runtime.set_connection_status(banner, None);
                        if matches!(
                            banner,
                            BannerConnectionStatus::Connecting
                                | BannerConnectionStatus::Updating
                                | BannerConnectionStatus::ProxyConnecting
                        ) {
                            shell.schedule_connecting_banner_grace_refresh(cx);
                        }
                        if !is_connected {
                            shell.coding_host.registered_for_auth_gen = None;
                        }
                        if is_connected && !was_connected {
                            let is_reconnect = shell.session_sync.has_connected_ws_once;
                            shell.session_sync.has_connected_ws_once = true;
                            let chat_list_fresh_in_flight = shell
                                .session_sync
                                .chat_list_in_flight_since
                                .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(3));
                            if is_reconnect || !chat_list_fresh_in_flight {
                                shell.refresh_chats(cx);
                            }
                            if let Some(cid) = shell.stage.read(cx).active_chat_id {
                                let history_fresh_in_flight = shell
                                    .session_sync
                                    .history_in_flight_chat
                                    .is_some_and(|(id, t)| {
                                        id == cid && t.elapsed() < std::time::Duration::from_secs(3)
                                    });
                                if is_reconnect || !history_fresh_in_flight {
                                    shell.load_historical_messages(cid, cx);
                                }
                            }
                            if shell.is_authenticated
                                && shell.coding_host.registered_for_auth_gen
                                    != Some(shell.session_sync.auth_generation)
                            {
                                shell.announce_coding_host_to_server(cx);
                            }
                        }
                        if let ConnectionStatus::Offline { ref reason } = status {
                            if reason.contains("401") || reason.contains("unauthorized") {
                                shell.logout(cx);
                                shell.auth_wizard.update(cx, |w, wcx| {
                                    w.set_login_error(Some(
                                        "Session expired or revoked from another device. Please sign in again."
                                            .to_string(),
                                    ));
                                    wcx.notify();
                                });
                            }
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn observe_drawer_and_modal_entities(&self, cx: &mut Context<Self>) {
        let mut prev_drawer_open = false;
        cx.observe(&self.main_menu_drawer, move |shell, drawer, cx| {
            let (is_open, cmd, bf, ma, upd) = drawer.update(cx, |d, _| {
                (
                    d.is_open,
                    d.take_command(),
                    d.take_botfather_open(),
                    d.take_miniapps_center(),
                    d.take_trigger_update(),
                )
            });
            if !prev_drawer_open && is_open {
                shell.occlude_browser_for_overlay(cx);
                cx.notify();
            }
            prev_drawer_open = is_open;
            if let Some(cmd) = cmd {
                shell.handle_main_menu_command(cmd, cx);
            }
            if bf {
                shell.open_botfather_chat(cx);
            }
            if ma {
                shell.open_miniapps_center(cx);
            }
            if upd {
                shell.trigger_one_click_update(cx);
            }
        })
        .detach();
        let mut prev_creation_dialog_open = false;
        let mut prev_modal_active = false;
        let mut prev_modal_active_field = ActiveTextField::Primary;
        let mut prev_modal_kind: Option<ModalDialogKind> = None;
        cx.observe(&self.modal_container, move |shell, modal, cx| {
            if let Some(outcome) = modal.update(cx, |m, _| m.take_outcome()) {
                shell.handle_modal_outcome(outcome, cx);
            }
            let (cur_modal_active, cur_creation_dialog_open, cur_modal_active_field, cur_modal_kind) = {
                let mc = modal.read(cx);
                let active = mc.is_modal_active();
                let creation = active
                    && matches!(
                        mc.dialog.kind,
                        Some(
                            ModalDialogKind::NewChannel
                                | ModalDialogKind::NewGroup
                                | ModalDialogKind::AddContact
                                | ModalDialogKind::Contacts
                        )
                    );
                (active, creation, mc.dialog.active_field, mc.dialog.kind.clone())
            };
            let cur_modal_owns_ime = shell.last_ime_modal_snapshot.as_ref().is_some_and(|(f, k)| {
                *f == cur_modal_active_field && *k == cur_modal_kind
            });
            if !prev_modal_active && cur_modal_active {
                if shell.is_modal_container_unoccluded(cx) && !cur_modal_owns_ime {
                    shell.clear_ime_and_blur_content_hub_for_transition(true, cx);
                } else {
                    shell.content_hub.update(cx, |h, hcx| {
                        if h.active_input != ContentHubInputTarget::None {
                            h.blur_input();
                            hcx.notify();
                        }
                    });
                }
                shell.sidebar.update(cx, |sb, scx| {
                    if sb.search_focused {
                        sb.search_focused = false;
                        scx.notify();
                    }
                });
                shell.stage.update(cx, |st, stcx| {
                    let mut changed = false;
                    if st.active_bot_card_field.is_some() {
                        st.blur_bot_card_input();
                        changed = true;
                    }
                    if st.search_open {
                        st.search_open = false;
                        changed = true;
                    }
                    if changed {
                        stcx.notify();
                    }
                });
                shell.inspector.update(cx, |ins, icx| {
                    if ins.is_bot_input_active() {
                        ins.blur_bot_input();
                        icx.notify();
                    }
                });
                shell.blur_composer_and_cancel_ime(cx);
                shell.blur_community_if_focused(cx);
                shell.reconcile_chat_scoped_toasts(cx);
            } else if prev_modal_active && !cur_modal_active && shell.is_modal_container_unoccluded(cx) {
                shell.clear_all_ime_preedits(cx);
            } else if prev_modal_active
                && cur_modal_active
                && (cur_modal_active_field != prev_modal_active_field || cur_modal_kind != prev_modal_kind)
                && shell.is_modal_container_unoccluded(cx)
                && !cur_modal_owns_ime
            {
                shell.clear_all_ime_preedits(cx);
            }
            prev_modal_active = cur_modal_active;
            prev_modal_active_field = cur_modal_active_field;
            prev_modal_kind = cur_modal_kind;
            if !prev_creation_dialog_open && cur_creation_dialog_open {
                shell.enter_compose_or_modal_isolation(cx);
            } else if prev_creation_dialog_open
                && !cur_creation_dialog_open
                && !shell.is_compose_or_creation_modal_active(cx)
            {
                shell.exit_compose_or_modal_isolation(true, cx);
            }
            if !cur_modal_active {
                shell.community_pending_share = None;
            }
            prev_creation_dialog_open = cur_creation_dialog_open;
        })
        .detach();
        let mut prev_studio_input = self.tasks_workspace.read(cx).active_input;
        let mut prev_studio_tab = self.tasks_workspace.read(cx).tab;
        cx.observe(&self.tasks_workspace, move |shell, tw, cx| {
            let (cur_input, cur_tab, act) =
                tw.update(cx, |w, _| (w.active_input, w.tab, w.take_action()));
            if shell.is_tasks_workspace_unoccluded(cx)
                && (cur_input != prev_studio_input
                    || (prev_studio_input != StudioInputTarget::None && cur_tab != prev_studio_tab))
            {
                shell.clear_all_ime_preedits(cx);
            }
            prev_studio_input = cur_input;
            prev_studio_tab = cur_tab;
            if let Some(act) = act {
                shell.handle_tasks_workspace_action(act, cx);
            }
        })
        .detach();
    }

    fn observe_nav_rail_entity(&self, cx: &mut Context<Self>) {
        cx.observe(&self.nav_rail, |shell, rail, cx| {
            let (mut dest, logout_req, trigger, menu_req, archive_req, theme_req, folder_req) =
                rail.update(cx, |r, _| {
                    let f = r.selected_folder_requested.take().and_then(|fid| {
                        r.custom_folders.iter().find(|i| i.folder_id == fid).map(|i| i.title.clone())
                    });
                    (
                        r.active_destination,
                        r.logout_requested,
                        r.trigger_counter,
                        std::mem::take(&mut r.menu_open_requested),
                        std::mem::take(&mut r.archive_open_requested),
                        std::mem::take(&mut r.theme_toggle_requested),
                        f,
                    )
                });
            if menu_req {
                shell.toggle_account_popover(cx);
                if shell.sidebar.read(cx).is_account_popover_open && shell.is_account_popover_unoccluded(cx) {
                    shell.clear_ime_and_blur_content_hub_for_transition(true, cx);
                    shell.blur_composer_and_cancel_ime(cx);
                    shell.blur_community_if_focused(cx);
                }
                cx.notify();
            }
            if archive_req {
                shell.clear_ime_and_blur_content_hub_for_transition(true, cx);
                shell.cleanup_settings_panel_if_active(cx);
                shell.leave_browser_panel_if_active(cx);
                shell.leave_community_panel_if_active(cx);
                shell.active_panel = ActiveWorkspacePanel::Chats;
                shell.sync_nav_rail_destination_silent(NavDestination::Chats, cx);
                dest = NavDestination::Chats;
                shell.sync_apple_sidebar_to_active_panel(cx);
                shell.sidebar.update(cx, |sb, _| sb.is_archived_expanded = !sb.is_archived_expanded);
                cx.notify();
            }
            if theme_req {
                let next = toggle_desktop_theme_mode(shell.theme_state.theme_mode);
                shell.apply_theme_selection(next, None, None, cx);
                shell.sidebar.update(cx, |_, sb_cx| sb_cx.notify());
            }
            if let Some(folder_title) = folder_req {
                shell.clear_ime_and_blur_content_hub_for_transition(true, cx);
                shell.cleanup_settings_panel_if_active(cx);
                shell.leave_browser_panel_if_active(cx);
                shell.leave_community_panel_if_active(cx);
                shell.active_panel = ActiveWorkspacePanel::Chats;
                shell.sync_nav_rail_destination_silent(NavDestination::Chats, cx);
                dest = NavDestination::Chats;
                shell.sync_apple_sidebar_to_active_panel(cx);
                shell.sidebar.update(cx, |sb, sb_cx| {
                    sb.select_folder(folder_title);
                    sb_cx.notify();
                });
                cx.notify();
            }
            if logout_req {
                rail.update(cx, |r, _| {
                    r.logout_requested = false;
                    r.is_connected = false;
                });
                shell.logout(cx);
                return;
            }
            if shell.last_handled_nav_trigger == trigger && shell.last_handled_nav_dest == Some(dest) {
                return;
            }
            shell.last_handled_nav_trigger = trigger;
            shell.activate_nav_destination(dest, cx);
        })
        .detach();
    }

    fn observe_sidebar_entity(&self, cx: &mut Context<Self>) {
        let mut prev_overlay_open = false;
        let mut prev_search_focused = false;
        let mut prev_contacts_modal_open = false;
        let mut prev_creation_key: Option<(
            SidebarCreationModal,
            AddContactKind,
            CreationInputField,
        )> = None;
        let mut prev_account_popover_open = false;
        let mut prev_compose_open = false;
        let mut prev_compose_title_focused = false;
        let mut prev_compose_or_modal = false;
        let mut prev_compose_query = String::new();
        cx.observe(&self.sidebar, move |shell, sb, cx| {
            let (
                selected_dialog, selection_rev, story_peer, search_focused, has_creation_modal,
                pending_contact, pending_group, search_query, search_cat, search_seq,
                open_menu, open_new_channel, context_action, is_collapsed, collapse_rev,
                open_botfather, install_open_app, open_app_only, open_bot_profile,
                launch_dialog_miniapp, open_search_hit,
            ) = sb.update(cx, |s, _| (
                s.selected_dialog().cloned(), s.selection_revision, s.story_clicked_peer.take(),
                s.search_focused, s.active_creation_modal.is_some(),
                s.pending_add_contact_request.take(), s.pending_create_group_request.take(),
                s.search_query.clone(), s.active_search_category.clone(), s.search_request_seq,
                s.take_open_main_menu_requested(), s.take_pending_open_new_channel(),
                s.take_pending_context_action(), s.is_collapsed, s.collapse_state_revision,
                std::mem::take(&mut s.pending_open_botfather),
                s.pending_install_open_miniapp.take(), s.pending_open_miniapp_only.take(),
                s.pending_open_bot_profile.take(), s.pending_launch_dialog_miniapp.take(),
                s.pending_open_search_hit.take(),
            ));
            let mut shell_dirty = false;
            if open_botfather {
                shell.open_botfather_chat(cx);
                shell_dirty = true;
            }
            if let Some(app_item) = install_open_app {
                shell.install_and_launch_miniapp(app_item, cx);
                shell_dirty = true;
            }
            if let Some(app_item) = open_app_only {
                if app_item.is_installed {
                    shell.launch_miniapp(
                        &app_item.bot_username, &app_item.short_name,
                        app_item.companion_chat_id, MiniAppLaunchMode::DirectLink, None, cx,
                    );
                } else {
                    shell.install_and_launch_miniapp(app_item, cx);
                }
                shell_dirty = true;
            }
            if let Some(bot_un) = open_bot_profile {
                shell.open_bot_profile_popup(&bot_un, cx);
                shell_dirty = true;
            }
            if let Some((cid, uri)) = launch_dialog_miniapp {
                let _ = shell.launch_miniapp_from_uri(
                    &uri,
                    Some(cid),
                    MiniAppLaunchMode::MainMiniApp,
                    cx,
                );
                shell_dirty = true;
            }
            if let Some(hit) = open_search_hit {
                shell.handle_sidebar_search_hit(hit, cx);
                shell_dirty = true;
            }
            if collapse_rev != shell.overlays.last_sidebar_collapse_revision {
                shell.overlays.last_sidebar_collapse_revision = collapse_rev;
                shell.update_preferences(cx, |p| p.appearance.sidebar_collapsed = is_collapsed);
                shell_dirty = true;
            }
            if open_menu {
                shell.toggle_account_popover(cx);
                shell_dirty = true;
            }
            let (
                sb_popup,
                cur_overlay_open,
                contacts_modal_open,
                account_popover_open,
                mut cur_compose_open,
                cur_compose_title_focused,
                cur_compose_query,
                cur_creation_key,
            ) = {
                let s = shell.sidebar.read(cx);
                let overlay_open = s.is_contacts_modal_open
                    || s.active_creation_modal.is_some()
                    || s.is_account_popover_open;
                (
                    s.is_plus_menu_open || overlay_open,
                    overlay_open,
                    s.is_contacts_modal_open,
                    s.is_account_popover_open,
                    s.is_plus_menu_open,
                    s.compose_title_focused,
                    s.compose_recipient_query.clone(),
                    s.active_creation_modal
                        .map(|m| (m, s.contact_kind, s.active_creation_field)),
                )
            };
            if !prev_overlay_open && cur_overlay_open {
                shell.occlude_browser_for_overlay(cx);
                shell.blur_community_if_focused(cx);
                shell_dirty = true;
            }
            if !prev_compose_open && cur_compose_open {
                if shell.theme_state.palette.is_apple_style()
                    && shell.active_panel != ActiveWorkspacePanel::Chats
                {
                    shell.cleanup_settings_panel_if_active(cx);
                    shell.leave_browser_panel_if_active(cx);
                    shell.leave_community_panel_if_active(cx);
                    shell.clear_ime_and_blur_content_hub_for_transition(true, cx);
                    shell.bot_browser.ime_preedit_input_gen = None;
                    shell.community_ime_preedit_input_gen = None;
                    shell.active_panel = ActiveWorkspacePanel::Chats;
                    shell.sync_nav_rail_destination_silent(NavDestination::Chats, cx);
                    shell.sync_apple_sidebar_to_active_panel(cx);
                } else {
                    shell.occlude_browser_for_overlay(cx);
                    if !shell.theme_state.palette.is_apple_style() {
                        if shell.is_base_workspace_unoccluded(cx) {
                            shell.occlude_community_for_overlay(cx);
                        } else {
                            shell.blur_community_if_focused(cx);
                        }
                    }
                }
                shell_dirty = true;
            } else if cur_compose_open
                && shell.theme_state.palette.is_apple_style()
                && shell.active_panel != ActiveWorkspacePanel::Chats
            {
                shell.sidebar.update(cx, |sb, scx| {
                    sb.cancel_compose_mode();
                    scx.notify();
                });
                if shell.composer.read(cx).is_compose_isolated
                    && !shell.is_compose_or_creation_modal_active(cx)
                {
                    shell.exit_compose_or_modal_isolation(true, cx);
                } else {
                    shell.sync_compose_state_to_composer(true, cx);
                }
                cur_compose_open = false;
                shell_dirty = true;
            }
            if !prev_search_focused && search_focused && shell.active_panel == ActiveWorkspacePanel::Browser {
                shell.ime.clear_preedit();
                shell.bot_browser.ime_preedit_input_gen = None;
                if let Some(ref bw) = shell.browser_workspace {
                    bw.update(cx, |w, wcx| {
                        if w.active_input != crate::views::browser::ActiveBrowserInput::None {
                            w.set_active_input(
                                crate::views::browser::ActiveBrowserInput::None,
                                None,
                                wcx,
                            );
                        }
                    });
                }
                shell_dirty = true;
            }
            if !prev_search_focused && search_focused && shell.active_panel == ActiveWorkspacePanel::Community {
                if shell.community_ime_preedit_input_gen.is_some() {
                    shell.ime.clear_preedit();
                    shell.community_ime_preedit_input_gen = None;
                }
                if let Some(ref cw) = shell.community_workspace {
                    cw.update(cx, |w, wcx| {
                        w.blur_active_input(wcx);
                    });
                }
                if shell.is_base_workspace_unoccluded(cx)
                    && shell.content_hub.read(cx).active_input
                        == ContentHubInputTarget::QuickDrawerSearch
                {
                    shell.content_hub.update(cx, |h, hcx| {
                        h.blur_input();
                        hcx.notify();
                    });
                }
                shell_dirty = true;
            }
            if sb_popup && shell.stage.read(cx).more_menu_open {
                shell.stage.update(cx, |st, st_cx| {
                    st.more_menu_open = false;
                    st.ttl_submenu_open = false;
                    st_cx.notify();
                });
                shell_dirty = true;
            }
            if open_new_channel {
                let title = tr_str(shell.active_lang_id(), "新建频道", "New Channel");
                shell.open_modal_dialog(title, ModalDialogKind::NewChannel, 420.0, 380.0, cx);
                shell_dirty = true;
            }
            if let Some(action) = context_action {
                shell.handle_sidebar_context_action(action, cx);
                shell_dirty = true;
            }
            let chats_unoccluded = shell.is_chats_inline_unoccluded(cx);
            let account_popover_unoccluded = shell.is_account_popover_unoccluded(cx);
            let sidebar_modal_unoccluded = shell.is_sidebar_modal_unoccluded(cx);
            let prev_creation_modal_open = prev_creation_key.is_some();
            let contacts_opening = !prev_contacts_modal_open && contacts_modal_open;
            let creation_opening = !prev_creation_modal_open && has_creation_modal;
            let account_popover_opening = !prev_account_popover_open && account_popover_open;
            let search_opening = !prev_search_focused && search_focused;
            let apple_compose_opening = !prev_compose_open
                && cur_compose_open
                && shell.theme_state.palette.is_apple_style();
            let apple_compose_field_switched = chats_unoccluded
                && prev_compose_open
                && cur_compose_open
                && shell.theme_state.palette.is_apple_style()
                && cur_compose_title_focused != prev_compose_title_focused;
            let creation_modal_closing_unoccluded =
                prev_creation_modal_open && !has_creation_modal && sidebar_modal_unoccluded;
            let contacts_modal_closing_unoccluded = prev_contacts_modal_open
                && !contacts_modal_open
                && sidebar_modal_unoccluded
                && !has_creation_modal;
            let creation_field_switched = prev_creation_key.is_some()
                && cur_creation_key.is_some()
                && sidebar_modal_unoccluded
                && cur_creation_key != prev_creation_key;
            let inline_or_popover_closing = (account_popover_unoccluded
                && prev_account_popover_open
                && !account_popover_open)
                || (chats_unoccluded
                    && ((prev_compose_open && !cur_compose_open)
                        || (prev_search_focused && !search_focused)));

            let global_modal_opening =
                sidebar_modal_unoccluded && (contacts_opening || creation_opening);
            let global_popover_opening = account_popover_unoccluded && account_popover_opening;
            let global_overlay_active =
                sidebar_modal_unoccluded && (has_creation_modal || contacts_modal_open);
            let inline_chats_opening = chats_unoccluded && (search_opening || apple_compose_opening);
            let sidebar_takes_focus =
                global_overlay_active || global_popover_opening || inline_chats_opening;
            let mut cleared_search_for_overlay = false;
            if sidebar_takes_focus {
                shell.blur_composer_and_cancel_ime(cx);
                let should_blur_hub = if global_overlay_active || global_popover_opening {
                    shell.content_hub.read(cx).active_input != ContentHubInputTarget::None
                } else {
                    shell.content_hub.read(cx).active_input
                        == ContentHubInputTarget::QuickDrawerSearch
                };
                if should_blur_hub {
                    shell.content_hub.update(cx, |h, hcx| {
                        h.blur_input();
                        hcx.notify();
                    });
                    shell.clear_all_ime_preedits(cx);
                    shell_dirty = true;
                }
                if (global_modal_opening || global_popover_opening) && search_focused {
                    shell.sidebar.update(cx, |s, scx| {
                        if s.search_focused {
                            s.search_focused = false;
                            scx.notify();
                        }
                    });
                    cleared_search_for_overlay = true;
                }
            }
            let sidebar_opening =
                global_modal_opening || global_popover_opening || inline_chats_opening;
            if sidebar_opening {
                shell.stage.update(cx, |st, stcx| {
                    let mut changed = false;
                    if st.active_bot_card_field.is_some() {
                        st.blur_bot_card_input();
                        changed = true;
                    }
                    if st.search_open {
                        st.search_open = false;
                        changed = true;
                    }
                    if changed {
                        stcx.notify();
                    }
                });
                shell.inspector.update(cx, |ins, icx| {
                    if ins.is_bot_input_active() {
                        ins.blur_bot_input();
                        icx.notify();
                    }
                });
            }
            if sidebar_opening
                || creation_modal_closing_unoccluded
                || contacts_modal_closing_unoccluded
                || creation_field_switched
                || apple_compose_field_switched
                || (inline_or_popover_closing && !other_input_owns_ime(shell, cx))
            {
                shell.clear_all_ime_preedits(cx);
                shell_dirty = true;
            }
            if let Some(req) = pending_contact {
                shell.dispatch_add_contact(req, cx);
                shell_dirty = true;
            }
            if let Some(req) = pending_group {
                shell.dispatch_create_group(req, cx);
                shell_dirty = true;
            }
            if search_focused && search_seq > 0 && shell.session_sync.last_dispatched_search_seq != search_seq {
                shell.dispatch_categorized_search(search_query, search_cat, search_seq, cx);
                shell_dirty = true;
            }
            if let Some(peer) = story_peer {
                shell.open_modal_dialog(peer.clone(), ModalDialogKind::StoriesViewer { peer_name: peer }, 420.0, 560.0, cx);
                shell_dirty = true;
            }
            let cur_compose_or_modal = shell.is_compose_or_creation_modal_active(cx);
            let mut selection_changed = false;
            if should_switch_to_chats_on_sidebar_selection(
                selection_rev, &mut shell.overlays.last_sidebar_selection_revision, selected_dialog.is_some(),
            ) {
                selection_changed = true;
                if let Some(d) = selected_dialog {
                    shell.clear_ime_and_blur_content_hub_for_transition(true, cx);
                    shell.cleanup_settings_panel_if_active(cx);
                    shell.leave_browser_panel_if_active(cx);
                    shell.leave_code_scene_panel_if_active(cx);
                    shell.leave_community_panel_if_active(cx);
                    shell.active_panel = ActiveWorkspacePanel::Chats;
                    shell.last_handled_nav_dest = Some(NavDestination::Chats);
                    let nav_trigger = shell.nav_rail.update(cx, |r, rcx| {
                        if r.active_destination != NavDestination::Chats {
                            r.set_active_destination(NavDestination::Chats);
                            rcx.notify();
                        }
                        r.trigger_counter
                    });
                    shell.last_handled_nav_trigger = nav_trigger;
                    shell.sync_apple_sidebar_to_active_panel(cx);
                    let (cur_active_id, cur_title) = {
                        let st = shell.stage.read(cx);
                        (st.active_chat_id, st.chat_title.clone())
                    };
                    let was_isolated = shell.composer.read(cx).is_compose_isolated;
                    if cur_active_id == Some(d.id) {
                        if was_isolated && !cur_compose_or_modal {
                            shell.exit_compose_or_modal_isolation(true, cx);
                        }
                        if cur_title != d.peer_name {
                            shell.sync_selected_chat_inspector_and_stage(d.id, cx);
                        }
                    } else {
                        // Keep `is_compose_isolated == true` across `sync_selected_chat_inspector_and_stage`
                        // so the previous chat's saved draft is never flushed/overwritten with `+` text.
                        shell.sync_selected_chat_inspector_and_stage(d.id, cx);
                        if was_isolated && !cur_compose_or_modal {
                            shell.finish_compose_isolation_for_switched_chat(d.id, cx);
                        } else {
                            shell.clear_compose_isolation_snapshot();
                        }
                    }
                    shell_dirty = true;
                }
            }
            if !prev_compose_or_modal && cur_compose_or_modal {
                if let Some(old_cid) = shell
                    .last_observed_toast_chat_id
                    .or_else(|| shell.stage.read(cx).active_chat_id)
                {
                    if shell.window_runtime.controller.clear_toasts_for_chat(old_cid) {
                        shell.schedule_current_toast_dismiss(cx);
                    }
                }
                shell.enter_compose_or_modal_isolation(cx);
                shell_dirty = true;
            } else if prev_compose_or_modal && !cur_compose_or_modal && !selection_changed {
                shell.exit_compose_or_modal_isolation(true, cx);
                shell_dirty = true;
            }
            if cur_overlay_open
                || prev_overlay_open
                || search_focused != prev_search_focused
                || cur_compose_open != prev_compose_open
                || cur_compose_title_focused != prev_compose_title_focused
                || cur_compose_or_modal != prev_compose_or_modal
                || cur_compose_query != prev_compose_query
            {
                shell_dirty = true;
            }
            prev_overlay_open = cur_overlay_open;
            prev_search_focused = if cleared_search_for_overlay {
                false
            } else {
                search_focused
            };
            prev_contacts_modal_open = contacts_modal_open;
            prev_creation_key = cur_creation_key;
            prev_account_popover_open = account_popover_open;
            prev_compose_open = cur_compose_open;
            prev_compose_title_focused = cur_compose_title_focused;
            prev_compose_or_modal = cur_compose_or_modal;
            prev_compose_query = cur_compose_query;
            shell.sync_apple_sidebar_badges(cx);
            if shell_dirty {
                cx.notify();
            }
        })
        .detach();
    }

    fn handle_sidebar_search_hit(&mut self, hit: CategorizedSearchResult, cx: &mut Context<Self>) {
        if hit.category.eq_ignore_ascii_case("Apps") {
            let chat_opt = (hit.chat_id > 0).then_some(hit.chat_id);
            if !self.launch_miniapp_from_uri(&hit.subtitle, chat_opt, MiniAppLaunchMode::DirectLink, cx) {
                let found = self.miniapps.catalog_miniapps.iter().find(|a| {
                    a.title.eq_ignore_ascii_case(&hit.title)
                        || hit.subtitle.to_lowercase().contains(&a.bot_username.to_lowercase())
                }).cloned();
                if let Some(app) = found {
                    self.launch_miniapp(
                        &app.bot_username, &app.short_name,
                        app.direct_chat_id.filter(|&id| id > 0).map(|id| id as u64),
                        MiniAppLaunchMode::DirectLink, None, cx,
                    );
                }
            }
        } else if hit.is_bot {
            let candidate_un = hit.subtitle.split_whitespace().find(|w| w.starts_with('@')).map(|w| {
                w.trim_start_matches('@').trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
            }).filter(|s| !s.is_empty());
            if let Some(un) = candidate_un {
                self.open_bot_chat_by_username(un, cx);
            } else if hit.chat_id > 0 {
                self.open_or_select_chat(hit.chat_id, Some(hit.title), true, cx);
            } else {
                self.open_bot_chat_by_username(&hit.title, cx);
            }
        } else if hit.chat_id > 0 {
            self.open_or_select_chat(hit.chat_id, Some(hit.title), hit.is_bot, cx);
        }
    }

    fn observe_stage_composer_inspector_entities(&self, cx: &mut Context<Self>) {
        let mut prev_stage_search_open = false;
        let mut prev_stage_search_focus_seq = 0u64;
        let mut prev_card_field: Option<(String, crate::views::stage::cards::BotAssistLoginField)> =
            None;
        cx.observe(&self.stage, move |shell, st, cx| {
            let (
                bot_start_clicked,
                bot_assist_act,
                cur_card_field,
                cur_chat_id,
                show_bot_start_bar,
                stage_search_open,
                stage_search_focus_seq,
            ) = st.update(cx, |s, _| {
                (
                    std::mem::take(&mut s.pending_bot_start),
                    s.pending_bot_assist_action.take(),
                    s.active_bot_card_field.clone(),
                    s.active_chat_id,
                    s.show_bot_start_bar,
                    s.search_open,
                    s.search_focus_seq,
                )
            });
            let chats_unoccluded = shell.is_chats_inline_unoccluded(cx);
            let prev_card_active = prev_card_field.is_some();
            let card_input_active = cur_card_field.is_some();
            let stage_search_focused_now = (!prev_stage_search_open && stage_search_open)
                || (stage_search_open && stage_search_focus_seq != prev_stage_search_focus_seq);
            let card_input_opening = !prev_card_active && card_input_active;
            let card_field_switched =
                prev_card_active && card_input_active && cur_card_field != prev_card_field;
            let stage_input_closing = (prev_stage_search_open && !stage_search_open)
                || (prev_card_active && !card_input_active);
            let mut blurred_card_for_search = false;
            let mut blurred_search_for_card = false;
            if chats_unoccluded && (stage_search_focused_now || card_input_opening) {
                if shell.content_hub.read(cx).active_input
                    == ContentHubInputTarget::QuickDrawerSearch
                {
                    shell.content_hub.update(cx, |h, hcx| {
                        h.blur_input();
                        hcx.notify();
                    });
                }
                shell.clear_all_ime_preedits(cx);
            } else if chats_unoccluded && card_field_switched {
                shell.clear_all_ime_preedits(cx);
            } else if chats_unoccluded
                && stage_input_closing
                && !other_input_owns_ime(shell, cx)
            {
                shell.clear_all_ime_preedits(cx);
            }
            if chats_unoccluded && (stage_search_focused_now || card_input_opening) {
                shell.sidebar.update(cx, |sb, scx| {
                    if sb.search_focused {
                        sb.search_focused = false;
                        scx.notify();
                    }
                });
                shell.inspector.update(cx, |ins, icx| {
                    if ins.is_bot_input_active() {
                        ins.blur_bot_input();
                        icx.notify();
                    }
                });
                if stage_search_focused_now && card_input_active {
                    st.update(cx, |s, stcx| {
                        s.blur_bot_card_input();
                        stcx.notify();
                    });
                    blurred_card_for_search = true;
                } else if card_input_opening && stage_search_open {
                    st.update(cx, |s, stcx| {
                        s.search_open = false;
                        stcx.notify();
                    });
                    blurred_search_for_card = true;
                }
            }
            prev_stage_search_open = if blurred_search_for_card {
                false
            } else {
                stage_search_open
            };
            prev_stage_search_focus_seq = stage_search_focus_seq;
            prev_card_field = if blurred_card_for_search {
                None
            } else {
                cur_card_field
            };
            if let Some(old_cid) = shell.last_observed_toast_chat_id {
                if cur_chat_id != Some(old_cid) {
                    shell.composer.update(cx, |c, _| c.clear_bot_reply_keyboard());
                }
            }
            shell.reconcile_chat_scoped_toasts(cx);
            let is_apple_compose_mode = shell.theme_state.palette.is_apple_style()
                && shell.sidebar.read(cx).is_plus_menu_open;
            if (chats_unoccluded && (card_input_opening || stage_search_focused_now))
                || (!is_apple_compose_mode && show_bot_start_bar)
            {
                shell.blur_composer_and_cancel_ime(cx);
            }
            if bot_start_clicked {
                shell.materialize_prestart_bot_and_start(cx);
            }
            if let Some(act) = bot_assist_act {
                shell.handle_bot_assist_card_action(act, cx);
            }
            shell.handle_stage_pending_actions(cx);
            let insp_open = st.read(cx).is_inspector_open;
            shell.inspector.update(cx, |insp, insp_cx| {
                if insp.is_open != insp_open {
                    insp.is_open = insp_open;
                    insp_cx.notify();
                }
            });
            cx.notify();
        })
        .detach();
        cx.observe(&self.composer, move |shell, comp, cx| {
            let (att_opt, focused, launch_miniapp, kb_press, selected_send_as, comp_isolated) =
                comp.update(cx, |c, _| {
                    (
                        c.submitted_attachment.take(),
                        c.is_input_focused,
                        c.take_launch_miniapp(),
                        c.take_bot_keyboard_press(),
                        c.selected_send_as_peer_id,
                        c.is_compose_isolated,
                    )
                });
            let is_compose_or_modal = shell.is_compose_or_creation_modal_active(cx);
            let composer_focus_active = focused
                && shell.is_chats_inline_unoccluded(cx)
                && !shell.should_hide_composer(cx);
            let was_focused = shell.prev_composer_focused.get();
            let composer_rising_edge = !was_focused && composer_focus_active;
            let composer_falling_edge = was_focused && !composer_focus_active;
            shell.prev_composer_focused.set(composer_focus_active);
            if composer_rising_edge {
                shell.sidebar.update(cx, |sb, scx| {
                    if sb.search_focused {
                        sb.search_focused = false;
                        scx.notify();
                    }
                });
                shell.stage.update(cx, |st, stcx| {
                    let mut changed = false;
                    if st.active_bot_card_field.is_some() {
                        st.blur_bot_card_input();
                        changed = true;
                    }
                    if st.search_open {
                        st.search_open = false;
                        changed = true;
                    }
                    if changed {
                        stcx.notify();
                    }
                });
                shell.inspector.update(cx, |ins, icx| {
                    if ins.is_bot_input_active() {
                        ins.blur_bot_input();
                        icx.notify();
                    }
                });
                if shell.content_hub.read(cx).active_input
                    == ContentHubInputTarget::QuickDrawerSearch
                {
                    shell.content_hub.update(cx, |h, hcx| {
                        h.blur_input();
                        hcx.notify();
                    });
                }
                shell.clear_all_ime_preedits(cx);
            } else if composer_falling_edge && comp.read(cx).ime_machine.is_composing() {
                comp.update(cx, |c, ccx| {
                    c.ime_machine.on_ime_cancel();
                    c.sync_autocomplete();
                    ccx.notify();
                });
            }
            if !is_compose_or_modal && !comp_isolated {
                if let (Some(chat_id), Some(pid)) = (shell.stage.read(cx).active_chat_id, selected_send_as) {
                    if shell.session_sync.send_as_peers.chosen(chat_id) != Some(pid) {
                        shell.session_sync.send_as_peers.set_chosen(chat_id, pid);
                    }
                }
            }
            if let Some(kind) = att_opt {
                shell.append_outgoing_attachment(&kind, cx);
            }
            if let Some((bot_id, bot_un, short_name, url)) = launch_miniapp {
                shell.launch_miniapp_from_composer(bot_id, &bot_un, &short_name, &url, cx);
            }
            if let Some(btn) = kb_press {
                if !is_compose_or_modal && !comp_isolated {
                    match btn.kind {
                        BotKeyboardButtonKind::WebApp(url) => {
                            if !shell.try_handle_or_swallow_apple_deep_link(&url, false, cx) {
                                shell.launch_miniapp_from_composer(0, "", "", &url, cx);
                            }
                        }
                        BotKeyboardButtonKind::Url(url) => {
                            if !shell.try_handle_or_swallow_apple_deep_link(&url, false, cx) {
                                let chat_opt = shell.stage.read(cx).active_chat_id;
                                if !shell.launch_miniapp_from_uri(
                                    &url,
                                    chat_opt,
                                    MiniAppLaunchMode::InlineKeyboard,
                                    cx,
                                ) {
                                    shell.enqueue_localized_toast(
                                        "已触发链接按钮",
                                        "Link triggered",
                                        1800,
                                        cx,
                                    );
                                }
                            }
                        }
                        BotKeyboardButtonKind::Callback(bytes) => {
                            let handled_apple = match std::str::from_utf8(&bytes) {
                                Ok(s) => shell.try_handle_or_swallow_apple_deep_link(s, true, cx),
                                Err(_) => crate::views::app_shell::shell_content_hub::is_apple_deep_link_candidate(
                                    &String::from_utf8_lossy(&bytes),
                                ),
                            };
                            if !handled_apple && !btn.text.trim().is_empty() {
                                shell.append_outgoing_text(btn.text, cx);
                            }
                        }
                        BotKeyboardButtonKind::SwitchInline { query, .. } => {
                            let focus_composer = shell.can_focus_chat_composer(cx);
                            shell.composer.update(cx, |c, ccx| {
                                c.set_text(query);
                                c.is_input_focused = focus_composer;
                                ccx.notify();
                            });
                        }
                        BotKeyboardButtonKind::Default
                        | BotKeyboardButtonKind::RequestPhone
                        | BotKeyboardButtonKind::RequestLocation
                        | BotKeyboardButtonKind::RequestPoll
                        | BotKeyboardButtonKind::Buy => {
                            if !btn.text.trim().is_empty() {
                                shell.append_non_composer_outgoing_text(btn.text, cx);
                            }
                        }
                    }
                }
            }
            shell.handle_composer_pending_actions(cx);
            if !is_compose_or_modal && !comp_isolated {
                shell.handle_composer_draft_change(cx);
            }
            cx.notify();
        })
        .detach();
        let mut prev_bot_focus: Option<(
            InspectorTab,
            InspectorSubpage,
            BotConfigFocusField,
            bool,
            u64,
        )> = None;
        cx.observe(&self.inspector, move |shell, insp, cx| {
            let (is_open, cur_bot_focus, media_open, action, leave_chat) = insp.update(cx, |i, _| {
                let focus = i.is_bot_input_active().then_some((
                    i.active_tab,
                    i.subpage,
                    i.bot_config_focus,
                    i.is_add_member_search_active(),
                    i.profile.bot_id,
                ));
                (
                    i.is_open,
                    focus,
                    i.pending_media_open.take(),
                    i.take_action(),
                    i.take_pending_leave_chat(),
                )
            });
            let chats_unoccluded = shell.is_chats_inline_unoccluded(cx);
            let prev_bot_active = prev_bot_focus.is_some();
            let cur_bot_active = cur_bot_focus.is_some();
            let bot_input_opening = !prev_bot_active && cur_bot_active;
            let bot_input_switched =
                prev_bot_active && cur_bot_active && cur_bot_focus != prev_bot_focus;
            if chats_unoccluded && bot_input_opening {
                if shell.content_hub.read(cx).active_input
                    == ContentHubInputTarget::QuickDrawerSearch
                {
                    shell.content_hub.update(cx, |h, hcx| {
                        h.blur_input();
                        hcx.notify();
                    });
                }
                shell.clear_all_ime_preedits(cx);
            } else if chats_unoccluded && bot_input_switched {
                shell.clear_all_ime_preedits(cx);
            } else if chats_unoccluded
                && prev_bot_active
                && !cur_bot_active
                && !other_input_owns_ime(shell, cx)
            {
                shell.clear_all_ime_preedits(cx);
            }
            if chats_unoccluded && bot_input_opening {
                shell.blur_composer_and_cancel_ime(cx);
                shell.sidebar.update(cx, |sb, scx| {
                    if sb.search_focused {
                        sb.search_focused = false;
                        scx.notify();
                    }
                });
                shell.stage.update(cx, |st, stcx| {
                    let mut changed = false;
                    if st.active_bot_card_field.is_some() {
                        st.blur_bot_card_input();
                        changed = true;
                    }
                    if st.search_open {
                        st.search_open = false;
                        changed = true;
                    }
                    if changed {
                        stcx.notify();
                    }
                });
            }
            prev_bot_focus = cur_bot_focus;
            let skip_dup_media_open = matches!(
                (&media_open, &action),
                (
                    Some((u1, v1)),
                    Some(InspectorAction::OpenMedia { url: u2, is_video: v2 })
                ) if u1 == u2 && v1 == v2
            );
            if let Some((url, is_video)) = media_open {
                if !skip_dup_media_open {
                    shell.show_media_viewer(url, is_video, cx);
                }
            }
            if let Some(act) = action {
                shell.handle_inspector_action(act, cx);
            }
            if leave_chat {
                shell.open_leave_or_delete_chat_modal(cx);
            }
            shell.stage.update(cx, |st, _| st.is_inspector_open = is_open);
            cx.notify();
        })
        .detach();
    }

    /// Dispatches events emitted by `AuthWizardView` (authentication, server URL updates, theme/locale toggles, toasts).
    pub fn handle_auth_wizard_event(&mut self, ev: &AuthWizardEvent, cx: &mut Context<Self>) {
        match ev {
            AuthWizardEvent::Authenticated(auth) => {
                let vault_bootstrap = self
                    .auth_wizard
                    .update(cx, |w, _| w.take_pending_vault_bootstrap());
                if self.is_authenticated {
                    drop(vault_bootstrap);
                    return;
                }
                self.apply_authenticated_session_with_vault(auth.clone(), vault_bootstrap, cx);
            }
            AuthWizardEvent::ServerUrlApplied(url) => {
                self.proxy_op_generation = self.proxy_op_generation.wrapping_add(1);
                let wizard_client = self.auth_wizard.read(cx).client.clone();
                let reuse_wizard_client = !Arc::ptr_eq(&wizard_client, &self.client)
                    && wizard_client.config.server_url == *url;
                self.client.local_store.clear_all();
                self.client.clear_session();
                let next_client = if reuse_wizard_client {
                    wizard_client
                } else {
                    let cache_dir = self.client.local_store.cache_dir().map(|p| p.to_path_buf());
                    let mut new_cfg = self.client.config.clone();
                    new_cfg.server_url = url.clone();
                    Arc::new(match cache_dir {
                        Some(dir) => TelegramClient::open_with_dir(new_cfg.clone(), dir)
                            .unwrap_or_else(|_| TelegramClient::new(new_cfg)),
                        None => TelegramClient::new(new_cfg),
                    })
                };
                next_client
                    .ws_transport
                    .set_max_reconnect_attempts(next_client.config.max_reconnect_attempts);
                self.client = next_client;
                if let Some(ref handler) = self.remote_host.ipc_handler {
                    handler.set_client(self.client.clone());
                    handler.set_local_node_id(None);
                }
                let client_for_wizard = self.client.clone();
                self.auth_wizard.update(cx, |w, _| {
                    w.sync_client_for_server_url(client_for_wizard, url.clone());
                });
                self.computers.reset_on_logout();
                self.session_sync.ws_stream_started = false;
                self.coding_host.registered_for_auth_gen = None;
                self.active_panel = ActiveWorkspacePanel::Chats;
                self.panel_state.reset();
                self.window_runtime.controller.clear_all_toasts();
                self.scheduled_toast_id = None;
                self.last_observed_toast_chat_id = None;
                self.compose_isolation_snapshot = None;
                self.reset_conversation_views(url, None, cx);
                self.reset_composer_for_account_boundary(cx);
                self.reset_tasks_workspace_for_account_boundary(cx);
                self.community_workspace = None;
                self.community_pending_share = None;
                self.community_ime_preedit_input_gen = None;
                self.reset_content_hub_for_logout_or_account_switch(cx);
                self.reset_nav_rail_for_logout_or_account_switch(cx);
                self.main_menu_drawer.update(cx, |m, _| {
                    m.close();
                    m.accounts.clear();
                    m.active_account_id = 0;
                });
                self.modal_container.update(cx, |m, mcx| {
                    let was_active = m.is_modal_active();
                    m.reset_for_logout_or_account_switch();
                    if was_active {
                        mcx.notify();
                    }
                });
                self.clear_all_ime_preedits(cx);
                self.stop_settings_mic_test_capture(cx);
                self.settings_router.update(cx, |r, rcx| {
                    r.reset_for_logout_or_account_switch(rcx);
                });
                self.spawn_ws_status_observer(cx);
                self.sync_settings_advanced_to_active_proxy(cx);
                cx.notify();
            }
            AuthWizardEvent::Toast { message, duration_ms } => {
                self.show_non_chat_toast(message.clone(), *duration_ms, cx);
            }
            AuthWizardEvent::ToggleTheme => {
                let next = toggle_desktop_theme_mode(self.theme_state.theme_mode);
                self.apply_theme_selection(next, None, None, cx);
                self.sync_auth_wizard_visuals(cx);
            }
            AuthWizardEvent::ToggleLanguage => {
                self.toggle_language_pref(cx);
                self.sync_auth_wizard_visuals(cx);
            }
        }
    }

    fn toggle_language_pref(&mut self, cx: &mut Context<Self>) {
        let next = toggled_language_code(&self.preferences.localization.language_code).to_string();
        self.update_preferences(cx, |p| p.localization.language_code = next);
    }

    /// Dispatches high-level actions and preference mutations emitted by `SettingsRouter`.
    pub fn handle_settings_router_event(&mut self, ev: &SettingsRouterEvent, cx: &mut Context<Self>) {
        match ev {
            SettingsRouterEvent::CloseRequested => {
                self.stop_settings_mic_test_capture(cx);
                self.close_settings(cx);
            }
            SettingsRouterEvent::LogoutRequested => {
                self.stop_settings_mic_test_capture(cx);
                self.logout(cx);
            }
            SettingsRouterEvent::LockAppNow => {
                self.stop_settings_mic_test_capture(cx);
                self.lock_app_now(cx);
            }
            SettingsRouterEvent::CopyClipboard(txt) => {
                cx.write_to_clipboard(ClipboardItem::new_string(txt.clone()));
                self.enqueue_localized_non_chat_toast("已复制到剪贴板", "Copied to clipboard", 1800, cx);
            }
            SettingsRouterEvent::UpdateInterfaceScale(pct) => {
                let pct = *pct;
                self.update_preferences(cx, |p| p.interface_scale_pct = pct);
            }
            SettingsRouterEvent::OpenModal { title, kind, width, height } =>
                self.open_modal_dialog(title.clone(), kind.clone(), *width, *height, cx),
            SettingsRouterEvent::ApplyTheme { mode, accent, wallpaper } =>
                self.apply_theme_selection(*mode, Some(*accent), Some(wallpaper.clone()), cx),
            SettingsRouterEvent::UpdateChatPrefs {
                font_size,
                bubble_radius,
                send_on_enter,
                suggest_stickers,
                loop_animated_stickers,
                blur_wallpaper,
            } => {
                self.theme_state.bubble_radius_px = *bubble_radius;
                self.composer.update(cx, |c, ccx| {
                    c.send_on_enter = *send_on_enter;
                    c.suggest_stickers_by_emoji = *suggest_stickers;
                    c.loop_animated_stickers = *loop_animated_stickers;
                    c.stickers_list.loop_animated = *loop_animated_stickers;
                    ccx.notify();
                });
                self.stage.update(cx, |st, stcx| {
                    st.loop_animated_stickers = *loop_animated_stickers;
                    stcx.notify();
                });
                self.update_preferences(cx, |p| {
                    p.appearance.font_size = *font_size;
                    p.appearance.bubble_radius = *bubble_radius;
                    p.appearance.send_on_enter = *send_on_enter;
                    p.appearance.suggest_stickers_by_emoji = *suggest_stickers;
                    p.appearance.loop_animated_stickers = *loop_animated_stickers;
                    p.appearance.blur_wallpaper = *blur_wallpaper;
                });
            }
            SettingsRouterEvent::SaveProfile { first_name, last_name, username, phone, bio } => {
                let req = ProtoUpdateProfileRequest {
                    first_name: Some(first_name.clone()), last_name: Some(last_name.clone()),
                    username: username.clone(), phone: Some(phone.clone()), bio: Some(bio.clone()),
                    avatar_media_id: None, emoji_status: None,
                };
                self.spawn_authed_shell_fut(
                    cx,
                    move |fence| async move {
                        fence
                            .client
                            .update_profile_with_token(&fence.token, fence.client_epoch, &req)
                            .await
                    },
                    |s, res, cx| match res {
                    Ok(_) => {
                        let msg = tr_str(s.active_lang_id(), "个人资料已保存", "Profile saved").to_string();
                        let info = s.settings_router.update(cx, |r, rcx| r.ensure_information(rcx));
                        info.update(cx, |is, icx| { is.status_message = Some(msg.clone()); icx.notify(); });
                        s.show_non_chat_toast(msg, 2200, cx);
                        s.refresh_settings_truth(cx);
                    }
                    Err(e) => { s.show_non_chat_toast(format!("Save failed: {e}"), 2600, cx); cx.notify(); }
                });
            }
            SettingsRouterEvent::TwoFactorMutation(pw_opt) => {
                let pw_opt = pw_opt.clone();
                let enabled = pw_opt.is_some();
                self.spawn_authed_shell_fut(
                    cx,
                    move |fence| async move {
                        match pw_opt {
                            Some(pw) => fence
                                .client
                                .set_2fa_with_token(
                                    &fence.token,
                                    fence.client_epoch,
                                    &TwoFactorSetRequest {
                                        current_password: None,
                                        new_password: pw,
                                        hint: None,
                                        recovery_email: None,
                                    },
                                )
                                .await
                                .is_ok(),
                            None => fence
                                .client
                                .disable_2fa_with_token(&fence.token, fence.client_epoch, None)
                                .await
                                .is_ok(),
                        }
                    },
                    move |s, ok, cx| {
                    if !ok {
                        s.enqueue_localized_non_chat_toast("两步验证操作失败", "Failed to update 2FA", 2400, cx);
                        return;
                    }
                    let pe = s.settings_router.update(cx, |r, rcx| r.ensure_privacy(rcx));
                    pe.update(cx, |ps, pcx| { ps.set_two_factor_enabled(enabled); pcx.notify(); });
                    let (zh, en) = if enabled {
                        ("两步验证密码已启用", "Two-Step Verification enabled")
                    } else {
                        ("两步验证已关闭", "Two-Step Verification disabled")
                    };
                    s.enqueue_localized_non_chat_toast(zh, en, 2200, cx);
                });
            }
            SettingsRouterEvent::PushUserSettings => self.push_user_settings_to_backend(cx),
            SettingsRouterEvent::RevokeSession(sid) => {
                let sid = sid.clone();
                self.spawn_authed_shell_fut(
                    cx,
                    move |fence| async move {
                        let _ = fence
                            .client
                            .revoke_session_with_token(&fence.token, fence.client_epoch, &sid)
                            .await;
                    },
                    |s, _, cx| {
                    s.enqueue_localized_non_chat_toast("已注销该设备会话", "Session terminated", 2000, cx);
                    s.refresh_settings_truth(cx);
                });
            }
            SettingsRouterEvent::RevokeOtherSessions => {
                self.spawn_authed_shell_fut(
                    cx,
                    |fence| async move {
                        let _ = fence
                            .client
                            .revoke_other_sessions_with_token(&fence.token, fence.client_epoch)
                            .await;
                    },
                    |s, _, cx| {
                    s.enqueue_localized_non_chat_toast("已注销所有其他设备", "Terminated all other sessions", 2200, cx);
                    s.refresh_settings_truth(cx);
                });
            }
            SettingsRouterEvent::UnblockPeer { user_id, .. } => {
                let uid = *user_id;
                self.spawn_authed_shell_fut(
                    cx,
                    move |fence| async move {
                        let _ = fence
                            .client
                            .unblock_user_with_token(&fence.token, fence.client_epoch, uid)
                            .await;
                    },
                    |s, _, cx| {
                    s.enqueue_localized_non_chat_toast("已取消屏蔽该用户", "User unblocked", 2000, cx);
                    s.refresh_chats(cx);
                });
            }
            SettingsRouterEvent::UpdateLocalPasscode { disable, hash, auto_lock_seconds } => {
                if *disable {
                    self.overlays.is_passcode_locked = false;
                    self.overlays.passcode_unlock_input.clear();
                    self.overlays.passcode_unlock_error = None;
                    self.update_preferences(cx, |p| p.local_passcode_hash = None);
                    self.push_user_settings_to_backend(cx);
                    self.enqueue_localized_non_chat_toast("本地密码锁已关闭", "Local passcode disabled", 2000, cx);
                } else {
                    self.update_preferences(cx, |p| {
                        p.local_passcode_hash = hash.clone();
                        p.auto_lock_seconds = u64::from(*auto_lock_seconds);
                    });
                }
            }
            SettingsRouterEvent::UpdateGlobalTtl(_) => { self.push_user_settings_to_backend(cx); cx.notify(); }
            SettingsRouterEvent::ApplyGlobalTtlToExistingChats(ttl) => {
                let ttl_secs = *ttl;
                let direct_chat_ids: Vec<u64> = {
                    let sb = self.sidebar.read(cx);
                    let saved_id = sb.saved_messages_id;
                    sb.dialogs
                        .iter()
                        .filter(|d| {
                            Some(d.id) != saved_id
                                && d.folder != "Groups"
                                && d.folder != "Channels"
                                && !self
                                    .session_sync
                                    .rich_chats_by_id
                                    .get(&d.id)
                                    .is_some_and(|rc| {
                                        rc.is_group
                                            || rc.is_channel
                                            || matches!(rc.chat_type, 1 | 2 | 3 | 5)
                                            || rc.kind.as_deref() == Some("saved")
                                    })
                        })
                        .map(|d| d.id)
                        .collect()
                };
                for &cid in &direct_chat_ids {
                    if let Some(rc) = self.session_sync.rich_chats_by_id.get_mut(&cid) {
                        rc.message_ttl_seconds = ttl_secs;
                    }
                }
                let active_cid = self.stage.read(cx).active_chat_id;
                if active_cid.is_some_and(|cid| direct_chat_ids.contains(&cid)) {
                    self.stage.update(cx, |st, st_cx| {
                        st.message_ttl_seconds = ttl_secs;
                        st_cx.notify();
                    });
                }
                if let Some(fence) = self.capture_client_fence() {
                    let ids = direct_chat_ids.clone();
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        for cid in ids {
                            if !fence.preflight_ui(&weak, app).await {
                                break;
                            }
                            let _ = fence
                                .client
                                .set_chat_ttl_with_token(
                                    &fence.token,
                                    fence.client_epoch,
                                    cid as i64,
                                    ttl_secs,
                                )
                                .await;
                        }
                    })
                    .detach();
                }
                let msg = if self.active_lang_id().starts_with("zh") {
                    format!("已将自动删除规则应用到 {} 个私聊会话", direct_chat_ids.len())
                } else {
                    format!(
                        "Applied auto-delete timer to {} private chats",
                        direct_chat_ids.len()
                    )
                };
                self.show_non_chat_toast(msg, 2200, cx);
                cx.notify();
            }
            SettingsRouterEvent::UpdateNotificationPrefs {
                private_chats, groups, channels, sound_enabled, flash_taskbar,
                include_muted_in_badge, badge_count_messages,
            } => {
                self.sidebar.update(cx, |sb, sb_cx| {
                    sb.set_notification_badge_rules(*badge_count_messages, *include_muted_in_badge);
                    sb_cx.notify();
                });
                self.update_preferences(cx, |p| {
                    p.notifications.private_chats = *private_chats;
                    p.notifications.groups = *groups;
                    p.notifications.channels = *channels;
                    p.notifications.sound_enabled = *sound_enabled;
                    p.notifications.flash_taskbar = *flash_taskbar;
                    p.notifications.include_muted_in_badge = *include_muted_in_badge;
                    p.notifications.badge_count_messages = *badge_count_messages;
                });
                self.push_user_settings_to_backend(cx);
            }
            SettingsRouterEvent::UpdateNotificationReactionsPrefs {
                notify_reactions,
                notify_stories,
                contacts_only,
                sound_enabled,
            } => {
                self.update_preferences(cx, |p| {
                    p.notifications.reaction_notifications = *notify_reactions;
                    p.notifications.story_reaction_notifications = *notify_stories;
                    p.notifications.reaction_contacts_only = *contacts_only;
                    p.notifications.reaction_sound_enabled = *sound_enabled;
                });
                self.push_user_settings_to_backend(cx);
            }
            SettingsRouterEvent::UpdateNotificationTypePrefs {
                alerts_enabled,
                preview_enabled,
                sound_enabled,
            } => {
                self.update_preferences(cx, |p| {
                    p.notifications.type_alerts_enabled = *alerts_enabled;
                    p.notifications.type_preview_enabled = *preview_enabled;
                    p.notifications.type_sound_enabled = *sound_enabled;
                });
                self.push_user_settings_to_backend(cx);
            }
            SettingsRouterEvent::UpdateFolderTags(on) => {
                self.sidebar.update(cx, |sb, sb_cx| {
                    sb.folder_tags_enabled = *on;
                    sb_cx.notify();
                });
                self.update_preferences(cx, |p| p.folder_tags_enabled = *on);
            }
            SettingsRouterEvent::CreateFolder { id, title } => {
                let dto = ChatFolderDto {
                    id: *id as i64, title: title.clone(),
                    include_contacts: true, include_non_contacts: true,
                    include_groups: true, include_channels: true, include_bots: true,
                    ..Default::default()
                };
                self.spawn_authed_shell_fut(
                    cx,
                    move |fence| async move {
                        let _ = fence
                            .client
                            .save_folder_with_token(&fence.token, fence.client_epoch, &dto)
                            .await;
                    },
                    |s, _, cx| s.refresh_folders(cx),
                );
            }
            SettingsRouterEvent::DeleteFolder(fid) => {
                let fid = *fid as i64;
                self.spawn_authed_shell_fut(
                    cx,
                    move |fence| async move {
                        let _ = fence
                            .client
                            .delete_folder_with_token(&fence.token, fence.client_epoch, fid)
                            .await;
                    },
                    |s, _, cx| s.refresh_folders(cx),
                );
            }
            SettingsRouterEvent::UpdateAdvancedPrefs {
                ask_download_path, opengl_enabled, native_window_frame, tray_icon_enabled, auto_update_enabled,
            } => self.update_preferences(cx, |p| {
                p.ask_download_path = *ask_download_path;
                p.opengl_enabled = *opengl_enabled;
                p.native_window_frame = *native_window_frame;
                p.tray_icon_enabled = *tray_icon_enabled;
                p.auto_update_enabled = *auto_update_enabled;
            }),
            SettingsRouterEvent::OpenExportDataModal => {
                let label = tr_str(self.active_lang_id(), "导出聊天数据", "Export Chat Data");
                let title = self.stage.read(cx).chat_title.clone();
                self.open_modal_dialog(label, ModalDialogKind::ExportData { chat_title: title }, 420.0, 360.0, cx);
            }
            SettingsRouterEvent::UpdateLocalStoragePrefs { max_cache_gb, retention_weeks } =>
                self.update_preferences(cx, |p| { p.max_cache_gb = u64::from(*max_cache_gb); p.retention_weeks = *retention_weeks; }),
            SettingsRouterEvent::ClearLocalCache => self.clear_local_cache_files(cx),
            SettingsRouterEvent::UpdateCallPrefs { input_device, output_device, camera_device } =>
                self.update_preferences(cx, |p| {
                    p.audio_input_device = input_device.clone();
                    p.audio_output_device = output_device.clone();
                    p.camera_device = camera_device.clone();
                }),
            SettingsRouterEvent::ToggleMicTest(active) => {
                self.overlays.mic_test_loop_gen = self.overlays.mic_test_loop_gen.wrapping_add(1);
                let expected_gen = self.overlays.mic_test_loop_gen;
                if *active {
                    let mut start_ok = true;
                    if self.rtc_talk.active_call_id.is_none()
                        && self.rtc_talk.talk_mode_overlay.is_none()
                        && !self.rtc_talk.voice_recorder.is_recording()
                    {
                        start_ok = self.rtc_talk.voice_recorder.start().is_ok();
                    }
                    if !start_ok && !self.rtc_talk.voice_recorder.is_recording() {
                        self.enqueue_localized_non_chat_toast(
                            "当前环境未检测到可用麦克风输入流",
                            "Microphone capture unavailable in current environment",
                            2200,
                            cx,
                        );
                    }
                    cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
                        loop {
                            app.background_executor()
                                .timer(std::time::Duration::from_millis(80))
                                .await;
                            let keep = weak_shell
                                .update(app, |shell, cx| {
                                    if shell.overlays.mic_test_loop_gen != expected_gen {
                                        return false;
                                    }
                                    let raw = shell.rtc_talk.voice_recorder.current_level();
                                    let norm = (f32::from(raw) / 8192.0).clamp(0.0, 1.0);
                                    let mut still_testing = false;
                                    shell.settings_router.update(cx, |sr, scx| {
                                        if let Some(ref sc) = sr.settings_calls {
                                            sc.update(scx, |sc, sc_cx| {
                                                still_testing = sc.mic_test_active;
                                                if still_testing {
                                                    sc.set_mic_level_normalized(norm);
                                                    sc_cx.notify();
                                                }
                                            });
                                        }
                                    });
                                    cx.notify();
                                    still_testing
                                })
                                .unwrap_or(false);
                            if !keep {
                                break;
                            }
                        }
                    })
                    .detach();
                } else {
                    self.stop_settings_mic_test_capture(cx);
                }
                cx.notify();
            }
            SettingsRouterEvent::UpdatePowerSavingPrefs {
                enabled, threshold_pct, animate_stickers_chat, animate_stickers_panel,
                animate_emoji_panel, autoplay_gifs, autoplay_videos,
            } => self.update_preferences(cx, |p| {
                p.power_saving_enabled = *enabled;
                p.power_saving_threshold_pct = *threshold_pct;
                p.animate_stickers_chat = *animate_stickers_chat;
                p.animate_stickers_panel = *animate_stickers_panel;
                p.animate_emoji_panel = *animate_emoji_panel;
                p.autoplay_gifs = *autoplay_gifs;
                p.autoplay_videos = *autoplay_videos;
            }),
            SettingsRouterEvent::TopupStars(amount) => {
                if !self.is_authenticated || self.session_sync.session_token.is_none() {
                    return;
                }
                let amount = i64::from(*amount);
                self.spawn_authed_shell_fut(
                    cx,
                    move |fence| async move {
                        fence
                            .client
                            .wallet_topup_with_token(&fence.token, fence.client_epoch, amount)
                            .await
                    },
                    |s, res, cx| {
                    match res {
                        Ok(_) => s.enqueue_localized_non_chat_toast("星币充值已到账", "Stars top-up completed", 2200, cx),
                        Err(e) => {
                            let pfx = tr_str(s.active_lang_id(), "星币充值失败", "Top-up failed");
                            s.show_non_chat_toast(format!("{pfx}: {e}"), 2600, cx);
                            cx.notify();
                        }
                    }
                    s.refresh_wallet(cx);
                });
            }
            SettingsRouterEvent::WithdrawStars => {
                self.enqueue_localized_non_chat_toast(
                    "已提交创作者收益提现申请",
                    "Creator TON withdrawal requested",
                    2200,
                    cx,
                );
            }
            SettingsRouterEvent::ActivatePremium(months) => {
                self.push_user_settings_to_backend(cx);
                let msg = if self.active_lang_id().starts_with("zh") {
                    format!("Telegram 高级会员（{months} 个月）已生效")
                } else {
                    format!("Telegram Premium ({months} mo) activated")
                };
                self.show_non_chat_toast(msg, 2200, cx);
                cx.notify();
            }
            SettingsRouterEvent::UpdatePasskeysPrefs(names) => {
                self.update_preferences(cx, |p| p.passkey_names = names.clone());
                self.enqueue_localized_non_chat_toast("通行密钥设置已保存", "Passkeys updated", 1800, cx);
            }
            SettingsRouterEvent::UpdateWebsitesPrefs(domains) => {
                self.update_preferences(cx, |p| p.connected_website_domains = domains.clone());
                self.enqueue_localized_non_chat_toast("已更新授权网站列表", "Connected websites updated", 1800, cx);
            }
            SettingsRouterEvent::UpdateShortcutsPrefs { enabled, filter } => {
                self.update_preferences(cx, |p| {
                    p.shortcuts_enabled = *enabled;
                    p.shortcuts_filter = filter.clone();
                });
            }
            SettingsRouterEvent::UpdateBusinessPrefs(tools) => {
                self.update_preferences(cx, |p| p.business_configured_tools = tools.clone());
            }
            SettingsRouterEvent::SelectLanguage(code) =>
                self.update_preferences(cx, |p| p.localization.language_code = code.to_string()),
            SettingsRouterEvent::ToggleLanguage => self.toggle_language_pref(cx),
            SettingsRouterEvent::Computers(ev) => self.handle_computer_settings_event(ev, cx),
            SettingsRouterEvent::AiProviders(ev) => self.handle_ai_providers_settings_event(ev, cx),
            SettingsRouterEvent::VaultCustody(act) => self.handle_settings_vault_action(act, cx),
            SettingsRouterEvent::ActiveInputFieldChanged => {
                if self.is_settings_unoccluded(cx) {
                    self.clear_all_ime_preedits(cx);
                }
            }
            SettingsRouterEvent::Navigated(sec) => {
                if self.is_settings_unoccluded(cx) {
                    self.clear_all_ime_preedits(cx);
                } else {
                    self.settings_router.update(cx, |sr, scx| sr.clear_all_preedits(scx));
                }
                if *sec != SettingsSectionType::Calls {
                    self.stop_settings_mic_test_capture(cx);
                }
                if *sec == SettingsSectionType::Computers {
                    self.refresh_computers(cx);
                    self.refresh_remote_assist_and_grants(cx);
                    self.sync_computers_to_views(cx);
                } else if *sec == SettingsSectionType::AiProviders {
                    self.refresh_magpie_state(cx);
                } else if *sec == SettingsSectionType::PrivacySecurity {
                    self.settings_router.update(cx, |r, rcx| {
                        let _ = r.ensure_privacy(rcx);
                    });
                    self.sync_vault_custody_to_settings(cx);
                    self.refresh_pending_device_sync_requests(cx);
                }
                cx.notify();
            }
            SettingsRouterEvent::CheckForUpdates => self.trigger_update_check(true, cx),
            SettingsRouterEvent::TriggerOneClickUpdate => self.trigger_one_click_update(cx),
        }
    }
}
