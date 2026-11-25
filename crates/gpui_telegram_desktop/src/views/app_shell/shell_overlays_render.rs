//! Overlay and workspace modal rendering helpers for `AppShell`.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::computer_contracts::{PendingComputerCommandDto, RemoteComputerOp};
use gpui_telegram_proto::contracts::chats::CreateChatRequest;
use gpui_telegram_proto::miniapp_contracts::{
    MiniAppControlMode, MiniAppTakeoverEvent, GROK_AVATAR_SHAPES,
};

use crate::app::session::send_as_peers::SendAsKey;
use crate::components::grok_bot_avatar::{
    render_grok_bot_avatar, trigger_grok_avatar_bounce, trigger_grok_avatar_spin,
    GrokBotAvatarProps, GrokPersonaState, GROK_COLOR_PALETTE,
};
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::shell_actions::localize_call_state;
use crate::views::app_shell::shell_panels::{render_empty_panel_notice, render_panel_header};
use crate::views::app_shell::shell_truth::{
    derive_initials, format_authoritative_timestamp, peer_subtitle_for_dialog, ClientCallFence,
};
use crate::views::app_shell::modal_container::{
    macos_modal_backdrop_rgba, macos_modal_card, macos_popover_surface, macos_primary_pill_btn,
    macos_secondary_pill_btn,
};
use crate::views::app_shell::peer_avatar_color;
use crate::views::boxes::modal_types::ModalDialogKind;
use crate::views::chat_sidebar::{
    AddContactKind, ComposeRecipientChip, CreationInputField, DialogDeliveryStatus,
    DialogItemModel, PendingComposeCreation, SidebarCreationModal,
};
use crate::views::drawer::inspector_panels::{segmented_active_bg, segmented_pill_shadow};
use crate::views::nav_rail::NavDestination;
use crate::views::settings::sections::SettingsVaultAction;
use crate::views::settings::SettingsSectionType;
use crate::views::shell::{ActiveWorkspacePanel, AppShell};
use crate::views::tdesktop_theme::tr_str;
use crate::views::{CallOverlay, ComputerWorkbench, IvViewer, MainMenuCommand, MediaViewer};

fn fullscreen_occluded_layer(child: impl IntoElement) -> AnyElement {
    div().absolute().top_0().left_0().size_full().occlude().child(child).into_any_element()
}

fn modal_backdrop_shell(id: &'static str, bg_color: Rgba) -> Stateful<Div> {
    div().id(id).absolute().top_0().left_0().size_full().occlude().bg(bg_color).flex().items_center().justify_center()
}

fn talk_pill(bg: u32, border: Option<Rgba>, fg: u32, label: &'static str) -> Div {
    let base = div().cursor_pointer().px(px(13.0)).py(px(6.5)).rounded(px(999.0)).bg(rgb(bg)).text_size(px(12.5)).text_color(rgb(fg)).child(label);
    if let Some(b) = border { base.border_1().border_color(b) } else { base.font_weight(FontWeight::SEMIBOLD) }
}

fn format_live_miniapp_telemetry(
    lang: &str,
    state_snap: Option<&serde_json::Value>,
    is_ai: bool,
    active_tool: Option<&str>,
    status_text: &str,
) -> String {
    if is_ai {
        if let Some(t) = active_tool.map(str::trim).filter(|s| !s.is_empty()) {
            return format!(
                "{}: {t}",
                tr_str(lang, "AI 正在操控 WebMCP", "AI running WebMCP")
            );
        }
    }
    if let Some(snap) = state_snap {
        if let Some(reported) = snap
            .get("status_text")
            .or_else(|| snap.get("pip_status_text"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return reported.to_string();
        }
    }
    if !status_text.trim().is_empty() {
        return status_text.to_string();
    }
    if let Some(title) = state_snap
        .and_then(|s| s.get("title"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return title.to_string();
    }
    String::new()
}

impl AppShell {
    fn close_workspace_modal_to_chats(&mut self, cx: &mut Context<Self>) {
        self.leave_browser_panel_if_active(cx);
        self.leave_community_panel_if_active(cx);
        self.active_panel = ActiveWorkspacePanel::Chats;
        self.last_handled_nav_dest = Some(NavDestination::Chats);
        self.nav_rail.update(cx, |r, _| {
            r.set_active_destination(NavDestination::Chats);
        });
        cx.notify();
    }

    fn render_workspace_modal_shell(
        backdrop_id: &'static str,
        box_id: &'static str,
        width_px: f32,
        height_px: f32,
        pal: TdesktopThemePalette,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let w = pal.scaled_px(width_px).min(1080.0);
        let h = pal.scaled_px(height_px).min(820.0);
        modal_backdrop_shell(backdrop_id, macos_modal_backdrop_rgba(pal))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_workspace_modal_to_chats(cx)))
            .child(
                macos_modal_card(box_id, pal)
                    .w(px(w))
                    .h(px(h))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(body),
            )
            .into_any_element()
    }

    /// Renders the centered, responsive `ComputerWorkbench` overlay layer.
    pub fn render_remote_computer_overlay(&self, overlay: Entity<ComputerWorkbench>) -> AnyElement {
        modal_backdrop_shell(
            "computer-workbench-backdrop",
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.56),
        )
        .p(px(20.0))
        .child(overlay)
        .into_any_element()
    }

    /// Renders the Talk Mode / WebRTC call overlay plus AI voice controls when active.
    pub fn render_talk_mode_overlay(&self, overlay: Entity<CallOverlay>, cx: &mut Context<Self>) -> AnyElement {
        let is_ai_talk = self.rtc_talk.active_talk_session_is_ai;
        let is_rec = self.rtc_talk.voice_recorder.is_recording();
        let talk_lang = self.active_lang_id();
        let talk_pal = self.theme_state.palette;

        div().absolute().top_0().left_0().size_full().occlude().bg(macos_modal_backdrop_rgba(talk_pal))
            .flex().flex_col().items_center().justify_center().gap(px(12.0))
            .child(overlay)
            .when(is_ai_talk, |wrap| {
                let ptt_lbl = if is_rec { tr_str(talk_lang, "停止并发送语音", "Stop & Send Voice") } else { tr_str(talk_lang, "按住/点击说话", "Push to Talk") };
                wrap.child(
                    div().flex().items_center().gap(px(10.0)).px(px(16.0)).py(px(10.0)).rounded(px(14.0))
                        .bg(rgb(talk_pal.modal_bg)).border_1().border_color(talk_pal.separator_rgba())
                        .shadow(talk_pal.popover_shadows())
                        .child(
                            talk_pill(if is_rec { talk_pal.danger } else { talk_pal.accent }, None, 0xFFFFFF, ptt_lbl)
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                    let next = !this.rtc_talk.voice_recorder.is_recording();
                                    this.toggle_voice_recording(next, cx);
                                })),
                        )
                        .child(
                            talk_pill(talk_pal.input_bg, Some(talk_pal.separator_rgba()), talk_pal.text_primary, tr_str(talk_lang, "发送对话回合", "Send Voice Turn"))
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                    let draft = this.composer.read(cx).text().trim().to_string();
                                    let prompt = if !draft.is_empty() {
                                        this.composer.update(cx, |c, _| c.set_text(String::new()));
                                        draft
                                    } else {
                                        tr_str(this.active_lang_id(), "请用简洁的语音总结当前状态", "Give me a concise spoken briefing of my current status.").to_string()
                                    };
                                    this.send_ai_talk_voice_turn(prompt, true, cx);
                                })),
                        )
                        .child(
                            talk_pill(talk_pal.input_bg, Some(TdesktopThemePalette::rgba_with_alpha(talk_pal.danger, 0.50)), talk_pal.danger, tr_str(talk_lang, "打断播报 (<60ms)", "Barge-In (<60ms)"))
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.interrupt_ai_talk_barge_in(cx))),
                        ),
                )
            })
            .into_any_element()
    }

    /// Renders the fullscreen `MediaViewer` overlay layer.
    pub fn render_media_viewer_overlay(&self, overlay: Entity<MediaViewer>) -> AnyElement {
        fullscreen_occluded_layer(overlay)
    }

    /// Renders the fullscreen `IvViewer` overlay layer.
    pub fn render_iv_viewer_overlay(&self, overlay: Entity<IvViewer>) -> AnyElement {
        fullscreen_occluded_layer(overlay)
    }

    /// Returns `true` when any modal, overlay, non-Chats workspace panel, or sidebar modal
    /// sits above the Mini App overlay and therefore occludes its native `WKWebView`.
    pub fn has_miniapp_top_modal(&self, cx: &App) -> bool {
        let sidebar_modal_open = {
            let sb = self.sidebar.read(cx);
            sb.is_contacts_modal_open
                || sb.active_creation_modal.is_some()
                || sb.is_account_popover_open
        };
        self.miniapps.pending_miniapp_confirm.is_some()
            || self.miniapps.miniapp_tos_modal_open
            || self
                .miniapps
                .bot_profile_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
            || self
                .miniapps
                .create_bot_box
                .as_ref()
                .is_some_and(|b| b.read(cx).is_visible)
            || self.rtc_talk.talk_mode_overlay.is_some()
            || self.overlays.media_viewer_overlay.is_some()
            || self.overlays.iv_viewer_overlay.is_some()
            || self.overlays.computer_overlay.is_some()
            || self.overlays.is_passcode_locked
            || self.active_panel != ActiveWorkspacePanel::Chats
            || sidebar_modal_open
            || self.modal_container.read(cx).is_modal_active()
    }

    /// Renders the minimized MiniApp pill, open MiniApp host sheet, confirmation/ToS modals,
    /// `BotProfileBox`, and NL Bot creator modal.
    pub fn render_miniapp_overlays(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut layers = Vec::new();
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();

        let has_top_modal = self.has_miniapp_top_modal(cx);

        let mut is_live_pip = false;
        let mut pip_live_enabled = true;
        if let Some(ref ov) = self.miniapps.miniapp_overlay {
            let ov_read = ov.read(cx);
            let is_bg_pip = ov_read.is_floating_pip && !ov_read.is_open;
            pip_live_enabled = ov_read.pip_live_preview;
            is_live_pip = is_bg_pip
                && ov_read.pip_live_preview
                && !has_top_modal
                && ov_read.disclaimer_accepted;
            let should_hide_native =
                (!ov_read.is_open && !is_bg_pip) || (ov_read.is_open && has_top_modal);
            let needs_park_pip = is_bg_pip && !is_live_pip && ov_read.disclaimer_accepted;
            let needs_sync = ov_read.external_modal_occluding != has_top_modal
                || (should_hide_native && ov_read.is_native_webview_visible());
            let _ = ov_read;
            if needs_sync || needs_park_pip || is_live_pip {
                ov.update(cx, |w, _| {
                    w.external_modal_occluding = has_top_modal;
                    if should_hide_native {
                        w.hide_native_webview();
                    } else if needs_park_pip {
                        w.park_native_webview_for_pip();
                    } else if is_live_pip {
                        w.set_native_webview_allow_focus(false);
                    }
                });
            }
            if is_bg_pip && !is_live_pip {
                layers.push(
                    div()
                        .id("miniapp-pip-bg-mount")
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(px(1.0))
                        .h(px(1.0))
                        .opacity(0.01)
                        .overflow_hidden()
                        .child(ov.clone())
                        .into_any_element(),
                );
            }
        }

        let overlay_open = self
            .miniapps
            .miniapp_overlay
            .as_ref()
            .is_some_and(|ov| ov.read(cx).is_open);
        let overlay_floating_pip = self
            .miniapps
            .miniapp_overlay
            .as_ref()
            .is_some_and(|ov| ov.read(cx).is_floating_pip && !ov.read(cx).is_open);
        let pip_active = !overlay_open
            && (self.miniapps.is_miniapp_minimized
                || overlay_floating_pip
                || self
                    .miniapps
                    .floating_pip_status
                    .as_ref()
                    .is_some_and(|s| s.mode != MiniAppControlMode::Idle));

        if pip_active {
            let pip_from_overlay = self.miniapps.miniapp_overlay.as_ref().map(|ov| {
                let w = ov.read(cx);
                let title = if !w.ui_schema.title.trim().is_empty() {
                    w.ui_schema.title.clone()
                } else if let Some(ref b) = w.active_bot {
                    b.name.clone()
                } else {
                    "Mini App".to_string()
                };
                (
                    title,
                    w.bot_username.trim_start_matches('@').to_string(),
                    w.current_short_name(),
                    w.target_chat_i64().unwrap_or(0),
                    w.bot_id,
                    w.control_mode,
                    w.control_status_text.clone(),
                    w.active_webmcp_tool.clone(),
                    w.last_webmcp_state_summary.clone(),
                )
            });
            let (
                title,
                raw_bot_un,
                short_name,
                chat_id,
                bot_uid,
                ctrl_mode,
                status_text,
                active_tool,
                state_snap,
            ) = if let Some(ref ps) = self.miniapps.floating_pip_status {
                let title = if !ps.app_title.trim().is_empty() {
                    ps.app_title.clone()
                } else if let Some(ref o) = pip_from_overlay {
                    o.0.clone()
                } else {
                    ps.short_name.clone()
                };
                (
                    title,
                    ps.bot_username.trim_start_matches('@').to_string(),
                    ps.short_name.clone(),
                    ps.chat_id,
                    ps.bot_user_id,
                    ps.mode,
                    ps.status_text.clone(),
                    ps.active_tool.clone(),
                    pip_from_overlay.and_then(|o| o.8),
                )
            } else if let Some(o) = pip_from_overlay {
                o
            } else {
                (
                    "Mini App".to_string(),
                    String::new(),
                    String::new(),
                    0,
                    0,
                    MiniAppControlMode::Idle,
                    String::new(),
                    None,
                    None,
                )
            };

            let bot_un_display = if !raw_bot_un.is_empty() {
                format!("@{raw_bot_un}")
            } else {
                String::new()
            };
            let is_takeover = ctrl_mode == MiniAppControlMode::UserTakeover;
            let is_waiting = ctrl_mode == MiniAppControlMode::WaitingForTakeover;
            let is_ai = ctrl_mode == MiniAppControlMode::AiControlling;
            let badge_label = match ctrl_mode {
                MiniAppControlMode::Idle => {
                    if is_live_pip {
                        tr_str(lang, "LIVE · 实时直播", "LIVE")
                    } else {
                        tr_str(lang, "运行中", "Active")
                    }
                }
                MiniAppControlMode::AiControlling => {
                    if is_live_pip {
                        tr_str(lang, "LIVE · AI 操控中", "LIVE · AI")
                    } else {
                        tr_str(lang, "AI 操控中", "AI Controlling")
                    }
                }
                MiniAppControlMode::WaitingForTakeover => tr_str(lang, "等待接管", "Needs Takeover"),
                MiniAppControlMode::UserTakeover => tr_str(lang, "手动接管中", "User Takeover"),
            };
            let badge_color = if is_takeover {
                0xD97706
            } else if is_waiting {
                0xDC2626
            } else if is_live_pip {
                0xEF4444
            } else {
                pal.accent
            };
            let detail_line = format_live_miniapp_telemetry(
                lang,
                state_snap.as_ref(),
                is_ai,
                active_tool.as_deref(),
                &status_text,
            );
            let live_mount_overlay = if is_live_pip {
                self.miniapps.miniapp_overlay.clone()
            } else {
                None
            };
            let has_toggle_overlay = self.miniapps.miniapp_overlay.is_some();

            let b_un_take = raw_bot_un.clone();
            let s_name_take = short_name.clone();
            let snap_finish = state_snap.clone();
            let b_un_fin = raw_bot_un.clone();
            let s_name_fin = short_name.clone();
            let b_un_close = raw_bot_un.clone();
            let s_name_close = short_name.clone();
            let snap_close = state_snap;

            layers.push(
                div()
                    .id("minimized-miniapp-pill")
                    .absolute()
                    .bottom(px(82.0))
                    .right(px(20.0))
                    .w(px(if is_live_pip { 340.0 } else { 300.0 }))
                    .px(px(12.0))
                    .py(px(10.0))
                    .rounded(px(14.0))
                    .bg(rgb(pal.modal_bg))
                    .border_1()
                    .border_color(if is_takeover || is_waiting || is_live_pip {
                        TdesktopThemePalette::rgba_with_alpha(badge_color, 0.55)
                    } else {
                        pal.separator_rgba()
                    })
                    .shadow(pal.popover_shadows())
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .id("miniapp-floating-pip-card")
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .min_w(px(0.0))
                                    .flex_1()
                                    .child(tg_icon(IconName::BotRobot, 17.0, badge_color))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .min_w(px(0.0))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.0))
                                                    .when(is_live_pip, |r| {
                                                        r.child(
                                                            div()
                                                                .w(px(7.0))
                                                                .h(px(7.0))
                                                                .rounded_full()
                                                                .bg(rgb(0xEF4444)),
                                                        )
                                                    })
                                                    .child(
                                                        div()
                                                            .text_size(px(13.0))
                                                            .font_weight(FontWeight::BOLD)
                                                            .text_color(rgb(pal.text_primary))
                                                            .child(title),
                                                    )
                                                    .child(
                                                        div()
                                                            .px(px(6.0))
                                                            .py(px(1.5))
                                                            .rounded(px(999.0))
                                                            .bg(rgb(badge_color))
                                                            .text_size(px(10.0))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(rgb(0xFFFFFF))
                                                            .child(badge_label),
                                                    ),
                                            )
                                            .when(!bot_un_display.is_empty(), |col| {
                                                col.child(
                                                    div()
                                                        .text_size(px(11.0))
                                                        .text_color(rgb(pal.text_link))
                                                        .child(bot_un_display),
                                                )
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .when(has_toggle_overlay, |row| {
                                        row.child(
                                            div()
                                                .id("minimized-miniapp-toggle-live-btn")
                                                .cursor_pointer()
                                                .px(px(7.0))
                                                .py(px(3.0))
                                                .rounded(px(7.0))
                                                .bg(rgb(pal.card_bg))
                                                .hover(move |s| s.bg(rgb(pal.hover_bg)))
                                                .text_size(px(11.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(pal.text_secondary))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        if let Some(ref ov) =
                                                            this.miniapps.miniapp_overlay
                                                        {
                                                            ov.update(cx, |w, wcx| {
                                                                w.is_floating_pip = true;
                                                                w.pip_live_preview =
                                                                    !w.pip_live_preview;
                                                                w.set_native_webview_allow_focus(
                                                                    false,
                                                                );
                                                                if !w.pip_live_preview {
                                                                    w.park_native_webview_for_pip();
                                                                }
                                                                wcx.notify();
                                                            });
                                                        }
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(if pip_live_enabled {
                                                    tr_str(lang, "收起", "Collapse")
                                                } else {
                                                    tr_str(lang, "直播", "Live")
                                                }),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id("minimized-miniapp-close-btn")
                                            .cursor_pointer()
                                            .w(px(22.0))
                                            .h(px(22.0))
                                            .rounded_full()
                                            .hover(move |s| s.bg(rgb(pal.hover_bg)))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(12.0))
                                            .text_color(rgb(pal.text_secondary))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    if this.is_authenticated
                                                        && is_takeover
                                                        && !b_un_close.is_empty()
                                                        && !s_name_close.is_empty()
                                                    {
                                                        this.client.send_miniapp_takeover_state(
                                                            MiniAppTakeoverEvent {
                                                                bot_username: b_un_close.clone(),
                                                                short_name: s_name_close.clone(),
                                                                chat_id,
                                                                bot_user_id: bot_uid as i64,
                                                                mode: MiniAppControlMode::Idle,
                                                                user_note: None,
                                                                state_snapshot: snap_close.clone(),
                                                            },
                                                        );
                                                    }
                                                    this.miniapps.bump_launch_generation();
                                                    this.miniapps.is_miniapp_minimized = false;
                                                    this.miniapps.floating_pip_status = None;
                                                    if let Some(ov) =
                                                        this.miniapps.miniapp_overlay.take()
                                                    {
                                                        ov.update(cx, |w, _| w.close());
                                                    }
                                                    cx.notify();
                                                }),
                                            )
                                            .child("×"),
                                    ),
                            ),
                    )
                    .when_some(live_mount_overlay, |card, ov_live| {
                        card.child(
                            div()
                                .id("miniapp-pip-live-viewport")
                                .w_full()
                                .h(px(360.0))
                                .rounded(px(12.0))
                                .overflow_hidden()
                                .bg(rgb(0x0B0E14))
                                .border_1()
                                .border_color(rgb(pal.border_fg))
                                .child(ov_live),
                        )
                    })
                    .when(!detail_line.trim().is_empty(), |card| {
                        card.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(detail_line),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(6.0))
                            .when(is_ai || is_waiting, |row| {
                                row.child(
                                    div()
                                        .id("pip-takeover-btn")
                                        .cursor_pointer()
                                        .px(px(9.0))
                                        .py(px(4.0))
                                        .rounded(px(8.0))
                                        .bg(rgb(0xF59E0B))
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.occlude_browser_for_overlay(cx);
                                                this.miniapps.is_miniapp_minimized = false;
                                                if let Some(ref ov) = this.miniapps.miniapp_overlay {
                                                    ov.update(cx, |w, wcx| {
                                                        w.is_open = true;
                                                        w.is_floating_pip = false;
                                                        w.control_mode =
                                                            MiniAppControlMode::UserTakeover;
                                                        w.control_status_text =
                                                            "手动接管中".to_string();
                                                        wcx.notify();
                                                    });
                                                }
                                                this.reconcile_chat_scoped_toasts(cx);
                                                if let Some(ref mut ps) =
                                                    this.miniapps.floating_pip_status
                                                {
                                                    ps.mode = MiniAppControlMode::UserTakeover;
                                                    ps.status_text =
                                                        "用户正在手动接管小程序".to_string();
                                                    ps.active_tool = None;
                                                }
                                                if this.is_authenticated
                                                    && !b_un_take.is_empty()
                                                    && !s_name_take.is_empty()
                                                {
                                                    this.client.send_miniapp_takeover_state(
                                                        MiniAppTakeoverEvent {
                                                            bot_username: b_un_take.clone(),
                                                            short_name: s_name_take.clone(),
                                                            chat_id,
                                                            bot_user_id: bot_uid as i64,
                                                            mode: MiniAppControlMode::UserTakeover,
                                                            user_note: None,
                                                            state_snapshot: None,
                                                        },
                                                    );
                                                }
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "✋ 接管控制", "Take Over")),
                                )
                            })
                            .when(is_takeover, |row| {
                                row.child(
                                    div()
                                        .id("pip-finish-takeover-btn")
                                        .cursor_pointer()
                                        .px(px(9.0))
                                        .py(px(4.0))
                                        .rounded(px(8.0))
                                        .bg(rgb(0x16A34A))
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                if let Some(ref ov) = this.miniapps.miniapp_overlay {
                                                    ov.update(cx, |w, wcx| {
                                                        w.control_mode =
                                                            MiniAppControlMode::AiControlling;
                                                        w.control_status_text =
                                                            "已结束接管，AI 继续执行".to_string();
                                                        wcx.notify();
                                                    });
                                                }
                                                if let Some(ref mut ps) =
                                                    this.miniapps.floating_pip_status
                                                {
                                                    ps.mode = MiniAppControlMode::AiControlling;
                                                    ps.status_text =
                                                        "已交还控制权，AI 继续中".to_string();
                                                }
                                                if this.is_authenticated
                                                    && !b_un_fin.is_empty()
                                                    && !s_name_fin.is_empty()
                                                {
                                                    this.client.send_miniapp_takeover_state(
                                                        MiniAppTakeoverEvent {
                                                            bot_username: b_un_fin.clone(),
                                                            short_name: s_name_fin.clone(),
                                                            chat_id,
                                                            bot_user_id: bot_uid as i64,
                                                            mode: MiniAppControlMode::Idle,
                                                            user_note: None,
                                                            state_snapshot: snap_finish.clone(),
                                                        },
                                                    );
                                                }
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "✅ 结束接管，AI 继续", "Finish & Resume")),
                                )
                            })
                            .child(
                                div()
                                    .id("pip-expand-btn")
                                    .cursor_pointer()
                                    .px(px(9.0))
                                    .py(px(4.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(pal.accent))
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0xFFFFFF))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.occlude_browser_for_overlay(cx);
                                            this.miniapps.is_miniapp_minimized = false;
                                            if let Some(ref ov) = this.miniapps.miniapp_overlay {
                                                ov.update(cx, |w, wcx| {
                                                    w.is_open = true;
                                                    w.is_floating_pip = false;
                                                    wcx.notify();
                                                });
                                            }
                                            this.reconcile_chat_scoped_toasts(cx);
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "展开", "Restore")),
                            ),
                    )
                    .into_any_element(),
            );
        }
        if let Some(overlay) = self.miniapps.miniapp_overlay.clone().filter(|ov| ov.read(cx).is_open) {
            layers.push(
                div()
                    .id("miniapp-overlay-host")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .child(fullscreen_occluded_layer(overlay))
                    .into_any_element(),
            );
        }
        if let Some(box_ent) = self.miniapps.create_bot_box.clone().filter(|b| b.read(cx).is_visible) {
            layers.push(
                modal_backdrop_shell("create-managed-bot-backdrop", macos_modal_backdrop_rgba(pal))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.miniapps.create_bot_box = None;
                        if !this.is_compose_or_creation_modal_active(cx) {
                            this.exit_compose_or_modal_isolation(true, cx);
                        }
                        cx.notify();
                    }))
                    .child(div().on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation())).child(box_ent))
                    .into_any_element(),
            );
        }
        if let Some(ref confirm) = self.miniapps.pending_miniapp_confirm {
            let bot_name = confirm.bot_display_name.clone();
            let bot_un = confirm.bot_username.clone();
            let bot_sub = if bot_un.is_empty() {
                String::new()
            } else {
                format!("@{}", bot_un.trim_start_matches('@'))
            };
            let bot_un_for_profile = bot_un.clone();
            let app_title = confirm.app_title.clone();
            let avatar_col = peer_avatar_color(confirm.bot_id.max(1));
            let init = {
                let i = derive_initials(&bot_name);
                if i.is_empty() { "BT".to_string() } else { i }
            };
            layers.push(
                modal_backdrop_shell("miniapp-launch-confirm-backdrop", macos_modal_backdrop_rgba(pal))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.miniapps.pending_miniapp_confirm = None;
                            cx.notify();
                        }),
                    )
                    .child(
                        macos_modal_card("miniapp-launch-confirm-modal", pal)
                            .w(px(368.0))
                            .p(px(22.0))
                            .flex()
                            .flex_col()
                            .gap(px(14.0))
                            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.0))
                                    .child(
                                        div()
                                            .w(px(48.0))
                                            .h(px(48.0))
                                            .rounded_full()
                                            .bg(rgb(avatar_col))
                                            .border_1()
                                            .border_color(pal.separator_rgba())
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(17.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0xFFFFFF))
                                            .child(init),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.0))
                                            .child(
                                                div()
                                                    .text_size(px(15.5))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(rgb(pal.text_primary))
                                                    .child(bot_name),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .text_color(rgb(pal.text_secondary))
                                                    .child(if !app_title.is_empty() && !bot_sub.is_empty() {
                                                        format!("{} · {}", app_title, bot_sub)
                                                    } else {
                                                        bot_sub
                                                    }),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .id("miniapp-confirm-more-info-btn")
                                    .cursor_pointer()
                                    .px(px(12.0))
                                    .py(px(9.0))
                                    .rounded(px(10.0))
                                    .bg(pal.card_surface_rgba())
                                    .border_1()
                                    .border_color(pal.separator_rgba())
                                    .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(pal.accent))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.open_bot_profile_popup(&bot_un_for_profile, cx);
                                        }),
                                    )
                                    .child(tr_str(
                                        lang,
                                        "本机器人的更多信息",
                                        "More information about this bot",
                                    ))
                                    .child(tg_icon(IconName::ChevronRight, 13.0, pal.accent)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(4.0))
                                    .text_size(px(12.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(
                                        lang,
                                        "启动此小程序即表示您同意",
                                        "By launching this mini app, you agree to the",
                                    ))
                                    .child(
                                        div()
                                            .id("miniapp-confirm-tos-link")
                                            .cursor_pointer()
                                            .text_color(rgb(pal.accent))
                                            .font_weight(FontWeight::MEDIUM)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.occlude_browser_for_overlay(cx);
                                                    this.miniapps.miniapp_tos_modal_open = true;
                                                    this.reconcile_chat_scoped_toasts(cx);
                                                    cx.notify();
                                                }),
                                            )
                                            .child(tr_str(
                                                lang,
                                                "小程序服务条款。",
                                                "Terms of Service for Mini Apps.",
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .mt(px(4.0))
                                    .flex()
                                    .justify_end()
                                    .gap(px(8.0))
                                    .child(
                                        macos_secondary_pill_btn(
                                            "miniapp-confirm-cancel-btn",
                                            tr_str(lang, "取消", "Cancel"),
                                            pal,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.miniapps.pending_miniapp_confirm = None;
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(
                                        macos_primary_pill_btn(
                                            "miniapp-confirm-open-btn",
                                            tr_str(lang, "打开应用", "Launch"),
                                            pal,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.confirm_pending_miniapp_launch(cx);
                                            }),
                                        ),
                                    ),
                            ),
                    )
                    .into_any_element(),
            );
        }
        if let Some(box_ent) = self.miniapps.bot_profile_box.clone().filter(|b| b.read(cx).is_visible) {
            layers.push(
                modal_backdrop_shell("bot-profile-box-backdrop", macos_modal_backdrop_rgba(pal))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if let Some(ref ent) = this.miniapps.bot_profile_box {
                                ent.update(cx, |b, b_cx| {
                                    b.close();
                                    b_cx.notify();
                                });
                            }
                            this.miniapps.bot_profile_box = None;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
                            .child(box_ent),
                    )
                    .into_any_element(),
            );
        }
        if self.miniapps.miniapp_tos_modal_open {
            layers.push(
                modal_backdrop_shell("miniapp-tos-modal-backdrop", macos_modal_backdrop_rgba(pal))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.miniapps.miniapp_tos_modal_open = false;
                            cx.notify();
                        }),
                    )
                    .child(
                        macos_modal_card("miniapp-tos-modal", pal)
                            .w(px(396.0))
                            .p(px(22.0))
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(tr_str(
                                        lang,
                                        "小程序服务条款",
                                        "Terms of Service for Mini Apps",
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(
                                        lang,
                                        "小程序由第三方机器人开发者独立提供和运营。启动小程序时，应用将接收您的基础公开身份标识与主题配色参数；仅在您主动启用对话或明确点击「允许发送消息」后，该机器人才能向您发送消息。",
                                        "Mini Apps are built and operated independently by third-party bot developers. When launched, the Mini App receives your signed basic profile identifier and theme parameters. Bots can only send you messages if you start the chat or explicitly grant write access.",
                                    )),
                            )
                            .child(
                                div()
                                    .mt(px(6.0))
                                    .flex()
                                    .justify_end()
                                    .child(
                                        macos_primary_pill_btn(
                                            "miniapp-tos-close-btn",
                                            tr_str(lang, "我知道了", "OK"),
                                            pal,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.miniapps.miniapp_tos_modal_open = false;
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            ),
                    )
                    .into_any_element(),
            );
        }
        layers
    }

    /// Renders the searchable Contacts modal backed by `session_sync.contacts_list` and sidebar dialogs.
    pub fn render_contacts_modal_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();
        let sb_ref = self.sidebar.read(cx);
        let contact_q = sb_ref.contacts_search_query.clone();
        let q_lower = contact_q.trim().to_lowercase();
        let mut rows: Vec<(u64, Option<u64>, String, String, String, u32)> = Vec::new();

        for c in &self.session_sync.contacts_list {
            let full = format!("{} {}", c.first_name, c.last_name.as_deref().unwrap_or("")).trim().to_string();
            let name = if full.is_empty() { c.username.clone().unwrap_or_else(|| format!("User {}", c.user_id)) } else { full };
            let sub = c.username.as_deref().map(|u| format!("@{u}")).or_else(|| c.phone.clone()).unwrap_or_default();
            if q_lower.is_empty() || name.to_lowercase().contains(&q_lower) || sub.to_lowercase().contains(&q_lower) {
                let resolved_chat = c.chat_id.map(|id| id as u64).or_else(|| {
                    sb_ref.dialogs.iter().find(|d| d.id == c.user_id || d.peer_name == name).map(|d| d.id)
                });
                rows.push((c.user_id, resolved_chat, derive_initials(&name), name, sub, peer_avatar_color(c.user_id)));
            }
        }
        for d in &sb_ref.dialogs {
            if d.folder != "Groups"
                && !d.peer_name.contains("Telegram Official Bot")
                && !rows.iter().any(|(_, cid, _, n, _, _)| *cid == Some(d.id) || n == &d.peer_name)
                && (q_lower.is_empty() || d.peer_name.to_lowercase().contains(&q_lower) || d.last_message.to_lowercase().contains(&q_lower))
            {
                rows.push((d.id, Some(d.id), d.avatar_initials.clone(), d.peer_name.clone(), peer_subtitle_for_dialog(d), d.avatar_color));
            }
        }

        modal_backdrop_shell("telegram-contacts-modal-backdrop", macos_modal_backdrop_rgba(pal))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.sidebar.update(cx, |sb, scx| { sb.close_contacts_modal(); scx.notify(); });
                if !this.is_compose_or_creation_modal_active(cx) {
                    this.exit_compose_or_modal_isolation(true, cx);
                } else {
                    this.sync_compose_state_to_composer(true, cx);
                }
                cx.notify();
            }))
            .child(
                macos_modal_card("telegram-contacts-modal-box", pal)
                    .w(px(pal.scaled_px(372.0).min(640.0)))
                    .h(px(pal.scaled_px(492.0).min(760.0)))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div().w_full().px(px(pal.scaled_px(20.0))).pt(px(pal.scaled_px(16.0))).pb(px(pal.scaled_px(10.0))).flex().items_center().justify_between()
                            .child(div().text_size(px(pal.scaled_px(16.0))).font_weight(FontWeight::BOLD).text_color(rgb(pal.text_primary)).child(tr_str(lang, "联系人", "Contacts")))
                            .child(div().text_size(px(pal.scaled_px(12.0))).text_color(rgb(pal.text_secondary)).child(format!("{} {}", rows.len(), tr_str(lang, "位联系人", "contacts")))),
                    )
                    .child(
                        div()
                            .w_full()
                            .px(px(pal.scaled_px(16.0)))
                            .pb(px(pal.scaled_px(10.0)))
                            .border_b_1()
                            .border_color(pal.separator_rgba())
                            .child(
                                div()
                                    .w_full()
                                    .px(px(pal.scaled_px(10.0)))
                                    .py(px(pal.scaled_px(7.0)))
                                    .rounded(px(pal.scaled_px(9.0)))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(pal.separator_rgba())
                                    .flex()
                                    .items_center()
                                    .gap(px(pal.scaled_px(8.0)))
                                    .child(tg_icon(IconName::Search, pal.scaled_px(14.0), pal.icon_fg))
                                    .child(
                                        div()
                                            .text_size(px(pal.scaled_px(13.0)))
                                            .text_color(rgb(if contact_q.is_empty() { pal.text_secondary } else { pal.text_primary }))
                                            .child(if contact_q.is_empty() { tr_str(lang, "搜索", "Search").to_string() } else { contact_q }),
                                    ),
                            ),
                    )
                    .child(
                        div().id("contacts-modal-scroll").flex_1().w_full().py(px(6.0)).overflow_y_scroll().flex().flex_col().gap(px(2.0))
                            .when(rows.is_empty(), |body| {
                                body.items_center().justify_center().child(
                                    div().text_size(px(pal.scaled_px(13.5))).text_color(rgb(pal.text_secondary)).child(tr_str(lang, "未找到相关联系人", "No matching contacts")),
                                )
                            })
                            .when(!rows.is_empty(), |body| {
                                body.children(rows.into_iter().map(|(uid, cid_opt, initials, name, sub, col)| {
                                    let click_name = name.clone();
                                    div().cursor_pointer().mx(px(pal.scaled_px(8.0))).px(px(pal.scaled_px(10.0))).py(px(pal.scaled_px(9.0))).rounded(px(pal.scaled_px(10.0)))
                                        .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg))).flex().items_center().gap(px(pal.scaled_px(12.0)))
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                            this.sidebar.update(cx, |sb, scx| {
                                                sb.close_contacts_modal();
                                                if let Some(cid) = cid_opt { sb.select_dialog_from_user(cid); }
                                                scx.notify();
                                            });
                                            if let Some(cid) = cid_opt {
                                                let prev = this.stage.read(cx).active_chat_id;
                                                this.sync_selected_chat_inspector_and_stage(cid, cx);
                                                if this.composer.read(cx).is_compose_isolated && !this.is_compose_or_creation_modal_active(cx) {
                                                    if prev == Some(cid) {
                                                        this.exit_compose_or_modal_isolation(true, cx);
                                                    } else {
                                                        this.finish_compose_isolation_for_switched_chat(cid, cx);
                                                    }
                                                }
                                            } else {
                                                if !this.is_compose_or_creation_modal_active(cx) {
                                                    this.exit_compose_or_modal_isolation(true, cx);
                                                } else {
                                                    this.sync_compose_state_to_composer(true, cx);
                                                }
                                                let req = CreateChatRequest { title: click_name.clone(), chat_type: Some(serde_json::json!(0)), peer_user_id: Some(uid), ..Default::default() };
                                                if let Some(fence) = this.capture_client_fence() {
                                                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                                                        if !fence.preflight_ui(&weak, app).await {
                                                            return;
                                                        }
                                                        if let Ok(created) = fence
                                                            .client
                                                            .create_chat_with_token(
                                                                &fence.token,
                                                                fence.client_epoch,
                                                                &req,
                                                            )
                                                            .await
                                                        {
                                                            let _ = weak.update(app, |s, cx| {
                                                                if !fence.is_valid(s) {
                                                                    return;
                                                                }
                                                                s.sidebar.update(cx, |sb, _| sb.select_dialog_from_user(created.chat_id as u64));
                                                                s.refresh_chats(cx);
                                                            });
                                                        }
                                                    }).detach();
                                                }
                                            }
                                            cx.notify();
                                        }))
                                        .child(
                                            div().w(px(pal.scaled_px(38.0))).h(px(pal.scaled_px(38.0))).rounded_full().bg(rgb(col)).flex().items_center().justify_center()
                                                .text_size(px(pal.scaled_px(13.5))).font_weight(FontWeight::BOLD).text_color(rgb(pal.active_button_fg)).child(initials),
                                        )
                                        .child(
                                            div().flex().flex_col().gap(px(1.0))
                                                .child(div().text_size(px(pal.scaled_px(13.5))).font_weight(FontWeight::SEMIBOLD).text_color(rgb(pal.text_primary)).child(name))
                                                .when(!sub.is_empty(), |el| el.child(div().text_size(px(pal.scaled_px(11.5))).text_color(rgb(pal.text_secondary)).child(sub))),
                                        )
                                }))
                            }),
                    )
                    .child(
                        div().w_full().px(px(16.0)).py(px(12.0)).border_t_1().border_color(pal.separator_rgba()).flex().items_center().justify_between()
                            .child(
                                macos_primary_pill_btn("contacts-modal-add-btn", tr_str(lang, "添加联系人", "Add Contact"), pal)
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.sidebar.update(cx, |sb, scx| { sb.close_contacts_modal(); scx.notify(); });
                                        let l = this.active_lang_id();
                                        this.open_modal_dialog(tr_str(l, "添加联系人", "Add Contact"), ModalDialogKind::AddContact, 380.0, 360.0, cx);
                                    })),
                            )
                            .child(
                                macos_secondary_pill_btn("contacts-modal-close-btn", tr_str(lang, "关闭", "Close"), pal)
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.sidebar.update(cx, |sb, scx| { sb.close_contacts_modal(); scx.notify(); });
                                        if !this.is_compose_or_creation_modal_active(cx) {
                                            this.exit_compose_or_modal_isolation(true, cx);
                                        } else {
                                            this.sync_compose_state_to_composer(true, cx);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Renders the centered Add Contact / New AI Bot / New Group sidebar creation modal.
    pub fn render_sidebar_creation_modal_overlay(&self, modal_kind: SidebarCreationModal, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();
        let sb_ref = self.sidebar.read(cx);
        let is_bot = sb_ref.contact_kind == AddContactKind::AiBot;
        let active_field = sb_ref.active_creation_field;
        let primary_val = sb_ref.creation_primary_input.clone();
        let secondary_val = sb_ref.creation_secondary_input.clone();
        let tertiary_val = sb_ref.creation_tertiary_input.clone();
        let creation_avatar_shape = sb_ref.creation_avatar_shape.clone();
        let creation_avatar_color = sb_ref.creation_avatar_color.clone();
        let creation_use_dynamic_avatar = sb_ref.creation_use_dynamic_avatar;

        let title = match modal_kind {
            SidebarCreationModal::AddContact if is_bot => tr_str(lang, "创建 AI 智能体助手", "New AI Bot"),
            SidebarCreationModal::AddContact => tr_str(lang, "添加联系人", "Add Contact"),
            SidebarCreationModal::CreateGroup => tr_str(lang, "新建群组", "New Group"),
        };

        let (lbl_1, ph_1, lbl_2, ph_2, lbl_3, ph_3, submit_lbl) = match modal_kind {
            SidebarCreationModal::AddContact if is_bot => (
                tr_str(lang, "机器人名称", "Bot Name"), tr_str(lang, "例如：翻译助手", "e.g. Research Assistant"),
                tr_str(lang, "机器人账号 (@username_bot)", "Bot Handle (@username_bot)"), "assistant_bot",
                tr_str(lang, "系统提示词 (System Prompt)", "System Prompt"), tr_str(lang, "设定智能体的角色与能力…", "Describe assistant persona..."),
                tr_str(lang, "创建机器人", "Create Bot"),
            ),
            SidebarCreationModal::AddContact => (
                tr_str(lang, "姓名", "Name"), tr_str(lang, "输入联系人姓名", "Enter contact name"),
                tr_str(lang, "用户名 (@username)", "Username (@handle)"), "alice",
                tr_str(lang, "手机号码（可选）", "Phone Number (optional)"), "+86 138 0000 0000",
                tr_str(lang, "添加", "Add"),
            ),
            SidebarCreationModal::CreateGroup => (
                tr_str(lang, "群组名称", "Group Name"), tr_str(lang, "输入群组名称", "Enter group title"),
                tr_str(lang, "邀请成员 (@username 或 ID，逗号分隔)", "Members (@username or ID)"), "@alice, @bob",
                tr_str(lang, "群组简介（可选）", "Group Description (optional)"), tr_str(lang, "简单介绍群组用途…", "Optional description..."),
                tr_str(lang, "创建群组", "Create"),
            ),
        };

        let make_field = |field: CreationInputField, label: &str, value: &str, placeholder: &str, cx: &mut Context<Self>| -> AnyElement {
            let focused = active_field == field;
            let shown = if value.is_empty() {
                if focused { "|".to_string() } else { placeholder.to_string() }
            } else if focused { format!("{value}|") } else { value.to_string() };
            div().w_full().flex().flex_col().gap(px(pal.scaled_px(5.0)))
                .child(div().text_size(px(pal.scaled_px(12.0))).font_weight(FontWeight::MEDIUM).text_color(rgb(if focused { pal.accent } else { pal.text_secondary })).child(label.to_string()))
                .child(
                    div().w_full().px(px(pal.scaled_px(12.0))).py(px(pal.scaled_px(9.5))).rounded(px(pal.scaled_px(10.0))).bg(rgb(pal.input_bg))
                        .border_1().border_color(if focused { rgb(pal.accent).into() } else { pal.separator_rgba() }).cursor_text()
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                            this.sidebar.update(cx, |sb, scx| { sb.active_creation_field = field; scx.notify(); });
                            cx.notify();
                        }))
                        .child(div().text_size(px(pal.scaled_px(13.5))).text_color(rgb(if value.is_empty() && !focused { pal.text_secondary } else { pal.text_primary })).child(shown)),
                )
                .into_any_element()
        };

        let make_kind_tab = |selected: bool, label: &'static str, kind: AddContactKind, cx: &mut Context<Self>| -> AnyElement {
            div()
                .flex_1()
                .py(px(pal.scaled_px(6.0)))
                .rounded(px(pal.scaled_px(7.0)))
                .when(selected, |el| {
                    el.bg(segmented_active_bg(pal))
                        .shadow(segmented_pill_shadow(pal))
                })
                .text_size(px(pal.scaled_px(12.5)))
                .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .text_color(rgb(if selected { pal.text_primary } else { pal.text_secondary }))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.sidebar.update(cx, |sb, scx| { sb.contact_kind = kind; scx.notify(); });
                    cx.notify();
                }))
                .child(label)
                .into_any_element()
        };

        let box_1 = make_field(CreationInputField::PrimaryName, lbl_1, &primary_val, ph_1, cx);
        let box_2 = make_field(CreationInputField::SecondaryHandle, lbl_2, &secondary_val, ph_2, cx);
        let box_3 = make_field(CreationInputField::TertiaryPromptOrPhone, lbl_3, &tertiary_val, ph_3, cx);

        let show_bot_avatar_chooser = modal_kind == SidebarCreationModal::AddContact && is_bot;
        let bot_avatar_chooser = if show_bot_avatar_chooser {
            let preview_initials = {
                let d = derive_initials(&primary_val);
                if d.is_empty() { "AI".to_string() } else { d }
            };
            let preview_el: AnyElement = if creation_use_dynamic_avatar {
                render_grok_bot_avatar(
                    GrokBotAvatarProps::new(
                        "sidebar-ai-bot-preview",
                        &creation_avatar_shape,
                        &creation_avatar_color,
                        52.0,
                    )
                    .with_state(GrokPersonaState::Happy)
                    .with_follow_pointer(true)
                    .with_surface_bg(pal.card_bg),
                )
                .into_any_element()
            } else {
                div()
                    .w(px(52.0))
                    .h(px(52.0))
                    .rounded_full()
                    .bg(rgb(pal.accent))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(18.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xFFFFFF))
                    .child(preview_initials)
                    .into_any_element()
            };

            let mode_pill = |id: &'static str, active: bool, label: &'static str, use_dyn: bool, cx: &mut Context<Self>| -> AnyElement {
                div()
                    .id(id)
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(999.0))
                    .bg(rgb(if active { pal.accent } else { pal.input_bg }))
                    .border_1()
                    .border_color(rgb(if active { pal.accent } else { pal.border_fg }))
                    .text_size(px(11.0))
                    .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                    .text_color(rgb(if active { pal.active_button_fg } else { pal.text_primary }))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.sidebar.update(cx, |sb, scx| {
                            sb.creation_use_dynamic_avatar = use_dyn;
                            if use_dyn {
                                trigger_grok_avatar_bounce("sidebar-ai-bot-preview");
                            }
                            scx.notify();
                        });
                        cx.notify();
                    }))
                    .child(label)
                    .into_any_element()
            };

            let mut shape_row = div().flex().flex_wrap().items_center().gap(px(4.0));
            for (idx, &shape) in GROK_AVATAR_SHAPES.iter().enumerate() {
                let selected = creation_avatar_shape.eq_ignore_ascii_case(shape);
                let shape_owned = shape.to_string();
                shape_row = shape_row.child(
                    div()
                        .id(SharedString::from(format!("sidebar-avatar-shape-{idx}")))
                        .cursor_pointer()
                        .p(px(3.0))
                        .rounded(px(7.0))
                        .bg(if selected {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                        } else {
                            rgb(pal.input_bg).into()
                        })
                        .border_1()
                        .border_color(rgb(if selected { pal.accent } else { pal.border_fg }))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                            this.sidebar.update(cx, |sb, scx| {
                                sb.creation_avatar_shape = shape_owned.clone();
                                trigger_grok_avatar_bounce("sidebar-ai-bot-preview");
                                scx.notify();
                            });
                            cx.notify();
                        }))
                        .child(
                            render_grok_bot_avatar(
                                GrokBotAvatarProps::new(
                                    format!("sidebar-shape-opt-{shape}"),
                                    shape,
                                    &creation_avatar_color,
                                    24.0,
                                )
                                .with_state(GrokPersonaState::Idle)
                                .with_paused(true)
                                .with_surface_bg(pal.card_bg),
                            ),
                        ),
                );
            }

            let mut color_row = div().flex().flex_wrap().items_center().gap(px(5.0));
            for (idx, spec) in GROK_COLOR_PALETTE.iter().enumerate() {
                let selected = creation_avatar_color.eq_ignore_ascii_case(spec.id);
                let color_id = spec.id.to_string();
                color_row = color_row.child(
                    div()
                        .id(SharedString::from(format!("sidebar-avatar-color-{idx}")))
                        .cursor_pointer()
                        .w(px(16.0))
                        .h(px(16.0))
                        .rounded_full()
                        .bg(rgb(spec.swatch_rgb))
                        .border_2()
                        .border_color(rgb(if selected { pal.text_primary } else { pal.border_fg }))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                            this.sidebar.update(cx, |sb, scx| {
                                sb.creation_avatar_color = color_id.clone();
                                trigger_grok_avatar_bounce("sidebar-ai-bot-preview");
                                scx.notify();
                            });
                            cx.notify();
                        })),
                );
            }

            Some(
                div()
                    .w_full()
                    .p(px(pal.scaled_px(12.0)))
                    .rounded(px(pal.scaled_px(12.0)))
                    .bg(pal.card_surface_rgba())
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .flex_col()
                    .gap(px(pal.scaled_px(8.0)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(pal.scaled_px(11.5)))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(lang, "Grok Bot 动态头像", "Grok Dynamic Avatar")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(mode_pill(
                                        "sidebar-avatar-mode-dynamic",
                                        creation_use_dynamic_avatar,
                                        tr_str(lang, "动态角色", "Dynamic"),
                                        true,
                                        cx,
                                    ))
                                    .child(mode_pill(
                                        "sidebar-avatar-mode-classic",
                                        !creation_use_dynamic_avatar,
                                        tr_str(lang, "经典字母", "Classic"),
                                        false,
                                        cx,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(pal.scaled_px(12.0)))
                            .child(
                                div()
                                    .id("sidebar-ai-bot-preview-btn")
                                    .cursor_pointer()
                                    .flex_shrink_0()
                                    .on_mouse_down(MouseButton::Left, cx.listener(|_this, _, _, cx| {
                                        trigger_grok_avatar_spin("sidebar-ai-bot-preview");
                                        cx.notify();
                                    }))
                                    .child(preview_el),
                            )
                            .child(if creation_use_dynamic_avatar {
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.0))
                                    .child(shape_row)
                                    .child(color_row)
                                    .into_any_element()
                            } else {
                                div()
                                    .flex_1()
                                    .text_size(px(pal.scaled_px(11.5)))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(
                                        lang,
                                        "使用经典圆形首字母头像（静态）",
                                        "Static classic circular initials avatar",
                                    ))
                                    .into_any_element()
                            }),
                    ),
            )
        } else {
            None
        };

        let modal_w = if show_bot_avatar_chooser { 436.0 } else { 410.0 };

        modal_backdrop_shell("telegram-creation-modal-backdrop", macos_modal_backdrop_rgba(pal))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.sidebar.update(cx, |sb, scx| { sb.close_creation_modal(); scx.notify(); });
                if !this.is_compose_or_creation_modal_active(cx) {
                    this.exit_compose_or_modal_isolation(true, cx);
                } else {
                    this.sync_compose_state_to_composer(true, cx);
                }
                cx.notify();
            }))
            .child(
                macos_modal_card("telegram-creation-modal-box", pal)
                    .w(px(pal.scaled_px(modal_w).min(720.0)))
                    .p(px(pal.scaled_px(22.0)))
                    .flex()
                    .flex_col()
                    .gap(px(pal.scaled_px(12.0)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div().flex().items_center().justify_between()
                            .child(div().text_size(px(pal.scaled_px(17.0))).font_weight(FontWeight::BOLD).text_color(rgb(pal.text_primary)).child(title))
                            .child(
                                div().cursor_pointer().w(px(pal.scaled_px(28.0))).h(px(pal.scaled_px(28.0))).rounded_full().hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                                    .flex().items_center().justify_center()
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.sidebar.update(cx, |sb, scx| { sb.close_creation_modal(); scx.notify(); });
                                        if !this.is_compose_or_creation_modal_active(cx) {
                                            this.exit_compose_or_modal_isolation(true, cx);
                                        } else {
                                            this.sync_compose_state_to_composer(true, cx);
                                        }
                                        cx.notify();
                                    }))
                                    .child(tg_icon(IconName::Close, pal.scaled_px(14.0), pal.icon_fg)),
                            ),
                    )
                    .when(modal_kind == SidebarCreationModal::AddContact, |el| {
                        el.child(
                            div().w_full().p(px(2.5)).rounded(px(9.0)).bg(rgb(pal.input_bg)).border_1().border_color(pal.separator_rgba()).flex().gap(px(2.0))
                                .child(make_kind_tab(!is_bot, tr_str(lang, "好友联系人", "Contact"), AddContactKind::Friend, cx))
                                .child(make_kind_tab(is_bot, tr_str(lang, "AI 智能体", "AI Bot"), AddContactKind::AiBot, cx)),
                        )
                    })
                    .when_some(bot_avatar_chooser, |el, card| el.child(card))
                    .child(box_1).child(box_2).child(box_3)
                    .child(
                        div().pt(px(4.0)).flex().justify_end().gap(px(8.0))
                            .child(
                                macos_secondary_pill_btn("creation-modal-cancel-btn", tr_str(lang, "取消", "Cancel"), pal)
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.sidebar.update(cx, |sb, scx| { sb.close_creation_modal(); scx.notify(); });
                                        if !this.is_compose_or_creation_modal_active(cx) {
                                            this.exit_compose_or_modal_isolation(true, cx);
                                        } else {
                                            this.sync_compose_state_to_composer(true, cx);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                macos_primary_pill_btn("creation-modal-submit-btn", submit_lbl, pal)
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        let created = this.sidebar.update(cx, |sb, scx| {
                                            let res = sb.submit_creation_modal();
                                            scx.notify();
                                            res
                                        });
                                        if created.is_none() {
                                            let msg = tr_str(
                                                this.active_lang_id(),
                                                "请输入对方的真实用户名或手机号",
                                                "Please enter a registered username or phone number",
                                            )
                                            .to_string();
                                            this.show_non_chat_toast(msg, 2600, cx);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Renders the centered Recent Calls modal overlay (`#telegram-calls-modal-box`).
    pub fn render_calls_modal_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let body = self.render_calls_panel(cx);
        Self::render_workspace_modal_shell("telegram-calls-modal-backdrop", "telegram-calls-modal-box", 440.0, 560.0, self.theme_state.palette, body, cx)
    }

    /// Renders the Recent Calls list panel inside the Calls modal.
    pub fn render_calls_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();
        let (title_block, back_btn) = render_panel_header(IconName::PhoneCall, tr_str(lang, "通话记录", "Recent Calls"), tr_str(lang, "返回对话", "Back to Chats"), &pal);
        let my_uid = self.session_sync.user_id.unwrap_or(0);
        let calls = self.panel_state.calls.clone();
        let contacts = self.session_sync.contacts_list.clone();

        div().flex_1().min_w(px(0.0)).h_full().overflow_hidden().bg(rgb(pal.window_bg)).p(px(24.0)).flex().flex_col().gap(px(16.0))
            .child(div().flex().items_center().justify_between().child(title_block).child(
                back_btn.on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_workspace_modal_to_chats(cx))),
            ))
            .when(self.panel_state.calls_loading, |el| {
                el.child(render_empty_panel_notice(IconName::PhoneCall, tr_str(lang, "正在加载通话记录...", "Loading call history..."), "", None, &pal))
            })
            .when(!self.panel_state.calls_loading && calls.is_empty(), |el| {
                el.child(render_empty_panel_notice(
                    IconName::PhoneCall,
                    tr_str(lang, "暂无通话记录", "No recent calls"),
                    tr_str(lang, "您发起或接收的通话将显示在这里。", "Calls you make or receive will appear here."),
                    self.panel_state.calls_error.as_deref(),
                    &pal,
                ))
            })
            .when(!calls.is_empty(), |el| {
                el.child(
                    div().id("calls-history-list").flex_1().w_full().overflow_y_scroll().flex().flex_col().gap(px(8.0))
                        .children(calls.into_iter().map(|call| {
                            let peer_id = if call.caller_id == my_uid { call.callee_id } else { call.caller_id };
                            let peer_name = contacts.iter().find(|ct| ct.user_id == peer_id)
                                .map(|ct| format!("{} {}", ct.first_name, ct.last_name.as_deref().unwrap_or("")).trim().to_string())
                                .filter(|s| !s.is_empty()).unwrap_or_else(|| format!("User {peer_id}"));
                            let ts = format_authoritative_timestamp(Some(call.started_at_ms));
                            let state_label = localize_call_state(&call.state, lang);
                            let sub = if call.duration_secs > 0 { format!("{state_label} · {}s · {ts}", call.duration_secs) }
                                else if !ts.is_empty() { format!("{state_label} · {ts}") } else { state_label };
                            let is_missed = call.state.eq_ignore_ascii_case("missed") || call.state.eq_ignore_ascii_case("declined");
                            let icon = if call.is_video { IconName::VideoCall } else { IconName::PhoneCall };
                            let redial_name = peer_name.clone();
                            let is_vid = call.is_video;

                            div().cursor_pointer().w_full().px(px(14.0)).py(px(12.0)).rounded(px(12.0))
                                .bg(pal.card_surface_rgba()).border_1().border_color(pal.separator_rgba()).hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                                .flex().items_center().justify_between()
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| this.start_webrtc_call(peer_id, redial_name.clone(), is_vid, cx)))
                                .child(
                                    div().flex().items_center().gap(px(12.0))
                                        .child(tg_icon(icon, 18.0, if is_missed { pal.danger } else { pal.accent }))
                                        .child(
                                            div().flex().flex_col().gap(px(2.0))
                                                .child(div().text_size(px(14.0)).font_weight(FontWeight::BOLD).text_color(rgb(pal.text_primary)).child(peer_name))
                                                .child(div().text_size(px(12.0)).text_color(rgb(if is_missed { pal.danger } else { pal.text_secondary })).child(sub)),
                                        ),
                                )
                                .child(tg_icon(IconName::PhoneCall, 16.0, pal.accent))
                        })),
                )
            })
            .into_any_element()
    }

    fn render_studio_modal_panel(&self, icon: IconName, title: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();
        let (title_block, back_btn) = render_panel_header(icon, title, tr_str(lang, "关闭", "Close"), &pal);
        div().flex_1().min_w(px(0.0)).h_full().overflow_hidden().bg(rgb(pal.modal_bg)).p(px(20.0)).flex().flex_col().gap(px(14.0))
            .child(div().flex().items_center().justify_between().child(title_block).child(
                back_btn.on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_workspace_modal_to_chats(cx))),
            ))
            .child(div().flex_1().min_h(px(0.0)).w_full().child(self.tasks_workspace.clone()))
            .into_any_element()
    }

    /// Renders the centered Task Center modal overlay (`#telegram-task-center-modal-box`).
    pub fn render_task_center_modal_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let body = self.render_task_center_panel(cx);
        Self::render_workspace_modal_shell("telegram-task-center-modal-backdrop", "telegram-task-center-modal-box", 760.0, 640.0, self.theme_state.palette, body, cx)
    }

    /// Renders the Fabushi Agent Studio panel inside the Task Center modal.
    pub fn render_task_center_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.active_lang_id();
        self.render_studio_modal_panel(IconName::Poll, tr_str(lang, "Fabushi 智能体工作台", "Fabushi Agent Studio"), cx)
    }

    /// Renders the centered MCP Tools modal overlay (`#telegram-mcp-tools-modal-box`).
    pub fn render_mcp_tools_modal_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let body = self.render_mcp_tools_panel(cx);
        Self::render_workspace_modal_shell("telegram-mcp-tools-modal-backdrop", "telegram-mcp-tools-modal-box", 760.0, 640.0, self.theme_state.palette, body, cx)
    }

    /// Renders the Fabushi Toolsets & Terminal Studio panel inside the MCP Tools modal.
    pub fn render_mcp_tools_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.active_lang_id();
        self.render_studio_modal_panel(IconName::TerminalMcp, tr_str(lang, "Fabushi 工具集与终端工作台", "Fabushi Toolsets & Terminal Studio"), cx)
    }

    /// Renders the bottom-left floating account quick-action popover overlay.
    pub fn render_account_popover_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();
        let is_dark = self.theme_state.theme_mode.is_dark();
        let is_collapsed = self.sidebar.read(cx).is_collapsed;

        let menu_row = |id: &'static str, icon: IconName, label: &'static str, fg_color: u32, is_danger: bool, action: fn(&mut AppShell, &mut Context<AppShell>)| -> AnyElement {
            let row_hover = if is_danger {
                TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.12)
            } else {
                rgb(pal.menu_hover_bg)
            };
            div().id(id).cursor_pointer().w_full().h(px(34.0)).px(px(10.0)).rounded(px(8.0)).hover(move |s| s.bg(row_hover))
                .flex().items_center().gap(px(10.0))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.sidebar.update(cx, |s, scx| { s.is_account_popover_open = false; scx.notify(); });
                    action(this, cx);
                }))
                .child(tg_icon(icon, 15.0, fg_color))
                .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(rgb(fg_color)).child(label))
                .into_any_element()
        };

        let has_passcode = self.has_local_passcode_configured();

        div().id("account-popover-backdrop").absolute().inset_0()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.sidebar.update(cx, |s, scx| { s.is_account_popover_open = false; scx.notify(); });
                cx.notify();
            }))
            .child(
                macos_popover_surface("account-popover-card", pal)
                    .absolute()
                    .left(px(pal.scaled_px(10.0)))
                    .bottom(if is_collapsed { px(pal.scaled_px(52.0)) } else { px(pal.scaled_px(58.0)) })
                    .w(px(pal.scaled_px(240.0)))
                    .p(px(pal.scaled_px(6.0)))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(menu_row("account-popover-profile", IconName::ContactsUsers, tr_str(lang, "个人资料", "Profile"), pal.text_primary, false, |this, cx| this.handle_main_menu_command(MainMenuCommand::MyProfile, cx)))
                    .child(menu_row("account-popover-saved", IconName::SavedMessages, tr_str(lang, "收藏夹", "Saved"), pal.text_primary, false, |this, cx| this.handle_main_menu_command(MainMenuCommand::SavedMessages, cx)))
                    .child(menu_row("account-popover-connectors", IconName::TerminalMcp, tr_str(lang, "连接器", "Connectors"), pal.text_primary, false, |this, cx| this.handle_main_menu_command(MainMenuCommand::McpTools, cx)))
                    .child(menu_row("account-popover-fabushi", IconName::BotRobot, tr_str(lang, "新建智能体 · Fabushi Bot", "New Fabushi Bot"), pal.accent, false, |this, cx| this.open_fabushi_creator_modal(cx)))
                    .child(menu_row("account-popover-botfather", IconName::BotRobot, tr_str(lang, "BotFather · 创建机器人/小程序", "BotFather · Create Bot/App"), pal.accent, false, |this, cx| this.open_botfather_chat(cx)))
                    .child(menu_row("account-popover-miniapps", IconName::FolderBots, tr_str(lang, "小程序中心", "Mini Apps Center"), pal.text_primary, false, |this, cx| this.open_miniapps_center(cx)))
                    .child(menu_row("account-popover-browser", IconName::ScreenShare, tr_str(lang, "内置浏览器 · 自动化应用", "Built-in Browser & Apps"), pal.accent, false, |this, cx| this.handle_main_menu_command(MainMenuCommand::Browser, cx)))
                    .child(menu_row("account-popover-community", IconName::CommunityGlobe, tr_str(lang, "社区 · Reddit × X", "Community · Reddit × X"), pal.accent, false, |this, cx| this.handle_main_menu_command(MainMenuCommand::Community, cx)))
                    .child(menu_row("account-popover-settings", IconName::SettingsGear, tr_str(lang, "设置", "Settings"), pal.text_primary, false, |this, cx| this.handle_main_menu_command(MainMenuCommand::Settings, cx)))
                    .when(has_passcode, |col| {
                        col.child(menu_row(
                            "account-popover-lock",
                            IconName::Lock,
                            tr_str(lang, "立即锁定", "Lock App"),
                            pal.accent,
                            false,
                            |this, cx| {
                                this.lock_app_now(cx);
                            },
                        ))
                    })
                    .child(div().mx(px(8.0)).h(px(1.0)).my(px(4.0)).bg(pal.separator_rgba()))
                    .child(menu_row(
                        "account-popover-theme",
                        if is_dark { IconName::SunDay } else { IconName::NightMoon },
                        if is_dark { tr_str(lang, "浅色模式", "Light Mode") } else { tr_str(lang, "深色模式", "Dark Mode") },
                        pal.text_primary,
                        false,
                        |this, cx| this.handle_main_menu_command(MainMenuCommand::ToggleNightMode, cx),
                    ))
                    .child(menu_row("account-popover-logout", IconName::BackArrow, tr_str(lang, "退出登录", "Log Out"), pal.danger, true, |this, cx| this.handle_main_menu_command(MainMenuCommand::Logout, cx))),
            )
            .into_any_element()
    }

    /// Renders the full-window blocking passcode lockscreen overlay (`#telegram-passcode-lock-overlay`).
    pub fn render_passcode_lock_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();
        let pin_len = self.overlays.passcode_unlock_input.chars().count();
        let masked_pin = if pin_len == 0 {
            tr_str(lang, "输入本地锁定密码…", "Enter your local passcode…").to_string()
        } else {
            format!("{}|", "•".repeat(pin_len))
        };
        let err_opt = self.overlays.passcode_unlock_error.clone();
        let lock_canvas_bg = if pal.is_apple_style() && !pal.mode.is_dark() {
            pal.dialogs_bg
        } else {
            pal.window_bg
        };

        div()
            .id("telegram-passcode-lock-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgb(lock_canvas_bg))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                macos_modal_card("telegram-passcode-lock-card", pal)
                    .w(px(pal.scaled_px(372.0)))
                    .p(px(pal.scaled_px(30.0)))
                    .rounded(px(pal.scaled_px(16.0)))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(pal.scaled_px(14.0)))
                    .child(
                        div()
                            .w(px(pal.scaled_px(60.0)))
                            .h(px(pal.scaled_px(60.0)))
                            .rounded(px(pal.scaled_px(16.0)))
                            .bg(rgb(pal.accent))
                            .shadow(vec![BoxShadow {
                                color: TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.30).into(),
                                offset: point(px(0.0), px(6.0)),
                                blur_radius: px(16.0),
                                spread_radius: px(0.0),
                            }])
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(tg_icon(
                                IconName::Lock,
                                pal.scaled_px(26.0),
                                pal.active_button_fg,
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(pal.scaled_px(18.0)))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(tr_str(lang, "Telegram 已锁定", "Telegram Locked")),
                    )
                    .child(
                        div()
                            .text_size(px(pal.scaled_px(12.5)))
                            .text_color(rgb(pal.text_secondary))
                            .text_center()
                            .child(tr_str(
                                lang,
                                "请输入您的本地锁定密码以解锁对话",
                                "Enter your local passcode to unlock chats",
                            )),
                    )
                    .child(
                        div()
                            .id("passcode-lock-input")
                            .w_full()
                            .px(px(pal.scaled_px(14.0)))
                            .py(px(pal.scaled_px(10.0)))
                            .rounded(px(pal.scaled_px(10.0)))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(rgb(if err_opt.is_some() {
                                pal.danger
                            } else {
                                pal.accent
                            }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(pal.scaled_px(if pin_len == 0 { 13.0 } else { 18.0 })))
                            .text_color(rgb(if pin_len == 0 {
                                pal.text_secondary
                            } else {
                                pal.text_primary
                            }))
                            .child(masked_pin),
                    )
                    .when_some(err_opt, |col, err| {
                        col.child(
                            div()
                                .id("passcode-lock-error")
                                .text_size(px(pal.scaled_px(12.0)))
                                .text_color(rgb(pal.danger))
                                .child(err),
                        )
                    })
                    .child(
                        div()
                            .id("passcode-lock-submit-btn")
                            .cursor_pointer()
                            .w_full()
                            .py(px(pal.scaled_px(10.5)))
                            .rounded(px(pal.scaled_px(10.0)))
                            .bg(rgb(pal.accent))
                            .hover(move |s| s.bg(rgb(pal.accent_hover)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(pal.scaled_px(14.0)))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.active_button_fg))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.submit_passcode_unlock_input(cx);
                                }),
                            )
                            .child(tr_str(lang, "解锁", "Unlock")),
                    ),
            )
            .into_any_element()
    }

    /// Renders a floating approval banner when an AI Bot or cross-device peer asks to control a host.
    pub fn render_pending_computer_approval_banner(
        &self,
        pending: &PendingComputerCommandDto,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();
        let is_zh = lang.starts_with("zh");
        let caller = if pending.caller_label.trim().is_empty() {
            if let Some(bot_id) = pending.caller_bot_id {
                format!("AI Bot #{bot_id}")
            } else if let Some(ref did) = pending.caller_device_id {
                did.clone()
            } else {
                tr_str(lang, "远程客户端", "Remote client").to_string()
            }
        } else {
            pending.caller_label.clone()
        };
        let op_summary = match &pending.operation {
            RemoteComputerOp::ShellExec { command, cwd, .. } => {
                if let Some(dir) = cwd {
                    format!("$ {command}  ({dir})")
                } else {
                    format!("$ {command}")
                }
            }
            RemoteComputerOp::WriteFile { path, content, .. } => {
                format!("Write {} ({} bytes)", path, content.len())
            }
            RemoteComputerOp::ReadFile { path, .. } => format!("Read {path}"),
            RemoteComputerOp::ListDir { path } => format!("List {path}"),
            RemoteComputerOp::CaptureScreen => tr_str(lang, "抓取当前屏幕画面", "Capture current screen").to_string(),
            RemoteComputerOp::GuiInput { action, x, y, text, key } => {
                format!("GUI {action} ({x:?}, {y:?}) text={text:?} key={key:?}")
            }
            RemoteComputerOp::MouseAction { action, x, y, dx, dy, button, .. } => {
                format!("Mouse {action} pos=({x:?}, {y:?}) delta=({dx:?}, {dy:?}) btn={button:?}")
            }
            RemoteComputerOp::KeyboardAction { action, key, text, modifiers } => {
                format!("Key {action} key={key:?} mods={modifiers:?} text={text:?}")
            }
            RemoteComputerOp::ClipboardGet => tr_str(lang, "读取剪贴板", "Read clipboard").to_string(),
            RemoteComputerOp::ClipboardSet { text, mime_type, .. } => {
                let len = text.as_ref().map(|t| t.len()).unwrap_or(0);
                format!("ClipboardSet ({len} chars, mime={mime_type:?})")
            }
            RemoteComputerOp::PowerAction { action } => format!("Power {action:?}"),
            RemoteComputerOp::DisplayList => tr_str(lang, "枚举显示器", "List displays").to_string(),
            RemoteComputerOp::DisplaySwitch { display_index } => format!("Switch display #{display_index}"),
            RemoteComputerOp::PrivacyScreenToggle { enabled, .. } => {
                format!("Privacy screen {}", if *enabled { "ON" } else { "OFF" })
            }
            RemoteComputerOp::TerminalOp { op, session_name, command, .. } => {
                format!("Terminal {op} session={session_name:?} cmd={command:?}")
            }
            RemoteComputerOp::AccessibilityTree { max_depth } => {
                format!("AccessibilityTree (max_depth={max_depth:?})")
            }
            RemoteComputerOp::SemanticAction { element_id, action, .. } => {
                format!("Semantic {action} on {element_id}")
            }
            RemoteComputerOp::FileChunkOp { op, transfer_id, path, offset, .. } => {
                format!("FileChunk {op} {path} id={transfer_id} offset={offset}")
            }
            RemoteComputerOp::TestConnection => tr_str(lang, "测试连接", "Test connection").to_string(),
        };
        let cmd_approve = pending.command_id.clone();
        let cmd_deny = pending.command_id.clone();
        let target_label = if is_zh {
            format!("{} 请求在 {} 上执行：", caller, pending.node_id)
        } else {
            format!("{} requests execution on {}:", caller, pending.node_id)
        };

        macos_modal_card("pending-computer-approval-banner", pal)
            .occlude()
            .absolute()
            .top(px(58.0))
            .right(px(20.0))
            .w(px(392.0))
            .p(px(14.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(tg_icon(IconName::ComputerDesktop, 15.0, pal.accent))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(tr_str(lang, "远程控制请求（每次询问）", "Remote Control Approval")),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(target_label),
            )
            .child(
                div()
                    .w_full()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(10.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .text_xs()
                    .text_color(rgb(pal.text_primary))
                    .child(op_summary),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("deny-computer-cmd-btn")
                            .cursor_pointer()
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(9.0))
                            .bg(pal.card_surface_rgba())
                            .border_1()
                            .border_color(pal.separator_rgba())
                            .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(pal.text_primary))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.decide_pending_computer_approval(cmd_deny.clone(), false, cx);
                            }))
                            .child(tr_str(lang, "拒绝", "Deny")),
                    )
                    .child(
                        div()
                            .id("approve-computer-cmd-btn")
                            .cursor_pointer()
                            .px(px(14.0))
                            .py(px(6.0))
                            .rounded(px(9.0))
                            .bg(rgb(pal.accent))
                            .hover(move |s| s.bg(rgb(pal.accent_hover)))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xFFFFFF))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.decide_pending_computer_approval(cmd_approve.clone(), true, cx);
                            }))
                            .child(tr_str(lang, "允许执行", "Allow")),
                    ),
            )
            .into_any_element()
    }

    /// Renders a sleek, non-blocking floating banner on first vault creation so the user
    /// can copy or inspect their one-time Emergency Kit without modal interruption.
    pub fn render_vault_first_setup_notice_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();

        macos_modal_card("vault-first-setup-notice-banner", pal)
            .occlude()
            .absolute()
            .bottom(px(76.0))
            .right(px(20.0))
            .w(px(368.0))
            .p(px(14.0))
            .flex()
            .flex_col()
            .gap(px(9.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.0))
                            .child(tg_icon(IconName::Lock, 15.0, pal.accent))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(tr_str(
                                        lang,
                                        "端到端加密保管库已自动启用",
                                        "E2EE Vault Enabled Automatically",
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id("dismiss-vault-banner-btn")
                            .cursor_pointer()
                            .w(px(24.0))
                            .h(px(24.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.bot_browser.first_creation_banner_open = false;
                                cx.notify();
                            }))
                            .child("×"),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(
                        lang,
                        "已为您的设备配置零知识密钥与一次性恢复密钥（绝不上传明文）。建议复制并离线备份。",
                        "Zero-knowledge vault keys and a local-only Emergency Kit were created. Copy it for offline backup.",
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("open-privacy-vault-settings-btn")
                            .cursor_pointer()
                            .px(px(11.0))
                            .py(px(6.0))
                            .rounded(px(9.0))
                            .bg(pal.card_surface_rgba())
                            .border_1()
                            .border_color(pal.separator_rgba())
                            .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(pal.text_primary))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.bot_browser.first_creation_banner_open = false;
                                this.leave_browser_panel_if_active(cx);
                                this.leave_community_panel_if_active(cx);
                                this.active_panel = ActiveWorkspacePanel::Settings;
                                this.last_handled_nav_dest = Some(NavDestination::Settings);
                                this.nav_rail.update(cx, |r, rcx| {
                                    r.set_active_destination(NavDestination::Settings);
                                    rcx.notify();
                                });
                                this.reconcile_chat_scoped_toasts(cx);
                                this.settings_router.update(cx, |r, rcx| {
                                    r.ensure_privacy(rcx);
                                    r.navigate_to(SettingsSectionType::PrivacySecurity, true, rcx);
                                });
                                this.sync_vault_custody_to_settings(cx);
                                this.refresh_pending_device_sync_requests(cx);
                                cx.notify();
                            }))
                            .child(tr_str(lang, "查看详情", "Details")),
                    )
                    .child(
                        div()
                            .id("copy-vault-emergency-kit-btn")
                            .cursor_pointer()
                            .px(px(13.0))
                            .py(px(6.0))
                            .rounded(px(9.0))
                            .bg(rgb(pal.accent))
                            .hover(move |s| s.bg(rgb(pal.accent_hover)))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xFFFFFF))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.handle_settings_vault_action(
                                    &SettingsVaultAction::CopyEmergencyKit,
                                    cx,
                                );
                            }))
                            .child(tr_str(lang, "复制恢复密钥", "Copy Kit")),
                    ),
            )
            .into_any_element()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AppleComposeRecipient {
    pub user_id: u64,
    pub chat_id: Option<u64>,
    pub username: Option<String>,
    pub initials: String,
    pub name: String,
    pub subtitle: String,
    pub is_bot: bool,
    pub bot_id: Option<String>,
    pub avatar_color: u32,
    pub avatar_shape: Option<String>,
}

impl AppleComposeRecipient {
    pub(crate) fn into_chip(self) -> ComposeRecipientChip {
        ComposeRecipientChip {
            user_id: self.user_id,
            chat_id: self.chat_id,
            username: self.username,
            display_name: self.name,
            initials: self.initials,
            is_bot: self.is_bot,
            bot_id: self.bot_id,
            avatar_color: self.avatar_color,
            avatar_shape: self.avatar_shape,
        }
    }
}

fn compose_query_matches(
    query: &str,
    name: &str,
    username: Option<&str>,
    bot_id: Option<&str>,
    subtitle: &str,
) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let name_l = name.to_lowercase();
    let un_l = username.unwrap_or("").trim().trim_start_matches('@').to_lowercase();
    let bot_l = bot_id.unwrap_or("").trim().to_lowercase();
    let sub_l = subtitle.to_lowercase();
    q.split_whitespace().all(|tok| {
        let clean_tok = tok.trim_start_matches('@');
        if clean_tok.is_empty() {
            return true;
        }
        name_l.contains(clean_tok)
            || (!un_l.is_empty() && un_l.contains(clean_tok))
            || (!bot_l.is_empty() && bot_l.contains(clean_tok))
            || sub_l.contains(clean_tok)
    })
}

impl AppShell {
    /// Resolves an existing 1:1 DM `chat_id` for a real `user_id` without ever returning a group/channel ID.
    pub(crate) fn resolve_existing_dm_chat_id_for_user(&self, user_id: u64, cx: &App) -> Option<u64> {
        if user_id == 0 {
            return None;
        }
        let sb_ref = self.sidebar.read(cx);
        let is_non_dm = |cid: u64| -> bool {
            sb_ref.group_chat_ids.contains(&cid)
                || sb_ref.channel_chat_ids.contains(&cid)
                || sb_ref.saved_messages_id == Some(cid)
                || self.session_sync.rich_chats_by_id.get(&cid).is_some_and(|rc| {
                    rc.is_group || rc.is_channel || matches!(rc.chat_type, 1 | 2 | 3 | 5)
                })
        };

        if let Some(cid) = self
            .miniapps
            .fabushi_bots
            .iter()
            .find(|b| b.bot_user_id > 0 && b.bot_user_id as u64 == user_id)
            .and_then(|b| b.companion_chat_id)
            .filter(|&cid| cid > 0)
            .map(|cid| cid as u64)
            .filter(|&cid| !is_non_dm(cid))
        {
            return Some(cid);
        }

        if let Some(cid) = self
            .session_sync
            .contacts_list
            .iter()
            .find(|c| c.user_id == user_id)
            .and_then(|c| c.chat_id)
            .filter(|&cid| cid > 0)
            .map(|cid| cid as u64)
            .filter(|&cid| !is_non_dm(cid))
        {
            return Some(cid);
        }

        for (&cid, &peer_uid) in &self.session_sync.chat_peer_user_ids {
            if peer_uid == user_id && !is_non_dm(cid) {
                return Some(cid);
            }
        }

        for (&cid, rc) in &self.session_sync.rich_chats_by_id {
            if rc.peer_user_id == Some(user_id) && !is_non_dm(cid) {
                return Some(cid);
            }
        }

        None
    }

    pub(crate) fn collect_apple_compose_recipients(&self, cx: &App) -> Vec<AppleComposeRecipient> {
        let sb_ref = self.sidebar.read(cx);
        let query = sb_ref.compose_recipient_query.trim();
        let my_uid = self.session_sync.user_id;
        let selected_user_ids: std::collections::HashSet<u64> = sb_ref
            .compose_draft
            .selected_recipients
            .iter()
            .filter(|chip| chip.user_id > 0)
            .map(|chip| chip.user_id)
            .collect();
        let selected_usernames: std::collections::HashSet<String> = sb_ref
            .compose_draft
            .selected_recipients
            .iter()
            .filter_map(|chip| {
                chip.username
                    .as_deref()
                    .map(|u| u.trim().trim_start_matches('@').to_ascii_lowercase())
                    .filter(|u| !u.is_empty())
            })
            .collect();

        let mut seen_user_ids: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut bot_rows: Vec<AppleComposeRecipient> = Vec::new();
        let mut contact_rows: Vec<AppleComposeRecipient> = Vec::new();

        // (a) First-class Fabushi AI Bots (always carry real bot_user_id, bot_id, username, avatar shape/color)
        for bot in &self.miniapps.fabushi_bots {
            if bot.bot_user_id <= 0 {
                continue;
            }
            let uid = bot.bot_user_id as u64;
            if Some(uid) == my_uid || selected_user_ids.contains(&uid) || !seen_user_ids.insert(uid) {
                continue;
            }
            let clean_un = bot
                .username
                .trim()
                .trim_start_matches('@')
                .to_string();
            let username_opt = (!clean_un.is_empty()).then_some(clean_un.clone());
            if username_opt
                .as_deref()
                .is_some_and(|u| selected_usernames.contains(&u.to_ascii_lowercase()))
            {
                continue;
            }
            let name = if !bot.display_name.trim().is_empty() {
                bot.display_name.trim().to_string()
            } else if !bot.title.trim().is_empty() {
                bot.title.trim().to_string()
            } else if !clean_un.is_empty() {
                clean_un.clone()
            } else if !bot.agent_id.trim().is_empty() {
                bot.agent_id.trim().to_string()
            } else {
                format!("Bot {uid}")
            };
            let bot_id_opt = (!bot.agent_id.trim().is_empty()).then(|| bot.agent_id.trim().to_string());
            let subtitle = match (username_opt.as_deref(), bot.description.trim()) {
                (Some(un), desc) if !desc.is_empty() => format!("@{un} · {desc}"),
                (Some(un), _) => format!("@{un}"),
                (None, desc) if !desc.is_empty() => desc.to_string(),
                _ => bot_id_opt.clone().unwrap_or_else(|| "AI Bot".to_string()),
            };
            if !compose_query_matches(
                query,
                &name,
                username_opt.as_deref(),
                bot_id_opt.as_deref(),
                &subtitle,
            ) {
                continue;
            }
            let resolved_chat = bot
                .companion_chat_id
                .filter(|&id| id > 0)
                .map(|id| id as u64)
                .or_else(|| self.resolve_existing_dm_chat_id_for_user(uid, cx));
            let avatar_color = crate::components::avatar::parse_fabushi_hex_color(&bot.avatar_color)
                .unwrap_or_else(|| peer_avatar_color(uid));
            let avatar_shape = (!bot.avatar_shape.trim().is_empty())
                .then(|| bot.avatar_shape.trim().to_string());
            bot_rows.push(AppleComposeRecipient {
                user_id: uid,
                chat_id: resolved_chat,
                username: username_opt,
                initials: derive_initials(&name),
                name,
                subtitle,
                is_bot: true,
                bot_id: bot_id_opt,
                avatar_color,
                avatar_shape,
            });
        }

        // (b) Saved contacts & existing 1:1 peers from `contacts_list`
        for c in &self.session_sync.contacts_list {
            if c.user_id == 0 || Some(c.user_id) == my_uid {
                continue;
            }
            if selected_user_ids.contains(&c.user_id) || !seen_user_ids.insert(c.user_id) {
                continue;
            }
            let clean_un = c
                .username
                .as_deref()
                .map(|u| u.trim().trim_start_matches('@').to_string())
                .filter(|u| !u.is_empty());
            if clean_un
                .as_deref()
                .is_some_and(|u| selected_usernames.contains(&u.to_ascii_lowercase()))
            {
                continue;
            }
            let full = format!("{} {}", c.first_name, c.last_name.as_deref().unwrap_or(""))
                .trim()
                .to_string();
            let name = if !full.is_empty() {
                full
            } else if let Some(ref un) = clean_un {
                un.clone()
            } else {
                format!("User {}", c.user_id)
            };
            let subtitle = clean_un
                .as_deref()
                .map(|u| format!("@{u}"))
                .or_else(|| c.phone.clone())
                .unwrap_or_default();
            if !compose_query_matches(query, &name, clean_un.as_deref(), None, &subtitle) {
                continue;
            }
            let resolved_chat = c
                .chat_id
                .filter(|&id| id > 0)
                .map(|id| id as u64)
                .or_else(|| self.resolve_existing_dm_chat_id_for_user(c.user_id, cx));
            let avatar_shape = resolved_chat
                .and_then(|cid| sb_ref.bot_grok_avatars.get(&cid))
                .map(|v| v.shape.clone());
            let row = AppleComposeRecipient {
                user_id: c.user_id,
                chat_id: resolved_chat,
                username: clean_un,
                initials: derive_initials(&name),
                name,
                subtitle,
                is_bot: c.is_bot,
                bot_id: None,
                avatar_color: peer_avatar_color(c.user_id),
                avatar_shape,
            };
            if c.is_bot {
                bot_rows.push(row);
            } else {
                contact_rows.push(row);
            }
        }

        // (c) 1:1 human/bot peers from `sb.dialogs` ONLY when real `peer_user_id` is resolvable
        // (NEVER pass `d.id` chat_id as `user_id`, and NEVER include groups/channels/saved messages).
        for d in &sb_ref.dialogs {
            if d.peer_name.contains("Telegram Official Bot")
                || sb_ref.group_chat_ids.contains(&d.id)
                || sb_ref.channel_chat_ids.contains(&d.id)
                || sb_ref.saved_messages_id == Some(d.id)
                || matches!(d.folder.as_str(), "Groups" | "Channels")
            {
                continue;
            }
            let rich = self.session_sync.rich_chats_by_id.get(&d.id);
            if rich.is_some_and(|rc| {
                rc.is_group || rc.is_channel || matches!(rc.chat_type, 1 | 2 | 3 | 5)
            }) {
                continue;
            }
            let Some(peer_uid) = self
                .session_sync
                .chat_peer_user_ids
                .get(&d.id)
                .copied()
                .or_else(|| rich.and_then(|rc| rc.peer_user_id))
                .or_else(|| {
                    self.session_sync
                        .contacts_list
                        .iter()
                        .find(|c| c.chat_id == Some(d.id as i64))
                        .map(|c| c.user_id)
                })
                .filter(|&uid| uid > 0 && Some(uid) != my_uid)
            else {
                continue;
            };
            if selected_user_ids.contains(&peer_uid) || !seen_user_ids.insert(peer_uid) {
                continue;
            }
            let clean_un = rich
                .and_then(|rc| rc.peer_username.clone().or_else(|| rc.username.clone()))
                .map(|u| u.trim().trim_start_matches('@').to_string())
                .filter(|u| !u.is_empty());
            if clean_un
                .as_deref()
                .is_some_and(|u| selected_usernames.contains(&u.to_ascii_lowercase()))
            {
                continue;
            }
            let is_bot = sb_ref.bot_ids.contains(&d.id)
                || d.folder == "Bots"
                || rich.is_some_and(|rc| rc.is_bot);
            let subtitle = clean_un
                .as_deref()
                .map(|u| format!("@{u}"))
                .unwrap_or_else(|| peer_subtitle_for_dialog(d));
            if !compose_query_matches(query, &d.peer_name, clean_un.as_deref(), None, &subtitle) {
                continue;
            }
            let avatar_shape = sb_ref.bot_grok_avatars.get(&d.id).map(|v| v.shape.clone());
            let row = AppleComposeRecipient {
                user_id: peer_uid,
                chat_id: Some(d.id),
                username: clean_un,
                initials: d.avatar_initials.clone(),
                name: d.peer_name.clone(),
                subtitle,
                is_bot,
                bot_id: None,
                avatar_color: d.avatar_color,
                avatar_shape,
            };
            if is_bot {
                bot_rows.push(row);
            } else {
                contact_rows.push(row);
            }
        }

        bot_rows.extend(contact_rows);
        bot_rows
    }

    /// Adds a candidate from the `NewChatBar` dropdown as a `MemberChip` and keeps the picker open
    /// so users can effortlessly multi-select 2, 3, 4+ contacts and AI Bots.
    pub(crate) fn select_apple_compose_candidate(
        &mut self,
        rec: AppleComposeRecipient,
        cx: &mut Context<Self>,
    ) {
        self.ime.clear_preedit();
        self.sidebar.update(cx, |sb, scx| {
            sb.compose_title_focused = false;
            sb.add_compose_recipient(rec.into_chip());
            scx.notify();
        });
        self.sync_compose_state_to_composer(true, cx);
        cx.notify();
    }

    /// Commits a single selected recipient chip straight into its 1:1 DM, carrying over any
    /// unsent draft text from the bottom composer.
    pub(crate) fn commit_single_compose_recipient_to_dm(
        &mut self,
        chip: ComposeRecipientChip,
        cx: &mut Context<Self>,
    ) {
        if !self.is_authenticated || self.session_sync.session_token.is_none() {
            return;
        }
        if self.sidebar.read(cx).compose_draft.pending_creation.is_some() {
            return;
        }
        self.ime.clear_preedit();
        let carry_draft = self.composer.read(cx).text().to_string();
        let existing_chat = chip
            .chat_id
            .or_else(|| self.resolve_existing_dm_chat_id_for_user(chip.user_id, cx));
        let has_local_dialog = existing_chat.is_some_and(|cid| {
            self.sidebar.read(cx).dialogs.iter().any(|d| d.id == cid)
        });

        if let Some(cid) = existing_chat.filter(|_| has_local_dialog) {
            let same_chat = self.stage.read(cx).active_chat_id == Some(cid);
            self.sidebar.update(cx, |sb, scx| {
                sb.exit_compose_mode();
                sb.select_dialog_from_user(cid);
                scx.notify();
            });
            // Keep `is_compose_isolated == true` during `sync_selected_chat_inspector_and_stage`
            // so `prev_chat_id`'s saved draft is never flushed/clobbered before switching.
            self.sync_selected_chat_inspector_and_stage(cid, cx);
            if same_chat && carry_draft.trim().is_empty() {
                self.exit_compose_or_modal_isolation(true, cx);
            } else {
                self.finish_compose_isolation_for_switched_chat(cid, cx);
                let show_start_bar = self.stage.read(cx).show_bot_start_bar;
                self.composer.update(cx, |c, ccx| {
                    if !carry_draft.trim().is_empty() {
                        c.set_text(carry_draft);
                    }
                    c.is_input_focused = !show_start_bar;
                    ccx.notify();
                });
            }
        } else {
            // Keep compose isolation active while `create_chat` is in flight so `carry_draft`
            // cannot leak into the background chat via `handle_composer_draft_change`.
            let seq = self.client.next_client_nonce();
            let client_nonce = format!("dm-{}-{}", chrono::Utc::now().timestamp_millis(), seq);
            let compose_epoch = self.sidebar.update(cx, |sb, scx| {
                sb.compose_draft.menu_open = false;
                sb.compose_draft.creation_error = None;
                sb.compose_draft.pending_creation = Some(PendingComposeCreation {
                    title: chip.display_name.clone(),
                    recipients: vec![chip.clone()],
                    initial_message: carry_draft.clone(),
                    client_nonce: client_nonce.clone(),
                });
                let epoch = sb.compose_draft.compose_epoch;
                scx.notify();
                epoch
            });
            self.sync_compose_state_to_composer(true, cx);

            let chat_type_num = if chip.is_bot { 4 } else { 0 };
            let req = CreateChatRequest {
                title: chip.display_name.clone(),
                chat_type: Some(serde_json::json!(chat_type_num)),
                peer_user_id: (chip.user_id > 0).then_some(chip.user_id),
                member_usernames: if chip.user_id == 0 {
                    chip.username.clone().into_iter().collect()
                } else {
                    Vec::new()
                },
                client_nonce: Some(client_nonce.clone()),
                ..Default::default()
            };
            let Some(fence) = self.capture_client_fence() else {
                return;
            };
            let chip_clone = chip.clone();
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                if !fence.preflight_ui(&weak, app).await {
                    return;
                }
                let res = fence
                    .client
                    .create_chat_with_token(&fence.token, fence.client_epoch, &req)
                    .await;
                let _ = weak.update(app, |s, cx| {
                    if !fence.is_valid(s) {
                        return;
                    }
                    let (matches_pending, plus_open, epoch_matches) = {
                        let sb = s.sidebar.read(cx);
                        (
                            sb.compose_draft
                                .pending_creation
                                .as_ref()
                                .is_some_and(|p| p.client_nonce == client_nonce),
                            sb.is_plus_menu_open,
                            sb.compose_draft.compose_epoch == compose_epoch,
                        )
                    };
                    if !matches_pending || !plus_open || !epoch_matches {
                        if matches_pending {
                            s.sidebar.update(cx, |sb, scx| {
                                if sb
                                    .compose_draft
                                    .pending_creation
                                    .as_ref()
                                    .is_some_and(|p| p.client_nonce == client_nonce)
                                {
                                    sb.compose_draft.pending_creation = None;
                                    scx.notify();
                                }
                            });
                        }
                        if res.is_ok() {
                            s.refresh_chats(cx);
                        }
                        return;
                    }
                    match res {
                        Ok(created) => {
                            let cid = created.chat_id.max(0) as u64;
                            let now_label =
                                tr_str(s.active_lang_id(), "刚刚", "Just now").to_string();
                            s.sidebar.update(cx, |sb, scx| {
                                sb.compose_draft.pending_creation = None;
                                sb.exit_compose_mode();
                                if !sb.dialogs.iter().any(|d| d.id == cid) {
                                    sb.dialogs.insert(
                                        0,
                                        DialogItemModel {
                                            id: cid,
                                            peer_name: if created.title.trim().is_empty() {
                                                chip_clone.display_name.clone()
                                            } else {
                                                created.title.clone()
                                            },
                                            avatar_initials: chip_clone.initials.clone(),
                                            avatar_color: chip_clone.avatar_color,
                                            last_message: String::new(),
                                            timestamp: now_label,
                                            unread_count: 0,
                                            is_muted: false,
                                            is_pinned: false,
                                            is_online: true,
                                            typing_indicator: None,
                                            delivery_status: DialogDeliveryStatus::Read,
                                            folder: if chip_clone.is_bot {
                                                "Bots".to_string()
                                            } else {
                                                "All".to_string()
                                            },
                                        },
                                    );
                                    if chip_clone.is_bot {
                                        sb.bot_ids.insert(cid);
                                    }
                                    sb.recompute_folder_badges();
                                }
                                sb.select_dialog_from_user(cid);
                                scx.notify();
                            });
                            s.sync_selected_chat_inspector_and_stage(cid, cx);
                            s.finish_compose_isolation_for_switched_chat(cid, cx);
                            let show_start_bar = s.stage.read(cx).show_bot_start_bar;
                            s.composer.update(cx, |c, ccx| {
                                if !carry_draft.trim().is_empty() {
                                    c.set_text(carry_draft);
                                }
                                c.is_input_focused = !show_start_bar;
                                ccx.notify();
                            });
                            s.refresh_chats(cx);
                            cx.notify();
                        }
                        Err(err) => {
                            s.sidebar.update(cx, |sb, scx| {
                                sb.compose_draft.pending_creation = None;
                                sb.is_plus_menu_open = true;
                                if sb.compose_draft.selected_recipients.is_empty() {
                                    sb.compose_draft.selected_recipients = vec![chip_clone.clone()];
                                }
                                scx.notify();
                            });
                            s.sync_compose_state_to_composer(true, cx);
                            let prefix =
                                tr_str(s.active_lang_id(), "无法打开对话", "Failed to open chat");
                            s.show_non_chat_toast(format!("{prefix}: {err}"), 3200, cx);
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    pub(crate) fn open_add_friend_from_compose_query(&mut self, cx: &mut Context<Self>) {
        self.ime.clear_preedit();
        self.enter_compose_or_modal_isolation(cx);
        let raw_q = self.sidebar.read(cx).compose_recipient_query.trim().to_string();
        self.sidebar.update(cx, |sb, scx| {
            sb.open_add_contact_modal(AddContactKind::Friend);
            if !raw_q.is_empty() {
                if raw_q.starts_with('@') {
                    sb.creation_secondary_input = raw_q.trim_start_matches('@').to_string();
                } else {
                    sb.creation_primary_input = raw_q;
                }
            }
            scx.notify();
        });
        self.sync_compose_state_to_composer(true, cx);
        cx.notify();
    }

    pub fn render_apple_compose_stage(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.theme_state.palette.clone();
        let lang = self.active_lang_id();
        let is_dark = pal.mode.is_dark();
        let (
            query,
            selected_chips,
            is_group_mode,
            highlighted_index,
            menu_open,
            custom_group_title,
            auto_group_title,
            pending_creation,
            creation_error,
            has_bot_chip,
            bot_notice_dismissed,
            title_focused,
        ) = {
            let sb = self.sidebar.read(cx);
            (
                sb.compose_recipient_query.clone(),
                sb.compose_draft.selected_recipients.clone(),
                sb.compose_draft.is_group_mode,
                sb.compose_draft.highlighted_index,
                sb.compose_draft.menu_open,
                sb.compose_draft.custom_group_title.clone(),
                sb.auto_generate_group_title(None),
                sb.compose_draft.pending_creation.clone(),
                sb.compose_draft.creation_error.clone(),
                sb.has_compose_bot_recipient(),
                sb.compose_bot_notice_dismissed,
                sb.compose_title_focused,
            )
        };
        let compose_focused = self.is_apple_compose_input_active(cx);
        let to_bar_focused = compose_focused && !title_focused;
        let preedit = if to_bar_focused {
            self.ime.ime_preedit.clone()
        } else {
            None
        };
        let title_preedit = if compose_focused && title_focused {
            self.ime.ime_preedit.clone()
        } else {
            None
        };
        let recipients = self.collect_apple_compose_recipients(cx);
        let effective_highlight = if recipients.is_empty() {
            0
        } else {
            highlighted_index.min(recipients.len() - 1)
        };
        let chip_surface_bg = if is_dark {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.10)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x1C1C1C, 0.06)
        };
        let chip_hover_bg = if is_dark {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.16)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x1C1C1C, 0.10)
        };
        let menu_row_hover = if is_dark {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.08)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x1C1C1C, 0.06)
        };
        let menu_surface_bg = if is_dark {
            rgb(0x1F1F1F)
        } else {
            rgb(pal.menu_bg)
        };
        let show_group_hint = is_group_mode || !selected_chips.is_empty();
        let single_chip_for_dm = if selected_chips.len() == 1 && pending_creation.is_none() {
            selected_chips.first().cloned()
        } else {
            None
        };

        let quick_pill = |id: &'static str, icon: IconName, tint: u32, label: &'static str| {
            div()
                .id(id)
                .cursor_pointer()
                .h(px(pal.scaled_px(28.0)))
                .px(px(pal.scaled_px(10.0)))
                .rounded_full()
                .bg(chip_surface_bg)
                .border_1()
                .border_color(pal.separator_rgba())
                .hover(move |s| s.bg(chip_hover_bg))
                .flex()
                .items_center()
                .gap(px(pal.scaled_px(6.0)))
                .child(tg_icon(icon, pal.scaled_px(12.5), tint))
                .child(
                    div()
                        .text_size(px(pal.scaled_px(12.0)))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.text_primary))
                        .child(label),
                )
        };

        let render_chip_avatar = |chip: &ComposeRecipientChip| -> AnyElement {
            let sz = pal.scaled_px(16.0);
            if chip.is_bot {
                if let Some(ref shape) = chip.avatar_shape {
                    if !shape.trim().is_empty() && !shape.eq_ignore_ascii_case("classic") {
                        return render_grok_bot_avatar(
                            GrokBotAvatarProps::new(
                                chip.bot_id
                                    .as_deref()
                                    .or(chip.username.as_deref())
                                    .unwrap_or(&chip.display_name),
                                shape,
                                format!("#{:06X}", chip.avatar_color & 0xFF_FFFF),
                                sz,
                            )
                            .with_state(GrokPersonaState::Idle)
                            .with_surface_bg(pal.input_bg)
                            .with_paused(true)
                            .with_follow_pointer(false),
                        )
                        .into_any_element();
                    }
                }
                return div()
                    .w(px(sz))
                    .h(px(sz))
                    .rounded_full()
                    .flex_shrink_0()
                    .bg(TdesktopThemePalette::rgba_with_alpha(chip.avatar_color, 0.22))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(tg_icon(IconName::BotRobot, pal.scaled_px(10.0), chip.avatar_color))
                    .into_any_element();
            }
            div()
                .w(px(sz))
                .h(px(sz))
                .rounded_full()
                .flex_shrink_0()
                .bg(rgb(chip.avatar_color))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(pal.scaled_px(8.5)))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .child(chip.initials.chars().take(1).collect::<String>())
                .into_any_element()
        };

        let render_row_avatar = |rec: &AppleComposeRecipient| -> AnyElement {
            let sz = pal.scaled_px(26.0);
            if rec.is_bot {
                if let Some(ref shape) = rec.avatar_shape {
                    if !shape.trim().is_empty() && !shape.eq_ignore_ascii_case("classic") {
                        return render_grok_bot_avatar(
                            GrokBotAvatarProps::new(
                                rec.bot_id
                                    .as_deref()
                                    .or(rec.username.as_deref())
                                    .unwrap_or(&rec.name),
                                shape,
                                format!("#{:06X}", rec.avatar_color & 0xFF_FFFF),
                                sz,
                            )
                            .with_state(GrokPersonaState::Idle)
                            .with_surface_bg(pal.menu_bg)
                            .with_paused(true)
                            .with_follow_pointer(false),
                        )
                        .into_any_element();
                    }
                }
                return div()
                    .w(px(sz))
                    .h(px(sz))
                    .rounded_full()
                    .flex_shrink_0()
                    .bg(TdesktopThemePalette::rgba_with_alpha(rec.avatar_color, 0.20))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(tg_icon(IconName::BotRobot, pal.scaled_px(14.0), rec.avatar_color))
                    .into_any_element();
            }
            div()
                .w(px(sz))
                .h(px(sz))
                .rounded_full()
                .flex_shrink_0()
                .bg(rgb(rec.avatar_color))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(pal.scaled_px(11.0)))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .child(rec.initials.clone())
                .into_any_element()
        };

        let mut bot_section_seen = false;
        let mut contact_section_seen = false;
        let mut dropdown_rows: Vec<AnyElement> = Vec::new();
        for (idx, rec) in recipients.iter().enumerate() {
            if rec.is_bot && !bot_section_seen {
                bot_section_seen = true;
                dropdown_rows.push(
                    div()
                        .px(px(pal.scaled_px(10.0)))
                        .pt(px(pal.scaled_px(6.0)))
                        .pb(px(pal.scaled_px(2.0)))
                        .text_size(px(pal.scaled_px(11.0)))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_secondary))
                        .child("AI Bot")
                        .into_any_element(),
                );
            } else if !rec.is_bot && !contact_section_seen {
                contact_section_seen = true;
                dropdown_rows.push(
                    div()
                        .px(px(pal.scaled_px(10.0)))
                        .pt(px(pal.scaled_px(if bot_section_seen { 8.0 } else { 6.0 })))
                        .pb(px(pal.scaled_px(2.0)))
                        .text_size(px(pal.scaled_px(11.0)))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(lang, "联系人", "Contacts"))
                        .into_any_element(),
                );
            }
            let is_highlighted = idx == effective_highlight;
            let rec_click = rec.clone();
            let badge_label = if rec.is_bot {
                "AI Bot"
            } else {
                tr_str(lang, "联系人", "Contact")
            };
            let badge_fg = if rec.is_bot { 0x8F59F9 } else { pal.text_secondary };
            let badge_bg = if rec.is_bot {
                TdesktopThemePalette::rgba_with_alpha(0x8F59F9, if is_dark { 0.14 } else { 0.10 })
            } else {
                chip_surface_bg
            };
            dropdown_rows.push(
                div()
                    .id(("apple-compose-rec-row", idx))
                    .cursor_pointer()
                    .h(px(pal.scaled_px(36.0)))
                    .w_full()
                    .px(px(pal.scaled_px(10.0)))
                    .rounded(px(pal.scaled_px(8.0)))
                    .when(is_highlighted, |el| el.bg(menu_row_hover))
                    .hover(move |s| s.bg(menu_row_hover))
                    .flex()
                    .items_center()
                    .gap(px(pal.scaled_px(10.0)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.select_apple_compose_candidate(rec_click.clone(), cx);
                        }),
                    )
                    .child(render_row_avatar(rec))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(pal.scaled_px(6.0)))
                            .overflow_hidden()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(pal.scaled_px(13.0)))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(pal.text_primary))
                                    .child(rec.name.clone()),
                            )
                            .when(!rec.subtitle.is_empty(), |el| {
                                el.child(
                                    div()
                                        .truncate()
                                        .text_size(px(pal.scaled_px(11.5)))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(rec.subtitle.clone()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(pal.scaled_px(6.0)))
                            .when(show_group_hint && is_highlighted, |el| {
                                el.child(
                                    div()
                                        .text_size(px(pal.scaled_px(11.0)))
                                        .text_color(rgb(pal.accent))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(tr_str(lang, "添加到群聊", "Add to group")),
                                )
                            })
                            .child(
                                div()
                                    .px(px(pal.scaled_px(6.0)))
                                    .py(px(pal.scaled_px(1.5)))
                                    .rounded(px(pal.scaled_px(5.0)))
                                    .bg(badge_bg)
                                    .text_size(px(pal.scaled_px(10.5)))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(badge_fg))
                                    .child(badge_label),
                            ),
                    )
                    .into_any_element(),
            );
        }

        div()
            .id("apple-compose-stage")
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .bg(rgb(pal.stage_bg))
            .flex()
            .flex_col()
            .overflow_hidden()
            // Top Grok Bot NewChatBar
            .child(
                div()
                    .w_full()
                    .min_h(px(pal.scaled_px(54.0)))
                    .px(px(pal.scaled_px(16.0)))
                    .py(px(pal.scaled_px(10.0)))
                    .border_b_1()
                    .border_color(pal.separator_rgba())
                    .window_control_area(WindowControlArea::Drag)
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id("apple-compose-to-bar")
                            .occlude()
                            .cursor_text()
                            .w_full()
                            .min_h(px(pal.scaled_px(38.0)))
                            .px(px(pal.scaled_px(12.0)))
                            .py(px(pal.scaled_px(4.0)))
                            .rounded(px(pal.scaled_px(12.0)))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(if to_bar_focused {
                                TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.55)
                            } else {
                                pal.separator_rgba()
                            })
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.sidebar.update(cx, |sb, scx| {
                                        sb.search_focused = false;
                                        sb.compose_title_focused = false;
                                        sb.compose_draft.menu_open = true;
                                        scx.notify();
                                    });
                                    this.composer.update(cx, |c, ccx| {
                                        c.is_input_focused = false;
                                        ccx.notify();
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(pal.scaled_px(6.0)))
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_size(px(pal.scaled_px(13.0)))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(pal.text_secondary))
                                            .child(tr_str(lang, "收件人：", "To:")),
                                    )
                                    .children(selected_chips.iter().enumerate().map(|(cidx, chip)| {
                                        let uid = chip.user_id;
                                        div()
                                            .id(("apple-compose-chip", cidx))
                                            .h(px(pal.scaled_px(28.0)))
                                            .pl(px(pal.scaled_px(6.0)))
                                            .pr(px(pal.scaled_px(4.0)))
                                            .rounded_full()
                                            .bg(chip_surface_bg)
                                            .border_1()
                                            .border_color(pal.separator_rgba())
                                            .flex()
                                            .items_center()
                                            .gap(px(pal.scaled_px(5.0)))
                                            .child(render_chip_avatar(chip))
                                            .child(
                                                div()
                                                    .max_w(px(pal.scaled_px(150.0)))
                                                    .truncate()
                                                    .text_size(px(pal.scaled_px(12.5)))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(pal.text_primary))
                                                    .child(chip.display_name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .id(("apple-compose-chip-remove", cidx))
                                                    .cursor_pointer()
                                                    .w(px(pal.scaled_px(18.0)))
                                                    .h(px(pal.scaled_px(18.0)))
                                                    .rounded_full()
                                                    .hover(move |s| s.bg(chip_hover_bg))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            let became_empty = this.sidebar.update(cx, |sb, scx| {
                                                                sb.remove_compose_recipient(uid);
                                                                let empty = sb.compose_draft.selected_recipients.is_empty();
                                                                if sb.compose_draft.selected_recipients.len() < 2 {
                                                                    sb.compose_title_focused = false;
                                                                }
                                                                if empty {
                                                                    sb.compose_draft.menu_open = true;
                                                                }
                                                                scx.notify();
                                                                empty
                                                            });
                                                            if became_empty {
                                                                this.composer.update(cx, |c, ccx| {
                                                                    c.is_input_focused = false;
                                                                    ccx.notify();
                                                                });
                                                            }
                                                            this.sync_compose_state_to_composer(
                                                                true, cx,
                                                            );
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child(tg_icon(
                                                        IconName::Close,
                                                        pal.scaled_px(9.5),
                                                        pal.text_secondary,
                                                    )),
                                            )
                                    }))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(pal.scaled_px(120.0)))
                                            .h(px(pal.scaled_px(26.0)))
                                            .flex()
                                            .items_center()
                                            .overflow_hidden()
                                            .when(!query.is_empty(), |el| {
                                                el.child(
                                                    div()
                                                        .text_size(px(pal.scaled_px(13.0)))
                                                        .text_color(rgb(pal.text_primary))
                                                        .child(query.clone()),
                                                )
                                            })
                                            .when_some(preedit.clone(), |el, p| {
                                                el.child(
                                                    div()
                                                        .border_b_1()
                                                        .border_color(rgb(pal.accent))
                                                        .text_size(px(pal.scaled_px(13.0)))
                                                        .text_color(rgb(pal.text_primary))
                                                        .child(p),
                                                )
                                            })
                                            .when(to_bar_focused, |el| {
                                                el.child(
                                                    div()
                                                        .w(px(1.5))
                                                        .h(px(pal.scaled_px(15.0)))
                                                        .rounded_full()
                                                        .flex_shrink_0()
                                                        .bg(rgb(pal.accent)),
                                                )
                                            })
                                            .when(query.is_empty() && preedit.is_none(), |el| {
                                                let hint = if selected_chips.is_empty() {
                                                    tr_str(
                                                        lang,
                                                        "输入联系人、@用户名或选择 AI Bot…",
                                                        "Start a chat with a contact or AI Bot…",
                                                    )
                                                } else {
                                                    tr_str(
                                                        lang,
                                                        "继续添加成员以创建群聊…",
                                                        "Add more members to create a group…",
                                                    )
                                                };
                                                el.child(
                                                    div()
                                                        .ml(px(2.0))
                                                        .truncate()
                                                        .text_size(px(pal.scaled_px(13.0)))
                                                        .text_color(
                                                            TdesktopThemePalette::rgba_with_alpha(
                                                                pal.text_secondary,
                                                                0.72,
                                                            ),
                                                        )
                                                        .child(hint),
                                                )
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(pal.scaled_px(6.0)))
                                    .when_some(single_chip_for_dm, |el, single_chip| {
                                        el.child(
                                            div()
                                                .id("apple-compose-open-dm-btn")
                                                .occlude()
                                                .cursor_pointer()
                                                .h(px(pal.scaled_px(26.0)))
                                                .px(px(pal.scaled_px(10.0)))
                                                .rounded_full()
                                                .bg(TdesktopThemePalette::rgba_with_alpha(
                                                    pal.accent,
                                                    if is_dark { 0.20 } else { 0.12 },
                                                ))
                                                .hover(move |s| {
                                                    s.bg(TdesktopThemePalette::rgba_with_alpha(
                                                        pal.accent,
                                                        if is_dark { 0.30 } else { 0.20 },
                                                    ))
                                                })
                                                .flex()
                                                .items_center()
                                                .text_size(px(pal.scaled_px(11.5)))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(pal.accent))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.commit_single_compose_recipient_to_dm(
                                                            single_chip.clone(),
                                                            cx,
                                                        );
                                                    }),
                                                )
                                                .child(tr_str(lang, "直接进入对话", "Open Chat")),
                                        )
                                    })
                                    .child(
                                        div()
                                            .id("apple-compose-close-btn")
                                            .occlude()
                                            .cursor_pointer()
                                            .w(px(pal.scaled_px(24.0)))
                                            .h(px(pal.scaled_px(24.0)))
                                            .rounded_full()
                                            .flex_shrink_0()
                                            .hover(move |s| s.bg(chip_hover_bg))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.ime.clear_preedit();
                                                    this.sidebar.update(cx, |sb, scx| {
                                                        sb.cancel_compose_mode();
                                                        scx.notify();
                                                    });
                                                    this.exit_compose_or_modal_isolation(true, cx);
                                                    cx.notify();
                                                }),
                                            )
                                            .child(tg_icon(
                                                IconName::Close,
                                                pal.scaled_px(12.0),
                                                pal.text_secondary,
                                            )),
                                    ),
                            ),
                    ),
            )
            // Group title preview banner when >=2 recipients selected
            .when(selected_chips.len() >= 2, |stage| {
                let effective_title = if custom_group_title.trim().is_empty() {
                    auto_group_title.clone()
                } else {
                    custom_group_title.clone()
                };
                stage.child(
                    div()
                        .w_full()
                        .px(px(pal.scaled_px(18.0)))
                        .py(px(pal.scaled_px(7.0)))
                        .border_b_1()
                        .border_color(pal.separator_rgba())
                        .bg(if is_dark {
                            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.03)
                        } else {
                            TdesktopThemePalette::rgba_with_alpha(0x1C1C1C, 0.025)
                        })
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(pal.scaled_px(12.0)))
                        .child(
                            div()
                                .id("apple-compose-group-title-field")
                                .cursor_text()
                                .flex_1()
                                .min_w(px(0.0))
                                .px(px(pal.scaled_px(8.0)))
                                .py(px(pal.scaled_px(4.0)))
                                .rounded(px(pal.scaled_px(7.0)))
                                .border_1()
                                .border_color(if title_focused {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.55)
                                } else {
                                    TdesktopThemePalette::rgba_with_alpha(0x000000, 0.0)
                                })
                                .flex()
                                .items_center()
                                .gap(px(pal.scaled_px(6.0)))
                                .overflow_hidden()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.sidebar.update(cx, |sb, scx| {
                                            sb.search_focused = false;
                                            sb.compose_title_focused = true;
                                            scx.notify();
                                        });
                                        this.composer.update(cx, |c, ccx| {
                                            c.is_input_focused = false;
                                            ccx.notify();
                                        });
                                        cx.notify();
                                    }),
                                )
                                .child(tg_icon(
                                    IconName::GroupUsers,
                                    pal.scaled_px(14.0),
                                    pal.accent,
                                ))
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(px(pal.scaled_px(12.0)))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(tr_str(lang, "群聊名称：", "Group name: ")),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .flex()
                                        .items_center()
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(pal.scaled_px(12.5)))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(pal.text_primary))
                                                .child(if title_focused {
                                                    custom_group_title.clone()
                                                } else {
                                                    effective_title
                                                }),
                                        )
                                        .when_some(title_preedit.clone(), |el, p| {
                                            el.child(
                                                div()
                                                    .border_b_1()
                                                    .border_color(rgb(pal.accent))
                                                    .text_size(px(pal.scaled_px(12.5)))
                                                    .text_color(rgb(pal.text_primary))
                                                    .child(p),
                                            )
                                        })
                                        .when(title_focused, |el| {
                                            el.child(
                                                div()
                                                    .w(px(1.5))
                                                    .h(px(pal.scaled_px(13.0)))
                                                    .rounded_full()
                                                    .flex_shrink_0()
                                                    .bg(rgb(pal.accent)),
                                            )
                                        })
                                        .when(
                                            title_focused
                                                && custom_group_title.is_empty()
                                                && title_preedit.is_none(),
                                            |el| {
                                                el.child(
                                                    div()
                                                        .ml(px(3.0))
                                                        .truncate()
                                                        .text_size(px(pal.scaled_px(12.0)))
                                                        .text_color(rgb(pal.text_secondary))
                                                        .child(auto_group_title.clone()),
                                                )
                                            },
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(pal.scaled_px(11.5)))
                                .text_color(rgb(pal.text_secondary))
                                .child(tr_str(
                                    lang,
                                    "发送首条消息将自动创建群聊",
                                    "Send first message to auto-create group",
                                )),
                        ),
                )
            })
            // Subtle dismissible AI Bot collaboration notice when >=2 recipients include a bot
            .when(
                selected_chips.len() >= 2 && has_bot_chip && !bot_notice_dismissed,
                |stage| {
                    stage.child(
                        div()
                            .w_full()
                            .px(px(pal.scaled_px(18.0)))
                            .py(px(pal.scaled_px(6.0)))
                            .border_b_1()
                            .border_color(pal.separator_rgba())
                            .bg(TdesktopThemePalette::rgba_with_alpha(
                                0x8F59F9,
                                if is_dark { 0.08 } else { 0.06 },
                            ))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(tg_icon(IconName::BotRobot, pal.scaled_px(13.0), 0x8F59F9))
                                    .child(
                                        div()
                                            .text_size(px(pal.scaled_px(11.5)))
                                            .text_color(rgb(pal.text_secondary))
                                            .child(tr_str(
                                                lang,
                                                "群聊中的 AI Bot 会协作读取群内消息",
                                                "AI Bots in this group collaboratively read group messages",
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .id("apple-compose-bot-notice-close")
                                    .cursor_pointer()
                                    .w(px(pal.scaled_px(20.0)))
                                    .h(px(pal.scaled_px(20.0)))
                                    .rounded_full()
                                    .hover(move |s| s.bg(chip_hover_bg))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.sidebar.update(cx, |sb, scx| {
                                                sb.compose_bot_notice_dismissed = true;
                                                scx.notify();
                                            });
                                            cx.notify();
                                        }),
                                    )
                                    .child(tg_icon(
                                        IconName::Close,
                                        pal.scaled_px(10.0),
                                        pal.text_secondary,
                                    )),
                            ),
                    )
                },
            )
            // Grok Bot `izn` inline error recovery banner
            .when_some(creation_error, |stage, err| {
                let err_draft = err.failed_draft.clone();
                stage.child(
                    div()
                        .id("apple-compose-error-banner")
                        .mx(px(pal.scaled_px(16.0)))
                        .mt(px(pal.scaled_px(10.0)))
                        .px(px(pal.scaled_px(12.0)))
                        .py(px(pal.scaled_px(10.0)))
                        .rounded(px(pal.scaled_px(10.0)))
                        .bg(TdesktopThemePalette::rgba_with_alpha(
                            pal.danger,
                            if is_dark { 0.14 } else { 0.09 },
                        ))
                        .border_1()
                        .border_color(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.35))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(pal.scaled_px(12.5)))
                                .text_color(rgb(pal.text_primary))
                                .child(err.message),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .id("apple-compose-retry-create-btn")
                                        .cursor_pointer()
                                        .px(px(pal.scaled_px(10.0)))
                                        .py(px(pal.scaled_px(5.0)))
                                        .rounded(px(pal.scaled_px(7.0)))
                                        .bg(rgb(pal.accent))
                                        .hover(move |s| s.bg(rgb(pal.accent_hover)))
                                        .text_size(px(pal.scaled_px(11.5)))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.retry_compose_group_creation(cx);
                                            }),
                                        )
                                        .child(tr_str(lang, "重试创建群组", "Retry")),
                                )
                                .child(
                                    div()
                                        .id("apple-compose-restore-draft-btn")
                                        .cursor_pointer()
                                        .px(px(pal.scaled_px(10.0)))
                                        .py(px(pal.scaled_px(5.0)))
                                        .rounded(px(pal.scaled_px(7.0)))
                                        .bg(chip_surface_bg)
                                        .hover(move |s| s.bg(chip_hover_bg))
                                        .text_size(px(pal.scaled_px(11.5)))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(pal.text_primary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                let draft = err_draft.clone();
                                                this.sidebar.update(cx, |sb, scx| {
                                                    sb.compose_draft.creation_error = None;
                                                    scx.notify();
                                                });
                                                this.composer.update(cx, |c, ccx| {
                                                    if !draft.trim().is_empty() {
                                                        c.set_text(draft);
                                                    }
                                                    c.is_input_focused = true;
                                                    ccx.notify();
                                                });
                                                this.sync_compose_state_to_composer(true, cx);
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "恢复草稿", "Restore Draft")),
                                )
                                .child(
                                    div()
                                        .id("apple-compose-dismiss-error-btn")
                                        .cursor_pointer()
                                        .px(px(pal.scaled_px(8.0)))
                                        .py(px(pal.scaled_px(5.0)))
                                        .rounded(px(pal.scaled_px(7.0)))
                                        .hover(move |s| s.bg(chip_hover_bg))
                                        .text_size(px(pal.scaled_px(11.5)))
                                        .text_color(rgb(pal.text_secondary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.sidebar.update(cx, |sb, scx| {
                                                    sb.compose_draft.creation_error = None;
                                                    scx.notify();
                                                });
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "关闭", "Dismiss")),
                                ),
                        ),
                )
            })
            // Clean minimal stage canvas with floating elevated recipient menu (`sand-new-chat-menu`)
            .child(
                div()
                    .id("apple-compose-canvas")
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .px(px(pal.scaled_px(16.0)))
                    .pt(px(pal.scaled_px(10.0)))
                    .pb(px(pal.scaled_px(16.0)))
                    .flex()
                    .flex_col()
                    .items_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.sidebar.update(cx, |sb, scx| {
                                sb.compose_title_focused = false;
                                sb.compose_draft.menu_open = false;
                                scx.notify();
                            });
                            cx.notify();
                        }),
                    )
                    .when(menu_open && pending_creation.is_none(), |canvas| {
                        canvas.child(
                            div()
                                .id("apple-compose-dropdown-menu")
                                .occlude()
                                .w_full()
                                .max_w(px(pal.scaled_px(680.0)))
                                .rounded(px(pal.scaled_px(14.0)))
                                .bg(menu_surface_bg)
                                .border_1()
                                .border_color(pal.separator_rgba())
                                .when(!is_dark, |el| el.shadow(pal.popover_shadows()))
                                .flex()
                                .flex_col()
                                .overflow_hidden()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .when(query.trim().is_empty(), |menu| {
                                    menu.child(
                                        div()
                                            .w_full()
                                            .px(px(pal.scaled_px(10.0)))
                                            .pt(px(pal.scaled_px(10.0)))
                                            .pb(px(pal.scaled_px(8.0)))
                                            .border_b_1()
                                            .border_color(pal.separator_rgba())
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(px(pal.scaled_px(6.0)))
                                            .child(
                                                quick_pill(
                                                    "apple-compose-pill-bot",
                                                    IconName::BotRobot,
                                                    0x8F59F9,
                                                    tr_str(lang, "创建 AI Bot", "New AI Bot"),
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.ime.clear_preedit();
                                                        this.enter_compose_or_modal_isolation(cx);
                                                        this.sidebar.update(cx, |sb, scx| {
                                                            sb.open_add_contact_modal(
                                                                AddContactKind::AiBot,
                                                            );
                                                            scx.notify();
                                                        });
                                                        this.sync_compose_state_to_composer(
                                                            true, cx,
                                                        );
                                                        cx.notify();
                                                    }),
                                                ),
                                            )
                                            .child(
                                                quick_pill(
                                                    "apple-compose-pill-friend",
                                                    IconName::ContactsUsers,
                                                    0x0A84FF,
                                                    tr_str(lang, "添加好友", "Add Friend"),
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.ime.clear_preedit();
                                                        this.enter_compose_or_modal_isolation(cx);
                                                        this.sidebar.update(cx, |sb, scx| {
                                                            sb.open_add_contact_modal(
                                                                AddContactKind::Friend,
                                                            );
                                                            scx.notify();
                                                        });
                                                        this.sync_compose_state_to_composer(
                                                            true, cx,
                                                        );
                                                        cx.notify();
                                                    }),
                                                ),
                                            )
                                            .when(!is_group_mode && selected_chips.is_empty(), |row| {
                                                row.child(
                                                    quick_pill(
                                                        "apple-compose-pill-group",
                                                        IconName::GroupUsers,
                                                        0x30D158,
                                                        tr_str(lang, "新建群聊", "New Group"),
                                                    )
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.ime.clear_preedit();
                                                            this.enter_compose_or_modal_isolation(cx);
                                                            this.sidebar.update(cx, |sb, scx| {
                                                                sb.open_compose_group_mode();
                                                                scx.notify();
                                                            });
                                                            this.sync_compose_state_to_composer(
                                                                true, cx,
                                                            );
                                                            cx.notify();
                                                        }),
                                                    ),
                                                )
                                            })
                                            .child(
                                                quick_pill(
                                                    "apple-compose-pill-channel",
                                                    IconName::ChannelMegaphone,
                                                    0xFF9F0A,
                                                    tr_str(lang, "新建频道", "New Channel"),
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.ime.clear_preedit();
                                                        let title = tr_str(
                                                            this.active_lang_id(),
                                                            "新建频道",
                                                            "New Channel",
                                                        );
                                                        this.open_modal_dialog(
                                                            title,
                                                            ModalDialogKind::NewChannel,
                                                            420.0,
                                                            380.0,
                                                            cx,
                                                        );
                                                        this.sidebar.update(cx, |sb, scx| {
                                                            sb.cancel_compose_mode();
                                                            scx.notify();
                                                        });
                                                        this.enter_compose_or_modal_isolation(cx);
                                                        cx.notify();
                                                    }),
                                                ),
                                            ),
                                    )
                                })
                                .child(
                                    div()
                                        .id("apple-compose-recipients-scroll")
                                        .w_full()
                                        .max_h(px(pal.scaled_px(360.0)))
                                        .p(px(pal.scaled_px(6.0)))
                                        .overflow_y_scroll()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .when(dropdown_rows.is_empty(), |col| {
                                            if !query.trim().is_empty() {
                                                col.child(
                                                    div()
                                                        .id("apple-compose-add-query-friend")
                                                        .cursor_pointer()
                                                        .px(px(pal.scaled_px(10.0)))
                                                        .py(px(pal.scaled_px(8.0)))
                                                        .rounded(px(pal.scaled_px(8.0)))
                                                        .hover(move |s| s.bg(menu_row_hover))
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(pal.scaled_px(10.0)))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(|this, _, _, cx| {
                                                                cx.stop_propagation();
                                                                this.open_add_friend_from_compose_query(cx);
                                                            }),
                                                        )
                                                        .child(
                                                            div()
                                                                .w(px(pal.scaled_px(26.0)))
                                                                .h(px(pal.scaled_px(26.0)))
                                                                .rounded_full()
                                                                .bg(TdesktopThemePalette::rgba_with_alpha(
                                                                    pal.accent,
                                                                    0.16,
                                                                ))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .child(tg_icon(
                                                                    IconName::ContactsUsers,
                                                                    pal.scaled_px(14.0),
                                                                    pal.accent,
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .min_w(px(0.0))
                                                                .truncate()
                                                                .text_size(px(pal.scaled_px(13.0)))
                                                                .font_weight(FontWeight::MEDIUM)
                                                                .text_color(rgb(pal.text_primary))
                                                                .child(format!(
                                                                    "{} \"{}\"",
                                                                    tr_str(
                                                                        lang,
                                                                        "添加联系人",
                                                                        "Add contact",
                                                                    ),
                                                                    query.trim()
                                                                )),
                                                        ),
                                                )
                                            } else {
                                                col.items_center().justify_center().py(px(20.0)).child(
                                                    div()
                                                        .text_size(px(pal.scaled_px(12.5)))
                                                        .text_color(rgb(pal.text_secondary))
                                                        .child(if selected_chips.is_empty() {
                                                            tr_str(
                                                                lang,
                                                                "暂无可选联系人或 AI Bot",
                                                                "No contacts or AI Bots available",
                                                            )
                                                        } else {
                                                            tr_str(
                                                                lang,
                                                                "所有联系人均已添加",
                                                                "All available contacts selected",
                                                            )
                                                        }),
                                                )
                                            }
                                        })
                                        .children(dropdown_rows),
                                ),
                        )
                    })
                    .when_some(pending_creation, |canvas, pending| {
                        canvas.child(
                            div()
                                .flex_1()
                                .w_full()
                                .max_w(px(pal.scaled_px(680.0)))
                                .flex()
                                .flex_col()
                                .justify_end()
                                .items_end()
                                .gap(px(pal.scaled_px(8.0)))
                                .child(
                                    div()
                                        .max_w(px(pal.scaled_px(440.0)))
                                        .px(px(pal.scaled_px(14.0)))
                                        .py(px(pal.scaled_px(9.0)))
                                        .rounded(px(pal.scaled_px(18.0)))
                                        .bg(pal.msg_out_gradient())
                                        .text_size(px(pal.scaled_px(14.0)))
                                        .text_color(rgb(pal.msg_out_fg))
                                        .child(pending.initial_message),
                                )
                                .child(
                                    div()
                                        .text_size(px(pal.scaled_px(11.5)))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!(
                                            "{} · {}",
                                            pending.title,
                                            tr_str(lang, "正在创建群聊…", "Creating group…")
                                        )),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}
