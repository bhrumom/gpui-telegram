use std::sync::Arc;

use gpui::{AsyncApp, ClipboardItem, Context, WeakEntity};
use gpui_telegram_client::net::http_client::AddContactHttpRequest;
use gpui_telegram_client::net::proxy::{ClientProxyConfig, ClientProxyType};
use gpui_telegram_client::net::RichSendMessageRequest;
use gpui_telegram_client::TelegramClient;
use gpui_telegram_proto::contracts::auth::UpdateProfileRequest as ProtoUpdateProfileRequest;
use gpui_telegram_proto::contracts::chats::CreateChatRequest;
use gpui_telegram_proto::contracts::features::StarGiftRequest;
use gpui_telegram_proto::contracts::messages::CreatePollHttpRequest;

use crate::components::ui::layers::standalone_layer_stack::LayerStackOption;
use crate::i18n::tr_str;
use crate::views::app_shell::shell_actions::{
    chat_dialogs_to_forward_targets, contacts_to_modal_rows, resolve_active_target_chat_id,
};
use crate::views::app_shell::shell_theme::toggle_desktop_theme_mode;
use crate::views::boxes::modal_types::{ModalActionOutcome, ModalDialogKind};
use crate::views::main_menu_drawer::MainMenuCommand;
use crate::views::nav_rail::NavDestination;
use crate::views::settings::sections::settings_advanced::ProxyType;
use crate::views::settings::settings_type::SettingsSectionType;
use crate::views::shell::{ActiveWorkspacePanel, AppShell};
use crate::views::global_tasks_workspace::TasksWorkspaceTab;
use crate::views::app_shell::window_connecting_widget::ConnectionStatus as BannerConnectionStatus;

#[derive(Clone)]
struct ModalAuthFence {
    auth_generation: u64,
    user_id: Option<u64>,
    client: Arc<TelegramClient>,
    client_epoch: u64,
    token: Option<String>,
}

fn modal_auth_fence(shell: &AppShell) -> Option<ModalAuthFence> {
    if !shell.is_authenticated {
        return None;
    }
    let session_token = shell
        .session_sync
        .session_token
        .clone()
        .filter(|t| !t.trim().is_empty());
    let token = shell
        .client
        .token()
        .filter(|t| !t.trim().is_empty())
        .or(session_token);
    Some(ModalAuthFence {
        auth_generation: shell.session_sync.auth_generation,
        user_id: shell.session_sync.user_id,
        client: shell.client.clone(),
        client_epoch: shell.client.session_epoch(),
        token,
    })
}

fn modal_auth_fence_matches(fence: &ModalAuthFence, shell: &AppShell) -> bool {
    shell.is_authenticated
        && shell.session_sync.auth_generation == fence.auth_generation
        && shell.session_sync.user_id == fence.user_id
        && Arc::ptr_eq(&shell.client, &fence.client)
        && shell.client.session_epoch() == fence.client_epoch
        && match fence.token.as_deref() {
            Some(tok) => fence
                .client
                .ensure_pinned_session(fence.client_epoch, tok)
                .is_ok(),
            None => fence
                .client
                .token()
                .is_none_or(|cur| cur.trim().is_empty()),
        }
}

async fn modal_preflight(
    fence: &ModalAuthFence,
    weak: &WeakEntity<AppShell>,
    app: &mut AsyncApp,
) -> bool {
    weak.update(app, |s, _| modal_auth_fence_matches(fence, s))
        .unwrap_or(false)
}

fn proxy_op_matches(fence: &ModalAuthFence, expected_proxy_gen: u64, shell: &AppShell) -> bool {
    modal_auth_fence_matches(fence, shell) && shell.proxy_op_generation == expected_proxy_gen
}

async fn proxy_op_preflight(
    fence: &ModalAuthFence,
    expected_proxy_gen: u64,
    weak: &WeakEntity<AppShell>,
    app: &mut AsyncApp,
) -> bool {
    weak.update(app, |s, _| proxy_op_matches(fence, expected_proxy_gen, s))
        .unwrap_or(false)
}

fn is_active_chat_visible_for_toast(shell: &AppShell, cid: u64, cx: &gpui::App) -> bool {
    shell.is_chat_visible_for_toast(cid, cx)
}

type SettingsAdvancedProxySnapshot = (ProxyType, String, u16);

fn proxy_type_to_kind_str(proxy_type: ProxyType) -> &'static str {
    match proxy_type {
        ProxyType::Direct => "Direct",
        ProxyType::Http => "HTTP",
        ProxyType::Socks5 => "SOCKS5",
        ProxyType::Mtproto => "MTProto",
    }
}

fn snapshot_settings_advanced_proxy(
    shell: &AppShell,
    cx: &gpui::App,
) -> Option<SettingsAdvancedProxySnapshot> {
    shell
        .settings_router
        .read(cx)
        .settings_advanced
        .as_ref()
        .map(|sa| {
            let r = sa.read(cx);
            (r.proxy_type, r.proxy_host.clone(), r.proxy_port)
        })
}

fn active_client_proxy_snapshot(client: &TelegramClient) -> SettingsAdvancedProxySnapshot {
    match client.http_client.proxy() {
        Some(cfg) if cfg.proxy_type != ClientProxyType::Direct => {
            let pt = match cfg.proxy_type {
                ClientProxyType::Direct => ProxyType::Direct,
                ClientProxyType::Http => ProxyType::Http,
                ClientProxyType::Socks5 => ProxyType::Socks5,
            };
            (pt, cfg.host, cfg.port)
        }
        _ => {
            let state = gpui_telegram_client::SessionPersistenceBridge::load_state_from_disk();
            if let Some(p) = state.proxies.into_iter().find(|p| {
                p.enabled
                    && p.kind.trim().eq_ignore_ascii_case("mtproto")
                    && ClientProxyConfig::validate_record(p).is_ok()
            }) {
                (ProxyType::Mtproto, p.host, p.port)
            } else {
                (ProxyType::Direct, String::new(), 1080)
            }
        }
    }
}

fn restore_settings_advanced_proxy(
    shell: &mut AppShell,
    snapshot: SettingsAdvancedProxySnapshot,
    cx: &mut Context<AppShell>,
) {
    let (pt, host, port) = snapshot;
    let enabled = pt != ProxyType::Direct && !host.trim().is_empty();
    let kind = proxy_type_to_kind_str(pt);
    shell.settings_router.update(cx, |sr, scx| {
        if let Some(ref sa) = sr.settings_advanced {
            sa.update(scx, |sa, sa_cx| {
                sa.sync_proxy(kind, &host, port, enabled);
                sa_cx.notify();
            });
        }
    });
}

impl AppShell {
    pub(crate) fn sync_settings_advanced_to_active_proxy(&mut self, cx: &mut Context<Self>) {
        let snapshot = active_client_proxy_snapshot(&self.client);
        restore_settings_advanced_proxy(self, snapshot, cx);
    }

    pub fn open_modal_dialog(&mut self, title: impl Into<String>, kind: ModalDialogKind, w: f32, h: f32, cx: &mut Context<Self>) {
        self.community_pending_share = None;
        self.clear_ime_and_blur_content_hub_for_transition(true, cx);
        // When Apple/Grok multi-select compose is available, never fall back to the legacy
        // 0-member `NewGroup` modal; route directly into multi-select compose group mode.
        if matches!(kind, ModalDialogKind::NewGroup) && self.theme_state.palette.is_apple_style() {
            self.leave_browser_panel_if_active(cx);
            self.leave_community_panel_if_active(cx);
            self.cleanup_settings_panel_if_active(cx);
            self.active_panel = ActiveWorkspacePanel::Chats;
            self.sync_nav_rail_destination_silent(NavDestination::Chats, cx);
            self.sync_apple_sidebar_to_active_panel(cx);
            self.bot_browser.ime_preedit_input_gen = None;
            self.community_ime_preedit_input_gen = None;
            self.enter_compose_or_modal_isolation(cx);
            self.sidebar.update(cx, |sb, sb_cx| {
                sb.open_compose_group_mode();
                sb_cx.notify();
            });
            self.blur_composer_and_cancel_ime(cx);
            self.sync_compose_state_to_composer(true, cx);
            self.reconcile_chat_scoped_toasts(cx);
            cx.notify();
            return;
        }
        let is_creation_or_contacts = matches!(
            kind,
            ModalDialogKind::NewChannel
                | ModalDialogKind::NewGroup
                | ModalDialogKind::AddContact
                | ModalDialogKind::Contacts
        );
        if is_creation_or_contacts {
            self.enter_compose_or_modal_isolation(cx);
        }
        self.sidebar.update(cx, |sb, sb_cx| {
            let changed = sb.is_plus_menu_open
                || !sb.compose_recipient_query.is_empty()
                || sb.compose_title_focused;
            sb.cancel_compose_mode();
            if changed {
                sb_cx.notify();
            }
        });
        self.stage.update(cx, |st, _| {
            st.more_menu_open = false;
            st.ttl_submenu_open = false;
        });
        self.occlude_browser_for_overlay(cx);
        self.occlude_community_for_overlay(cx);
        let is_connection_modal = matches!(kind, ModalDialogKind::ConnectionProxy);
        let targets = chat_dialogs_to_forward_targets(&self.sidebar.read(cx).dialogs);
        let rows = contacts_to_modal_rows(&self.session_sync.contacts_list);
        self.modal_container.update(cx, |m, mcx| {
            m.dialog.sync_contacts(rows);
            m.show_dialog(title, kind, targets, w, h, LayerStackOption::CloseOther);
            mcx.notify();
        });
        self.blur_composer_and_cancel_ime(cx);
        if self.composer.read(cx).is_compose_isolated
            && !self.is_compose_or_creation_modal_active(cx)
            && !is_creation_or_contacts
        {
            self.exit_compose_or_modal_isolation(true, cx);
        } else {
            self.sync_compose_state_to_composer(true, cx);
        }
        if is_connection_modal {
            if let Some(fence) = modal_auth_fence(self) {
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !modal_preflight(&fence, &weak, app).await {
                        return;
                    }
                    if let Ok(proxies) = fence
                        .client
                        .list_proxies_for_session(fence.token.as_deref(), fence.client_epoch)
                        .await
                    {
                        let _ = weak.update(app, |s, cx| {
                            if !modal_auth_fence_matches(&fence, s) {
                                return;
                            }
                            s.modal_container.update(cx, |m, mcx| {
                                if matches!(m.dialog.kind, Some(ModalDialogKind::ConnectionProxy)) {
                                    m.dialog.proxies = proxies;
                                    mcx.notify();
                                }
                            });
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
        }
        self.reconcile_chat_scoped_toasts(cx);
        cx.notify();
    }

    pub fn handle_main_menu_command(&mut self, cmd: MainMenuCommand, cx: &mut Context<Self>) {
        self.clear_ime_and_blur_content_hub_for_transition(true, cx);
        let lang = self.active_lang_id();
        self.main_menu_drawer.update(cx, |d, _| d.close());
        self.sidebar.update(cx, |sb, sb_cx| {
            let changed = sb.is_plus_menu_open
                || !sb.compose_recipient_query.is_empty()
                || sb.compose_title_focused;
            sb.cancel_compose_mode();
            if changed {
                sb_cx.notify();
            }
        });
        if self.composer.read(cx).is_compose_isolated && !self.is_compose_or_creation_modal_active(cx)
        {
            self.exit_compose_or_modal_isolation(true, cx);
        } else {
            self.sync_compose_state_to_composer(true, cx);
        }
        self.stage.update(cx, |st, _| {
            st.more_menu_open = false;
            st.ttl_submenu_open = false;
        });
        match cmd {
            MainMenuCommand::SwitchAccount(acc_id) => {
                if Some(acc_id as u64) != self.session_sync.user_id {
                    self.show_non_chat_toast(tr_str(lang, "正在切换账号…", "Switching account…"), 2000, cx);
                }
            }
            MainMenuCommand::AddAccount => self.logout(cx),
            MainMenuCommand::SetEmojiStatus(emoji) => {
                let Some(fence) = self.capture_client_fence() else {
                    return;
                };
                let req = ProtoUpdateProfileRequest {
                    first_name: None,
                    last_name: None,
                    username: None,
                    phone: None,
                    bio: None,
                    avatar_media_id: None,
                    emoji_status: Some(emoji),
                };
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let _ = fence
                        .client
                        .update_profile_with_token(&fence.token, fence.client_epoch, &req)
                        .await;
                })
                .detach();
            }
            MainMenuCommand::MyProfile => {
                self.navigate_settings_to(SettingsSectionType::Information, cx);
            }
            MainMenuCommand::NewFabushiBot => self.open_fabushi_creator_modal(cx),
            MainMenuCommand::NewGroup => {
                if self.theme_state.palette.is_apple_style() {
                    self.leave_browser_panel_if_active(cx);
                    self.leave_community_panel_if_active(cx);
                    self.cleanup_settings_panel_if_active(cx);
                    self.active_panel = ActiveWorkspacePanel::Chats;
                    self.sync_nav_rail_destination_silent(NavDestination::Chats, cx);
                    self.sync_apple_sidebar_to_active_panel(cx);
                    self.bot_browser.ime_preedit_input_gen = None;
                    self.community_ime_preedit_input_gen = None;
                    self.enter_compose_or_modal_isolation(cx);
                    self.sidebar.update(cx, |sb, sb_cx| {
                        sb.open_compose_group_mode();
                        sb_cx.notify();
                    });
                    self.blur_composer_and_cancel_ime(cx);
                    self.sync_compose_state_to_composer(true, cx);
                } else {
                    self.open_modal_dialog(
                        tr_str(lang, "新建群组", "New Group"),
                        ModalDialogKind::NewGroup,
                        420.0,
                        380.0,
                        cx,
                    );
                }
            }
            MainMenuCommand::NewChannel => self.open_modal_dialog(tr_str(lang, "新建频道", "New Channel"), ModalDialogKind::NewChannel, 420.0, 380.0, cx),
            MainMenuCommand::Contacts => {
                self.refresh_contacts(cx);
                self.open_modal_dialog(tr_str(lang, "联系人", "Contacts"), ModalDialogKind::Contacts, 420.0, 500.0, cx);
            }
            MainMenuCommand::Calls => {
                self.cleanup_settings_panel_if_active(cx);
                self.leave_browser_panel_if_active(cx);
                self.leave_code_scene_panel_if_active(cx);
                self.leave_community_panel_if_active(cx);
                self.active_panel = ActiveWorkspacePanel::Calls;
                self.sync_nav_rail_destination_silent(NavDestination::Calls, cx);
                self.sync_apple_sidebar_to_active_panel(cx);
                self.sync_calls_history(cx);
            }
            MainMenuCommand::SavedMessages => {
                self.cleanup_settings_panel_if_active(cx);
                self.leave_browser_panel_if_active(cx);
                self.leave_code_scene_panel_if_active(cx);
                self.leave_community_panel_if_active(cx);
                self.active_panel = ActiveWorkspacePanel::Chats;
                self.sync_nav_rail_destination_silent(NavDestination::SavedMessages, cx);
                self.sync_apple_sidebar_to_active_panel(cx);
                let target_id = self.sidebar.read(cx).saved_messages_id.or_else(|| {
                    let uid = self.session_sync.user_id?;
                    self.sidebar.read(cx).dialogs.iter().find(|d| d.id == uid).map(|d| d.id)
                });
                if let Some(id) = target_id {
                    self.sidebar.update(cx, |sb, _| sb.select_dialog(id));
                    self.sync_selected_chat_inspector_and_stage(id, cx);
                }
            }
            MainMenuCommand::TaskCenter => {
                self.cleanup_settings_panel_if_active(cx);
                self.leave_browser_panel_if_active(cx);
                self.leave_code_scene_panel_if_active(cx);
                self.leave_community_panel_if_active(cx);
                self.active_panel = ActiveWorkspacePanel::TaskCenter;
                self.sync_nav_rail_destination_silent(NavDestination::TaskCenter, cx);
                self.sync_apple_sidebar_to_active_panel(cx);
                self.tasks_workspace.update(cx, |w, _| w.set_tab(TasksWorkspaceTab::Tasks));
                self.refresh_tasks_and_mcp(cx);
            }
            MainMenuCommand::McpTools => {
                self.cleanup_settings_panel_if_active(cx);
                self.leave_browser_panel_if_active(cx);
                self.leave_code_scene_panel_if_active(cx);
                self.leave_community_panel_if_active(cx);
                self.active_panel = ActiveWorkspacePanel::McpTools;
                self.sync_nav_rail_destination_silent(NavDestination::McpTools, cx);
                self.sync_apple_sidebar_to_active_panel(cx);
                self.tasks_workspace.update(cx, |w, _| w.set_tab(TasksWorkspaceTab::McpTools));
                self.refresh_tasks_and_mcp(cx);
            }
            MainMenuCommand::RemoteComputer => {
                self.leave_code_scene_panel_if_active(cx);
                self.open_remote_computers_hub(cx);
            }
            MainMenuCommand::CodeScene => {
                self.open_code_scene_workspace(cx);
            }
            MainMenuCommand::Browser => {
                self.leave_code_scene_panel_if_active(cx);
                self.open_browser_workspace(cx);
            }
            MainMenuCommand::Community => {
                self.leave_code_scene_panel_if_active(cx);
                self.open_community_workspace(cx);
            }
            MainMenuCommand::Settings => {
                self.leave_code_scene_panel_if_active(cx);
                self.navigate_settings_to(SettingsSectionType::Main, cx);
            }
            MainMenuCommand::ToggleNightMode => {
                let next = toggle_desktop_theme_mode(self.theme_state.theme_mode);
                self.apply_theme_selection(next, None, None, cx);
            }
            MainMenuCommand::WalletAndStars => {
                self.navigate_settings_to(SettingsSectionType::Credits, cx);
            }
            MainMenuCommand::ExportData => {
                let title = self.stage.read(cx).chat_title.clone();
                self.open_modal_dialog(tr_str(lang, "导出聊天数据", "Export Chat Data"), ModalDialogKind::ExportData { chat_title: title }, 420.0, 340.0, cx);
            }
            MainMenuCommand::ShortcutsLegend => self.open_modal_dialog(tr_str(lang, "键盘快捷键", "Keyboard Shortcuts"), ModalDialogKind::ShortcutsLegend, 460.0, 440.0, cx),
            MainMenuCommand::AboutTelegram => self.open_modal_dialog(
                tr_str(lang, "关于 Fabushi", "About Fabushi"),
                ModalDialogKind::AboutTelegram,
                420.0,
                320.0,
                cx,
            ),
            MainMenuCommand::Logout => self.logout(cx),
        }
        if !self.can_focus_chat_composer(cx) {
            self.blur_composer_and_cancel_ime(cx);
        }
        self.reconcile_chat_scoped_toasts(cx);
        cx.notify();
    }

    pub fn handle_modal_outcome(&mut self, outcome: ModalActionOutcome, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let active_chat = resolve_active_target_chat_id(self.stage.read(cx).active_chat_id, self.sidebar.read(cx).active_chat_id);
        match outcome {
            ModalActionOutcome::CreatePollSubmitted { question, poll_type, options } => {
                if let (Some(chat_id), Some(fence)) = (active_chat, self.capture_client_fence()) {
                    let is_anon = matches!(poll_type, crate::views::stage::cards::PollType::Anonymous);
                    let is_quiz = matches!(poll_type, crate::views::stage::cards::PollType::Quiz);
                    let req = CreatePollHttpRequest {
                        question,
                        options: options.into_iter().map(|o| o.text).collect(),
                        is_anonymous: is_anon,
                        multiple_choice: false,
                        is_quiz,
                        correct_option_id: if is_quiz { Some(0) } else { None },
                        solution: None,
                        topic_id: None,
                        client_nonce: Some(client.next_client_nonce()),
                    };
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let _ = fence
                            .client
                            .create_poll_with_token(
                                &fence.token,
                                fence.client_epoch,
                                chat_id as i64,
                                &req,
                            )
                            .await;
                        let _ = weak.update(app, |s, cx| {
                            if !fence.is_valid(s) {
                                return;
                            }
                            if s.stage.read(cx).active_chat_id == Some(chat_id) {
                                s.load_historical_messages(chat_id, cx);
                            }
                        });
                    }).detach();
                }
            }
            ModalActionOutcome::ForwardToChat { target_chat_id, message_ids, extra_caption, .. } => {
                let pending_community_share = self.community_pending_share.take();
                let Some(fence) = self.capture_client_fence() else {
                    return;
                };
                if message_ids.is_empty() {
                    let summary_clean = pending_community_share
                        .as_ref()
                        .map(|(_, s)| s.trim())
                        .filter(|s| !s.is_empty());
                    let cap = extra_caption.trim();
                    let body = match (summary_clean, cap) {
                        (Some(summary), cap) if !cap.is_empty() => {
                            format!("{summary}\n\n{cap}")
                        }
                        (Some(summary), _) => summary.to_string(),
                        (None, cap) if !cap.is_empty() => cap.to_string(),
                        (None, _) => return,
                    };
                    let lang = self.active_lang_id();
                    self.show_non_chat_toast(
                        tr_str(lang, "已分享帖子到会话", "Shared post to chat"),
                        2200,
                        cx,
                    );
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let send_client = fence.client.clone();
                        let exp_epoch = fence.client_epoch;
                        let exp_tok = fence.token.clone();
                        let _ = fence
                            .client
                            .runtime()
                            .spawn(async move {
                                if send_client
                                    .ensure_pinned_session(exp_epoch, &exp_tok)
                                    .is_err()
                                {
                                    return;
                                }
                                let req = RichSendMessageRequest {
                                    chat_id: target_chat_id as i64,
                                    text: Some(body),
                                    content: None,
                                    entities: Vec::new(),
                                    reply_to: None,
                                    quote_text: None,
                                    topic_id: None,
                                    client_nonce: Some(send_client.next_client_nonce()),
                                    forward_from_chat_id: None,
                                    forward_from_msg_id: None,
                                    silent: false,
                                    scheduled_at_ms: None,
                                    inline_keyboard: None,
                                    send_as_peer_id: None,
                                    skip_server_bot_reply: false,
                                };
                                let _ = send_client
                                    .send_message_durable_with_token(&exp_tok, exp_epoch, req)
                                    .await;
                            })
                            .await;
                        let _ = weak.update(app, |s, cx| {
                            if fence.is_valid(s) {
                                s.refresh_chats(cx);
                            }
                        });
                    })
                    .detach();
                    return;
                }
                let from_chat = active_chat.unwrap_or(target_chat_id) as i64;
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    for mid in message_ids {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let send_client = fence.client.clone();
                        let exp_epoch = fence.client_epoch;
                        let exp_tok = fence.token.clone();
                        let caption = extra_caption.clone();
                        let _ = fence
                            .client
                            .runtime()
                            .spawn(async move {
                                if send_client
                                    .ensure_pinned_session(exp_epoch, &exp_tok)
                                    .is_err()
                                {
                                    return;
                                }
                                let text_opt = if caption.trim().is_empty() {
                                    None
                                } else {
                                    Some(caption)
                                };
                                let req = RichSendMessageRequest {
                                    chat_id: target_chat_id as i64,
                                    text: text_opt,
                                    content: None,
                                    entities: Vec::new(),
                                    reply_to: None,
                                    quote_text: None,
                                    topic_id: None,
                                    client_nonce: Some(send_client.next_client_nonce()),
                                    forward_from_chat_id: Some(from_chat),
                                    forward_from_msg_id: Some(mid as i64),
                                    silent: false,
                                    scheduled_at_ms: None,
                                    inline_keyboard: None,
                                    send_as_peer_id: None,
                                    skip_server_bot_reply: false,
                                };
                                let _ = send_client
                                    .send_message_durable_with_token(&exp_tok, exp_epoch, req)
                                    .await;
                            })
                            .await;
                    }
                    let _ = weak.update(app, |s, cx| {
                        if fence.is_valid(s) {
                            s.refresh_chats(cx);
                        }
                    });
                }).detach();
            }
            ModalActionOutcome::CreateGroupSubmitted { title, enable_forum_topics, .. } => {
                if self.theme_state.palette.is_apple_style() && !enable_forum_topics {
                    // Guard against creating an empty 0-member group when multi-select compose is available.
                    let clean_title = title.trim().chars().take(72).collect::<String>();
                    self.clear_ime_and_blur_content_hub_for_transition(true, cx);
                    self.cleanup_settings_panel_if_active(cx);
                    self.leave_browser_panel_if_active(cx);
                    self.leave_community_panel_if_active(cx);
                    self.active_panel = ActiveWorkspacePanel::Chats;
                    self.sync_nav_rail_destination_silent(NavDestination::Chats, cx);
                    self.sync_apple_sidebar_to_active_panel(cx);
                    self.enter_compose_or_modal_isolation(cx);
                    self.sidebar.update(cx, |sb, sb_cx| {
                        sb.open_compose_group_mode();
                        sb.compose_draft.custom_group_title = clean_title;
                        sb_cx.notify();
                    });
                    self.blur_composer_and_cancel_ime(cx);
                    self.sync_compose_state_to_composer(true, cx);
                    let toast_msg = tr_str(
                        self.active_lang_id(),
                        "请选择至少一位联系人或 AI Bot 以创建群聊",
                        "Select at least one contact or AI Bot to create the group",
                    );
                    self.show_non_chat_toast(toast_msg, 2600, cx);
                } else if let Some(fence) = self.capture_client_fence() {
                    let req = CreateChatRequest {
                        title,
                        chat_type: Some(serde_json::json!(1)),
                        is_forum: enable_forum_topics,
                        ..Default::default()
                    };
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        if let Ok(created) = fence
                            .client
                            .create_chat_with_token(&fence.token, fence.client_epoch, &req)
                            .await
                        {
                            let _ = weak.update(app, |s, cx| {
                                if !fence.is_valid(s) {
                                    return;
                                }
                                s.refresh_chats(cx);
                                s.sync_selected_chat_inspector_and_stage(created.chat_id as u64, cx);
                            });
                        }
                    })
                    .detach();
                }
            }
            ModalActionOutcome::CreateChannelSubmitted { title, username_slug, .. } => {
                if let Some(fence) = self.capture_client_fence() {
                    let req = CreateChatRequest {
                        title, chat_type: Some(serde_json::json!(2)),
                        username: Some(username_slug).filter(|s| !s.is_empty()), ..Default::default()
                    };
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        if let Ok(created) = fence
                            .client
                            .create_chat_with_token(&fence.token, fence.client_epoch, &req)
                            .await
                        {
                            let _ = weak.update(app, |s, cx| {
                                if !fence.is_valid(s) {
                                    return;
                                }
                                s.refresh_chats(cx);
                                s.sync_selected_chat_inspector_and_stage(created.chat_id as u64, cx);
                            });
                        }
                    }).detach();
                }
            }
            ModalActionOutcome::AddContactSubmitted { first_name, last_name, phone } => {
                if let Some(fence) = self.capture_client_fence() {
                    let req = AddContactHttpRequest {
                        first_name, last_name: Some(last_name).filter(|s| !s.is_empty()),
                        username: None, phone: Some(phone).filter(|s| !s.is_empty()), is_bot: false, bot_system_prompt: None,
                    };
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let _ = fence
                            .client
                            .add_contact_with_token(&fence.token, fence.client_epoch, req)
                            .await;
                        let _ = weak.update(app, |s, cx| {
                            if !fence.is_valid(s) {
                                return;
                            }
                            s.refresh_contacts(cx);
                            s.refresh_chats(cx);
                        });
                    }).detach();
                }
            }
            ModalActionOutcome::OpenContactChat { name, .. } => {
                if let Some(id) = self.sidebar.read(cx).dialogs.iter().find(|d| d.peer_name == name).map(|d| d.id) {
                    self.sidebar.update(cx, |sb, sb_cx| {
                        let was_compose = sb.is_plus_menu_open
                            || !sb.compose_recipient_query.is_empty()
                            || sb.compose_title_focused;
                        sb.select_dialog_from_user(id);
                        if was_compose {
                            sb_cx.notify();
                        }
                    });
                    self.sync_selected_chat_inspector_and_stage(id, cx);
                }
            }
            ModalActionOutcome::StartCallWithPeer { peer_name, is_video } => {
                let pid = self.current_peer_user_id(cx).unwrap_or(0);
                if pid > 0 { self.start_webrtc_call(pid, peer_name, is_video, cx); }
            }
            ModalActionOutcome::CopyToClipboard(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            ModalActionOutcome::SelectBackground { preset_id } => self.apply_theme_selection(self.theme_state.theme_mode, None, Some(preset_id), cx),
            ModalActionOutcome::SelectLanguage { language_code } => {
                self.update_preferences(cx, |p| p.localization.language_code = language_code);
            }
            ModalActionOutcome::SavePeerEdit { title, bio, .. } => {
                if let (Some(cid), Some(fence)) = (active_chat, self.capture_client_fence()) {
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let _ = fence
                            .client
                            .update_chat_info_with_token(
                                &fence.token,
                                fence.client_epoch,
                                cid as i64,
                                Some(&title),
                                Some(&bio),
                            )
                            .await;
                        let _ = weak.update(app, |s, cx| {
                            if fence.is_valid(s) {
                                s.refresh_chats(cx);
                            }
                        });
                    }).detach();
                }
            }
            ModalActionOutcome::ConfirmDeleteMessages { message_ids, revoke_for_everyone } => {
                if let (Some(cid), Some(fence)) = (active_chat, self.capture_client_fence()) {
                    self.stage.update(cx, |st, _| st.delete_messages_by_ids(&message_ids));
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        for mid in message_ids {
                            if !fence.preflight_ui(&weak, app).await {
                                break;
                            }
                            let _ = fence
                                .client
                                .delete_message_with_token(
                                    &fence.token,
                                    fence.client_epoch,
                                    cid as i64,
                                    mid as i64,
                                    revoke_for_everyone,
                                )
                                .await;
                        }
                    }).detach();
                }
            }
            ModalActionOutcome::ConfirmPinMessage { message_id, .. } => {
                if let (Some(cid), Some(fence)) = (active_chat, self.capture_client_fence()) {
                    self.stage.update(cx, |st, _| st.toggle_pin_message(message_id));
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let _ = fence
                            .client
                            .pin_message_with_token(
                                &fence.token,
                                fence.client_epoch,
                                cid as i64,
                                message_id as i64,
                            )
                            .await;
                    }).detach();
                }
            }
            ModalActionOutcome::RunChatExport { format_json } => {
                if let (Some(cid), Some(fence)) = (active_chat, self.capture_client_fence()) {
                    let fmt = if format_json { Some("json") } else { Some("html") }.map(str::to_string);
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        if let Ok(path) = fence
                            .client
                            .export_chat_history_with_token(
                                &fence.token,
                                fence.client_epoch,
                                cid as i64,
                                fmt.as_deref(),
                            )
                            .await
                        {
                            let _ = weak.update(app, |s, cx| {
                                if !fence.is_valid(s) {
                                    return;
                                }
                                if is_active_chat_visible_for_toast(s, cid, cx) {
                                    let pfx = tr_str(s.active_lang_id(), "已导出聊天记录", "Exported");
                                    s.show_chat_toast(cid, format!("{pfx}: {path}"), 3000, cx);
                                }
                                cx.notify();
                            });
                        }
                    }).detach();
                }
            }
            ModalActionOutcome::SendStarGift { title, stars, .. } => {
                if let (Some(pid), Some(fence)) = (self.current_peer_user_id(cx), self.capture_client_fence()) {
                    let req = StarGiftRequest { to_user_id: pid, chat_id: active_chat.map(|v| v as i64), amount_stars: stars as i64, gift_id: None, message: Some(title) };
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let _ = fence
                            .client
                            .wallet_gift_stars_with_token(&fence.token, fence.client_epoch, &req)
                            .await;
                        let _ = weak.update(app, |s, cx| {
                            if fence.is_valid(s) {
                                s.refresh_wallet(cx);
                            }
                        });
                    }).detach();
                }
            }
            ModalActionOutcome::ConfirmClearChatHistory { .. } => {
                if let Some(cid) = active_chat {
                    let fence = self.capture_client_fence();
                    let lang = self.active_lang_id();
                    self.sidebar.update(cx, |sb, sb_cx| {
                        sb.clear_history(cid);
                        sb_cx.notify();
                    });
                    self.stage.update(cx, |st, st_cx| {
                        st.messages.clear();
                        st.chat_histories.insert(cid, Vec::new());
                        st.pinned_message = None;
                        st.row_height_cache.rebuild_prefix_sums(&st.messages);
                        st_cx.notify();
                    });
                    if is_active_chat_visible_for_toast(self, cid, cx) {
                        self.show_chat_toast(
                            cid,
                            tr_str(lang, "聊天记录已清空", "Chat history cleared"),
                            2200,
                            cx,
                        );
                    }
                    if let Some(fence) = fence {
                        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                            if !fence.preflight_ui(&weak, app).await {
                                return;
                            }
                            let _ = fence
                                .client
                                .clear_chat_history_with_token(
                                    &fence.token,
                                    fence.client_epoch,
                                    cid as i64,
                                )
                                .await;
                            let _ = weak.update(app, |s, cx| {
                                if fence.is_valid(s) {
                                    s.refresh_chats(cx);
                                }
                            });
                        })
                        .detach();
                    }
                }
            }
            ModalActionOutcome::ConfirmLeaveOrDeleteChat => {
                if let Some(cid) = active_chat {
                    let fence = self.capture_client_fence();
                    let lang = self.active_lang_id();
                    self.cleanup_deleted_or_left_chat_state(cid, cx);
                    self.show_non_chat_toast(
                        tr_str(lang, "对话已移除", "Chat removed"),
                        2200,
                        cx,
                    );
                    if let Some(fence) = fence {
                        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                            if !fence.preflight_ui(&weak, app).await {
                                return;
                            }
                            let _ = fence
                                .client
                                .leave_or_delete_chat_with_token(
                                    &fence.token,
                                    fence.client_epoch,
                                    cid as i64,
                                )
                                .await;
                            let _ = weak.update(app, |s, cx| {
                                if fence.is_valid(s) {
                                    s.refresh_chats(cx);
                                }
                            });
                        })
                        .detach();
                    }
                }
            }
            ModalActionOutcome::SetLocalPasscode(pin_opt) => {
                let enabled = pin_opt.as_ref().is_some_and(|s| !s.trim().is_empty());
                if !enabled {
                    self.overlays.is_passcode_locked = false;
                    self.overlays.passcode_unlock_input.clear();
                    self.overlays.passcode_unlock_error = None;
                }
                let mut next_hash: Option<String> = None;
                let mut next_auto_lock: u64 = 3600;
                self.settings_router.update(cx, |sr, scx| {
                    let lp = sr.ensure_local_passcode(scx);
                    lp.update(scx, |lp, lp_cx| {
                        if let Some(pin) = pin_opt {
                            let _ = lp.set_passcode(&pin);
                        } else {
                            lp.has_passcode = false;
                            lp.passcode_hash = None;
                            lp.is_locked = false;
                        }
                        lp.pending_prefs_sync = false;
                        next_hash = lp.passcode_hash.clone();
                        next_auto_lock = lp.auto_lock.as_seconds();
                        lp_cx.notify();
                    });
                });
                self.update_preferences(cx, |p| {
                    p.local_passcode_hash = next_hash;
                    p.auto_lock_seconds = next_auto_lock;
                });
                self.settings_router.update(cx, |sr, scx| {
                    let sp = sr.ensure_privacy(scx);
                    sp.update(scx, |sp, sp_cx| {
                        sp.set_passcode_enabled(enabled);
                        sp_cx.notify();
                    });
                });
                self.push_user_settings_to_backend(cx);
                let lang = self.active_lang_id();
                self.show_non_chat_toast(
                    if enabled {
                        tr_str(lang, "本地锁定密码已启用", "Local passcode enabled")
                    } else {
                        tr_str(lang, "本地锁定密码已关闭", "Local passcode disabled")
                    },
                    2200,
                    cx,
                );
            }
            ModalActionOutcome::AddProxy(proxy_rec) => {
                let Some(fence) = modal_auth_fence(self) else {
                    return;
                };
                if let Err(err) = ClientProxyConfig::validate_record(&proxy_rec) {
                    self.show_non_chat_toast(format!("Proxy error: {err}"), 2600, cx);
                    cx.notify();
                    return;
                }
                let prev_ui_proxy = snapshot_settings_advanced_proxy(self, cx)
                    .unwrap_or_else(|| active_client_proxy_snapshot(&self.client));
                self.proxy_op_generation = self.proxy_op_generation.wrapping_add(1);
                let expected_proxy_gen = self.proxy_op_generation;
                let (kind, host, port, enabled) = (
                    proxy_rec.kind.clone(),
                    proxy_rec.host.clone(),
                    proxy_rec.port,
                    proxy_rec.enabled,
                );
                if enabled {
                    self.settings_router.update(cx, |sr, scx| {
                        let sa = sr.ensure_advanced(scx);
                        sa.update(scx, |sa, sa_cx| {
                            sa.sync_proxy(&kind, &host, port, true);
                            sa_cx.notify();
                        });
                    });
                } else {
                    self.sync_settings_advanced_to_active_proxy(cx);
                }
                let proxy_cfg = ClientProxyConfig::from_record(&proxy_rec);
                let c = fence.client.clone();
                let lang_str = self.active_lang_id().to_string();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !proxy_op_preflight(&fence, expected_proxy_gen, &weak, app).await {
                        return;
                    }
                    let mut saved_rec = proxy_rec;
                    if let Some(ref cfg) = proxy_cfg {
                        if let Ok(latency_ms) = c.http_client.probe_proxy(cfg).await {
                            saved_rec.ping_ms = latency_ms;
                        }
                    }
                    if !proxy_op_preflight(&fence, expected_proxy_gen, &weak, app).await {
                        return;
                    }
                    let upsert_res = c
                        .upsert_proxy_for_session(
                            fence.token.as_deref(),
                            fence.client_epoch,
                            saved_rec,
                        )
                        .await;
                    let _ = weak.update(app, |s, cx| {
                        if !proxy_op_matches(&fence, expected_proxy_gen, s) {
                            return;
                        }
                        match upsert_res {
                            Ok(()) => {
                                if enabled {
                                    s.settings_router.update(cx, |sr, scx| {
                                        let sa = sr.ensure_advanced(scx);
                                        sa.update(scx, |sa, sa_cx| {
                                            sa.sync_proxy(&kind, &host, port, true);
                                            sa_cx.notify();
                                        });
                                    });
                                    if let Some(ref cfg) = proxy_cfg {
                                        s.window_runtime.set_connection_status(
                                            BannerConnectionStatus::Connecting,
                                            Some(cfg.display_endpoint()),
                                        );
                                        if s.is_authenticated {
                                            s.client.disconnect_ws();
                                            s.client.connect_ws();
                                        }
                                    }
                                } else {
                                    s.sync_settings_advanced_to_active_proxy(cx);
                                }
                                s.show_non_chat_toast(
                                    tr_str(&lang_str, "代理节点已保存", "Proxy saved"),
                                    2200,
                                    cx,
                                );
                            }
                            Err(err) => {
                                if enabled {
                                    restore_settings_advanced_proxy(s, prev_ui_proxy, cx);
                                }
                                s.show_non_chat_toast(format!("Proxy error: {err}"), 2600, cx);
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            ModalActionOutcome::SelectProxy { proxy_id } => {
                let Some(fence) = modal_auth_fence(self) else {
                    return;
                };
                let selected_rec = if proxy_id == 0 {
                    None
                } else {
                    self.modal_container
                        .read(cx)
                        .dialog
                        .proxies
                        .iter()
                        .find(|p| p.id == proxy_id)
                        .cloned()
                };
                let (prev_ui_proxy, expected_proxy_gen) = if proxy_id == 0 {
                    let prev = snapshot_settings_advanced_proxy(self, cx)
                        .unwrap_or_else(|| active_client_proxy_snapshot(&self.client));
                    self.proxy_op_generation = self.proxy_op_generation.wrapping_add(1);
                    let proxy_gen = self.proxy_op_generation;
                    self.settings_router.update(cx, |sr, scx| {
                        let sa = sr.ensure_advanced(scx);
                        sa.update(scx, |sa, sa_cx| {
                            sa.sync_proxy("Direct", "", 1080, false);
                            sa_cx.notify();
                        });
                    });
                    (Some(prev), proxy_gen)
                } else if let Some(mut rec) = selected_rec.clone() {
                    rec.enabled = true;
                    if let Err(err) = ClientProxyConfig::validate_record(&rec) {
                        self.show_non_chat_toast(format!("Proxy error: {err}"), 2600, cx);
                        cx.notify();
                        return;
                    }
                    let prev = snapshot_settings_advanced_proxy(self, cx)
                        .unwrap_or_else(|| active_client_proxy_snapshot(&self.client));
                    self.proxy_op_generation = self.proxy_op_generation.wrapping_add(1);
                    let proxy_gen = self.proxy_op_generation;
                    self.settings_router.update(cx, |sr, scx| {
                        let sa = sr.ensure_advanced(scx);
                        sa.update(scx, |sa, sa_cx| {
                            sa.sync_proxy(&rec.kind, &rec.host, rec.port, true);
                            sa_cx.notify();
                        });
                    });
                    (Some(prev), proxy_gen)
                } else {
                    let prev = active_client_proxy_snapshot(&self.client);
                    self.proxy_op_generation = self.proxy_op_generation.wrapping_add(1);
                    restore_settings_advanced_proxy(self, prev.clone(), cx);
                    (Some(prev), self.proxy_op_generation)
                };
                let c = fence.client.clone();
                let lang_str = self.active_lang_id().to_string();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    enum SelectProxyOutcome {
                        Ok,
                        Err(String),
                        NotFound,
                    }

                    if !proxy_op_preflight(&fence, expected_proxy_gen, &weak, app).await {
                        return;
                    }
                    let mut activated_cfg: Option<ClientProxyConfig> = None;
                    let mut uncached_applied_rec: Option<gpui_telegram_client::ProxyEndpointRecord> = None;
                    let mut outcome = SelectProxyOutcome::Ok;

                    if proxy_id == 0 {
                        match c
                            .list_proxies_for_session(fence.token.as_deref(), fence.client_epoch)
                            .await
                        {
                            Ok(existing) => {
                                for mut item in existing {
                                    if item.enabled {
                                        if !proxy_op_preflight(
                                            &fence,
                                            expected_proxy_gen,
                                            &weak,
                                            app,
                                        )
                                        .await
                                        {
                                            return;
                                        }
                                        item.enabled = false;
                                        if let Err(err) = c
                                            .upsert_proxy_for_session(
                                                fence.token.as_deref(),
                                                fence.client_epoch,
                                                item,
                                            )
                                            .await
                                        {
                                            outcome = SelectProxyOutcome::Err(err.to_string());
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(err) => {
                                outcome = SelectProxyOutcome::Err(err.to_string());
                            }
                        }
                        if matches!(outcome, SelectProxyOutcome::Ok) {
                            if !proxy_op_preflight(&fence, expected_proxy_gen, &weak, app).await {
                                return;
                            }
                            if let Err(err) = c.apply_proxy(None) {
                                outcome = SelectProxyOutcome::Err(err.to_string());
                            }
                        }
                    } else if let Some(mut rec) = selected_rec {
                        rec.enabled = true;
                        let cfg_opt = ClientProxyConfig::from_record(&rec);
                        if let Some(ref cfg) = cfg_opt {
                            if let Ok(latency_ms) = c.http_client.probe_proxy(cfg).await {
                                rec.ping_ms = latency_ms;
                            }
                        }
                        if !proxy_op_preflight(&fence, expected_proxy_gen, &weak, app).await {
                            return;
                        }
                        match c
                            .upsert_proxy_for_session(
                                fence.token.as_deref(),
                                fence.client_epoch,
                                rec,
                            )
                            .await
                        {
                            Ok(()) => {
                                activated_cfg = cfg_opt;
                            }
                            Err(err) => {
                                outcome = SelectProxyOutcome::Err(err.to_string());
                            }
                        }
                    } else {
                        match c
                            .list_proxies_for_session(fence.token.as_deref(), fence.client_epoch)
                            .await
                        {
                            Ok(existing) => {
                                if !proxy_op_preflight(&fence, expected_proxy_gen, &weak, app)
                                    .await
                                {
                                    return;
                                }
                                if let Some(mut rec) =
                                    existing.into_iter().find(|p| p.id == proxy_id)
                                {
                                    rec.enabled = true;
                                    if let Err(err) = ClientProxyConfig::validate_record(&rec) {
                                        outcome = SelectProxyOutcome::Err(err.to_string());
                                    } else {
                                        let applied_cfg = ClientProxyConfig::from_record(&rec);
                                        if let Some(ref cfg) = applied_cfg {
                                            if let Ok(latency_ms) =
                                                c.http_client.probe_proxy(cfg).await
                                            {
                                                rec.ping_ms = latency_ms;
                                            }
                                        }
                                        if !proxy_op_preflight(
                                            &fence,
                                            expected_proxy_gen,
                                            &weak,
                                            app,
                                        )
                                        .await
                                        {
                                            return;
                                        }
                                        match c
                                            .upsert_proxy_for_session(
                                                fence.token.as_deref(),
                                                fence.client_epoch,
                                                rec.clone(),
                                            )
                                            .await
                                        {
                                            Ok(()) => {
                                                activated_cfg = applied_cfg;
                                                uncached_applied_rec = Some(rec);
                                            }
                                            Err(err) => {
                                                outcome = SelectProxyOutcome::Err(err.to_string());
                                            }
                                        }
                                    }
                                } else {
                                    outcome = SelectProxyOutcome::NotFound;
                                }
                            }
                            Err(err) => {
                                outcome = SelectProxyOutcome::Err(err.to_string());
                            }
                        }
                    }
                    let _ = weak.update(app, |s, cx| {
                        if !proxy_op_matches(&fence, expected_proxy_gen, s) {
                            return;
                        }
                        match outcome {
                            SelectProxyOutcome::Err(err) => {
                                if let Some(prev) = prev_ui_proxy {
                                    restore_settings_advanced_proxy(s, prev, cx);
                                }
                                s.show_non_chat_toast(format!("Proxy error: {err}"), 2600, cx);
                            }
                            SelectProxyOutcome::NotFound => {
                                if let Some(prev) = prev_ui_proxy {
                                    restore_settings_advanced_proxy(s, prev, cx);
                                }
                                s.show_non_chat_toast(
                                    tr_str(&lang_str, "未找到所选代理节点", "Selected proxy not found"),
                                    2600,
                                    cx,
                                );
                            }
                            SelectProxyOutcome::Ok => {
                                if proxy_id == 0 {
                                    s.settings_router.update(cx, |sr, scx| {
                                        let sa = sr.ensure_advanced(scx);
                                        sa.update(scx, |sa, sa_cx| {
                                            sa.sync_proxy("Direct", "", 1080, false);
                                            sa_cx.notify();
                                        });
                                    });
                                    s.window_runtime.set_connection_status(
                                        BannerConnectionStatus::Connecting,
                                        None,
                                    );
                                    if s.is_authenticated {
                                        s.client.disconnect_ws();
                                        s.client.connect_ws();
                                    }
                                } else {
                                    if let Some(ref cfg) = activated_cfg {
                                        s.window_runtime.set_connection_status(
                                            BannerConnectionStatus::Connecting,
                                            Some(cfg.display_endpoint()),
                                        );
                                        if s.is_authenticated {
                                            s.client.disconnect_ws();
                                            s.client.connect_ws();
                                        }
                                    }
                                    if let Some(ref rec) = uncached_applied_rec {
                                        s.settings_router.update(cx, |sr, scx| {
                                            let sa = sr.ensure_advanced(scx);
                                            sa.update(scx, |sa, sa_cx| {
                                                sa.sync_proxy(
                                                    &rec.kind,
                                                    &rec.host,
                                                    rec.port,
                                                    true,
                                                );
                                                sa_cx.notify();
                                            });
                                        });
                                    }
                                }
                                let msg = if proxy_id == 0 {
                                    tr_str(
                                        &lang_str,
                                        "已切换为直连模式",
                                        "Switched to Direct connection",
                                    )
                                } else {
                                    tr_str(&lang_str, "已启用所选代理服务器", "Proxy server enabled")
                                };
                                s.show_non_chat_toast(msg, 2200, cx);
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            ModalActionOutcome::SavePrivacyRule { category, rule } => {
                self.apply_privacy_rule_update(&category, rule, cx);
            }
            ModalActionOutcome::CheckForUpdates => {
                self.modal_container.update(cx, |m, mcx| {
                    m.hide_all();
                    mcx.notify();
                });
                self.trigger_update_check(true, cx);
            }
            _ => {}
        }
        cx.notify();
    }
}
