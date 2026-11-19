//! AppShell bridge for Pure-Obscura `BotBrowserOrchestrator` & strictly-local Bot Assist cards.
//!
//! Security Red Line #3: Every Bot Assist card (`VaultLogin`, `ReuseConfirm`, `QrLogin`, `ClickCaptcha`)
//! and every credential submission/approval/click action executes strictly in-process and locally.
//! Nothing here ever touches `send_message_durable`, `send_rich_text_options`, `upload_media`,
//! or `gpui_telegram_proto::message::MessageContent`.

use std::collections::HashMap;
use std::sync::Arc;

use bot_browser::orchestrator::{BotBrowserOrchestrator, OrchestratorConfig};
use bot_browser::vault::{CredentialVault, FileMasterKeyStore};
use bot_browser::{AssistCardStatus, BotBrowserError, BotBrowserEvent};
use gpui::{App, AsyncApp, Context, WeakEntity};

use crate::i18n::tr_str;
use crate::views::app_shell::shell_actions::resolve_active_target_chat_id;
use crate::views::app_shell::shell_truth::format_authoritative_timestamp;
use crate::views::shell::AppShell;
use crate::views::stage::cards::{BotAssistCardAction, BOT_ASSIST_LOCAL_MSG_ID_BASE};

/// AppShell-owned state tracking the local `BotBrowserOrchestrator` and `card_id -> (chat_id, local_msg_id)`.
pub struct ShellBotBrowserState {
    pub orchestrator: Option<Arc<BotBrowserOrchestrator>>,
    pub card_locations: HashMap<String, (u64, u64)>,
    pub next_local_msg_id: u64,
    pub listener_generation: u64,
}

impl Default for ShellBotBrowserState {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellBotBrowserState {
    pub fn new() -> Self {
        Self {
            orchestrator: None,
            card_locations: HashMap::new(),
            next_local_msg_id: BOT_ASSIST_LOCAL_MSG_ID_BASE + 1,
            listener_generation: 0,
        }
    }

    pub fn reset_on_logout(&mut self) {
        self.card_locations.clear();
        self.next_local_msg_id = BOT_ASSIST_LOCAL_MSG_ID_BASE + 1;
        self.listener_generation = self.listener_generation.wrapping_add(1);
    }

    /// Returns the deterministic local message ID for `card_id` in `chat_id`, allocating one if new.
    pub fn allocate_local_msg_id(&mut self, card_id: &str, chat_id: u64) -> u64 {
        if let Some(&(_, existing_id)) = self.card_locations.get(card_id) {
            return existing_id;
        }
        let assigned = self.next_local_msg_id;
        self.next_local_msg_id = self.next_local_msg_id.saturating_add(1);
        self.card_locations
            .insert(card_id.to_string(), (chat_id, assigned));
        assigned
    }
}

impl AppShell {
    /// Attaches a `BotBrowserOrchestrator`, rehydrates any active cards, and subscribes to `BotBrowserEvent`s.
    pub fn attach_bot_browser_orchestrator(
        &mut self,
        orchestrator: Arc<BotBrowserOrchestrator>,
        cx: &mut Context<Self>,
    ) {
        self.bot_browser.orchestrator = Some(Arc::clone(&orchestrator));
        self.bot_browser.listener_generation =
            self.bot_browser.listener_generation.wrapping_add(1);
        let listener_gen = self.bot_browser.listener_generation;

        for card in orchestrator.list_cards_for_chat(None) {
            self.apply_bot_browser_event(BotBrowserEvent::UpsertCard(card), cx);
        }
        self.spawn_bot_browser_event_listener(orchestrator, listener_gen, cx);
    }

    /// Lazily initializes a local `CredentialVault` + `BotBrowserOrchestrator` rooted at `preferences_dir/bot_browser`.
    pub fn ensure_bot_browser_orchestrator(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<Arc<BotBrowserOrchestrator>, BotBrowserError> {
        if let Some(ref existing) = self.bot_browser.orchestrator {
            return Ok(Arc::clone(existing));
        }
        let data_dir = self.preferences_dir.join("bot_browser");
        let key_store = FileMasterKeyStore::new(data_dir.join("vault.key"));
        let vault = Arc::new(CredentialVault::open(
            data_dir.join("vault.db"),
            &key_store,
        )?);
        let orch = Arc::new(BotBrowserOrchestrator::new(
            OrchestratorConfig::new(data_dir),
            vault,
        ));
        self.attach_bot_browser_orchestrator(Arc::clone(&orch), cx);
        Ok(orch)
    }

    fn resolve_bot_card_chat_id(
        &self,
        card_id: &str,
        event_chat_id: Option<i64>,
        cx: &App,
    ) -> Option<u64> {
        event_chat_id
            .and_then(|cid| u64::try_from(cid).ok())
            .filter(|&cid| cid > 0)
            .or_else(|| self.bot_browser.card_locations.get(card_id).map(|&(c, _)| c))
            .or_else(|| {
                resolve_active_target_chat_id(
                    self.stage.read(cx).active_chat_id,
                    self.sidebar.read(cx).active_chat_id,
                )
            })
    }

    fn spawn_bot_browser_event_listener(
        &self,
        orchestrator: Arc<BotBrowserOrchestrator>,
        expected_gen: u64,
        cx: &mut Context<Self>,
    ) {
        let mut rx = orchestrator.subscribe_events();
        cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let keep_running = weak_shell
                            .update(app, |shell, cx| {
                                if shell.bot_browser.listener_generation != expected_gen {
                                    return false;
                                }
                                shell.apply_bot_browser_event(ev, cx);
                                true
                            })
                            .unwrap_or(false);
                        if !keep_running {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .detach();
    }

    /// Applies a `BotBrowserEvent` strictly to the local `ConversationStage` transcript without MTProto leakage.
    pub fn apply_bot_browser_event(&mut self, event: BotBrowserEvent, cx: &mut Context<Self>) {
        match event {
            BotBrowserEvent::UpsertCard(payload) => {
                let Some(chat_id) =
                    self.resolve_bot_card_chat_id(payload.card_id(), payload.chat_id(), cx)
                else {
                    return;
                };
                let local_msg_id = self
                    .bot_browser
                    .allocate_local_msg_id(payload.card_id(), chat_id);
                let ts = format_authoritative_timestamp(Some(chrono::Utc::now().timestamp_millis()));
                self.stage.update(cx, |st, st_cx| {
                    st.upsert_bot_assist_card(chat_id, local_msg_id, payload, ts);
                    st_cx.notify();
                });
                cx.notify();
            }
            BotBrowserEvent::UpdateCardStatus {
                card_id,
                chat_id,
                status,
                status_note,
                saved_credential_id,
                saved_account_hint,
                redirected_url,
            } => {
                let cid_u64 = self.resolve_bot_card_chat_id(&card_id, chat_id, cx);
                let updated = self.stage.update(cx, |st, st_cx| {
                    let changed = st.update_bot_assist_card_status(
                        cid_u64,
                        &card_id,
                        status,
                        status_note,
                        saved_credential_id,
                        saved_account_hint,
                        redirected_url,
                    );
                    if changed {
                        st_cx.notify();
                    }
                    changed
                });
                if updated {
                    cx.notify();
                }
            }
            BotBrowserEvent::RefreshQrImage {
                card_id,
                chat_id,
                png_bytes,
                countdown_secs,
                poll_attempt,
            } => {
                let cid_u64 = self.resolve_bot_card_chat_id(&card_id, chat_id, cx);
                let updated = self.stage.update(cx, |st, st_cx| {
                    let changed = st.refresh_bot_assist_qr(
                        cid_u64,
                        &card_id,
                        png_bytes,
                        countdown_secs,
                        poll_attempt,
                    );
                    if changed {
                        st_cx.notify();
                    }
                    changed
                });
                if updated {
                    cx.notify();
                }
            }
            BotBrowserEvent::TaskWakeupTriggered(wakeup) => {
                let lang = self.active_lang_id();
                let msg = format!(
                    "{} ({})",
                    tr_str(
                        lang,
                        "已保存并自动填充登录，正在唤醒 Bot 续跑任务",
                        "Credentials saved & auto-filled; resuming bot task",
                    ),
                    wakeup.origin
                );
                self.window_runtime.enqueue_toast(msg, 3200);
                cx.notify();
            }
            BotBrowserEvent::SidecarRestarted {
                session_id,
                restart_count,
                restored_url,
            } => {
                eprintln!(
                    "[BotBrowser] Obscura sidecar restarted (session={session_id}, count={restart_count}, restored_url={restored_url:?})"
                );
            }
        }
    }

    /// Dispatches a user interaction from any of the 4 Bot Assist cards strictly in-process.
    pub fn handle_bot_assist_card_action(
        &mut self,
        action: BotAssistCardAction,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id();
        let runtime = self.client.runtime().clone();
        match action {
            BotAssistCardAction::FocusLoginField { .. }
            | BotAssistCardAction::ToggleShowPassword { .. }
            | BotAssistCardAction::ToggleAutoReuse { .. }
            | BotAssistCardAction::ClickCaptchaPoint { .. }
            | BotAssistCardAction::ResetCaptchaClicks { .. } => {}
            BotAssistCardAction::SubmitLogin { card_id } => {
                let sub_res = self
                    .stage
                    .update(cx, |st, st_cx| {
                        let res = st.take_bot_card_login_submission(&card_id);
                        st_cx.notify();
                        res
                    });
                let submission = match sub_res {
                    Ok(sub) => sub,
                    Err(err_msg) => {
                        self.window_runtime.enqueue_toast(err_msg, 2400);
                        cx.notify();
                        return;
                    }
                };
                let Some(orch) = self.bot_browser.orchestrator.clone() else {
                    self.window_runtime.enqueue_toast(
                        tr_str(
                            lang,
                            "本地安全浏览器服务未初始化",
                            "Local bot browser orchestrator unavailable",
                        ),
                        2600,
                    );
                    cx.notify();
                    return;
                };
                let card_id_for_ui = card_id.clone();
                let cid_u64 = self.resolve_bot_card_chat_id(&card_id, None, cx);
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    let res = runtime
                        .spawn(async move { orch.submit_login_card(submission).await })
                        .await
                        .map_err(|e| BotBrowserError::Io(format!("task join error: {e}")))
                        .and_then(|inner| inner);
                    let _ = weak.update(app, |shell, cx| {
                        match res {
                            Ok(outcome) => {
                                shell.stage.update(cx, |st, st_cx| {
                                    st.update_bot_assist_card_status(
                                        cid_u64,
                                        &card_id_for_ui,
                                        AssistCardStatus::SavedAndLoggedIn,
                                        None,
                                        Some(outcome.credential_id.clone()),
                                        Some(outcome.account_hint.clone()),
                                        None,
                                    );
                                    st_cx.notify();
                                });
                                let note = if outcome.resumed_timed_out_task {
                                    tr_str(
                                        shell.active_lang_id(),
                                        "已保存凭据并自动续跑原任务",
                                        "Saved & resumed previous task",
                                    )
                                } else {
                                    tr_str(
                                        shell.active_lang_id(),
                                        "登录凭据已加密保存至本地安全库",
                                        "Credentials encrypted & saved locally",
                                    )
                                };
                                shell.window_runtime.enqueue_toast(note, 2600);
                            }
                            Err(e) => {
                                shell
                                    .window_runtime
                                    .enqueue_toast(format!("Vault login error: {e}"), 3200);
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            BotAssistCardAction::ApproveReuse { card_id, approved } => {
                let Some(orch) = self.bot_browser.orchestrator.clone() else {
                    return;
                };
                let cid_u64 = self.resolve_bot_card_chat_id(&card_id, None, cx);
                let next_local_status = if approved {
                    AssistCardStatus::ApprovedAndFilling
                } else {
                    AssistCardStatus::Denied
                };
                self.stage.update(cx, |st, st_cx| {
                    st.update_bot_assist_card_status(
                        cid_u64,
                        &card_id,
                        next_local_status,
                        None,
                        None,
                        None,
                        None,
                    );
                    st_cx.notify();
                });
                let card_id_req = card_id.clone();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    let res = runtime
                        .spawn(async move { orch.approve_reuse_card(&card_id_req, approved).await })
                        .await
                        .map_err(|e| BotBrowserError::Io(format!("task join error: {e}")))
                        .and_then(|inner| inner);
                    let _ = weak.update(app, |shell, cx| {
                        match res {
                            Ok(outcome) => {
                                let final_status = if outcome.approved {
                                    AssistCardStatus::Completed
                                } else {
                                    AssistCardStatus::Denied
                                };
                                shell.stage.update(cx, |st, st_cx| {
                                    st.update_bot_assist_card_status(
                                        cid_u64,
                                        &card_id,
                                        final_status,
                                        None,
                                        None,
                                        None,
                                        None,
                                    );
                                    st_cx.notify();
                                });
                            }
                            Err(e) => {
                                shell
                                    .window_runtime
                                    .enqueue_toast(format!("Reuse approval failed: {e}"), 2800);
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            BotAssistCardAction::SubmitCaptchaClicks { card_id } => {
                let points = self.stage.read(cx).bot_assist_click_drafts.get(&card_id).map(|d| d.points.clone()).unwrap_or_default();
                if points.is_empty() {
                    self.window_runtime.enqueue_toast(
                        tr_str(lang, "请先在验证图上点击目标位置", "Click target points first"),
                        2200,
                    );
                    cx.notify();
                    return;
                }
                let Some(orch) = self.bot_browser.orchestrator.clone() else {
                    return;
                };
                let cid_u64 = self.resolve_bot_card_chat_id(&card_id, None, cx);
                let card_id_req = card_id.clone();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    let res = runtime
                        .spawn(async move { orch.submit_click_captcha(&card_id_req, points).await })
                        .await
                        .map_err(|e| BotBrowserError::Io(format!("task join error: {e}")))
                        .and_then(|inner| inner);
                    let _ = weak.update(app, |shell, cx| {
                        match res {
                            Ok(_) => {
                                shell.stage.update(cx, |st, st_cx| {
                                    st.update_bot_assist_card_status(
                                        cid_u64,
                                        &card_id,
                                        AssistCardStatus::Completed,
                                        None,
                                        None,
                                        None,
                                        None,
                                    );
                                    st_cx.notify();
                                });
                            }
                            Err(e) => {
                                shell
                                    .window_runtime
                                    .enqueue_toast(format!("Click captcha failed: {e}"), 2800);
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            BotAssistCardAction::CancelCard { card_id } => {
                let cid_u64 = self.resolve_bot_card_chat_id(&card_id, None, cx);
                self.stage.update(cx, |st, st_cx| {
                    st.update_bot_assist_card_status(
                        cid_u64,
                        &card_id,
                        AssistCardStatus::Cancelled,
                        None,
                        None,
                        None,
                        None,
                    );
                    st_cx.notify();
                });
                if let Some(orch) = self.bot_browser.orchestrator.clone() {
                    let card_id_req = card_id.clone();
                    cx.spawn(async move |_: WeakEntity<Self>, _: &mut AsyncApp| {
                        let _ = runtime
                            .spawn(async move { orch.cancel_card(&card_id_req).await })
                            .await;
                    })
                    .detach();
                }
            }
        }
    }
}
