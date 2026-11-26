//! Keyboard shortcuts, window key dispatch, inspector toggle, language toggle, and nav switching.

use gpui::{ClipboardItem, Context, KeyDownEvent, Window};

use crate::views::app_shell::shell_input_ime::ShellInputTarget;
use crate::views::boxes::modal_types::ModalDialogKind;
use crate::views::content_hub::{ContentHubInputTarget, ContentHubSection};
use crate::views::global_tasks_workspace::TasksWorkspaceTab;
use crate::views::nav_rail::NavDestination;
use crate::views::settings::settings_type::SettingsSectionType;
use crate::views::shell::{
    nav_destination_for_shortcut_digit, next_tdesktop_theme_cycle, toggled_language_code,
    ActiveWorkspacePanel, AppShell,
};
use crate::views::tdesktop_theme::tr_str;

impl AppShell {
    /// Synchronizes `ContextInspector` open state with `ConversationStage::is_inspector_open`.
    pub fn set_inspector_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if !open && self.is_inspector_bot_input_active(cx) {
            self.clear_all_ime_preedits(cx);
        }
        self.inspector.update(cx, |insp, insp_cx| {
            if open {
                insp.open();
            } else {
                insp.close();
            }
            insp_cx.notify();
        });
        self.stage.update(cx, |st, st_cx| {
            st.is_inspector_open = open;
            st_cx.notify();
        });
        cx.notify();
    }

    /// Toggles the active interface language (`zh-CN` <-> `en-US`) and persists preferences.
    pub fn toggle_language(&mut self, cx: &mut Context<Self>) {
        let next_lang = toggled_language_code(&self.preferences.localization.language_code);
        self.update_preferences(cx, |p| {
            p.localization.language_code = next_lang.to_string();
        });
    }

    /// Switches to a `NavDestination` panel and synchronizes `NavRail` + workspace data cleanly.
    pub fn activate_nav_destination(&mut self, dest: NavDestination, cx: &mut Context<Self>) {
        self.clear_ime_and_blur_content_hub_for_transition(true, cx);
        if matches!(
            dest,
            NavDestination::Chats
                | NavDestination::Folders
                | NavDestination::SavedMessages
                | NavDestination::Calls
                | NavDestination::Browser
                | NavDestination::Community
                | NavDestination::OrcaBots
                | NavDestination::OrcaIde
                | NavDestination::TaskCenter
                | NavDestination::McpTools
        ) {
            self.cleanup_settings_panel_if_active(cx);
        } else if dest == NavDestination::TalkMode {
            self.stop_settings_mic_test_capture(cx);
        }
        self.last_handled_nav_dest = Some(dest);
        let nav_trigger = self.nav_rail.update(cx, |r, r_cx| {
            r.set_active_destination(dest);
            r_cx.notify();
            r.trigger_counter
        });
        self.last_handled_nav_trigger = nav_trigger;
        if dest != NavDestination::Browser {
            self.hide_browser_native_webview(cx);
        }
        if dest != NavDestination::Community {
            self.leave_community_panel_if_active(cx);
        }
        match dest {
            NavDestination::Chats | NavDestination::Folders => {
                self.active_panel = ActiveWorkspacePanel::Chats;
                self.sync_apple_sidebar_to_active_panel(cx);
                self.sidebar.update(cx, |sb, sb_cx| {
                    sb.cancel_compose_mode();
                    sb.is_account_popover_open = false;
                    match dest {
                        NavDestination::Chats => sb.select_folder("All"),
                        NavDestination::Folders => {
                            if let Some(first_folder) = sb
                                .folders
                                .iter()
                                .find(|(f, _)| f != "All")
                                .map(|(f, _)| f.clone())
                            {
                                sb.select_folder(first_folder);
                            }
                        }
                        _ => {}
                    }
                    sb_cx.notify();
                });
                if self.composer.read(cx).is_compose_isolated
                    && !self.is_compose_or_creation_modal_active(cx)
                {
                    self.exit_compose_or_modal_isolation(true, cx);
                } else {
                    self.sync_compose_state_to_composer(true, cx);
                }
                self.stage.update(cx, |st, st_cx| {
                    st.more_menu_open = false;
                    st.ttl_submenu_open = false;
                    st_cx.notify();
                });
            }
            NavDestination::SavedMessages => {
                self.active_panel = ActiveWorkspacePanel::Chats;
                self.sync_apple_sidebar_to_active_panel(cx);
                let target_saved = {
                    let sb = self.sidebar.read(cx);
                    sb.saved_messages_id.or_else(|| {
                        let uid = self.session_sync.user_id?;
                        sb.dialogs.iter().find(|d| d.id == uid).map(|d| d.id)
                    })
                };
                let was_isolated = self.composer.read(cx).is_compose_isolated;
                let prev_active = self.stage.read(cx).active_chat_id;
                self.sidebar.update(cx, |sb, sb_cx| {
                    sb.cancel_compose_mode();
                    sb.is_account_popover_open = false;
                    if let Some(saved_id) = target_saved {
                        sb.select_dialog(saved_id);
                    }
                    sb_cx.notify();
                });
                if let Some(saved_id) = target_saved {
                    if prev_active == Some(saved_id) {
                        if was_isolated && !self.is_compose_or_creation_modal_active(cx) {
                            self.exit_compose_or_modal_isolation(true, cx);
                        } else {
                            self.sync_compose_state_to_composer(true, cx);
                        }
                        self.sync_selected_chat_inspector_and_stage(saved_id, cx);
                    } else {
                        // Keep `is_compose_isolated == true` across `sync_selected_chat_inspector_and_stage`
                        // so the previous chat's saved draft is never flushed/overwritten before switching.
                        self.sync_selected_chat_inspector_and_stage(saved_id, cx);
                        if was_isolated && !self.is_compose_or_creation_modal_active(cx) {
                            self.finish_compose_isolation_for_switched_chat(saved_id, cx);
                        } else {
                            self.clear_compose_isolation_snapshot();
                            self.sync_compose_state_to_composer(true, cx);
                        }
                    }
                } else if was_isolated && !self.is_compose_or_creation_modal_active(cx) {
                    self.exit_compose_or_modal_isolation(true, cx);
                } else {
                    self.sync_compose_state_to_composer(true, cx);
                }
                self.stage.update(cx, |st, st_cx| {
                    st.more_menu_open = false;
                    st.ttl_submenu_open = false;
                    st_cx.notify();
                });
            }
            NavDestination::Calls => {
                let shell = &mut *self;
                shell.active_panel = ActiveWorkspacePanel::Calls;
                shell.sync_calls_history(cx);
            }
            NavDestination::TalkMode => {
                if self.is_active_chat_bot(cx) {
                    self.start_ai_talk_mode_session(false, cx);
                } else if let Some(pid) = self.current_peer_user_id(cx) {
                    let name = self.stage.read(cx).chat_title.clone();
                    self.start_webrtc_call(pid, name, false, cx);
                } else {
                    self.show_talk_mode(cx);
                }
            }
            NavDestination::RemoteComputer => {
                self.open_remote_computers_hub(cx);
            }
            NavDestination::Browser => {
                self.open_browser_workspace(cx);
            }
            NavDestination::Community => {
                self.open_community_workspace(cx);
            }
            NavDestination::OrcaBots => {
                let _ = self.open_orca_bots_workspace(cx);
            }
            NavDestination::OrcaIde => {
                let _ = self.open_orca_ide_workspace(cx);
            }
            NavDestination::TaskCenter => {
                self.active_panel = ActiveWorkspacePanel::TaskCenter;
                self.tasks_workspace
                    .update(cx, |w, _| w.set_tab(TasksWorkspaceTab::Tasks));
                self.refresh_tasks_and_mcp(cx);
            }
            NavDestination::McpTools => {
                self.active_panel = ActiveWorkspacePanel::McpTools;
                self.tasks_workspace
                    .update(cx, |w, _| w.set_tab(TasksWorkspaceTab::McpTools));
                self.refresh_tasks_and_mcp(cx);
            }
            NavDestination::Settings => {
                if self.active_panel == ActiveWorkspacePanel::Settings {
                    self.settings_router.update(cx, |sr, scx| {
                        sr.settings_stack.clear();
                        sr.navigate_to(SettingsSectionType::Main, false, scx);
                    });
                } else {
                    self.navigate_settings_to(SettingsSectionType::Main, cx);
                }
            }
        }
        if matches!(
            dest,
            NavDestination::Calls
                | NavDestination::TalkMode
                | NavDestination::RemoteComputer
                | NavDestination::Browser
                | NavDestination::Community
                | NavDestination::OrcaBots
                | NavDestination::OrcaIde
                | NavDestination::TaskCenter
                | NavDestination::McpTools
                | NavDestination::Settings
        ) {
            self.sidebar.update(cx, |sb, sb_cx| {
                sb.cancel_compose_mode();
                sb.is_account_popover_open = false;
                sb_cx.notify();
            });
            if self.composer.read(cx).is_compose_isolated
                && !self.is_compose_or_creation_modal_active(cx)
            {
                self.exit_compose_or_modal_isolation(true, cx);
            } else {
                self.sync_compose_state_to_composer(true, cx);
            }
            self.stage.update(cx, |st, st_cx| {
                st.more_menu_open = false;
                st.ttl_submenu_open = false;
                st_cx.notify();
            });
            self.sync_apple_sidebar_to_active_panel(cx);
        }
        self.reconcile_chat_scoped_toasts(cx);
        cx.notify();
    }

    /// Processes Cmd/Ctrl-prefixed shell and workspace keyboard shortcuts.
    pub fn handle_shortcut_key(&mut self, key: &str, is_shift: bool, cx: &mut Context<Self>) -> bool {
        let key_lower = key.to_ascii_lowercase();
        if self.is_authenticated && self.overlays.is_passcode_locked {
            match key_lower.as_str() {
                "v" => {
                    if let Some(clip_text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.insert_committed_text(&clip_text, cx);
                    }
                    return true;
                }
                "backspace" | "a" | "u" => {
                    self.clear_active_field(cx);
                    return true;
                }
                _ => return true,
            }
        }
        if self.is_authenticated && self.has_active_orca_security_modal(cx) {
            return true;
        }
        if self.is_authenticated && self.orca_jump_palette.is_some() {
            self.record_user_activity();
            if !is_shift {
                match key_lower.as_str() {
                    "1" | "2" | "3" | "4" | "5" | "6" => {
                        let digit = key_lower.as_bytes()[0] - b'0';
                        let _ = self.handle_jump_palette_shortcut_digit(digit, cx);
                        return true;
                    }
                    "j" => {
                        self.close_scoped_jump_palette(cx);
                        return true;
                    }
                    "v" => {
                        if let Some(clip_text) =
                            cx.read_from_clipboard().and_then(|item| item.text())
                        {
                            self.insert_committed_text(&clip_text, cx);
                        }
                        return true;
                    }
                    "backspace" | "a" | "u" => {
                        self.clear_active_field(cx);
                        return true;
                    }
                    _ => return true,
                }
            }
            return true;
        }
        if self.is_authenticated {
            self.record_user_activity();
        }
        let target = self.resolve_input_target(cx);
        let publish_modal_ime_active = target == ShellInputTarget::PublishReviewModal
            && self.is_publish_review_modal_input_active(cx);
        let content_hub_ime_active = target == ShellInputTarget::ContentHubInput;
        match key_lower.as_str() {
            "v" => {
                if let Some(clip_text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.insert_committed_text(&clip_text, cx);
                }
                return true;
            }
            "c" if !is_shift && (publish_modal_ime_active || content_hub_ime_active) => {
                let (committed, _) = self.active_text_and_preedit(cx);
                if !committed.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(committed));
                }
                return true;
            }
            "a" if publish_modal_ime_active || content_hub_ime_active => {
                if publish_modal_ime_active {
                    self.content_hub.update(cx, |h, hcx| {
                        let _ = h.publish_modal.select_all_active_field();
                        hcx.notify();
                    });
                } else {
                    self.content_hub.update(cx, |h, hcx| {
                        let _ = h.select_all_active_input();
                        hcx.notify();
                    });
                }
                cx.notify();
                return true;
            }
            "backspace" | "a" | "u" => {
                self.clear_active_field(cx);
                return true;
            }
            "d" => {
                let next = next_tdesktop_theme_cycle(self.theme_state.theme_mode);
                self.apply_theme_selection(next, None, None, cx);
                return true;
            }
            "l" => {
                if is_shift && self.is_authenticated && self.has_local_passcode_configured() {
                    self.lock_app_now(cx);
                } else {
                    self.toggle_language(cx);
                }
                return true;
            }
            _ => {}
        }

        if self.is_authenticated && self.has_blocking_modal_or_fullscreen_overlay_for_shortcuts(cx) {
            return true;
        }

        if !self.is_authenticated || !self.preferences.shortcuts_enabled {
            return false;
        }

        if self.sidebar.read(cx).is_account_popover_open && key_lower != "e" {
            return true;
        }

        let lang = self.active_lang_id();
        match key_lower.as_str() {
            "k" => match self.active_panel {
                ActiveWorkspacePanel::ContentHub(sec) => {
                    self.clear_ime_and_blur_content_hub_for_transition(false, cx);
                    let target = if sec == ContentHubSection::MyCreations {
                        ContentHubInputTarget::CreationsSearch
                    } else {
                        ContentHubInputTarget::CatalogSearch
                    };
                    self.content_hub.update(cx, |h, hcx| {
                        h.focus_input(target, hcx);
                    });
                    self.blur_composer_and_cancel_ime(cx);
                    cx.notify();
                    true
                }
                ActiveWorkspacePanel::Browser => {
                    self.ime.clear_preedit();
                    self.bot_browser.ime_preedit_input_gen = None;
                    if let Some(ref bw) = self.browser_workspace {
                        bw.update(cx, |w, wcx| {
                            w.set_active_input(
                                crate::views::browser::ActiveBrowserInput::None,
                                None,
                                wcx,
                            );
                        });
                    }
                    self.sidebar.update(cx, |sb, sb_cx| {
                        sb.focus_search();
                        sb_cx.notify();
                    });
                    if self.composer.read(cx).is_compose_isolated
                        && !self.is_compose_or_creation_modal_active(cx)
                    {
                        self.exit_compose_or_modal_isolation(true, cx);
                    } else {
                        self.sync_compose_state_to_composer(true, cx);
                    }
                    cx.notify();
                    true
                }
                ActiveWorkspacePanel::Community => {
                    self.clear_ime_and_blur_content_hub_for_transition(true, cx);
                    self.community_ime_preedit_input_gen = None;
                    if let Some(ref cw) = self.community_workspace {
                        cw.update(cx, |w, wcx| {
                            w.blur_active_input(wcx);
                        });
                    }
                    self.sidebar.update(cx, |sb, sb_cx| {
                        sb.focus_search();
                        sb_cx.notify();
                    });
                    if self.composer.read(cx).is_compose_isolated
                        && !self.is_compose_or_creation_modal_active(cx)
                    {
                        self.exit_compose_or_modal_isolation(true, cx);
                    } else {
                        self.sync_compose_state_to_composer(true, cx);
                    }
                    self.blur_composer_and_cancel_ime(cx);
                    cx.notify();
                    true
                }
                ActiveWorkspacePanel::Chats => {
                    self.clear_ime_and_blur_content_hub_for_transition(true, cx);
                    self.sidebar.update(cx, |sb, sb_cx| {
                        sb.focus_search();
                        sb_cx.notify();
                    });
                    if self.composer.read(cx).is_compose_isolated
                        && !self.is_compose_or_creation_modal_active(cx)
                    {
                        self.exit_compose_or_modal_isolation(true, cx);
                    } else {
                        self.sync_compose_state_to_composer(true, cx);
                    }
                    self.blur_composer_and_cancel_ime(cx);
                    cx.notify();
                    true
                }
                _ => true,
            },
            "," if self.is_authenticated => {
                self.navigate_settings_to(SettingsSectionType::Main, cx);
                true
            }
            "[" if self.active_panel == ActiveWorkspacePanel::Settings => {
                self.navigate_settings_back(cx);
                true
            }
            "i" if is_shift && self.active_panel == ActiveWorkspacePanel::Browser => {
                if let Some(ref bw) = self.browser_workspace {
                    bw.update(cx, |w, wcx| w.toggle_design_mode(wcx));
                }
                true
            }
            "i" => {
                if self.active_panel == ActiveWorkspacePanel::Chats {
                    let next_open = !self.inspector.read(cx).is_open;
                    self.set_inspector_open(next_open, cx);
                }
                true
            }
            "b" if is_shift => {
                if self.active_panel == ActiveWorkspacePanel::Browser {
                    self.activate_nav_destination(NavDestination::Chats, cx);
                } else {
                    self.open_browser_workspace(cx);
                }
                true
            }
            "c" if is_shift => {
                if self.active_panel == ActiveWorkspacePanel::Community {
                    self.activate_nav_destination(NavDestination::Chats, cx);
                } else {
                    self.open_community_workspace(cx);
                }
                true
            }
            "b" | "\\" => {
                if matches!(
                    self.active_panel,
                    ActiveWorkspacePanel::Chats
                        | ActiveWorkspacePanel::Browser
                        | ActiveWorkspacePanel::Community
                        | ActiveWorkspacePanel::OrcaBots
                        | ActiveWorkspacePanel::OrcaIde
                ) {
                    self.sidebar.update(cx, |sb, sb_cx| {
                        sb.toggle_collapsed_from_user();
                        sb_cx.notify();
                    });
                }
                true
            }
            "e" => {
                self.clear_ime_and_blur_content_hub_for_transition(true, cx);
                self.toggle_account_popover(cx);
                if self.sidebar.read(cx).is_account_popover_open {
                    self.blur_composer_and_cancel_ime(cx);
                    self.blur_community_if_focused(cx);
                }
                cx.notify();
                true
            }
            "j" if is_shift => {
                self.flip_chat_terminal_session(cx);
                true
            }
            "j" => {
                self.toggle_scoped_jump_palette(cx);
                true
            }
            "n" => {
                self.cleanup_settings_panel_if_active(cx);
                self.leave_browser_panel_if_active(cx);
                self.leave_community_panel_if_active(cx);
                self.active_panel = ActiveWorkspacePanel::Chats;
                self.last_handled_nav_dest = Some(NavDestination::Chats);
                let nav_trigger = self.nav_rail.update(cx, |r, r_cx| {
                    r.set_active_destination(NavDestination::Chats);
                    r_cx.notify();
                    r.trigger_counter
                });
                self.last_handled_nav_trigger = nav_trigger;
                self.sync_apple_sidebar_to_active_panel(cx);
                self.clear_ime_and_blur_content_hub_for_transition(true, cx);
                self.bot_browser.ime_preedit_input_gen = None;
                self.community_ime_preedit_input_gen = None;
                if self.theme_state.palette.is_apple_style() {
                    self.enter_compose_or_modal_isolation(cx);
                    self.sidebar.update(cx, |sb, sb_cx| {
                        if is_shift {
                            sb.open_compose_group_mode();
                        } else if !sb.is_plus_menu_open {
                            sb.toggle_plus_menu();
                        } else if sb.compose_draft.pending_creation.is_none() {
                            sb.compose_recipient_query.clear();
                            sb.compose_draft.menu_open = true;
                            sb.compose_draft.highlighted_index = 0;
                            sb.compose_title_focused = false;
                        }
                        sb_cx.notify();
                    });
                    self.blur_composer_and_cancel_ime(cx);
                    self.sync_compose_state_to_composer(true, cx);
                } else if is_shift {
                    self.open_modal_dialog(
                        tr_str(lang, "新建群组", "New Group"),
                        ModalDialogKind::NewGroup,
                        420.0,
                        380.0,
                        cx,
                    );
                } else {
                    // Classic renders `render_plus_dropdown` in the sidebar without hiding/isolating Composer.
                    self.sidebar.update(cx, |sb, sb_cx| {
                        sb.toggle_plus_menu();
                        sb_cx.notify();
                    });
                }
                cx.notify();
                true
            }
            "p" => {
                self.open_modal_dialog(
                    tr_str(lang, "创建投票", "Create Poll"),
                    ModalDialogKind::CreatePoll,
                    420.0,
                    480.0,
                    cx,
                );
                true
            }
            digit => {
                if let Some(dest) = nav_destination_for_shortcut_digit(digit) {
                    self.activate_nav_destination(dest, cx);
                    true
                } else {
                    false
                }
            }
        }
    }

    fn try_route_interactive_pty_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.is_authenticated || self.overlays.is_passcode_locked {
            return false;
        }
        let Some(ov) = self
            .overlays
            .computer_overlay
            .as_ref()
            .filter(|ov| ov.read(cx).is_interactive_pty_focused(cx))
            .cloned()
        else {
            return false;
        };
        let m = event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        // Forward Control combinations (Ctrl+C, Ctrl+D, etc.), Cmd+C selection copy,
        // and non-printable navigation/control keys to RemoteTerminalView.
        let is_ctrl_combo = m.control && !m.platform;
        let is_cmd_copy = m.platform && key.eq_ignore_ascii_case("c");
        if is_ctrl_combo || is_cmd_copy {
            return ov.update(cx, |wb, wcx| {
                wb.handle_terminal_key_stroke(
                    key,
                    m.shift,
                    m.control,
                    m.alt,
                    m.platform,
                    event.keystroke.key_char.as_deref(),
                    wcx,
                )
            });
        }
        false
    }

    /// Handles window-level key down events without swallowing printable IME character input.
    pub fn handle_window_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.try_route_interactive_pty_key(event, cx) {
            cx.stop_propagation();
            return;
        }
        let key = event.keystroke.key.as_str();

        // 1. Handle Cmd / Ctrl shortcuts (Theme, Language, Navigation, Modals, Search, Paste)
        if event.keystroke.modifiers.platform || event.keystroke.modifiers.control {
            if self.handle_shortcut_key(key, event.keystroke.modifiers.shift, cx) {
                cx.stop_propagation();
            }
            return;
        }

        if self.is_browser_cdp_viewport_focused(window, cx) {
            cx.stop_propagation();
            return;
        }

        // 2. Handle non-character control keys (Enter, Backspace/Alt+Backspace, Tab, Escape)
        if self.handle_non_char_key_with_alt(
            key,
            event.keystroke.modifiers.shift,
            event.keystroke.modifiers.alt,
            cx,
        ) {
            cx.stop_propagation();
        }
        // 3. Printable characters are NOT stopped here so GPUI's macOS NSTextInputClient
        //    routes both English keys (`insertText:`) and Chinese Pinyin (`setMarkedText:`)
        //    through `EntityInputHandler` without duplication.
    }

    /// Dispatches a synthetic or view-local `KeyDownEvent` (used by tests and fallback key handlers).
    pub fn handle_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.try_route_interactive_pty_key(event, cx) {
            return;
        }
        let key = event.keystroke.key.as_str();

        if event.keystroke.modifiers.platform || event.keystroke.modifiers.control {
            self.handle_shortcut_key(key, event.keystroke.modifiers.shift, cx);
            return;
        }

        if self.handle_non_char_key_with_alt(
            key,
            event.keystroke.modifiers.shift,
            event.keystroke.modifiers.alt,
            cx,
        ) {
            return;
        }

        if key == "space" {
            self.insert_committed_text(" ", cx);
        } else if let Some(ref ch) = event.keystroke.key_char {
            self.insert_committed_text(ch, cx);
        }
    }
}
