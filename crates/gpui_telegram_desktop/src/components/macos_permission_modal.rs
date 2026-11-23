//! Gemini-style macOS System Permissions modal & Settings card with native
//! drag-to-System-Settings floating HUD, live status updates, restart-to-apply
//! detection, and stale ad-hoc signature recovery.

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::i18n::tr_str;
use crate::platform::mac::macos_permissions_drag::{
    MacPermissionsSnapshot, PermissionKind, ScreenPermissionState, hide_drag_hud,
    open_permission_with_drag_hud, relaunch_current_app, request_permissions_refresh,
    reset_stale_permission_and_reopen,
};
use crate::theme::palette::TdesktopThemePalette;

const BADGE_BLUE: u32 = 0x2563EB;
const BADGE_ORANGE: u32 = 0xEA580C;
const BADGE_AMBER: u32 = 0xD97706;

fn pill_badge(label: &'static str, color_hex: u32) -> Div {
    div()
        .px(px(7.0))
        .py(px(1.5))
        .rounded(px(999.0))
        .bg(TdesktopThemePalette::rgba_with_alpha(color_hex, 0.16))
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(color_hex))
        .child(label)
}

fn action_pill(
    id: &'static str,
    label: &'static str,
    emphasized: bool,
    pal: TdesktopThemePalette,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .px(px(14.0))
        .py(px(6.5))
        .rounded(px(999.0))
        .bg(if emphasized {
            rgb(pal.accent).into()
        } else {
            TdesktopThemePalette::rgba_with_alpha(pal.text_primary, 0.08)
        })
        .border_1()
        .border_color(if emphasized {
            rgb(pal.accent).into()
        } else {
            TdesktopThemePalette::rgba_with_alpha(pal.border_fg, 0.9)
        })
        .hover(move |s| {
            if emphasized {
                s.bg(rgb(pal.accent_hover))
            } else {
                s.bg(rgb(pal.hover_bg))
            }
        })
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(if emphasized {
            pal.active_button_fg
        } else {
            pal.text_primary
        }))
        .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
            cx.stop_propagation();
            on_click(ev, window, cx);
        })
        .child(label)
}

fn pending_restart_pill(label: &'static str) -> Div {
    div()
        .px(px(14.0))
        .py(px(6.5))
        .rounded(px(999.0))
        .bg(TdesktopThemePalette::rgba_with_alpha(BADGE_AMBER, 0.14))
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(BADGE_AMBER, 0.38))
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(BADGE_AMBER))
        .child(label)
}

fn done_pill(label: &'static str, pal: TdesktopThemePalette) -> Div {
    div()
        .px(px(14.0))
        .py(px(6.5))
        .rounded(px(999.0))
        .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.12))
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.35))
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(pal.success))
        .child(label)
}

/// Renders the grouped 2-row permission list (Accessibility + Screen & System Audio)
/// with inline restart-to-apply and stale-signature reset banners.
pub fn render_permission_rows_card(
    id_prefix: &'static str,
    snap: &MacPermissionsSnapshot,
    pal: TdesktopThemePalette,
    lang: &str,
) -> Div {
    let lang_ax_open = lang.to_string();
    let lang_ax_reset = lang.to_string();
    let lang_scr_open = lang.to_string();
    let lang_scr_reset = lang.to_string();

    let ax_ok = snap.accessibility_granted;
    let screen_state = snap.screen_state;
    let screen_ok = screen_state == ScreenPermissionState::GrantedActive;
    let screen_needs_restart = screen_state == ScreenPermissionState::GrantedNeedsRestart;
    let app_name = snap.signing.display_app_name();

    let ax_row = div()
        .px(px(16.0))
        .py(px(13.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(14.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(13.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(lang, "无障碍 / 辅助功能权限", "Accessibility")),
                                )
                                .child(pill_badge(
                                    tr_str(lang, "必需", "Required"),
                                    BADGE_BLUE,
                                ))
                                .when(ax_ok, |r| {
                                    r.child(pill_badge(
                                        tr_str(lang, "已生效", "Active"),
                                        pal.success,
                                    ))
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(tr_str(
                                    lang,
                                    "远程控制时，Fabushi 会使用辅助功能权限来执行鼠标点击与键盘输入。",
                                    "Fabushi uses Accessibility to perform remote mouse clicks and keyboard input.",
                                )),
                        ),
                )
                .child(if ax_ok {
                    done_pill(tr_str(lang, "完成", "Done"), pal).into_any_element()
                } else {
                    action_pill(
                        if id_prefix == "modal" {
                            "perm-modal-ax-open-btn"
                        } else {
                            "perm-settings-ax-open-btn"
                        },
                        tr_str(lang, "在“设置”中开启", "Open in Settings"),
                        false,
                        pal,
                        move |_, _, _| {
                            open_permission_with_drag_hud(
                                PermissionKind::Accessibility,
                                &lang_ax_open,
                            );
                        },
                    )
                    .into_any_element()
                }),
        )
        .when(!ax_ok && snap.stale_ax_suspected, |col| {
            col.child(
                div()
                    .pt(px(9.0))
                    .border_t_1()
                    .border_color(TdesktopThemePalette::rgba_with_alpha(pal.border_fg, 0.7))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.5))
                            .text_color(rgb(BADGE_AMBER))
                            .child(tr_str(
                                lang,
                                "设置中显示已开启但未生效？可能是应用重签导致旧授权失效，请重置后拖入应用。",
                                "Enabled in Settings but not detected? Reset the stale signature entry and re-drag the app.",
                            )),
                    )
                    .child(action_pill(
                        if id_prefix == "modal" {
                            "perm-modal-ax-reset-btn"
                        } else {
                            "perm-settings-ax-reset-btn"
                        },
                        tr_str(lang, "重置并重新拖入", "Reset & Re-drag"),
                        false,
                        pal,
                        move |_, _, _| {
                            reset_stale_permission_and_reopen(
                                PermissionKind::Accessibility,
                                &lang_ax_reset,
                            );
                        },
                    )),
            )
        });

    let restart_label_str = if lang.starts_with("zh") {
        format!("重启 {app_name}")
    } else {
        format!("Restart {app_name}")
    };

    let screen_row = div()
        .px(px(16.0))
        .py(px(13.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(14.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(13.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(
                                            lang,
                                            "屏幕和系统音频",
                                            "Screen & System Audio",
                                        )),
                                )
                                .child(pill_badge(
                                    tr_str(lang, "必需", "Required"),
                                    BADGE_ORANGE,
                                ))
                                .when(screen_ok, |r| {
                                    r.child(pill_badge(
                                        tr_str(lang, "已生效", "Active"),
                                        pal.success,
                                    ))
                                })
                                .when(screen_needs_restart, |r| {
                                    r.child(pill_badge(
                                        tr_str(lang, "需重启生效", "Restart Needed"),
                                        BADGE_AMBER,
                                    ))
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(tr_str(
                                    lang,
                                    "Fabushi 运行“远程控制”功能时会使用屏幕录制权限来实时查看桌面画面。",
                                    "Fabushi uses Screen Recording to stream the live desktop during Remote Control.",
                                )),
                        ),
                )
                .child(if screen_ok {
                    done_pill(tr_str(lang, "完成", "Done"), pal).into_any_element()
                } else if screen_needs_restart {
                    pending_restart_pill(tr_str(lang, "已开启", "Enabled")).into_any_element()
                } else {
                    action_pill(
                        if id_prefix == "modal" {
                            "perm-modal-screen-open-btn"
                        } else {
                            "perm-settings-screen-open-btn"
                        },
                        tr_str(lang, "在“设置”中开启", "Open in Settings"),
                        false,
                        pal,
                        move |_, _, _| {
                            open_permission_with_drag_hud(
                                PermissionKind::ScreenRecording,
                                &lang_scr_open,
                            );
                        },
                    )
                    .into_any_element()
                }),
        )
        .when(screen_needs_restart, |col| {
            col.child(
                div()
                    .pt(px(10.0))
                    .border_t_1()
                    .border_color(TdesktopThemePalette::rgba_with_alpha(pal.border_fg, 0.7))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(pal.text_primary))
                            .child(tr_str(
                                lang,
                                "重启后可让“录屏与系统录音”权限生效。",
                                "Restart to let \"Screen & System Audio Recording\" take effect.",
                            )),
                    )
                    .child(
                        div()
                            .id(if id_prefix == "modal" {
                                "perm-modal-restart-btn"
                            } else {
                                "perm-settings-restart-btn"
                            })
                            .cursor_pointer()
                            .px(px(14.0))
                            .py(px(6.5))
                            .rounded(px(999.0))
                            .bg(rgb(pal.accent))
                            .hover(move |s| s.bg(rgb(pal.accent_hover)))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.active_button_fg))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                                if relaunch_current_app() {
                                    cx.quit();
                                }
                            })
                            .child(restart_label_str),
                    ),
            )
        })
        .when(!screen_ok && !screen_needs_restart && snap.stale_screen_suspected, |col| {
            col.child(
                div()
                    .pt(px(9.0))
                    .border_t_1()
                    .border_color(TdesktopThemePalette::rgba_with_alpha(pal.border_fg, 0.7))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.5))
                            .text_color(rgb(BADGE_AMBER))
                            .child(tr_str(
                                lang,
                                "设置中显示已开启但未识别？旧版本签名与当前应用不一致，请重置后重新拖入。",
                                "Already ON in Settings but unrecognized? Previous build signature mismatched; reset and re-drag.",
                            )),
                    )
                    .child(action_pill(
                        if id_prefix == "modal" {
                            "perm-modal-screen-reset-btn"
                        } else {
                            "perm-settings-screen-reset-btn"
                        },
                        tr_str(lang, "重置并重新拖入", "Reset & Re-drag"),
                        false,
                        pal,
                        move |_, _, _| {
                            reset_stale_permission_and_reopen(
                                PermissionKind::ScreenRecording,
                                &lang_scr_reset,
                            );
                        },
                    )),
            )
        });

    div()
        .rounded(px(14.0))
        .bg(rgb(pal.card_bg))
        .border_1()
        .border_color(rgb(pal.border_fg))
        .flex()
        .flex_col()
        .child(ax_row)
        .child(div().h(px(1.0)).mx(px(16.0)).bg(rgb(pal.border_fg)))
        .child(screen_row)
}

/// Renders the centered Gemini-style "允许授予系统权限" modal overlay.
pub fn render_gemini_permission_modal(
    snap: &MacPermissionsSnapshot,
    pal: TdesktopThemePalette,
    lang: &str,
    on_dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_complete: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let all_ready = snap.all_ready();
    let app_name = snap.signing.display_app_name();
    let bundle_path = snap.signing.drag_target_path().to_string();

    div()
        .id("gemini-permission-modal-backdrop")
        .absolute()
        .inset_0()
        .bg(rgba(0x090D16B8))
        .flex()
        .items_center()
        .justify_center()
        .p(px(20.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .w(px(520.0))
                .max_w_full()
                .p(px(24.0))
                .rounded(px(20.0))
                .bg(rgb(pal.modal_bg))
                .border_1()
                .border_color(rgb(pal.border_fg))
                .shadow_lg()
                .flex()
                .flex_col()
                .gap(px(16.0))
                // Centered Gemini-style title & subtitle
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(12.0))
                        .child(
                            div()
                                .text_size(px(18.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(
                                    lang,
                                    "允许授予系统权限",
                                    "Allow System Permissions",
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(rgb(pal.text_secondary))
                                .text_center()
                                .child(if lang.starts_with("zh") {
                                    format!(
                                        "允许 {app_name} 访问系统信息，以启用远程控制与桌面查看。点击“在设置中开启”后，可将悬浮卡片中的 {app_name} 直接拖入系统设置列表，完成后返回此应用继续。"
                                    )
                                } else {
                                    format!(
                                        "Allow {app_name} to access system permissions for Remote Control. After clicking Open in Settings, drag {app_name} from the floating helper card into System Settings, then return here."
                                    )
                                }),
                        ),
                )
                // Grouped permission rows card
                .child(render_permission_rows_card("modal", snap, pal, lang))
                // Active floating drag HUD indicator strip
                .when(snap.hud_visible, |card| {
                    card.child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .rounded(px(10.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                            .border_1()
                            .border_color(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.3))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal.text_primary))
                                    .child(tr_str(
                                        lang,
                                        "悬浮拖拽卡片已就绪：切换至“系统设置”后，可直接按住卡片中的 Fabushi 拖入授权列表。",
                                        "The floating drag card is active — switch to System Settings and drag the app tile into the list.",
                                    )),
                            )
                            .child(action_pill(
                                "perm-modal-hide-hud-btn",
                                tr_str(lang, "隐藏卡片", "Hide Card"),
                                false,
                                pal,
                                |_, _, _| {
                                    hide_drag_hud();
                                },
                            )),
                    )
                })
                // Subtle diagnostic line when ad-hoc signed and permissions are missing
                .when(snap.any_missing() && !bundle_path.is_empty(), |card| {
                    card.child(
                        div()
                            .px(px(4.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .truncate()
                                    .child(if lang.starts_with("zh") {
                                        format!("当前授权目标：{bundle_path}")
                                    } else {
                                        format!("Target bundle: {bundle_path}")
                                    }),
                            )
                            .child(
                                div()
                                    .id("perm-modal-recheck-link")
                                    .cursor_pointer()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                        request_permissions_refresh(true);
                                    })
                                    .child(tr_str(lang, "立即重新检测", "Re-check Now")),
                            ),
                    )
                })
                // Footer action buttons (matching Gemini: 不用了 / 完成)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_end()
                        .gap(px(10.0))
                        .pt(px(2.0))
                        .child(action_pill(
                            "perm-modal-dismiss-btn",
                            tr_str(lang, "不用了", "Not Now"),
                            false,
                            pal,
                            move |ev, window, cx| {
                                hide_drag_hud();
                                on_dismiss(ev, window, cx);
                            },
                        ))
                        .child(action_pill(
                            "perm-modal-done-btn",
                            tr_str(lang, "完成", "Done"),
                            all_ready || snap.screen_state.is_active(),
                            pal,
                            move |ev, window, cx| {
                                request_permissions_refresh(true);
                                hide_drag_hud();
                                on_complete(ev, window, cx);
                            },
                        )),
                ),
        )
}

/// Renders a clear guidance modal when a REMOTE computer (not this local Mac) rejected
/// screen capture or GUI input due to missing host permissions.
pub fn render_remote_host_permission_modal(
    remote_device_name: &str,
    needs_screen: bool,
    needs_accessibility: bool,
    detail: Option<&str>,
    pal: TdesktopThemePalette,
    lang: &str,
    on_dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_retry: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_open_local: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let missing_desc = match (needs_screen, needs_accessibility, lang.starts_with("zh")) {
        (true, true, true) => format!(
            "被控端「{remote_device_name}」尚未授予「录屏与系统录音」或「辅助功能」权限。请在该被控电脑上的 Fabushi → 设置 → 电脑与远控中开启权限（或拖拽应用至系统设置并重启）。"
        ),
        (true, false, true) => format!(
            "被控端「{remote_device_name}」尚未开启「录屏与系统录音」权限（或授权后尚未重启 Fabushi）。请在被控端电脑上完成授权后重试。"
        ),
        (false, true, true) => format!(
            "被控端「{remote_device_name}」尚未开启「辅助功能」权限，无法响应远程鼠标点击和键盘输入。请在被控端电脑上开启后重试。"
        ),
        (true, true, false) => format!(
            "Remote host \"{remote_device_name}\" is missing Screen Recording or Accessibility permissions. Enable them in Fabushi Settings on that computer and restart if prompted."
        ),
        (true, false, false) => format!(
            "Remote host \"{remote_device_name}\" has not enabled Screen Recording (or needs a restart after enabling). Complete permission setup on that host and retry."
        ),
        _ => format!(
            "Remote host \"{remote_device_name}\" has not enabled Accessibility permission for remote mouse/keyboard input. Enable it on that host and retry."
        ),
    };

    div()
        .id("remote-host-permission-modal-backdrop")
        .absolute()
        .inset_0()
        .bg(rgba(0x090D16B8))
        .flex()
        .items_center()
        .justify_center()
        .p(px(20.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .w(px(500.0))
                .max_w_full()
                .p(px(24.0))
                .rounded(px(20.0))
                .bg(rgb(pal.modal_bg))
                .border_1()
                .border_color(rgb(pal.border_fg))
                .shadow_lg()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    div()
                        .text_size(px(17.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(
                            lang,
                            "需要在被控端电脑开启系统权限",
                            "Host Permissions Required on Remote Computer",
                        )),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(rgb(pal.text_secondary))
                        .child(missing_desc),
                )
                .when_some(detail.filter(|s| !s.trim().is_empty()), |card, err_text| {
                    card.child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .rounded(px(10.0))
                            .bg(rgb(pal.card_bg))
                            .border_1()
                            .border_color(rgb(pal.border_fg))
                            .text_size(px(11.5))
                            .text_color(rgb(pal.text_secondary))
                            .child(err_text.to_string()),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .pt(px(4.0))
                        .child(action_pill(
                            "remote-perm-open-local-btn",
                            tr_str(lang, "检查本机权限", "Check Local Permissions"),
                            false,
                            pal,
                            on_open_local,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .child(action_pill(
                                    "remote-perm-dismiss-btn",
                                    tr_str(lang, "关闭", "Close"),
                                    false,
                                    pal,
                                    on_dismiss,
                                ))
                                .child(action_pill(
                                    "remote-perm-retry-btn",
                                    tr_str(lang, "重新连接画面", "Retry Screen"),
                                    true,
                                    pal,
                                    on_retry,
                                )),
                        ),
                ),
        )
}

