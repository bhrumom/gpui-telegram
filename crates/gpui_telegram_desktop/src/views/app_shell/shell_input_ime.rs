//! AppShell input routing, non-character key dispatch, and macOS/GPUI IME `EntityInputHandler`.

use std::ops::Range;

use gpui::{
    point, px, size, App, Bounds, Context, Entity, EntityInputHandler, KeyDownEvent, Keystroke,
    Modifiers, Pixels, Point, UTF16Selection, Window,
};

use crate::components::ui::{
    delete_prev_word, pop_last_char, sanitize_paste_text, utf16_len, utf16_offset_to_utf8,
};
use crate::views::chat_sidebar::CreationInputField;
use crate::views::composer::ime_machine::KeyAction;
use crate::views::global_tasks_workspace::StudioInputTarget;
use crate::views::nav_rail::NavDestination;
use crate::views::shell::{ActiveWorkspacePanel, AppShell};
use crate::views::stage::cards::BotAssistCardAction;

fn backspace_string(target: &mut String, is_alt: bool) -> bool {
    if is_alt { delete_prev_word(target) } else { pop_last_char(target) }
}

/// Single authoritative top-to-bottom visual Z-order input/IME target resolver matching
/// `AppShell::render()` (`shell.rs:1478-1567`) and `shell_overlays_render.rs:768-1064`.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ShellInputTarget {
    AuthWizard,
    PasscodeLock,
    OrcaSecurityModal,
    OrcaJumpPalette,
    ComputerTerminal(Entity<crate::views::ComputerWorkbench>),
    PublishReviewModal,
    ModalContainer,
    StudioInput,
    SettingsInput,
    CreationModal,
    ContactsModal,
    CreateBotBox(Entity<crate::views::boxes::peers::create_managed_bot_box::CreateManagedBotBoxPeers>),
    MiniAppInput(Entity<crate::components::bots::bot_attach_web_view::BotAttachWebView>),
    AppleCompose,
    InspectorBot,
    BotAssistCard,
    ContentHubInput,
    BrowserInput,
    CommunityInput,
    OrcaBotsInput,
    OrcaIdeInput,
    OpenBotInput,
    SidebarSearch,
    StageSearch,
    Composer,
    Absorbed,
}

impl ShellInputTarget {
    pub(crate) fn is_non_composer_ime_owner(&self) -> bool {
        !matches!(self, Self::Absorbed | Self::Composer)
    }
}

impl AppShell {
    fn active_computer_terminal_overlay(
        &self,
        cx: &App,
    ) -> Option<Entity<crate::views::ComputerWorkbench>> {
        self.overlays
            .computer_overlay
            .as_ref()
            .filter(|ov| {
                let wb = ov.read(cx);
                wb.is_terminal_input_active() || wb.is_interactive_pty_focused(cx)
            })
            .cloned()
    }

    fn raw_apple_compose_input_active(&self, cx: &App) -> bool {
        let sb = self.sidebar.read(cx);
        self.active_panel == ActiveWorkspacePanel::Chats
            && self.theme_state.palette.is_apple_style()
            && sb.is_plus_menu_open
            && !sb.search_focused
            && !sb.is_contacts_modal_open
            && sb.active_creation_modal.is_none()
            && !self.composer.read(cx).is_input_focused
            && {
                let h = self.content_hub.read(cx);
                !(h.quick_drawer_open
                    && h.active_input
                        == crate::views::content_hub::ContentHubInputTarget::QuickDrawerSearch)
            }
    }

    /// Resolves the single topmost visible input/IME target strictly matching `shell.rs` render Z-order.
    pub(crate) fn resolve_input_target(&self, cx: &App) -> ShellInputTarget {
        // Z #17: AuthWizardView
        if !self.is_authenticated {
            return ShellInputTarget::AuthWizard;
        }
        // Z #16: PasscodeLock
        if self.overlays.is_passcode_locked {
            return ShellInputTarget::PasscodeLock;
        }
        // Z #15.8: Top-Z Non-Themable Orca SecuritySurfaceModal
        if self.orca_security_modal.is_some() {
            return ShellInputTarget::OrcaSecurityModal;
        }
        // Z #15.5: Top-Level Orca ScopedJumpPalette (`Cmd+J`)
        if self.orca_jump_palette.is_some() {
            return ShellInputTarget::OrcaJumpPalette;
        }
        // Z #15: ComputerWorkbench (terminal/PTY or non-terminal absorber)
        if self.overlays.computer_overlay.is_some() {
            return match self.active_computer_terminal_overlay(cx) {
                Some(ov) => ShellInputTarget::ComputerTerminal(ov),
                None => ShellInputTarget::Absorbed,
            };
        }
        // Z #14: PublishReviewModal
        if self.is_publish_review_modal_active(cx) {
            return ShellInputTarget::PublishReviewModal;
        }
        // Z #13: ModalContainer
        if self.modal_container.read(cx).is_modal_active() {
            return ShellInputTarget::ModalContainer;
        }
        let sb = self.sidebar.read(cx);
        // Z #12: AccountPopover (full-window backdrop)
        if sb.is_account_popover_open {
            return ShellInputTarget::Absorbed;
        }
        // Z #11 / #10: McpTools / TaskCenter
        if matches!(
            self.active_panel,
            ActiveWorkspacePanel::McpTools | ActiveWorkspacePanel::TaskCenter
        ) {
            return if self.tasks_workspace.read(cx).active_input != StudioInputTarget::None {
                ShellInputTarget::StudioInput
            } else {
                ShellInputTarget::Absorbed
            };
        }
        // Z #9: Settings
        if self.active_panel == ActiveWorkspacePanel::Settings {
            return if self.settings_router.read(cx).is_input_active(cx) {
                ShellInputTarget::SettingsInput
            } else {
                ShellInputTarget::Absorbed
            };
        }
        // Z #8: Calls
        if self.active_panel == ActiveWorkspacePanel::Calls {
            return ShellInputTarget::Absorbed;
        }
        // Z #7: CreationModal
        if sb.active_creation_modal.is_some() {
            return ShellInputTarget::CreationModal;
        }
        // Z #6: ContactsModal
        if sb.is_contacts_modal_open {
            return ShellInputTarget::ContactsModal;
        }
        // Z #5e / #5d / #5c: MiniAppToSModal / BotProfileBox / PendingMiniAppConfirm
        if self.miniapps.miniapp_tos_modal_open
            || self
                .miniapps
                .bot_profile_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
            || self.miniapps.pending_miniapp_confirm.is_some()
        {
            return ShellInputTarget::Absorbed;
        }
        // Z #5b: CreateManagedBotBox
        if let Some(ent) = self
            .miniapps
            .create_bot_box
            .as_ref()
            .filter(|b| b.read(cx).is_visible)
            .cloned()
        {
            return ShellInputTarget::CreateBotBox(ent);
        }
        // Z #5a: MiniAppOverlay (unminimized)
        if let Some(ov) = self
            .miniapps
            .miniapp_overlay
            .as_ref()
            .filter(|w| w.read(cx).is_open && !self.miniapps.is_miniapp_minimized)
            .cloned()
        {
            return if ov.read(cx).has_focused_text_input() {
                ShellInputTarget::MiniAppInput(ov)
            } else {
                ShellInputTarget::Absorbed
            };
        }
        // Z #4 / #3 / #2: IvViewer / MediaViewer / TalkMode
        if self.overlays.iv_viewer_overlay.is_some()
            || self.overlays.media_viewer_overlay.is_some()
            || self.rtc_talk.talk_mode_overlay.is_some()
        {
            return ShellInputTarget::Absorbed;
        }
        // Z #1: Base Workspace — ContentHub
        if matches!(self.active_panel, ActiveWorkspacePanel::ContentHub(_)) {
            return if self.content_hub.read(cx).has_focused_input(true) {
                ShellInputTarget::ContentHubInput
            } else {
                ShellInputTarget::Absorbed
            };
        }
        // Z #1: Base Workspace — BrowserWorkspace (occluded by the main menu drawer too)
        if self.active_panel == ActiveWorkspacePanel::Browser {
            let browser_focused = !self.main_menu_drawer.read(cx).is_open
                && self.browser_workspace.as_ref().is_some_and(|bw| {
                    let bw_read = bw.read(cx);
                    !bw_read.is_externally_occluded() && bw_read.has_focused_text_input()
                });
            return if browser_focused {
                ShellInputTarget::BrowserInput
            } else if sb.search_focused {
                ShellInputTarget::SidebarSearch
            } else {
                ShellInputTarget::Absorbed
            };
        }
        // Z #1: Base Workspace — CommunityWorkspace (occluded by main menu drawer & Classic plus-menu)
        if self.active_panel == ActiveWorkspacePanel::Community {
            if self.content_hub.read(cx).has_focused_input(false) {
                return ShellInputTarget::ContentHubInput;
            }
            if sb.search_focused {
                return ShellInputTarget::SidebarSearch;
            }
            let community_focused = !self.main_menu_drawer.read(cx).is_open
                && (self.theme_state.palette.is_apple_style() || !sb.is_plus_menu_open)
                && self
                    .community_workspace
                    .as_ref()
                    .is_some_and(|cw| cw.read(cx).has_focused_text_input());
            return if community_focused {
                ShellInputTarget::CommunityInput
            } else {
                ShellInputTarget::Absorbed
            };
        }
        // Z #1: Base Workspace — OrcaBotsWorkspace
        if self.active_panel == ActiveWorkspacePanel::OrcaBots {
            if self
                .orca_bots_workspace
                .as_ref()
                .is_some_and(|w| w.read(cx).has_active_security_modal())
            {
                return ShellInputTarget::OrcaSecurityModal;
            }
            if sb.search_focused {
                return ShellInputTarget::SidebarSearch;
            }
            let bots_focused = !self.main_menu_drawer.read(cx).is_open
                && self
                    .orca_bots_workspace
                    .as_ref()
                    .is_some_and(|w| w.read(cx).has_active_text_input());
            return if bots_focused {
                ShellInputTarget::OrcaBotsInput
            } else {
                ShellInputTarget::Absorbed
            };
        }
        // Z #1: Base Workspace — OrcaIdeWorkspace
        if self.active_panel == ActiveWorkspacePanel::OrcaIde {
            if self
                .orca_ide_workspace
                .as_ref()
                .is_some_and(|w| w.read(cx).has_active_security_modal())
            {
                return ShellInputTarget::OrcaSecurityModal;
            }
            if sb.search_focused {
                return ShellInputTarget::SidebarSearch;
            }
            let ide_focused = !self.main_menu_drawer.read(cx).is_open
                && self
                    .orca_ide_workspace
                    .as_ref()
                    .is_some_and(|w| w.read(cx).has_active_text_input());
            return if ide_focused {
                ShellInputTarget::OrcaIdeInput
            } else {
                ShellInputTarget::Absorbed
            };
        }
        // Z #1: Base Workspace — OpenBotWorkspaceShell
        if self.active_panel == ActiveWorkspacePanel::OpenBot {
            return if !self.main_menu_drawer.read(cx).is_open && self.openbot_workspace.is_some() {
                ShellInputTarget::OpenBotInput
            } else {
                ShellInputTarget::Absorbed
            };
        }
        // Z #1: Base Workspace — Chats inline inputs
        if self.raw_apple_compose_input_active(cx) {
            return ShellInputTarget::AppleCompose;
        }
        if self.inspector.read(cx).is_bot_input_active() {
            return ShellInputTarget::InspectorBot;
        }
        if self.stage.read(cx).active_bot_card_field.is_some() {
            return ShellInputTarget::BotAssistCard;
        }
        if self.content_hub.read(cx).has_focused_input(false) {
            return ShellInputTarget::ContentHubInput;
        }
        if sb.search_focused {
            return ShellInputTarget::SidebarSearch;
        }
        if self.stage.read(cx).search_open {
            return ShellInputTarget::StageSearch;
        }
        if self.should_hide_composer(cx) {
            return ShellInputTarget::Absorbed;
        }
        ShellInputTarget::Composer
    }

    /// Returns `true` when any blocking modal or fullscreen overlay occludes global workspace shortcuts.
    /// Intentionally excludes Apple Compose (`is_plus_menu_open`) and workspace panels (`Settings`,
    /// `Calls`, `TaskCenter`, `McpTools`) so `Cmd+E`, `Cmd+[`, `Cmd+,`, `Cmd+1..8` remain functional.
    pub(crate) fn has_blocking_modal_or_fullscreen_overlay_for_shortcuts(&self, cx: &App) -> bool {
        let sb = self.sidebar.read(cx);
        self.overlays.is_passcode_locked
            || self.has_active_orca_security_modal(cx)
            || self.has_blocking_fullscreen_overlay(cx)
            || self.is_publish_review_modal_active(cx)
            || self.modal_container.read(cx).is_modal_active()
            || sb.is_contacts_modal_open
            || sb.active_creation_modal.is_some()
            || self
                .miniapps
                .create_bot_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
            || self.miniapps.pending_miniapp_confirm.is_some()
            || self.miniapps.miniapp_tos_modal_open
            || self
                .miniapps
                .bot_profile_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
    }

    /// Returns `true` when the active workspace is Settings and the current settings section has a focused text input.
    pub fn is_settings_input_active(&self, cx: &App) -> bool {
        self.resolve_input_target(cx) == ShellInputTarget::SettingsInput
    }

    /// Returns `true` when `TasksWorkspace` (TaskCenter / McpTools) holds an active studio text target.
    pub fn is_studio_input_active(&self, cx: &App) -> bool {
        self.resolve_input_target(cx) == ShellInputTarget::StudioInput
    }

    /// Returns `true` when an authenticated shell modal, overlay, drawer, or popover visually
    /// occludes `BrowserWorkspace`.
    pub fn has_browser_visual_occlusion_overlay(&self, cx: &App) -> bool {
        let sb_occluded = {
            let sb = self.sidebar.read(cx);
            sb.is_contacts_modal_open
                || sb.active_creation_modal.is_some()
                || sb.is_account_popover_open
        };
        sb_occluded
            || self.main_menu_drawer.read(cx).is_open
            || self.modal_container.read(cx).is_modal_active()
            || self.is_publish_review_modal_active(cx)
            || self.overlays.is_passcode_locked
            || self.orca_security_modal.is_some()
            || self.orca_jump_palette.is_some()
            || self.rtc_talk.talk_mode_overlay.is_some()
            || self.overlays.media_viewer_overlay.is_some()
            || self.overlays.iv_viewer_overlay.is_some()
            || self.overlays.computer_overlay.is_some()
            || self.miniapps.pending_miniapp_confirm.is_some()
            || self.miniapps.miniapp_tos_modal_open
            || self
                .miniapps
                .create_bot_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
            || self
                .miniapps
                .bot_profile_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
            || (self
                .miniapps
                .miniapp_overlay
                .as_ref()
                .is_some_and(|ov| ov.read(cx).is_open)
                && !self.miniapps.is_miniapp_minimized)
    }

    /// Returns `true` when any shell modal/drawer/overlay/popover/lockscreen/search blocks
    /// keyboard or IME routing to `BrowserWorkspace` inputs and `CdpViewportSurface`.
    pub fn has_browser_shell_blocker(&self, cx: &App) -> bool {
        self.sidebar.read(cx).search_focused || self.has_browser_visual_occlusion_overlay(cx)
    }

    /// Returns `true` when `BrowserWorkspace` is active and has an active text input target
    /// without an occluding shell overlay/modal/popover/lockscreen.
    pub fn is_browser_workspace_input_active(&self, cx: &App) -> bool {
        self.resolve_input_target(cx) == ShellInputTarget::BrowserInput
    }

    /// Returns `true` when any shell modal/drawer/overlay/popover/lockscreen occludes `CommunityWorkspace`.
    pub fn has_community_visual_occlusion_overlay(&self, cx: &App) -> bool {
        self.has_browser_visual_occlusion_overlay(cx)
            || (!self.theme_state.palette.is_apple_style()
                && self.sidebar.read(cx).is_plus_menu_open)
    }

    /// Returns `true` when `CommunityWorkspace` is active and holds the top-Z text input target.
    pub fn is_community_input_active(&self, cx: &App) -> bool {
        self.resolve_input_target(cx) == ShellInputTarget::CommunityInput
    }

    /// Returns `true` when `BrowserWorkspace`'s `CdpViewportSurface` holds GPUI focus
    /// and no shell modal/overlay/search/browser text input is active.
    pub fn is_browser_cdp_viewport_focused(&self, window: &Window, cx: &App) -> bool {
        self.active_panel == ActiveWorkspacePanel::Browser
            && !self.has_browser_shell_blocker(cx)
            && !self.is_browser_workspace_input_active(cx)
            && self.browser_workspace.as_ref().is_some_and(|bw| {
                let bw_read = bw.read(cx);
                !bw_read.is_externally_occluded() && bw_read.is_cdp_viewport_focused(window, cx)
            })
    }

    /// Returns `true` when `ActiveWorkspacePanel::OpenBot` is active without an occluding shell modal.
    pub fn is_openbot_workspace_input_active(&self, cx: &App) -> bool {
        self.resolve_input_target(cx) == ShellInputTarget::OpenBotInput
    }

    /// Returns `true` when the `ComputerWorkbench` overlay's Remote Terminal or interactive PTY is focused.
    pub fn is_computer_workbench_input_active(&self, cx: &App) -> bool {
        matches!(self.resolve_input_target(cx), ShellInputTarget::ComputerTerminal(_))
    }

    /// Returns `true` when `ContextInspector` is showing `BotConfigEditor` with an active input field.
    pub fn is_inspector_bot_input_active(&self, cx: &App) -> bool {
        self.resolve_input_target(cx) == ShellInputTarget::InspectorBot
    }

    /// Returns `true` when an in-transcript Bot Assist Vault Login card input field is focused.
    pub fn is_bot_card_input_active(&self, cx: &App) -> bool {
        self.resolve_input_target(cx) == ShellInputTarget::BotAssistCard
    }

    /// Returns `true` when in-chat transcript search (`ConversationStage::search_open`) is the
    /// active top-Z input target on the visible `Chats` workspace panel.
    pub fn is_stage_search_active(&self, cx: &App) -> bool {
        self.resolve_input_target(cx) == ShellInputTarget::StageSearch
    }

    /// Returns `true` when Apple Messages inline Compose mode (`收件人:`) should receive keyboard & IME input.
    pub fn is_apple_compose_input_active(&self, cx: &App) -> bool {
        self.resolve_input_target(cx) == ShellInputTarget::AppleCompose
    }

    pub(crate) fn set_composer_focused(&mut self, focused: bool, cx: &mut Context<Self>) {
        let effective = focused
            && self.is_chats_inline_unoccluded(cx)
            && !self.should_hide_composer(cx)
            && !self.is_content_hub_input_active(cx);
        if effective {
            self.composer.update(cx, |c, c_cx| {
                c.is_input_focused = true;
                c_cx.notify();
            });
        } else {
            self.blur_composer_and_cancel_ime(cx);
        }
    }

    /// Returns `true` when `PublishReviewModal` is open (over Chats or any ContentHub section).
    pub fn is_publish_review_modal_active(&self, cx: &App) -> bool {
        self.content_hub.read(cx).is_publish_review_modal_open()
    }

    /// Returns `true` when `PublishReviewModal` is open and has an active focused input field.
    pub fn is_publish_review_modal_input_active(&self, cx: &App) -> bool {
        self.content_hub.read(cx).publish_modal.has_focused_field()
    }

    /// Returns `true` when `PublishReviewModal` is the active top-Z IME/shortcut target (Z #14).
    pub fn is_publish_review_modal_ime_target(&self, cx: &App) -> bool {
        self.resolve_input_target(cx) == ShellInputTarget::PublishReviewModal
    }

    /// Returns `true` when a `ContentHubWorkspace` search bar or sidebar-adjacent Quick Drawer
    /// search bar is focused AND not occluded by any higher-Z target.
    pub fn is_content_hub_input_active(&self, cx: &App) -> bool {
        self.resolve_input_target(cx) == ShellInputTarget::ContentHubInput
    }

    /// Returns `true` when any non-composer IME owner actively owns IME preedit.
    /// Excludes `ShellInputTarget::Absorbed` (e.g. unfocused `ContentHub(_)`, fullscreen overlays,
    /// `should_hide_composer` pre-start bots) and `ShellInputTarget::Composer`.
    pub fn has_active_non_composer_ime_owner(&self, cx: &App) -> bool {
        self.resolve_input_target(cx).is_non_composer_ime_owner()
    }

    /// Returns `true` when keyboard/IME input should target a non-composer field or be absorbed.
    pub fn is_non_composer_text_capturing(&self, cx: &App) -> bool {
        self.resolve_input_target(cx) != ShellInputTarget::Composer
    }

    fn take_ime_preedit_commit(&mut self, cx: &mut Context<Self>) -> bool {
        self.bot_browser.ime_preedit_input_gen = None;
        self.community_ime_preedit_input_gen = None;
        if let Some(p) = self.ime.ime_preedit.take().filter(|s| !s.is_empty()) {
            self.insert_committed_text(&p, cx);
            true
        } else {
            false
        }
    }

    /// Atomically clears `self.ime` and ALL mirrored child IME preedit buffers without allocating
    /// lazy views and without canceling `self.composer.ime_machine`.
    pub fn clear_all_ime_preedits(&mut self, cx: &mut Context<Self>) {
        self.ime.clear_ime_state();
        self.bot_browser.ime_preedit_input_gen = None;
        self.community_ime_preedit_input_gen = None;
        if let Some(cw) = self.community_workspace.clone() {
            if cw.read(cx).active_input_ime_preedit().is_some() {
                cw.update(cx, |w, wcx| w.set_ime_preedit(None, wcx));
            }
        }
        self.last_ime_modal_snapshot = None;
        self.content_hub.update(cx, |h, hcx| {
            h.publish_modal.pending_close_ime_clear = false;
            if h.clear_all_preedits() {
                hcx.notify();
            }
        });
        if let Some(ov) = self.overlays.computer_overlay.clone() {
            ov.update(cx, |w, wcx| {
                let mut changed = false;
                if let Some(c) = w.terminal_input.as_ref() {
                    let dirty = {
                        let r = c.read(wcx);
                        r.ime_preedit.is_some()
                            || r.just_committed_ime
                            || r.last_ime_commit_at.is_some()
                    };
                    if dirty {
                        c.update(wcx, |ctrl, _| ctrl.clear_ime_state());
                        changed = true;
                    }
                }
                if let Some(term) = w.terminal_view.as_ref() {
                    if term.read(wcx).ime_preedit.is_some() {
                        term.update(wcx, |t, tcx| {
                            t.ime_preedit = None;
                            tcx.notify();
                        });
                        changed = true;
                    }
                }
                if changed {
                    wcx.notify();
                }
            });
        }
        self.settings_router.update(cx, |sr, scx| {
            sr.clear_all_preedits(scx);
        });
        self.inspector.update(cx, |ins, icx| {
            ins.clear_all_bot_preedits(icx);
        });
        if self.stage.read(cx).bot_assist_ime_preedit.is_some() {
            self.stage.update(cx, |st, stcx| {
                st.bot_assist_ime_preedit = None;
                stcx.notify();
            });
        }
        let auth_ime_dirty = {
            let w = self.auth_wizard.read(cx);
            w.ime.ime_preedit.is_some()
                || w.ime.just_committed_ime
                || w.ime.last_ime_commit_at.is_some()
        };
        if auth_ime_dirty {
            self.auth_wizard.update(cx, |w, wcx| {
                w.ime.clear_ime_state();
                wcx.notify();
            });
        }
    }

    /// Clears all shell/child IME preedits, cancels active composer IME composition, and
    /// optionally blurs any focused `ContentHubWorkspace` input on panel/modal/lock transitions.
    pub(crate) fn clear_ime_and_blur_content_hub_for_transition(
        &mut self,
        blur_content_hub: bool,
        cx: &mut Context<Self>,
    ) {
        self.clear_all_ime_preedits(cx);
        if self.composer.read(cx).ime_machine.is_composing() {
            self.composer.update(cx, |c, ccx| {
                c.ime_machine.on_ime_cancel();
                c.sync_autocomplete();
                ccx.notify();
            });
        }
        if blur_content_hub {
            self.content_hub.update(cx, |h, hcx| {
                if h.active_input != crate::views::content_hub::ContentHubInputTarget::None {
                    h.blur_input();
                    hcx.notify();
                }
            });
        }
    }

    fn target_accepts_ime_text(&self, target: &ShellInputTarget, cx: &App) -> bool {
        match target {
            ShellInputTarget::Absorbed | ShellInputTarget::OrcaSecurityModal => false,
            ShellInputTarget::PublishReviewModal => self.is_publish_review_modal_input_active(cx),
            ShellInputTarget::ModalContainer => self.modal_container.read(cx).dialog.has_text_input(),
            _ => true,
        }
    }

    fn combined_active_text(&self, cx: &App) -> String {
        let (committed, preedit) = self.active_text_and_preedit(cx);
        match preedit { Some(p) => format!("{committed}{p}"), None => committed }
    }

    fn active_utf16_len(&self, cx: &App) -> usize {
        let (committed, preedit) = self.active_text_and_preedit(cx);
        utf16_len(&committed) + preedit.as_deref().map_or(0, utf16_len)
    }

    /// Returns `(committed_text, optional_ime_preedit)` for the currently focused shell input field.
    pub fn active_text_and_preedit(&self, cx: &App) -> (String, Option<String>) {
        let pre = || self.ime.ime_preedit.clone();
        match self.resolve_input_target(cx) {
            ShellInputTarget::AuthWizard => self.auth_wizard.read(cx).active_text_and_preedit(),
            ShellInputTarget::PasscodeLock => (self.overlays.passcode_unlock_input.clone(), pre()),
            ShellInputTarget::OrcaSecurityModal => (String::new(), None),
            ShellInputTarget::OrcaJumpPalette => {
                let q = self
                    .orca_jump_palette
                    .as_ref()
                    .map(|jp| jp.read(cx).state().query.clone())
                    .unwrap_or_default();
                (q, pre())
            }
            ShellInputTarget::ComputerTerminal(ov) => {
                let (committed, term_pre) = ov.read(cx).terminal_text_and_preedit(cx);
                (committed, term_pre.or_else(pre))
            }
            ShellInputTarget::PublishReviewModal => self
                .content_hub
                .read(cx)
                .publish_modal
                .active_text_and_preedit(&self.ime),
            ShellInputTarget::ModalContainer => {
                let mc = self.modal_container.read(cx);
                if mc.dialog.has_text_input() {
                    (mc.dialog.active_field_text(), pre())
                } else {
                    (String::new(), None)
                }
            }
            ShellInputTarget::StudioInput => (String::new(), pre()),
            ShellInputTarget::SettingsInput => self
                .settings_router
                .read(cx)
                .active_input_text_and_preedit(&self.ime, cx)
                .unwrap_or_else(|| (String::new(), pre())),
            ShellInputTarget::CreationModal => {
                let sb = self.sidebar.read(cx);
                let txt = match sb.active_creation_field {
                    CreationInputField::PrimaryName => &sb.creation_primary_input,
                    CreationInputField::SecondaryHandle => &sb.creation_secondary_input,
                    CreationInputField::TertiaryPromptOrPhone => &sb.creation_tertiary_input,
                };
                (txt.clone(), pre())
            }
            ShellInputTarget::ContactsModal => {
                (self.sidebar.read(cx).contacts_search_query.clone(), pre())
            }
            ShellInputTarget::CreateBotBox(b) => {
                (b.read(cx).active_field_text().to_string(), pre())
            }
            ShellInputTarget::MiniAppInput(ov) => (ov.read(cx).active_input_text(), pre()),
            ShellInputTarget::AppleCompose => {
                let sb = self.sidebar.read(cx);
                if sb.compose_title_focused {
                    (sb.compose_draft.custom_group_title.clone(), pre())
                } else {
                    (sb.compose_recipient_query.clone(), pre())
                }
            }
            ShellInputTarget::InspectorBot => {
                let (committed, bot_pre) = self.inspector.read(cx).active_bot_input_text_and_preedit(cx);
                (committed, bot_pre.or_else(pre))
            }
            ShellInputTarget::BotAssistCard => {
                let st = self.stage.read(cx);
                (
                    st.active_bot_card_text().to_string(),
                    st.bot_assist_ime_preedit.clone().or_else(pre),
                )
            }
            ShellInputTarget::ContentHubInput => self
                .content_hub
                .read(cx)
                .active_input_text_and_preedit(&self.ime),
            ShellInputTarget::BrowserInput => {
                let txt = self
                    .browser_workspace
                    .as_ref()
                    .map(|bw| bw.read(cx).active_input_text())
                    .unwrap_or_default();
                (txt, pre())
            }
            ShellInputTarget::CommunityInput => {
                let (txt, cw_pre) = self
                    .community_workspace
                    .as_ref()
                    .map(|cw| {
                        let r = cw.read(cx);
                        (r.active_input_text(), r.active_input_ime_preedit())
                    })
                    .unwrap_or_else(|| (String::new(), None));
                (txt, cw_pre.or_else(pre))
            }
            ShellInputTarget::OrcaBotsInput | ShellInputTarget::OrcaIdeInput => {
                (String::new(), pre())
            }
            ShellInputTarget::OpenBotInput => {
                let txt = self
                    .openbot_workspace
                    .as_ref()
                    .map(|ws| {
                        let w = ws.read(cx);
                        if w.global_search.open {
                            w.global_search.query.clone()
                        } else if w.show_channel_create_dialog {
                            w.channel_editor.name.clone()
                        } else if w.conversation.search.open {
                            w.conversation.search.query.clone()
                        } else if w.active_page == crate::views::openbot::OpenBotPage::Agents
                            && w.agents.is_empty()
                            && w.active_target == crate::views::openbot::ActiveOpenBotTarget::None
                        {
                            w.first_agent_setup.purpose.clone()
                        } else {
                            w.composer.text.clone()
                        }
                    })
                    .unwrap_or_default();
                (txt, pre())
            }
            ShellInputTarget::SidebarSearch => {
                (self.sidebar.read(cx).search_query.clone(), pre())
            }
            ShellInputTarget::StageSearch => (self.stage.read(cx).search_query.clone(), pre()),
            ShellInputTarget::Composer => {
                let c = self.composer.read(cx);
                (
                    c.text().to_string(),
                    c.ime_machine.preedit_text().map(str::to_string),
                )
            }
            ShellInputTarget::Absorbed => (String::new(), None),
        }
    }

    /// Returns `true` when the currently active field has an uncommitted IME composition string.
    pub fn has_active_ime_preedit(&self, cx: &App) -> bool {
        self.active_text_and_preedit(cx).1.is_some_and(|p| !p.is_empty())
    }

    /// Clears all text in the currently focused input field.
    pub fn clear_active_field(&mut self, cx: &mut Context<Self>) {
        let target = self.resolve_input_target(cx);
        if target == ShellInputTarget::Absorbed {
            return;
        }
        self.clear_all_ime_preedits(cx);
        match target {
            ShellInputTarget::AuthWizard => {
                self.auth_wizard.update(cx, |w, wcx| w.clear_active_field(wcx));
            }
            ShellInputTarget::PasscodeLock => {
                self.overlays.passcode_unlock_input.clear();
                self.overlays.passcode_unlock_error = None;
            }
            ShellInputTarget::OrcaSecurityModal => return,
            ShellInputTarget::OrcaJumpPalette => {
                if let Some(jp) = self.orca_jump_palette.clone() {
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    jp.update(cx, |p, pcx| {
                        p.state_mut().set_query(String::new(), now_ms);
                        pcx.notify();
                    });
                }
            }
            ShellInputTarget::ComputerTerminal(ov) => {
                ov.update(cx, |w, wcx| w.clear_terminal_input(wcx));
            }
            ShellInputTarget::PublishReviewModal => {
                self.content_hub.update(cx, |h, hcx| {
                    let _ = h.publish_modal.clear_active_field();
                    hcx.notify();
                });
            }
            ShellInputTarget::ModalContainer => {
                if self.modal_container.read(cx).dialog.has_text_input() {
                    self.modal_container.update(cx, |mc, mcx| {
                        mc.dialog.clear_active_field();
                        mcx.notify();
                    });
                }
            }
            ShellInputTarget::StudioInput | ShellInputTarget::MiniAppInput(_) => {}
            ShellInputTarget::SettingsInput => {
                self.settings_router.update(cx, |sr, scx| {
                    sr.clear_active_input(scx);
                });
            }
            ShellInputTarget::CreationModal => {
                self.sidebar.update(cx, |sb, scx| {
                    sb.clear_creation_text();
                    scx.notify();
                });
            }
            ShellInputTarget::ContactsModal => {
                self.sidebar.update(cx, |sb, scx| {
                    sb.contacts_search_query.clear();
                    scx.notify();
                });
            }
            ShellInputTarget::CreateBotBox(box_ent) => {
                box_ent.update(cx, |b, bcx| {
                    b.clear_active_field();
                    bcx.notify();
                });
            }
            ShellInputTarget::AppleCompose => {
                self.sidebar.update(cx, |sb, scx| {
                    if sb.compose_title_focused {
                        sb.compose_draft.custom_group_title.clear();
                    } else {
                        sb.compose_recipient_query.clear();
                        sb.compose_draft.highlighted_index = 0;
                    }
                    scx.notify();
                });
            }
            ShellInputTarget::InspectorBot => {
                self.inspector.update(cx, |ins, icx| {
                    let _ = ins.clear_active_bot_input(icx);
                });
            }
            ShellInputTarget::BotAssistCard => {
                self.stage.update(cx, |st, stcx| {
                    st.bot_assist_ime_preedit = None;
                    st.clear_active_bot_card_field();
                    stcx.notify();
                });
            }
            ShellInputTarget::ContentHubInput => {
                self.content_hub.update(cx, |h, hcx| {
                    let _ = h.clear_active_input();
                    hcx.notify();
                });
            }
            ShellInputTarget::BrowserInput => {
                if let Some(bw) = self.browser_workspace.clone() {
                    bw.update(cx, |w, wcx| w.clear_active_input(wcx));
                }
            }
            ShellInputTarget::CommunityInput => {
                if let Some(cw) = self.community_workspace.clone() {
                    cw.update(cx, |w, wcx| {
                        w.set_ime_preedit(None, wcx);
                        w.clear_active_input(wcx);
                    });
                }
            }
            ShellInputTarget::OrcaBotsInput | ShellInputTarget::OrcaIdeInput => {}
            ShellInputTarget::OpenBotInput => {
                if let Some(ws) = self.openbot_workspace.clone() {
                    ws.update(cx, |w, wcx| {
                        if w.global_search.open {
                            w.global_search.set_query(String::new(), 0.0, w.now_ms);
                        } else if w.show_channel_create_dialog {
                            w.channel_editor.set_name(String::new());
                        } else if w.conversation.search.open {
                            w.conversation.run_search("");
                        } else if w.active_page == crate::views::openbot::OpenBotPage::Agents
                            && w.agents.is_empty()
                            && w.active_target == crate::views::openbot::ActiveOpenBotTarget::None
                        {
                            let _ = w.first_agent_setup.set_purpose(String::new());
                        } else {
                            w.composer.text.clear();
                            w.composer.close_picker();
                        }
                        wcx.notify();
                    });
                }
            }
            ShellInputTarget::SidebarSearch => {
                self.sidebar.update(cx, |sb, scx| {
                    sb.set_search_query("");
                    scx.notify();
                });
            }
            ShellInputTarget::StageSearch => {
                self.stage.update(cx, |st, stcx| {
                    st.search_query.clear();
                    st.recompute_search_matches();
                    stcx.notify();
                });
            }
            ShellInputTarget::Composer => {
                self.composer.update(cx, |c, ccx| {
                    c.clear();
                    ccx.notify();
                });
            }
            ShellInputTarget::Absorbed => return,
        }
        cx.notify();
    }

    /// Inserts sanitized committed/pasted text into the currently focused shell input target.
    pub fn insert_committed_text(&mut self, raw_text: &str, cx: &mut Context<Self>) {
        let target = self.resolve_input_target(cx);
        self.clear_all_ime_preedits(cx);
        let clean = sanitize_paste_text(raw_text, false);
        let clean_multiline = sanitize_paste_text(raw_text, true);
        let raw_passthrough = matches!(
            target,
            ShellInputTarget::BrowserInput
                | ShellInputTarget::CommunityInput
                | ShellInputTarget::OrcaBotsInput
                | ShellInputTarget::OrcaIdeInput
                | ShellInputTarget::OpenBotInput
        ) && !raw_text.is_empty();
        if clean.is_empty() && clean_multiline.is_empty() && !raw_passthrough {
            if target == ShellInputTarget::Composer
                && self.composer.read(cx).ime_machine.is_composing()
            {
                self.composer.update(cx, |c, ccx| {
                    c.ime_machine.on_ime_cancel();
                    c.sync_autocomplete();
                    ccx.notify();
                });
            }
            cx.notify();
            return;
        }
        if !matches!(
            target,
            ShellInputTarget::AuthWizard | ShellInputTarget::PasscodeLock
        ) {
            self.record_user_activity();
        }
        match target {
            ShellInputTarget::AuthWizard => {
                if !clean.is_empty() {
                    self.auth_wizard.update(cx, |w, wcx| w.insert_committed_text(&clean, wcx));
                }
            }
            ShellInputTarget::PasscodeLock => {
                if !clean.is_empty() {
                    self.overlays.passcode_unlock_input.push_str(&clean);
                    self.overlays.passcode_unlock_error = None;
                }
            }
            ShellInputTarget::OrcaSecurityModal => {}
            ShellInputTarget::OrcaJumpPalette => {
                if !clean.is_empty()
                    && let Some(jp) = self.orca_jump_palette.clone()
                {
                    jp.update(cx, |p, pcx| {
                        let _ = p.insert_committed_text(&clean, pcx);
                    });
                }
            }
            ShellInputTarget::ComputerTerminal(ov) => {
                if !clean.is_empty() {
                    ov.update(cx, |w, wcx| w.push_terminal_text(&clean, wcx));
                }
            }
            ShellInputTarget::PublishReviewModal => {
                let allow_nl = self
                    .content_hub
                    .read(cx)
                    .publish_modal
                    .is_active_field_multiline();
                let txt = if allow_nl { &clean_multiline } else { &clean };
                if !txt.is_empty() {
                    self.content_hub.update(cx, |h, hcx| {
                        let _ = h.publish_modal.insert_text(txt);
                        hcx.notify();
                    });
                }
            }
            ShellInputTarget::ModalContainer => {
                if !clean.is_empty() && self.modal_container.read(cx).dialog.has_text_input() {
                    self.modal_container.update(cx, |mc, mcx| {
                        mc.handle_key_input("", Some(&clean));
                        mcx.notify();
                    });
                }
            }
            ShellInputTarget::StudioInput => {
                if !clean.is_empty() {
                    self.tasks_workspace.update(cx, |w, wcx| {
                        w.apply_text_key_to_active_input("", Some(&clean));
                        wcx.notify();
                    });
                }
            }
            ShellInputTarget::SettingsInput => {
                if !clean.is_empty() {
                    self.settings_router.update(cx, |sr, scx| {
                        sr.push_input_text(&clean, scx);
                    });
                }
            }
            ShellInputTarget::CreationModal => {
                if !clean.is_empty() {
                    self.sidebar.update(cx, |sb, scx| {
                        sb.push_creation_text(&clean);
                        scx.notify();
                    });
                }
            }
            ShellInputTarget::ContactsModal => {
                if !clean.is_empty() {
                    self.sidebar.update(cx, |sb, scx| {
                        sb.contacts_search_query.push_str(&clean);
                        scx.notify();
                    });
                }
            }
            ShellInputTarget::CreateBotBox(box_ent) => {
                if !clean.is_empty() {
                    box_ent.update(cx, |b, bcx| {
                        b.push_text(&clean);
                        bcx.notify();
                    });
                }
            }
            ShellInputTarget::MiniAppInput(ov) => {
                if !clean.is_empty() {
                    ov.update(cx, |w, wcx| {
                        w.push_text_to_focused_input(&clean);
                        wcx.notify();
                    });
                }
            }
            ShellInputTarget::AppleCompose => {
                if !clean.is_empty() {
                    let title_focused = self.sidebar.read(cx).compose_title_focused;
                    if title_focused {
                        self.sidebar.update(cx, |sb, scx| {
                            let mut next = sb.compose_draft.custom_group_title.clone();
                            next.push_str(&clean);
                            sb.compose_draft.custom_group_title = next.chars().take(72).collect();
                            scx.notify();
                        });
                    } else if clean == "," || clean == "，" {
                        let (query_non_empty, menu_open, highlight) = {
                            let sb = self.sidebar.read(cx);
                            (
                                !sb.compose_recipient_query.trim().is_empty(),
                                sb.compose_draft.menu_open,
                                sb.compose_draft.highlighted_index,
                            )
                        };
                        let candidates = self.collect_apple_compose_recipients(cx);
                        if (query_non_empty || menu_open) && !candidates.is_empty() {
                            let idx = highlight.min(candidates.len() - 1);
                            let chosen = candidates[idx].clone();
                            self.sidebar.update(cx, |sb, scx| {
                                sb.compose_draft.is_group_mode = true;
                                scx.notify();
                            });
                            self.select_apple_compose_candidate(chosen, cx);
                        }
                    } else {
                        self.sidebar.update(cx, |sb, scx| {
                            sb.compose_recipient_query.push_str(&clean);
                            sb.compose_draft.highlighted_index = 0;
                            sb.compose_draft.menu_open = true;
                            scx.notify();
                        });
                    }
                }
            }
            ShellInputTarget::InspectorBot => {
                if !clean_multiline.is_empty() {
                    self.inspector.update(cx, |ins, icx| {
                        let _ = ins.push_bot_input_text(&clean_multiline, icx);
                    });
                }
            }
            ShellInputTarget::BotAssistCard => {
                if !clean.is_empty() {
                    self.stage.update(cx, |st, stcx| {
                        st.bot_assist_ime_preedit = None;
                        st.insert_bot_card_text(&clean);
                        stcx.notify();
                    });
                }
            }
            ShellInputTarget::ContentHubInput => {
                if !clean.is_empty() {
                    self.content_hub.update(cx, |h, hcx| {
                        let _ = h.insert_text_to_active_input(&clean);
                        hcx.notify();
                    });
                }
            }
            ShellInputTarget::BrowserInput => {
                if let Some(bw) = self.browser_workspace.clone() {
                    bw.update(cx, |w, wcx| {
                        w.insert_text_to_active_input(raw_text, wcx);
                    });
                }
            }
            ShellInputTarget::CommunityInput => {
                if let Some(cw) = self.community_workspace.clone() {
                    cw.update(cx, |w, wcx| {
                        w.set_ime_preedit(None, wcx);
                        w.insert_text_to_active_input(raw_text, wcx);
                    });
                }
            }
            ShellInputTarget::OrcaBotsInput => {
                if let Some(ob) = self.orca_bots_workspace.clone() {
                    ob.update(cx, |w, wcx| {
                        let _ = w.insert_committed_text(raw_text, wcx);
                    });
                }
            }
            ShellInputTarget::OrcaIdeInput => {
                if let Some(oi) = self.orca_ide_workspace.clone() {
                    oi.update(cx, |w, wcx| {
                        let _ = w.insert_committed_text(raw_text, wcx);
                    });
                }
            }
            ShellInputTarget::OpenBotInput => {
                if let Some(ws) = self.openbot_workspace.clone() {
                    ws.update(cx, |w, wcx| {
                        w.handle_text_input(raw_text);
                        wcx.notify();
                    });
                }
            }
            ShellInputTarget::SidebarSearch => {
                if !clean.is_empty() {
                    self.sidebar.update(cx, |sb, scx| {
                        let mut q = sb.search_query.clone();
                        q.push_str(&clean);
                        sb.set_search_query(q);
                        scx.notify();
                    });
                }
            }
            ShellInputTarget::StageSearch => {
                if !clean.is_empty() {
                    self.stage.update(cx, |st, stcx| {
                        st.search_query.push_str(&clean);
                        st.recompute_search_matches();
                        stcx.notify();
                    });
                }
            }
            ShellInputTarget::Composer => {
                self.composer.update(cx, |c, ccx| {
                    if !clean.is_empty() {
                        c.is_input_focused = true;
                        if c.ime_machine.is_composing() {
                            c.ime_machine.on_ime_commit(&clean);
                        } else {
                            c.ime_machine.insert_text(&clean);
                        }
                        c.sync_autocomplete();
                        ccx.notify();
                    } else if c.ime_machine.is_composing() {
                        c.ime_machine.on_ime_cancel();
                        c.sync_autocomplete();
                        ccx.notify();
                    }
                });
            }
            ShellInputTarget::Absorbed => {
                if matches!(self.active_panel, ActiveWorkspacePanel::ContentHub(_))
                    && !self.has_blocking_modal_or_fullscreen_overlay_for_shortcuts(cx)
                    && !self.sidebar.read(cx).is_account_popover_open
                    && self.content_hub.read(cx).active_game.is_some()
                {
                    let game_input = match clean.to_ascii_lowercase().as_str() {
                        "w" => Some(crate::views::content_hub::GameInput::MoveUp),
                        "s" => Some(crate::views::content_hub::GameInput::MoveDown),
                        "a" => Some(crate::views::content_hub::GameInput::MoveLeft),
                        "d" => Some(crate::views::content_hub::GameInput::MoveRight),
                        " " => Some(crate::views::content_hub::GameInput::PrimaryAction),
                        "1" => Some(crate::views::content_hub::GameInput::SelectOption(0)),
                        "2" => Some(crate::views::content_hub::GameInput::SelectOption(1)),
                        "3" => Some(crate::views::content_hub::GameInput::SelectOption(2)),
                        "4" => Some(crate::views::content_hub::GameInput::SelectOption(3)),
                        _ => None,
                    };
                    if let Some(inp) = game_input {
                        self.content_hub.update(cx, |h, hcx| {
                            if let Some(ref mut session) = h.active_game {
                                session.apply_input(inp);
                                hcx.notify();
                            }
                        });
                    }
                }
            }
        }
        cx.notify();
    }

    fn dismiss_pre_auth_escape(&mut self, is_shift: bool, cx: &mut Context<Self>) -> bool {
        // Step 0: Cancel active non-composer IME preedit before dismissing any overlay.
        let target = self.resolve_input_target(cx);
        if self.target_accepts_ime_text(&target, cx)
            && (self.ime.has_preedit()
                || (target.is_non_composer_ime_owner() && self.has_active_ime_preedit(cx)))
        {
            self.clear_all_ime_preedits(cx);
            return true;
        }
        // Step 0.5 — Z #15.8 / #15.5: Top-Z Orca SecuritySurfaceModal & ScopedJumpPalette
        if self.orca_security_modal.is_some() {
            self.close_orca_security_modal(cx);
            return true;
        }
        if self.orca_jump_palette.is_some() {
            self.close_scoped_jump_palette(cx);
            return true;
        }
        // Step 1 — Z #15: ComputerWorkbench
        if self.overlays.computer_overlay.is_some() {
            self.hide_remote_computer(cx);
            return true;
        }
        // Step 2 — Z #14: PublishReviewModal
        if self.is_publish_review_modal_active(cx) {
            self.close_publish_review_modal(cx);
            return true;
        }
        // Step 3 — Z #13: ModalContainer
        if self.modal_container.read(cx).is_modal_active() {
            self.clear_all_ime_preedits(cx);
            self.modal_container.update(cx, |mc, mcx| {
                mc.dismiss_top();
                mcx.notify();
            });
            self.community_pending_share = None;
            if self.composer.read(cx).is_compose_isolated
                && !self.is_compose_or_creation_modal_active(cx)
            {
                self.exit_compose_or_modal_isolation(true, cx);
            }
            return true;
        }
        // Step 4 — Z #12: AccountPopover
        if self.sidebar.read(cx).is_account_popover_open {
            self.clear_all_ime_preedits(cx);
            self.sidebar.update(cx, |s, scx| {
                s.is_account_popover_open = false;
                scx.notify();
            });
            return true;
        }
        // Step 5 — Z #11 / #10: McpTools / TaskCenter
        if matches!(
            self.active_panel,
            ActiveWorkspacePanel::McpTools | ActiveWorkspacePanel::TaskCenter
        ) {
            if self.tasks_workspace.read(cx).active_input != StudioInputTarget::None {
                self.clear_all_ime_preedits(cx);
                self.tasks_workspace.update(cx, |w, wcx| {
                    w.active_input = StudioInputTarget::None;
                    wcx.notify();
                });
                return true;
            }
            self.active_panel = ActiveWorkspacePanel::Chats;
            self.last_handled_nav_dest = Some(NavDestination::Chats);
            self.nav_rail.update(cx, |r, _| {
                r.set_active_destination(NavDestination::Chats);
            });
            self.sync_apple_sidebar_to_active_panel(cx);
            return true;
        }
        // Step 6 — Z #9: Settings (exact literal preserved for regression suite)
        if self.active_panel == ActiveWorkspacePanel::Settings {
            if self.is_settings_input_active(cx)
                && self
                    .settings_router
                    .update(cx, |sr, scx| sr.handle_non_char_key("escape", false, scx))
                    == Some(true)
            {
                self.clear_all_ime_preedits(cx);
                return true;
            }
            self.navigate_settings_back(cx);
            return true;
        }
        // Step 7 — Z #8: Calls
        if self.active_panel == ActiveWorkspacePanel::Calls {
            self.active_panel = ActiveWorkspacePanel::Chats;
            self.last_handled_nav_dest = Some(NavDestination::Chats);
            self.nav_rail.update(cx, |r, _| {
                r.set_active_destination(NavDestination::Chats);
            });
            self.sync_apple_sidebar_to_active_panel(cx);
            return true;
        }
        // Step 8 — Z #7: CreationModal
        if self.sidebar.read(cx).active_creation_modal.is_some() {
            self.clear_all_ime_preedits(cx);
            self.sidebar.update(cx, |sb, scx| {
                sb.close_creation_modal();
                scx.notify();
            });
            if !self.is_compose_or_creation_modal_active(cx) {
                self.exit_compose_or_modal_isolation(true, cx);
            } else {
                self.sync_compose_state_to_composer(true, cx);
                self.set_composer_focused(true, cx);
            }
            return true;
        }
        // Step 9 — Z #6: ContactsModal
        if self.sidebar.read(cx).is_contacts_modal_open {
            self.clear_all_ime_preedits(cx);
            self.sidebar.update(cx, |sb, scx| {
                sb.close_contacts_modal();
                scx.notify();
            });
            if !self.is_compose_or_creation_modal_active(cx) {
                self.exit_compose_or_modal_isolation(true, cx);
            } else {
                self.sync_compose_state_to_composer(true, cx);
            }
            return true;
        }
        // Step 10 — Z #5e / #5d / #5c: MiniAppToSModal / BotProfileBox / PendingMiniAppConfirm
        if self.miniapps.miniapp_tos_modal_open {
            self.clear_all_ime_preedits(cx);
            self.miniapps.miniapp_tos_modal_open = false;
            return true;
        }
        if let Some(bx) = self
            .miniapps
            .bot_profile_box
            .as_ref()
            .filter(|b| b.read(cx).is_visible)
            .cloned()
        {
            self.clear_all_ime_preedits(cx);
            bx.update(cx, |b, bcx| {
                b.close();
                bcx.notify();
            });
            self.miniapps.bot_profile_box = None;
            return true;
        }
        if self.miniapps.pending_miniapp_confirm.is_some() {
            self.clear_all_ime_preedits(cx);
            self.miniapps.pending_miniapp_confirm = None;
            return true;
        }
        // Step 11 — Z #5b: CreateManagedBotBox
        if self
            .miniapps
            .create_bot_box
            .as_ref()
            .is_some_and(|b| b.read(cx).is_visible)
        {
            self.clear_all_ime_preedits(cx);
            self.miniapps.create_bot_box = None;
            if !self.is_compose_or_creation_modal_active(cx) {
                self.exit_compose_or_modal_isolation(true, cx);
            }
            return true;
        }
        // Step 12 — Z #5a: MiniAppOverlay (unminimized)
        if let Some(ov) = self
            .miniapps
            .miniapp_overlay
            .as_ref()
            .filter(|ov| ov.read(cx).is_open && !self.miniapps.is_miniapp_minimized)
            .cloned()
        {
            self.clear_all_ime_preedits(cx);
            let mut minimized = false;
            ov.update(cx, |w, wcx| {
                if w.confirm_close_open {
                    w.confirm_close_open = false;
                } else if w.menu_open {
                    w.menu_open = false;
                } else if w.focused_input.is_some() {
                    w.focused_input = None;
                } else {
                    w.minimize();
                    w.focused_input = None;
                    minimized = true;
                }
                wcx.notify();
            });
            if minimized {
                self.miniapps.is_miniapp_minimized = true;
            }
            return true;
        }
        // Step 13 — Z #4: IvViewer
        if self.overlays.iv_viewer_overlay.is_some() {
            self.clear_all_ime_preedits(cx);
            self.overlays.iv_viewer_overlay = None;
            return true;
        }
        // Step 14 — Z #3: MediaViewer
        if self.overlays.media_viewer_overlay.is_some() {
            self.close_media_viewer(cx);
            return true;
        }
        // Step 15 — Z #2: TalkMode
        if self.rtc_talk.talk_mode_overlay.is_some() {
            self.clear_all_ime_preedits(cx);
            self.hangup_active_call(cx);
            return true;
        }
        // Step 16 — Z #1: Unoccluded Composer IME & Autocomplete only
        if !self.is_non_composer_text_capturing(cx) {
            if self.composer.read(cx).ime_machine.is_composing() {
                self.composer.update(cx, |c, ccx| {
                    c.ime_machine.on_ime_cancel();
                    c.sync_autocomplete();
                    ccx.notify();
                });
                return true;
            }
            if self.composer.update(cx, |c, ccx| {
                let ok = c.handle_hermes_autocomplete_nav("escape", is_shift);
                if ok {
                    ccx.notify();
                }
                ok
            }) {
                return true;
            }
        }
        false
    }

    fn dismiss_post_auth_escape(&mut self, cx: &mut Context<Self>) -> bool {
        // Reached ONLY when Z #2..#16 overlays/panels are all closed (Z #1 Base Workspace).
        if self.is_content_hub_input_active(cx)
            && self.content_hub.read(cx).active_input
                == crate::views::content_hub::ContentHubInputTarget::QuickDrawerSearch
        {
            self.toggle_quick_creations_drawer(false, cx);
            return true;
        }
        if self.active_panel == ActiveWorkspacePanel::Chats
            && self.theme_state.palette.is_apple_style()
            && self.sidebar.read(cx).is_plus_menu_open
        {
            // Progressive 3-step Grok Bot Esc cascade:
            // (0) blur custom title -> (1) clear query -> (2) close dropdown -> (3) exit compose mode
            let mut exited_compose = false;
            self.sidebar.update(cx, |sb, scx| {
                if sb.compose_title_focused {
                    sb.compose_title_focused = false;
                } else if !sb.compose_recipient_query.is_empty() {
                    sb.compose_recipient_query.clear();
                    sb.compose_draft.highlighted_index = 0;
                } else if sb.compose_draft.menu_open {
                    sb.compose_draft.menu_open = false;
                } else {
                    sb.cancel_compose_mode();
                    exited_compose = true;
                }
                scx.notify();
            });
            if exited_compose {
                if !self.is_compose_or_creation_modal_active(cx) {
                    self.exit_compose_or_modal_isolation(true, cx);
                } else {
                    self.sync_compose_state_to_composer(true, cx);
                    self.set_composer_focused(true, cx);
                }
            }
            return true;
        }
        if matches!(
            self.active_panel,
            ActiveWorkspacePanel::Chats
                | ActiveWorkspacePanel::Browser
                | ActiveWorkspacePanel::Community
        ) && self.sidebar.read(cx).is_plus_menu_open
            && (self.active_panel == ActiveWorkspacePanel::Chats
                || !self.theme_state.palette.is_apple_style())
        {
            self.clear_all_ime_preedits(cx);
            self.sidebar.update(cx, |s, scx| {
                s.cancel_compose_mode();
                scx.notify();
            });
            if !self.is_compose_or_creation_modal_active(cx) {
                self.exit_compose_or_modal_isolation(true, cx);
            } else {
                self.sync_compose_state_to_composer(true, cx);
            }
            return true;
        }
        if self.main_menu_drawer.read(cx).is_open {
            self.main_menu_drawer.update(cx, |m, mcx| {
                m.close();
                mcx.notify();
            });
            return true;
        }
        if self.content_hub.read(cx).quick_drawer_open {
            self.toggle_quick_creations_drawer(false, cx);
            return true;
        }
        if self.is_content_hub_input_active(cx) {
            self.clear_all_ime_preedits(cx);
            self.content_hub.update(cx, |h, hcx| {
                h.blur_input();
                hcx.notify();
            });
            return true;
        }
        if self.active_panel == ActiveWorkspacePanel::Browser {
            if self.sidebar.read(cx).search_focused {
                self.clear_all_ime_preedits(cx);
                self.sidebar.update(cx, |sb, scx| {
                    sb.exit_search();
                    scx.notify();
                });
                return true;
            }
            let design_picker_active = self.browser_workspace.as_ref().is_some_and(|bw| {
                let r = bw.read(cx);
                r.design_mode.enabled && r.design_mode.picker_active
            });
            if self.is_browser_workspace_input_active(cx) || design_picker_active {
                if let Some(bw) = self.browser_workspace.clone() {
                    let handled =
                        bw.update(cx, |w, wcx| w.handle_non_char_key_input("escape", false, wcx));
                    if handled {
                        return true;
                    }
                }
            }
            self.hide_browser_native_webview(cx);
            self.active_panel = ActiveWorkspacePanel::Chats;
            self.last_handled_nav_dest = Some(NavDestination::Chats);
            self.nav_rail.update(cx, |r, _| {
                r.set_active_destination(NavDestination::Chats);
            });
            self.sync_apple_sidebar_to_active_panel(cx);
            self.reconcile_chat_scoped_toasts(cx);
            return true;
        }
        if self.active_panel == ActiveWorkspacePanel::OpenBot {
            if let Some(ws) = self.openbot_workspace.clone() {
                ws.update(cx, |w, wcx| {
                    w.handle_escape();
                    wcx.notify();
                });
                return true;
            }
        }
        if matches!(self.active_panel, ActiveWorkspacePanel::ContentHub(_)) {
            let closed_game_state = self.content_hub.update(cx, |h, hcx| {
                if let Some(session) = h.active_game.take() {
                    let st = (session.creation_id.clone(), session.to_upsert_state_request());
                    hcx.notify();
                    Some(st)
                } else {
                    None
                }
            });
            if let Some((cid, req)) = closed_game_state {
                self.content_hub_state.bump_game_tick_gen();
                self.upsert_creation_state_fenced(cid, req, cx);
                return true;
            }
            let closed_reader_state = self.content_hub.update(cx, |h, hcx| {
                if let Some(reader) = h.book_reader.take() {
                    let st = (reader.creation_id.clone(), reader.to_upsert_state_request());
                    hcx.notify();
                    Some(st)
                } else {
                    None
                }
            });
            if let Some((cid, req)) = closed_reader_state {
                self.upsert_creation_state_fenced(cid, req, cx);
                return true;
            }
            let closed_detail = self.content_hub.update(cx, |h, hcx| {
                if h.selected_creation_id.is_some()
                    || h.selected_detail_creation.is_some()
                    || h.selected_catalog_item.is_some()
                {
                    h.selected_creation_id = None;
                    h.selected_catalog_item = None;
                    h.selected_detail_creation = None;
                    hcx.notify();
                    true
                } else {
                    false
                }
            });
            if closed_detail {
                return true;
            }
        }
        if self.active_panel == ActiveWorkspacePanel::Community {
            if self.sidebar.read(cx).search_focused {
                self.clear_all_ime_preedits(cx);
                self.sidebar.update(cx, |sb, scx| {
                    sb.exit_search();
                    scx.notify();
                });
                return true;
            }
            if let Some(cw) = self.community_workspace.clone() {
                if cw.update(cx, |w, wcx| w.handle_escape_key(wcx)) {
                    return true;
                }
            }
            if !self.sidebar.read(cx).search_query.is_empty() {
                self.clear_all_ime_preedits(cx);
                self.sidebar.update(cx, |sb, scx| {
                    sb.exit_search();
                    scx.notify();
                });
                return true;
            }
            self.leave_community_panel_if_active(cx);
            self.active_panel = ActiveWorkspacePanel::Chats;
            self.last_handled_nav_dest = Some(NavDestination::Chats);
            let nav_trigger = self.nav_rail.update(cx, |r, rcx| {
                r.set_active_destination(NavDestination::Chats);
                rcx.notify();
                r.trigger_counter
            });
            self.last_handled_nav_trigger = nav_trigger;
            self.sync_apple_sidebar_to_active_panel(cx);
            self.reconcile_chat_scoped_toasts(cx);
            return true;
        }
        if self.active_panel == ActiveWorkspacePanel::OrcaBots {
            if self.sidebar.read(cx).search_focused {
                self.clear_all_ime_preedits(cx);
                self.sidebar.update(cx, |sb, scx| {
                    sb.exit_search();
                    scx.notify();
                });
                return true;
            }
            if let Some(ob) = self.orca_bots_workspace.clone() {
                let esc_ev = KeyDownEvent {
                    keystroke: Keystroke {
                        modifiers: Modifiers::default(),
                        key: "escape".to_string(),
                        key_char: None,
                    },
                    is_held: false,
                };
                if ob.update(cx, |w, wcx| w.handle_key_down(&esc_ev, wcx)) {
                    return true;
                }
            }
            self.active_panel = ActiveWorkspacePanel::Chats;
            self.last_handled_nav_dest = Some(NavDestination::Chats);
            self.sync_nav_rail_destination_silent(NavDestination::Chats, cx);
            self.sync_apple_sidebar_to_active_panel(cx);
            self.reconcile_chat_scoped_toasts(cx);
            return true;
        }
        if self.active_panel == ActiveWorkspacePanel::OrcaIde {
            if self.sidebar.read(cx).search_focused {
                self.clear_all_ime_preedits(cx);
                self.sidebar.update(cx, |sb, scx| {
                    sb.exit_search();
                    scx.notify();
                });
                return true;
            }
            if let Some(oi) = self.orca_ide_workspace.clone() {
                let esc_ev = KeyDownEvent {
                    keystroke: Keystroke {
                        modifiers: Modifiers::default(),
                        key: "escape".to_string(),
                        key_char: None,
                    },
                    is_held: false,
                };
                if oi.update(cx, |w, wcx| w.handle_key_down(&esc_ev, wcx)) {
                    return true;
                }
            }
            self.active_panel = ActiveWorkspacePanel::Chats;
            self.last_handled_nav_dest = Some(NavDestination::Chats);
            self.sync_nav_rail_destination_silent(NavDestination::Chats, cx);
            self.sync_apple_sidebar_to_active_panel(cx);
            self.reconcile_chat_scoped_toasts(cx);
            return true;
        }
        if self.active_panel != ActiveWorkspacePanel::Chats {
            return false;
        }
        if self.stage.read(cx).active_context_menu.is_some()
            || self.stage.read(cx).reaction_picker_msg_id.is_some()
        {
            self.stage.update(cx, |st, stcx| {
                st.active_context_menu = None;
                st.context_menu_reactions_expanded = false;
                st.reaction_picker_msg_id = None;
                stcx.notify();
            });
            return true;
        }
        if self.stage.read(cx).more_menu_open {
            self.stage.update(cx, |st, stcx| {
                st.more_menu_open = false;
                st.ttl_submenu_open = false;
                stcx.notify();
            });
            return true;
        }
        if self.stage.read(cx).is_selecting_messages() {
            self.stage.update(cx, |st, stcx| {
                st.clear_selected_messages();
                stcx.notify();
            });
            return true;
        }
        if self.is_bot_card_input_active(cx) {
            self.clear_all_ime_preedits(cx);
            self.stage.update(cx, |st, stcx| {
                st.blur_bot_card_input();
                stcx.notify();
            });
            return true;
        }
        if self.inspector.read(cx).is_open {
            self.set_inspector_open(false, cx);
            return true;
        }
        if self.stage.read(cx).search_open {
            self.stage.update(cx, |st, stcx| {
                st.search_open = false;
                st.search_query.clear();
                st.search_match_ids.clear();
                st.search_match_cursor = 0;
                stcx.notify();
            });
            self.set_composer_focused(true, cx);
            return true;
        }
        if self.sidebar.read(cx).search_focused || !self.sidebar.read(cx).search_query.is_empty() {
            self.sidebar.update(cx, |sb, scx| {
                sb.exit_search();
                scx.notify();
            });
            self.set_composer_focused(true, cx);
            return true;
        }
        if self.composer.read(cx).replying_to.is_some()
            || self.stage.read(cx).pending_reply.is_some()
        {
            self.composer.update(cx, |c, ccx| {
                c.cancel_reply_or_edit();
                ccx.notify();
            });
            self.stage.update(cx, |st, stcx| {
                st.pending_reply = None;
                stcx.notify();
            });
            return true;
        }
        false
    }

    fn handle_escape_key(&mut self, is_shift: bool, cx: &mut Context<Self>) -> bool {
        if !self.is_authenticated {
            if self.auth_wizard.update(cx, |w, wcx| {
                w.handle_non_char_key("escape", is_shift, false, wcx)
            }) {
                cx.notify();
                return true;
            }
            if self.ime.has_preedit() {
                self.clear_all_ime_preedits(cx);
                cx.notify();
                return true;
            }
            return false;
        }
        if self.overlays.is_passcode_locked {
            self.clear_all_ime_preedits(cx);
            self.overlays.passcode_unlock_input.clear();
            self.overlays.passcode_unlock_error = None;
            cx.notify();
            return true;
        }
        if self.dismiss_pre_auth_escape(is_shift, cx) || self.dismiss_post_auth_escape(cx) {
            if self.active_panel == ActiveWorkspacePanel::Browser {
                let occluded = self.has_browser_visual_occlusion_overlay(cx);
                if let Some(ref bw) = self.browser_workspace {
                    let needs_sync = bw.read(cx).is_externally_occluded() != occluded;
                    if needs_sync {
                        bw.update(cx, |w, wcx| w.set_occluded(occluded, wcx));
                    }
                }
            }
            cx.notify();
            return true;
        }
        false
    }

    /// Handles non-character navigation and editing keys (`escape`, `enter`, `backspace`, `tab`, arrows).
    pub fn handle_non_char_key(&mut self, key: &str, is_shift: bool, cx: &mut Context<Self>) -> bool {
        self.handle_non_char_key_with_alt(key, is_shift, false, cx)
    }

    /// Handles non-character navigation and editing keys with optional `Alt/Option` word deletion modifier.
    pub fn handle_non_char_key_with_alt(
        &mut self,
        key: &str,
        is_shift: bool,
        is_alt: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if key == "escape" {
            return self.handle_escape_key(is_shift, cx);
        }
        let target = self.resolve_input_target(cx);
        if !matches!(
            target,
            ShellInputTarget::AuthWizard | ShellInputTarget::PasscodeLock
        ) {
            self.record_user_activity();
        }
        match target {
            ShellInputTarget::AuthWizard => {
                let handled = self
                    .auth_wizard
                    .update(cx, |w, wcx| w.handle_non_char_key(key, is_shift, is_alt, wcx));
                if handled {
                    cx.notify();
                }
                handled
            }
            ShellInputTarget::PasscodeLock => {
                match key {
                    "backspace" => {
                        if self.ime.ime_preedit.take().is_none() {
                            backspace_string(&mut self.overlays.passcode_unlock_input, is_alt);
                            self.overlays.passcode_unlock_error = None;
                        }
                        cx.notify();
                    }
                    "enter" => {
                        if !(self.take_ime_preedit_commit(cx) || self.ime.take_just_committed()) {
                            self.submit_passcode_unlock_input(cx);
                        }
                    }
                    _ => {}
                }
                true
            }
            ShellInputTarget::OrcaSecurityModal => true,
            ShellInputTarget::OrcaJumpPalette => {
                if key == "backspace" && self.ime.ime_preedit.take().is_some() {
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    cx.notify();
                    return true;
                }
                if let Some(jp) = self.orca_jump_palette.clone() {
                    if key == "tab" {
                        self.clear_all_ime_preedits(cx);
                        let handled = jp.update(cx, |p, pcx| p.handle_tab_chip(pcx));
                        cx.notify();
                        return handled;
                    }
                    let ev = KeyDownEvent {
                        keystroke: Keystroke {
                            modifiers: Modifiers {
                                shift: is_shift,
                                alt: is_alt,
                                ..Default::default()
                            },
                            key: key.to_string(),
                            key_char: None,
                        },
                        is_held: false,
                    };
                    let handled = jp.update(cx, |p, pcx| p.handle_key_down(&ev, pcx));
                    self.drain_jump_palette_selection(cx);
                    cx.notify();
                    return handled;
                }
                true
            }
            ShellInputTarget::ComputerTerminal(ov) => {
                if key == "backspace"
                    && (self.ime.has_preedit() || self.has_active_ime_preedit(cx))
                {
                    self.clear_all_ime_preedits(cx);
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    return true;
                }
                let handled = ov.update(cx, |w, wcx| {
                    w.handle_terminal_non_char_key(key, is_alt, wcx)
                });
                if handled {
                    cx.notify();
                }
                handled
            }
            ShellInputTarget::PublishReviewModal => {
                if key == "backspace"
                    && (self.ime.has_preedit() || self.has_active_ime_preedit(cx))
                {
                    self.clear_all_ime_preedits(cx);
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    return true;
                }
                if key == "tab" {
                    self.clear_all_ime_preedits(cx);
                }
                self.content_hub.update(cx, |h, hcx| {
                    let _ = h.publish_modal.handle_non_char_key(key, is_shift, is_alt);
                    hcx.notify();
                });
                cx.notify();
                true
            }
            ShellInputTarget::ModalContainer => {
                if key == "backspace" && self.ime.has_preedit() {
                    self.clear_all_ime_preedits(cx);
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    return true;
                }
                if key == "tab" {
                    self.clear_all_ime_preedits(cx);
                }
                let handled = self.modal_container.update(cx, |mc, mcx| {
                    let ok = mc.handle_key_input(key, None);
                    if ok {
                        mcx.notify();
                    }
                    ok
                });
                if let Some(outcome) = self.modal_container.update(cx, |mc, _| mc.take_outcome()) {
                    self.handle_modal_outcome(outcome, cx);
                }
                if handled {
                    cx.notify();
                }
                true
            }
            ShellInputTarget::StudioInput => {
                if key == "backspace" && self.ime.has_preedit() {
                    self.clear_all_ime_preedits(cx);
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    return true;
                }
                if key == "tab" {
                    self.clear_all_ime_preedits(cx);
                }
                let handled = self.tasks_workspace.update(cx, |w, wcx| {
                    let ok = w.apply_text_key_to_active_input(key, None);
                    if ok {
                        wcx.notify();
                    }
                    ok
                });
                if handled {
                    cx.notify();
                }
                handled
            }
            ShellInputTarget::SettingsInput => {
                if key == "backspace"
                    && (self.ime.has_preedit() || self.has_active_ime_preedit(cx))
                {
                    self.clear_all_ime_preedits(cx);
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    return true;
                }
                if key == "tab" {
                    self.clear_all_ime_preedits(cx);
                }
                if let Some(handled) = self
                    .settings_router
                    .update(cx, |sr, scx| sr.handle_non_char_key(key, is_shift, scx))
                {
                    if handled {
                        cx.notify();
                    }
                    handled
                } else {
                    false
                }
            }
            ShellInputTarget::CreationModal => {
                match key {
                    "tab" => {
                        self.clear_all_ime_preedits(cx);
                        self.sidebar.update(cx, |sb, scx| {
                            sb.cycle_creation_field();
                            scx.notify();
                        });
                    }
                    "backspace" => {
                        if self.ime.has_preedit() {
                            self.clear_all_ime_preedits(cx);
                        } else {
                            self.sidebar.update(cx, |sb, scx| {
                                sb.backspace_creation_text();
                                scx.notify();
                            });
                        }
                    }
                    "enter" => {
                        if self.take_ime_preedit_commit(cx) || self.ime.take_just_committed() {
                            return true;
                        }
                        self.sidebar.update(cx, |sb, scx| {
                            sb.submit_creation_modal();
                            scx.notify();
                        });
                    }
                    _ => return false,
                }
                cx.notify();
                true
            }
            ShellInputTarget::ContactsModal => match key {
                "backspace" => {
                    if self.ime.has_preedit() {
                        self.clear_all_ime_preedits(cx);
                    } else {
                        self.sidebar.update(cx, |sb, scx| {
                            backspace_string(&mut sb.contacts_search_query, is_alt);
                            scx.notify();
                        });
                    }
                    cx.notify();
                    true
                }
                "enter" => {
                    let _ = self.take_ime_preedit_commit(cx) || self.ime.take_just_committed();
                    cx.notify();
                    true
                }
                _ => false,
            },
            ShellInputTarget::CreateBotBox(box_ent) => {
                match key {
                    "tab" => {
                        self.clear_all_ime_preedits(cx);
                        box_ent.update(cx, |b, bcx| {
                            b.cycle_active_field();
                            bcx.notify();
                        });
                    }
                    "backspace" => {
                        if self.ime.has_preedit() {
                            self.clear_all_ime_preedits(cx);
                        } else {
                            box_ent.update(cx, |b, bcx| {
                                b.backspace();
                                bcx.notify();
                            });
                        }
                    }
                    "enter" => {
                        if !(self.take_ime_preedit_commit(cx) || self.ime.take_just_committed()) {
                            box_ent.update(cx, |b, bcx| {
                                b.submit();
                                bcx.notify();
                            });
                        }
                    }
                    _ => return false,
                }
                cx.notify();
                true
            }
            ShellInputTarget::MiniAppInput(ov) => match key {
                "backspace" => {
                    if self.ime.has_preedit() {
                        self.clear_all_ime_preedits(cx);
                    } else {
                        ov.update(cx, |w, wcx| {
                            w.backspace_focused_input();
                            wcx.notify();
                        });
                    }
                    cx.notify();
                    true
                }
                "enter" => {
                    if !(self.take_ime_preedit_commit(cx) || self.ime.take_just_committed()) {
                        ov.update(cx, |w, wcx| {
                            w.submit_focused_input();
                            wcx.notify();
                        });
                    }
                    cx.notify();
                    true
                }
                _ => false,
            },
            ShellInputTarget::AppleCompose => {
                if self.sidebar.read(cx).compose_title_focused {
                    match key {
                        "backspace" => {
                            if self.ime.has_preedit() {
                                self.clear_all_ime_preedits(cx);
                            } else {
                                self.sidebar.update(cx, |sb, scx| {
                                    backspace_string(
                                        &mut sb.compose_draft.custom_group_title,
                                        is_alt,
                                    );
                                    scx.notify();
                                });
                            }
                            cx.notify();
                            return true;
                        }
                        "enter" | "tab" => {
                            if key == "enter"
                                && (self.take_ime_preedit_commit(cx)
                                    || self.ime.take_just_committed())
                            {
                                return true;
                            }
                            self.clear_all_ime_preedits(cx);
                            self.sidebar.update(cx, |sb, scx| {
                                sb.compose_title_focused = false;
                                scx.notify();
                            });
                            self.set_composer_focused(true, cx);
                            cx.notify();
                            return true;
                        }
                        _ => return false,
                    }
                }
                match key {
                    "backspace" => {
                        if self.ime.ime_preedit.take().is_none() {
                            let popped_chip = self.sidebar.update(cx, |sb, scx| {
                                let popped = if sb.compose_recipient_query.is_empty() {
                                    sb.pop_last_compose_recipient().is_some()
                                } else {
                                    backspace_string(&mut sb.compose_recipient_query, is_alt);
                                    sb.compose_draft.highlighted_index = 0;
                                    sb.compose_draft.menu_open = true;
                                    false
                                };
                                scx.notify();
                                popped
                            });
                            if popped_chip {
                                self.sync_compose_state_to_composer(true, cx);
                            }
                        }
                        cx.notify();
                        true
                    }
                    "down" | "arrowdown" => {
                        let total = self.collect_apple_compose_recipients(cx).len();
                        self.sidebar.update(cx, |sb, scx| {
                            sb.move_compose_highlight(1, total);
                            scx.notify();
                        });
                        cx.notify();
                        true
                    }
                    "up" | "arrowup" => {
                        let total = self.collect_apple_compose_recipients(cx).len();
                        self.sidebar.update(cx, |sb, scx| {
                            sb.move_compose_highlight(-1, total);
                            scx.notify();
                        });
                        cx.notify();
                        true
                    }
                    "enter" => {
                        if self.take_ime_preedit_commit(cx) || self.ime.take_just_committed() {
                            return true;
                        }
                        let (query_empty, menu_open, highlight, selected) = {
                            let sb = self.sidebar.read(cx);
                            (
                                sb.compose_recipient_query.trim().is_empty(),
                                sb.compose_draft.menu_open,
                                sb.compose_draft.highlighted_index,
                                sb.compose_draft.selected_recipients.clone(),
                            )
                        };
                        let recipients = self.collect_apple_compose_recipients(cx);
                        if !query_empty {
                            if let Some(chosen) = recipients
                                .get(highlight.min(recipients.len().saturating_sub(1)))
                                .cloned()
                            {
                                self.select_apple_compose_candidate(chosen, cx);
                            } else {
                                self.open_add_friend_from_compose_query(cx);
                            }
                        } else if selected.len() == 1 {
                            self.commit_single_compose_recipient_to_dm(selected[0].clone(), cx);
                        } else if menu_open && !recipients.is_empty() {
                            let idx = highlight.min(recipients.len() - 1);
                            self.select_apple_compose_candidate(recipients[idx].clone(), cx);
                        } else if !selected.is_empty() {
                            self.set_composer_focused(true, cx);
                        }
                        cx.notify();
                        true
                    }
                    "tab" => {
                        self.clear_all_ime_preedits(cx);
                        let (query_empty, menu_open, highlight, selected_len) = {
                            let sb = self.sidebar.read(cx);
                            (
                                sb.compose_recipient_query.trim().is_empty(),
                                sb.compose_draft.menu_open,
                                sb.compose_draft.highlighted_index,
                                sb.compose_draft.selected_recipients.len(),
                            )
                        };
                        if !query_empty {
                            let recipients = self.collect_apple_compose_recipients(cx);
                            if let Some(chosen) = recipients
                                .get(highlight.min(recipients.len().saturating_sub(1)))
                                .cloned()
                            {
                                self.sidebar.update(cx, |sb, scx| {
                                    sb.compose_draft.is_group_mode = true;
                                    scx.notify();
                                });
                                self.select_apple_compose_candidate(chosen, cx);
                            } else {
                                self.set_composer_focused(true, cx);
                            }
                        } else if selected_len >= 1 {
                            self.sidebar.update(cx, |sb, scx| {
                                sb.compose_draft.menu_open = false;
                                scx.notify();
                            });
                            self.set_composer_focused(true, cx);
                        } else if menu_open {
                            let recipients = self.collect_apple_compose_recipients(cx);
                            if let Some(chosen) = recipients
                                .get(highlight.min(recipients.len().saturating_sub(1)))
                                .cloned()
                            {
                                self.sidebar.update(cx, |sb, scx| {
                                    sb.compose_draft.is_group_mode = true;
                                    scx.notify();
                                });
                                self.select_apple_compose_candidate(chosen, cx);
                            } else {
                                self.set_composer_focused(true, cx);
                            }
                        } else {
                            self.set_composer_focused(true, cx);
                        }
                        cx.notify();
                        true
                    }
                    _ => false,
                }
            }
            ShellInputTarget::InspectorBot => {
                if key == "backspace"
                    && (self.ime.has_preedit() || self.has_active_ime_preedit(cx))
                {
                    self.clear_all_ime_preedits(cx);
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    return true;
                }
                if key == "tab" {
                    self.clear_all_ime_preedits(cx);
                }
                let handled = self
                    .inspector
                    .update(cx, |ins, icx| ins.handle_bot_non_char_key(key, is_alt, icx));
                if handled {
                    cx.notify();
                }
                handled
            }
            ShellInputTarget::BotAssistCard => {
                match key {
                    "tab" => {
                        self.clear_all_ime_preedits(cx);
                        self.stage.update(cx, |st, stcx| {
                            st.bot_assist_ime_preedit = None;
                            st.cycle_bot_card_field(is_shift);
                            stcx.notify();
                        });
                    }
                    "backspace" => {
                        if self.ime.has_preedit() || self.has_active_ime_preedit(cx) {
                            self.clear_all_ime_preedits(cx);
                        } else {
                            self.stage.update(cx, |st, stcx| {
                                st.backspace_bot_card_field(is_alt);
                                stcx.notify();
                            });
                        }
                    }
                    "enter" => {
                        if self.take_ime_preedit_commit(cx) || self.ime.take_just_committed() {
                            return true;
                        }
                        let submitted = self.stage.update(cx, |st, stcx| {
                            let cid = st.active_bot_card_field.as_ref().map(|(id, _)| id.clone());
                            if let Some(ref card_id) = cid {
                                st.pending_bot_assist_action =
                                    Some(BotAssistCardAction::SubmitLogin {
                                        card_id: card_id.clone(),
                                    });
                                stcx.notify();
                            }
                            cid.is_some()
                        });
                        if submitted {
                            self.handle_stage_pending_actions(cx);
                        }
                    }
                    _ => return false,
                }
                cx.notify();
                true
            }
            ShellInputTarget::ContentHubInput => {
                if key == "backspace"
                    && (self.ime.has_preedit() || self.has_active_ime_preedit(cx))
                {
                    self.clear_all_ime_preedits(cx);
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    return true;
                }
                if key == "tab" {
                    self.clear_all_ime_preedits(cx);
                    cx.notify();
                    return true;
                }
                if matches!(key, "up" | "arrowup" | "down" | "arrowdown") {
                    // Consume cleanly so QuickDrawerSearch over Chats never leaks arrows to composer autocomplete.
                    return true;
                }
                let (handled, active_target) = self.content_hub.update(cx, |h, hcx| {
                    let ok = h.handle_non_char_key_for_active_input(key, is_alt);
                    if ok {
                        hcx.notify();
                    }
                    (ok, h.active_input)
                });
                if handled {
                    if key == "enter"
                        && matches!(
                            active_target,
                            crate::views::content_hub::ContentHubInputTarget::CatalogSearch
                                | crate::views::content_hub::ContentHubInputTarget::CreationsSearch
                        )
                    {
                        if let ActiveWorkspacePanel::ContentHub(sec) = self.active_panel {
                            self.refresh_content_hub_catalog(sec, cx);
                        }
                    }
                    cx.notify();
                }
                handled
            }
            ShellInputTarget::BrowserInput => {
                if key == "backspace" && self.ime.ime_preedit.take().is_some() {
                    self.bot_browser.ime_preedit_input_gen = None;
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    cx.notify();
                    return true;
                }
                if key == "tab" {
                    self.clear_all_ime_preedits(cx);
                }
                if let Some(bw) = self.browser_workspace.clone() {
                    if bw.update(cx, |w, wcx| w.handle_non_char_key_input(key, is_alt, wcx)) {
                        cx.notify();
                        return true;
                    }
                }
                key == "tab"
            }
            ShellInputTarget::CommunityInput => {
                if key == "backspace"
                    && (self.ime.has_preedit() || self.has_active_ime_preedit(cx))
                {
                    self.clear_all_ime_preedits(cx);
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    self.clear_all_ime_preedits(cx);
                    cx.notify();
                    return true;
                }
                if key == "tab" {
                    self.clear_all_ime_preedits(cx);
                }
                if let Some(cw) = self.community_workspace.clone() {
                    if cw.update(cx, |w, wcx| {
                        w.handle_non_char_key_input(key, is_shift, is_alt, wcx)
                    }) {
                        cx.notify();
                        return true;
                    }
                }
                key == "tab"
            }
            ShellInputTarget::OrcaBotsInput => {
                if key == "backspace" && self.ime.ime_preedit.take().is_some() {
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    cx.notify();
                    return true;
                }
                if let Some(ob) = self.orca_bots_workspace.clone() {
                    if key == "space" {
                        let handled = ob.update(cx, |w, wcx| w.insert_committed_text(" ", wcx));
                        cx.notify();
                        return handled;
                    }
                    let ev = KeyDownEvent {
                        keystroke: Keystroke {
                            modifiers: Modifiers {
                                shift: is_shift,
                                alt: is_alt,
                                ..Default::default()
                            },
                            key: key.to_string(),
                            key_char: None,
                        },
                        is_held: false,
                    };
                    let handled = ob.update(cx, |w, wcx| w.handle_key_down(&ev, wcx));
                    cx.notify();
                    return handled;
                }
                false
            }
            ShellInputTarget::OrcaIdeInput => {
                if key == "backspace" && self.ime.ime_preedit.take().is_some() {
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    cx.notify();
                    return true;
                }
                if let Some(oi) = self.orca_ide_workspace.clone() {
                    if key == "space" {
                        let handled = oi.update(cx, |w, wcx| w.insert_committed_text(" ", wcx));
                        cx.notify();
                        return handled;
                    }
                    let ev = KeyDownEvent {
                        keystroke: Keystroke {
                            modifiers: Modifiers {
                                shift: is_shift,
                                alt: is_alt,
                                ..Default::default()
                            },
                            key: key.to_string(),
                            key_char: None,
                        },
                        is_held: false,
                    };
                    let handled = oi.update(cx, |w, wcx| w.handle_key_down(&ev, wcx));
                    cx.notify();
                    return handled;
                }
                false
            }
            ShellInputTarget::OpenBotInput => {
                if key == "backspace" && self.ime.ime_preedit.take().is_some() {
                    cx.notify();
                    return true;
                }
                if key == "enter"
                    && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                {
                    cx.notify();
                    return true;
                }
                if let Some(ws) = self.openbot_workspace.clone() {
                    let handled = match key {
                        "backspace" => {
                            ws.update(cx, |w, wcx| {
                                w.handle_backspace();
                                wcx.notify();
                            });
                            true
                        }
                        "enter" => {
                            ws.update(cx, |w, wcx| {
                                if is_shift {
                                    w.handle_text_input("\n");
                                } else {
                                    w.handle_enter(false);
                                }
                                wcx.notify();
                            });
                            true
                        }
                        "tab" => {
                            self.ime.clear_preedit();
                            ws.update(cx, |w, wcx| {
                                w.handle_tab();
                                wcx.notify();
                            });
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        cx.notify();
                        return true;
                    }
                }
                false
            }
            ShellInputTarget::SidebarSearch => match key {
                "tab" => {
                    self.clear_all_ime_preedits(cx);
                    self.sidebar.update(cx, |sb, scx| {
                        sb.exit_search();
                        scx.notify();
                    });
                    self.set_composer_focused(true, cx);
                    cx.notify();
                    true
                }
                "backspace" => {
                    if self.ime.ime_preedit.take().is_none() {
                        self.sidebar.update(cx, |sb, scx| {
                            let mut q = sb.search_query.clone();
                            backspace_string(&mut q, is_alt);
                            sb.set_search_query(q);
                            scx.notify();
                        });
                    }
                    cx.notify();
                    true
                }
                "enter" => {
                    if self.take_ime_preedit_commit(cx) || self.ime.take_just_committed() {
                        return true;
                    }
                    self.sidebar.update(cx, |sb, scx| {
                        sb.search_focused = false;
                        scx.notify();
                    });
                    self.set_composer_focused(true, cx);
                    cx.notify();
                    true
                }
                _ => false,
            },
            ShellInputTarget::StageSearch => match key {
                "backspace" => {
                    if self.ime.ime_preedit.take().is_none() {
                        self.stage.update(cx, |st, stcx| {
                            backspace_string(&mut st.search_query, is_alt);
                            st.recompute_search_matches();
                            stcx.notify();
                        });
                    }
                    cx.notify();
                    true
                }
                "enter" | "down" => {
                    if !(self.take_ime_preedit_commit(cx)
                        || (key == "enter" && self.ime.take_just_committed()))
                    {
                        self.stage.update(cx, |st, stcx| {
                            st.step_search_match(if is_shift { -1 } else { 1 });
                            stcx.notify();
                        });
                    }
                    cx.notify();
                    true
                }
                "up" => {
                    self.stage.update(cx, |st, stcx| {
                        st.step_search_match(-1);
                        stcx.notify();
                    });
                    cx.notify();
                    true
                }
                "tab" => {
                    self.clear_all_ime_preedits(cx);
                    self.sidebar.update(cx, |sb, scx| {
                        sb.focus_search();
                        scx.notify();
                    });
                    self.set_composer_focused(false, cx);
                    cx.notify();
                    true
                }
                _ => false,
            },
            ShellInputTarget::Composer => {
                if key == "enter" && self.ime.take_just_committed() {
                    self.composer.update(cx, |c, ccx| {
                        c.ime_machine.reset_commit_fence();
                        ccx.notify();
                    });
                    cx.notify();
                    return true;
                }
                let composer_composing = self.composer.read(cx).ime_machine.is_composing();
                if !composer_composing
                    && matches!(key, "up" | "down" | "tab" | "enter")
                    && self.composer.update(cx, |c, ccx| {
                        let ok = c.handle_hermes_autocomplete_nav(key, is_shift);
                        if ok {
                            ccx.notify();
                        }
                        ok
                    })
                {
                    cx.notify();
                    return true;
                }
                match key {
                    "tab" => {
                        self.clear_all_ime_preedits(cx);
                        if self.composer.read(cx).ime_machine.is_composing() {
                            self.composer.update(cx, |c, ccx| {
                                c.ime_machine.on_ime_cancel();
                                c.sync_autocomplete();
                                ccx.notify();
                            });
                        }
                        self.sidebar.update(cx, |sb, scx| {
                            sb.focus_search();
                            scx.notify();
                        });
                        self.set_composer_focused(false, cx);
                        cx.notify();
                        true
                    }
                    "enter" => {
                        let action = self.composer.update(cx, |c, ccx| {
                            c.ime_machine.reset_commit_fence();
                            let a = c.handle_enter(is_shift);
                            c.sync_autocomplete();
                            ccx.notify();
                            a
                        });
                        if action == KeyAction::SendMessage {
                            self.composer.update(cx, |c, ccx| {
                                c.submit_primary();
                                ccx.notify();
                            });
                            self.handle_composer_pending_actions(cx);
                        }
                        cx.notify();
                        true
                    }
                    "backspace" => {
                        self.composer.update(cx, |c, ccx| {
                            if c.ime_machine.is_composing() {
                                c.ime_machine.on_ime_cancel();
                            } else {
                                c.ime_machine.backspace();
                            }
                            c.sync_autocomplete();
                            ccx.notify();
                        });
                        cx.notify();
                        true
                    }
                    _ => false,
                }
            }
            ShellInputTarget::Absorbed => {
                if matches!(self.active_panel, ActiveWorkspacePanel::ContentHub(_))
                    && !self.has_blocking_modal_or_fullscreen_overlay_for_shortcuts(cx)
                    && !self.sidebar.read(cx).is_account_popover_open
                {
                    if self.content_hub.read(cx).active_game.is_some() {
                        let game_input = match key {
                            "up" | "arrowup" => Some(crate::views::content_hub::GameInput::MoveUp),
                            "down" | "arrowdown" => {
                                Some(crate::views::content_hub::GameInput::MoveDown)
                            }
                            "left" | "arrowleft" => {
                                Some(crate::views::content_hub::GameInput::MoveLeft)
                            }
                            "right" | "arrowright" => {
                                Some(crate::views::content_hub::GameInput::MoveRight)
                            }
                            "enter" => Some(crate::views::content_hub::GameInput::Confirm),
                            "space" => Some(crate::views::content_hub::GameInput::PrimaryAction),
                            _ => None,
                        };
                        if let Some(inp) = game_input {
                            self.content_hub.update(cx, |h, hcx| {
                                if let Some(ref mut session) = h.active_game {
                                    session.apply_input(inp);
                                    hcx.notify();
                                }
                            });
                            cx.notify();
                            return true;
                        }
                    } else if self.content_hub.read(cx).book_reader.is_some() {
                        let synced = self.content_hub.update(cx, |h, hcx| {
                            let Some(ref mut reader) = h.book_reader else {
                                return None;
                            };
                            let turned = match key {
                                "left" | "arrowleft" => reader.prev_page(),
                                "right" | "arrowright" => reader.next_page(),
                                _ => return None,
                            };
                            if turned {
                                hcx.notify();
                                Some((reader.creation_id.clone(), reader.to_upsert_state_request()))
                            } else {
                                None
                            }
                        });
                        if let Some((cid, req)) = synced {
                            self.upsert_creation_state_fenced(cid, req, cx);
                            cx.notify();
                            return true;
                        }
                    }
                }
                false
            }
        }
    }

    /// Commits any pending IME preedit for the active input (equivalent to `EntityInputHandler::unmark_text`).
    pub fn commit_ime_unmark_text(&mut self, cx: &mut Context<Self>) {
        let target = self.resolve_input_target(cx);
        if target == ShellInputTarget::AuthWizard {
            let pending = self.auth_wizard.update(cx, |w, wcx| {
                let p = w.ime.ime_preedit.take().filter(|s| !s.is_empty());
                if p.is_some() {
                    wcx.notify();
                }
                p
            });
            if let Some(p) = pending {
                self.insert_committed_text(&p, cx);
                self.auth_wizard.update(cx, |w, _| w.ime.mark_ime_committed());
                return;
            }
            cx.notify();
            return;
        }
        if target == ShellInputTarget::Absorbed || !self.target_accepts_ime_text(&target, cx) {
            self.clear_all_ime_preedits(cx);
            cx.notify();
            return;
        }
        if let Some(p) = self.ime.ime_preedit.take().filter(|p| !p.is_empty()) {
            self.insert_committed_text(&p, cx);
            self.ime.mark_ime_committed();
            return;
        }
        self.clear_all_ime_preedits(cx);
        if target == ShellInputTarget::Composer {
            if let Some(p) = self
                .composer
                .read(cx)
                .ime_machine
                .preedit_text()
                .filter(|p| !p.is_empty())
                .map(str::to_string)
            {
                self.composer.update(cx, |c, ccx| {
                    c.ime_machine.on_ime_commit(&p);
                    c.sync_autocomplete();
                    ccx.notify();
                });
                self.ime.mark_ime_committed();
            }
        }
        cx.notify();
    }

    /// Updates or clears the active IME preedit (equivalent to `EntityInputHandler::replace_and_mark_text_in_range`).
    pub fn update_ime_marked_text(&mut self, new_text: &str, cx: &mut Context<Self>) {
        let target = self.resolve_input_target(cx);
        if target == ShellInputTarget::AuthWizard {
            self.auth_wizard.update(cx, |w, wcx| {
                if new_text.is_empty() {
                    w.ime.clear_ime_state();
                } else {
                    w.ime.ime_preedit = Some(new_text.to_string());
                    w.ime.clear_commit_fence();
                }
                wcx.notify();
            });
            cx.notify();
            return;
        }
        if new_text.is_empty() {
            self.clear_all_ime_preedits(cx);
            if target == ShellInputTarget::Composer {
                self.composer.update(cx, |c, ccx| {
                    c.ime_machine.on_ime_cancel();
                    c.sync_autocomplete();
                    ccx.notify();
                });
            }
            cx.notify();
            return;
        }
        if target != ShellInputTarget::PublishReviewModal
            && self.content_hub.read(cx).publish_modal.pending_close_ime_clear
        {
            self.content_hub.update(cx, |h, _| {
                h.publish_modal.pending_close_ime_clear = false;
            });
        }
        if target != ShellInputTarget::ModalContainer {
            self.last_ime_modal_snapshot = None;
        }
        if target != ShellInputTarget::BrowserInput {
            self.bot_browser.ime_preedit_input_gen = None;
        }
        if target != ShellInputTarget::CommunityInput {
            self.community_ime_preedit_input_gen = None;
            if let Some(cw) = self.community_workspace.clone() {
                if cw.read(cx).active_input_ime_preedit().is_some() {
                    cw.update(cx, |w, wcx| w.set_ime_preedit(None, wcx));
                }
            }
        }
        match target {
            ShellInputTarget::Absorbed | ShellInputTarget::OrcaSecurityModal => {
                self.clear_all_ime_preedits(cx);
            }
            ShellInputTarget::Composer => {
                self.ime.clear_commit_fence();
                self.composer.update(cx, |c, ccx| {
                    c.is_input_focused = true;
                    c.ime_machine.on_ime_update(new_text, new_text.len());
                    c.sync_autocomplete();
                    ccx.notify();
                });
            }
            ShellInputTarget::ComputerTerminal(ov) => {
                self.ime.ime_preedit = Some(new_text.to_string());
                self.ime.clear_commit_fence();
                ov.update(cx, |w, wcx| {
                    if w.is_interactive_pty_focused(wcx) {
                        let _ = w.replace_and_mark_terminal_input(new_text, wcx);
                    } else {
                        w.ensure_terminal_input(wcx);
                        if let Some(c) = w.terminal_input.as_ref() {
                            c.update(wcx, |ctrl, _| {
                                ctrl.ime_preedit = Some(new_text.to_string());
                                ctrl.clear_commit_fence();
                            });
                        }
                    }
                    wcx.notify();
                });
            }
            ShellInputTarget::PublishReviewModal => {
                if self.is_publish_review_modal_input_active(cx) {
                    self.ime.ime_preedit = Some(new_text.to_string());
                    self.ime.clear_commit_fence();
                    self.content_hub.update(cx, |h, hcx| {
                        h.publish_modal.set_ime_preedit(Some(new_text));
                        hcx.notify();
                    });
                } else {
                    self.clear_all_ime_preedits(cx);
                }
            }
            ShellInputTarget::ModalContainer => {
                let modal_snap = {
                    let mc = self.modal_container.read(cx);
                    mc.dialog
                        .has_text_input()
                        .then(|| (mc.dialog.active_field, mc.dialog.kind.clone()))
                };
                if let Some(snap) = modal_snap {
                    self.ime.ime_preedit = Some(new_text.to_string());
                    self.ime.clear_commit_fence();
                    self.last_ime_modal_snapshot = Some(snap);
                } else {
                    self.clear_all_ime_preedits(cx);
                }
            }
            ShellInputTarget::SettingsInput => {
                self.ime.ime_preedit = Some(new_text.to_string());
                self.ime.clear_commit_fence();
                self.settings_router.update(cx, |sr, scx| {
                    let _ = sr.set_active_input_ime_preedit(Some(new_text), scx);
                });
            }
            ShellInputTarget::InspectorBot => {
                self.ime.ime_preedit = Some(new_text.to_string());
                self.ime.clear_commit_fence();
                self.inspector
                    .update(cx, |ins, icx| ins.set_bot_ime_preedit(Some(new_text), icx));
            }
            ShellInputTarget::BotAssistCard => {
                self.ime.ime_preedit = Some(new_text.to_string());
                self.ime.clear_commit_fence();
                self.stage.update(cx, |st, stcx| {
                    st.bot_assist_ime_preedit = Some(new_text.to_string());
                    stcx.notify();
                });
            }
            ShellInputTarget::ContentHubInput => {
                self.ime.ime_preedit = Some(new_text.to_string());
                self.ime.clear_commit_fence();
                self.content_hub.update(cx, |h, hcx| {
                    h.set_active_input_ime_preedit(Some(new_text));
                    hcx.notify();
                });
            }
            ShellInputTarget::BrowserInput => {
                self.ime.ime_preedit = Some(new_text.to_string());
                self.ime.clear_commit_fence();
                self.bot_browser.ime_preedit_input_gen = self
                    .browser_workspace
                    .as_ref()
                    .map(|bw| bw.read(cx).active_input_generation());
            }
            ShellInputTarget::CommunityInput => {
                self.ime.ime_preedit = Some(new_text.to_string());
                self.ime.clear_commit_fence();
                self.community_ime_preedit_input_gen = self
                    .community_workspace
                    .as_ref()
                    .map(|cw| cw.read(cx).active_input_generation());
                if let Some(cw) = self.community_workspace.clone() {
                    cw.update(cx, |w, wcx| w.set_ime_preedit(Some(new_text), wcx));
                }
            }
            ShellInputTarget::PasscodeLock
            | ShellInputTarget::OrcaJumpPalette
            | ShellInputTarget::OrcaBotsInput
            | ShellInputTarget::OrcaIdeInput
            | ShellInputTarget::OpenBotInput
            | ShellInputTarget::StudioInput
            | ShellInputTarget::CreationModal
            | ShellInputTarget::ContactsModal
            | ShellInputTarget::CreateBotBox(_)
            | ShellInputTarget::MiniAppInput(_)
            | ShellInputTarget::AppleCompose
            | ShellInputTarget::SidebarSearch
            | ShellInputTarget::StageSearch => {
                self.ime.ime_preedit = Some(new_text.to_string());
                self.ime.clear_commit_fence();
            }
            ShellInputTarget::AuthWizard => {}
        }
        cx.notify();
    }

    /// Commits IME replacement text (equivalent to `EntityInputHandler::replace_text_in_range`),
    /// arming the 80ms post-commit Enter fence strictly when the active target was composing.
    pub fn commit_replace_text_in_range(&mut self, text: &str, cx: &mut Context<Self>) {
        let was_composing = self.has_active_ime_preedit(cx)
            || (!self.is_non_composer_text_capturing(cx)
                && self.composer.read(cx).ime_machine.is_composing());
        self.insert_committed_text(text, cx);
        if was_composing {
            if self.is_authenticated {
                self.ime.mark_ime_committed();
            } else {
                self.auth_wizard.update(cx, |w, _| w.ime.mark_ime_committed());
            }
        }
    }
}

impl EntityInputHandler for AppShell {
    fn text_for_range(&mut self, range_utf16: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) -> Option<String> {
        let full = self.combined_active_text(cx);
        let start = utf16_offset_to_utf8(&full, range_utf16.start);
        let end = utf16_offset_to_utf8(&full, range_utf16.end).max(start);
        Some(full.get(start..end).unwrap_or("").to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, cx: &mut Context<Self>) -> Option<UTF16Selection> {
        let total = self.active_utf16_len(cx);
        Some(UTF16Selection { range: total..total, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, cx: &mut Context<Self>) -> Option<Range<usize>> {
        let (committed, preedit) = self.active_text_and_preedit(cx);
        preedit.filter(|p| !p.is_empty()).map(|p| {
            let start = utf16_len(&committed);
            start..start + utf16_len(&p)
        })
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.commit_ime_unmark_text(cx);
    }

    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.commit_replace_text_in_range(text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_ime_marked_text(new_text, cx);
    }

    fn bounds_for_range(&mut self, _: Range<usize>, element_bounds: Bounds<Pixels>, _: &mut Window, cx: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let target = self.resolve_input_target(cx);
        let full_text = self.combined_active_text(cx);
        let char_count = full_text.chars().count();
        match target {
            ShellInputTarget::AuthWizard
            | ShellInputTarget::PasscodeLock
            | ShellInputTarget::OrcaSecurityModal
            | ShellInputTarget::OrcaJumpPalette
            | ShellInputTarget::ComputerTerminal(_)
            | ShellInputTarget::PublishReviewModal
            | ShellInputTarget::ModalContainer
            | ShellInputTarget::StudioInput
            | ShellInputTarget::SettingsInput
            | ShellInputTarget::CreationModal
            | ShellInputTarget::ContactsModal
            | ShellInputTarget::CreateBotBox(_)
            | ShellInputTarget::MiniAppInput(_) => {
                let cx_mid = element_bounds.origin.x + element_bounds.size.width / 2.0;
                let cy_mid = element_bounds.origin.y + element_bounds.size.height / 2.0;
                let caret_x = cx_mid - px(140.0) + px((char_count as f32 * 8.0).min(280.0));
                let caret_y = if matches!(target, ShellInputTarget::AuthWizard) {
                    cy_mid + px(10.0)
                } else {
                    cy_mid - px(24.0)
                };
                Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(20.0))))
            }
            ShellInputTarget::AppleCompose => {
                let (sb_width, chip_count, title_focused) = {
                    let sb = self.sidebar.read(cx);
                    (
                        sb.effective_width(),
                        sb.compose_draft.selected_recipients.len(),
                        sb.compose_title_focused,
                    )
                };
                let chip_offset = if title_focused {
                    0.0
                } else {
                    (chip_count as f32 * 82.0).min(320.0)
                };
                let apple_w = self.apple_sidebar.read(cx).effective_width();
                let caret_x = element_bounds.origin.x
                    + px(apple_w + sb_width + 82.0 + chip_offset + (char_count as f32 * 8.0).min(260.0));
                let caret_y =
                    element_bounds.origin.y + px(if title_focused { 66.0 } else { 22.0 });
                Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(18.0))))
            }
            ShellInputTarget::InspectorBot => {
                let caret_x = (element_bounds.origin.x + element_bounds.size.width - px(290.0)
                    + px((char_count as f32 * 7.0).min(220.0)))
                .max(element_bounds.origin.x + px(40.0));
                let caret_y = element_bounds.origin.y + px(380.0);
                Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(18.0))))
            }
            ShellInputTarget::BotAssistCard => {
                let apple_w = self.apple_sidebar.read(cx).effective_width();
                let sb_w = self.sidebar.read(cx).effective_width();
                let caret_x = (element_bounds.origin.x + px(apple_w + sb_w + 60.0)
                    + px((char_count as f32 * 7.5).min(240.0)))
                .min(element_bounds.origin.x + element_bounds.size.width - px(80.0));
                let caret_y = (element_bounds.origin.y + element_bounds.size.height - px(220.0))
                    .max(element_bounds.origin.y + px(100.0));
                Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(18.0))))
            }
            ShellInputTarget::ContentHubInput => {
                let sidebar_w = self.apple_sidebar.read(cx).effective_width();
                let caret_x = element_bounds.origin.x
                    + px(sidebar_w + 36.0 + (char_count as f32 * 7.5).min(220.0));
                let caret_y = element_bounds.origin.y + px(72.0);
                Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(18.0))))
            }
            ShellInputTarget::BrowserInput => {
                let apple_w = self.apple_sidebar.read(cx).effective_width();
                let caret_x = element_bounds.origin.x
                    + px(apple_w + 140.0)
                    + px((char_count as f32 * 7.5).min(360.0));
                Some(Bounds::new(
                    point(caret_x, element_bounds.origin.y + px(28.0)),
                    size(px(2.0), px(18.0)),
                ))
            }
            ShellInputTarget::CommunityInput
            | ShellInputTarget::OrcaBotsInput
            | ShellInputTarget::OrcaIdeInput => {
                let apple_w = self.apple_sidebar.read(cx).effective_width();
                let caret_x = element_bounds.origin.x
                    + px(apple_w + 260.0 + (char_count as f32 * 8.0).min(420.0));
                Some(Bounds::new(
                    point(caret_x, element_bounds.origin.y + px(140.0)),
                    size(px(2.0), px(18.0)),
                ))
            }
            ShellInputTarget::OpenBotInput => {
                let apple_w = self.apple_sidebar.read(cx).effective_width();
                let caret_x = element_bounds.origin.x
                    + px(apple_w + 416.0)
                    + px((char_count as f32 * 7.5).min(360.0));
                let caret_y = element_bounds.origin.y + element_bounds.size.height - px(52.0);
                Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(18.0))))
            }
            ShellInputTarget::StageSearch => {
                let apple_w = self.apple_sidebar.read(cx).effective_width();
                let sb_w = self.sidebar.read(cx).effective_width();
                let caret_x = element_bounds.origin.x
                    + px(apple_w + sb_w + 48.0)
                    + px((char_count as f32 * 8.0).min(280.0));
                Some(Bounds::new(
                    point(caret_x, element_bounds.origin.y + px(84.0)),
                    size(px(2.0), px(18.0)),
                ))
            }
            ShellInputTarget::SidebarSearch => {
                let apple_w = self.apple_sidebar.read(cx).effective_width();
                let sb_chars = self.sidebar.read(cx).search_query.chars().count();
                let caret_x = element_bounds.origin.x
                    + px(apple_w + 48.0)
                    + px((sb_chars as f32 * 7.5).min(200.0));
                Some(Bounds::new(
                    point(caret_x, element_bounds.origin.y + px(22.0)),
                    size(px(2.0), px(18.0)),
                ))
            }
            ShellInputTarget::Composer | ShellInputTarget::Absorbed => {
                let mut advance = 0.0f32;
                for ch in full_text.chars() {
                    advance = if ch == '\n' {
                        0.0
                    } else if ch.is_ascii() {
                        advance + 8.2
                    } else {
                        advance + 14.0
                    };
                }
                let apple_w = self.apple_sidebar.read(cx).effective_width();
                let sb_w = self.sidebar.read(cx).effective_width();
                let base_x = apple_w + sb_w + 64.0;
                let max_w = (f32::from(element_bounds.size.width) - base_x - 140.0).max(100.0);
                let caret_x = element_bounds.origin.x + px(base_x + advance.clamp(0.0, max_w));
                let caret_y = element_bounds.origin.y + element_bounds.size.height - px(36.0);
                Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(20.0))))
            }
        }
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        Some(self.active_utf16_len(cx))
    }
}
