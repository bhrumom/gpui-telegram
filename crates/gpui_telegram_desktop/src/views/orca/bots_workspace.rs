//! Root GPUI `0.2.2` `Entity<OrcaBotsWorkspace>` combining the Bots Sidebar,
//! expandable Bot Chat Tree (`BotChatTreeBuilder` / `build_chat_tree`), Group Rooms list,
//! Priority-Folded `"Needs You"` Inbox, 4-tab Bot Home, interactive dialogs, Design Mode
//! handoff injection slot, and non-themable `SecuritySurfaceModal`.

use std::sync::Arc;

use bot_browser::DesignModeChatHandoff;
use gpui::prelude::FluentBuilder;
use gpui::*;
use orca_bots::{HandOverTarget, TidyUpSchedule};
use orca_core::bot_ecosystem::{BotChatLiveState, BotChatTreeEntry, TeamSharePublicIdentity};
use orca_core::bot_groups::BotGroupReplyMode;
use orca_core::bot_items::{AskSelection, BotItemStatus};
use orca_core::bot_look_around::LookAroundEvery;
use orca_core::bot_types::{BotAgentId, BotAvatarOutfit, BotMascotId};
use orca_core::triggers_webhooks::WebhookAdapterId;

use crate::components::ui::{delete_prev_word, pop_last_char, sanitize_paste_text};
use crate::i18n::tr_str;
use crate::theme::{IconName, TdesktopThemePalette, tg_icon};

use super::bots_dialogs::render_active_bot_dialog;
use super::bots_home::render_bot_home_panel;
use super::bots_state::{
    BotDialogState, BotsInputTarget, OrcaBotsAction, OrcaBotsSurfaceState,
};
use super::runtime::OrcaDesktopRuntime;
use super::security_surface::{
    SecuritySurfaceDecision, SecuritySurfaceModal, SecuritySurfaceModalState,
};

#[derive(Debug, Clone)]
enum SecurityApprovalContext {
    BotAccess { request_id: String },
    E2eeTeamShare { peer: TeamSharePublicIdentity, offer_json: String },
}

pub struct OrcaBotsWorkspace {
    pub runtime: Arc<OrcaDesktopRuntime>,
    pub state: OrcaBotsSurfaceState,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    focus_handle: FocusHandle,
    pub sidebar_scroll: ScrollHandle,
    pub chat_scroll: ScrollHandle,
    pub items_scroll: ScrollHandle,
    pub up_next_scroll: ScrollHandle,
    pub settings_scroll: ScrollHandle,
    pub dialog_scroll: ScrollHandle,
    pending_actions: Vec<OrcaBotsAction>,
    security_modal: Option<Entity<SecuritySurfaceModal>>,
    security_context: Option<SecurityApprovalContext>,
}

impl OrcaBotsWorkspace {
    pub fn new(
        runtime: Arc<OrcaDesktopRuntime>,
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut state = OrcaBotsSurfaceState::default();
        let now_ms = chrono::Utc::now().timestamp_millis();
        if let Err(err) = state.refresh_from_local(&runtime, now_ms) {
            state.error_banner = Some(err);
        }
        let badge = state.needs_you_fold.visible.len() + state.needs_you_fold.folded.len();
        Self {
            runtime,
            state,
            palette,
            language_id: language_id.into(),
            focus_handle: cx.focus_handle(),
            sidebar_scroll: ScrollHandle::new(),
            chat_scroll: ScrollHandle::new(),
            items_scroll: ScrollHandle::new(),
            up_next_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            dialog_scroll: ScrollHandle::new(),
            pending_actions: vec![OrcaBotsAction::NeedsYouBadgeChanged(badge)],
            security_modal: None,
            security_context: None,
        }
    }

    pub fn take_pending_actions(&mut self) -> Vec<OrcaBotsAction> {
        std::mem::take(&mut self.pending_actions)
    }

    /// Synchronizes workspace palette and locale. Deliberately NEVER propagates
    /// `TdesktopThemePalette` into `SecuritySurfaceModal` (locale only!).
    pub fn set_theme_and_locale(
        &mut self,
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.palette = palette;
        let lang = language_id.into();
        self.language_id = lang.clone();
        if let Some(ref modal) = self.security_modal {
            modal.update(cx, |m, mcx| m.set_locale(lang, mcx));
        }
        cx.notify();
    }

    pub fn inject_design_mode_handoff(
        &mut self,
        handoff: DesignModeChatHandoff,
        cx: &mut Context<Self>,
    ) {
        self.state.inject_design_mode_handoff(handoff);
        self.pending_actions.push(OrcaBotsAction::ShowToast(
            "Design Mode capture attached to Orca Bot chat".to_string(),
        ));
        cx.notify();
    }

    pub fn has_active_security_modal(&self) -> bool {
        self.security_modal.is_some()
    }

    pub fn has_active_text_input(&self) -> bool {
        self.state.active_input != BotsInputTarget::None
    }

    pub fn set_active_input(
        &mut self,
        target: BotsInputTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.active_input = target;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn active_input_string_mut(&mut self) -> Option<&mut String> {
        match self.state.active_input {
            BotsInputTarget::None => None,
            BotsInputTarget::BotChatDraft => Some(&mut self.state.chat_draft),
            BotsInputTarget::NewBotName => Some(&mut self.state.new_bot_name),
            BotsInputTarget::NewItemTitle => Some(&mut self.state.new_item_title),
            BotsInputTarget::MemoryForgetQuery => Some(&mut self.state.memory_forget_query),
            BotsInputTarget::GroupMessageDraft => match &mut self.state.dialog {
                BotDialogState::GroupRoom { draft, .. } => Some(draft),
                _ => None,
            },
            BotsInputTarget::NewGroupName => match &mut self.state.dialog {
                BotDialogState::CreateGroup { name, .. } => Some(name),
                _ => None,
            },
            BotsInputTarget::NewThreadTitle => match &mut self.state.dialog {
                BotDialogState::GroupRoom {
                    new_thread_title, ..
                } => Some(new_thread_title),
                _ => None,
            },
            BotsInputTarget::TemplateCustomName => match &mut self.state.dialog {
                BotDialogState::TemplateGallery { custom_name, .. } => Some(custom_name),
                _ => None,
            },
            BotsInputTarget::PackageImportBase64 => match &mut self.state.dialog {
                BotDialogState::PackageManager { import_zip_b64, .. } => Some(import_zip_b64),
                _ => None,
            },
            BotsInputTarget::E2eePeerIdentityJson => match &mut self.state.dialog {
                BotDialogState::TeamShare {
                    peer_identity_json, ..
                } => Some(peer_identity_json),
                _ => None,
            },
            BotsInputTarget::E2eeOfferJson => match &mut self.state.dialog {
                BotDialogState::TeamShare {
                    incoming_offer_json,
                    ..
                } => Some(incoming_offer_json),
                _ => None,
            },
            BotsInputTarget::ImportPasteText => match &mut self.state.dialog {
                BotDialogState::CrossAppImport { paste_text, .. } => Some(paste_text),
                _ => None,
            },
            BotsInputTarget::HandOverWorkspaceName => match &mut self.state.dialog {
                BotDialogState::HandOver { workspace_name, .. } => Some(workspace_name),
                _ => None,
            },
            BotsInputTarget::HandOverRepoSlug => match &mut self.state.dialog {
                BotDialogState::HandOver { repo_slug, .. } => Some(repo_slug),
                _ => None,
            },
            BotsInputTarget::HandOverPrompt => match &mut self.state.dialog {
                BotDialogState::HandOver { recent_prompt, .. } => Some(recent_prompt),
                _ => None,
            },
            BotsInputTarget::HandOverCustomName => match &mut self.state.dialog {
                BotDialogState::HandOver { custom_name, .. } => Some(custom_name),
                _ => None,
            },
        }
    }

    pub fn insert_committed_text(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if self.security_modal.is_some() {
            return false;
        }
        let allow_nl = matches!(
            self.state.active_input,
            BotsInputTarget::BotChatDraft
                | BotsInputTarget::ImportPasteText
                | BotsInputTarget::E2eePeerIdentityJson
                | BotsInputTarget::E2eeOfferJson
        );
        let clean = sanitize_paste_text(text, allow_nl);
        if clean.is_empty() {
            return false;
        }
        if let Some(buf) = self.active_input_string_mut() {
            buf.push_str(&clean);
            cx.notify();
            true
        } else {
            false
        }
    }
}

impl OrcaBotsWorkspace {
    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    fn emit_needs_you_badge(&mut self) {
        let total = self.state.needs_you_fold.visible.len()
            + self.state.needs_you_fold.folded.len();
        self.pending_actions
            .push(OrcaBotsAction::NeedsYouBadgeChanged(total));
    }

    fn record_result<T>(&mut self, res: Result<T, String>) -> Option<T> {
        match res {
            Ok(v) => {
                self.state.error_banner = None;
                self.emit_needs_you_badge();
                Some(v)
            }
            Err(err) => {
                self.state.error_banner = Some(err);
                None
            }
        }
    }

    pub fn handle_key_down(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.security_modal.is_some() {
            return true;
        }
        let key = ev.keystroke.key.as_str();
        let mods = ev.keystroke.modifiers;
        if key.eq_ignore_ascii_case("escape") {
            if self.state.dialog != BotDialogState::None {
                self.close_dialog(cx);
                return true;
            }
            if self.state.active_input != BotsInputTarget::None {
                self.state.active_input = BotsInputTarget::None;
                cx.notify();
                return true;
            }
            return false;
        }
        if self.state.active_input == BotsInputTarget::None {
            return false;
        }
        if (mods.platform || mods.control) && key.eq_ignore_ascii_case("v") {
            if let Some(item) = cx.read_from_clipboard()
                && let Some(text) = item.text()
            {
                return self.insert_committed_text(&text, cx);
            }
            return true;
        }
        if key.eq_ignore_ascii_case("backspace") {
            if let Some(buf) = self.active_input_string_mut() {
                if mods.alt {
                    delete_prev_word(buf);
                } else {
                    pop_last_char(buf);
                }
                cx.notify();
            }
            return true;
        }
        if key.eq_ignore_ascii_case("enter") && !mods.shift {
            match self.state.active_input {
                BotsInputTarget::BotChatDraft => self.send_chat_draft(cx),
                BotsInputTarget::NewBotName => self.create_new_bot_from_input(cx),
                BotsInputTarget::NewItemTitle => self.add_new_item(cx),
                BotsInputTarget::MemoryForgetQuery => self.forget_memory_fact(cx),
                BotsInputTarget::GroupMessageDraft => {
                    if let BotDialogState::GroupRoom { ref group_id, .. } = self.state.dialog {
                        let gid = group_id.clone();
                        self.post_group_draft(&gid, cx);
                    }
                }
                BotsInputTarget::NewGroupName => self.submit_create_group(cx),
                BotsInputTarget::NewThreadTitle => {
                    if let BotDialogState::GroupRoom { ref group_id, .. } = self.state.dialog {
                        let gid = group_id.clone();
                        self.create_group_thread(&gid, cx);
                    }
                }
                _ => {}
            }
            return true;
        }
        if !mods.platform && !mods.control && !mods.alt {
            if let Some(ch) = ev.keystroke.key_char.as_deref() {
                return self.insert_committed_text(ch, cx);
            }
        }
        false
    }

    fn mount_security_modal(
        &mut self,
        modal_state: SecuritySurfaceModalState,
        ctx: SecurityApprovalContext,
        cx: &mut Context<Self>,
    ) {
        let lang = self.language_id.clone();
        let entity = cx.new(|mcx| SecuritySurfaceModal::new(modal_state, lang, mcx));
        cx.observe(&entity, |ws, ent, cx| {
            let dec = ent.update(cx, |m, _| m.take_decision());
            if let Some(decision) = dec {
                ws.resolve_security_modal(decision, cx);
            }
        })
        .detach();
        self.security_modal = Some(entity);
        self.security_context = Some(ctx);
        cx.notify();
    }

    fn resolve_security_modal(&mut self, decision: SecuritySurfaceDecision, cx: &mut Context<Self>) {
        self.security_modal = None;
        let ctx = self.security_context.take();
        let approved = decision == SecuritySurfaceDecision::Approved;
        let now_ms = Self::now_ms();
        match ctx {
            Some(SecurityApprovalContext::BotAccess { request_id }) => {
                let rt = self.runtime.clone();
                let res = self
                    .state
                    .decide_access_request_local(&rt, &request_id, approved, now_ms);
                self.record_result(res);
            }
            Some(SecurityApprovalContext::E2eeTeamShare { peer, offer_json }) => {
                if approved {
                    let rt = self.runtime.clone();
                    let res = self
                        .state
                        .open_verified_team_share_offer_local(&rt, peer, &offer_json, now_ms);
                    self.record_result(res);
                } else {
                    self.state.status_banner =
                        Some("E2EE peer verification denied; offer rejected.".to_string());
                }
            }
            None => {}
        }
        cx.notify();
    }

    pub fn copy_text_to_clipboard(&mut self, text: String, toast: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.state.status_banner = Some(toast.to_string());
        self.pending_actions
            .push(OrcaBotsAction::CopyToClipboard(text));
        self.pending_actions
            .push(OrcaBotsAction::ShowToast(toast.to_string()));
        cx.notify();
    }

    pub fn close_dialog(&mut self, cx: &mut Context<Self>) {
        self.state.dialog = BotDialogState::None;
        self.state.active_input = BotsInputTarget::None;
        cx.notify();
    }

    pub fn select_bot_by_id(&mut self, bot_id: &str, cx: &mut Context<Self>) {
        self.state.select_bot(bot_id);
        let rt = self.runtime.clone();
        let res = self.state.refresh_from_local(&rt, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn create_new_bot_from_input(&mut self, cx: &mut Context<Self>) {
        let name = self.state.new_bot_name.trim().to_string();
        let name_opt = (!name.is_empty()).then_some(name);
        let rt = self.runtime.clone();
        let res = self
            .state
            .create_bot_local(&rt, name_opt, BotAgentId::Claude, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn delete_selected_bot(&mut self, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self.state.delete_selected_bot_local(&rt, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn toggle_selected_bot_paused(&mut self, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self.state.toggle_bot_paused_local(&rt, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn send_chat_draft(&mut self, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self.state.send_chat_message_local(&rt, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn focus_needs_you_item(&mut self, item_uid: &str, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let bot_id = self.state.selected_bot_id.clone();
        let res = self
            .state
            .compute_needs_you_fold(&rt, bot_id.as_deref(), Some(item_uid), Self::now_ms())
            .map(|_| ());
        self.record_result(res);
        cx.notify();
    }
}

impl OrcaBotsWorkspace {
    pub fn submit_ask_answer(
        &mut self,
        item_uid: &str,
        question_event: u64,
        selections: Vec<AskSelection>,
        cx: &mut Context<Self>,
    ) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .answer_ask_item(&rt, item_uid, question_event, selections, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn answer_offer_card(&mut self, offer_id: &str, sent: bool, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .apply_offer_card_action(&rt, offer_id, sent, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn add_new_item(&mut self, cx: &mut Context<Self>) {
        let title = self.state.new_item_title.trim().to_string();
        if title.is_empty() {
            return;
        }
        let rt = self.runtime.clone();
        let res = self
            .state
            .create_item_local(&rt, title, BotItemStatus::Planned, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn transition_item(
        &mut self,
        item_uid: &str,
        status: BotItemStatus,
        expected_version: u64,
        cx: &mut Context<Self>,
    ) {
        let rt = self.runtime.clone();
        let res = self.state.transition_item_status_local(
            &rt,
            item_uid,
            status,
            expected_version,
            Self::now_ms(),
        );
        self.record_result(res);
        cx.notify();
    }

    pub fn toggle_item_snooze(&mut self, item_uid: &str, is_snoozed: bool, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let now_ms = Self::now_ms();
        let res = if is_snoozed {
            self.state.unsnooze_item_local(&rt, item_uid, now_ms)
        } else {
            self.state
                .snooze_item_local(&rt, item_uid, now_ms + 4 * 3_600_000, now_ms)
        };
        self.record_result(res);
        cx.notify();
    }

    pub fn patch_look_around(
        &mut self,
        on: Option<bool>,
        every: Option<LookAroundEvery>,
        weekdays_only: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .patch_look_around_local(&rt, on, every, weekdays_only, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn start_up_next_suggestion(&mut self, suggestion_id: &str, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .start_up_next_suggestion_local(&rt, suggestion_id, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn dismiss_up_next_suggestion(&mut self, suggestion_id: &str, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .dismiss_up_next_suggestion_local(&rt, suggestion_id, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn forget_memory_fact(&mut self, cx: &mut Context<Self>) {
        let q = self.state.memory_forget_query.trim().to_string();
        if q.is_empty() {
            return;
        }
        let rt = self.runtime.clone();
        let res = self.state.forget_memory_fact_local(&rt, &q, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn undo_memory_change(&mut self, row_id: &str, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .undo_memory_change_local(&rt, row_id, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn configure_tidy_up(
        &mut self,
        enabled: bool,
        schedule: TidyUpSchedule,
        cx: &mut Context<Self>,
    ) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .configure_tidy_up_local(&rt, enabled, schedule, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn update_avatar_style(
        &mut self,
        mascot: Option<BotMascotId>,
        color: Option<String>,
        outfit: Option<BotAvatarOutfit>,
        cx: &mut Context<Self>,
    ) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .update_avatar_style_local(&rt, mascot, color, outfit, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn set_host_mcp_servers(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .set_host_mcp_servers_local(&rt, enabled, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn create_webhook(&mut self, adapter: WebhookAdapterId, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .create_webhook_local(&rt, adapter, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn open_access_security_modal(&mut self, request_id: &str, cx: &mut Context<Self>) {
        match self.state.request_access_approval_modal(request_id) {
            Ok(modal_state) => {
                self.mount_security_modal(
                    modal_state,
                    SecurityApprovalContext::BotAccess {
                        request_id: request_id.to_string(),
                    },
                    cx,
                );
            }
            Err(err) => {
                self.state.error_banner = Some(err);
                cx.notify();
            }
        }
    }

    pub fn open_template_gallery(&mut self, cx: &mut Context<Self>) {
        self.state.dialog = BotDialogState::TemplateGallery {
            selected_category: None,
            show_skills: false,
            custom_name: String::new(),
            last_installed: None,
        };
        cx.notify();
    }

    pub fn install_template(&mut self, template_id: &str, cx: &mut Context<Self>) {
        let custom = match &self.state.dialog {
            BotDialogState::TemplateGallery { custom_name, .. } if !custom_name.trim().is_empty() => {
                Some(custom_name.trim().to_string())
            }
            _ => None,
        };
        let rt = self.runtime.clone();
        let res = self.state.install_template_local(
            &rt,
            template_id,
            custom.as_deref(),
            Self::now_ms(),
        );
        self.record_result(res);
        cx.notify();
    }

    pub fn install_bundled_skill(&mut self, slug: &str, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .install_bundled_skill_local(&rt, slug, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }
}

impl OrcaBotsWorkspace {
    pub fn open_create_group_dialog(&mut self, cx: &mut Context<Self>) {
        self.state.dialog = BotDialogState::CreateGroup {
            name: String::new(),
            selected_bot_ids: self.state.bots.iter().take(2).map(|b| b.id.clone()).collect(),
        };
        cx.notify();
    }

    pub fn submit_create_group(&mut self, cx: &mut Context<Self>) {
        let (name, members) = match &self.state.dialog {
            BotDialogState::CreateGroup {
                name,
                selected_bot_ids,
            } => (name.trim().to_string(), selected_bot_ids.clone()),
            _ => return,
        };
        let group_name = if name.is_empty() {
            "Bot Room".to_string()
        } else {
            name
        };
        let rt = self.runtime.clone();
        let res = self
            .state
            .create_group_local(&rt, group_name, members, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn open_group_room(&mut self, group_id: &str, cx: &mut Context<Self>) {
        self.state.selected_group_id = Some(group_id.to_string());
        self.state.dialog = BotDialogState::GroupRoom {
            group_id: group_id.to_string(),
            selected_thread_id: None,
            draft: String::new(),
            new_thread_title: String::new(),
        };
        let rt = self.runtime.clone();
        let res = self.state.refresh_group_room(&rt, group_id);
        self.record_result(res);
        cx.notify();
    }

    pub fn select_group_thread(&mut self, thread_id: Option<String>, cx: &mut Context<Self>) {
        let gid = match &mut self.state.dialog {
            BotDialogState::GroupRoom {
                group_id,
                selected_thread_id,
                ..
            } => {
                *selected_thread_id = thread_id;
                group_id.clone()
            }
            _ => return,
        };
        let rt = self.runtime.clone();
        let res = self.state.refresh_group_room(&rt, &gid);
        self.record_result(res);
        cx.notify();
    }

    pub fn set_group_reply_mode(
        &mut self,
        group_id: &str,
        mode: BotGroupReplyMode,
        cx: &mut Context<Self>,
    ) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .set_group_reply_mode_local(&rt, group_id, mode, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn create_group_thread(&mut self, group_id: &str, cx: &mut Context<Self>) {
        let title = match &self.state.dialog {
            BotDialogState::GroupRoom {
                new_thread_title, ..
            } => new_thread_title.trim().to_string(),
            _ => return,
        };
        if title.is_empty() {
            return;
        }
        let rt = self.runtime.clone();
        let res = self.state.create_group_thread_local(&rt, group_id, &title);
        self.record_result(res);
        cx.notify();
    }

    pub fn post_group_draft(&mut self, group_id: &str, cx: &mut Context<Self>) {
        let (thread_id, draft) = match &self.state.dialog {
            BotDialogState::GroupRoom {
                selected_thread_id,
                draft,
                ..
            } => (selected_thread_id.clone(), draft.trim().to_string()),
            _ => return,
        };
        if draft.is_empty() {
            return;
        }
        let rt = self.runtime.clone();
        let res = self
            .state
            .post_group_message_local(&rt, group_id, thread_id, draft);
        self.record_result(res);
        cx.notify();
    }

    pub fn flush_group_pending_mail(&mut self, group_id: &str, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self.state.flush_paused_group_mail_local(&rt, group_id);
        self.record_result(res);
        cx.notify();
    }

    pub fn open_package_dialog(&mut self, cx: &mut Context<Self>) {
        self.state.dialog = BotDialogState::PackageManager {
            exported_zip_b64: None,
            exported_file_count: 0,
            scan_report: None,
            runnable_warnings: Vec::new(),
            import_zip_b64: String::new(),
            last_imported_bot: None,
        };
        cx.notify();
    }

    pub fn export_selected_bot_package(&mut self, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self.state.export_selected_bot_package_local(&rt);
        self.record_result(res);
        cx.notify();
    }

    pub fn import_bot_package_b64(&mut self, cx: &mut Context<Self>) {
        let b64 = match &self.state.dialog {
            BotDialogState::PackageManager { import_zip_b64, .. } => import_zip_b64.clone(),
            _ => return,
        };
        let rt = self.runtime.clone();
        let res = self
            .state
            .import_bot_package_b64_local(&rt, &b64, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn open_team_share_dialog(&mut self, cx: &mut Context<Self>) {
        self.state.dialog = BotDialogState::TeamShare {
            peer_identity_json: String::new(),
            sealed_offer_json: None,
            incoming_offer_json: String::new(),
            pending_peer_verify: None,
            offer_state_label: None,
        };
        cx.notify();
    }

    pub fn seal_team_share_offer(&mut self, cx: &mut Context<Self>) {
        let peer_json = match &self.state.dialog {
            BotDialogState::TeamShare {
                peer_identity_json, ..
            } => peer_identity_json.clone(),
            _ => return,
        };
        let rt = self.runtime.clone();
        let res = self
            .state
            .seal_team_share_offer_local(&rt, &peer_json, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn open_e2ee_verification_modal(&mut self, cx: &mut Context<Self>) {
        let (peer_json, offer_json) = match &self.state.dialog {
            BotDialogState::TeamShare {
                peer_identity_json,
                incoming_offer_json,
                ..
            } => (peer_identity_json.clone(), incoming_offer_json.clone()),
            _ => return,
        };
        match self.state.request_e2ee_peer_verification(&peer_json) {
            Ok(modal_state) => {
                if let Ok(peer) = serde_json::from_str::<TeamSharePublicIdentity>(peer_json.trim()) {
                    self.mount_security_modal(
                        modal_state,
                        SecurityApprovalContext::E2eeTeamShare { peer, offer_json },
                        cx,
                    );
                }
            }
            Err(err) => {
                self.state.error_banner = Some(err);
                cx.notify();
            }
        }
    }

    pub fn open_cross_app_import_dialog(&mut self, cx: &mut Context<Self>) {
        self.state.dialog = BotDialogState::CrossAppImport {
            selected_source: None,
            discovered: Vec::new(),
            paste_text: String::new(),
            last_imported_bot: None,
        };
        cx.notify();
    }

    pub fn discover_cross_app_imports(&mut self, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let res = self
            .state
            .discover_cross_app_imports_local(&rt, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn parse_import_paste(&mut self, cx: &mut Context<Self>) {
        let paste = match &self.state.dialog {
            BotDialogState::CrossAppImport { paste_text, .. } => paste_text.clone(),
            _ => return,
        };
        let res = self
            .state
            .parse_generic_import_paste_local(&paste, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }

    pub fn import_discovered_candidate(&mut self, index: usize, cx: &mut Context<Self>) {
        let (cand_opt, raw_paste) = match &self.state.dialog {
            BotDialogState::CrossAppImport {
                discovered,
                paste_text,
                ..
            } => (
                discovered.get(index).cloned(),
                (!paste_text.trim().is_empty()).then(|| paste_text.clone()),
            ),
            _ => (None, None),
        };
        let Some(cand) = cand_opt else {
            return;
        };
        let rt = self.runtime.clone();
        let res = self.state.import_discovered_candidate_local(
            &rt,
            &cand,
            raw_paste.as_deref(),
            Self::now_ms(),
        );
        self.record_result(res);
        cx.notify();
    }

    pub fn open_hand_over_dialog(&mut self, cx: &mut Context<Self>) {
        self.state.dialog = BotDialogState::HandOver {
            workspace_id: format!("ws_{}", Self::now_ms()),
            workspace_name: "fix-relay-reconnect".to_string(),
            repo_slug: String::new(),
            recent_prompt: String::new(),
            custom_name: String::new(),
            target_existing_bot_id: None,
            preview: None,
            last_outcome: None,
        };
        self.refresh_hand_over_preview(cx);
    }

    pub fn refresh_hand_over_preview(&mut self, cx: &mut Context<Self>) {
        let (ws_name, repo, prompt) = match &self.state.dialog {
            BotDialogState::HandOver {
                workspace_name,
                repo_slug,
                recent_prompt,
                ..
            } => (
                workspace_name.clone(),
                (!repo_slug.trim().is_empty()).then(|| repo_slug.clone()),
                (!recent_prompt.trim().is_empty()).then(|| recent_prompt.clone()),
            ),
            _ => return,
        };
        let res = self
            .state
            .preview_hand_over(&ws_name, repo.as_deref(), prompt.as_deref());
        self.record_result(res);
        cx.notify();
    }

    pub fn execute_hand_over(&mut self, cx: &mut Context<Self>) {
        let (ws_id, target) = match &self.state.dialog {
            BotDialogState::HandOver {
                workspace_id,
                workspace_name,
                repo_slug,
                recent_prompt,
                custom_name,
                target_existing_bot_id,
                preview,
                ..
            } => {
                let target = if let Some(bot_id) = target_existing_bot_id {
                    let purpose = preview
                        .as_ref()
                        .map(|p| p.purpose.clone())
                        .unwrap_or_else(|| format!("Supervise workspace {workspace_name}"));
                    HandOverTarget::ExistingBot {
                        bot_id: bot_id.clone(),
                        purpose,
                    }
                } else {
                    HandOverTarget::CreateNewBot {
                        workspace_name: workspace_name.clone(),
                        repo_slug: (!repo_slug.trim().is_empty()).then(|| repo_slug.clone()),
                        recent_prompt: (!recent_prompt.trim().is_empty())
                            .then(|| recent_prompt.clone()),
                        agent: BotAgentId::Claude,
                        custom_name: (!custom_name.trim().is_empty())
                            .then(|| custom_name.clone()),
                    }
                };
                (workspace_id.clone(), target)
            }
            _ => return,
        };
        let rt = self.runtime.clone();
        let res = self
            .state
            .execute_hand_over_local(&rt, &ws_id, target, Self::now_ms());
        self.record_result(res);
        cx.notify();
    }
}

impl Focusable for OrcaBotsWorkspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl OrcaBotsWorkspace {
    fn render_chat_tree_entry(
        &self,
        entry: &BotChatTreeEntry,
        depth: usize,
        pal: TdesktopThemePalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let node = &entry.node;
        let selected = self.state.selected_chat_session_id.as_deref() == Some(node.session_id.as_str());
        let sid = node.session_id.clone();
        let state_dot_color = match node.live_state {
            BotChatLiveState::NeedsYou | BotChatLiveState::Failed => pal.danger,
            BotChatLiveState::Replying | BotChatLiveState::Running | BotChatLiveState::AskingBot => {
                pal.accent
            }
            BotChatLiveState::Done => pal.success,
            BotChatLiveState::Idle | BotChatLiveState::Unreachable => pal.text_secondary,
        };
        let indent = px((depth as f32) * 12.0);
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .id(SharedString::from(format!("bot-tree-node-{}", node.session_id)))
                    .cursor_pointer()
                    .pl(px(8.0) + indent)
                    .pr(px(8.0))
                    .py(px(4.0))
                    .rounded(px(5.0))
                    .bg(if selected {
                        pal.row_active_rgba()
                    } else {
                        rgba(0x00000000)
                    })
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |ws, _, _, cx| {
                            cx.stop_propagation();
                            ws.state.selected_chat_session_id = Some(sid.clone());
                            ws.state.active_tab = super::bots_state::BotHomeTab::Chat;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(rgb(state_dot_color)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(pal.text_primary))
                                    .child(node.title.clone()),
                            ),
                    )
                    .when(node.needs_you || node.unread, |r| {
                        r.child(
                            div()
                                .px(px(5.0))
                                .py(px(1.0))
                                .rounded(px(4.0))
                                .bg(if node.needs_you {
                                    rgb(pal.danger)
                                } else {
                                    rgb(pal.unread_badge_bg)
                                })
                                .text_xs()
                                .text_color(rgb(pal.unread_badge_fg))
                                .child(if node.needs_you { "!" } else { "•" }),
                        )
                    }),
            )
            .children(
                entry
                    .children
                    .iter()
                    .map(|child| self.render_chat_tree_entry(child, depth + 1, pal, cx)),
            )
            .into_any_element()
    }

    fn render_sidebar(&self, pal: TdesktopThemePalette, lang: &str, cx: &mut Context<Self>) -> AnyElement {
        let new_bot_active = self.state.active_input == BotsInputTarget::NewBotName;
        let new_bot_display = if self.state.new_bot_name.is_empty() && !new_bot_active {
            tr_str(lang, "新建机器人名称…", "New bot name…").to_string()
        } else if new_bot_active {
            format!("{}|", self.state.new_bot_name)
        } else {
            self.state.new_bot_name.clone()
        };

        div()
            .id("orca-bots-sidebar")
            .w(px(280.0))
            .h_full()
            .bg(pal.sidebar_bg_rgba())
            .border_r_1()
            .border_color(pal.separator_rgba())
            .flex()
            .flex_col()
            .child(
                div()
                    .p(px(12.0))
                    .border_b_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(tg_icon(IconName::FolderBots, 16.0, pal.accent))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(tr_str(lang, "Orca 机器人", "Orca Bots")),
                                    ),
                            )
                            .child(
                                div()
                                    .id("orca-open-ide-from-bots")
                                    .cursor_pointer()
                                    .px(px(7.0))
                                    .py(px(3.0))
                                    .rounded(px(5.0))
                                    .bg(rgb(pal.input_bg))
                                    .text_xs()
                                    .text_color(rgb(pal.accent))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|ws, _, _, cx| {
                                            cx.stop_propagation();
                                            ws.pending_actions
                                                .push(OrcaBotsAction::OpenIdeWorkspace);
                                            cx.notify();
                                        }),
                                    )
                                    .child("Agent IDE"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .id("orca-new-bot-input")
                                    .flex_1()
                                    .cursor_text()
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(if new_bot_active {
                                        rgb(pal.accent)
                                    } else {
                                        pal.hairline_border_rgba()
                                    })
                                    .text_xs()
                                    .text_color(rgb(pal.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|ws, _, window, cx| {
                                            cx.stop_propagation();
                                            ws.set_active_input(
                                                BotsInputTarget::NewBotName,
                                                window,
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(new_bot_display),
                            )
                            .child(
                                div()
                                    .id("orca-new-bot-create-btn")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(px(6.0))
                                    .bg(rgb(pal.accent))
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.active_button_fg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|ws, _, _, cx| {
                                            cx.stop_propagation();
                                            ws.create_new_bot_from_input(cx);
                                        }),
                                    )
                                    .child("+"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(5.0))
                            .child(sidebar_pill(
                                "orca-sb-templates",
                                "Templates (24)",
                                pal,
                                cx.listener(|ws, _, _, cx| {
                                    cx.stop_propagation();
                                    ws.open_template_gallery(cx);
                                }),
                            ))
                            .child(sidebar_pill(
                                "orca-sb-import",
                                "Import",
                                pal,
                                cx.listener(|ws, _, _, cx| {
                                    cx.stop_propagation();
                                    ws.open_cross_app_import_dialog(cx);
                                }),
                            ))
                            .child(sidebar_pill(
                                "orca-sb-new-group",
                                "+ Group",
                                pal,
                                cx.listener(|ws, _, _, cx| {
                                    cx.stop_propagation();
                                    ws.open_create_group_dialog(cx);
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .id("orca-bots-sidebar-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.sidebar_scroll)
                    .p(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(self.state.bots.iter().map(|bot| {
                        let selected =
                            self.state.selected_bot_id.as_deref() == Some(bot.id.as_str());
                        let bid = bot.id.clone();
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .id(SharedString::from(format!("orca-bot-row-{}", bot.id)))
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(7.0))
                                    .rounded(px(6.0))
                                    .bg(if selected {
                                        pal.row_active_rgba()
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |ws, _, _, cx| {
                                            cx.stop_propagation();
                                            ws.select_bot_by_id(&bid, cx);
                                        }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(tg_icon(IconName::BotRobot, 15.0, pal.accent))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(pal.text_primary))
                                                    .child(bot.name.as_str().to_string()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(if bot.paused {
                                                rgb(pal.danger)
                                            } else {
                                                rgb(pal.text_secondary)
                                            })
                                            .child(if bot.paused {
                                                "paused"
                                            } else {
                                                bot.agent.as_str()
                                            }),
                                    ),
                            )
                            .when(selected, |col| {
                                if let Some(ref tree_view) = self.state.chat_tree {
                                    col.children(
                                        tree_view
                                            .tree
                                            .roots
                                            .iter()
                                            .map(|r| self.render_chat_tree_entry(r, 1, pal, cx)),
                                    )
                                } else {
                                    col
                                }
                            })
                    }))
                    .when(!self.state.groups.is_empty(), |col| {
                        col.child(
                            div()
                                .pt(px(8.0))
                                .px(px(6.0))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_secondary))
                                .child(tr_str(lang, "多机器人群组", "Bot Group Rooms")),
                        )
                        .children(self.state.groups.iter().map(|g| {
                            let gid = g.id.clone();
                            div()
                                .id(SharedString::from(format!("orca-grp-row-{}", g.id)))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(rgb(pal.card_bg))
                                .flex()
                                .items_center()
                                .justify_between()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.open_group_room(&gid, cx);
                                    }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(tg_icon(IconName::GroupUsers, 14.0, pal.accent))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(pal.text_primary))
                                                .child(g.name.clone()),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("{} bots", g.member_bot_ids.len())),
                                )
                        }))
                    }),
            )
            .into_any_element()
    }
}

fn sidebar_pill(
    id: &'static str,
    label: &'static str,
    pal: TdesktopThemePalette,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .cursor_pointer()
        .px(px(7.0))
        .py(px(3.0))
        .rounded(px(5.0))
        .bg(rgb(pal.input_bg))
        .text_xs()
        .text_color(rgb(pal.text_primary))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
        .into_any_element()
}

impl Render for OrcaBotsWorkspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let lang = self.language_id.clone();
        let selected_bot = self.state.selected_bot().cloned();
        let sidebar = self.render_sidebar(pal, &lang, cx);
        let dialog_opt = render_active_bot_dialog(self, pal, &lang, cx);

        let right_stage: AnyElement = match selected_bot {
            Some(ref bot) => render_bot_home_panel(self, bot, pal, &lang, cx),
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.0))
                .p(px(24.0))
                .child(tg_icon(IconName::BotRobot, 36.0, pal.accent))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(&lang, "欢迎使用 Orca 机器人操作系统", "Welcome to Orca Bot OS")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("orca-empty-browse-templates")
                                .cursor_pointer()
                                .px(px(12.0))
                                .py(px(7.0))
                                .rounded(px(6.0))
                                .bg(rgb(pal.accent))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.active_button_fg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.open_template_gallery(cx);
                                    }),
                                )
                                .child("Browse 24 Templates"),
                        )
                        .child(
                            div()
                                .id("orca-empty-create-bot")
                                .cursor_pointer()
                                .px(px(12.0))
                                .py(px(7.0))
                                .rounded(px(6.0))
                                .bg(rgb(pal.input_bg))
                                .text_xs()
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.create_new_bot_from_input(cx);
                                    }),
                                )
                                .child("Create New Bot"),
                        ),
                )
                .into_any_element(),
        };

        div()
            .id("orca-bots-workspace-root")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(pal.window_bg))
            .on_key_down(cx.listener(|ws, ev: &KeyDownEvent, _, cx| {
                if ws.handle_key_down(ev, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .when_some(self.state.error_banner.clone(), |col, err| {
                        col.child(
                            div()
                                .px(px(12.0))
                                .py(px(6.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.16))
                                .text_xs()
                                .text_color(rgb(pal.danger))
                                .child(err),
                        )
                    })
                    .when_some(self.state.status_banner.clone(), |col, msg| {
                        col.child(
                            div()
                                .px(px(12.0))
                                .py(px(5.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.12))
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .child(msg),
                        )
                    })
                    .child(right_stage),
            )
            .when_some(dialog_opt, |root, dlg| root.child(dlg))
            .when_some(self.security_modal.clone(), |root, modal| root.child(modal))
            .into_any_element()
    }
}
