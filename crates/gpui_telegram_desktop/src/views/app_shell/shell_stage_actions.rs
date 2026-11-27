//! Stage action dispatch, chat inspector synchronization, bot peer detection, and theme propagation.

use std::collections::{HashMap, HashSet};
use std::mem;

use gpui::{AppContext, AsyncApp, ClipboardItem, Context, WeakEntity};
use gpui_telegram_client::net::hermes_client::{
    HermesApprovalDecisionRequest, HermesCommandRequest, HermesRichChatDto,
};
use gpui_telegram_client::net::http_client::ChatMemberItem;
use gpui_telegram_core::types::ChatId;
use gpui_telegram_proto::contracts::auth::ContactDto;
use gpui_telegram_proto::contracts::{CreateChatRequest, DraftDto};
use gpui_telegram_proto::miniapp_contracts::{
    parse_miniapp_uri, FabushiBotSummaryDto, MiniAppLaunchMode,
};

use crate::app::session::send_as_peers::{SendAsKey, SendAsPeer};
use crate::components::tool_permission_dock::{
    PendingToolApproval, ToolApprovalDecision, ToolApprovalPolicyTier,
};
use crate::i18n::tr_str;
use crate::theme::{TdesktopThemeMode, TdesktopThemePalette};
use crate::views::app_shell::shell_actions::{
    build_chat_profile_facts, map_channel_stats_to_chart, map_chat_members_to_inspector,
    map_stage_media_to_gallery, peer_avatar_color, resolve_active_target_chat_id,
};
use crate::views::app_shell::shell_theme::{
    compute_shell_palette, propagate_theme_and_locale, tdesktop_mode_to_app_theme, ShellThemeTargets,
};
use crate::views::app_shell::shell_truth::{
    build_cached_history_models, derive_initials, select_authoritative_dialog, ClientCallFence,
};
use crate::views::boxes::modal_types::ModalDialogKind;
use crate::views::composer::field_autocomplete::MentionRow;
use crate::views::conversation_stage::{PinnedBannerEntry, StageAction};
use crate::views::iv::IvViewer;
use crate::views::media_viewer::MediaViewerItem;
use crate::views::nav_rail::NavDestination;
use crate::views::shell::{ActiveWorkspacePanel, AppShell};
use crate::views::MediaViewer;

fn contact_display_name(c: &ContactDto) -> String {
    let mut full = c.first_name.trim().to_string();
    if let Some(last) = c.last_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if !full.is_empty() {
            full.push(' ');
        }
        full.push_str(last);
    }
    if full.is_empty() {
        c.username
            .as_deref()
            .map(|u| u.trim().trim_start_matches('@').to_string())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| format!("User {}", c.user_id))
    } else {
        full
    }
}

/// Builds deduplicated `@mention` autocomplete candidates + role hints for the active chat.
///
/// - **Groups**: prepends `@everyone` (`"所有人 / 全部 Bot"`), then includes all group members
///   enriched with Fabushi bot metadata (`is_bot = true`, `"AI Bot"`, real `@username`) and contacts.
/// - **DMs / Global**: includes active peer (if any) followed by all available Fabushi bots.
pub fn build_mention_candidates(
    is_group: bool,
    members: &[ChatMemberItem],
    contacts: &[ContactDto],
    fabushi_bots: &[FabushiBotSummaryDto],
    active_dm_peer: Option<(u64, String, String, bool)>,
) -> Vec<(MentionRow, Option<String>)> {
    let mut out: Vec<(MentionRow, Option<String>)> = Vec::new();
    let mut seen_ids: HashSet<u64> = HashSet::new();
    let mut seen_usernames: HashSet<String> = HashSet::new();

    if is_group {
        seen_usernames.insert("everyone".to_string());
        out.push((
            MentionRow::new(0, "everyone", "所有人 / 全部 Bot", false),
            Some("所有人".to_string()),
        ));

        for m in members {
            if m.user_id == 0 || !seen_ids.insert(m.user_id) {
                continue;
            }
            if let Some(fb) = fabushi_bots
                .iter()
                .find(|b| b.bot_user_id > 0 && b.bot_user_id as u64 == m.user_id)
            {
                let uname = fb.username.trim().trim_start_matches('@').to_string();
                let effective_un = if uname.is_empty() {
                    fb.agent_id.trim().to_string()
                } else {
                    uname
                };
                let name = if fb.display_name.trim().is_empty() {
                    effective_un.clone()
                } else {
                    fb.display_name.trim().to_string()
                };
                if !effective_un.is_empty() {
                    seen_usernames.insert(effective_un.to_ascii_lowercase());
                }
                out.push((
                    MentionRow::new(m.user_id, effective_un, name, true),
                    Some("AI Bot".to_string()),
                ));
                continue;
            }

            let contact = contacts.iter().find(|c| c.user_id == m.user_id);
            let is_bot = contact.is_some_and(|c| c.is_bot);
            let name = contact
                .map(contact_display_name)
                .unwrap_or_else(|| format!("User {}", m.user_id));
            let uname = contact
                .and_then(|c| c.username.as_deref())
                .map(|u| u.trim().trim_start_matches('@').to_string())
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| format!("user_{}", m.user_id));
            let role_hint = if is_bot {
                "AI Bot".to_string()
            } else {
                match m.role.trim().to_ascii_lowercase().as_str() {
                    "owner" | "creator" => "群主".to_string(),
                    "admin" | "administrator" => "管理员".to_string(),
                    _ => "成员".to_string(),
                }
            };
            seen_usernames.insert(uname.to_ascii_lowercase());
            out.push((MentionRow::new(m.user_id, uname, name, is_bot), Some(role_hint)));
        }
    } else if let Some((uid, username, display_name, is_bot)) = active_dm_peer {
        let clean_un = username.trim().trim_start_matches('@').to_string();
        if uid > 0 {
            seen_ids.insert(uid);
        }
        if !clean_un.is_empty() {
            seen_usernames.insert(clean_un.to_ascii_lowercase());
        }
        if !clean_un.is_empty() || !display_name.trim().is_empty() {
            let role = if is_bot { "AI Bot" } else { "成员" };
            out.push((
                MentionRow::new(uid, clean_un, display_name, is_bot),
                Some(role.to_string()),
            ));
        }
    }

    // For DMs/global (or groups where member roster hasn't resolved yet), also expose available Fabushi bots.
    if !is_group || members.is_empty() {
        for fb in fabushi_bots {
            let uid = fb.bot_user_id.max(0) as u64;
            let uname = fb.username.trim().trim_start_matches('@').to_string();
            let effective_un = if uname.is_empty() {
                fb.agent_id.trim().to_string()
            } else {
                uname
            };
            let key = effective_un.to_ascii_lowercase();
            if effective_un.is_empty() || seen_usernames.contains(&key) {
                continue;
            }
            if uid > 0 && seen_ids.contains(&uid) {
                continue;
            }
            if uid > 0 {
                seen_ids.insert(uid);
            }
            seen_usernames.insert(key);
            let name = if fb.display_name.trim().is_empty() {
                effective_un.clone()
            } else {
                fb.display_name.trim().to_string()
            };
            out.push((
                MentionRow::new(uid, effective_un, name, true),
                Some("AI Bot".to_string()),
            ));
        }
    }

    out
}

fn normalized_username(raw: Option<&str>) -> String {
    let trimmed = raw.unwrap_or("").trim();
    if trimmed.is_empty() {
        String::new()
    } else if trimmed.starts_with('@') {
        trimmed.to_string()
    } else {
        format!("@{trimmed}")
    }
}

fn enrich_facts_with_rich_chat(
    mut facts: Vec<(String, String)>,
    rc: &HermesRichChatDto,
    is_fabushi_bot: bool,
    lang: &str,
) -> Vec<(String, String)> {
    if rc.is_bot {
        if is_fabushi_bot {
            facts.push((
                tr_str(lang, "智能体引擎", "Agent Engine").to_string(),
                "Fabushi AI · Talk Mode & Tools".to_string(),
            ));
        }
    } else if rc.members_count > 0 && !facts.iter().any(|(k, _)| k == "Members" || k == "成员") {
        facts.push((tr_str(lang, "成员", "Members").to_string(), rc.members_count.to_string()));
    }
    facts
}

fn attach_media_viewer_overlay(
    shell: &mut AppShell,
    mut viewer: MediaViewer,
    lang: &'static str,
    cx: &mut Context<AppShell>,
) {
    shell.occlude_browser_for_overlay(cx);
    shell.prepare_for_media_or_call_overlay_open(cx);
    viewer.apply_theme(shell.theme_state.palette, lang);
    let overlay = cx.new(|_| viewer);
    cx.observe(
        &overlay,
        |shell: &mut AppShell, ov: gpui::Entity<MediaViewer>, cx: &mut Context<AppShell>| {
            if ov.read(cx).should_close {
                shell.close_media_viewer(cx);
                cx.notify();
            }
        },
    )
    .detach();
    shell.overlays.media_viewer_overlay = Some(overlay);
    shell.reconcile_chat_scoped_toasts(cx);
}

fn toggle_chat_dialog_flag(
    shell: &mut AppShell,
    cid: u64,
    is_pin: bool,
    lang: &'static str,
    cx: &mut Context<AppShell>,
) {
    shell.sidebar.update(cx, |sb, sb_cx| {
        if is_pin { sb.toggle_pin_dialog(cid); } else { sb.toggle_mute_dialog(cid); }
        sb_cx.notify();
    });
    let flag = shell
        .sidebar
        .read(cx)
        .dialogs
        .iter()
        .find(|d| d.id == cid)
        .map(|d| if is_pin { d.is_pinned } else { d.is_muted })
        .unwrap_or(true);
    shell.stage.update(cx, |st, _| {
        if is_pin { st.is_pinned_chat = flag; } else { st.is_muted_chat = flag; }
    });
    if let Some(rc) = shell.session_sync.rich_chats_by_id.get_mut(&cid) {
        if is_pin { rc.is_pinned = flag; } else { rc.is_muted = flag; }
    }
    let toast_msg = match (is_pin, flag) {
        (true, true) => tr_str(lang, "已置顶该对话", "Chat pinned"),
        (true, false) => tr_str(lang, "已取消置顶", "Chat unpinned"),
        (false, true) => tr_str(lang, "已开启消息免打扰", "Notifications muted"),
        (false, false) => tr_str(lang, "已恢复消息通知", "Notifications unmuted"),
    };
    if shell.active_panel == ActiveWorkspacePanel::Chats
        && !shell.sidebar.read(cx).is_plus_menu_open
        && shell.stage.read(cx).active_chat_id == Some(cid)
    {
        shell.show_chat_toast(cid, toast_msg, 2000, cx);
    }
    if let Some(fence) = shell.capture_client_fence() {
        cx.spawn(async move |weak: WeakEntity<AppShell>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            if is_pin {
                let _ = fence
                    .client
                    .set_chat_pinned_with_token(
                        &fence.token,
                        fence.client_epoch,
                        cid as i64,
                        flag,
                    )
                    .await;
            } else {
                let _ = fence
                    .client
                    .set_chat_muted_with_token(
                        &fence.token,
                        fence.client_epoch,
                        cid as i64,
                        flag,
                        None,
                    )
                    .await;
            }
        })
        .detach();
    }
}

impl AppShell {
    /// Computes the active palette and propagates theme and locale to all mounted views and overlays.
    pub fn sync_theme_and_locale_to_views(&mut self, cx: &mut Context<Self>) {
        let lang = self.active_lang_id();
        let (mode, pal) = compute_shell_palette(
            &self.preferences,
            Some(self.theme_state.theme_mode),
            self.theme_state.accent_color,
            self.theme_state.bubble_radius_px,
            self.theme_state.wallpaper_id.as_deref(),
        );
        self.theme_state.theme_mode = mode;
        self.theme_state.palette = pal;
        let targets = ShellThemeTargets {
            nav_rail: &self.nav_rail,
            apple_sidebar: &self.apple_sidebar,
            content_hub: &self.content_hub,
            main_menu_drawer: &self.main_menu_drawer,
            sidebar: &self.sidebar,
            stage: &self.stage,
            composer: &self.composer,
            inspector: &self.inspector,
            modal_container: &self.modal_container,
            tasks_workspace: &self.tasks_workspace,
            browser_workspace: self.browser_workspace.as_ref(),
            openbot_workspace: self.openbot_workspace.as_ref(),
            community_workspace: self.community_workspace.as_ref(),
            orca_bots_workspace: self.orca_bots_workspace.as_ref(),
            orca_ide_workspace: self.orca_ide_workspace.as_ref(),
            orca_jump_palette: self.orca_jump_palette.as_ref(),
            talk_mode_overlay: self.rtc_talk.talk_mode_overlay.as_ref(),
            computer_overlay: self.overlays.computer_overlay.as_ref(),
            media_viewer_overlay: self.overlays.media_viewer_overlay.as_ref(),
            iv_viewer_overlay: self.overlays.iv_viewer_overlay.as_ref(),
            auth_wizard: &self.auth_wizard,
            settings_router: &self.settings_router,
            miniapp_overlay: self.miniapps.miniapp_overlay.as_ref(),
            create_bot_box: self.miniapps.create_bot_box.as_ref(),
        };
        propagate_theme_and_locale(targets, &pal, lang, cx);
        // Anti-spoofing invariant: propagate ONLY locale into top-Z SecuritySurfaceModal, NEVER theme palette!
        if let Some(ref sec_modal) = self.orca_security_modal {
            sec_modal.update(cx, |m, mcx| m.set_locale(lang, mcx));
        }
        if self.theme_state.palette.is_apple_style()
            && self.active_panel != ActiveWorkspacePanel::Chats
            && self.sidebar.read(cx).is_plus_menu_open
        {
            self.sidebar.update(cx, |sb, scx| {
                sb.cancel_compose_mode();
                scx.notify();
            });
            if self.composer.read(cx).is_compose_isolated
                && !self.is_compose_or_creation_modal_active(cx)
            {
                self.exit_compose_or_modal_isolation(true, cx);
            } else {
                self.sync_compose_state_to_composer(true, cx);
            }
            if self.active_panel == ActiveWorkspacePanel::Browser {
                let occluded = self.has_browser_visual_occlusion_overlay(cx);
                if let Some(ref bw) = self.browser_workspace {
                    bw.update(cx, |w, wcx| {
                        w.set_occluded(occluded, wcx);
                    });
                }
            }
        }
    }

    /// Updates active theme mode, accent color, and wallpaper selection and persists preferences.
    pub fn apply_theme_selection(
        &mut self,
        mode: TdesktopThemeMode,
        accent: Option<u32>,
        wallpaper: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let was_apple = self.theme_state.palette.is_apple_style();
        self.theme_state.theme_mode = mode;
        if let Some(a) = accent {
            self.theme_state.accent_color = Some(a);
        }
        if let Some(w) = wallpaper {
            self.theme_state.wallpaper_id = Some(w);
        }
        self.update_preferences(cx, |p| {
            let mapped = tdesktop_mode_to_app_theme(mode);
            p.theme_mode = mapped;
            p.appearance.theme_mode = mapped;
            p.appearance.chat_preset = mode.as_name().to_string();
        });
        let now_apple = self.theme_state.palette.is_apple_style();
        if was_apple && !now_apple && self.sidebar.read(cx).is_plus_menu_open {
            if !self.has_blocking_modal_or_fullscreen_overlay_for_shortcuts(cx) {
                self.clear_all_ime_preedits(cx);
            }
            self.sidebar.update(cx, |sb, sb_cx| {
                sb.cancel_compose_mode();
                sb_cx.notify();
            });
            if self.composer.read(cx).is_compose_isolated
                && !self.is_compose_or_creation_modal_active(cx)
            {
                self.exit_compose_or_modal_isolation(true, cx);
            } else {
                self.sync_compose_state_to_composer(true, cx);
            }
        }
    }

    /// Flushes any pending unsaved composer draft for `chat_id` immediately (e.g. on chat switch).
    pub fn flush_pending_draft_for_chat(&mut self, chat_id: u64, cx: &mut Context<Self>) {
        if !self.is_authenticated || chat_id == 0 {
            return;
        }
        let Some(token) = self.session_sync.session_token.clone() else {
            return;
        };
        let text = self.composer.read(cx).text().to_string();
        let trimmed_empty = text.trim().is_empty();
        if trimmed_empty && self.session_sync.has_unmodified_in_flight_send_draft(chat_id) {
            return;
        }
        let already_saved = self
            .session_sync
            .last_saved_draft_by_chat
            .get(&chat_id)
            .is_some_and(|prev| prev == &text)
            && self
                .session_sync
                .last_draft_text_by_chat
                .get(&chat_id)
                .is_some_and(|prev| prev == &text);
        if already_saved {
            return;
        }
        let flushed_gen = self
            .session_sync
            .bump_draft_op_gen_and_clear_in_flight(chat_id);
        self.session_sync
            .last_draft_text_by_chat
            .insert(chat_id, text.clone());
        self.session_sync
            .last_saved_draft_by_chat
            .insert(chat_id, text.clone());
        self.session_sync.draft_debounce_gen =
            self.session_sync.draft_debounce_gen.wrapping_add(1);
        let draft_dto = DraftDto {
            chat_id: chat_id as i64,
            text: text.clone(),
            entities: Vec::new(),
            reply_to_msg_id: None,
            topic_id: None,
            date_ms: chrono::Utc::now().timestamp_millis(),
        };
        self.client.local_store.set_draft(
            ChatId(chat_id as i64),
            (!trimmed_empty).then(|| draft_dto.clone()),
        );
        self.sidebar
            .update(cx, |sb, _| sb.set_draft(chat_id, text.clone()));
        if let Some(fence) = self.capture_client_fence() {
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                if !fence.preflight_ui(&weak, app).await {
                    return;
                }
                let _ = fence
                    .client
                    .save_draft_with_token(
                        &fence.token,
                        fence.client_epoch,
                        chat_id as i64,
                        &draft_dto,
                    )
                    .await;
                if trimmed_empty {
                    let _ = weak.update(app, |shell, _| {
                        if fence.is_valid(shell)
                            && shell
                                .session_sync
                                .is_current_draft_op_gen(chat_id, flushed_gen)
                        {
                            shell
                                .client
                                .local_store
                                .set_draft(ChatId(chat_id as i64), None);
                        } else if fence.is_valid(shell)
                            && !shell.session_sync.has_unmodified_in_flight_send_draft(chat_id)
                        {
                            let cur_text = shell
                                .session_sync
                                .last_draft_text_by_chat
                                .get(&chat_id)
                                .cloned()
                                .unwrap_or_default();
                            let dto = (!cur_text.trim().is_empty()).then(|| DraftDto {
                                chat_id: chat_id as i64,
                                text: cur_text,
                                entities: Vec::new(),
                                reply_to_msg_id: None,
                                topic_id: None,
                                date_ms: chrono::Utc::now().timestamp_millis(),
                            });
                            shell
                                .client
                                .local_store
                                .set_draft(ChatId(chat_id as i64), dto);
                        }
                    });
                }
            })
            .detach();
        }
    }

    /// Handles composer text changes with local draft caching and 450ms server save debouncing.
    pub fn handle_composer_draft_change(&mut self, cx: &mut Context<Self>) {
        if !self.is_authenticated {
            return;
        }
        let Some(chat_id) = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        ) else {
            return;
        };
        let Some(token) = self.session_sync.session_token.clone() else {
            return;
        };
        let text = self.composer.read(cx).text().to_string();
        let prev_draft = self.session_sync.last_draft_text_by_chat.get(&chat_id);
        if prev_draft.is_some_and(|prev| prev == &text) {
            return;
        }
        if prev_draft.is_none() && text.trim().is_empty() {
            self.session_sync
                .last_draft_text_by_chat
                .insert(chat_id, text);
            return;
        }
        let op_gen = self
            .session_sync
            .bump_draft_op_gen_and_clear_in_flight(chat_id);
        self.session_sync
            .last_draft_text_by_chat
            .insert(chat_id, text.clone());
        self.sidebar
            .update(cx, |sb, _| sb.set_draft(chat_id, text.clone()));
        let draft_dto = DraftDto {
            chat_id: chat_id as i64,
            text: text.clone(),
            entities: Vec::new(),
            reply_to_msg_id: None,
            topic_id: None,
            date_ms: chrono::Utc::now().timestamp_millis(),
        };
        self.client.local_store.set_draft(
            ChatId(chat_id as i64),
            (!text.trim().is_empty()).then(|| draft_dto.clone()),
        );

        self.session_sync.draft_debounce_gen =
            self.session_sync.draft_debounce_gen.wrapping_add(1);
        let debounce_gen = self.session_sync.draft_debounce_gen;
        let Some(fence) = self.capture_client_fence() else {
            return;
        };
        let _ = token;
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            app.background_executor()
                .timer(std::time::Duration::from_millis(450))
                .await;
            let should_save = weak
                .read_with(app, |shell, _| {
                    fence.is_valid(shell)
                        && shell.session_sync.draft_debounce_gen == debounce_gen
                        && shell.session_sync.is_current_draft_op_gen(chat_id, op_gen)
                        && shell
                            .session_sync
                            .last_saved_draft_by_chat
                            .get(&chat_id)
                            != Some(&text)
                })
                .unwrap_or(false);
            if !should_save {
                return;
            }
            if fence
                .client
                .save_draft_with_token(
                    &fence.token,
                    fence.client_epoch,
                    chat_id as i64,
                    &draft_dto,
                )
                .await
                .is_ok()
            {
                let _ = weak.update(app, |shell, _| {
                    if !fence.is_valid(shell) {
                        return;
                    }
                    if shell.session_sync.is_current_draft_op_gen(chat_id, op_gen) {
                        if text.trim().is_empty() {
                            shell
                                .client
                                .local_store
                                .set_draft(ChatId(chat_id as i64), None);
                        }
                        shell
                            .session_sync
                            .last_saved_draft_by_chat
                            .insert(chat_id, text);
                    } else if !shell.session_sync.has_unmodified_in_flight_send_draft(chat_id) {
                        let cur_text = shell
                            .session_sync
                            .last_draft_text_by_chat
                            .get(&chat_id)
                            .cloned()
                            .unwrap_or_default();
                        let dto = (!cur_text.trim().is_empty()).then(|| DraftDto {
                            chat_id: chat_id as i64,
                            text: cur_text,
                            entities: Vec::new(),
                            reply_to_msg_id: None,
                            topic_id: None,
                            date_ms: chrono::Utc::now().timestamp_millis(),
                        });
                        shell
                            .client
                            .local_store
                            .set_draft(ChatId(chat_id as i64), dto);
                    }
                });
            }
        })
        .detach();
    }

    /// Synchronizes live `@mention` autocomplete candidates, group composite avatar tiles, and
    /// per-bot hue mappings for `chat_id`.
    pub fn sync_composer_mention_candidates(
        &mut self,
        chat_id: u64,
        members: Option<&[ChatMemberItem]>,
        cx: &mut Context<Self>,
    ) {
        let rc_opt = self.session_sync.rich_chats_by_id.get(&chat_id).cloned();
        let is_group = rc_opt
            .as_ref()
            .map(|rc| rc.is_group || rc.is_channel || matches!(rc.chat_type, 1 | 2 | 3))
            .unwrap_or_else(|| self.stage.read(cx).is_group_chat);

        let fallback_members: Vec<ChatMemberItem> = if members.is_none() && is_group {
            self.inspector
                .read(cx)
                .members
                .members
                .iter()
                .map(|m| ChatMemberItem {
                    user_id: m.id,
                    role: m.role.clone(),
                    admin_title: None,
                    admin_rights: 0,
                    permissions_bitmask: 0,
                    joined_at_ms: 0,
                })
                .collect()
        } else {
            Vec::new()
        };
        let effective_members = members.unwrap_or(&fallback_members);

        let active_dm_peer = if !is_group {
            let peer_uid = rc_opt
                .as_ref()
                .and_then(|rc| rc.peer_user_id)
                .or_else(|| self.session_sync.chat_peer_user_ids.get(&chat_id).copied())
                .unwrap_or(0);
            let fb_match = self
                .miniapps
                .fabushi_bots
                .iter()
                .find(|b| peer_uid > 0 && b.bot_user_id > 0 && b.bot_user_id as u64 == peer_uid);
            if let Some(fb) = fb_match {
                Some((
                    peer_uid,
                    fb.username.clone(),
                    fb.display_name.clone(),
                    true,
                ))
            } else if let Some(ref rc) = rc_opt {
                let uname = rc
                    .peer_username
                    .clone()
                    .or_else(|| rc.username.clone())
                    .unwrap_or_default();
                let name = rc
                    .peer_display_name
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| rc.title.clone());
                Some((peer_uid, uname, name, rc.is_bot))
            } else {
                None
            }
        } else {
            None
        };

        let candidates = build_mention_candidates(
            is_group,
            effective_members,
            &self.session_sync.contacts_list,
            &self.miniapps.fabushi_bots,
            active_dm_peer,
        );
        self.composer.update(cx, |c, c_cx| {
            c.set_mention_candidates_with_roles(candidates);
            c_cx.notify();
        });

        let bot_hues: HashMap<u64, String> = self
            .miniapps
            .fabushi_bots
            .iter()
            .filter(|b| b.bot_user_id > 0)
            .map(|b| (b.bot_user_id as u64, b.avatar_color.clone()))
            .collect();
        let is_dark = self.theme_state.palette.mode.is_dark();
        let member_tiles: Vec<(String, u32)> = if is_group && !effective_members.is_empty() {
            effective_members
                .iter()
                .take(3)
                .map(|m| {
                    if let Some(fb) = self
                        .miniapps
                        .fabushi_bots
                        .iter()
                        .find(|b| b.bot_user_id > 0 && b.bot_user_id as u64 == m.user_id)
                    {
                        let init = derive_initials(&fb.display_name);
                        let hue = if TdesktopThemePalette::is_tinted_bot_hue(&fb.avatar_color) {
                            fb.avatar_color.as_str()
                        } else {
                            TdesktopThemePalette::bot_hue_name_for_id(m.user_id)
                        };
                        let (fill, _, _) = TdesktopThemePalette::bot_hue_colors(hue, is_dark);
                        (if init.is_empty() { "B".to_string() } else { init }, fill)
                    } else if let Some(c) = self
                        .session_sync
                        .contacts_list
                        .iter()
                        .find(|c| c.user_id == m.user_id)
                    {
                        let name = contact_display_name(c);
                        let init = derive_initials(&name);
                        let (fill, _, _) = TdesktopThemePalette::bot_hue_colors(
                            TdesktopThemePalette::bot_hue_name_for_id(m.user_id),
                            is_dark,
                        );
                        (if init.is_empty() { "U".to_string() } else { init }, fill)
                    } else {
                        let (fill, _, _) = TdesktopThemePalette::bot_hue_colors(
                            TdesktopThemePalette::bot_hue_name_for_id(m.user_id),
                            is_dark,
                        );
                        ("M".to_string(), fill)
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        self.stage.update(cx, |st, st_cx| {
            st.set_bot_hues_map(bot_hues);
            if st.active_chat_id == Some(chat_id) && !member_tiles.is_empty() {
                st.set_group_member_tiles(member_tiles);
            }
            st_cx.notify();
        });
    }

    /// Navigates straight to a group message sender's 1:1 DM (reusing an existing DM or creating one).
    pub fn open_sender_dm_by_user_id(
        &mut self,
        user_id: u64,
        sender_name: String,
        cx: &mut Context<Self>,
    ) {
        if user_id == 0 {
            return;
        }
        if self.session_sync.user_id == Some(user_id) {
            return;
        }
        // 1. Check existing 1:1 DM in rich_chats_by_id / chat_peer_user_ids / contacts_list
        let existing_dm_chat_id = self
            .session_sync
            .rich_chats_by_id
            .iter()
            .find(|(_, rc)| {
                !rc.is_group
                    && !rc.is_channel
                    && !matches!(rc.chat_type, 1 | 2 | 3)
                    && rc.peer_user_id == Some(user_id)
            })
            .map(|(&cid, _)| cid)
            .or_else(|| {
                self.session_sync
                    .contacts_list
                    .iter()
                    .find(|c| c.user_id == user_id)
                    .and_then(|c| c.chat_id)
                    .filter(|&cid| cid > 0)
                    .map(|cid| cid as u64)
            });

        if let Some(dm_cid) = existing_dm_chat_id {
            self.sidebar.update(cx, |sb, sb_cx| {
                sb.select_dialog(dm_cid);
                sb_cx.notify();
            });
            self.sync_selected_chat_inspector_and_stage(dm_cid, cx);
            return;
        }

        let is_bot = self
            .miniapps
            .fabushi_bots
            .iter()
            .any(|b| b.bot_user_id > 0 && b.bot_user_id as u64 == user_id)
            || self
                .session_sync
                .contacts_list
                .iter()
                .any(|c| c.user_id == user_id && c.is_bot);
        let title = if sender_name.trim().is_empty() {
            format!("User {user_id}")
        } else {
            sender_name
        };
        let req = CreateChatRequest {
            title,
            chat_type: Some(serde_json::json!(if is_bot { 4 } else { 0 })),
            peer_user_id: Some(user_id),
            ..Default::default()
        };
        let origin_chat = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        );
        let captured_panel = self.active_panel;
        let captured_plus_open = self.sidebar.read(cx).is_plus_menu_open;
        let Some(call_fence) = self.capture_client_fence() else {
            return;
        };
        let lang_owned = self.active_lang_id().to_string();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !call_fence.preflight_ui(&weak, app).await {
                return;
            }
            let res = call_fence
                .client
                .create_chat_with_token(&call_fence.token, call_fence.client_epoch, &req)
                .await;
            let _ = weak.update(app, |shell, cx| {
                if !call_fence.is_valid(shell) {
                    return;
                }
                match res {
                    Ok(created) if created.chat_id > 0 => {
                        let cid = created.chat_id as u64;
                        let init = derive_initials(&created.title);
                        let color = peer_avatar_color(user_id);
                        shell.sidebar.update(cx, |sb, sb_cx| {
                            if !sb.dialogs.iter().any(|d| d.id == cid) {
                                sb.dialogs.insert(
                                    0,
                                    crate::views::chat_sidebar::DialogItemModel {
                                        id: cid,
                                        peer_name: created.title.clone(),
                                        avatar_initials: if init.is_empty() {
                                            "U".to_string()
                                        } else {
                                            init
                                        },
                                        avatar_color: color,
                                        last_message: String::new(),
                                        timestamp: String::new(),
                                        unread_count: 0,
                                        is_muted: false,
                                        is_pinned: false,
                                        is_online: is_bot,
                                        typing_indicator: None,
                                        delivery_status:
                                            crate::views::chat_sidebar::DialogDeliveryStatus::Read,
                                        folder: if is_bot {
                                            "Bots".to_string()
                                        } else {
                                            "Personal".to_string()
                                        },
                                    },
                                );
                            }
                            if is_bot {
                                sb.bot_ids.insert(cid);
                            }
                            sb.select_dialog(cid);
                            sb_cx.notify();
                        });
                        shell.sync_selected_chat_inspector_and_stage(cid, cx);
                        shell.refresh_chats(cx);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let msg = format!(
                            "{}: {}",
                            tr_str(&lang_owned, "打开私聊失败", "Failed to open DM"),
                            e
                        );
                        let cur_active = resolve_active_target_chat_id(
                            shell.stage.read(cx).active_chat_id,
                            shell.sidebar.read(cx).active_chat_id,
                        );
                        if let Some(cid) = origin_chat {
                            if shell.active_panel == ActiveWorkspacePanel::Chats
                                && !shell.sidebar.read(cx).is_plus_menu_open
                                && shell.stage.read(cx).active_chat_id == Some(cid)
                            {
                                shell.show_chat_toast(cid, msg, 2400, cx);
                            }
                        } else if shell.active_panel == captured_panel
                            && shell.sidebar.read(cx).is_plus_menu_open == captured_plus_open
                            && cur_active.is_none()
                        {
                            shell.show_non_chat_toast(msg, 2400, cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens the Leave Group / Delete Chat confirmation modal for the currently active conversation.
    pub(crate) fn open_leave_or_delete_chat_modal(&mut self, cx: &mut Context<Self>) {
        let lang = self.active_lang_id();
        let active_chat = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        );
        let st_ref = self.stage.read(cx);
        let peer_name = st_ref.chat_title.clone();
        let is_group = st_ref.is_group_chat
            || st_ref.chat_subtitle.contains("member")
            || st_ref.chat_subtitle.contains("subscriber")
            || st_ref.chat_subtitle.contains("成员")
            || st_ref.chat_subtitle == "group"
            || self.inspector.read(cx).members.is_group_or_channel
            || active_chat
                .and_then(|cid| self.session_sync.rich_chats_by_id.get(&cid))
                .is_some_and(|rc| rc.is_group || rc.is_channel || matches!(rc.chat_type, 1 | 2 | 3));
        let header = if is_group {
            tr_str(lang, "退出群组", "Leave Group")
        } else {
            tr_str(lang, "删除对话", "Delete Chat")
        };
        self.open_modal_dialog(
            header,
            ModalDialogKind::LeaveOrDeleteChat { peer_name, is_group },
            390.0,
            230.0,
            cx,
        );
    }

    /// Synchronizes sidebar/stage/inspector selection and asynchronously fetches authoritative chat details.
    pub fn sync_selected_chat_inspector_and_stage(&mut self, chat_id: u64, cx: &mut Context<Self>) {
        let prev_chat_id = self.stage.read(cx).active_chat_id;
        let is_compose_isolated = self.composer.read(cx).is_compose_isolated;
        if !is_compose_isolated {
            if let Some(prev) = prev_chat_id.filter(|&p| p != chat_id) {
                self.flush_pending_draft_for_chat(prev, cx);
            }
        }
        if prev_chat_id != Some(chat_id) {
            self.clear_compose_isolation_snapshot();
        }
        let mut selected = false;
        let mut max_msg_id = 0i64;
        let rich_meta_opt = self.session_sync.rich_chats_by_id.get(&chat_id).cloned();
        let self_uid = self.session_sync.user_id.unwrap_or(0);
        let lang_for_sync = self.active_lang_id().to_string();
        let cached_history_snap = self.client.local_store.get_cached_chat_history(chat_id);
        self.sidebar.update(cx, |sb, sb_cx| {
            let dlg_info = sb
                .dialogs
                .iter()
                .find(|d| d.id == chat_id)
                .map(|d| (d.is_muted, d.is_pinned, d.peer_name.clone(), d.avatar_initials.clone(), d.avatar_color));
            self.stage.update(sb_cx, |st, st_cx| {
                let had_stage_focus = st.search_open || st.active_bot_card_field.is_some();
                // Hydrate persisted SQLite history into `chat_histories` BEFORE `set_chat` so
                // previously visited or prefetched chats paint with full messages on Frame 0.
                if st.chat_histories.get(&chat_id).map_or(true, |h| h.is_empty()) {
                    if let (Some(snap), Some((_, _, peer_name, peer_init, peer_color))) =
                        (&cached_history_snap, &dlg_info)
                    {
                        if !snap.messages.is_empty() {
                            let models = build_cached_history_models(
                                &snap.messages,
                                &snap.rich_messages,
                                self_uid,
                                peer_name,
                                peer_init,
                                *peer_color,
                            );
                            st.chat_histories.insert(chat_id, models);
                        }
                    }
                }
                self.inspector.update(st_cx, |insp, insp_cx| {
                    let had_insp_focus = insp.is_bot_input_active();
                    selected = select_authoritative_dialog(sb, st, insp, chat_id);
                    if selected {
                        if let Some((muted, pinned, _, _, _)) = dlg_info {
                            st.is_muted_chat = muted;
                            st.is_pinned_chat = pinned;
                        } else if let Some(ref rc) = rich_meta_opt {
                            st.is_muted_chat = rc.is_muted;
                            st.is_pinned_chat = rc.is_pinned;
                        }
                        if let Some(ref rc) = rich_meta_opt {
                            st.is_peer_blocked = rc.is_blocked;
                            st.is_bot_chat = rc.is_bot;
                            st.is_group_chat = rc.is_group || rc.is_channel || matches!(rc.chat_type, 1 | 2 | 3);
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
                                st.chat_subtitle = if lang_for_sync.starts_with("zh") {
                                    format!("{} 位成员", rc.members_count)
                                } else {
                                    format!("{} members", rc.members_count)
                                };
                            }
                        }
                        if st.messages.is_empty()
                            && !st.loaded_history_chats.contains(&chat_id)
                            && !st.show_bot_start_bar
                        {
                            st.is_loading_history = true;
                        } else {
                            st.is_loading_history = false;
                        }
                        max_msg_id = st.messages.iter().map(|m| m.id as i64).max().unwrap_or(0);
                        let (_, media, files, links, voices) = map_stage_media_to_gallery(&st.messages);
                        insp.sync_shared_content(media, files, links, voices, Vec::new());
                        let fixed_init = derive_initials(&insp.profile.name);
                        if (insp.profile.avatar_initials.is_empty() || insp.profile.avatar_initials == "?")
                            && !fixed_init.is_empty()
                        {
                            insp.profile.avatar_initials = fixed_init;
                        }
                        if had_insp_focus && !insp.is_bot_input_active() {
                            insp_cx.notify();
                        }
                    }
                });
                let stage_focus_cleared =
                    had_stage_focus && !(st.search_open || st.active_bot_card_field.is_some());
                if selected && (prev_chat_id != Some(chat_id) || stage_focus_cleared) {
                    st_cx.notify();
                }
            });
        });
        if !selected {
            return;
        }
        self.client.local_store.set_last_active_chat_id(Some(chat_id));
        if prev_chat_id != Some(chat_id) {
            let _ = self.client.subscribe_chat(chat_id as i64);
            if let Some(prev) = prev_chat_id {
                if self.window_runtime.controller.clear_toasts_for_chat(prev) {
                    self.schedule_current_toast_dismiss(cx);
                }
            }
            self.reconcile_chat_scoped_toasts(cx);
            let in_flight_send_draft = self
                .session_sync
                .has_unmodified_in_flight_send_draft(chat_id);
            let cached_draft = if in_flight_send_draft {
                String::new()
            } else {
                self.client
                    .local_store
                    .get_draft(ChatId(chat_id as i64))
                    .map(|d| d.text)
                    .unwrap_or_default()
            };
            self.session_sync
                .last_draft_text_by_chat
                .insert(chat_id, cached_draft.clone());
            let cached_send_as_list = self
                .session_sync
                .send_as_peers
                .list(&SendAsKey::message(chat_id))
                .to_vec();
            let cached_chosen_send_as = self.session_sync.send_as_peers.chosen(chat_id);
            self.composer.update(cx, |c, _| {
                if !c.is_compose_isolated {
                    if cached_draft.is_empty() {
                        c.clear();
                    } else {
                        c.set_text(cached_draft.clone());
                    }
                    c.cancel_reply_or_edit();
                    c.editor.attachments.clear();
                    c.submitted_attachment = None;
                    c.pending_attached_files = None;
                    c.clear_bot_reply_keyboard();
                    c.set_send_as_options(cached_send_as_list, cached_chosen_send_as);
                }
            });
            self.sidebar
                .update(cx, |sb, _| sb.set_draft(chat_id, cached_draft));
        }
        self.miniapps.pending_prestart_bot = None;
        let sb_is_bot = self.sidebar.read(cx).bot_ids.contains(&chat_id);
        let fallback_bot_target = rich_meta_opt
            .as_ref()
            .and_then(|rc| rc.peer_username.clone().or_else(|| rc.username.clone()))
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                rich_meta_opt
                    .as_ref()
                    .and_then(|rc| rc.peer_user_id)
                    .filter(|&id| id > 0)
                    .map(|id| id.to_string())
            })
            .or_else(|| {
                self.session_sync
                    .chat_peer_user_ids
                    .get(&chat_id)
                    .filter(|&&id| id > 0)
                    .map(|id| id.to_string())
            });
        let history_fresh_in_flight = prev_chat_id == Some(chat_id)
            && self
                .session_sync
                .history_in_flight_chat
                .is_some_and(|(id, t)| id == chat_id && t.elapsed() < std::time::Duration::from_secs(3));
        if !history_fresh_in_flight {
            self.load_historical_messages(chat_id, cx);
        }
        self.sync_composer_and_inspector_miniapp_state(cx);
        self.sync_active_bot_computer_to_inspector(cx);
        self.sync_fabushi_bots_to_views(cx);
        if !self.composer.read(cx).is_compose_isolated {
            self.sync_composer_mention_candidates(chat_id, None, cx);
        }
        let Some(call_fence) = self.capture_client_fence() else {
            return;
        };
        let captured_draft_op_gen = self.session_sync.draft_op_gen(chat_id);
        let active_bot_id_for_approvals = self.active_chat_bot_id(cx);
        let lang = self.active_lang_id().to_string();
        let contacts = self.session_sync.contacts_list.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !call_fence.preflight_ui(&weak, app).await {
                return;
            }
            let cid_i64 = chat_id as i64;
            let (
                details_res,
                rich_chat_res,
                members_res,
                invites_res,
                pins_res,
                draft_res,
                stats_res,
                send_as_res,
            ) = futures::join!(
                call_fence.client.get_chat_with_token(&call_fence.token, call_fence.client_epoch, cid_i64),
                call_fence.client.hermes.get_chat_rich(&call_fence.token, cid_i64),
                call_fence.client.get_chat_members_with_token(&call_fence.token, call_fence.client_epoch, cid_i64),
                call_fence.client.list_invites_with_token(&call_fence.token, call_fence.client_epoch, cid_i64),
                call_fence.client.list_pinned_messages_with_token(&call_fence.token, call_fence.client_epoch, cid_i64),
                call_fence.client.get_draft_with_token(&call_fence.token, call_fence.client_epoch, cid_i64),
                call_fence.client.get_channel_statistics_with_token(&call_fence.token, call_fence.client_epoch, cid_i64),
                call_fence.client.http_client.get_send_as_peers(&call_fence.token, cid_i64),
            );
            let details = details_res.ok();
            let rich_chat = rich_chat_res.ok();
            let members = members_res.ok();
            let invites = invites_res.ok();
            let pins = pins_res.ok();
            let draft = draft_res.ok().flatten();
            let stats = stats_res.ok();
            let send_as_dto = send_as_res.ok();

            if !call_fence.preflight_ui(&weak, app).await {
                return;
            }
            let rc_is_bot_flag = rich_chat.as_ref().is_some_and(|rc| rc.is_bot) || sb_is_bot;
            let bot_target = if rc_is_bot_flag {
                rich_chat
                    .as_ref()
                    .and_then(|rc| rc.peer_username.clone().or_else(|| rc.username.clone()))
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| {
                        rich_chat
                            .as_ref()
                            .and_then(|rc| rc.peer_user_id)
                            .filter(|&id| id > 0)
                            .map(|id| id.to_string())
                    })
                    .or(fallback_bot_target)
            } else {
                None
            };
            let approvals_bot_id = active_bot_id_for_approvals.or_else(|| {
                rich_chat
                    .as_ref()
                    .filter(|rc| rc.is_bot)
                    .and_then(|rc| rc.peer_user_id)
            });

            let bot_profile_fut = async {
                if let Some(ref target) = bot_target {
                    call_fence
                        .client
                        .get_bot_public_profile_with_token(
                            &call_fence.token,
                            call_fence.client_epoch,
                            target,
                        )
                        .await
                        .ok()
                } else {
                    None
                }
            };
            let approvals_fut = async {
                call_fence
                    .client
                    .hermes
                    .list_approvals_for_bot(&call_fence.token, approvals_bot_id)
                    .await
                    .ok()
            };
            let mark_read_fut = async {
                if max_msg_id > 0 {
                    let _ = call_fence
                        .client
                        .mark_chat_read_with_token(
                            &call_fence.token,
                            call_fence.client_epoch,
                            cid_i64,
                            max_msg_id,
                            None,
                        )
                        .await;
                }
            };
            let (bot_profile, pending_approvals, _) =
                futures::join!(bot_profile_fut, approvals_fut, mark_read_fut);

            let _ = weak.update(app, |shell, cx| {
                if !call_fence.is_valid(shell) {
                    return;
                }
                if let Some(ref prof) = bot_profile {
                    let key = prof.bot.username.trim().trim_start_matches('@').to_ascii_lowercase();
                    if !key.is_empty() {
                        shell.miniapps.bot_profiles_cache.insert(key, prof.clone());
                    }
                }
                if let Some(ref rc) = rich_chat {
                    shell.client.local_store.upsert_cached_rich_chat(rc.clone());
                    shell.session_sync.rich_chats_by_id.insert(chat_id, rc.clone());
                    if let Some(pid) = rc.peer_user_id.filter(|&pid| pid > 0) {
                        shell.session_sync.chat_peer_user_ids.insert(chat_id, pid);
                    }
                    if rc.is_bot {
                        shell.sidebar.update(cx, |sb, _| { sb.bot_ids.insert(chat_id); });
                    }
                    let lang_for_stage = lang.clone();
                    shell.stage.update(cx, |st, st_cx| {
                        if st.active_chat_id == Some(chat_id) {
                            st.is_muted_chat = rc.is_muted;
                            st.is_pinned_chat = rc.is_pinned;
                            st.is_peer_blocked = rc.is_blocked;
                            st.is_bot_chat = rc.is_bot;
                            st.is_group_chat = rc.is_group || rc.is_channel || matches!(rc.chat_type, 1 | 2 | 3);
                            st.is_saved_messages = rc.chat_type == 5
                                || rc.kind.as_deref() == Some("saved")
                                || (rc.chat_type == 0 && rc.peer_user_id.is_none() && rc.members_count <= 1 && !rc.is_bot);
                            st.message_ttl_seconds = rc.message_ttl_seconds;
                            if st.is_group_chat && rc.members_count > 0 && st.chat_subtitle.trim().is_empty() {
                                st.chat_subtitle = if lang_for_stage.starts_with("zh") {
                                    format!("{} 位成员", rc.members_count)
                                } else {
                                    format!("{} members", rc.members_count)
                                };
                            }
                            st_cx.notify();
                        }
                    });
                }
                let is_active_chat = shell.stage.read(cx).active_chat_id == Some(chat_id);
                if is_active_chat {
                    let viewer_uid = shell.session_sync.user_id;
                    let peer_uid_for_fabushi = rich_chat
                        .as_ref()
                        .and_then(|rc| rc.peer_user_id)
                        .or_else(|| bot_profile.as_ref().map(|p| p.bot.bot_user_id.max(0) as u64));
                    let is_fabushi_owner_bot = peer_uid_for_fabushi.is_some_and(|uid| {
                        shell
                            .miniapps
                            .fabushi_bots
                            .iter()
                            .any(|fb| fb.bot_user_id > 0 && fb.bot_user_id as u64 == uid)
                    }) || bot_profile.as_ref().is_some_and(|p| {
                        p.bot.is_fabushi_bot && viewer_uid.is_some_and(|u| p.bot.owner_user_id == u as i64)
                    });
                    shell.inspector.update(cx, |insp, _| {
                        let mut username = String::new();
                        let mut phone = String::new();
                        let mut bio = String::new();
                        let mut status = insp.profile.status.clone();
                        let mut avatar_path = details.as_ref().and_then(|d| d.photo_file_id.clone());
                        let mut facts = details.as_ref().map(|d| build_chat_profile_facts(d, &lang)).unwrap_or_default();

                        if let Some(ref rc) = rich_chat {
                            let resolved_title = rc
                                .peer_display_name
                                .as_deref()
                                .filter(|s| !s.trim().is_empty())
                                .or_else(|| Some(rc.title.trim()).filter(|t| !t.is_empty()));
                            if let Some(title) = resolved_title {
                                insp.profile.name = title.to_string();
                                let init = derive_initials(title);
                                if !init.is_empty() {
                                    insp.profile.avatar_initials = init;
                                }
                            }
                            username = normalized_username(
                                rc.peer_username
                                    .as_deref()
                                    .filter(|s| !s.trim().is_empty())
                                    .or(rc.username.as_deref()),
                            );
                            phone = rc.peer_phone.clone().unwrap_or_default();
                            bio = rc
                                .peer_bio
                                .as_deref()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .or_else(|| rc.about.as_deref().map(str::trim).filter(|s| !s.is_empty()))
                                .unwrap_or("")
                                .to_string();
                            if rc.is_bot {
                                if is_fabushi_owner_bot {
                                    status = tr_str(&lang, "AI 机器人 · Fabushi 智能体", "AI Bot · Fabushi Agent").to_string();
                                } else if let Some(ref prof) = bot_profile {
                                    if prof.bot.mau_count > 0 {
                                        status = crate::views::drawer::bot_computer_inspector::format_bot_mau_subtitle(
                                            prof.bot.mau_count,
                                            &lang,
                                        );
                                    } else {
                                        status = tr_str(&lang, "机器人", "bot").to_string();
                                    }
                                } else {
                                    status = tr_str(&lang, "机器人", "bot").to_string();
                                }
                            }
                            facts = enrich_facts_with_rich_chat(facts, rc, is_fabushi_owner_bot, &lang);
                        }
                        if let Some(ref prof) = bot_profile {
                            if username.is_empty() && !prof.bot.username.trim().is_empty() {
                                username = normalized_username(Some(prof.bot.username.as_str()));
                            }
                            if bio.is_empty() {
                                let b = if !prof.bot.about.trim().is_empty() {
                                    prof.bot.about.trim()
                                } else {
                                    prof.bot.description.trim()
                                };
                                if !b.is_empty() {
                                    bio = b.to_string();
                                }
                            }
                        }
                        if username.is_empty() {
                            let contact = rich_chat
                                .as_ref()
                                .and_then(|rc| rc.peer_user_id)
                                .and_then(|pid| contacts.iter().find(|c| c.user_id == pid))
                                .or_else(|| contacts.iter().find(|c| c.chat_id == Some(chat_id as i64) || c.user_id == chat_id));
                            if let Some(c) = contact {
                                username = normalized_username(c.username.as_deref());
                                if phone.is_empty() {
                                    phone = c.phone.clone().unwrap_or_default();
                                }
                            }
                        }
                        if bio.is_empty() {
                            if let Some(about) = details.as_ref().and_then(|d| d.about.as_ref()).filter(|a| !a.trim().is_empty()) {
                                bio = about.trim().to_string();
                            }
                        }
                        insp.profile.bio = bio;
                        if avatar_path.is_none() && details.is_none() {
                            avatar_path = insp.profile.avatar_path.clone();
                        }
                        if let Some(ref rc) = rich_chat {
                            insp.profile.is_bot = rc.is_bot;
                            if !rc.is_bot {
                                insp.profile.is_verified = false;
                            }
                        } else {
                            insp.profile.is_bot = rc_is_bot_flag;
                            if !rc_is_bot_flag {
                                insp.profile.is_verified = false;
                            }
                        }
                        insp.sync_profile_details(username, phone, status, avatar_path, facts);
                        let inv = invites.as_ref().and_then(|v| v.first()).map(|i| i.link.clone()).unwrap_or_default();
                        let is_grp = rich_chat
                            .as_ref()
                            .map(|rc| rc.is_group || rc.is_channel || matches!(rc.chat_type, 1 | 2 | 3))
                            .unwrap_or_else(|| details.as_ref().is_some_and(|d| d.chat_type != 0) && !rc_is_bot_flag);
                        if is_grp {
                            if let Some(ref mems) = members {
                                let mut mapped = map_chat_members_to_inspector(mems, &contacts);
                                let is_dark = shell.theme_state.palette.mode.is_dark();
                                for item in &mut mapped {
                                    if let Some(fb) = shell
                                        .miniapps
                                        .fabushi_bots
                                        .iter()
                                        .find(|b| b.bot_user_id > 0 && b.bot_user_id as u64 == item.id)
                                    {
                                        if !fb.display_name.trim().is_empty() {
                                            item.name = fb.display_name.trim().to_string();
                                            let init = derive_initials(&item.name);
                                            if !init.is_empty() {
                                                item.initials = init;
                                            }
                                        }
                                        let hue = if TdesktopThemePalette::is_tinted_bot_hue(&fb.avatar_color) {
                                            fb.avatar_color.as_str()
                                        } else {
                                            TdesktopThemePalette::bot_hue_name_for_id(item.id)
                                        };
                                        item.color = TdesktopThemePalette::bot_hue_colors(hue, is_dark).0;
                                        item.role = "AI Bot".to_string();
                                        item.is_online = true;
                                    }
                                }
                                insp.sync_members(mapped, inv, true);
                            } else {
                                insp.sync_members(Vec::new(), inv, true);
                            }
                        } else {
                            insp.sync_members(Vec::new(), String::new(), false);
                        }
                        if let Some(ref st) = stats {
                            let (lvl, cur, tgt, bars) = map_channel_stats_to_chart(st);
                            insp.sync_channel_statistics(lvl, cur, tgt, bars);
                        }
                    });
                }
                if let Some(pin_list) = pins {
                    let entries: Vec<PinnedBannerEntry> = pin_list
                        .into_iter()
                        .filter_map(|p| {
                            let msg_id = u64::try_from(p.message_id).ok()?;
                            let preview = if p.preview_text.trim().is_empty() {
                                format!("Pinned message #{msg_id}")
                            } else {
                                p.preview_text
                            };
                            Some(PinnedBannerEntry {
                                msg_id,
                                preview_text: preview,
                                sender_name: String::new(),
                                thumb_url: None,
                            })
                        })
                        .collect();
                    shell.stage.update(cx, |st, _| {
                        st.set_pinned_entries(chat_id, entries);
                    });
                }
                let draft_unchanged = shell.session_sync.draft_op_gen(chat_id)
                    == captured_draft_op_gen
                    && !shell
                        .session_sync
                        .has_unmodified_in_flight_send_draft(chat_id);
                if draft_unchanged {
                    if let Some(dr) = draft {
                        shell
                            .client
                            .local_store
                            .set_draft(ChatId(chat_id as i64), Some(dr.clone()));
                        shell
                            .session_sync
                            .last_saved_draft_by_chat
                            .insert(chat_id, dr.text.clone());
                        shell
                            .session_sync
                            .last_draft_text_by_chat
                            .insert(chat_id, dr.text.clone());
                        shell.sidebar.update(cx, |sb, _| sb.set_draft(chat_id, dr.text.clone()));
                        let is_compose_isolated = shell.composer.read(cx).is_compose_isolated;
                        if shell.stage.read(cx).active_chat_id == Some(chat_id)
                            && !is_compose_isolated
                            && !dr.text.is_empty()
                            && shell.composer.read(cx).text().is_empty()
                        {
                            shell.composer.update(cx, |c, _| c.set_text(dr.text));
                        }
                    }
                } else if !shell
                    .session_sync
                    .has_unmodified_in_flight_send_draft(chat_id)
                {
                    let cur_text = shell
                        .session_sync
                        .last_draft_text_by_chat
                        .get(&chat_id)
                        .cloned()
                        .unwrap_or_default();
                    let dto = (!cur_text.trim().is_empty()).then(|| DraftDto {
                        chat_id: chat_id as i64,
                        text: cur_text,
                        entities: Vec::new(),
                        reply_to_msg_id: None,
                        topic_id: None,
                        date_ms: chrono::Utc::now().timestamp_millis(),
                    });
                    shell
                        .client
                        .local_store
                        .set_draft(ChatId(chat_id as i64), dto);
                }
                let my_uid = shell.session_sync.user_id.unwrap_or(0);
                let my_name = shell
                    .session_sync
                    .username
                    .clone()
                    .unwrap_or_else(|| "Me".to_string());
                let key = SendAsKey::message(chat_id);
                let send_as_list: Vec<SendAsPeer> = if let Some(dto_list) = send_as_dto {
                    let mut mapped: Vec<SendAsPeer> = dto_list
                        .into_iter()
                        .map(|p| SendAsPeer::new(p.peer_id, p.title, false))
                        .collect();
                    if mapped.is_empty() && my_uid > 0 {
                        mapped.push(SendAsPeer::new(my_uid, my_name, false));
                    }
                    shell
                        .session_sync
                        .send_as_peers
                        .set_list(key, mapped.clone());
                    mapped
                } else {
                    let existing = shell.session_sync.send_as_peers.list(&key).to_vec();
                    if !existing.is_empty() {
                        existing
                    } else if my_uid > 0 {
                        let fallback = vec![SendAsPeer::new(my_uid, my_name, false)];
                        shell
                            .session_sync
                            .send_as_peers
                            .set_list(key, fallback.clone());
                        fallback
                    } else {
                        Vec::new()
                    }
                };
                let chosen = shell.session_sync.send_as_peers.chosen(chat_id);
                let is_compose_isolated = shell.composer.read(cx).is_compose_isolated;
                if shell.stage.read(cx).active_chat_id == Some(chat_id) && !is_compose_isolated {
                    shell.composer.update(cx, |c, _| {
                        c.set_send_as_options(send_as_list, chosen);
                    });
                }
                if is_active_chat {
                    if !shell.composer.read(cx).is_compose_isolated {
                        shell.sync_composer_and_inspector_miniapp_state(cx);
                        shell.sync_composer_mention_candidates(chat_id, members.as_deref(), cx);
                    }
                    shell.sync_active_bot_computer_to_inspector(cx);
                    if let Some(approvals) = pending_approvals {
                        let mapped: Vec<PendingToolApproval> = approvals
                            .into_iter()
                            .filter(|a| a.status.eq_ignore_ascii_case("pending"))
                            .map(|a| {
                                let tier = ToolApprovalPolicyTier::from_policy_str(&a.risk_level);
                                let summary = if a.reason.trim().is_empty() {
                                    a.command_preview.clone()
                                } else {
                                    a.reason.clone()
                                };
                                PendingToolApproval {
                                    id: a.approval_id,
                                    tool_name: a.tool_name,
                                    summary,
                                    arguments_preview: a.command_preview.clone(),
                                    risk_level: a.risk_level,
                                    created_at: String::new(),
                                    target: a.command_preview,
                                    cwd: String::new(),
                                    policy_tier: tier,
                                }
                            })
                            .collect();
                        shell.stage.update(cx, |st, st_cx| {
                            if st.active_chat_id == Some(chat_id) {
                                st.sync_pending_tool_approvals(mapped);
                                st_cx.notify();
                            }
                        });
                    }
                }
                shell.sync_fabushi_bots_to_views(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Dispatches Hermes inline callback commands, approvals, Talk Mode, Remote Computer, or Mini-Apps.
    pub fn execute_hermes_inline_callback(&mut self, raw_action: String, cx: &mut Context<Self>) {
        if self.try_handle_or_swallow_apple_deep_link(&raw_action, true, cx) {
            return;
        }
        let trimmed = raw_action.trim().to_string();
        let lower = trimmed.to_ascii_lowercase();
        let lang = self.active_lang_id();

        if matches!(
            lower.as_str(),
            "hermes:open_ai_providers"
                | "open_ai_providers"
                | "action_open_ai_providers"
                | "/providers"
                | "/magpie"
        ) {
            self.open_ai_providers_settings(cx);
            return;
        }

        if matches!(
            lower.as_str(),
            "action_talk_mode" | "talk_mode" | "talk:start" | "/talk" | "launch talk mode"
        ) || lower.contains("talk_mode")
            || lower.starts_with("talk:")
        {
            self.nav_rail.update(cx, |r, _| r.set_active_destination(NavDestination::TalkMode));
            self.start_ai_talk_mode_session(false, cx);
            return;
        }

        if matches!(
            lower.as_str(),
            "action_remote_computer" | "remote_computer" | "computer:open" | "/computer" | "remote computer"
        ) || lower.contains("remote_computer")
            || lower.starts_with("computer:")
        {
            self.nav_rail.update(cx, |r, _| r.set_active_destination(NavDestination::RemoteComputer));
            self.open_remote_computers_hub(cx);
            return;
        }

        if parse_miniapp_uri(&trimmed).is_some() {
            let active_chat = resolve_active_target_chat_id(
                self.stage.read(cx).active_chat_id,
                self.sidebar.read(cx).active_chat_id,
            );
            self.launch_miniapp_from_uri(&trimmed, active_chat, MiniAppLaunchMode::InlineKeyboard, cx);
            return;
        }

        if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("url:") {
            let url = if lower.starts_with("url:") {
                trimmed.get(4..).unwrap_or("").trim().to_string()
            } else {
                trimmed.clone()
            };
            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
            self.show_toast(
                format!("{}: {}", tr_str(lang, "已复制链接", "Copied URL"), url),
                2600,
                cx,
            );
            return;
        }

        let active_chat = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        );
        let bot_id = self.active_chat_bot_id(cx).or_else(|| {
            active_chat.and_then(|cid| {
                self.session_sync
                    .rich_chats_by_id
                    .get(&cid)
                    .and_then(|rc| rc.peer_user_id)
                    .or_else(|| self.session_sync.chat_peer_user_ids.get(&cid).copied())
            })
        });
        let session_id = self.rtc_talk.active_talk_session_id.clone();

        if lower.starts_with("approval:")
            || lower.starts_with("/approve")
            || lower.starts_with("/deny")
            || lower.starts_with("hermes:")
        {
            let Some(call_fence) = self.capture_client_fence() else {
                return;
            };
            let captured_panel = self.active_panel;
            let captured_plus_open = self.sidebar.read(cx).is_plus_menu_open;
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                if !call_fence.preflight_ui(&weak, app).await {
                    return;
                }
                let toast = if lower.starts_with("approval:") {
                    let parts: Vec<&str> = trimmed.split(':').collect();
                    let approval_id = parts.get(1).copied().unwrap_or("");
                    let dec = parts.get(2).copied().unwrap_or("approve_once");
                    match call_fence
                        .client
                        .hermes
                        .decide_approval(
                            &call_fence.token,
                            approval_id,
                            &HermesApprovalDecisionRequest {
                                decision: dec.to_string(),
                                reason: Some("Inline callback button".to_string()),
                            },
                        )
                        .await
                    {
                        Ok(a) => format!("Approval {}: {}", a.approval_id, a.status),
                        Err(e) => format!("Approval error: {}", e),
                    }
                } else {
                    let cmd_text = trimmed.strip_prefix("hermes:").unwrap_or(&trimmed).trim().to_string();
                    match call_fence
                        .client
                        .hermes
                        .run_command(
                            &call_fence.token,
                            &HermesCommandRequest {
                                command: cmd_text,
                                session_id,
                                chat_id: active_chat.map(|id| id as i64),
                                bot_id,
                            },
                        )
                        .await
                    {
                        Ok(resp) => resp
                            .reply_text
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| {
                                if resp.output.trim().is_empty() {
                                    format!("Executed {}", resp.command)
                                } else {
                                    resp.output
                                }
                            }),
                        Err(e) => format!("Fabushi command failed: {}", e),
                    }
                };
                let _ = weak.update(app, |shell, cx| {
                    if !call_fence.is_valid(shell) {
                        return;
                    }
                    let summary: String = toast.lines().next().unwrap_or(&toast).chars().take(120).collect();
                    if let Some(cid) = active_chat {
                        if shell.active_panel == ActiveWorkspacePanel::Chats
                            && !shell.sidebar.read(cx).is_plus_menu_open
                            && shell.stage.read(cx).active_chat_id == Some(cid)
                        {
                            shell.show_chat_toast(cid, summary, 3000, cx);
                        }
                    } else {
                        let cur_active = resolve_active_target_chat_id(
                            shell.stage.read(cx).active_chat_id,
                            shell.sidebar.read(cx).active_chat_id,
                        );
                        if shell.active_panel == captured_panel
                            && cur_active.is_none()
                            && shell.sidebar.read(cx).is_plus_menu_open == captured_plus_open
                        {
                            shell.show_non_chat_toast(summary, 3000, cx);
                        }
                    }
                    shell.refresh_tasks_and_mcp(cx);
                    if let Some(cid) = active_chat {
                        if shell.stage.read(cx).active_chat_id == Some(cid) {
                            shell.load_historical_messages(cid, cx);
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
            return;
        }

        self.append_non_composer_outgoing_text(trimmed, cx);
    }

    /// Drains pending user intents queued by `ConversationStage` and routes each to shell actions or RPCs.
    pub fn handle_stage_pending_actions(&mut self, cx: &mut Context<Self>) {
        let lang = self.active_lang_id();
        let active_chat = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        );
        let (
            (call_v, call_vid),
            (media_single, open_url_req, media_gal, iv_url),
            (edit_req, fwd_ids, del_ids, pin_req, tr_id, poll_vote, react_reqs),
            (voice_play, dl_req, drops, open_prof),
            (clear_hist, export_chat, toggle_mute, change_wallpaper, toggle_pin_chat, set_ttl_secs, toggle_block_peer, delete_or_leave_chat),
            (bot_act, bot_btn_act),
        ) = self.stage.update(cx, |st, _| {
            (
                (mem::take(&mut st.pending_call_request), mem::take(&mut st.pending_video_call)),
                (st.pending_media_url.take(), st.pending_open_url.take(), st.pending_media_viewer.take(), st.pending_iv_url.take()),
                (
                    st.pending_edit_message.take(), st.pending_forward_message_ids.take(),
                    st.pending_delete_message_ids.take(), st.pending_pin_toggle.take(),
                    st.pending_translate_message_id.take(), st.pending_poll_vote.take(),
                    st.drain_pending_reaction_actions(),
                ),
                (
                    st.pending_voice_playback.take(), st.pending_download_url.take(),
                    st.pending_dropped_paths.take(), st.pending_open_profile_peer_id.take(),
                ),
                (
                    mem::take(&mut st.pending_clear_history), mem::take(&mut st.pending_export_chat),
                    mem::take(&mut st.pending_toggle_mute), mem::take(&mut st.pending_change_wallpaper),
                    mem::take(&mut st.pending_toggle_pin_chat), st.pending_set_ttl_seconds.take(),
                    mem::take(&mut st.pending_toggle_block_peer), mem::take(&mut st.pending_delete_or_leave_chat),
                ),
                (st.pending_bot_action.take(), st.pending_bot_button_action.take()),
            )
        });
        if self.stage.read(cx).more_menu_open && self.sidebar.read(cx).is_plus_menu_open {
            self.sidebar.update(cx, |sb, sb_cx| {
                sb.cancel_compose_mode();
                sb_cx.notify();
            });
            if self.composer.read(cx).is_compose_isolated
                && !self.is_compose_or_creation_modal_active(cx)
            {
                self.exit_compose_or_modal_isolation(true, cx);
            }
        }
        if call_v || call_vid {
            if self.is_active_chat_bot(cx) {
                self.start_ai_talk_mode_session(call_vid, cx);
            } else if let Some(pid) = self.current_peer_user_id(cx) {
                let name = self.stage.read(cx).chat_title.clone();
                self.start_webrtc_call(pid, name, call_vid, cx);
            } else {
                self.show_toast(
                    tr_str(lang, "当前群组或会话未绑定直接通话用户，请进入私聊或机器人对话。", "No direct user peer resolved for WebRTC call in this chat."),
                    3000,
                    cx,
                );
            }
        }
        if let Some(url) = open_url_req {
            if !self.try_handle_or_swallow_apple_deep_link(&url, false, cx)
                && !self.handle_deep_link(&url, cx)
                && !self.launch_miniapp_from_uri(
                    &url,
                    active_chat,
                    MiniAppLaunchMode::DirectLink,
                    cx,
                )
            {
                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("open").arg(&url).spawn();
                self.show_toast(
                    format!("{}: {}", tr_str(lang, "已复制链接", "Copied URL"), url),
                    2400,
                    cx,
                );
            }
        }
        if let Some((items, idx)) = media_gal {
            let active_url = items
                .get(idx)
                .map(|(u, _, _)| u.clone())
                .or_else(|| media_single.as_ref().map(|(u, _)| u.clone()));
            let handled_active = active_url
                .as_deref()
                .is_some_and(|u| self.try_handle_or_swallow_apple_deep_link(u, false, cx));
            if !handled_active {
                let mut adjusted_idx = 0usize;
                let mut mv_items = Vec::new();
                for (orig_i, (url, is_video, caption)) in items.into_iter().enumerate() {
                    if crate::views::app_shell::shell_content_hub::is_apple_deep_link_candidate(&url)
                    {
                        continue;
                    }
                    if orig_i == idx {
                        adjusted_idx = mv_items.len();
                    }
                    mv_items.push(MediaViewerItem {
                        id: mv_items.len() as u64 + 1,
                        url,
                        is_video,
                        caption,
                        sender_name: String::new(),
                        date: String::new(),
                        duration_secs: None,
                    });
                }
                if !mv_items.is_empty() {
                    attach_media_viewer_overlay(
                        self,
                        MediaViewer::from_gallery(mv_items, adjusted_idx),
                        lang,
                        cx,
                    );
                }
            }
        } else if let Some((url, is_vid)) = media_single {
            if !self.try_handle_or_swallow_apple_deep_link(&url, false, cx) {
                attach_media_viewer_overlay(self, MediaViewer::new(url, is_vid), lang, cx);
            }
        }
        if let Some((url, title)) = iv_url {
            if !self.try_handle_or_swallow_apple_deep_link(&url, false, cx) {
                self.occlude_browser_for_overlay(cx);
                self.prepare_for_media_or_call_overlay_open(cx);
                let pal = self.theme_state.palette;
                let overlay = cx.new(|_| { let mut iv = IvViewer::new(url, title); iv.apply_theme(pal, lang); iv });
                cx.observe(&overlay, |shell: &mut AppShell, ov: gpui::Entity<IvViewer>, cx: &mut Context<AppShell>| {
                    let (close, share) = ov.update(cx, |v, _| (v.should_close, v.pending_share_url.take()));
                    if let Some(u) = share {
                        if !shell.try_handle_or_swallow_apple_deep_link(&u, false, cx) {
                            shell.append_non_composer_outgoing_text(u, cx);
                        }
                    }
                    if close {
                        shell.overlays.iv_viewer_overlay = None;
                        shell.clear_ime_if_below_z5_unoccluded(cx);
                        cx.notify();
                    }
                })
                .detach();
                self.overlays.iv_viewer_overlay = Some(overlay);
                self.reconcile_chat_scoped_toasts(cx);
            }
        }
        if let Some((mid, existing)) = edit_req {
            self.composer.update(cx, |c, _| c.begin_edit(mid, existing));
        }
        if let Some(ids) = fwd_ids {
            let preview_snippet = format!("{} {}", ids.len(), tr_str(lang, "条消息", "messages"));
            let kind = ModalDialogKind::ForwardMessages { message_ids: ids, preview_snippet };
            self.open_modal_dialog(tr_str(lang, "转发消息", "Forward Messages"), kind, 420.0, 460.0, cx);
        }
        if let Some(ids) = del_ids {
            let peer_name = self.stage.read(cx).chat_title.clone();
            let kind = ModalDialogKind::DeleteMessages { message_ids: ids, peer_name };
            self.open_modal_dialog(tr_str(lang, "删除消息", "Delete Messages"), kind, 380.0, 260.0, cx);
        }
        if let Some((mid, pin)) = pin_req {
            if pin {
                let peer_name = self.stage.read(cx).chat_title.clone();
                let kind = ModalDialogKind::PinMessage { message_id: mid, peer_name, is_group: false };
                self.open_modal_dialog(tr_str(lang, "置顶消息", "Pin Message"), kind, 380.0, 240.0, cx);
            } else if let Some(cid) = active_chat {
                self.stage.update(cx, |st, _| st.toggle_pin_message(mid));
                if let Some(fence) = self.capture_client_fence() {
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let _ = fence
                            .client
                            .unpin_message_with_token(
                                &fence.token,
                                fence.client_epoch,
                                cid as i64,
                                mid as i64,
                            )
                            .await;
                    })
                    .detach();
                }
            }
        }
        if let Some(mid) = tr_id {
            if let Some(original_text) = self.stage.read(cx).copy_message_text(mid) {
                let kind = ModalDialogKind::TranslateMessage { original_text };
                self.open_modal_dialog(tr_str(lang, "翻译消息", "Translate Message"), kind, 420.0, 320.0, cx);
            }
        }
        if let Some((mid, opt)) = poll_vote {
            self.stage.update(cx, |st, _| st.vote_poll(mid, opt));
            if let Some(fence) = self.capture_client_fence() {
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let _ = fence
                        .client
                        .vote_poll_with_token(
                            &fence.token,
                            fence.client_epoch,
                            mid,
                            vec![opt as u8],
                        )
                        .await;
                })
                .detach();
            }
        }
        if let Some(cid) = active_chat {
            for (mid, emoji, is_added) in react_reqs {
                self.dispatch_reaction_mutation(cid, mid, emoji, is_added, cx);
            }
        }
        if let Some((mid, play, _pos, spd)) = voice_play {
            self.rtc_talk.audio_player.set_speed(spd as f64);
            if !play {
                let _ = self.rtc_talk.audio_player.pause();
            } else if let Some(fence) = self.capture_client_fence() {
                let key = format!("voice_{mid}");
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    if let Ok((_, bytes, _)) = fence
                        .client
                        .download_media_range_with_token(
                            &fence.token,
                            fence.client_epoch,
                            &key,
                            0,
                            None,
                        )
                        .await
                    {
                        let _ = weak.update(app, |s, _| {
                            if !fence.is_valid(s) {
                                return;
                            }
                            let _ = s.rtc_talk.audio_player.play_wav_bytes(key, &bytes);
                        });
                    }
                })
                .detach();
            }
        }
        if let Some((url, name)) = dl_req {
            let fence = if self.try_handle_or_swallow_apple_deep_link(&url, false, cx) {
                None
            } else {
                self.capture_client_fence()
            };
            if let Some(fence) = fence {
                let origin_chat = active_chat;
                let captured_panel = self.active_panel;
                let captured_plus_open = self.sidebar.read(cx).is_plus_menu_open;
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let res = fence
                        .client
                        .download_media_range_with_token(
                            &fence.token,
                            fence.client_epoch,
                            &url,
                            0,
                            None,
                        )
                        .await;
                    let _ = weak.update(app, |s, cx| {
                        if !fence.is_valid(s) {
                            return;
                        }
                        let cur_active = resolve_active_target_chat_id(
                            s.stage.read(cx).active_chat_id,
                            s.sidebar.read(cx).active_chat_id,
                        );
                        if s.active_panel != captured_panel
                            || cur_active != origin_chat
                            || s.sidebar.read(cx).is_plus_menu_open != captured_plus_open
                        {
                            return;
                        }
                        let msg = if res.is_ok() {
                            let pfx = tr_str(s.active_lang_id(), "已保存文件", "Saved");
                            format!("{pfx}: {name}")
                        } else {
                            let pfx = tr_str(s.active_lang_id(), "下载失败", "Download failed");
                            format!("{pfx}: {name}")
                        };
                        if let Some(cid) = origin_chat {
                            if s.active_panel == ActiveWorkspacePanel::Chats
                                && !s.sidebar.read(cx).is_plus_menu_open
                                && s.stage.read(cx).active_chat_id == Some(cid)
                            {
                                s.show_chat_toast(cid, msg, 2500, cx);
                            }
                        } else {
                            s.show_non_chat_toast(msg, 2500, cx);
                        }
                    });
                })
                .detach();
            }
        }
        if let Some(paths) = drops {
            self.upload_and_send_paths(paths, cx);
        }
        if open_prof.is_some() {
            self.set_inspector_open(true, cx);
        }
        if clear_hist {
            let peer_name = self.stage.read(cx).chat_title.clone();
            self.open_modal_dialog(tr_str(lang, "清空聊天记录", "Clear History"), ModalDialogKind::ClearChatHistory { peer_name }, 390.0, 250.0, cx);
        }
        if export_chat {
            let chat_title = self.stage.read(cx).chat_title.clone();
            self.open_modal_dialog(tr_str(lang, "导出聊天", "Export Chat"), ModalDialogKind::ExportData { chat_title }, 400.0, 320.0, cx);
        }
        if let (true, Some(cid)) = (toggle_mute, active_chat) {
            toggle_chat_dialog_flag(self, cid, false, lang, cx);
        }
        if change_wallpaper {
            let peer_name = Some(self.stage.read(cx).chat_title.clone());
            self.open_modal_dialog(tr_str(lang, "聊天背景设置", "Chat Wallpaper"), ModalDialogKind::BackgroundPicker { peer_name }, 440.0, 420.0, cx);
        }
        if let (true, Some(cid)) = (toggle_pin_chat, active_chat) {
            toggle_chat_dialog_flag(self, cid, true, lang, cx);
        }
        if let (Some(ttl_secs), Some(cid)) = (set_ttl_secs, active_chat) {
            if let Some(fence) = self.capture_client_fence() {
                let prev_ttl = self
                    .session_sync
                    .rich_chats_by_id
                    .get(&cid)
                    .map(|rc| rc.message_ttl_seconds)
                    .unwrap_or(0);
                if let Some(rc) = self.session_sync.rich_chats_by_id.get_mut(&cid) {
                    rc.message_ttl_seconds = ttl_secs;
                }
                let lang_owned = lang.to_string();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let res = fence
                        .client
                        .set_chat_ttl_with_token(
                            &fence.token,
                            fence.client_epoch,
                            cid as i64,
                            ttl_secs,
                        )
                        .await;
                    let _ = weak.update(app, |s, cx| {
                        if !fence.is_valid(s) {
                            return;
                        }
                        let can_show_chat_toast = s.active_panel == ActiveWorkspacePanel::Chats
                            && !s.sidebar.read(cx).is_plus_menu_open
                            && s.stage.read(cx).active_chat_id == Some(cid);
                        match res {
                            Ok(_) => {
                                if can_show_chat_toast {
                                    let toast_msg = if ttl_secs == 0 {
                                        tr_str(&lang_owned, "已关闭自动删除消息", "Auto-delete turned off")
                                    } else if ttl_secs <= 86_400 {
                                        tr_str(&lang_owned, "自动删除已设为 1 天", "Auto-delete set to 1 day")
                                    } else if ttl_secs <= 604_800 {
                                        tr_str(&lang_owned, "自动删除已设为 1 周", "Auto-delete set to 1 week")
                                    } else {
                                        tr_str(&lang_owned, "自动删除已设为 1 个月", "Auto-delete set to 1 month")
                                    };
                                    s.show_chat_toast(cid, toast_msg, 2200, cx);
                                }
                            }
                            Err(_) => {
                                if let Some(rc) = s.session_sync.rich_chats_by_id.get_mut(&cid) {
                                    rc.message_ttl_seconds = prev_ttl;
                                }
                                s.stage.update(cx, |st, st_cx| {
                                    if st.active_chat_id == Some(cid) {
                                        st.message_ttl_seconds = prev_ttl;
                                        st_cx.notify();
                                    }
                                });
                                if can_show_chat_toast {
                                    s.show_chat_toast(
                                        cid,
                                        tr_str(&lang_owned, "自动删除设置保存失败", "Failed to update auto-delete timer"),
                                        2500,
                                        cx,
                                    );
                                }
                            }
                        }
                    });
                })
                .detach();
            }
        }
        if toggle_block_peer {
            let prev_blocked = self.stage.read(cx).is_peer_blocked;
            let my_uid = self.session_sync.user_id;
            if let Some(peer_uid) = self.current_peer_user_id(cx).filter(|&pid| pid > 0 && Some(pid) != my_uid) {
                let blocked_now = !prev_blocked;
                self.stage.update(cx, |st, st_cx| { st.is_peer_blocked = blocked_now; st_cx.notify(); });
                if let Some(rc) = active_chat.and_then(|cid| self.session_sync.rich_chats_by_id.get_mut(&cid)) {
                    rc.is_blocked = blocked_now;
                }
                if let Some(fence) = self.capture_client_fence() {
                    let lang_owned = lang.to_string();
                    let captured_panel = self.active_panel;
                    let captured_plus_open = self.sidebar.read(cx).is_plus_menu_open;
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let res = if blocked_now {
                            fence
                                .client
                                .block_user_with_token(&fence.token, fence.client_epoch, peer_uid)
                                .await
                                .map(|_| ())
                        } else {
                            fence
                                .client
                                .unblock_user_with_token(&fence.token, fence.client_epoch, peer_uid)
                                .await
                                .map(|_| ())
                        };
                        let _ = weak.update(app, |s, cx| {
                            if !fence.is_valid(s) {
                                return;
                            }
                            if res.is_ok() {
                                let toast_msg = if blocked_now {
                                    tr_str(&lang_owned, "已屏蔽该用户", "User blocked")
                                } else {
                                    tr_str(&lang_owned, "已取消屏蔽该用户", "User unblocked")
                                };
                                if let Some(cid) = active_chat {
                                    if s.active_panel == ActiveWorkspacePanel::Chats
                                        && !s.sidebar.read(cx).is_plus_menu_open
                                        && s.stage.read(cx).active_chat_id == Some(cid)
                                    {
                                        s.show_chat_toast(cid, toast_msg, 2200, cx);
                                    }
                                } else if s.active_panel == captured_panel
                                    && s.sidebar.read(cx).is_plus_menu_open == captured_plus_open
                                    && resolve_active_target_chat_id(
                                        s.stage.read(cx).active_chat_id,
                                        s.sidebar.read(cx).active_chat_id,
                                    ).is_none()
                                {
                                    s.show_non_chat_toast(toast_msg, 2200, cx);
                                }
                                s.refresh_settings_truth(cx);
                            } else {
                                if let Some(rc) = active_chat.and_then(|cid| s.session_sync.rich_chats_by_id.get_mut(&cid)) {
                                    rc.is_blocked = prev_blocked;
                                }
                                s.stage.update(cx, |st, st_cx| {
                                    if st.active_chat_id == active_chat { st.is_peer_blocked = prev_blocked; st_cx.notify(); }
                                });
                                let cur_active = resolve_active_target_chat_id(
                                    s.stage.read(cx).active_chat_id,
                                    s.sidebar.read(cx).active_chat_id,
                                );
                                let msg = tr_str(&lang_owned, "屏蔽状态同步失败", "Failed to update block status");
                                if let Some(cid) = active_chat {
                                    if s.active_panel == ActiveWorkspacePanel::Chats
                                        && !s.sidebar.read(cx).is_plus_menu_open
                                        && s.stage.read(cx).active_chat_id == Some(cid)
                                    {
                                        s.show_chat_toast(cid, msg, 2500, cx);
                                    }
                                } else if s.active_panel == captured_panel
                                    && cur_active.is_none()
                                    && s.sidebar.read(cx).is_plus_menu_open == captured_plus_open
                                {
                                    s.show_non_chat_toast(msg, 2500, cx);
                                }
                            }
                        });
                    })
                    .detach();
                }
            } else {
                let msg = tr_str(lang, "群组或收藏夹不支持单用户屏蔽", "Blocking applies only to direct user peers");
                if let Some(cid) = active_chat {
                    if self.active_panel == ActiveWorkspacePanel::Chats
                        && !self.sidebar.read(cx).is_plus_menu_open
                        && self.stage.read(cx).active_chat_id == Some(cid)
                    {
                        self.show_chat_toast(cid, msg, 2200, cx);
                    }
                } else {
                    self.show_non_chat_toast(msg, 2200, cx);
                }
            }
        }
        let leave_from_inspector = self.inspector.update(cx, |insp, _| insp.take_pending_leave_chat());
        let delete_or_leave_chat = delete_or_leave_chat || leave_from_inspector;
        if delete_or_leave_chat {
            self.open_leave_or_delete_chat_modal(cx);
        }
        let bot_assist_act = self
            .stage
            .update(cx, |st, _| st.pending_bot_assist_action.take());
        if let Some(act) = bot_assist_act {
            self.handle_bot_assist_card_action(act, cx);
        }
        let stage_act = self.stage.update(cx, |st, _| st.take_stage_action());
        if let Some(act) = stage_act {
            match act {
                StageAction::OpenSenderDm {
                    user_id,
                    sender_name,
                } => {
                    self.open_sender_dm_by_user_id(user_id, sender_name, cx);
                }
                StageAction::ToggleBotCompanion => {
                    self.toggle_bot_companion_panel(None, cx);
                }
                StageAction::DecideToolApproval {
                    approval_id,
                    decision,
                } => {
                    let dec_str = match decision {
                        ToolApprovalDecision::AllowOnce => "approve_once",
                        ToolApprovalDecision::AllowAlways => "approve_permanent",
                        ToolApprovalDecision::Deny => "denied",
                    };
                    if let Some(fence) = self.capture_client_fence() {
                        let lang_owned = lang.to_string();
                        let captured_panel = self.active_panel;
                        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                            if !fence.preflight_ui(&weak, app).await {
                                return;
                            }
                            let res = fence
                                .client
                                .hermes
                                .decide_approval(
                                    &fence.token,
                                    &approval_id,
                                    &HermesApprovalDecisionRequest {
                                        decision: dec_str.to_string(),
                                        reason: Some("Stage ToolPermissionDock".to_string()),
                                    },
                                )
                                .await;
                            let _ = weak.update(app, |shell, cx| {
                                if !fence.is_valid(shell) {
                                    return;
                                }
                                let msg = match res {
                                    Ok(a) => format!(
                                        "{}: {}",
                                        tr_str(&lang_owned, "已处理工具审批", "Tool approval updated"),
                                        a.status
                                    ),
                                    Err(e) => format!(
                                        "{}: {}",
                                        tr_str(&lang_owned, "审批提交失败", "Approval decision failed"),
                                        e
                                    ),
                                };
                                if let Some(cid) = active_chat {
                                    if shell.active_panel == ActiveWorkspacePanel::Chats
                                        && !shell.sidebar.read(cx).is_plus_menu_open
                                        && shell.stage.read(cx).active_chat_id == Some(cid)
                                    {
                                        shell.show_chat_toast(cid, msg, 2400, cx);
                                    }
                                } else if shell.active_panel == captured_panel
                                    && shell.stage.read(cx).active_chat_id.is_none()
                                {
                                    shell.show_non_chat_toast(msg, 2400, cx);
                                }
                                shell.refresh_tasks_and_mcp(cx);
                            });
                        })
                        .detach();
                    }
                }
            }
        }
        if let Some(act) = bot_act {
            self.execute_hermes_inline_callback(act, cx);
        }
        if let Some((btn_msg_id, btn)) = bot_btn_act {
            self.handle_stage_bot_button(btn_msg_id, btn, cx);
        }
    }
}
