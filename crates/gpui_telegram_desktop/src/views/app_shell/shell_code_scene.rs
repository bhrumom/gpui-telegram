//! `AppShell` integration adapter for the Code Scene ("代码场景") workspace (`CS-F15`).
//!
//! Mirrors `shell_bot_browser.rs` lifecycle semantics: lazy entity initialization,
//! clean pause & telemetry flush when leaving or occluded, chat `GenerativeCard`
//! callback dispatch (`code_scene:open/remix/share:<id>`), and Share-to-Chat card
//! forwarding into any Telegram conversation.

use std::sync::Arc;

use gpui::{AppContext, AsyncApp, Context, Entity, WeakEntity};
use gpui_telegram_agent::code_scene_bot::build_code_scene_generative_card;
use gpui_telegram_client::RichSendMessageRequest;
use gpui_telegram_proto::code_scene_contracts::{CodeSceneAssetDto, CodeSceneCardAction};

use crate::i18n::tr_str;
use crate::views::app_shell::shell_actions::{
    chat_dialogs_to_forward_targets, peer_avatar_color, resolve_active_target_chat_id,
};
use crate::views::app_shell::shell_truth::derive_initials;
use crate::views::code_scene::{
    CodeScenePendingAction, CodeSceneWorkspace, CodeSceneWorkspaceConfig,
};
use crate::views::nav_rail::NavDestination;
use crate::views::shell::{ActiveWorkspacePanel, AppShell};
use crate::views::stage::cards::{
    DeliveryStatus, MessageContent, MessageDirection, MessageItemModel,
};

impl AppShell {
    /// Opens or focuses the full-pane `CodeSceneWorkspace` sibling beside the left sidebar.
    pub fn open_code_scene_workspace(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Entity<CodeSceneWorkspace> {
        let was_outside = self.active_panel != ActiveWorkspacePanel::CodeScene;
        if was_outside {
            self.ime.clear_preedit();
            self.bot_browser.ime_preedit_input_gen = None;
        }
        self.hide_browser_native_webview(cx);
        self.active_panel = ActiveWorkspacePanel::CodeScene;
        self.last_handled_nav_dest = Some(NavDestination::CodeScene);
        self.nav_rail.update(cx, |r, rcx| {
            r.set_active_destination(NavDestination::CodeScene);
            rcx.notify();
        });
        self.sidebar.update(cx, |sb, scx| {
            if sb.search_focused {
                sb.search_focused = false;
                scx.notify();
            }
            sb.is_account_popover_open = false;
        });
        self.composer.update(cx, |c, ccx| {
            if c.is_input_focused {
                c.is_input_focused = false;
                ccx.notify();
            }
        });

        let pal = self.theme_state.palette;
        let lang = self.active_lang_id().to_string();
        let token = self.session_sync.session_token.clone();
        let share_targets = chat_dialogs_to_forward_targets(&self.sidebar.read(cx).dialogs);

        let cs = if let Some(existing) = self.code_scene_workspace.clone() {
            existing.update(cx, |w, wcx| {
                w.set_session_token(token);
                w.sync_share_targets(share_targets);
                w.sync_theme_and_locale(pal, lang, wcx);
                w.set_occluded(false, wcx);
                w.set_visible(true, wcx);
            });
            existing
        } else {
            let service = Arc::new(self.client.build_code_scene_service());
            let cfg = CodeSceneWorkspaceConfig {
                service,
                tokio_handle: Some(self.client.runtime().handle().clone()),
                session_token: token,
                palette: pal,
                language_id: lang,
            };
            let created = cx.new(|wcx| {
                let mut w = CodeSceneWorkspace::new(cfg, wcx);
                w.sync_share_targets(share_targets);
                w.trigger_background_sync(wcx);
                w
            });
            cx.observe(&created, |shell, ws, cx| {
                let pending = ws.update(cx, |w, _| w.take_pending_action());
                if let Some(act) = pending {
                    shell.handle_code_scene_pending_action(act, cx);
                }
            })
            .detach();
            self.code_scene_workspace = Some(created.clone());
            created
        };
        self.reconcile_chat_scoped_toasts(cx);
        cx.notify();
        cs
    }

    /// Pauses active Code Scene playback and flushes behavioral telemetry when leaving the panel.
    pub fn leave_code_scene_panel_if_active(&mut self, cx: &mut Context<Self>) {
        if let Some(ref cs) = self.code_scene_workspace {
            cs.update(cx, |w, wcx| {
                w.set_visible(false, wcx);
            });
        }
    }

    /// Temporarily pauses Code Scene VM when a shell modal/overlay occludes the stage.
    pub fn pause_code_scene_for_overlay(&mut self, cx: &mut Context<Self>) {
        if let Some(ref cs) = self.code_scene_workspace {
            cs.update(cx, |w, wcx| {
                w.set_occluded(true, wcx);
            });
        }
    }

    /// Deep-links directly into Code Scene focused on `scene_id`.
    pub fn open_code_scene_by_id(&mut self, scene_id: &str, cx: &mut Context<Self>) {
        let cs = self.open_code_scene_workspace(cx);
        cs.update(cx, |w, wcx| w.open_scene_by_id(scene_id, wcx));
    }

    /// Opens Code Scene AI Bot Studio pre-seeded with `scene_id` for remixing.
    pub fn open_code_scene_remix(&mut self, scene_id: &str, cx: &mut Context<Self>) {
        let cs = self.open_code_scene_workspace(cx);
        cs.update(cx, |w, wcx| w.open_remix_studio_for_scene(scene_id, wcx));
    }

    /// Opens Share-to-Chat sheet focused on `scene_id`.
    pub fn open_code_scene_share(&mut self, scene_id: &str, cx: &mut Context<Self>) {
        let cs = self.open_code_scene_workspace(cx);
        cs.update(cx, |w, wcx| w.open_share_drawer_for_scene(scene_id, wcx));
    }

    /// Dispatches a parsed chat `GenerativeCard` or inline keyboard `code_scene:*` callback action.
    pub fn handle_code_scene_card_action(
        &mut self,
        action: CodeSceneCardAction,
        cx: &mut Context<Self>,
    ) {
        match action {
            CodeSceneCardAction::Open(scene_id) => self.open_code_scene_by_id(&scene_id, cx),
            CodeSceneCardAction::Remix(scene_id) => self.open_code_scene_remix(&scene_id, cx),
            CodeSceneCardAction::Share(scene_id) => self.open_code_scene_share(&scene_id, cx),
        }
    }

    fn handle_code_scene_pending_action(
        &mut self,
        action: CodeScenePendingAction,
        cx: &mut Context<Self>,
    ) {
        match action {
            CodeScenePendingAction::ExitToChats => {
                self.leave_code_scene_panel_if_active(cx);
                self.active_panel = ActiveWorkspacePanel::Chats;
                self.last_handled_nav_dest = Some(NavDestination::Chats);
                self.nav_rail.update(cx, |r, rcx| {
                    r.set_active_destination(NavDestination::Chats);
                    rcx.notify();
                });
                cx.notify();
            }
            CodeScenePendingAction::ToggleSidebarCollapse => {
                self.sidebar.update(cx, |sb, scx| {
                    sb.toggle_collapsed_from_user();
                    scx.notify();
                });
                cx.notify();
            }
            CodeScenePendingAction::ShareSceneToChat { chat_id, asset } => {
                self.share_code_scene_to_chat(chat_id, asset, cx);
            }
            CodeScenePendingAction::SharePublishedToActiveChat { asset } => {
                if let Some(chat_id) = resolve_active_target_chat_id(
                    self.stage.read(cx).active_chat_id,
                    self.sidebar.read(cx).active_chat_id,
                ) {
                    self.share_code_scene_to_chat(chat_id, asset, cx);
                } else {
                    let msg = format!(
                        "{}「{}」",
                        tr_str(self.active_lang_id(), "已发布到代码场景：", "Published to Code Scene: "),
                        asset.title
                    );
                    self.show_non_chat_toast(msg, 2400, cx);
                }
            }
            CodeScenePendingAction::NotifyToast(msg) => {
                self.show_non_chat_toast(msg, 2400, cx);
            }
        }
    }

    /// Sends an interactive Code Scene `GenerativeCard` message into `chat_id`.
    pub fn share_code_scene_to_chat(
        &mut self,
        chat_id: u64,
        asset: CodeSceneAssetDto,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id().to_string();
        let card = build_code_scene_generative_card(&asset);
        let fallback_text = format!(
            "🎬 {} · {} (@{})",
            tr_str(&lang, "代码场景", "Code Scene"),
            asset.title,
            asset.author_username.trim_start_matches('@')
        );

        // Optimistically insert the GenerativeCard into the active chat stage if it matches `chat_id`.
        if self.stage.read(cx).active_chat_id == Some(chat_id) {
            let sender_id = self.session_sync.user_id.unwrap_or(0);
            let sender_name = self.session_sync.username.clone().unwrap_or_default();
            let avatar_initials = derive_initials(if sender_name.is_empty() {
                "Me"
            } else {
                &sender_name
            });
            let avatar_color = peer_avatar_color(sender_id);
            let optimistic_id = (chrono::Utc::now().timestamp_millis().unsigned_abs() % 1_000_000_000)
                .saturating_add(1);
            let msg_model = MessageItemModel {
                id: optimistic_id,
                sender_id,
                sender_name,
                avatar_initials,
                avatar_color,
                direction: MessageDirection::Outgoing,
                timestamp: chrono::Local::now().format("%H:%M").to_string(),
                delivery_status: DeliveryStatus::Sending,
                reply_to: None,
                reactions: Vec::new(),
                content: MessageContent::GenerativeCard(card.clone()),
                bot_keyboard: None,
                is_pinned: false,
            };
            self.stage.update(cx, |st, stcx| {
                st.messages.push(msg_model);
                stcx.notify();
            });
        }

        if let Some(fence) = self.capture_client_fence() {
            let proto_content = Some(gpui_telegram_proto::MessageContent::GenerativeCard(card));
            let title = asset.title.clone();
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                if !fence.preflight_ui(&weak, app).await {
                    return;
                }
                let send_client = fence.client.clone();
                let exp_epoch = fence.client_epoch;
                let exp_tok = fence.token.clone();
                let req = RichSendMessageRequest {
                    chat_id: chat_id as i64,
                    text: Some(fallback_text),
                    content: proto_content,
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
                    skip_server_bot_reply: true,
                };
                let res = fence
                    .client
                    .runtime()
                    .spawn(async move {
                        if send_client
                            .ensure_pinned_session(exp_epoch, &exp_tok)
                            .is_err()
                        {
                            return Err("session changed".to_string());
                        }
                        send_client
                            .send_message_durable_with_token(&exp_tok, exp_epoch, req)
                            .await
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                    })
                    .await;
                let _ = weak.update(app, |shell, cx| {
                    if !fence.is_valid(shell) {
                        return;
                    }
                    let msg = match res {
                        Ok(Ok(())) => format!(
                            "{}「{title}」",
                            tr_str(&lang, "已转发代码场景卡片：", "Shared Code Scene card: ")
                        ),
                        _ => tr_str(&lang, "已保存分享卡片（离线队列）", "Card queued for sharing")
                            .to_string(),
                    };
                    shell.show_non_chat_toast(msg, 2400, cx);
                    shell.refresh_chats(cx);
                });
            })
            .detach();
        } else {
            self.show_non_chat_toast(
                format!(
                    "{}「{}」",
                    tr_str(&lang, "已转发代码场景卡片：", "Shared Code Scene card: "),
                    asset.title
                ),
                2400,
                cx,
            );
        }
    }
}
