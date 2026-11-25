use std::path::PathBuf;

use gpui::{AsyncApp, Context, WeakEntity};
use gpui_telegram_client::net::RichSendMessageRequest;
use gpui_telegram_proto::contracts::chats::CreateChatRequest;
use gpui_telegram_proto::message::MessageContent as ProtoMessageContent;

use crate::i18n::tr_str;
use crate::views::app_shell::peer_avatar_color;
use crate::views::app_shell::shell_actions::resolve_active_target_chat_id;
use crate::views::app_shell::shell_truth::{
    derive_initials, evaluate_compose_send_decision, try_consume_composer_for_send,
    ComposeSendDecision,
};
use crate::views::boxes::modal_types::ModalDialogKind;
use crate::views::chat_sidebar::{
    ComposeCreationError, ComposeRecipientChip, DialogDeliveryStatus, DialogItemModel,
    PendingComposeCreation,
};
use crate::views::composer::voice_gesture::VoiceGestureState;
use crate::views::shell::AppShell;

impl AppShell {
    /// Routes a message submitted from the bottom composer while Apple/Grok compose mode is active.
    /// Guarantees zero leakage into any previously selected chat:
    /// - 0 recipients -> restores draft safely & emits toast
    /// - 1 recipient  -> opens/creates 1:1 DM and dispatches the message there
    /// - 2+ recipients -> mounts transient `"创建中…"` stand-in, creates group atomically,
    ///   selects the real group, and dispatches the opening message (triggering server bot routing).
    pub(crate) fn dispatch_compose_submitted_message(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id();
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        if !self.is_authenticated || self.session_sync.session_token.is_none() {
            self.composer.update(cx, |c, ccx| {
                if c.text().trim().is_empty() {
                    c.set_text(trimmed);
                }
                ccx.notify();
            });
            return;
        }

        let decision = {
            let sb = self.sidebar.read(cx);
            evaluate_compose_send_decision(sb, None)
        };

        match decision {
            ComposeSendDecision::BlockedEmptyRecipients => {
                self.composer.update(cx, |c, ccx| {
                    if c.text().trim().is_empty() {
                        c.set_text(trimmed);
                    }
                    c.is_input_focused = false;
                    ccx.notify();
                });
                self.sidebar.update(cx, |sb, scx| {
                    sb.compose_draft.menu_open = true;
                    sb.compose_title_focused = false;
                    scx.notify();
                });
                self.show_toast(
                    tr_str(
                        lang,
                        "请先选择至少一位联系人或 AI Bot",
                        "Please select at least one contact or AI Bot first",
                    ),
                    2600,
                    cx,
                );
                self.sync_compose_state_to_composer(true, cx);
            }
            ComposeSendDecision::BlockedPendingCreation => {
                self.composer.update(cx, |c, ccx| {
                    if c.text().trim().is_empty() {
                        c.set_text(trimmed);
                    }
                    ccx.notify();
                });
                self.show_toast(
                    tr_str(lang, "正在创建群聊，请稍候…", "Creating group, please wait…"),
                    2200,
                    cx,
                );
            }
            ComposeSendDecision::DirectMessage { recipient } => {
                self.dispatch_compose_single_dm_send(recipient, trimmed, cx);
            }
            ComposeSendDecision::CreateGroup {
                title,
                recipients,
                member_ids,
                member_usernames,
            } => {
                self.dispatch_compose_group_creation_and_send(
                    title,
                    recipients,
                    member_ids,
                    member_usernames,
                    trimmed,
                    cx,
                );
            }
        }
    }

    fn mark_compose_bot_chat_started(
        &mut self,
        chat_id: u64,
        recipient: &ComposeRecipientChip,
        cx: &mut Context<Self>,
    ) {
        if !recipient.is_bot && !self.is_chat_bot(chat_id, cx) {
            return;
        }
        self.miniapps.pending_prestart_bot = None;
        let mut keys: Vec<String> = Vec::new();
        if let Some(ref un) = recipient.username {
            let clean = un.trim().trim_start_matches('@').to_ascii_lowercase();
            if !clean.is_empty() {
                keys.push(clean);
            }
        }
        if let Some(rc) = self.session_sync.rich_chats_by_id.get(&chat_id) {
            if let Some(un) = rc.peer_username.as_deref().or(rc.username.as_deref()) {
                let clean = un.trim().trim_start_matches('@').to_ascii_lowercase();
                if !clean.is_empty() && !keys.contains(&clean) {
                    keys.push(clean);
                }
            }
        }
        for key in keys {
            if let Some(prof) = self.miniapps.bot_profiles_cache.get_mut(&key) {
                prof.has_started_chat = true;
                prof.existing_chat_id = Some(chat_id as i64);
            }
        }
        if recipient.user_id > 0 {
            for prof in self.miniapps.bot_profiles_cache.values_mut() {
                if prof.bot.bot_user_id == recipient.user_id as i64 {
                    prof.has_started_chat = true;
                    prof.existing_chat_id = Some(chat_id as i64);
                }
            }
        }
        self.stage.update(cx, |st, st_cx| {
            st.pending_bot_start = false;
            st.show_bot_start_bar = false;
            st_cx.notify();
        });
    }

    fn dispatch_compose_single_dm_send(
        &mut self,
        recipient: ComposeRecipientChip,
        initial_message: String,
        cx: &mut Context<Self>,
    ) {
        let existing_chat = recipient
            .chat_id
            .or_else(|| self.resolve_existing_dm_chat_id_for_user(recipient.user_id, cx));
        let has_local_dialog = existing_chat.is_some_and(|cid| {
            self.sidebar.read(cx).dialogs.iter().any(|d| d.id == cid)
        });

        if let Some(cid) = existing_chat.filter(|_| has_local_dialog) {
            self.sidebar.update(cx, |sb, scx| {
                sb.exit_compose_mode();
                sb.select_dialog_from_user(cid);
                scx.notify();
            });
            self.mark_compose_bot_chat_started(cid, &recipient, cx);
            self.sync_selected_chat_inspector_and_stage(cid, cx);
            self.finish_compose_isolation_for_switched_chat(cid, cx);
            self.mark_compose_bot_chat_started(cid, &recipient, cx);
            let focus_composer = self.can_focus_chat_composer(cx);
            self.composer.update(cx, |c, ccx| {
                c.clear();
                c.is_input_focused = focus_composer;
                ccx.notify();
            });
            self.append_outgoing_text(initial_message, cx);
            cx.notify();
            return;
        }

        let seq = self.client.next_client_nonce();
        let client_nonce = format!("dm-{}-{}", chrono::Utc::now().timestamp_millis(), seq);
        let auth_gen = self.session_sync.auth_generation;
        let title = recipient.display_name.clone();
        self.sidebar.update(cx, |sb, scx| {
            sb.compose_draft.menu_open = false;
            sb.compose_draft.creation_error = None;
            sb.compose_draft.pending_creation = Some(PendingComposeCreation {
                title: title.clone(),
                recipients: vec![recipient.clone()],
                initial_message: initial_message.clone(),
                client_nonce: client_nonce.clone(),
            });
            scx.notify();
        });
        self.composer.update(cx, |c, ccx| {
            c.clear();
            ccx.notify();
        });
        self.sync_compose_state_to_composer(true, cx);

        let req = CreateChatRequest {
            title: title.clone(),
            chat_type: Some(serde_json::json!(if recipient.is_bot { 4 } else { 0 })),
            peer_user_id: (recipient.user_id > 0).then_some(recipient.user_id),
            member_usernames: if recipient.user_id == 0 {
                recipient.username.clone().into_iter().collect()
            } else {
                Vec::new()
            },
            client_nonce: Some(client_nonce),
            ..Default::default()
        };
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = client.create_chat(&req).await;
            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated || shell.session_sync.auth_generation != auth_gen {
                    return;
                }
                match res {
                    Ok(created) => {
                        let cid = created.chat_id.max(0) as u64;
                        let final_title = if created.title.trim().is_empty() {
                            title.clone()
                        } else {
                            created.title.clone()
                        };
                        let now_label =
                            tr_str(shell.active_lang_id(), "刚刚", "Just now").to_string();
                        shell.sidebar.update(cx, |sb, scx| {
                            sb.compose_draft.pending_creation = None;
                            sb.exit_compose_mode();
                            if !sb.dialogs.iter().any(|d| d.id == cid) {
                                sb.dialogs.insert(
                                    0,
                                    DialogItemModel {
                                        id: cid,
                                        peer_name: final_title.clone(),
                                        avatar_initials: if recipient.initials.is_empty() {
                                            derive_initials(&final_title)
                                        } else {
                                            recipient.initials.clone()
                                        },
                                        avatar_color: recipient.avatar_color,
                                        last_message: String::new(),
                                        timestamp: now_label,
                                        unread_count: 0,
                                        is_muted: false,
                                        is_pinned: false,
                                        is_online: true,
                                        typing_indicator: None,
                                        delivery_status: DialogDeliveryStatus::Read,
                                        folder: if recipient.is_bot {
                                            "Bots".to_string()
                                        } else {
                                            "All".to_string()
                                        },
                                    },
                                );
                                if recipient.is_bot {
                                    sb.bot_ids.insert(cid);
                                }
                                sb.recompute_folder_badges();
                            }
                            sb.select_dialog_from_user(cid);
                            scx.notify();
                        });
                        if recipient.user_id > 0 {
                            shell
                                .session_sync
                                .chat_peer_user_ids
                                .insert(cid, recipient.user_id);
                        }
                        shell.mark_compose_bot_chat_started(cid, &recipient, cx);
                        shell.sync_selected_chat_inspector_and_stage(cid, cx);
                        shell.finish_compose_isolation_for_switched_chat(cid, cx);
                        shell.mark_compose_bot_chat_started(cid, &recipient, cx);
                        let focus_composer = shell.can_focus_chat_composer(cx);
                        shell.composer.update(cx, |c, ccx| {
                            c.clear();
                            c.is_input_focused = focus_composer;
                            ccx.notify();
                        });
                        shell.append_outgoing_text(initial_message, cx);
                        shell.refresh_chats(cx);
                        cx.notify();
                    }
                    Err(err) => {
                        let err_msg = format!(
                            "{}: {}",
                            tr_str(shell.active_lang_id(), "创建对话失败", "Failed to start chat"),
                            err
                        );
                        shell.sidebar.update(cx, |sb, scx| {
                            sb.compose_draft.pending_creation = None;
                            sb.is_plus_menu_open = true;
                            if sb.compose_draft.selected_recipients.is_empty() {
                                sb.compose_draft.selected_recipients = vec![recipient.clone()];
                            }
                            sb.compose_draft.creation_error = Some(ComposeCreationError {
                                message: err_msg.clone(),
                                failed_draft: initial_message.clone(),
                                recipients: vec![recipient.clone()],
                                title: title.clone(),
                            });
                            scx.notify();
                        });
                        shell.composer.update(cx, |c, ccx| {
                            if c.text().trim().is_empty() {
                                c.set_text(initial_message);
                            }
                            ccx.notify();
                        });
                        shell.sync_compose_state_to_composer(true, cx);
                        shell.show_toast(err_msg, 3400, cx);
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn dispatch_compose_group_creation_and_send(
        &mut self,
        title: String,
        recipients: Vec<ComposeRecipientChip>,
        member_ids: Vec<u64>,
        member_usernames: Vec<String>,
        initial_message: String,
        cx: &mut Context<Self>,
    ) {
        let seq = self.client.next_client_nonce();
        let client_nonce = format!("grp-{}-{}", chrono::Utc::now().timestamp_millis(), seq);
        let auth_gen = self.session_sync.auth_generation;

        // Mount transient `"创建中…"` sidebar stand-in only after submit commit.
        self.sidebar.update(cx, |sb, scx| {
            sb.compose_draft.menu_open = false;
            sb.compose_draft.creation_error = None;
            sb.compose_draft.pending_creation = Some(PendingComposeCreation {
                title: title.clone(),
                recipients: recipients.clone(),
                initial_message: initial_message.clone(),
                client_nonce: client_nonce.clone(),
            });
            scx.notify();
        });
        self.composer.update(cx, |c, ccx| {
            c.clear();
            ccx.notify();
        });
        self.sync_compose_state_to_composer(true, cx);

        let req = CreateChatRequest {
            title: title.clone(),
            chat_type: Some(serde_json::json!(1)),
            member_ids,
            member_usernames,
            client_nonce: Some(client_nonce),
            ..Default::default()
        };
        let client = self.client.clone();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = client.create_chat(&req).await;
            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated || shell.session_sync.auth_generation != auth_gen {
                    return;
                }
                match res {
                    Ok(created) => {
                        let cid = created.chat_id.max(0) as u64;
                        let final_title = if created.title.trim().is_empty() {
                            title.clone()
                        } else {
                            created.title.clone()
                        };
                        let now_label =
                            tr_str(shell.active_lang_id(), "刚刚", "Just now").to_string();
                        shell.sidebar.update(cx, |sb, scx| {
                            sb.compose_draft.pending_creation = None;
                            sb.exit_compose_mode();
                            if !sb.dialogs.iter().any(|d| d.id == cid) {
                                sb.dialogs.insert(
                                    0,
                                    DialogItemModel {
                                        id: cid,
                                        peer_name: final_title.clone(),
                                        avatar_initials: derive_initials(&final_title),
                                        avatar_color: peer_avatar_color(cid),
                                        last_message: String::new(),
                                        timestamp: now_label,
                                        unread_count: 0,
                                        is_muted: false,
                                        is_pinned: false,
                                        is_online: true,
                                        typing_indicator: None,
                                        delivery_status: DialogDeliveryStatus::Read,
                                        folder: "Groups".to_string(),
                                    },
                                );
                            }
                            sb.group_chat_ids.insert(cid);
                            sb.recompute_folder_badges();
                            sb.select_dialog_from_user(cid);
                            scx.notify();
                        });
                        shell.sync_selected_chat_inspector_and_stage(cid, cx);
                        shell.finish_compose_isolation_for_switched_chat(cid, cx);
                        let focus_composer = shell.can_focus_chat_composer(cx);
                        shell.composer.update(cx, |c, ccx| {
                            c.clear();
                            c.is_input_focused = focus_composer;
                            ccx.notify();
                        });
                        // Immediately dispatch the user's opening message into the newly created group
                        // so server-side multi-bot routing wakes any included AI Bots.
                        shell.append_outgoing_text(initial_message, cx);
                        shell.refresh_chats(cx);
                        cx.notify();
                    }
                    Err(err) => {
                        let err_msg = format!(
                            "{}: {}",
                            tr_str(shell.active_lang_id(), "创建群组失败", "Failed to create group"),
                            err
                        );
                        shell.sidebar.update(cx, |sb, scx| {
                            sb.compose_draft.pending_creation = None;
                            sb.is_plus_menu_open = true;
                            sb.compose_draft.is_group_mode = true;
                            if sb.compose_draft.selected_recipients.is_empty() {
                                sb.compose_draft.selected_recipients = recipients.clone();
                            }
                            sb.compose_draft.creation_error = Some(ComposeCreationError {
                                message: err_msg.clone(),
                                failed_draft: initial_message.clone(),
                                recipients: recipients.clone(),
                                title: title.clone(),
                            });
                            scx.notify();
                        });
                        shell.composer.update(cx, |c, ccx| {
                            if c.text().trim().is_empty() {
                                c.set_text(initial_message);
                            }
                            ccx.notify();
                        });
                        shell.sync_compose_state_to_composer(true, cx);
                        shell.show_toast(err_msg, 3600, cx);
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Retries a failed group/DM creation directly from the inline `izn` error recovery banner.
    pub(crate) fn retry_compose_group_creation(&mut self, cx: &mut Context<Self>) {
        let Some(err) = self.sidebar.update(cx, |sb, scx| {
            let e = sb.compose_draft.creation_error.take();
            if let Some(ref info) = e {
                if sb.compose_draft.selected_recipients.is_empty() {
                    sb.compose_draft.selected_recipients = info.recipients.clone();
                }
            }
            scx.notify();
            e
        }) else {
            return;
        };
        let current_composer_text = self.composer.read(cx).text().trim().to_string();
        let retry_text = if !current_composer_text.is_empty() {
            current_composer_text
        } else {
            err.failed_draft
        };
        if retry_text.trim().is_empty() {
            let lang = self.active_lang_id();
            self.show_toast(
                tr_str(lang, "请输入首条消息后重试", "Enter a message to retry"),
                2400,
                cx,
            );
            self.set_composer_focused(true, cx);
            return;
        }
        self.dispatch_compose_submitted_message(retry_text, cx);
    }

    pub fn send_rich_text_options(
        &mut self,
        text: String,
        silent: bool,
        scheduled_at_ms: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        if self.theme_state.palette.is_apple_style() && self.sidebar.read(cx).is_plus_menu_open {
            self.dispatch_compose_submitted_message(text, cx);
            return;
        }
        if self.try_handle_local_vault_command(&text, cx) {
            return;
        }
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        let Some(chat_id) = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        ) else {
            return;
        };
        let send_as_peer_id = self
            .session_sync
            .send_as_peers
            .chosen(chat_id)
            .or_else(|| self.composer.read(cx).selected_send_as_peer_id);
        let req = RichSendMessageRequest {
            chat_id: chat_id as i64,
            text: Some(trimmed.clone()),
            content: Some(ProtoMessageContent::plain_text(trimmed)),
            entities: Vec::new(),
            reply_to: None,
            quote_text: None,
            topic_id: None,
            client_nonce: Some(self.client.next_client_nonce()),
            forward_from_chat_id: None,
            forward_from_msg_id: None,
            silent,
            scheduled_at_ms,
            inline_keyboard: None,
            send_as_peer_id,
            skip_server_bot_reply: false,
        };
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let _ = client.send_message_durable(req).await;
            let _ = weak.update(app, |s, cx| s.load_historical_messages(chat_id, cx));
        })
        .detach();
    }

    pub fn upload_and_send_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.theme_state.palette.is_apple_style() && self.sidebar.read(cx).is_plus_menu_open {
            let lang = self.active_lang_id();
            self.show_toast(
                tr_str(
                    lang,
                    "请先发送首条消息创建对话后再发送附件",
                    "Create the chat with a message first before uploading attachments",
                ),
                2600,
                cx,
            );
            return;
        }
        let Some(chat_id) = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        ) else {
            return;
        };
        let send_as_peer_id = self
            .session_sync
            .send_as_peers
            .chosen(chat_id)
            .or_else(|| self.composer.read(cx).selected_send_as_peer_id);
        let client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            for path in paths {
                let Ok(bytes) = tokio::fs::read(&path).await else {
                    continue;
                };
                let filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("attachment.bin")
                    .to_string();
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let (mime, kind) = match ext.as_str() {
                    "png" => ("image/png", "photo"),
                    "jpg" | "jpeg" | "webp" | "gif" => ("image/jpeg", "photo"),
                    "mp4" | "mov" | "webm" => ("video/mp4", "video"),
                    _ => ("application/octet-stream", "doc"),
                };
                if let Ok(up) = client.upload_media(&filename, mime, bytes).await {
                    let content = match kind {
                        "photo" => ProtoMessageContent::Photo {
                            file_id: up.file_id.clone(),
                            caption: None,
                            width: up.width.unwrap_or(0),
                            height: up.height.unwrap_or(0),
                        },
                        "video" => ProtoMessageContent::Video {
                            file_id: up.file_id.clone(),
                            duration: up.duration_seconds.unwrap_or(0),
                            width: up.width.unwrap_or(0),
                            height: up.height.unwrap_or(0),
                            caption: None,
                        },
                        _ => ProtoMessageContent::Document {
                            file_id: up.file_id.clone(),
                            file_name: filename.clone(),
                            mime_type: mime.to_string(),
                            size_bytes: up.size_bytes,
                        },
                    };
                    let req = RichSendMessageRequest {
                        chat_id: chat_id as i64,
                        text: Some(filename),
                        content: Some(content),
                        entities: Vec::new(),
                        reply_to: None,
                        quote_text: None,
                        topic_id: None,
                        client_nonce: Some(client.next_client_nonce()),
                        forward_from_chat_id: None,
                        forward_from_msg_id: None,
                        silent: false,
                        scheduled_at_ms: None,
                        inline_keyboard: None,
                        send_as_peer_id,
                        skip_server_bot_reply: false,
                    };
                    let _ = client.send_message_durable(req).await;
                }
            }
            let _ = weak.update(app, |s, cx| s.load_historical_messages(chat_id, cx));
        })
        .detach();
    }

    pub fn handle_composer_pending_actions(&mut self, cx: &mut Context<Self>) {
        let lang = self.active_lang_id();
        let is_apple_compose_mode =
            self.theme_state.palette.is_apple_style() && self.sidebar.read(cx).is_plus_menu_open;
        if is_apple_compose_mode {
            let (
                compose_submitted,
                compose_silent,
                compose_sched,
                compose_sticker,
                compose_dice,
                compose_gif,
            ) = self.composer.update(cx, |c, _| {
                let _ = c.pending_attached_files.take();
                (
                    c.submitted_message
                        .take()
                        .filter(|s| !s.trim().is_empty()),
                    c.pending_send_silent.take(),
                    c.pending_schedule_message.take(),
                    c.pending_send_sticker_emoji.take(),
                    c.pending_send_dice.take(),
                    c.pending_send_gif.take(),
                )
            });
            if let Some(text) = compose_submitted
                .or(compose_silent)
                .or(compose_sched)
                .or(compose_sticker)
                .or(compose_dice)
                .or_else(|| compose_gif.map(|(_, t)| t))
            {
                self.dispatch_compose_submitted_message(text, cx);
            }
            return;
        }

        let active_chat = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        );
        let is_authenticated = self.is_authenticated;
        let session_token = self.session_sync.session_token.clone();
        let user_id = self.session_sync.user_id;
        let submitted = self.sidebar.update(cx, |sb, sb_cx| {
            self.stage.update(sb_cx, |st, st_cx| {
                self.composer.update(st_cx, |c, _| {
                    try_consume_composer_for_send(
                        is_authenticated,
                        session_token.as_deref(),
                        user_id,
                        sb,
                        st,
                        c,
                    )
                    .map(|(_, text)| text)
                })
            })
        });
        let (
            silent_text,
            sched_text,
            edit_commit,
            attached_files,
            sticker,
            dice,
            gif,
            open_poll,
            open_loc,
            open_contact,
            is_rec_ui,
            is_rec_done,
        ) = self.composer.update(cx, |c, _| {
            let rec_ui = c.is_recording_voice();
            let rec_done = matches!(c.voice_gesture.state, VoiceGestureState::Completed { .. });
            if rec_done || matches!(c.voice_gesture.state, VoiceGestureState::Discarded) {
                c.voice_gesture.state = VoiceGestureState::Idle;
            }
            (
                c.pending_send_silent.take(),
                c.pending_schedule_message.take(),
                c.pending_edit_commit.take(),
                c.pending_attached_files.take(),
                c.pending_send_sticker_emoji.take(),
                c.pending_send_dice.take(),
                c.pending_send_gif.take(),
                if c.pending_open_poll_modal {
                    c.pending_open_poll_modal = false;
                    true
                } else {
                    false
                },
                if c.pending_open_location_modal {
                    c.pending_open_location_modal = false;
                    true
                } else {
                    false
                },
                if c.pending_open_contact_modal {
                    c.pending_open_contact_modal = false;
                    true
                } else {
                    false
                },
                rec_ui,
                rec_done,
            )
        });

        if is_rec_ui && !self.rtc_talk.voice_recorder.is_recording() {
            self.toggle_voice_recording(true, cx);
        } else if !is_rec_ui && self.rtc_talk.voice_recorder.is_recording() {
            if is_rec_done {
                self.toggle_voice_recording(false, cx);
            } else {
                let _ = self.rtc_talk.voice_recorder.stop();
            }
        }
        if let Some(text) = submitted {
            self.append_outgoing_text(text, cx);
        }
        if let Some(text) = silent_text {
            self.send_rich_text_options(text, true, None, cx);
        }
        if let Some(text) = sched_text {
            let sched_ms = chrono::Utc::now().timestamp_millis() + 60_000;
            self.send_rich_text_options(text, false, Some(sched_ms), cx);
        }
        if let Some((mid, new_text)) = edit_commit {
            if let Some(cid) = active_chat {
                self.stage
                    .update(cx, |st, _| st.edit_message_text(mid, new_text.clone()));
                let client = self.client.clone();
                cx.spawn(async move |_: WeakEntity<Self>, _: &mut AsyncApp| {
                    let _ = client
                        .edit_message(cid as i64, mid as i64, new_text, Vec::new())
                        .await;
                })
                .detach();
            }
        }
        if let Some(paths) = attached_files {
            self.upload_and_send_paths(paths, cx);
        }
        if let Some(emoji) = sticker {
            self.append_outgoing_text(emoji, cx);
        }
        if let Some(d) = dice {
            self.append_outgoing_text(d, cx);
        }
        if let Some((_id, title)) = gif {
            self.append_outgoing_text(title, cx);
        }
        if open_poll {
            self.open_modal_dialog(
                tr_str(lang, "创建投票", "Create Poll"),
                ModalDialogKind::CreatePoll,
                420.0,
                480.0,
                cx,
            );
        }
        if open_loc {
            self.show_toast(
                tr_str(lang, "请选择地图位置并发送", "Select a location to share"),
                2200,
                cx,
            );
        }
        if open_contact {
            self.refresh_contacts(cx);
            self.open_modal_dialog(
                tr_str(lang, "分享联系人", "Share Contact"),
                ModalDialogKind::Contacts,
                420.0,
                500.0,
                cx,
            );
        }
    }
}
