//! Overlay and workspace modal rendering helpers for `AppShell`.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::computer_contracts::{PendingComputerCommandDto, RemoteComputerOp};
use gpui_telegram_proto::contracts::chats::CreateChatRequest;
use gpui_telegram_proto::miniapp_contracts::{
    MiniAppControlMode, MiniAppTakeoverEvent, GROK_AVATAR_SHAPES,
};

use crate::components::grok_bot_avatar::{
    render_grok_bot_avatar, trigger_grok_avatar_bounce, trigger_grok_avatar_spin,
    GrokBotAvatarProps, GrokPersonaState, GROK_COLOR_PALETTE,
};
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::shell_actions::localize_call_state;
use crate::views::app_shell::shell_panels::{render_empty_panel_notice, render_panel_header};
use crate::views::app_shell::shell_truth::{
    derive_initials, format_authoritative_timestamp, peer_subtitle_for_dialog,
};
use crate::views::app_shell::modal_container::{
    macos_modal_backdrop_rgba, macos_modal_card, macos_popover_surface, macos_primary_pill_btn,
    macos_secondary_pill_btn,
};
use crate::views::app_shell::peer_avatar_color;
use crate::views::boxes::modal_types::ModalDialogKind;
use crate::views::chat_sidebar::{AddContactKind, CreationInputField, SidebarCreationModal};
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

    /// Renders the minimized MiniApp pill, open MiniApp host sheet, confirmation/ToS modals,
    /// `BotProfileBox`, and NL Bot creator modal.
    pub fn render_miniapp_overlays(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut layers = Vec::new();
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id();

        let sidebar_modal_open = {
            let sb = self.sidebar.read(cx);
            sb.is_contacts_modal_open
                || sb.active_creation_modal.is_some()
                || sb.is_account_popover_open
        };
        let has_top_modal = self.miniapps.pending_miniapp_confirm.is_some()
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
            || self.modal_container.read(cx).is_modal_active();

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
            if needs_sync || needs_park_pip {
                ov.update(cx, |w, _| {
                    w.external_modal_occluding = has_top_modal;
                    if should_hide_native {
                        w.hide_native_webview();
                    } else if needs_park_pip {
                        w.park_native_webview_for_pip();
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
                                                    if is_takeover && !b_un_close.is_empty() {
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
                                                if let Some(ref mut ps) =
                                                    this.miniapps.floating_pip_status
                                                {
                                                    ps.mode = MiniAppControlMode::UserTakeover;
                                                    ps.status_text =
                                                        "用户正在手动接管小程序".to_string();
                                                    ps.active_tool = None;
                                                }
                                                if !b_un_take.is_empty() {
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
                                                if !b_un_fin.is_empty() {
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
                                            this.miniapps.is_miniapp_minimized = false;
                                            if let Some(ref ov) = this.miniapps.miniapp_overlay {
                                                ov.update(cx, |w, wcx| {
                                                    w.is_open = true;
                                                    w.is_floating_pip = false;
                                                    wcx.notify();
                                                });
                                            }
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
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| { this.miniapps.create_bot_box = None; cx.notify(); }))
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
                                                    this.miniapps.miniapp_tos_modal_open = true;
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
                                                this.sync_selected_chat_inspector_and_stage(cid, cx);
                                            } else {
                                                let req = CreateChatRequest { title: click_name.clone(), chat_type: Some(serde_json::json!(0)), peer_user_id: Some(uid), ..Default::default() };
                                                let client = this.client.clone();
                                                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                                                    if let Ok(created) = client.create_chat(&req).await {
                                                        let _ = weak.update(app, |s, cx| {
                                                            s.sidebar.update(cx, |sb, _| sb.select_dialog_from_user(created.chat_id as u64));
                                                            s.refresh_chats(cx);
                                                        });
                                                    }
                                                }).detach();
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
                                            this.window_runtime.enqueue_toast(msg, 2600);
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
                    .child(menu_row("account-popover-voice", IconName::Mic, tr_str(lang, "语音对话", "Voice Chat"), pal.text_primary, false, |this, cx| this.activate_nav_destination(NavDestination::TalkMode, cx)))
                    .child(menu_row("account-popover-fabushi", IconName::BotRobot, tr_str(lang, "新建智能体 · Fabushi Bot", "New Fabushi Bot"), pal.accent, false, |this, cx| this.open_fabushi_creator_modal(cx)))
                    .child(menu_row("account-popover-botfather", IconName::BotRobot, tr_str(lang, "BotFather · 创建机器人/小程序", "BotFather · Create Bot/App"), pal.accent, false, |this, cx| this.open_botfather_chat(cx)))
                    .child(menu_row("account-popover-miniapps", IconName::FolderBots, tr_str(lang, "小程序中心", "Mini Apps Center"), pal.text_primary, false, |this, cx| this.open_miniapps_center(cx)))
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
                                this.active_panel = ActiveWorkspacePanel::Settings;
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
