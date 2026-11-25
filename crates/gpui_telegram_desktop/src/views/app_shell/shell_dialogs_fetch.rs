//! HTTP dialog/history fetch workflows, contact/group creation, categorized search,
//! and attachment dispatch helpers for `AppShell`.

use std::collections::HashSet;

use gpui::{AsyncApp, Context, WeakEntity};
use gpui_telegram_client::net::http_client::AddContactHttpRequest;

use crate::i18n::tr_str;
use crate::views::app_shell::shell_actions::map_stage_media_to_gallery;
use crate::views::app_shell::shell_truth::{
    apply_fenced_chat_list_response, apply_fenced_history_response_preserving,
    build_cached_history_models, derive_initials, overlay_rich_message_metadata,
    SessionRequestFence,
};
use crate::views::boxes::modal_types::ModalDialogKind;
use crate::views::chat_sidebar::{
    BotGrokAvatarVisual, CategorizedSearchResult, DialogItemModel, PendingAddContactRequest,
    PendingCreateGroupRequest,
};
use crate::views::shell::{ActiveWorkspacePanel, AppShell};

impl AppShell {
    /// Dispatches `POST /api/v1/contacts/add` for adding a friend or creating an AI bot dialog.
    pub fn dispatch_add_contact(&mut self, req: PendingAddContactRequest, cx: &mut Context<Self>) {
        let Some(fence) = self.capture_client_fence() else {
            return;
        };
        let http_req = AddContactHttpRequest {
            first_name: req.first_name.clone(),
            last_name: req.last_name.clone(),
            username: req.username.clone(),
            phone: req.phone.clone(),
            is_bot: req.is_bot,
            bot_system_prompt: req.system_prompt.clone(),
        };
        let local_dialog_id = req.local_dialog_id;
        let is_bot = req.is_bot;
        let avatar_shape = req.avatar_shape.clone();
        let avatar_color = req.avatar_color.clone();

        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak_shell, app).await {
                return;
            }
            let res = fence
                .client
                .http_client
                .add_contact_with_avatar(&fence.token, http_req, avatar_shape, avatar_color)
                .await;
            let _ = weak_shell.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                if let Ok(resp) = res {
                    let server_chat_id = resp.chat_id;
                    let mut selected_peer: Option<DialogItemModel> = None;
                    shell.sidebar.update(cx, |sb, sb_cx| {
                        if let Some(d) = sb.dialogs.iter_mut().find(|d| d.id == local_dialog_id) {
                            d.id = server_chat_id;
                            d.peer_name = resp.title.clone();
                            d.avatar_color = resp.avatar_color.unwrap_or(0x2AABEE);
                            selected_peer = Some(d.clone());
                        }
                        if sb.active_chat_id == Some(local_dialog_id) {
                            sb.active_chat_id = Some(server_chat_id);
                        }
                        if sb.bot_ids.remove(&local_dialog_id) {
                            sb.bot_ids.insert(server_chat_id);
                        }
                        if let Some(mut grok_vis) = sb.bot_grok_avatars.remove(&local_dialog_id) {
                            if let Some(ref srv_shape) = resp.avatar_shape {
                                grok_vis.shape = srv_shape.clone();
                            }
                            if let Some(ref srv_col) = resp.avatar_color_id {
                                grok_vis.color = srv_col.clone();
                            }
                            if !resp.username.is_empty() {
                                grok_vis.agent_id = resp.username.clone();
                            }
                            sb.bot_grok_avatars.insert(server_chat_id, grok_vis);
                        } else if let Some(ref srv_shape) = resp.avatar_shape {
                            if !srv_shape.eq_ignore_ascii_case("classic") {
                                let color_str = resp.avatar_color_id.clone().unwrap_or_else(|| {
                                    resp.avatar_color
                                        .map(|c| format!("#{:06X}", c & 0xFF_FFFF))
                                        .unwrap_or_else(|| "blue".to_string())
                                });
                                sb.bot_grok_avatars.insert(
                                    server_chat_id,
                                    BotGrokAvatarVisual {
                                        agent_id: if resp.username.is_empty() {
                                            server_chat_id.to_string()
                                        } else {
                                            resp.username.clone()
                                        },
                                        shape: srv_shape.clone(),
                                        color: color_str,
                                        execution_state: "idle".to_string(),
                                    },
                                );
                            } else {
                                sb.bot_grok_avatars.remove(&server_chat_id);
                            }
                        }
                        if let Some(fab_vis) = sb.bot_fabushi_visuals.remove(&local_dialog_id) {
                            sb.bot_fabushi_visuals.insert(server_chat_id, fab_vis);
                        }
                        sb_cx.notify();
                    });
                    if let Some(d) = selected_peer {
                        shell.stage.update(cx, |st, st_cx| {
                            st.set_chat(
                                server_chat_id,
                                &d.peer_name,
                                "online",
                                &d.avatar_initials,
                                d.avatar_color,
                            );
                            st_cx.notify();
                        });
                    }
                    shell.reconcile_chat_scoped_toasts(cx);
                    if shell.stage.read(cx).active_chat_id == Some(server_chat_id) {
                        shell.load_historical_messages(server_chat_id, cx);
                    }
                    if is_bot {
                        shell.refresh_miniapps_catalog(None, cx);
                    }
                    cx.notify();
                } else if let Err(err) = res {
                    // Roll back the optimistic local dialog so non-existent/unverified contacts never appear as fake data.
                    shell.sidebar.update(cx, |sb, sb_cx| {
                        sb.dialogs.retain(|d| d.id != local_dialog_id);
                        sb.bot_ids.remove(&local_dialog_id);
                        sb.bot_grok_avatars.remove(&local_dialog_id);
                        sb.bot_fabushi_visuals.remove(&local_dialog_id);
                        if sb.active_chat_id == Some(local_dialog_id) {
                            sb.active_chat_id = sb.dialogs.first().map(|d| d.id);
                            sb.selection_revision = sb.selection_revision.wrapping_add(1);
                        }
                        sb_cx.notify();
                    });
                    let lang = shell.active_lang_id();
                    let msg = if !is_bot {
                        tr_str(
                            lang,
                            "未找到该注册用户，请核对对方的用户名或手机号",
                            "User not found. Please check the username or phone number.",
                        )
                        .to_string()
                    } else {
                        format!(
                            "{}: {}",
                            tr_str(lang, "添加联系人失败", "Failed to add contact"),
                            err
                        )
                    };
                    shell.show_non_chat_toast(msg, 3200, cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Dispatches `POST /api/v1/chats/create` to materialize a newly created group chat.
    pub fn dispatch_create_group(
        &mut self,
        req: PendingCreateGroupRequest,
        cx: &mut Context<Self>,
    ) {
        let Some(fence) = self.capture_client_fence() else {
            return;
        };
        let title = req.title.clone();
        let member_ids = req.member_ids.clone();
        let member_usernames = req.member_usernames.clone();
        let about = req.about.clone();
        let local_dialog_id = req.local_dialog_id;

        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak_shell, app).await {
                return;
            }
            let res = fence
                .client
                .http_client
                .create_group_with_token(&fence.token, &title, member_ids, member_usernames, about)
                .await;
            let _ = weak_shell.update(app, |shell, cx| {
                if !fence.is_valid(shell) {
                    return;
                }
                if let Ok(resp) = res {
                    let server_chat_id = resp.chat_id.max(0) as u64;
                    let mut selected_peer: Option<DialogItemModel> = None;
                    shell.sidebar.update(cx, |sb, sb_cx| {
                        if let Some(d) = sb.dialogs.iter_mut().find(|d| d.id == local_dialog_id) {
                            d.id = server_chat_id;
                            selected_peer = Some(d.clone());
                        }
                        if sb.active_chat_id == Some(local_dialog_id) {
                            sb.active_chat_id = Some(server_chat_id);
                        }
                        sb_cx.notify();
                    });
                    if let Some(d) = selected_peer {
                        shell.stage.update(cx, |st, st_cx| {
                            st.set_chat(
                                server_chat_id,
                                &d.peer_name,
                                "group",
                                &d.avatar_initials,
                                d.avatar_color,
                            );
                            st_cx.notify();
                        });
                    }
                    shell.reconcile_chat_scoped_toasts(cx);
                    if shell.stage.read(cx).active_chat_id == Some(server_chat_id) {
                        shell.load_historical_messages(server_chat_id, cx);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Dispatches `GET /api/v1/search/messages?q=...&category=...` for categorized sidebar search.
    pub fn dispatch_categorized_search(
        &mut self,
        query: String,
        category: String,
        search_seq: u64,
        cx: &mut Context<Self>,
    ) {
        self.session_sync.last_dispatched_search_seq = search_seq;
        if category.eq_ignore_ascii_case("Apps") {
            let q_opt = (!query.trim().is_empty()).then(|| query.clone());
            self.refresh_miniapps_catalog(q_opt, cx);
        }
        let Some(fence) = self.capture_client_fence() else {
            return;
        };

        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak_shell, app).await {
                return;
            }
            let res = fence
                .client
                .http_client
                .search_global(&fence.token, &query, &category)
                .await;
            let _ = weak_shell.update(app, |shell, cx| {
                if !fence.is_valid(shell)
                    || shell.sidebar.read(cx).search_request_seq != search_seq
                {
                    return;
                }
                if let Ok(hits) = res {
                    let mapped: Vec<CategorizedSearchResult> = hits
                        .into_iter()
                        .map(|hit| {
                            let title = if hit.title.is_empty() {
                                format!("Chat #{}", hit.chat_id)
                            } else {
                                hit.title
                            };
                            let initials =
                                title.chars().take(2).collect::<String>().to_uppercase();
                            let cat = if hit.category.is_empty() {
                                category.clone()
                            } else {
                                hit.category
                            };
                            CategorizedSearchResult {
                                chat_id: hit.chat_id,
                                message_id: hit.message_id,
                                category: cat,
                                title,
                                subtitle: hit.snippet,
                                timestamp: "刚刚".to_string(),
                                avatar_initials: if initials.is_empty() {
                                    "TG".to_string()
                                } else {
                                    initials
                                },
                                avatar_color: hit.avatar_color.filter(|&c| c != 0).unwrap_or(0x2AABEE),
                                is_bot: hit.is_bot,
                            }
                        })
                        .collect();
                    shell.sidebar.update(cx, |sb, sb_cx| {
                        sb.remote_search_results = mapped;
                        sb_cx.notify();
                    });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Refreshes the authoritative chat list from `GET /api/v1/chats` and enriches bot/peer metadata.
    pub fn refresh_chats(&mut self, cx: &mut Context<Self>) {
        let (Some(token), Some(uid)) = (
            self.session_sync.session_token.clone(),
            self.session_sync.user_id,
        ) else {
            return;
        };
        if !self.is_authenticated {
            return;
        }
        let Some(call_fence) = self.capture_client_fence() else {
            return;
        };
        self.session_sync.chat_list_request_generation =
            self.session_sync.chat_list_request_generation.wrapping_add(1);
        self.session_sync.chat_list_in_flight_since = Some(std::time::Instant::now());

        let show_loading_skeleton = {
            let sb = self.sidebar.read(cx);
            sb.dialogs.is_empty() && !sb.has_synced_dialogs
        };
        if show_loading_skeleton {
            self.sidebar.update(cx, |sb, sb_cx| {
                if !sb.is_loading_dialogs {
                    sb.is_loading_dialogs = true;
                    sb_cx.notify();
                }
            });
            self.stage.update(cx, |st, st_cx| {
                if st.active_chat_id.is_none() && !st.is_loading_workspace {
                    st.is_loading_workspace = true;
                    st_cx.notify();
                }
            });
        }

        let fence = SessionRequestFence {
            auth_generation: self.session_sync.auth_generation,
            request_generation: self.session_sync.chat_list_request_generation,
            token,
            user_id: uid,
        };

        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            if !call_fence.preflight_ui(&weak_shell, app).await {
                return;
            }
            let (res, rich_res) = futures::join!(
                call_fence.client.http_client.get_chats(&call_fence.token),
                call_fence.client.hermes.list_chats_rich(&call_fence.token),
            );
            let rich_chats = rich_res.ok();
            let _ = weak_shell.update(app, |shell, cx| {
                if !call_fence.is_valid(shell) {
                    return;
                }
                if shell.session_sync.chat_list_request_generation == fence.request_generation {
                    shell.session_sync.chat_list_in_flight_since = None;
                }
                match res {
                    Ok(chats) => {
                        let applied = shell.sidebar.update(cx, |sb, sb_cx| {
                            let ok = shell.stage.update(sb_cx, |st, st_cx| {
                                let applied = shell.inspector.update(st_cx, |insp, insp_cx| {
                                    let applied = apply_fenced_chat_list_response(
                                        &fence,
                                        shell.is_authenticated,
                                        shell.session_sync.auth_generation,
                                        shell.session_sync.chat_list_request_generation,
                                        shell.session_sync.session_token.as_deref(),
                                        shell.session_sync.user_id,
                                        sb,
                                        st,
                                        insp,
                                        &chats,
                                    );
                                    if applied {
                                        insp_cx.notify();
                                    }
                                    applied
                                });
                                if applied {
                                    st_cx.notify();
                                }
                                applied
                            });
                            if ok {
                                for c in &chats {
                                    if c.is_bot.unwrap_or(false) {
                                        sb.bot_ids.insert(c.chat_id);
                                    }
                                }
                                if let Some(ref rich_list) = rich_chats {
                                    for rc in rich_list {
                                        if rc.chat_id > 0 {
                                            let cid = rc.chat_id as u64;
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
                                    }
                                    sb.recompute_folder_badges();
                                }
                                for d in &mut sb.dialogs {
                                    if d.avatar_initials.is_empty() || d.avatar_initials == "?" {
                                        let fixed = derive_initials(&d.peer_name);
                                        if !fixed.is_empty() {
                                            d.avatar_initials = fixed;
                                        }
                                    }
                                }
                                sb_cx.notify();
                            }
                            ok
                        });
                        if applied {
                            shell.client.local_store.replace_cached_dialog_summaries(&chats);
                            if let Some(ref rich_list) = rich_chats {
                                shell.client.local_store.replace_cached_rich_chats(rich_list);
                            }
                            shell.reconcile_chat_scoped_toasts(cx);
                            if let Some(ref rich_list) = rich_chats {
                                for rc in rich_list {
                                    if rc.chat_id > 0 {
                                        let cid = rc.chat_id as u64;
                                        shell.session_sync.rich_chats_by_id.insert(cid, rc.clone());
                                        if let Some(pid) = rc.peer_user_id {
                                            if pid > 0 {
                                                shell.session_sync.chat_peer_user_ids.insert(cid, pid);
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some(selected_id) = shell.sidebar.read(cx).active_chat_id {
                                shell.sync_selected_chat_inspector_and_stage(selected_id, cx);
                            }
                            shell.prefetch_top_chat_histories(cx);
                            shell.refresh_miniapps_catalog(None, cx);
                        }
                    }
                    Err(err) => {
                        if fence.matches(
                            shell.is_authenticated,
                            shell.session_sync.auth_generation,
                            shell.session_sync.chat_list_request_generation,
                            shell.session_sync.session_token.as_deref(),
                            shell.session_sync.user_id,
                        ) {
                            shell.sidebar.update(cx, |sb, sb_cx| {
                                if sb.is_loading_dialogs {
                                    sb.is_loading_dialogs = false;
                                    sb_cx.notify();
                                }
                            });
                            shell.stage.update(cx, |st, st_cx| {
                                if st.is_loading_workspace {
                                    st.is_loading_workspace = false;
                                    st_cx.notify();
                                }
                            });
                            shell.show_non_chat_toast(
                                format!(
                                    "{}: {}",
                                    tr_str(
                                        shell.active_lang_id(),
                                        "刷新会话列表失败",
                                        "Failed to refresh chats"
                                    ),
                                    err
                                ),
                                3200,
                                cx,
                            );
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Telegram-style background prefetcher: warms the latest messages of the top visible dialogs
    /// into `LocalStore` and `stage.chat_histories` with bounded concurrency so opening recent chats
    /// is instant even after a fresh login.
    pub fn prefetch_top_chat_histories(&mut self, cx: &mut Context<Self>) {
        let Some(call_fence) = self.capture_client_fence() else {
            return;
        };
        let Some(self_uid) = self.session_sync.user_id else {
            return;
        };
        let active_chat_id = self.stage.read(cx).active_chat_id;
        let candidates: Vec<(u64, String, String, u32)> = {
            let sb = self.sidebar.read(cx);
            let st = self.stage.read(cx);
            let mut ordered: Vec<&DialogItemModel> = sb.dialogs.iter().filter(|d| d.is_pinned).collect();
            ordered.extend(sb.dialogs.iter().filter(|d| !d.is_pinned));
            ordered
                .into_iter()
                .filter(|d| {
                    Some(d.id) != active_chat_id
                        && !self.session_sync.prefetched_history_chats.contains(&d.id)
                        && !st.loaded_history_chats.contains(&d.id)
                        && st.chat_histories.get(&d.id).map_or(true, |h| h.is_empty())
                        && self
                            .client
                            .local_store
                            .get_cached_chat_history(d.id)
                            .map_or(true, |snap| snap.messages.is_empty())
                })
                .take(8)
                .map(|d| (d.id, d.peer_name.clone(), d.avatar_initials.clone(), d.avatar_color))
                .collect()
        };
        if candidates.is_empty() {
            return;
        }
        for (cid, _, _, _) in &candidates {
            self.session_sync.prefetched_history_chats.insert(*cid);
        }

        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            // Yield initial startup bandwidth to active chat & primary panels before warming
            // background dialog histories (matches Telegram Desktop preload cadence).
            app.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            for chunk in candidates.chunks(3) {
                if !call_fence.preflight_ui(&weak_shell, app).await {
                    return;
                }
                let batch_futs = chunk.iter().map(|(cid, peer_name, peer_init, peer_color)| {
                    let fence = call_fence.clone();
                    let cid = *cid;
                    let peer_name = peer_name.clone();
                    let peer_init = peer_init.clone();
                    let peer_color = *peer_color;
                    async move {
                        let (msgs_res, rich_res) = futures::join!(
                            fence.client.get_chat_messages_for_session(
                                &fence.token,
                                fence.client_epoch,
                                cid,
                                Some(50),
                            ),
                            fence.client.hermes_messages_history_rich_with_token(
                                &fence.token,
                                fence.client_epoch,
                                cid as i64,
                                Some(50),
                            ),
                        );
                        (cid, peer_name, peer_init, peer_color, msgs_res.ok(), rich_res.ok())
                    }
                });
                let results = futures::future::join_all(batch_futs).await;
                let keep_going = weak_shell
                    .update(app, |shell, cx| {
                        if !call_fence.is_valid(shell) {
                            return false;
                        }
                        let mut active_updated = false;
                        for (cid, peer_name, peer_init, peer_color, msgs_opt, rich_opt) in results {
                            let Some(msgs) = msgs_opt else {
                                continue;
                            };
                            let rich_slice = rich_opt.as_deref().unwrap_or(&[]);
                            shell
                                .client
                                .local_store
                                .replace_cached_chat_history(cid, &msgs, rich_slice);

                            let still_in_sidebar = shell
                                .sidebar
                                .read(cx)
                                .dialogs
                                .iter()
                                .any(|d| d.id == cid);
                            if !still_in_sidebar {
                                continue;
                            }
                            let models = build_cached_history_models(
                                &msgs,
                                rich_slice,
                                self_uid,
                                &peer_name,
                                &peer_init,
                                peer_color,
                            );
                            shell.stage.update(cx, |st, st_cx| {
                                let should_seed = st
                                    .chat_histories
                                    .get(&cid)
                                    .map_or(true, |h| h.is_empty());
                                if st.active_chat_id == Some(cid) {
                                    if st.messages.is_empty() {
                                        st.messages = models.clone();
                                        st.chat_histories.insert(cid, models);
                                        st.loaded_history_chats.insert(cid);
                                        st.is_loading_history = false;
                                        st.row_height_cache.rebuild_prefix_sums(&st.messages);
                                        st.scroll_to_bottom();
                                        st_cx.notify();
                                        active_updated = true;
                                    }
                                } else if should_seed {
                                    st.chat_histories.insert(cid, models);
                                    st.loaded_history_chats.insert(cid);
                                }
                            });
                        }
                        if active_updated {
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    return;
                }
            }
        })
        .detach();
    }

    /// Fetches historical messages from the backend for `chat_id` under a session/generation fence.
    pub fn load_historical_messages(&mut self, chat_id: u64, cx: &mut Context<Self>) {
        let (Some(token), Some(uid)) = (
            self.session_sync.session_token.clone(),
            self.session_sync.user_id,
        ) else {
            return;
        };
        if !self.is_authenticated {
            return;
        }
        let Some(call_fence) = self.capture_client_fence() else {
            return;
        };
        self.session_sync.history_request_generation =
            self.session_sync.history_request_generation.wrapping_add(1);
        self.session_sync.history_in_flight_chat = Some((chat_id, std::time::Instant::now()));

        self.stage.update(cx, |st, st_cx| {
            if st.active_chat_id == Some(chat_id)
                && st.messages.is_empty()
                && !st.loaded_history_chats.contains(&chat_id)
                && !st.show_bot_start_bar
                && !st.is_loading_history
            {
                st.is_loading_history = true;
                st_cx.notify();
            }
        });

        let fence = SessionRequestFence {
            auth_generation: self.session_sync.auth_generation,
            request_generation: self.session_sync.history_request_generation,
            token,
            user_id: uid,
        };

        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            if !call_fence.preflight_ui(&weak_shell, app).await {
                return;
            }
            let (res, rich_res) = futures::join!(
                call_fence.client.get_chat_messages_for_session(
                    &call_fence.token,
                    call_fence.client_epoch,
                    chat_id,
                    Some(50),
                ),
                call_fence.client.hermes_messages_history_rich_with_token(
                    &call_fence.token,
                    call_fence.client_epoch,
                    chat_id as i64,
                    Some(50),
                ),
            );
            let rich_msgs = rich_res.ok();
            let _ = weak_shell.update(app, |shell, cx| {
                if !call_fence.is_valid(shell) {
                    return;
                }
                if shell.session_sync.history_request_generation == fence.request_generation {
                    shell.session_sync.history_in_flight_chat = None;
                }
                match res {
                    Ok(msgs) => {
                        shell.client.local_store.replace_cached_chat_history(
                            chat_id,
                            &msgs,
                            rich_msgs.as_deref().unwrap_or(&[]),
                        );
                        let exp_pairs: Vec<(u64, i64)> = msgs
                            .iter()
                            .filter_map(|m| {
                                let mid = m.message_id;
                                let exp = m.expires_at_ms.or_else(|| {
                                    m.ttl_seconds
                                        .filter(|&ttl| ttl > 0)
                                        .and_then(|ttl| {
                                            m.date.map(|d| {
                                                let base_ms = if d < 10_000_000_000 { d * 1000 } else { d };
                                                base_ms.saturating_add(i64::from(ttl) * 1000)
                                            })
                                        })
                                })?;
                                Some((mid, exp))
                            })
                            .collect();
                        let protected_ids: HashSet<u64> = shell
                            .session_sync
                            .in_flight_reactions
                            .iter()
                            .filter_map(|(&(cid, mid), &cnt)| {
                                (cid == chat_id && cnt > 0).then_some(mid)
                            })
                            .collect();
                        let msgs_for_bg = msgs.clone();
                        let ok = shell.sidebar.update(cx, |sb, sb_cx| {
                            shell.stage.update(sb_cx, |st, st_cx| {
                                let res = apply_fenced_history_response_preserving(
                                    &fence,
                                    shell.is_authenticated,
                                    shell.session_sync.auth_generation,
                                    shell.session_sync.history_request_generation,
                                    shell.session_sync.session_token.as_deref(),
                                    shell.session_sync.user_id,
                                    sb,
                                    st,
                                    chat_id,
                                    msgs,
                                    &protected_ids,
                                );
                                if res {
                                    for (mid, exp) in exp_pairs {
                                        st.message_expirations.insert(mid, exp);
                                    }
                                    if let Some(ref rich) = rich_msgs {
                                        overlay_rich_message_metadata(&mut st.messages, rich);
                                        st.row_height_cache.mark_dirty();
                                        if let Some(hist) = st.chat_histories.get_mut(&chat_id) {
                                            overlay_rich_message_metadata(hist, rich);
                                        }
                                    }
                                    sb.recompute_folder_badges();
                                    st_cx.notify();
                                } else if st.active_chat_id != Some(chat_id) {
                                    // User switched rapidly to another chat within the same valid session:
                                    // retain the completed fetch in background `chat_histories` for 0ms return.
                                    if let Some(dlg) = sb.dialogs.iter().find(|d| d.id == chat_id) {
                                        let models = build_cached_history_models(
                                            &msgs_for_bg,
                                            rich_msgs.as_deref().unwrap_or(&[]),
                                            fence.user_id,
                                            &dlg.peer_name,
                                            &dlg.avatar_initials,
                                            dlg.avatar_color,
                                        );
                                        st.chat_histories.insert(chat_id, models);
                                        st.loaded_history_chats.insert(chat_id);
                                    }
                                }
                                res
                            })
                        });
                        if ok && shell.stage.read(cx).active_chat_id == Some(chat_id) {
                            let (_, media, files, links, voices) =
                                map_stage_media_to_gallery(&shell.stage.read(cx).messages);
                            shell.inspector.update(cx, |insp, insp_cx| {
                                insp.sync_shared_content(media, files, links, voices, Vec::new());
                                insp_cx.notify();
                            });
                        }
                    }
                    Err(err) => {
                        if fence.matches(
                            shell.is_authenticated,
                            shell.session_sync.auth_generation,
                            shell.session_sync.history_request_generation,
                            shell.session_sync.session_token.as_deref(),
                            shell.session_sync.user_id,
                        ) {
                            shell.stage.update(cx, |st, st_cx| {
                                if st.active_chat_id == Some(chat_id) && st.is_loading_history {
                                    st.is_loading_history = false;
                                    st_cx.notify();
                                }
                            });
                            if shell.active_panel == ActiveWorkspacePanel::Chats
                                && !(shell.theme_state.palette.is_apple_style()
                                    && shell.sidebar.read(cx).is_plus_menu_open)
                                && shell.stage.read(cx).active_chat_id == Some(chat_id)
                            {
                                shell.show_chat_toast(
                                    chat_id,
                                    format!(
                                        "{}: {}",
                                        tr_str(
                                            shell.active_lang_id(),
                                            "加载聊天记录失败",
                                            "Failed to load history"
                                        ),
                                        err
                                    ),
                                    3200,
                                    cx,
                                );
                            }
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn append_outgoing_attachment(&mut self, kind: &str, cx: &mut Context<Self>) {
        let lang = self.active_lang_id();
        match kind {
            "poll" => {
                self.open_modal_dialog(
                    tr_str(lang, "创建投票", "Create Poll"),
                    ModalDialogKind::CreatePoll,
                    420.0,
                    480.0,
                    cx,
                );
            }
            "voice" => {
                let start = !self.rtc_talk.voice_recorder.is_recording();
                self.toggle_voice_recording(start, cx);
            }
            "photo" | "video" | "file" => {
                self.show_toast(
                    tr_str(
                        lang,
                        "请将文件拖拽至聊天窗口上传发送",
                        "Drop a local file into the chat window to upload and send",
                    ),
                    2600,
                    cx,
                );
            }
            "location" => {
                self.show_toast(
                    tr_str(
                        lang,
                        "位置共享需要选择真实位置后发送",
                        "Location sharing requires selecting a location",
                    ),
                    2600,
                    cx,
                );
            }
            _ => {}
        }
    }
}
