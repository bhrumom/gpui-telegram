//! AppShell input routing, non-character key dispatch, and macOS/GPUI IME `EntityInputHandler`.

use std::ops::Range;

use gpui::{
    point, px, size, App, Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection,
    Window,
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

impl AppShell {
    /// Returns `true` when the active workspace is Settings and the current settings section has a focused text input.
    pub fn is_settings_input_active(&self, cx: &App) -> bool {
        self.active_panel == ActiveWorkspacePanel::Settings
            && self.settings_router.read(cx).is_input_active(cx)
    }

    /// Returns `true` when `TasksWorkspace` (TaskCenter / McpTools) holds an active studio text target.
    pub fn is_studio_input_active(&self, cx: &App) -> bool {
        (self.active_panel == ActiveWorkspacePanel::TaskCenter
            || self.active_panel == ActiveWorkspacePanel::McpTools)
            && self.tasks_workspace.read(cx).active_input != StudioInputTarget::None
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
            || self.overlays.is_passcode_locked
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
        self.active_panel == ActiveWorkspacePanel::Browser
            && !self.has_browser_visual_occlusion_overlay(cx)
            && self.browser_workspace.as_ref().is_some_and(|bw| {
                let bw_read = bw.read(cx);
                !bw_read.is_externally_occluded() && bw_read.has_focused_text_input()
            })
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

    /// Returns `true` when a shell modal, lockscreen, popover, or sidebar search blocks Code Scene keys.
    pub fn has_code_scene_shell_blocker(&self, cx: &App) -> bool {
        let sb = self.sidebar.read(cx);
        self.overlays.is_passcode_locked
            || self.has_browser_visual_occlusion_overlay(cx)
            || sb.search_focused
            || sb.is_contacts_modal_open
            || sb.active_creation_modal.is_some()
            || sb.is_account_popover_open
            || sb.is_plus_menu_open
    }

    /// Returns `true` when `CodeSceneWorkspace` is active and has a focused text input field
    /// (Search, Comment, Code Editor, or AI Bot Studio).
    pub fn is_code_scene_input_active(&self, cx: &App) -> bool {
        self.active_panel == ActiveWorkspacePanel::CodeScene
            && !self.has_code_scene_shell_blocker(cx)
            && self
                .code_scene_workspace
                .as_ref()
                .is_some_and(|cs| cs.read(cx).has_focused_text_input())
    }

    /// Returns `true` when the `ComputerWorkbench` overlay's Remote Terminal or interactive PTY is focused.
    pub fn is_computer_workbench_input_active(&self, cx: &App) -> bool {
        self.active_computer_terminal_overlay(cx).is_some()
    }

    fn active_computer_terminal_overlay(
        &self,
        cx: &App,
    ) -> Option<gpui::Entity<crate::views::ComputerWorkbench>> {
        self.overlays
            .computer_overlay
            .as_ref()
            .filter(|ov| {
                let wb = ov.read(cx);
                wb.is_terminal_input_active() || wb.is_interactive_pty_focused(cx)
            })
            .cloned()
    }

    /// Returns `true` when `ContextInspector` is showing `BotConfigEditor` with an active input field.
    pub fn is_inspector_bot_input_active(&self, cx: &App) -> bool {
        self.active_panel == ActiveWorkspacePanel::Chats
            && self.inspector.read(cx).is_bot_input_active()
    }

    /// Returns `true` when an in-transcript Bot Assist Vault Login card input field is focused.
    pub fn is_bot_card_input_active(&self, cx: &App) -> bool {
        self.active_panel == ActiveWorkspacePanel::Chats
            && self.stage.read(cx).active_bot_card_field.is_some()
    }

    /// Returns `true` when Apple Messages inline Compose mode (`收件人:`) should receive keyboard & IME input.
    pub fn is_apple_compose_input_active(&self, cx: &App) -> bool {
        let sb = self.sidebar.read(cx);
        self.active_panel == ActiveWorkspacePanel::Chats
            && self.theme_state.palette.is_apple_style()
            && sb.is_plus_menu_open
            && !sb.search_focused
            && !sb.is_contacts_modal_open
            && sb.active_creation_modal.is_none()
            && !self.composer.read(cx).is_input_focused
    }

    fn set_composer_focused(&mut self, focused: bool, cx: &mut Context<Self>) {
        self.composer.update(cx, |c, c_cx| {
            c.is_input_focused = focused;
            c_cx.notify();
        });
    }

    /// Returns `true` when keyboard/IME input should target a non-composer field or be suppressed.
    pub fn is_non_composer_text_capturing(&self, cx: &App) -> bool {
        let sb = self.sidebar.read(cx);
        self.overlays.is_passcode_locked
            || self.is_computer_workbench_input_active(cx)
            || sb.search_focused
            || sb.active_creation_modal.is_some()
            || sb.is_contacts_modal_open
            || self.is_apple_compose_input_active(cx)
            || self.modal_container.read(cx).is_modal_active()
            || self.miniapps.create_bot_box.as_ref().is_some_and(|b| b.read(cx).is_visible)
            || self.miniapps.miniapp_overlay.as_ref().is_some_and(|w| w.read(cx).has_focused_text_input())
            || self.is_studio_input_active(cx)
            || self.is_browser_workspace_input_active(cx)
            || self.is_code_scene_input_active(cx)
            || self.is_settings_input_active(cx)
            || self.is_inspector_bot_input_active(cx)
            || self.is_bot_card_input_active(cx)
            || self.stage.read(cx).search_open
            || self.should_hide_composer(cx)
    }

    fn take_ime_preedit_commit(&mut self, cx: &mut Context<Self>) -> bool {
        self.bot_browser.ime_preedit_input_gen = None;
        if let Some(p) = self.ime.ime_preedit.take() {
            self.insert_committed_text(&p, cx);
            true
        } else {
            false
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
        if !self.is_authenticated {
            return self.auth_wizard.read(cx).active_text_and_preedit();
        }
        let pre = || self.ime.ime_preedit.clone();
        if self.overlays.is_passcode_locked {
            return (self.overlays.passcode_unlock_input.clone(), pre());
        }
        if let Some(ov) = self.active_computer_terminal_overlay(cx) {
            let (committed, term_pre) = ov.read(cx).terminal_text_and_preedit(cx);
            return (committed, term_pre.or_else(pre));
        }
        if let Some(b) = self.miniapps.create_bot_box.as_ref().filter(|b| b.read(cx).is_visible) {
            return (b.read(cx).active_field_text().to_string(), pre());
        }
        if let Some(ov) = self.miniapps.miniapp_overlay.as_ref().filter(|w| w.read(cx).has_focused_text_input()) {
            return (ov.read(cx).active_input_text(), pre());
        }
        if self.modal_container.read(cx).is_modal_active() {
            return (self.modal_container.read(cx).dialog.active_field_text(), pre());
        }
        let sb = self.sidebar.read(cx);
        if sb.is_contacts_modal_open {
            return (sb.contacts_search_query.clone(), pre());
        }
        if sb.active_creation_modal.is_some() {
            let txt = match sb.active_creation_field {
                CreationInputField::PrimaryName => &sb.creation_primary_input,
                CreationInputField::SecondaryHandle => &sb.creation_secondary_input,
                CreationInputField::TertiaryPromptOrPhone => &sb.creation_tertiary_input,
            };
            return (txt.clone(), pre());
        }
        if self.is_settings_input_active(cx) {
            return self.settings_router.read(cx)
                .active_input_text_and_preedit(&self.ime, cx)
                .unwrap_or_else(|| (String::new(), pre()));
        }
        if self.is_inspector_bot_input_active(cx) {
            let (committed, bot_pre) = self.inspector.read(cx).active_bot_input_text_and_preedit(cx);
            return (committed, bot_pre.or_else(pre));
        }
        if self.is_bot_card_input_active(cx) {
            let st = self.stage.read(cx);
            return (
                st.active_bot_card_text().to_string(),
                st.bot_assist_ime_preedit.clone().or_else(pre),
            );
        }
        if self.is_studio_input_active(cx) { return (String::new(), pre()); }
        if self.is_browser_workspace_input_active(cx) {
            let txt = self
                .browser_workspace
                .as_ref()
                .map(|bw| bw.read(cx).active_input_text())
                .unwrap_or_default();
            return (txt, pre());
        }
        if self.is_code_scene_input_active(cx) {
            let (txt, cs_pre) = self
                .code_scene_workspace
                .as_ref()
                .map(|cs| {
                    let r = cs.read(cx);
                    (r.active_input_text(), r.ime_preedit.clone())
                })
                .unwrap_or_default();
            return (txt, cs_pre.or_else(pre));
        }
        if self.is_apple_compose_input_active(cx) {
            if sb.compose_title_focused {
                return (sb.compose_draft.custom_group_title.clone(), pre());
            }
            return (sb.compose_recipient_query.clone(), pre());
        }
        if sb.search_focused { return (sb.search_query.clone(), pre()); }
        if self.stage.read(cx).search_open { return (self.stage.read(cx).search_query.clone(), pre()); }
        let c = self.composer.read(cx);
        (c.text().to_string(), c.ime_machine.preedit_text().map(str::to_string))
    }

    /// Returns `true` when the currently active field has an uncommitted IME composition string.
    pub fn has_active_ime_preedit(&self, cx: &App) -> bool {
        self.active_text_and_preedit(cx).1.is_some_and(|p| !p.is_empty())
    }

    /// Clears all text in the currently focused input field.
    pub fn clear_active_field(&mut self, cx: &mut Context<Self>) {
        self.ime.clear_preedit();
        self.bot_browser.ime_preedit_input_gen = None;
        if !self.is_authenticated {
            self.auth_wizard.update(cx, |w, wcx| w.clear_active_field(wcx));
            cx.notify();
            return;
        }
        if self.overlays.is_passcode_locked {
            self.overlays.passcode_unlock_input.clear();
            self.overlays.passcode_unlock_error = None;
            cx.notify();
            return;
        }
        if let Some(ov) = self.active_computer_terminal_overlay(cx) {
            ov.update(cx, |w, wcx| w.clear_terminal_input(wcx));
            cx.notify();
            return;
        }
        if let Some(box_ent) = self.miniapps.create_bot_box.as_ref().filter(|b| b.read(cx).is_visible).cloned() {
            box_ent.update(cx, |b, bcx| {
                b.clear_active_field();
                bcx.notify();
            });
        } else if self.modal_container.read(cx).is_modal_active() {
            self.modal_container.update(cx, |mc, mcx| { mc.dialog.clear_active_field(); mcx.notify(); });
        } else if self.sidebar.read(cx).is_contacts_modal_open {
            self.sidebar.update(cx, |sb, scx| { sb.contacts_search_query.clear(); scx.notify(); });
        } else if self.sidebar.read(cx).active_creation_modal.is_some() {
            self.sidebar.update(cx, |sb, scx| { sb.clear_creation_text(); scx.notify(); });
        } else if self.is_apple_compose_input_active(cx) {
            self.sidebar.update(cx, |sb, scx| {
                if sb.compose_title_focused {
                    sb.compose_draft.custom_group_title.clear();
                } else {
                    sb.compose_recipient_query.clear();
                    sb.compose_draft.highlighted_index = 0;
                }
                scx.notify();
            });
        } else if self.is_settings_input_active(cx) {
            self.settings_router.update(cx, |sr, scx| { sr.clear_active_input(scx); });
        } else if self.is_browser_workspace_input_active(cx) {
            if let Some(bw) = self.browser_workspace.clone() {
                bw.update(cx, |w, wcx| w.clear_active_input(wcx));
            }
        } else if self.is_code_scene_input_active(cx) {
            if let Some(cs) = self.code_scene_workspace.clone() {
                cs.update(cx, |w, wcx| {
                    let _ = w.clear_active_input(wcx);
                });
            }
        } else if self.is_inspector_bot_input_active(cx) {
            self.inspector.update(cx, |ins, icx| { let _ = ins.clear_active_bot_input(icx); });
        } else if self.is_bot_card_input_active(cx) {
            self.stage.update(cx, |st, stcx| {
                st.bot_assist_ime_preedit = None;
                st.clear_active_bot_card_field();
                stcx.notify();
            });
        } else if self.sidebar.read(cx).search_focused {
            self.sidebar.update(cx, |sb, scx| { sb.set_search_query(""); scx.notify(); });
        } else if self.stage.read(cx).search_open {
            self.stage.update(cx, |st, stcx| {
                st.search_query.clear();
                st.recompute_search_matches();
                stcx.notify();
            });
        } else if !self.should_hide_composer(cx) {
            self.composer.update(cx, |c, ccx| { c.clear(); ccx.notify(); });
        }
        cx.notify();
    }

    /// Inserts sanitized committed/pasted text into the currently focused shell input target.
    pub fn insert_committed_text(&mut self, raw_text: &str, cx: &mut Context<Self>) {
        self.ime.clear_preedit();
        self.bot_browser.ime_preedit_input_gen = None;
        if raw_text.is_empty() {
            cx.notify();
            return;
        }
        let clean = sanitize_paste_text(raw_text, false);
        if clean.is_empty()
            && !self.is_inspector_bot_input_active(cx)
            && !self.is_browser_workspace_input_active(cx)
            && !self.is_code_scene_input_active(cx)
        {
            cx.notify();
            return;
        }
        if !self.is_authenticated {
            self.auth_wizard.update(cx, |w, wcx| w.insert_committed_text(&clean, wcx));
            cx.notify();
            return;
        }
        if self.overlays.is_passcode_locked {
            self.overlays.passcode_unlock_input.push_str(&clean);
            self.overlays.passcode_unlock_error = None;
            cx.notify();
            return;
        }
        self.record_user_activity();
        if let Some(ov) = self.active_computer_terminal_overlay(cx) {
            ov.update(cx, |w, wcx| w.push_terminal_text(&clean, wcx));
        } else if let Some(box_ent) = self.miniapps.create_bot_box.as_ref().filter(|b| b.read(cx).is_visible).cloned() {
            box_ent.update(cx, |b, bcx| { b.push_text(&clean); bcx.notify(); });
        } else if let Some(ov) = self.miniapps.miniapp_overlay.as_ref().filter(|w| w.read(cx).has_focused_text_input()).cloned() {
            ov.update(cx, |w, wcx| { w.push_text_to_focused_input(&clean); wcx.notify(); });
        } else if self.modal_container.read(cx).is_modal_active() {
            self.modal_container.update(cx, |mc, mcx| { mc.handle_key_input("", Some(&clean)); mcx.notify(); });
        } else if self.sidebar.read(cx).is_contacts_modal_open {
            self.sidebar.update(cx, |sb, scx| { sb.contacts_search_query.push_str(&clean); scx.notify(); });
        } else if self.sidebar.read(cx).active_creation_modal.is_some() {
            self.sidebar.update(cx, |sb, scx| { sb.push_creation_text(&clean); scx.notify(); });
        } else if self.is_apple_compose_input_active(cx) {
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
        } else if self.is_settings_input_active(cx) {
            self.settings_router.update(cx, |sr, scx| { sr.push_input_text(&clean, scx); });
        } else if self.is_inspector_bot_input_active(cx) {
            let allow_nl = true;
            let txt = sanitize_paste_text(raw_text, allow_nl);
            if !txt.is_empty() {
                self.inspector.update(cx, |ins, icx| { let _ = ins.push_bot_input_text(&txt, icx); });
            }
        } else if self.is_bot_card_input_active(cx) {
            self.stage.update(cx, |st, stcx| {
                st.bot_assist_ime_preedit = None;
                st.insert_bot_card_text(&clean);
                stcx.notify();
            });
        } else if self.is_studio_input_active(cx) {
            self.tasks_workspace.update(cx, |w, wcx| { w.apply_text_key_to_active_input("", Some(&clean)); wcx.notify(); });
        } else if self.is_browser_workspace_input_active(cx) {
            if let Some(bw) = self.browser_workspace.clone() {
                bw.update(cx, |w, wcx| {
                    w.insert_text_to_active_input(raw_text, wcx);
                });
            }
        } else if self.is_code_scene_input_active(cx) {
            if let Some(cs) = self.code_scene_workspace.clone() {
                let allow_nl = matches!(
                    cs.read(cx).active_input,
                    crate::views::code_scene::ActiveCodeSceneInput::CodeEditor
                        | crate::views::code_scene::ActiveCodeSceneInput::StudioPrompt
                );
                let txt = sanitize_paste_text(raw_text, allow_nl);
                cs.update(cx, |w, wcx| {
                    w.set_ime_preedit(None, wcx);
                    if !txt.is_empty() {
                        let _ = w.insert_text_input(&txt, wcx);
                    }
                });
            }
        } else if self.sidebar.read(cx).search_focused {
            self.sidebar.update(cx, |sb, scx| {
                let mut q = sb.search_query.clone();
                q.push_str(&clean);
                sb.set_search_query(q);
                scx.notify();
            });
        } else if self.stage.read(cx).search_open {
            self.stage.update(cx, |st, stcx| {
                st.search_query.push_str(&clean);
                st.recompute_search_matches();
                stcx.notify();
            });
        } else if !self.should_hide_composer(cx) {
            self.composer.update(cx, |c, ccx| {
                c.is_input_focused = true;
                if c.ime_machine.is_composing() { c.ime_machine.on_ime_commit(&clean); } else { c.ime_machine.insert_text(&clean); }
                c.sync_autocomplete();
                ccx.notify();
            });
        }
        cx.notify();
    }

    fn dismiss_pre_auth_escape(&mut self, is_shift: bool, cx: &mut Context<Self>) -> bool {
        if self.ime.ime_preedit.take().is_some() {
            self.bot_browser.ime_preedit_input_gen = None;
            if self.is_bot_card_input_active(cx) {
                self.stage.update(cx, |st, stcx| {
                    st.bot_assist_ime_preedit = None;
                    stcx.notify();
                });
            }
            return true;
        }
        if self.composer.read(cx).ime_machine.is_composing() {
            self.composer.update(cx, |c, ccx| { c.ime_machine.on_ime_cancel(); c.sync_autocomplete(); ccx.notify(); });
            return true;
        }
        if self.is_studio_input_active(cx) {
            self.tasks_workspace.update(cx, |w, wcx| { w.active_input = StudioInputTarget::None; wcx.notify(); });
            return true;
        }
        if self.composer.update(cx, |c, ccx| {
            let ok = c.handle_hermes_autocomplete_nav("escape", is_shift);
            if ok { ccx.notify(); }
            ok
        }) {
            return true;
        }
        if self.miniapps.miniapp_tos_modal_open {
            self.miniapps.miniapp_tos_modal_open = false;
            self.ime.clear_preedit();
            self.bot_browser.ime_preedit_input_gen = None;
            return true;
        }
        if self.miniapps.bot_profile_box.as_ref().is_some_and(|b| b.read(cx).is_visible) {
            if let Some(ref ent) = self.miniapps.bot_profile_box {
                ent.update(cx, |b, b_cx| {
                    b.close();
                    b_cx.notify();
                });
            }
            self.miniapps.bot_profile_box = None;
            self.ime.clear_preedit();
            self.bot_browser.ime_preedit_input_gen = None;
            return true;
        }
        if self.miniapps.pending_miniapp_confirm.is_some() {
            self.miniapps.pending_miniapp_confirm = None;
            self.ime.clear_preedit();
            self.bot_browser.ime_preedit_input_gen = None;
            return true;
        }
        if self.miniapps.create_bot_box.as_ref().is_some_and(|b| b.read(cx).is_visible) {
            self.miniapps.create_bot_box = None;
            if !self.is_compose_or_creation_modal_active(cx) {
                self.exit_compose_or_modal_isolation(true, cx);
            }
            return true;
        }
        if let Some(ref ov) = self.miniapps.miniapp_overlay {
            if ov.read(cx).is_open {
                let mut minimized = false;
                ov.update(cx, |w, wcx| {
                    if w.confirm_close_open { w.confirm_close_open = false; }
                    else if w.menu_open { w.menu_open = false; }
                    else if w.focused_input.is_some() { w.focused_input = None; }
                    else { w.minimize(); minimized = true; }
                    wcx.notify();
                });
                if minimized { self.miniapps.is_miniapp_minimized = true; }
                return true;
            }
        }
        if self.overlays.iv_viewer_overlay.is_some() { self.overlays.iv_viewer_overlay = None; return true; }
        if self.overlays.media_viewer_overlay.is_some() { self.hide_media_viewer(); return true; }
        if self.rtc_talk.talk_mode_overlay.is_some() { self.hangup_active_call(cx); return true; }
        if self.overlays.computer_overlay.is_some() { self.hide_remote_computer(cx); return true; }
        if self.modal_container.read(cx).is_modal_active() {
            self.modal_container.update(cx, |mc, mcx| { mc.dismiss_top(); mcx.notify(); });
            if self.composer.read(cx).is_compose_isolated && !self.is_compose_or_creation_modal_active(cx) {
                self.exit_compose_or_modal_isolation(true, cx);
            }
            return true;
        }
        false
    }

    fn dismiss_post_auth_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.sidebar.read(cx).is_contacts_modal_open {
            self.sidebar.update(cx, |sb, scx| { sb.close_contacts_modal(); scx.notify(); });
            if self.composer.read(cx).is_compose_isolated
                && !self.is_compose_or_creation_modal_active(cx)
            {
                self.exit_compose_or_modal_isolation(true, cx);
            } else {
                self.sync_compose_state_to_composer(true, cx);
            }
            return true;
        }
        if self.sidebar.read(cx).active_creation_modal.is_some() {
            self.sidebar.update(cx, |sb, scx| { sb.close_creation_modal(); scx.notify(); });
            if self.composer.read(cx).is_compose_isolated
                && !self.is_compose_or_creation_modal_active(cx)
            {
                self.exit_compose_or_modal_isolation(true, cx);
            } else {
                self.sync_compose_state_to_composer(true, cx);
                self.set_composer_focused(true, cx);
            }
            return true;
        }
        if self.theme_state.palette.is_apple_style() && self.sidebar.read(cx).is_plus_menu_open {
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
                if self.composer.read(cx).is_compose_isolated
                    && !self.is_compose_or_creation_modal_active(cx)
                {
                    self.exit_compose_or_modal_isolation(true, cx);
                } else {
                    self.sync_compose_state_to_composer(true, cx);
                    self.set_composer_focused(true, cx);
                }
            }
            return true;
        }
        if self.sidebar.read(cx).is_account_popover_open || self.sidebar.read(cx).is_plus_menu_open {
            self.ime.clear_preedit();
            self.bot_browser.ime_preedit_input_gen = None;
            self.sidebar.update(cx, |s, scx| {
                s.is_account_popover_open = false;
                s.cancel_compose_mode();
                scx.notify();
            });
            if self.active_panel == ActiveWorkspacePanel::Browser {
                let occluded = self.has_browser_visual_occlusion_overlay(cx);
                if let Some(ref bw) = self.browser_workspace {
                    bw.update(cx, |w, wcx| {
                        w.set_occluded(occluded, wcx);
                    });
                }
            }
            if self.composer.read(cx).is_compose_isolated
                && !self.is_compose_or_creation_modal_active(cx)
            {
                self.exit_compose_or_modal_isolation(true, cx);
            } else {
                self.sync_compose_state_to_composer(true, cx);
            }
            return true;
        }
        if self.main_menu_drawer.read(cx).is_open {
            self.main_menu_drawer.update(cx, |m, mcx| { m.close(); mcx.notify(); });
            return true;
        }
        if self.active_panel == ActiveWorkspacePanel::Settings {
            if self.is_settings_input_active(cx)
                && self
                    .settings_router
                    .update(cx, |sr, scx| sr.handle_non_char_key("escape", false, scx))
                    == Some(true)
            {
                return true;
            }
            self.navigate_settings_back(cx);
            return true;
        }
        if self.active_panel == ActiveWorkspacePanel::Browser {
            if self.is_browser_workspace_input_active(cx) {
                if let Some(bw) = self.browser_workspace.clone() {
                    let handled = bw.update(cx, |w, wcx| w.handle_non_char_key_input("escape", false, wcx));
                    if handled {
                        return true;
                    }
                }
            }
            self.hide_browser_native_webview(cx);
            self.active_panel = ActiveWorkspacePanel::Chats;
            self.last_handled_nav_dest = Some(NavDestination::Chats);
            self.nav_rail.update(cx, |r, _| { r.set_active_destination(NavDestination::Chats); });
            return true;
        }
        if self.active_panel == ActiveWorkspacePanel::CodeScene {
            if let Some(cs) = self.code_scene_workspace.clone() {
                if cs.update(cx, |w, wcx| w.handle_non_char_key_input("escape", false, false, wcx)) {
                    return true;
                }
            }
            self.leave_code_scene_panel_if_active(cx);
            self.active_panel = ActiveWorkspacePanel::Chats;
            self.last_handled_nav_dest = Some(NavDestination::Chats);
            self.nav_rail.update(cx, |r, _| { r.set_active_destination(NavDestination::Chats); });
            return true;
        }
        if matches!(self.active_panel, ActiveWorkspacePanel::Calls | ActiveWorkspacePanel::TaskCenter | ActiveWorkspacePanel::McpTools) {
            self.active_panel = ActiveWorkspacePanel::Chats;
            self.last_handled_nav_dest = Some(NavDestination::Chats);
            self.nav_rail.update(cx, |r, _| { r.set_active_destination(NavDestination::Chats); });
            return true;
        }
        if self.stage.read(cx).active_context_menu.is_some() || self.stage.read(cx).reaction_picker_msg_id.is_some() {
            self.stage.update(cx, |st, stcx| {
                st.active_context_menu = None;
                st.context_menu_reactions_expanded = false;
                st.reaction_picker_msg_id = None;
                stcx.notify();
            });
            return true;
        }
        if self.stage.read(cx).more_menu_open {
            self.stage.update(cx, |st, stcx| { st.more_menu_open = false; st.ttl_submenu_open = false; stcx.notify(); });
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
            self.ime.clear_preedit();
            self.stage.update(cx, |st, stcx| {
                st.blur_bot_card_input();
                stcx.notify();
            });
            return true;
        }
        if self.inspector.read(cx).is_open { self.set_inspector_open(false, cx); return true; }
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
            self.sidebar.update(cx, |sb, scx| { sb.exit_search(); scx.notify(); });
            self.set_composer_focused(true, cx);
            return true;
        }
        if self.composer.read(cx).replying_to.is_some() || self.stage.read(cx).pending_reply.is_some() {
            self.composer.update(cx, |c, ccx| { c.cancel_reply_or_edit(); ccx.notify(); });
            self.stage.update(cx, |st, stcx| { st.pending_reply = None; stcx.notify(); });
            return true;
        }
        false
    }

    fn handle_escape_key(&mut self, is_shift: bool, cx: &mut Context<Self>) -> bool {
        if self.is_authenticated && self.overlays.is_passcode_locked {
            self.ime.clear_preedit();
            self.overlays.passcode_unlock_input.clear();
            self.overlays.passcode_unlock_error = None;
            cx.notify();
            return true;
        }
        if !self.is_authenticated
            && self.auth_wizard.update(cx, |w, wcx| w.handle_non_char_key("escape", is_shift, false, wcx))
        {
            cx.notify();
            return true;
        }
        if self.dismiss_pre_auth_escape(is_shift, cx)
            || (self.is_authenticated && self.dismiss_post_auth_escape(cx))
        {
            if self.is_authenticated && self.active_panel == ActiveWorkspacePanel::Browser {
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
    pub fn handle_non_char_key_with_alt(&mut self, key: &str, is_shift: bool, is_alt: bool, cx: &mut Context<Self>) -> bool {
        if key == "escape" { return self.handle_escape_key(is_shift, cx); }
        if self.is_authenticated && self.overlays.is_passcode_locked {
            match key {
                "backspace" => {
                    if self.ime.ime_preedit.take().is_none() {
                        backspace_string(&mut self.overlays.passcode_unlock_input, is_alt);
                        self.overlays.passcode_unlock_error = None;
                    }
                    cx.notify();
                }
                "enter" => {
                    if !self.take_ime_preedit_commit(cx) {
                        self.submit_passcode_unlock_input(cx);
                    }
                }
                _ => {}
            }
            return true;
        }
        if self.is_authenticated {
            if let Some(ov) = self.active_computer_terminal_overlay(cx) {
                if key == "backspace" && self.ime.ime_preedit.take().is_some() {
                    ov.update(cx, |w, wcx| {
                        if let Some(c) = w.terminal_input.as_ref() {
                            c.update(wcx, |ctrl, _| ctrl.clear_preedit());
                        }
                        if let Some(term) = w.terminal_view.as_ref() {
                            term.update(wcx, |t, tcx| {
                                t.ime_preedit = None;
                                tcx.notify();
                            });
                        }
                        wcx.notify();
                    });
                    cx.notify();
                    return true;
                }
                if key == "enter" && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed()) {
                    return true;
                }
                if ov.update(cx, |w, wcx| w.handle_terminal_non_char_key(key, is_alt, wcx)) {
                    cx.notify();
                    return true;
                }
            }
        }
        if self.is_authenticated {
            self.record_user_activity();
        }
        if self.is_authenticated && self.modal_container.read(cx).is_modal_active() {
            let handled = self.modal_container.update(cx, |mc, mcx| {
                let ok = mc.handle_key_input(key, None);
                if ok { mcx.notify(); }
                ok
            });
            if let Some(outcome) = self.modal_container.update(cx, |mc, _| mc.take_outcome()) {
                self.handle_modal_outcome(outcome, cx);
            }
            if handled { cx.notify(); return true; }
        }
        if !self.is_authenticated {
            let handled = self.auth_wizard.update(cx, |w, wcx| w.handle_non_char_key(key, is_shift, is_alt, wcx));
            if handled { cx.notify(); }
            return handled;
        }
        if let Some(box_ent) = self.miniapps.create_bot_box.as_ref().filter(|b| b.read(cx).is_visible).cloned() {
            match key {
                "tab" => {
                    self.ime.clear_preedit();
                    box_ent.update(cx, |b, bcx| {
                        b.cycle_active_field();
                        bcx.notify();
                    });
                }
                "backspace" => if self.ime.ime_preedit.take().is_none() { box_ent.update(cx, |b, bcx| { b.backspace(); bcx.notify(); }); },
                "enter" => if !self.take_ime_preedit_commit(cx) { box_ent.update(cx, |b, bcx| { b.submit(); bcx.notify(); }); },
                _ => return false,
            }
            cx.notify();
            return true;
        }
        if let Some(ov) = self.miniapps.miniapp_overlay.as_ref().filter(|w| w.read(cx).has_focused_text_input()).cloned() {
            match key {
                "backspace" => {
                    if self.ime.ime_preedit.take().is_none() { ov.update(cx, |w, wcx| { w.backspace_focused_input(); wcx.notify(); }); }
                    cx.notify();
                    return true;
                }
                "enter" => {
                    if !self.take_ime_preedit_commit(cx) { ov.update(cx, |w, wcx| { w.submit_focused_input(); wcx.notify(); }); }
                    cx.notify();
                    return true;
                }
                _ => {}
            }
        }
        if self.sidebar.read(cx).is_contacts_modal_open {
            if key == "backspace" {
                if self.ime.ime_preedit.take().is_none() {
                    self.sidebar.update(cx, |sb, scx| { backspace_string(&mut sb.contacts_search_query, is_alt); scx.notify(); });
                }
                cx.notify();
                return true;
            }
            return false;
        }
        if self.sidebar.read(cx).active_creation_modal.is_some() {
            match key {
                "tab" => {
                    self.ime.clear_preedit();
                    self.sidebar.update(cx, |sb, scx| { sb.cycle_creation_field(); scx.notify(); });
                }
                "backspace" => if self.ime.ime_preedit.take().is_none() {
                    self.sidebar.update(cx, |sb, scx| { sb.backspace_creation_text(); scx.notify(); });
                },
                "enter" => {
                    if self.take_ime_preedit_commit(cx) { return true; }
                    self.sidebar.update(cx, |sb, scx| { sb.submit_creation_modal(); scx.notify(); });
                }
                _ => return false,
            }
            cx.notify();
            return true;
        }
        if self.is_apple_compose_input_active(cx) {
            if self.sidebar.read(cx).compose_title_focused {
                match key {
                    "backspace" => {
                        if self.ime.ime_preedit.take().is_none() {
                            self.sidebar.update(cx, |sb, scx| {
                                backspace_string(&mut sb.compose_draft.custom_group_title, is_alt);
                                scx.notify();
                            });
                        }
                        cx.notify();
                        return true;
                    }
                    "enter" | "tab" => {
                        if key == "enter"
                            && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed())
                        {
                            return true;
                        }
                        self.ime.clear_preedit();
                        self.sidebar.update(cx, |sb, scx| {
                            sb.compose_title_focused = false;
                            scx.notify();
                        });
                        self.set_composer_focused(true, cx);
                        cx.notify();
                        return true;
                    }
                    _ => {}
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
                    return true;
                }
                "down" | "arrowdown" => {
                    let total = self.collect_apple_compose_recipients(cx).len();
                    self.sidebar.update(cx, |sb, scx| {
                        sb.move_compose_highlight(1, total);
                        scx.notify();
                    });
                    cx.notify();
                    return true;
                }
                "up" | "arrowup" => {
                    let total = self.collect_apple_compose_recipients(cx).len();
                    self.sidebar.update(cx, |sb, scx| {
                        sb.move_compose_highlight(-1, total);
                        scx.notify();
                    });
                    cx.notify();
                    return true;
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
                    return true;
                }
                "tab" => {
                    self.ime.clear_preedit();
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
                    return true;
                }
                _ => {}
            }
        }
        if self.is_settings_input_active(cx) {
            if key == "backspace" && self.ime.ime_preedit.take().is_some() {
                self.settings_router.update(cx, |sr, scx| {
                    let _ = sr.set_active_input_ime_preedit(None, scx);
                });
                cx.notify();
                return true;
            }
            if key == "enter" && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed()) {
                return true;
            }
            if let Some(handled) = self.settings_router.update(cx, |sr, scx| sr.handle_non_char_key(key, is_shift, scx)) {
                if handled { cx.notify(); }
                return handled;
            }
        }
        if self.is_inspector_bot_input_active(cx) {
            if key == "backspace" && self.ime.ime_preedit.take().is_some() {
                self.inspector.update(cx, |ins, icx| ins.set_bot_ime_preedit(None, icx));
                cx.notify();
                return true;
            }
            if key == "enter" && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed()) {
                return true;
            }
            if self.inspector.update(cx, |ins, icx| ins.handle_bot_non_char_key(key, is_alt, icx)) {
                cx.notify();
                return true;
            }
        }
        if self.is_bot_card_input_active(cx) {
            match key {
                "tab" => {
                    self.ime.clear_preedit();
                    self.stage.update(cx, |st, stcx| {
                        st.bot_assist_ime_preedit = None;
                        st.cycle_bot_card_field(is_shift);
                        stcx.notify();
                    });
                }
                "backspace" => {
                    if self.ime.ime_preedit.take().is_some() {
                        self.stage.update(cx, |st, stcx| {
                            st.bot_assist_ime_preedit = None;
                            stcx.notify();
                        });
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
            return true;
        }
        if self.stage.read(cx).search_open && !self.sidebar.read(cx).search_focused {
            match key {
                "backspace" => {
                    if self.ime.ime_preedit.take().is_none() {
                        self.stage.update(cx, |st, stcx| {
                            backspace_string(&mut st.search_query, is_alt);
                            st.recompute_search_matches();
                            stcx.notify();
                        });
                    }
                    cx.notify();
                    return true;
                }
                "enter" | "down" => {
                    if !self.take_ime_preedit_commit(cx) {
                        self.stage.update(cx, |st, stcx| { st.step_search_match(if is_shift { -1 } else { 1 }); stcx.notify(); });
                    }
                    cx.notify();
                    return true;
                }
                "up" => {
                    self.stage.update(cx, |st, stcx| { st.step_search_match(-1); stcx.notify(); });
                    cx.notify();
                    return true;
                }
                _ => {}
            }
        }
        if self.is_studio_input_active(cx)
            && self.tasks_workspace.update(cx, |w, wcx| { let ok = w.apply_text_key_to_active_input(key, None); if ok { wcx.notify(); } ok })
        {
            cx.notify();
            return true;
        }
        if self.is_browser_workspace_input_active(cx) {
            if key == "backspace" && self.ime.ime_preedit.take().is_some() {
                cx.notify();
                return true;
            }
            if key == "enter" && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed()) {
                cx.notify();
                return true;
            }
            if key == "tab" {
                self.ime.clear_preedit();
            }
            if let Some(bw) = self.browser_workspace.clone() {
                if bw.update(cx, |w, wcx| w.handle_non_char_key_input(key, is_alt, wcx)) {
                    cx.notify();
                    return true;
                }
            }
        }
        if self.is_code_scene_input_active(cx) {
            if key == "backspace" && self.ime.ime_preedit.take().is_some() {
                if let Some(cs) = self.code_scene_workspace.clone() {
                    cs.update(cx, |w, wcx| w.set_ime_preedit(None, wcx));
                }
                cx.notify();
                return true;
            }
            if key == "enter" && (self.take_ime_preedit_commit(cx) || self.ime.take_just_committed()) {
                cx.notify();
                return true;
            }
            if key == "tab" {
                self.ime.clear_preedit();
            }
            if let Some(cs) = self.code_scene_workspace.clone() {
                if cs.update(cx, |w, wcx| w.handle_non_char_key_input(key, is_shift, is_alt, wcx)) {
                    cx.notify();
                    return true;
                }
            }
        }
        if self.active_panel == ActiveWorkspacePanel::Chats
            && !self.sidebar.read(cx).search_focused
            && self.sidebar.read(cx).active_creation_modal.is_none()
            && !self.should_hide_composer(cx)
            && matches!(key, "up" | "down" | "tab" | "enter")
            && self.composer.update(cx, |c, ccx| { let ok = c.handle_hermes_autocomplete_nav(key, is_shift); if ok { ccx.notify(); } ok })
        {
            cx.notify();
            return true;
        }
        if key == "tab" {
            if matches!(
                self.active_panel,
                ActiveWorkspacePanel::Browser | ActiveWorkspacePanel::CodeScene
            ) && !self.sidebar.read(cx).search_focused
            {
                return true;
            }
            let currently_search = self.sidebar.read(cx).search_focused;
            self.sidebar.update(cx, |sb, scx| {
                if currently_search { sb.exit_search(); } else { sb.focus_search(); }
                scx.notify();
            });
            self.set_composer_focused(currently_search, cx);
            cx.notify();
            return true;
        }
        if self.sidebar.read(cx).search_focused {
            match key {
                "backspace" => if self.ime.ime_preedit.take().is_none() {
                    self.sidebar.update(cx, |sb, scx| {
                        let mut q = sb.search_query.clone();
                        backspace_string(&mut q, is_alt);
                        sb.set_search_query(q);
                        scx.notify();
                    });
                },
                "enter" => {
                    if self.take_ime_preedit_commit(cx) { return true; }
                    self.sidebar.update(cx, |sb, scx| { sb.search_focused = false; scx.notify(); });
                    self.set_composer_focused(true, cx);
                }
                _ => return false,
            }
            cx.notify();
            return true;
        }
        if self.should_hide_composer(cx) {
            return false;
        }
        match key {
            "enter" => {
                if self.ime.take_just_committed() {
                    self.composer.update(cx, |c, ccx| { c.ime_machine.reset_commit_fence(); ccx.notify(); });
                    cx.notify();
                    return true;
                }
                let action = self.composer.update(cx, |c, ccx| {
                    c.ime_machine.reset_commit_fence();
                    let a = c.handle_enter(is_shift);
                    ccx.notify();
                    a
                });
                if action == KeyAction::SendMessage {
                    self.composer.update(cx, |c, ccx| { c.submit_primary(); ccx.notify(); });
                    self.handle_composer_pending_actions(cx);
                }
                cx.notify();
                true
            }
            "backspace" => {
                self.composer.update(cx, |c, ccx| {
                    if c.ime_machine.is_composing() { c.ime_machine.on_ime_cancel(); } else { c.ime_machine.backspace(); }
                    c.sync_autocomplete();
                    ccx.notify();
                });
                cx.notify();
                true
            }
            _ => false,
        }
    }
    /// Commits any pending IME preedit for the active input (equivalent to `EntityInputHandler::unmark_text`).
    pub fn commit_ime_unmark_text(&mut self, cx: &mut Context<Self>) {
        if !self.is_authenticated {
            let pending = self.auth_wizard.update(cx, |w, wcx| {
                w.ime.just_committed_ime = true;
                let p = w.ime.ime_preedit.take();
                wcx.notify();
                p
            });
            if let Some(p) = pending.filter(|p| !p.is_empty()) { self.insert_committed_text(&p, cx); return; }
            cx.notify();
            return;
        }
        self.ime.mark_ime_committed();
        if let Some(p) = self.ime.ime_preedit.take().filter(|p| !p.is_empty()) { self.insert_committed_text(&p, cx); return; }
        if !self.should_hide_composer(cx) {
            if let Some(p) = self.composer.read(cx).ime_machine.preedit_text().map(str::to_string) {
                self.composer.update(cx, |c, ccx| { c.ime_machine.on_ime_commit(&p); ccx.notify(); });
            }
        }
        cx.notify();
    }

    /// Updates or clears the active IME preedit (equivalent to `EntityInputHandler::replace_and_mark_text_in_range`).
    pub fn update_ime_marked_text(&mut self, new_text: &str, cx: &mut Context<Self>) {
        if !self.is_authenticated {
            self.auth_wizard.update(cx, |w, wcx| {
                if new_text.is_empty() { w.ime.clear_preedit(); }
                else { w.ime.ime_preedit = Some(new_text.to_string()); w.ime.just_committed_ime = false; }
                wcx.notify();
            });
            cx.notify();
            return;
        }
        let non_composer = self.is_non_composer_text_capturing(cx);
        if new_text.is_empty() {
            self.ime.clear_preedit();
            self.bot_browser.ime_preedit_input_gen = None;
            if let Some(ov) = self.active_computer_terminal_overlay(cx) {
                ov.update(cx, |w, wcx| {
                    if let Some(c) = w.terminal_input.as_ref() {
                        c.update(wcx, |ctrl, _| ctrl.clear_preedit());
                    }
                    if let Some(term) = w.terminal_view.as_ref() {
                        term.update(wcx, |t, tcx| {
                            t.ime_preedit = None;
                            tcx.notify();
                        });
                    }
                    wcx.notify();
                });
            } else if self.is_settings_input_active(cx) {
                self.settings_router.update(cx, |sr, scx| {
                    let _ = sr.set_active_input_ime_preedit(None, scx);
                });
            } else if self.is_inspector_bot_input_active(cx) {
                self.inspector.update(cx, |ins, icx| ins.set_bot_ime_preedit(None, icx));
            } else if self.is_bot_card_input_active(cx) {
                self.stage.update(cx, |st, stcx| {
                    st.bot_assist_ime_preedit = None;
                    stcx.notify();
                });
            } else if self.is_code_scene_input_active(cx) {
                if let Some(cs) = self.code_scene_workspace.clone() {
                    cs.update(cx, |w, wcx| w.set_ime_preedit(None, wcx));
                }
            }
            if !non_composer {
                self.composer.update(cx, |c, ccx| { c.ime_machine.on_ime_cancel(); c.sync_autocomplete(); ccx.notify(); });
            }
        } else if non_composer {
            self.ime.ime_preedit = Some(new_text.to_string());
            self.ime.just_committed_ime = false;
            self.bot_browser.ime_preedit_input_gen = if self.is_browser_workspace_input_active(cx) {
                self.browser_workspace
                    .as_ref()
                    .map(|bw| bw.read(cx).active_input_generation())
            } else {
                None
            };
            if let Some(ov) = self.active_computer_terminal_overlay(cx) {
                ov.update(cx, |w, wcx| {
                    if w.is_interactive_pty_focused(wcx) {
                        let _ = w.replace_and_mark_terminal_input(new_text, wcx);
                    } else {
                        w.ensure_terminal_input(wcx);
                        if let Some(c) = w.terminal_input.as_ref() {
                            c.update(wcx, |ctrl, _| {
                                ctrl.ime_preedit = Some(new_text.to_string());
                                ctrl.just_committed_ime = false;
                            });
                        }
                    }
                    wcx.notify();
                });
            } else if self.is_settings_input_active(cx) {
                self.settings_router.update(cx, |sr, scx| {
                    let _ = sr.set_active_input_ime_preedit(Some(new_text), scx);
                });
            } else if self.is_inspector_bot_input_active(cx) {
                self.inspector.update(cx, |ins, icx| ins.set_bot_ime_preedit(Some(new_text), icx));
            } else if self.is_bot_card_input_active(cx) {
                self.stage.update(cx, |st, stcx| {
                    st.bot_assist_ime_preedit = Some(new_text.to_string());
                    stcx.notify();
                });
            } else if self.is_code_scene_input_active(cx) {
                if let Some(cs) = self.code_scene_workspace.clone() {
                    cs.update(cx, |w, wcx| {
                        w.set_ime_preedit(Some(new_text.to_string()), wcx);
                    });
                }
            }
        } else {
            self.composer.update(cx, |c, ccx| {
                c.is_input_focused = true;
                c.ime_machine.on_ime_update(new_text, new_text.len());
                c.sync_autocomplete();
                ccx.notify();
            });
        }
        cx.notify();
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
        let was_composing = self.has_active_ime_preedit(cx) || self.composer.read(cx).ime_machine.is_composing();
        self.insert_committed_text(text, cx);
        if was_composing {
            if self.is_authenticated { self.ime.mark_ime_committed(); }
            else { self.auth_wizard.update(cx, |w, _| w.ime.mark_ime_committed()); }
        }
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
        let centered_modal = !self.is_authenticated
            || self.overlays.is_passcode_locked
            || self.is_computer_workbench_input_active(cx)
            || self.modal_container.read(cx).is_modal_active()
            || self.miniapps.create_bot_box.as_ref().is_some_and(|b| b.read(cx).is_visible)
            || self.miniapps.miniapp_overlay.as_ref().is_some_and(|w| w.read(cx).has_focused_text_input())
            || self.sidebar.read(cx).active_creation_modal.is_some()
            || self.sidebar.read(cx).is_contacts_modal_open
            || self.is_settings_input_active(cx)
            || self.is_studio_input_active(cx);

        let full_text = self.combined_active_text(cx);
        let char_count = full_text.chars().count();
        if self.is_inspector_bot_input_active(cx) {
            let caret_x = (element_bounds.origin.x + element_bounds.size.width - px(290.0)
                + px((char_count as f32 * 7.0).min(220.0)))
                .max(element_bounds.origin.x + px(40.0));
            let caret_y = element_bounds.origin.y + px(380.0);
            return Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(18.0))));
        }
        if self.is_bot_card_input_active(cx) {
            let caret_x = (element_bounds.origin.x + px(420.0)
                + px((char_count as f32 * 7.5).min(240.0)))
            .min(element_bounds.origin.x + element_bounds.size.width - px(80.0));
            let caret_y = (element_bounds.origin.y + element_bounds.size.height - px(220.0))
                .max(element_bounds.origin.y + px(100.0));
            return Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(18.0))));
        }
        if centered_modal {
            let cx_mid = element_bounds.origin.x + element_bounds.size.width / 2.0;
            let cy_mid = element_bounds.origin.y + element_bounds.size.height / 2.0;
            let caret_x = cx_mid - px(140.0) + px((char_count as f32 * 8.0).min(280.0));
            let caret_y = if !self.is_authenticated { cy_mid + px(10.0) } else { cy_mid - px(24.0) };
            return Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(20.0))));
        }
        if self.is_apple_compose_input_active(cx) {
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
            let caret_x = element_bounds.origin.x
                + px(sb_width + 82.0 + chip_offset + (char_count as f32 * 8.0).min(260.0));
            let caret_y = element_bounds.origin.y + px(if title_focused { 66.0 } else { 22.0 });
            return Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(18.0))));
        }
        if self.is_browser_workspace_input_active(cx) {
            let caret_x = element_bounds.origin.x
                + px(360.0)
                + px((char_count as f32 * 7.5).min(360.0));
            return Some(Bounds::new(point(caret_x, element_bounds.origin.y + px(28.0)), size(px(2.0), px(18.0))));
        }
        if self.is_code_scene_input_active(cx) {
            let caret_x = (element_bounds.origin.x + element_bounds.size.width - px(360.0)
                + px((char_count as f32 * 7.5).min(280.0)))
                .max(element_bounds.origin.x + px(320.0));
            let caret_y = element_bounds.origin.y + px(180.0);
            return Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(18.0))));
        }
        if self.stage.read(cx).search_open {
            let caret_x = element_bounds.origin.x + px(360.0) + px((char_count as f32 * 8.0).min(280.0));
            return Some(Bounds::new(point(caret_x, element_bounds.origin.y + px(84.0)), size(px(2.0), px(18.0))));
        }
        if self.sidebar.read(cx).search_focused {
            let sb_chars = self.sidebar.read(cx).search_query.chars().count();
            let caret_x = element_bounds.origin.x + px(96.0) + px((sb_chars as f32 * 7.5).min(200.0));
            return Some(Bounds::new(point(caret_x, element_bounds.origin.y + px(22.0)), size(px(2.0), px(18.0))));
        }
        let mut advance = 0.0f32;
        for ch in full_text.chars() {
            advance = if ch == '\n' { 0.0 } else if ch.is_ascii() { advance + 8.2 } else { advance + 14.0 };
        }
        let max_w = (f32::from(element_bounds.size.width) - 818.0).max(100.0);
        let caret_x = element_bounds.origin.x + px(458.0 + advance.clamp(0.0, max_w));
        let caret_y = element_bounds.origin.y + element_bounds.size.height - px(36.0);
        Some(Bounds::new(point(caret_x, caret_y), size(px(2.0), px(20.0))))
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        Some(self.active_utf16_len(cx))
    }
}
