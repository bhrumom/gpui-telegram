//! Secondary panel synchronization: folders, contacts, wallet credits, inspector, and sidebar actions.

use gpui::{AppContext, AsyncApp, ClipboardItem, Context, WeakEntity};
use gpui_telegram_proto::miniapp_contracts::MiniAppLaunchMode;

use crate::i18n::tr_str;
use crate::theme::TdesktopThemePalette;
use crate::views::app_shell::shell_actions::{
    contacts_to_modal_rows, map_chat_members_to_inspector, peer_avatar_color,
    resolve_active_target_chat_id,
};
use crate::views::app_shell::shell_settings_sync::{apply_folders_to_shell, apply_wallet_to_shell};
use crate::views::app_shell::shell_truth::{derive_initials, ClientCallFence};
use crate::views::boxes::modal_types::ModalDialogKind;
use crate::views::context_inspector::build_add_member_candidates;
use crate::views::drawer::inspector_tabs::InspectorAction;
use crate::views::shell::AppShell;
use crate::views::sidebar::dialogs_row_view::SidebarContextAction;
use crate::views::MediaViewer;

impl AppShell {
    /// Fetches chat folders from backend and updates navigation rail and folder settings.
    pub fn refresh_folders(&mut self, cx: &mut Context<Self>) {
        let Some(fence) = self.capture_client_fence() else {
            return;
        };
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            if let Ok(folders) = fence
                .client
                .list_folders_with_token(&fence.token, fence.client_epoch)
                .await
            {
                let _ = weak.update(app, |shell, cx| {
                    if !fence.is_valid(shell) {
                        return;
                    }
                    shell.client.local_store.set_folders(folders.clone());
                    apply_folders_to_shell(shell, &folders, cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Fetches user contacts from backend and syncs both shell cache and contacts modal.
    pub fn refresh_contacts(&mut self, cx: &mut Context<Self>) {
        let Some(fence) = self.capture_client_fence() else {
            return;
        };
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            if let Ok(list) = fence
                .client
                .list_contacts_with_token(&fence.token, fence.client_epoch)
                .await
            {
                let _ = weak.update(app, |shell, cx| {
                    if !fence.is_valid(shell) {
                        return;
                    }
                    let rows = contacts_to_modal_rows(&list);
                    let contact_cids: std::collections::HashSet<u64> = list
                        .iter()
                        .filter(|c| !c.is_bot)
                        .filter_map(|c| c.chat_id.map(|cid| cid as u64))
                        .collect();
                    shell.session_sync.contacts_list = list;
                    shell.sidebar.update(cx, |sb, sb_cx| {
                        sb.contact_chat_ids = contact_cids;
                        sb.recompute_folder_badges();
                        sb_cx.notify();
                    });
                    shell
                        .modal_container
                        .update(cx, |m, _| m.dialog.sync_contacts(rows));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Fetches Stars and TON wallet balance/transactions and lazily updates credits settings.
    pub fn refresh_wallet(&mut self, cx: &mut Context<Self>) {
        let Some(fence) = self.capture_client_fence() else {
            return;
        };
        let uname = self.session_sync.username.clone().unwrap_or_default();
        self.settings_router.update(cx, |sr, scx| {
            if let Some(ref sm) = sr.settings_main {
                sm.update(scx, |sm, _| {
                    if sm.profile.name.is_empty() && sm.profile.username.is_empty() {
                        sm.sync_profile(uname.clone(), String::new(), uname, String::new(), false, false);
                    }
                });
            }
        });
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let bal = fence
                .client
                .wallet_balance_with_token(&fence.token, fence.client_epoch)
                .await
                .ok();
            let txs = if fence.preflight_ui(&weak, app).await {
                fence
                    .client
                    .wallet_transactions_with_token(&fence.token, fence.client_epoch)
                    .await
                    .unwrap_or_default()
            } else {
                return;
            };
            let _ = weak.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                if let Some(b) = bal {
                    apply_wallet_to_shell(shell, &b, &txs, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Routes actions emitted by the right-side `ContextInspector` drawer.
    pub fn handle_inspector_action(&mut self, action: InspectorAction, cx: &mut Context<Self>) {
        let lang = self.active_lang_id();
        let active_chat = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        );
        match action {
            InspectorAction::StartVoiceCall(name) => {
                if self.is_active_chat_bot(cx) {
                    self.start_ai_talk_mode_session(false, cx);
                } else if let Some(pid) = self.current_peer_user_id(cx) {
                    self.start_webrtc_call(pid, name, false, cx);
                } else {
                    let msg = tr_str(lang, "请选择私聊用户或机器人对话", "Select a direct peer or bot chat");
                    if let Some(cid) = active_chat {
                        self.show_chat_toast(cid, msg, 2600, cx);
                    } else {
                        self.show_non_chat_toast(msg, 2600, cx);
                    }
                }
            }
            InspectorAction::StartVideoCall(name) => {
                if self.is_active_chat_bot(cx) {
                    self.start_ai_talk_mode_session(true, cx);
                } else if let Some(pid) = self.current_peer_user_id(cx) {
                    self.start_webrtc_call(pid, name, true, cx);
                } else {
                    let msg = tr_str(lang, "请选择私聊用户或机器人对话", "Select a direct peer or bot chat");
                    if let Some(cid) = active_chat {
                        self.show_chat_toast(cid, msg, 2600, cx);
                    } else {
                        self.show_non_chat_toast(msg, 2600, cx);
                    }
                }
            }
            InspectorAction::CopyText(txt) => {
                cx.write_to_clipboard(ClipboardItem::new_string(txt))
            }
            InspectorAction::ToggleNotifications(enabled) => {
                if let (Some(cid), Some(fence)) = (active_chat, self.capture_client_fence()) {
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let _ = fence
                            .client
                            .set_chat_muted_with_token(
                                &fence.token,
                                fence.client_epoch,
                                cid as i64,
                                !enabled,
                                None,
                            )
                            .await;
                    })
                    .detach();
                }
            }
            InspectorAction::OpenMedia { url, is_video } => {
                self.occlude_browser_for_overlay(cx);
                let pal = self.theme_state.palette;
                self.overlays.media_viewer_overlay = Some(cx.new(|_| {
                    let mut m = MediaViewer::new(url, is_video);
                    m.apply_theme(pal, lang);
                    m
                }));
                self.reconcile_chat_scoped_toasts(cx);
            }
            InspectorAction::OpenEditPeer => {
                let is_channel = active_chat.is_some_and(|cid| {
                    self.session_sync
                        .rich_chats_by_id
                        .get(&cid)
                        .is_some_and(|rc| rc.is_channel || rc.chat_type == 3)
                        || self.sidebar.read(cx).channel_chat_ids.contains(&cid)
                });
                let prof = &self.inspector.read(cx).profile;
                self.open_modal_dialog(
                    tr_str(lang, "编辑资料", "Edit Info"),
                    ModalDialogKind::EditPeer {
                        peer_title: prof.name.clone(),
                        peer_username: prof.username.clone(),
                        peer_bio: prof.bio.clone(),
                        is_channel,
                    },
                    420.0,
                    380.0,
                    cx,
                );
            }
            InspectorAction::RevokeInviteLink => {
                if let (Some(cid), Some(fence)) = (active_chat, self.capture_client_fence()) {
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        if let Ok(invs) = fence
                            .client
                            .list_invites_with_token(&fence.token, fence.client_epoch, cid as i64)
                            .await
                        {
                            if let Some(first) = invs.first() {
                                if !fence.preflight_ui(&weak, app).await {
                                    return;
                                }
                                let _ = fence
                                    .client
                                    .revoke_invite_with_token(
                                        &fence.token,
                                        fence.client_epoch,
                                        cid as i64,
                                        &first.invite_id,
                                    )
                                    .await;
                            }
                        }
                    })
                    .detach();
                }
            }
            InspectorAction::OpenAddMember => {
                let pending_add_uid = self
                    .inspector
                    .update(cx, |i, _| i.take_pending_add_member_user_id());
                if let Some(target_uid) = pending_add_uid {
                    let Some(cid) = active_chat else {
                        return;
                    };
                    let Some(fence) = self.capture_client_fence() else {
                        return;
                    };
                    let lang_owned = lang.to_string();
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let add_res = fence
                            .client
                            .add_chat_members_with_token(
                                &fence.token,
                                fence.client_epoch,
                                cid as i64,
                                &[target_uid],
                            )
                            .await;
                        let refreshed_members =
                            if add_res.is_ok() && fence.preflight_ui(&weak, app).await {
                                fence
                                    .client
                                    .get_chat_members_with_token(
                                        &fence.token,
                                        fence.client_epoch,
                                        cid as i64,
                                    )
                                    .await
                                    .ok()
                            } else {
                                None
                            };
                        let _ = weak.update(app, |shell, cx| {
                            if !fence.is_valid(shell) {
                                return;
                            }
                            match add_res {
                                Ok(_) => {
                                    if let Some(ref mems) = refreshed_members {
                                        let mut mapped = map_chat_members_to_inspector(
                                            mems,
                                            &shell.session_sync.contacts_list,
                                        );
                                        let is_dark = shell.theme_state.palette.mode.is_dark();
                                        for item in &mut mapped {
                                            if let Some(fb) = shell
                                                .miniapps
                                                .fabushi_bots
                                                .iter()
                                                .find(|b| {
                                                    b.bot_user_id > 0
                                                        && b.bot_user_id as u64 == item.id
                                                })
                                            {
                                                if !fb.display_name.trim().is_empty() {
                                                    item.name = fb.display_name.trim().to_string();
                                                    let init = derive_initials(&item.name);
                                                    if !init.is_empty() {
                                                        item.initials = init;
                                                    }
                                                }
                                                let hue = if TdesktopThemePalette::is_tinted_bot_hue(
                                                    &fb.avatar_color,
                                                ) {
                                                    fb.avatar_color.as_str()
                                                } else {
                                                    TdesktopThemePalette::bot_hue_name_for_id(
                                                        item.id,
                                                    )
                                                };
                                                item.color =
                                                    TdesktopThemePalette::bot_hue_colors(hue, is_dark)
                                                        .0;
                                                item.role = "AI Bot".to_string();
                                                item.is_online = true;
                                            }
                                        }
                                        let member_count = mapped.len();
                                        let invite_link = shell
                                            .inspector
                                            .read(cx)
                                            .members
                                            .invite_link
                                            .clone();
                                        shell.inspector.update(cx, |insp, icx| {
                                            insp.sync_members(mapped, invite_link, true);
                                            insp.remove_add_member_candidate(target_uid);
                                            icx.notify();
                                        });
                                        shell.stage.update(cx, |st, st_cx| {
                                            if st.active_chat_id == Some(cid) && member_count > 0 {
                                                st.chat_subtitle = if lang_owned.starts_with("zh") {
                                                    format!("{member_count} 位成员")
                                                } else {
                                                    format!("{member_count} members")
                                                };
                                                st_cx.notify();
                                            }
                                        });
                                        shell.sync_composer_mention_candidates(
                                            cid,
                                            Some(mems.as_slice()),
                                            cx,
                                        );
                                    } else {
                                        shell.inspector.update(cx, |insp, icx| {
                                            insp.remove_add_member_candidate(target_uid);
                                            icx.notify();
                                        });
                                        shell.sync_composer_mention_candidates(cid, None, cx);
                                    }
                                    if shell.stage.read(cx).active_chat_id == Some(cid) {
                                        shell.show_chat_toast(
                                            cid,
                                            tr_str(
                                                &lang_owned,
                                                "已添加成员到当前群聊",
                                                "Added member to group",
                                            ),
                                            2200,
                                            cx,
                                        );
                                    }
                                }
                                Err(e) => {
                                    if shell.stage.read(cx).active_chat_id == Some(cid) {
                                        shell.show_chat_toast(
                                            cid,
                                            format!(
                                                "{}: {}",
                                                tr_str(
                                                    &lang_owned,
                                                    "添加成员失败",
                                                    "Failed to add member"
                                                ),
                                                e
                                            ),
                                            2600,
                                            cx,
                                        );
                                    }
                                }
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                } else {
                    if self.session_sync.contacts_list.is_empty() {
                        self.refresh_contacts(cx);
                    }
                    let mut existing_ids: Vec<u64> = self
                        .inspector
                        .read(cx)
                        .members
                        .members
                        .iter()
                        .map(|m| m.id)
                        .collect();
                    if let Some(me) = self.session_sync.user_id {
                        existing_ids.push(me);
                    }
                    let candidates = build_add_member_candidates(
                        &existing_ids,
                        &self.session_sync.contacts_list,
                        &self.miniapps.fabushi_bots,
                    );
                    self.inspector.update(cx, |insp, icx| {
                        insp.open_add_member_picker(candidates);
                        icx.notify();
                    });
                }
            }
            InspectorAction::ChangeAutoDelete(secs) => {
                if let Some(cid) = active_chat {
                    let ttl_u32 = match secs.trim() {
                        "24 Hours" | "1d" => 86_400,
                        "7 Days" | "1w" => 604_800,
                        "30 Days" | "1m" => 2_592_000,
                        "Off" | "0" | "" => 0,
                        other => other.parse::<u32>().unwrap_or(0),
                    };
                    self.stage.update(cx, |st, st_cx| {
                        st.message_ttl_seconds = ttl_u32;
                        st_cx.notify();
                    });
                    if let Some(rc) = self.session_sync.rich_chats_by_id.get_mut(&cid) {
                        rc.message_ttl_seconds = ttl_u32;
                    }
                    if let Some(fence) = self.capture_client_fence() {
                        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                            if !fence.preflight_ui(&weak, app).await {
                                return;
                            }
                            let _ = fence
                                .client
                                .set_chat_ttl_with_token(
                                    &fence.token,
                                    fence.client_epoch,
                                    cid as i64,
                                    ttl_u32,
                                )
                                .await;
                        })
                        .detach();
                    }
                }
            }
            InspectorAction::CycleMemberRole(_) => {}
            InspectorAction::LaunchMiniApp(url) => {
                if !self.launch_miniapp_from_uri(
                    &url,
                    active_chat,
                    MiniAppLaunchMode::MenuButton,
                    cx,
                ) {
                    if let Some(cid) = active_chat {
                        let peer_uid = self
                            .session_sync
                            .rich_chats_by_id
                            .get(&cid)
                            .and_then(|rc| rc.peer_user_id)
                            .or_else(|| self.session_sync.chat_peer_user_ids.get(&cid).copied());
                        if let Some(app) = self
                            .miniapps
                            .catalog_miniapps
                            .iter()
                            .chain(self.miniapps.installed_miniapps.iter())
                            .find(|a| {
                                a.direct_chat_id == Some(cid as i64)
                                    || peer_uid.is_some_and(|pid| a.bot_user_id == pid as i64)
                            })
                            .cloned()
                        {
                            self.request_launch_miniapp_with_confirm(
                                &app.bot_username,
                                &app.short_name,
                                Some(cid),
                                MiniAppLaunchMode::MenuButton,
                                None,
                                cx,
                            );
                        }
                    }
                }
            }
            InspectorAction::ToggleAttachMenuBot(bot_id) => {
                let in_attach_now = self.inspector.read(cx).profile.in_attach_menu;
                let target_app = self
                    .miniapps
                    .installed_miniapps
                    .iter()
                    .chain(self.miniapps.catalog_miniapps.iter())
                    .find(|a| {
                        (bot_id > 0 && a.bot_user_id == bot_id as i64)
                            || active_chat.is_some_and(|cid| a.direct_chat_id == Some(cid as i64))
                    })
                    .cloned();
                if let Some(app) = target_app {
                    let Some(fence) = self.capture_client_fence() else {
                        return;
                    };
                    let bot_un = app.bot_username.clone();
                    let short = app.short_name.clone();
                    let target_cid = app
                        .direct_chat_id
                        .and_then(|v| u64::try_from(v).ok())
                        .filter(|&id| id > 0)
                        .or(active_chat);
                    let captured_active_chat = active_chat;
                    let captured_panel = self.active_panel;
                    cx.spawn(async move |weak: WeakEntity<Self>, app_cx: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app_cx).await {
                            return;
                        }
                        let res = if in_attach_now {
                            fence
                                .client
                                .uninstall_mini_app_with_token(
                                    &fence.token,
                                    fence.client_epoch,
                                    &bot_un,
                                    &short,
                                )
                                .await
                        } else {
                            fence
                                .client
                                .install_mini_app_with_token(
                                    &fence.token,
                                    fence.client_epoch,
                                    &bot_un,
                                    &short,
                                    true,
                                )
                                .await
                        };
                        let _ = weak.update(app_cx, |shell, cx| {
                            if !fence.is_valid(shell) {
                                return;
                            }
                            if let Ok(r) = res {
                                if let Some(t) = r.toast {
                                    if let Some(cid) = target_cid {
                                        if shell.stage.read(cx).active_chat_id == Some(cid) {
                                            shell.show_chat_toast(cid, t, 2200, cx);
                                        }
                                    } else {
                                        let cur_active = resolve_active_target_chat_id(
                                            shell.stage.read(cx).active_chat_id,
                                            shell.sidebar.read(cx).active_chat_id,
                                        );
                                        if shell.active_panel == captured_panel
                                            && cur_active == captured_active_chat
                                        {
                                            if let Some(cid) = captured_active_chat {
                                                shell.show_chat_toast(cid, t, 2200, cx);
                                            } else {
                                                shell.show_non_chat_toast(t, 2200, cx);
                                            }
                                        }
                                    }
                                }
                                shell.refresh_miniapps_catalog(None, cx);
                            }
                        });
                    })
                    .detach();
                } else {
                    let msg = tr_str(
                        lang,
                        "该机器人暂未绑定可安装的小程序",
                        "No Mini App available for this bot",
                    );
                    if let Some(cid) = active_chat {
                        self.show_chat_toast(cid, msg, 2200, cx);
                    } else {
                        self.show_non_chat_toast(msg, 2200, cx);
                    }
                }
            }
            InspectorAction::RunBotCommand(cmd) => {
                let trimmed = cmd.trim();
                if !trimmed.is_empty() {
                    let slash = if trimmed.starts_with('/') {
                        trimmed.to_string()
                    } else {
                        format!("/{trimmed}")
                    };
                    self.append_non_composer_outgoing_text(slash, cx);
                }
            }
            InspectorAction::FocusBotComposer => {
                self.composer.update(cx, |c, c_cx| {
                    c.is_input_focused = true;
                    c_cx.notify();
                });
            }
            InspectorAction::OpenMiniAppTos => {
                self.occlude_browser_for_overlay(cx);
                self.miniapps.miniapp_tos_modal_open = true;
                self.reconcile_chat_scoped_toasts(cx);
            }
            InspectorAction::OpenBotPrivacyPolicy(url) => {
                self.open_external_url_with_toast(&url, cx);
            }
            InspectorAction::OpenBotSettings(_bot_id) => {
                if active_chat.is_some() {
                    self.append_non_composer_outgoing_text("/settings".to_string(), cx);
                } else {
                    self.enqueue_localized_toast(
                        "请先点击下方开始或打开机器人对话以配置设置",
                        "Start or open the bot chat first to manage settings",
                        2200,
                        cx,
                    );
                }
            }
            InspectorAction::OpenAiProvidersSettings => {
                self.open_ai_providers_settings(cx);
            }
            InspectorAction::OpenAddBotToGroup(bot_id) => {
                let is_dark = self.theme_state.palette.mode.is_dark();
                let fab_bot = self
                    .miniapps
                    .fabushi_bots
                    .iter()
                    .find(|b| bot_id > 0 && b.bot_user_id > 0 && b.bot_user_id as u64 == bot_id)
                    .cloned()
                    .or_else(|| self.inspector.read(cx).active_fabushi_bot.clone());

                let (user_id, display_name, username, agent_id, avatar_color) =
                    if let Some(fb) = fab_bot {
                        let uid = fb.bot_user_id.max(0) as u64;
                        let uname = fb.username.trim().trim_start_matches('@').to_string();
                        let name = if fb.display_name.trim().is_empty() {
                            if !uname.is_empty() {
                                uname.clone()
                            } else {
                                fb.agent_id.clone()
                            }
                        } else {
                            fb.display_name.trim().to_string()
                        };
                        let hue = if TdesktopThemePalette::is_tinted_bot_hue(&fb.avatar_color) {
                            fb.avatar_color.as_str()
                        } else {
                            TdesktopThemePalette::bot_hue_name_for_id(uid)
                        };
                        let (fill, _, _) = TdesktopThemePalette::bot_hue_colors(hue, is_dark);
                        (
                            if uid > 0 { uid } else { bot_id },
                            name,
                            (!uname.is_empty()).then_some(uname),
                            (!fb.agent_id.trim().is_empty()).then_some(fb.agent_id),
                            fill,
                        )
                    } else {
                        let prof = &self.inspector.read(cx).profile;
                        let uid = if bot_id > 0 { bot_id } else { prof.bot_id };
                        let uname = prof.username.trim().trim_start_matches('@').to_string();
                        let name = if prof.name.trim().is_empty() {
                            if !uname.is_empty() {
                                uname.clone()
                            } else {
                                format!("Bot {uid}")
                            }
                        } else {
                            prof.name.trim().to_string()
                        };
                        let col = if prof.avatar_color != 0 {
                            prof.avatar_color
                        } else {
                            peer_avatar_color(uid)
                        };
                        (uid, name, (!uname.is_empty()).then_some(uname), None, col)
                    };

                if user_id > 0 {
                    self.inspector.update(cx, |i, icx| {
                        i.close();
                        icx.notify();
                    });
                    self.enter_compose_or_modal_isolation(cx);
                    self.composer.update(cx, |c, ccx| {
                        c.is_input_focused = false;
                        ccx.notify();
                    });
                    self.sidebar.update(cx, |sb, sb_cx| {
                        sb.open_compose_with_seed_recipient(
                            user_id,
                            display_name,
                            username,
                            true,
                            agent_id,
                            avatar_color,
                        );
                        sb_cx.notify();
                    });
                    self.sync_compose_state_to_composer(true, cx);
                } else {
                    let msg = tr_str(lang, "未能识别该 Bot 信息", "Unable to resolve bot identity");
                    if let Some(cid) = active_chat {
                        self.show_chat_toast(cid, msg, 2200, cx);
                    } else {
                        self.show_non_chat_toast(msg, 2200, cx);
                    }
                }
            }
            InspectorAction::ReportBot(_bot_id) => {
                self.enqueue_localized_toast(
                    "已提交举报，感谢您的反馈",
                    "Report submitted. Thank you for your feedback.",
                    2200,
                    cx,
                );
            }
            InspectorAction::ToggleBlockBot(bot_id) => {
                self.toggle_bot_block_status(bot_id, cx);
            }
            act @ (InspectorAction::OpenBotComputersSubpage
            | InspectorAction::CloseBotComputersSubpage
            | InspectorAction::OpenRemoteWorkbench { .. }
            | InspectorAction::OpenComputerDetailInSettings { .. }
            | InspectorAction::OpenCreateCloudComputerModal { .. }
            | InspectorAction::BindBotToComputer { .. }
            | InspectorAction::SaveBotComputerConfig { .. }) => {
                let _ = self.handle_computer_inspector_action(act, cx);
            }
            InspectorAction::PatchFabushiBot { bot_id, req } => {
                let Some(fence) = self.capture_client_fence() else {
                    return;
                };
                let lang_owned = lang.to_string();
                let target_cid = active_chat;
                let captured_panel = self.active_panel;
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let res = fence
                        .client
                        .patch_fabushi_bot_with_token(&fence.token, fence.client_epoch, bot_id, &req)
                        .await;
                    let _ = weak.update(app, |shell, cx| {
                        if !fence.is_valid(shell) {
                            return;
                        }
                        let show_patch_toast =
                            |shell: &mut Self, msg: String, duration_ms: u64, cx: &mut Context<Self>| {
                                if let Some(cid) = target_cid {
                                    if shell.stage.read(cx).active_chat_id == Some(cid) {
                                        shell.show_chat_toast(cid, msg, duration_ms, cx);
                                    }
                                } else {
                                    let cur_active = resolve_active_target_chat_id(
                                        shell.stage.read(cx).active_chat_id,
                                        shell.sidebar.read(cx).active_chat_id,
                                    );
                                    if shell.active_panel == captured_panel
                                        && cur_active.is_none()
                                    {
                                        shell.show_non_chat_toast(msg, duration_ms, cx);
                                    }
                                }
                            };
                        match res {
                            Ok(updated) => {
                                if let Some(slot) = shell
                                    .miniapps
                                    .fabushi_bots
                                    .iter_mut()
                                    .find(|b| b.bot_user_id == updated.bot_user_id)
                                {
                                    *slot = updated.clone();
                                } else {
                                    shell.miniapps.fabushi_bots.push(updated.clone());
                                }
                                for prof in shell.miniapps.bot_profiles_cache.values_mut() {
                                    if prof.bot.bot_user_id == updated.bot_user_id {
                                        prof.bot.coding_agent = updated.coding_agent.clone();
                                        prof.bot.preferred_model = updated.preferred_model.clone();
                                    }
                                }
                                if let Some(comp_prof) = shell
                                    .computers
                                    .bot_profiles
                                    .get_mut(&(updated.bot_user_id.max(0) as u64))
                                {
                                    comp_prof.preferred_model = updated.preferred_model.clone();
                                    let synced_prof = comp_prof.clone();
                                    if shell.inspector.read(cx).profile.bot_id
                                        == updated.bot_user_id.max(0) as u64
                                    {
                                        shell.inspector.update(cx, |insp, icx| {
                                            insp.sync_bot_computer_profile(Some(synced_prof));
                                            icx.notify();
                                        });
                                    }
                                }
                                shell.sync_fabushi_bots_to_views(cx);
                                shell.refresh_miniapps_catalog(None, cx);
                                let msg = tr_str(
                                    &lang_owned,
                                    "已更新 Fabushi 智能体配置",
                                    "Updated Fabushi bot settings",
                                );
                                show_patch_toast(shell, msg.to_string(), 1800, cx);
                            }
                            Err(e) => {
                                let msg = format!(
                                    "{}: {}",
                                    tr_str(&lang_owned, "更新失败", "Update failed"),
                                    e
                                );
                                show_patch_toast(shell, msg, 2600, cx);
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Shared cleanup for leaving or deleting a chat across both modal confirmation and sidebar
    /// context-menu deletion so drafts, toasts, composer state, and inspector facts never leak.
    pub(crate) fn cleanup_deleted_or_left_chat_state(
        &mut self,
        cid: u64,
        cx: &mut Context<Self>,
    ) {
        let was_active_chat = self.stage.read(cx).active_chat_id == Some(cid)
            || self.sidebar.read(cx).active_chat_id == Some(cid);
        self.session_sync.rich_chats_by_id.remove(&cid);
        self.session_sync.chat_peer_user_ids.remove(&cid);
        self.session_sync
            .bump_draft_op_gen_and_clear_in_flight(cid);
        self.session_sync.last_draft_text_by_chat.remove(&cid);
        self.session_sync.last_saved_draft_by_chat.remove(&cid);
        self.client
            .local_store
            .set_draft(gpui_telegram_core::types::ChatId(cid as i64), None);

        let cid_i64 = cid as i64;
        for profile in self.miniapps.bot_profiles_cache.values_mut() {
            if profile.existing_chat_id == Some(cid_i64) {
                profile.existing_chat_id = None;
                profile.has_started_chat = false;
            }
            if let Some(ref mut main_app) = profile.main_app {
                if main_app.direct_chat_id == Some(cid_i64) {
                    main_app.direct_chat_id = None;
                }
            }
            for app in &mut profile.apps {
                if app.direct_chat_id == Some(cid_i64) {
                    app.direct_chat_id = None;
                }
            }
        }
        for fb in &mut self.miniapps.fabushi_bots {
            if fb.companion_chat_id == Some(cid_i64) {
                fb.companion_chat_id = None;
            }
        }
        for app in self
            .miniapps
            .catalog_miniapps
            .iter_mut()
            .chain(self.miniapps.installed_miniapps.iter_mut())
            .chain(self.miniapps.recent_mini_apps.iter_mut())
        {
            if app.direct_chat_id == Some(cid_i64) {
                app.direct_chat_id = None;
            }
        }
        if let Some(ref box_ent) = self.miniapps.bot_profile_box {
            box_ent.update(cx, |bx, bx_cx| {
                if bx.profile.existing_chat_id == Some(cid_i64) {
                    bx.profile.existing_chat_id = None;
                    bx.profile.has_started_chat = false;
                    bx_cx.notify();
                }
            });
        }
        self.miniapps
            .pending_webmcp_commands
            .retain(|cmd| cmd.chat_id != cid_i64);
        if self
            .miniapps
            .floating_pip_status
            .as_ref()
            .is_some_and(|pip| pip.chat_id == cid_i64)
        {
            self.miniapps.floating_pip_status = None;
        }
        if self.miniapps.miniapp_overlay.as_ref().is_some_and(|ov| {
            let w = ov.read(cx);
            w.companion_chat_id == Some(cid_i64) || w.source_chat_id == Some(cid_i64)
        }) {
            self.miniapps.miniapp_overlay = None;
            self.miniapps.is_miniapp_minimized = false;
        }

        self.stage.update(cx, |st, _| {
            st.chat_histories.remove(&cid);
            st.chat_pinned_messages.remove(&cid);
            st.chat_pinned_entries.remove(&cid);
            st.chat_scroll_positions.remove(&cid);
        });

        let next_chat_id = self.sidebar.update(cx, |sb, sb_cx| {
            let needs_delete = sb.dialogs.iter().any(|d| d.id == cid)
                || sb.drafts.contains_key(&cid)
                || sb.active_chat_id == Some(cid);
            if needs_delete {
                sb.delete_dialog(cid);
                sb_cx.notify();
            }
            sb.dialogs.first().map(|d| d.id)
        });

        if self.window_runtime.controller.clear_toasts_for_chat(cid) {
            self.schedule_current_toast_dismiss(cx);
        }
        if was_active_chat || self.last_observed_toast_chat_id == Some(cid) {
            self.last_observed_toast_chat_id = None;
        }
        if was_active_chat
            || self
                .compose_isolation_snapshot
                .as_ref()
                .is_some_and(|s| s.chat_id == Some(cid))
        {
            self.clear_compose_isolation_snapshot();
        }
        if was_active_chat {
            self.miniapps.pending_prestart_bot = None;
            self.stage.update(cx, |st, _| {
                st.active_chat_id = None;
            });
            if let Some(next_id) = next_chat_id {
                self.composer.update(cx, |comp, c_cx| {
                    if !comp.is_compose_isolated {
                        comp.cancel_reply_or_edit();
                        comp.editor.attachments.clear();
                        comp.submitted_attachment = None;
                        comp.pending_attached_files = None;
                        comp.clear_bot_reply_keyboard();
                        c_cx.notify();
                    }
                });
                self.sidebar.update(cx, |sb, _| sb.select_dialog(next_id));
                self.sync_selected_chat_inspector_and_stage(next_id, cx);
            } else {
                self.stage.update(cx, |st, st_cx| {
                    st.clear_selection();
                    st_cx.notify();
                });
                let still_isolated = self.is_compose_or_creation_modal_active(cx);
                self.composer.update(cx, |comp, c_cx| {
                    if !still_isolated {
                        comp.clear();
                        comp.clear_bot_menu(Some(c_cx));
                        comp.set_bot_slash_commands(Vec::new());
                        comp.set_hermes_slash_enabled(false);
                        comp.clear_bot_reply_keyboard();
                        comp.cancel_reply_or_edit();
                        comp.editor.attachments.clear();
                        comp.submitted_attachment = None;
                        comp.pending_attached_files = None;
                        comp.set_send_as_options(Vec::new(), None);
                        comp.is_compose_isolated = false;
                        c_cx.notify();
                    }
                });
                self.inspector.update(cx, |insp, insp_cx| {
                    insp.clear_peer_facts();
                    insp_cx.notify();
                });
                self.set_inspector_open(false, cx);
                self.sync_composer_and_inspector_miniapp_state(cx);
            }
        }
    }

    /// Handles pin, mute, archive, clear history, and delete context-menu actions drained from
    /// `ChatSidebar::pending_context_action`.
    ///
    /// `ChatSidebar::apply_context_action` has already applied the local state mutation before
    /// `observe_sidebar_entity` drains `pending_context_action`, so this handler must NOT re-apply
    /// or re-queue the action onto `self.sidebar`.
    pub fn handle_sidebar_context_action(
        &mut self,
        action: SidebarContextAction,
        cx: &mut Context<Self>,
    ) {
        match action {
            SidebarContextAction::TogglePin(id) => {
                let pinned = self
                    .sidebar
                    .read(cx)
                    .dialogs
                    .iter()
                    .find(|d| d.id == id)
                    .map(|d| d.is_pinned)
                    .unwrap_or(false);
                if let Some(rc) = self.session_sync.rich_chats_by_id.get_mut(&id) {
                    rc.is_pinned = pinned;
                }
                if self.stage.read(cx).active_chat_id == Some(id) {
                    self.stage.update(cx, |st, st_cx| {
                        st.is_pinned_chat = pinned;
                        st_cx.notify();
                    });
                }
                if let Some(fence) = self.capture_client_fence() {
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let _ = fence
                            .client
                            .set_chat_pinned_with_token(
                                &fence.token,
                                fence.client_epoch,
                                id as i64,
                                pinned,
                            )
                            .await;
                    })
                    .detach();
                }
            }
            SidebarContextAction::ToggleMute(id) => {
                let muted = self
                    .sidebar
                    .read(cx)
                    .dialogs
                    .iter()
                    .find(|d| d.id == id)
                    .map(|d| d.is_muted)
                    .unwrap_or(false);
                if let Some(rc) = self.session_sync.rich_chats_by_id.get_mut(&id) {
                    rc.is_muted = muted;
                }
                if self.stage.read(cx).active_chat_id == Some(id) {
                    self.stage.update(cx, |st, st_cx| {
                        st.is_muted_chat = muted;
                        st_cx.notify();
                    });
                }
                if let Some(fence) = self.capture_client_fence() {
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let _ = fence
                            .client
                            .set_chat_muted_with_token(
                                &fence.token,
                                fence.client_epoch,
                                id as i64,
                                muted,
                                None,
                            )
                            .await;
                    })
                    .detach();
                }
            }
            SidebarContextAction::ToggleArchive(id) => {
                let arch = self.sidebar.read(cx).archived_ids.contains(&id);
                if let Some(rc) = self.session_sync.rich_chats_by_id.get_mut(&id) {
                    rc.is_archived = arch;
                }
                if let Some(fence) = self.capture_client_fence() {
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let _ = fence
                            .client
                            .set_chat_archived_with_token(
                                &fence.token,
                                fence.client_epoch,
                                id as i64,
                                arch,
                            )
                            .await;
                    })
                    .detach();
                }
            }
            SidebarContextAction::DeleteChat(id) => {
                let fence = self.capture_client_fence();
                self.cleanup_deleted_or_left_chat_state(id, cx);
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
                                id as i64,
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
            SidebarContextAction::ClearHistory(id) => {
                let fence = self.capture_client_fence();
                self.stage.update(cx, |st, st_cx| {
                    st.chat_histories.insert(id, Vec::new());
                    if st.active_chat_id == Some(id) {
                        st.messages.clear();
                        st.row_height_cache.rebuild_prefix_sums(&st.messages);
                        st_cx.notify();
                    }
                });
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
                                id as i64,
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
            SidebarContextAction::ToggleUnread(_) => {}
        }
        cx.notify();
    }
}
