//! `AppShell` hosting, real Human+Bot identity sync, realtime WS event bridging,
//! and cross-app Telegram bridges (`MediaViewer`, DM `open_or_select_chat`, Forward modal,
//! toasts, unread badges) for `CommunityWorkspace`.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use gpui::{App, AppContext, AsyncApp, Context, Entity, WeakEntity};
use gpui_telegram_client::{
    ClientDomainEvent, TelegramBotSyncInput, TelegramHumanSyncInput,
};
use gpui_telegram_proto::contracts::CreateChatRequest;

use crate::i18n::tr_str;
use crate::views::boxes::modal_types::ModalDialogKind;
use crate::views::community::{
    ActiveCommunityInput, CommunityWorkspace, CommunityWorkspaceAction,
};
use crate::views::content_hub::ContentHubInputTarget;
use crate::views::nav_rail::NavDestination;
use crate::views::shell::{ActiveWorkspacePanel, AppShell};

impl AppShell {
    /// Opens or focuses the fused Reddit + X `CommunityWorkspace` main-pane panel.
    pub fn open_community_workspace(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Entity<CommunityWorkspace> {
        self.leave_browser_panel_if_active(cx);
        self.leave_code_scene_panel_if_active(cx);
        self.cleanup_settings_panel_if_active(cx);
        let was_outside = self.active_panel != ActiveWorkspacePanel::Community;
        if was_outside {
            self.clear_ime_and_blur_content_hub_for_transition(true, cx);
            self.community_ime_preedit_input_gen = None;
        }
        self.active_panel = ActiveWorkspacePanel::Community;
        self.sync_nav_rail_destination_silent(NavDestination::Community, cx);
        self.sync_apple_sidebar_to_active_panel(cx);
        self.sidebar.update(cx, |sb, scx| {
            sb.cancel_compose_mode();
            sb.is_account_popover_open = false;
            if sb.search_focused {
                sb.search_focused = false;
            }
            scx.notify();
        });
        if self.composer.read(cx).is_compose_isolated
            && !self.is_compose_or_creation_modal_active(cx)
        {
            self.exit_compose_or_modal_isolation(true, cx);
        } else {
            self.sync_compose_state_to_composer(true, cx);
        }
        self.composer.update(cx, |c, ccx| {
            let mut changed = false;
            if c.is_input_focused {
                c.is_input_focused = false;
                changed = true;
            }
            if c.ime_machine.is_composing() {
                c.ime_machine.on_ime_cancel();
                c.sync_autocomplete();
                changed = true;
            }
            if c.bot_ac_visible || c.hermes_ac_visible || c.context_ref_hint_open {
                c.bot_ac_visible = false;
                c.bot_ac_matches.clear();
                c.bot_ac_selected = 0;
                c.bot_ac_pinned_by_button = false;
                c.hermes_ac_visible = false;
                c.hermes_ac_matches.clear();
                c.hermes_ac_pinned_by_button = false;
                c.context_ref_hint_open = false;
                c.autocomplete.hide();
                changed = true;
            }
            if changed {
                ccx.notify();
            }
        });
        self.stage.update(cx, |st, st_cx| {
            if st.more_menu_open || st.ttl_submenu_open {
                st.more_menu_open = false;
                st.ttl_submenu_open = false;
                st_cx.notify();
            }
        });

        let pal = self.theme_state.palette;
        let lang = self.active_lang_id().to_string();
        let sidebar_w = self.sidebar.read(cx).effective_width();
        let owned_actor_ids = self.collect_shell_owned_community_actor_ids();

        let cw = if let Some(existing) = self.community_workspace.clone() {
            existing.update(cx, |w, wcx| {
                w.set_host_sidebar_width(sidebar_w);
                w.set_known_owned_actor_ids(owned_actor_ids);
                w.sync_theme_and_locale(pal, lang.clone(), wcx);
            });
            existing
        } else {
            let client = Arc::clone(&self.client);
            let created = cx.new(|wcx| {
                let mut w = CommunityWorkspace::new(client, pal, lang, wcx);
                w.set_host_sidebar_width(sidebar_w);
                w.set_known_owned_actor_ids(owned_actor_ids);
                w
            });
            let mut last_input_gen = created.read(cx).active_input_generation();
            cx.observe(&created, move |shell, cw, cx| {
                let (cur_input, cur_gen) = {
                    let r = cw.read(cx);
                    (r.state.active_input, r.active_input_generation())
                };
                if cur_gen != last_input_gen {
                    last_input_gen = cur_gen;
                    let preedit_matches = cur_input != ActiveCommunityInput::None
                        && shell.community_ime_preedit_input_gen == Some(cur_gen);
                    let community_unoccluded = shell.active_panel == ActiveWorkspacePanel::Community
                        && !shell.has_community_visual_occlusion_overlay(cx);
                    if cur_input != ActiveCommunityInput::None {
                        if community_unoccluded {
                            if shell.sidebar.read(cx).search_focused {
                                shell.sidebar.update(cx, |sb, scx| {
                                    sb.search_focused = false;
                                    scx.notify();
                                });
                            }
                            if shell.content_hub.read(cx).active_input
                                == ContentHubInputTarget::QuickDrawerSearch
                            {
                                shell.content_hub.update(cx, |h, hcx| {
                                    h.blur_input();
                                    hcx.notify();
                                });
                            }
                            if !preedit_matches {
                                shell.clear_all_ime_preedits(cx);
                            }
                            cx.notify();
                        } else {
                            if shell.community_ime_preedit_input_gen.is_some() {
                                shell.ime.clear_preedit();
                                shell.community_ime_preedit_input_gen = None;
                            }
                            cw.update(cx, |w, wcx| w.blur_active_input(wcx));
                            last_input_gen = cw.read(cx).active_input_generation();
                            cx.notify();
                        }
                    } else if shell.community_ime_preedit_input_gen.is_some()
                        || (community_unoccluded
                            && !shell.sidebar.read(cx).search_focused
                            && !shell.content_hub.read(cx).has_focused_input(false))
                    {
                        shell.ime.clear_preedit();
                        shell.community_ime_preedit_input_gen = None;
                        cx.notify();
                    }
                }
                let actions = cw.update(cx, |w, _| w.take_pending_actions());
                for act in actions {
                    shell.handle_community_action(act, cx);
                }
            })
            .detach();
            self.community_workspace = Some(created.clone());
            created
        };

        self.sync_community_identities(cx);
        self.reconcile_chat_scoped_toasts(cx);
        cx.notify();
        cw
    }

    pub fn community_workspace(&self) -> Option<&Entity<CommunityWorkspace>> {
        self.community_workspace.as_ref()
    }

    /// Cleanly blurs Community inputs and clears stale IME preedit when navigating away.
    pub fn leave_community_panel_if_active(&mut self, cx: &mut Context<Self>) {
        if self.active_panel == ActiveWorkspacePanel::Community {
            self.ime.clear_preedit();
            self.community_ime_preedit_input_gen = None;
            if let Some(cw) = self.community_workspace.clone() {
                cw.update(cx, |w, wcx| {
                    w.state.active_popover = None;
                    w.blur_active_input(wcx);
                });
            }
        }
    }

    /// Occludes `CommunityWorkspace` when a shell modal/drawer/overlay/popover opens on top of it,
    /// blurring Community inputs and clearing stale IME preedit iff Community or base workspace owns it.
    pub fn occlude_community_for_overlay(&mut self, cx: &mut Context<Self>) {
        if self.active_panel == ActiveWorkspacePanel::Community {
            if self.community_ime_preedit_input_gen.is_some()
                || self.is_base_workspace_unoccluded(cx)
            {
                self.ime.clear_preedit();
            }
            self.community_ime_preedit_input_gen = None;
            if let Some(cw) = self.community_workspace.clone() {
                cw.update(cx, |w, wcx| {
                    w.blur_active_input(wcx);
                });
            }
        }
    }

    /// Updates the `NavDestination::Community` unread badge across AppleAppSidebar, NavRail, Expanded Sidebar & Collapsed Rail.
    pub fn set_community_unread_badge(&mut self, count: u32, cx: &mut Context<Self>) {
        self.nav_rail.update(cx, |r, rcx| {
            r.set_unread(NavDestination::Community, count);
            rcx.notify();
        });
        self.apple_sidebar.update(cx, |sb, scx| {
            if sb.community_unread_count != count {
                sb.set_community_unread(count);
                scx.notify();
            }
        });
        self.sidebar.update(cx, |_, scx| scx.notify());
        cx.notify();
    }

    /// Builds real Human + owned Fabushi/Telegram Bot identity sync payloads from live `AppShell` state.
    pub fn build_community_identity_sync_inputs(
        &self,
        cx: &App,
    ) -> Option<(TelegramHumanSyncInput, Vec<TelegramBotSyncInput>)> {
        let uid = self.session_sync.user_id.filter(|&id| id > 0)?;
        let uid_i64 = uid as i64;
        let username = self
            .session_sync
            .username
            .clone()
            .filter(|u| !u.trim().is_empty());
        let display_name = self
            .client
            .local_store
            .get_cached_user_display_name()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                let acc = self.nav_rail.read(cx).account_name.clone();
                (!acc.trim().is_empty()).then_some(acc)
            })
            .or_else(|| username.clone())
            .unwrap_or_else(|| format!("User {uid}"));

        let human = TelegramHumanSyncInput {
            user_id: uid_i64,
            username,
            display_name,
            bio: None,
            avatar_media_id: None,
        };

        let mut seen_bot_ids = HashSet::new();
        let mut owned_bots = Vec::new();
        for fb in &self.miniapps.fabushi_bots {
            if fb.bot_user_id <= 0
                || (fb.owner_user_id != 0 && fb.owner_user_id != uid_i64)
                || !seen_bot_ids.insert(fb.bot_user_id)
            {
                continue;
            }
            let dname = if fb.display_name.trim().is_empty() {
                fb.username.clone()
            } else {
                fb.display_name.clone()
            };
            owned_bots.push(TelegramBotSyncInput {
                bot_user_id: fb.bot_user_id,
                username: fb.username.clone(),
                display_name: dname,
                bio: (!fb.description.trim().is_empty()).then(|| fb.description.clone()),
                avatar_media_id: None,
                avatar_shape: (!fb.avatar_shape.trim().is_empty())
                    .then(|| fb.avatar_shape.clone()),
                avatar_color: (!fb.avatar_color.trim().is_empty())
                    .then(|| fb.avatar_color.clone()),
                bot_model: (!fb.preferred_model.trim().is_empty())
                    .then(|| fb.preferred_model.clone()),
                bot_capabilities: if fb.coding_agent.is_some() {
                    vec!["coding".to_string(), "community".to_string()]
                } else {
                    vec!["ai".to_string(), "community".to_string()]
                },
                bot_persona: (!fb.title.trim().is_empty())
                    .then(|| fb.title.clone())
                    .or_else(|| (!fb.description.trim().is_empty()).then(|| fb.description.clone())),
            });
        }
        for mb in &self.miniapps.my_bots {
            if mb.bot_user_id <= 0
                || (mb.owner_user_id != 0 && mb.owner_user_id != uid_i64)
                || !seen_bot_ids.insert(mb.bot_user_id)
            {
                continue;
            }
            let dname = if mb.display_name.trim().is_empty() {
                mb.username.clone()
            } else {
                mb.display_name.clone()
            };
            let bio = (!mb.description.trim().is_empty())
                .then(|| mb.description.clone())
                .or_else(|| (!mb.about.trim().is_empty()).then(|| mb.about.clone()));
            owned_bots.push(TelegramBotSyncInput {
                bot_user_id: mb.bot_user_id,
                username: mb.username.clone(),
                display_name: dname,
                bio,
                avatar_media_id: None,
                avatar_shape: (!mb.avatar_shape.trim().is_empty())
                    .then(|| mb.avatar_shape.clone()),
                avatar_color: (!mb.avatar_color.trim().is_empty())
                    .then(|| mb.avatar_color.clone()),
                bot_model: (!mb.preferred_model.trim().is_empty())
                    .then(|| mb.preferred_model.clone()),
                bot_capabilities: mb.commands.iter().map(|c| c.command.clone()).collect(),
                bot_persona: (!mb.ai_persona.trim().is_empty())
                    .then(|| mb.ai_persona.clone()),
            });
        }

        Some((human, owned_bots))
    }

    /// Collects all synchronously known Community actor IDs owned by the active Telegram user
    /// (`u:{uid}` plus all owned `b:{bot_user_id}` from `fabushi_bots` and `my_bots`).
    pub fn collect_shell_owned_community_actor_ids(&self) -> Vec<String> {
        let Some(uid) = self.session_sync.user_id.filter(|&id| id > 0) else {
            return Vec::new();
        };
        let uid_i64 = uid as i64;
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        let human_id = format!("u:{uid}");
        if seen.insert(human_id.clone()) {
            ids.push(human_id);
        }
        for fb in &self.miniapps.fabushi_bots {
            if fb.bot_user_id > 0 && (fb.owner_user_id == 0 || fb.owner_user_id == uid_i64) {
                let bid = format!("b:{}", fb.bot_user_id);
                if seen.insert(bid.clone()) {
                    ids.push(bid);
                }
            }
        }
        for mb in &self.miniapps.my_bots {
            if mb.bot_user_id > 0 && (mb.owner_user_id == 0 || mb.owner_user_id == uid_i64) {
                let bid = format!("b:{}", mb.bot_user_id);
                if seen.insert(bid.clone()) {
                    ids.push(bid);
                }
            }
        }
        ids
    }

    /// Returns `true` if `actor_id` belongs to the active Telegram user's Human identity
    /// or any synchronously known owned Fabushi/Telegram Bot in `AppShell`.
    pub fn is_shell_owned_community_actor(&self, actor_id: Option<&str>) -> bool {
        let Some(aid) = actor_id.map(str::trim).filter(|s| !s.is_empty()) else {
            return false;
        };
        self.collect_shell_owned_community_actor_ids()
            .iter()
            .any(|owned| owned == aid)
    }

    /// Synchronizes the logged-in Human and all owned Bots into the mounted `CommunityWorkspace`.
    pub fn sync_community_identities(&mut self, cx: &mut Context<Self>) {
        let Some(cw) = self.community_workspace.clone() else {
            return;
        };
        let owned_actor_ids = self.collect_shell_owned_community_actor_ids();
        cw.update(cx, |w, _| {
            w.set_known_owned_actor_ids(owned_actor_ids);
        });
        let Some((human, bots)) = self.build_community_identity_sync_inputs(cx) else {
            return;
        };
        cw.update(cx, |w, wcx| {
            w.sync_identities(human, bots, wcx);
        });
    }
}

impl AppShell {
    /// Handles a realtime `UpdateEvent::CommunityEvent` from WebSocket or local runtime.
    #[allow(clippy::too_many_arguments)]
    pub fn handle_community_ws_event(
        &mut self,
        event_kind: String,
        community_id: Option<String>,
        post_id: Option<String>,
        comment_id: Option<String>,
        actor_id: Option<String>,
        payload_json: String,
        timestamp_ms: i64,
        cx: &mut Context<Self>,
    ) {
        let trimmed_kind = event_kind.trim();
        if trimmed_kind.is_empty() {
            return;
        }
        // Validate JSON leniently so malformed frames never panic.
        let _ = serde_json::from_str::<serde_json::Value>(&payload_json).ok();

        let owned_actor_ids = self.collect_shell_owned_community_actor_ids();
        let is_shell_self = self.is_shell_owned_community_actor(actor_id.as_deref());

        if let Some(cw) = self.community_workspace.clone() {
            cw.update(cx, |w, wcx| {
                w.set_known_owned_actor_ids(owned_actor_ids);
                if is_shell_self {
                    if let Some(aid) = actor_id.as_deref().map(str::trim).filter(|s| !s.is_empty())
                    {
                        w.state.known_owned_actor_ids.insert(aid.to_string());
                    }
                }
                w.on_realtime_event(
                    trimmed_kind,
                    community_id.as_deref(),
                    post_id.as_deref(),
                    comment_id.as_deref(),
                    actor_id.as_deref(),
                    &payload_json,
                    timestamp_ms,
                    wcx,
                );
            });
            return;
        }

        // Workspace is not yet mounted: still keep the sidebar/NavRail unread badge live!
        let lower = trimmed_kind.to_ascii_lowercase();
        let is_notif = matches!(
            lower.as_str(),
            "mention"
                | "reply"
                | "comment_created"
                | "award"
                | "notification"
                | "notification_created"
                | "bot_replied"
                | "bot_reply"
                | "follow"
        );
        let is_self = is_shell_self;
        if is_notif && !is_self {
            let next = self
                .nav_rail
                .read(cx)
                .items
                .iter()
                .find(|i| i.destination == NavDestination::Community)
                .map(|i| i.unread_badge.saturating_add(1))
                .unwrap_or(1);
            self.set_community_unread_badge(next, cx);
        }
    }

    /// Bridges a `ClientDomainEvent::CommunityEvent` directly into `AppShell`.
    pub fn handle_community_domain_event(
        &mut self,
        ev: ClientDomainEvent,
        cx: &mut Context<Self>,
    ) {
        if let ClientDomainEvent::CommunityEvent {
            event_kind,
            community_id,
            post_id,
            comment_id,
            actor_id,
            payload_json,
            timestamp_ms,
        } = ev
        {
            self.handle_community_ws_event(
                event_kind,
                community_id,
                post_id,
                comment_id,
                actor_id,
                payload_json,
                timestamp_ms,
                cx,
            );
        }
    }

    /// Dispatches cross-app Telegram bridge actions emitted by [`CommunityWorkspace`].
    pub fn handle_community_action(
        &mut self,
        action: CommunityWorkspaceAction,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id();
        match action {
            CommunityWorkspaceAction::OpenMediaLightbox(media_ref) => {
                let raw = media_ref.trim();
                if raw.is_empty() {
                    return;
                }
                let raw_lower = raw.to_ascii_lowercase();
                let resolved_url = if raw_lower.starts_with("http://")
                    || raw_lower.starts_with("https://")
                    || raw_lower.starts_with("file://")
                    || Path::new(raw).exists()
                {
                    raw.to_string()
                } else if raw.starts_with('/') {
                    format!(
                        "{}{}",
                        self.client.http_client.base_url.trim_end_matches('/'),
                        raw
                    )
                } else {
                    self.client
                        .http_client
                        .media_stream_url(raw.trim_start_matches('/'))
                };
                let lower = resolved_url.to_ascii_lowercase();
                let path_part = lower.split(['?', '#']).next().unwrap_or("");
                let is_video = path_part.ends_with(".mp4")
                    || path_part.ends_with(".webm")
                    || path_part.ends_with(".mov")
                    || path_part.ends_with(".m4v");
                self.show_media_viewer(resolved_url, is_video, cx);
                let pal = self.theme_state.palette;
                if let Some(ref ov) = self.overlays.media_viewer_overlay {
                    ov.update(cx, |o, ocx| {
                        o.apply_theme(pal, lang);
                        ocx.notify();
                    });
                }
                cx.notify();
            }
            CommunityWorkspaceAction::OpenDmWithActor { linked_user_id } => {
                if linked_user_id <= 0 {
                    self.show_non_chat_toast(
                        tr_str(
                            lang,
                            "该社区角色未关联 Telegram 账号",
                            "Actor has no linked Telegram account",
                        ),
                        2400,
                        cx,
                    );
                    return;
                }
                let uid = linked_user_id as u64;
                if self.session_sync.user_id == Some(uid) {
                    self.show_non_chat_toast(
                        tr_str(lang, "无法向自己发起私聊", "Cannot open a DM with yourself"),
                        2400,
                        cx,
                    );
                    return;
                }

                let existing_cid = self
                    .session_sync
                    .chat_peer_user_ids
                    .iter()
                    .find(|(_, pid)| **pid == uid)
                    .map(|(cid, _)| *cid)
                    .or_else(|| {
                        self.session_sync
                            .rich_chats_by_id
                            .iter()
                            .find(|(_, rc)| {
                                !rc.is_group && !rc.is_channel && rc.peer_user_id == Some(uid)
                            })
                            .map(|(&cid, _)| cid)
                    })
                    .or_else(|| {
                        self.session_sync
                            .contacts_list
                            .iter()
                            .find(|c| c.user_id == uid)
                            .and_then(|c| c.chat_id)
                            .filter(|&cid| cid > 0)
                            .map(|cid| cid as u64)
                    })
                    .or_else(|| {
                        self.miniapps
                            .fabushi_bots
                            .iter()
                            .find(|b| b.bot_user_id == linked_user_id)
                            .and_then(|b| b.companion_chat_id)
                            .filter(|&cid| cid > 0)
                            .map(|cid| cid as u64)
                    });

                let is_bot = self
                    .miniapps
                    .fabushi_bots
                    .iter()
                    .any(|b| b.bot_user_id == linked_user_id)
                    || self
                        .miniapps
                        .my_bots
                        .iter()
                        .any(|b| b.bot_user_id == linked_user_id);

                let title_hint = self
                    .miniapps
                    .fabushi_bots
                    .iter()
                    .find(|b| b.bot_user_id == linked_user_id)
                    .map(|b| b.display_name.clone())
                    .or_else(|| {
                        self.session_sync
                            .contacts_list
                            .iter()
                            .find(|c| c.user_id == uid)
                            .map(|c| {
                                let full = format!(
                                    "{} {}",
                                    c.first_name,
                                    c.last_name.as_deref().unwrap_or("")
                                );
                                let trimmed = full.trim();
                                if trimmed.is_empty() {
                                    c.username.clone().unwrap_or_default()
                                } else {
                                    trimmed.to_string()
                                }
                            })
                    })
                    .filter(|s| !s.trim().is_empty());

                self.leave_community_panel_if_active(cx);
                if let Some(cid) = existing_cid {
                    self.open_or_select_chat(cid, title_hint, is_bot, cx);
                    self.sync_apple_sidebar_to_active_panel(cx);
                } else {
                    self.open_or_select_chat(uid, title_hint.clone(), is_bot, cx);
                    self.sync_apple_sidebar_to_active_panel(cx);
                    if let Some(fence) = self.capture_client_fence() {
                        let req = CreateChatRequest {
                            title: title_hint.clone().unwrap_or_else(|| format!("User {uid}")),
                            chat_type: Some(serde_json::json!(if is_bot { 4 } else { 0 })),
                            peer_user_id: Some(uid),
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
                                let cid = created.chat_id as u64;
                                if cid > 0 {
                                    let _ = weak.update(app, |shell, cx| {
                                        if fence.is_valid(shell) {
                                            shell.session_sync.chat_peer_user_ids.insert(cid, uid);
                                            shell.open_or_select_chat(cid, title_hint, is_bot, cx);
                                            shell.sync_apple_sidebar_to_active_panel(cx);
                                        }
                                    });
                                }
                            }
                        })
                        .detach();
                    }
                }
            }
            CommunityWorkspaceAction::SharePostToChat { post_id, summary } => {
                self.open_modal_dialog(
                    tr_str(lang, "分享帖子到会话", "Share Post to Chat"),
                    ModalDialogKind::ForwardMessages {
                        message_ids: Vec::new(),
                        preview_snippet: summary.clone(),
                    },
                    420.0,
                    460.0,
                    cx,
                );
                self.community_pending_share = Some((post_id, summary));
            }
            CommunityWorkspaceAction::ShowToast { text, is_error } => {
                let msg = if is_error && !Self::is_error_toast_text(&text) {
                    if lang.starts_with("zh") {
                        format!("错误: {text}")
                    } else {
                        format!("Error: {text}")
                    }
                } else {
                    text
                };
                self.show_non_chat_toast(msg, if is_error { 3500 } else { 2400 }, cx);
            }
            CommunityWorkspaceAction::UnreadCountChanged(count) => {
                self.set_community_unread_badge(count, cx);
            }
            CommunityWorkspaceAction::OpenExternalUrl(url) => {
                let trimmed = url.trim();
                if !trimmed.is_empty() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(trimmed.to_string()));
                    self.show_non_chat_toast(
                        if lang.starts_with("zh") {
                            format!("已复制链接: {trimmed}")
                        } else {
                            format!("Link copied: {trimmed}")
                        },
                        2200,
                        cx,
                    );
                }
            }
        }
    }
}
