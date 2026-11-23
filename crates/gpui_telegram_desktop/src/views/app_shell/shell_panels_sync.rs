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
use crate::views::app_shell::shell_truth::{derive_initials, SessionRequestFence};
use crate::views::boxes::modal_types::ModalDialogKind;
use crate::views::context_inspector::build_add_member_candidates;
use crate::views::drawer::inspector_tabs::InspectorAction;
use crate::views::shell::AppShell;
use crate::views::sidebar::dialogs_row_view::SidebarContextAction;
use crate::views::MediaViewer;

fn session_fence(shell: &AppShell) -> Option<SessionRequestFence> {
    let (Some(token), Some(uid)) =
        (shell.session_sync.session_token.clone(), shell.session_sync.user_id)
    else {
        return None;
    };
    Some(SessionRequestFence {
        auth_generation: shell.session_sync.auth_generation,
        request_generation: 0,
        token,
        user_id: uid,
    })
}

fn fence_matches(fence: &SessionRequestFence, shell: &AppShell) -> bool {
    fence.matches(
        shell.is_authenticated,
        shell.session_sync.auth_generation,
        0,
        shell.session_sync.session_token.as_deref(),
        shell.session_sync.user_id,
    )
}

impl AppShell {
    /// Fetches chat folders from backend and updates navigation rail and folder settings.
    pub fn refresh_folders(&mut self, cx: &mut Context<Self>) {
        let Some(fence) = session_fence(self) else {
            return;
        };
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if let Ok(folders) = client.list_folders().await {
                let _ = weak.update(app, |shell, cx| {
                    if !fence_matches(&fence, shell) {
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
        let Some(fence) = session_fence(self) else {
            return;
        };
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if let Ok(list) = client.list_contacts().await {
                let _ = weak.update(app, |shell, cx| {
                    if !fence_matches(&fence, shell) {
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
        let Some(fence) = session_fence(self) else {
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
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let bal = client.wallet_balance().await.ok();
            let txs = client.wallet_transactions().await.unwrap_or_default();
            let _ = weak.update(app, |shell, cx| {
                if !fence_matches(&fence, shell) {
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
                    self.window_runtime.enqueue_toast(
                        tr_str(lang, "请选择私聊用户或机器人对话", "Select a direct peer or bot chat"),
                        2600,
                    );
                }
            }
            InspectorAction::StartVideoCall(name) => {
                if self.is_active_chat_bot(cx) {
                    self.start_ai_talk_mode_session(true, cx);
                } else if let Some(pid) = self.current_peer_user_id(cx) {
                    self.start_webrtc_call(pid, name, true, cx);
                } else {
                    self.window_runtime.enqueue_toast(
                        tr_str(lang, "请选择私聊用户或机器人对话", "Select a direct peer or bot chat"),
                        2600,
                    );
                }
            }
            InspectorAction::CopyText(txt) => {
                cx.write_to_clipboard(ClipboardItem::new_string(txt))
            }
            InspectorAction::ToggleNotifications(enabled) => {
                if let Some(cid) = active_chat {
                    let c = self.client.clone();
                    cx.spawn(async move |_: WeakEntity<Self>, _: &mut AsyncApp| {
                        let _ = c.set_chat_muted(cid as i64, !enabled, None).await;
                    })
                    .detach();
                }
            }
            InspectorAction::OpenMedia { url, is_video } => {
                let pal = self.theme_state.palette;
                self.overlays.media_viewer_overlay = Some(cx.new(|_| {
                    let mut m = MediaViewer::new(url, is_video);
                    m.apply_theme(pal, lang);
                    m
                }));
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
                if let Some(cid) = active_chat {
                    let c = self.client.clone();
                    cx.spawn(async move |_: WeakEntity<Self>, _: &mut AsyncApp| {
                        if let Ok(invs) = c.list_invites(cid as i64).await {
                            if let Some(first) = invs.first() {
                                let _ = c.revoke_invite(cid as i64, &first.invite_id).await;
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
                    let Some(fence) = session_fence(self) else {
                        return;
                    };
                    let client = self.client.clone();
                    let lang_owned = lang.to_string();
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        let add_res = client.add_chat_members(cid as i64, &[target_uid]).await;
                        let refreshed_members = if add_res.is_ok() {
                            client.get_chat_members(cid as i64).await.ok()
                        } else {
                            None
                        };
                        let _ = weak.update(app, |shell, cx| {
                            if !fence_matches(&fence, shell) {
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
                                    shell.window_runtime.enqueue_toast(
                                        tr_str(
                                            &lang_owned,
                                            "已添加成员到当前群聊",
                                            "Added member to group",
                                        ),
                                        2200,
                                    );
                                }
                                Err(e) => {
                                    shell.window_runtime.enqueue_toast(
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
                                    );
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
                    let c = self.client.clone();
                    cx.spawn(async move |_: WeakEntity<Self>, _: &mut AsyncApp| {
                        let _ = c.set_chat_ttl(cid as i64, ttl_u32).await;
                    })
                    .detach();
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
                    let client = self.client.clone();
                    let Some(token) = self.session_sync.session_token.clone() else {
                        return;
                    };
                    let fence = SessionRequestFence {
                        auth_generation: self.session_sync.auth_generation,
                        request_generation: 0,
                        token,
                        user_id: self.session_sync.user_id.unwrap_or(0),
                    };
                    let bot_un = app.bot_username.clone();
                    let short = app.short_name.clone();
                    cx.spawn(async move |weak: WeakEntity<Self>, app_cx: &mut AsyncApp| {
                        let res = if in_attach_now {
                            client.uninstall_mini_app(&bot_un, &short).await
                        } else {
                            client.install_mini_app(&bot_un, &short, true).await
                        };
                        let _ = weak.update(app_cx, |shell, cx| {
                            if !fence_matches(&fence, shell) {
                                return;
                            }
                            if let Ok(r) = res {
                                if let Some(t) = r.toast {
                                    shell.window_runtime.enqueue_toast(t, 2200);
                                }
                                shell.refresh_miniapps_catalog(None, cx);
                            }
                        });
                    })
                    .detach();
                } else {
                    self.window_runtime.enqueue_toast(
                        tr_str(lang, "该机器人暂未绑定可安装的小程序", "No Mini App available for this bot"),
                        2200,
                    );
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
                    self.append_outgoing_text(slash, cx);
                }
            }
            InspectorAction::FocusBotComposer => {
                self.composer.update(cx, |c, c_cx| {
                    c.is_input_focused = true;
                    c_cx.notify();
                });
            }
            InspectorAction::OpenMiniAppTos => {
                self.miniapps.miniapp_tos_modal_open = true;
            }
            InspectorAction::OpenBotPrivacyPolicy(url) => {
                self.open_external_url_with_toast(&url, cx);
            }
            InspectorAction::OpenBotSettings(_bot_id) => {
                if active_chat.is_some() {
                    self.append_outgoing_text("/settings".to_string(), cx);
                } else {
                    self.enqueue_localized_toast(
                        "请先点击下方开始或打开机器人对话以配置设置",
                        "Start or open the bot chat first to manage settings",
                        2200,
                        cx,
                    );
                }
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
                } else {
                    self.window_runtime.enqueue_toast(
                        tr_str(lang, "未能识别该 Bot 信息", "Unable to resolve bot identity"),
                        2200,
                    );
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
                let Some(fence) = session_fence(self) else {
                    return;
                };
                let client = self.client.clone();
                let lang_owned = lang.to_string();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    let res = client.patch_fabushi_bot(bot_id, &req).await;
                    let _ = weak.update(app, |shell, cx| {
                        if !fence_matches(&fence, shell) {
                            return;
                        }
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
                                    }
                                }
                                shell.sync_fabushi_bots_to_views(cx);
                                shell.refresh_miniapps_catalog(None, cx);
                                shell.window_runtime.enqueue_toast(
                                    tr_str(&lang_owned, "已更新 Fabushi 智能体配置", "Updated Fabushi bot settings"),
                                    1800,
                                );
                            }
                            Err(e) => {
                                shell.window_runtime.enqueue_toast(
                                    format!("{}: {}", tr_str(&lang_owned, "更新失败", "Update failed"), e),
                                    2600,
                                );
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
        let client = self.client.clone();
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
                cx.spawn(async move |_: WeakEntity<Self>, _: &mut AsyncApp| {
                    let _ = client.set_chat_pinned(id as i64, pinned).await;
                })
                .detach();
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
                cx.spawn(async move |_: WeakEntity<Self>, _: &mut AsyncApp| {
                    let _ = client.set_chat_muted(id as i64, muted, None).await;
                })
                .detach();
            }
            SidebarContextAction::ToggleArchive(id) => {
                let arch = self.sidebar.read(cx).archived_ids.contains(&id);
                if let Some(rc) = self.session_sync.rich_chats_by_id.get_mut(&id) {
                    rc.is_archived = arch;
                }
                cx.spawn(async move |_: WeakEntity<Self>, _: &mut AsyncApp| {
                    let _ = client.set_chat_archived(id as i64, arch).await;
                })
                .detach();
            }
            SidebarContextAction::DeleteChat(id) => {
                self.session_sync.rich_chats_by_id.remove(&id);
                if self.stage.read(cx).active_chat_id == Some(id) {
                    let next_id = self.sidebar.read(cx).dialogs.first().map(|d| d.id);
                    if let Some(nid) = next_id {
                        self.sidebar.update(cx, |sb, _| sb.select_dialog(nid));
                        self.sync_selected_chat_inspector_and_stage(nid, cx);
                    } else {
                        self.stage.update(cx, |st, _| st.clear_selection());
                    }
                }
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    let _ = client.leave_or_delete_chat(id as i64).await;
                    let _ = weak.update(app, |s, cx| s.refresh_chats(cx));
                })
                .detach();
            }
            SidebarContextAction::ClearHistory(id) => {
                self.stage.update(cx, |st, st_cx| {
                    st.chat_histories.insert(id, Vec::new());
                    if st.active_chat_id == Some(id) {
                        st.messages.clear();
                        st.row_height_cache.rebuild_prefix_sums(&st.messages);
                        st_cx.notify();
                    }
                });
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    let _ = client.clear_chat_history(id as i64).await;
                    let _ = weak.update(app, |s, cx| s.refresh_chats(cx));
                })
                .detach();
            }
            SidebarContextAction::ToggleUnread(_) => {}
        }
        cx.notify();
    }
}
