//! OpenBot 1:1 `BrowserFullView` Renderer for [`BrowserWorkspace`].
//!
//! Mirrors upstream `nightly-labs/openbot` (`image_2.png`):
//! - `packages/ui/src/features/browser/BrowserFullView.tsx`
//! - `packages/ui/src/features/browser/BrowserTabStrip.tsx` & `BrowserTabMenu.tsx`
//! - `packages/ui/src/features/browser/BrowserAddressRow.tsx` & `BrowserPageMenu.tsx`
//! - `packages/ui/src/features/browser/BrowserTeachBar.tsx`
//! - `packages/ui/src/features/browser/BrowserDock.tsx` & `BrowserIsland.tsx`

use bot_browser::BrowserControlMode;
use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::theme::openbot_palette::OpenBotPalette;
use crate::views::browser::openbot_browser_chrome::{
    browser_profile_hue_color, openbot_browser_icon, render_browser_owner_avatar,
    OpenBotBrowserIcon,
};
use crate::views::browser::openbot_session::{
    browser_completion, browser_display_url, browser_shortcut_glyph, BrowserIslandView,
    BrowserShortcutName, BrowserShortcutPlatform, BrowserSuggestionKind, BrowserTeachUiState,
    BROWSER_ADDRESS_ROW_HEIGHT_PX, BROWSER_DOCK_MAX_WIDTH_PX, BROWSER_FULL_TOP_HEIGHT_PX,
    BROWSER_MAC_TRAFFIC_LIGHTS_INSET_PX, BROWSER_MAX_TABS, BROWSER_TAB_MAX_WIDTH_PX,
    BROWSER_TAB_MIN_WIDTH_PX,
};
use crate::views::browser::recorder_panel::BrowserLogLevel;
use crate::views::browser::workspace::{
    ActiveBrowserInput, BrowserInspectorTab, BrowserWorkspace,
};
use crate::views::browser::{BrowserEngineKind, BrowserWorkspaceEvent};
use crate::views::tdesktop_theme::tr_str;

fn dock_glass_bg(ob: &OpenBotPalette) -> Rgba {
    if ob.scheme.is_dark() {
        Rgba {
            r: 0.13,
            g: 0.13,
            b: 0.13,
            a: 0.86,
        }
    } else {
        Rgba {
            r: 0.99,
            g: 0.99,
            b: 1.0,
            a: 0.90,
        }
    }
}

fn dock_glass_border(ob: &OpenBotPalette) -> Rgba {
    if ob.scheme.is_dark() {
        Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.11,
        }
    } else {
        Rgba {
            r: 0.08,
            g: 0.08,
            b: 0.08,
            a: 0.12,
        }
    }
}

fn soft_rgba(color: Rgba, alpha: f32) -> Rgba {
    Rgba {
        r: color.r,
        g: color.g,
        b: color.b,
        a: alpha,
    }
}

fn menu_row_button(
    id: impl Into<ElementId>,
    label: String,
    shortcut: Option<&'static str>,
    disabled: bool,
    danger: bool,
    checked: bool,
    ob: &OpenBotPalette,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let fg = if disabled {
        ob.text_dim()
    } else if danger {
        ob.danger()
    } else {
        ob.text_primary()
    };
    let hover_bg = if disabled {
        Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }
    } else {
        ob.hover()
    };
    let mut row = div()
        .id(id)
        .w_full()
        .h(px(30.0))
        .px(px(10.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .text_size(px(12.5))
        .text_color(fg)
        .hover(move |s| s.bg(hover_bg))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .min_w(px(0.0))
                .child(div().truncate().child(label)),
        );

    if checked || shortcut.is_some() {
        let right_text = if checked {
            "✓".to_string()
        } else {
            shortcut.unwrap_or("").to_string()
        };
        row = row.child(
            div()
                .flex_shrink_0()
                .text_size(px(11.0))
                .text_color(if checked { ob.accent_text() } else { ob.text_muted() })
                .child(right_text),
        );
    }

    if !disabled {
        row = row.cursor_pointer().on_mouse_down(MouseButton::Left, on_click);
    }
    row
}

fn menu_divider(ob: &OpenBotPalette) -> Div {
    div()
        .w_full()
        .h(px(1.0))
        .my(px(4.0))
        .bg(ob.border_hairline())
}

impl BrowserWorkspace {
    /// Renders the complete 1:1 OpenBot `BrowserFullView` (`image_2.png`).
    pub(crate) fn render_openbot_full_view(
        &mut self,
        viewport_element: AnyElement,
        inspector_body: AnyElement,
        dock_floats_over_viewport: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let is_zh = self.language_id.starts_with("zh");
        let lang = self.language_id.clone();

        let inspector_tabs: &[(BrowserInspectorTab, &str)] = &[
            (BrowserInspectorTab::Copilot, "Copilot"),
            (BrowserInspectorTab::Recorder, "Script Apps"),
            (BrowserInspectorTab::Session, "Session"),
            (BrowserInspectorTab::Store, "Store"),
            (BrowserInspectorTab::Design, "Design"),
        ];

        let top_bar = self.render_openbot_full_top_bar(&ob, is_zh, cx);
        let address_row = self.render_openbot_address_row(&ob, is_zh, cx);
        let popup_alert = self.render_openbot_popup_alert(&ob, is_zh, cx);
        let teach_bar = self.render_openbot_teach_bar(&ob, is_zh, cx);
        let dock_element = self.render_openbot_browser_dock(&ob, is_zh, cx);
        let tab_menu_overlay = self.render_openbot_tab_context_menu(&ob, is_zh, cx);
        let page_menu_overlay = self.render_openbot_page_menu(&ob, is_zh, cx);
        let suggestions_overlay = self.render_openbot_address_suggestions(&ob, is_zh, cx);

        let is_teach_recording = self.teach.is_recording();

        let viewport_stack = if dock_floats_over_viewport {
            div()
                .relative()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .h_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .when(is_teach_recording, |el| {
                    el.border_2().border_color(soft_rgba(ob.danger(), 0.35))
                })
                .child(viewport_element)
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(16.0))
                        .px(px(24.0))
                        .flex()
                        .justify_center()
                        .child(dock_element),
                )
        } else {
            // Native macOS WKWebView: keep dock in an explicit bottom tray outside NSView bounds
            // so the native WebKit subview never paints over or intercepts clicks on BrowserDock.
            div()
                .relative()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .h_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .when(is_teach_recording, |el| {
                    el.border_2().border_color(soft_rgba(ob.danger(), 0.35))
                })
                .child(viewport_element)
                .child(
                    div()
                        .flex_shrink_0()
                        .w_full()
                        .px(px(24.0))
                        .py(px(10.0))
                        .bg(ob.bg_canvas())
                        .border_t_1()
                        .border_color(ob.border_hairline())
                        .flex()
                        .justify_center()
                        .child(dock_element),
                )
        };

        div()
            .id("browser-workspace-root")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if this.is_workspace_occluded() {
                    return;
                }
                let key = ev.keystroke.key.as_str();
                let mods = ev.keystroke.modifiers;
                if mods.platform && key.eq_ignore_ascii_case("v") {
                    if let Some(clip) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        this.insert_text_to_active_input(&clip, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                // Handle OpenBot browser view keyboard shortcuts (`Cmd+T`, `Cmd+W`, `Cmd+L`, `Cmd+R`, etc.)
                if let Some(cmd) = crate::views::browser::openbot_session::resolve_browser_view_command(
                    key,
                    mods,
                    BrowserShortcutPlatform::current(),
                ) {
                    let is_single_tab_digit = matches!(
                        cmd,
                        crate::views::browser::openbot_session::BrowserViewCommand::SelectIndex(_)
                    ) && this.session.tabs.len() <= 1;
                    if !is_single_tab_digit
                        && this.handle_browser_view_command(cmd, Some(window), cx)
                    {
                        cx.stop_propagation();
                        return;
                    }
                }
                if this.handle_non_char_key_input(key, mods.alt, cx) {
                    cx.stop_propagation();
                    return;
                }
                if let Some(ref ch) = ev.keystroke.key_char {
                    if !mods.platform
                        && !mods.control
                        && this.insert_text_to_active_input(ch, cx)
                    {
                        cx.stop_propagation();
                    }
                }
            }))
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .w_full()
            .h_full()
            .overflow_hidden()
            .bg(ob.bg_frame())
            .text_color(ob.text_primary())
            // 38px OpenBot top tab strip (`browser-full-top`)
            .child(top_bar)
            // Rounded-top main browser card (`browser-full-card`)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .w_full()
                    .rounded_t(px(14.0))
                    .border_t_1()
                    .border_color(ob.frame_card_border())
                    .bg(ob.bg_canvas())
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    // 44px Address Row (`browser-full-address-row`)
                    .child(address_row)
                    // Optional Popup Blocked Alert
                    .when_some(popup_alert, |col, alert| col.child(alert))
                    // Optional Error Strip (dismissible)
                    .when_some(self.last_error.clone(), |col, err| {
                        col.child(
                            div()
                                .flex_shrink_0()
                                .mx(px(8.0))
                                .mt(px(6.0))
                                .px(px(10.0))
                                .py(px(6.0))
                                .rounded(px(8.0))
                                .bg(soft_rgba(ob.danger(), 0.14))
                                .border_1()
                                .border_color(soft_rgba(ob.danger(), 0.28))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .text_size(px(12.0))
                                        .text_color(ob.danger())
                                        .truncate()
                                        .child(err),
                                )
                                .child(
                                    div()
                                        .id("browser-error-dismiss-btn")
                                        .w(px(20.0))
                                        .h(px(20.0))
                                        .rounded(px(5.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(ob.hover()))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.last_error = None;
                                                cx.notify();
                                            }),
                                        )
                                        .child(openbot_browser_icon(
                                            OpenBotBrowserIcon::CloseX,
                                            11.0,
                                            ob.danger(),
                                        )),
                                ),
                        )
                    })
                    // Optional Teach Bar (`BrowserTeachBar`)
                    .when_some(teach_bar, |col, bar| col.child(bar))
                    // Split-Pane Body: Viewport + Optional Tools Inspector Drawer
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.0))
                            .w_full()
                            .flex()
                            .overflow_hidden()
                            .child(viewport_stack)
                            .when(self.inspector_open, |row| {
                                row.child(
                                    div()
                                        .id("browser-tools-inspector-drawer")
                                        .w(px(360.0))
                                        .h_full()
                                        .flex_shrink_0()
                                        .bg(rgb(pal.dialogs_bg))
                                        .border_l_1()
                                        .border_color(ob.border_hairline())
                                        .p(px(10.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .text_size(px(12.5))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(ob.text_primary())
                                                        .child(tr_str(
                                                            &lang,
                                                            "浏览器开发者工具",
                                                            "Browser Tools",
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .id("browser-inspector-close-btn")
                                                        .w(px(24.0))
                                                        .h(px(24.0))
                                                        .rounded(px(6.0))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .cursor_pointer()
                                                        .hover(|s| s.bg(ob.hover()))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(|this, _, _, cx| {
                                                                this.inspector_open = false;
                                                                if this
                                                                    .active_input
                                                                    .owner_inspector_tab()
                                                                    .is_some()
                                                                {
                                                                    this.set_active_input(
                                                                        ActiveBrowserInput::None,
                                                                        None,
                                                                        cx,
                                                                    );
                                                                } else {
                                                                    cx.notify();
                                                                }
                                                            }),
                                                        )
                                                        .child(openbot_browser_icon(
                                                            OpenBotBrowserIcon::CloseX,
                                                            12.0,
                                                            ob.text_muted(),
                                                        )),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(4.0))
                                                .children(inspector_tabs.iter().map(
                                                    |(tab, title)| {
                                                        let is_sel = self.inspector_tab == *tab;
                                                        let t = *tab;
                                                        div()
                                                            .flex_1()
                                                            .px(px(6.0))
                                                            .py(px(5.0))
                                                            .rounded(px(6.0))
                                                            .bg(if is_sel {
                                                                ob.accent()
                                                            } else {
                                                                ob.bg_control()
                                                            })
                                                            .text_xs()
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(if is_sel {
                                                                rgb(0xFFFFFF)
                                                            } else {
                                                                ob.text_secondary()
                                                            })
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .cursor_pointer()
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                cx.listener(move |this, _, _, cx| {
                                                                    this.set_inspector_tab(t, cx);
                                                                }),
                                                            )
                                                            .child(*title)
                                                    },
                                                )),
                                        )
                                        .child(inspector_body),
                                )
                            }),
                    ),
            )
            // Deferred Popover Layers (render above browser card & address bar)
            .when_some(suggestions_overlay, |root, ov| root.child(ov))
            .when_some(page_menu_overlay, |root, ov| root.child(ov))
            .when_some(tab_menu_overlay, |root, ov| root.child(ov))
            .into_any_element()
    }

    /// Renders the 38px `#0d0d0d` OpenBot top bar: macOS traffic lights inset + tab strip + `+` + `[← 返回聊天]`.
    fn render_openbot_full_top_bar(
        &self,
        ob: &OpenBotPalette,
        is_zh: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let active_id = self.session.active_tab_id.as_deref();
        let can_add = self.session.tabs.len() < BROWSER_MAX_TABS;
        let is_agent_acting = matches!(
            self.control_mode(),
            BrowserControlMode::AiControlling { .. }
        );
        let default_owner = &self.dock.active_agent;
        let hovered_tab = self.hovered_tab_id.as_deref();

        let left_inset = if cfg!(target_os = "macos") {
            BROWSER_MAC_TRAFFIC_LIGHTS_INSET_PX
        } else {
            8.0
        };

        let mut tab_strip = div()
            .id("browser-full-tab-strip")
            .flex()
            .items_center()
            .gap(px(4.0))
            .min_w(px(0.0))
            .overflow_x_scroll()
            .flex_shrink();

        for (idx, tab) in self.session.tabs.iter().enumerate() {
            let is_active = active_id == Some(tab.id.as_str());
            let is_hovered = hovered_tab == Some(tab.id.as_str());
            let tab_id_click = tab.id.clone();
            let tab_id_mid = tab.id.clone();
            let tab_id_ctx = tab.id.clone();
            let tab_id_hover = tab.id.clone();
            let tab_id_close = tab.id.clone();
            let title = tab.display_title(is_zh);
            let owner = tab.owner_agent_id.as_ref().map(|_| default_owner);
            let show_acting = is_active && is_agent_acting;

            let leading: AnyElement = if is_hovered {
                div()
                    .id(SharedString::from(format!("browser-tab-close-{idx}")))
                    .w(px(18.0))
                    .h(px(18.0))
                    .rounded(px(5.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(ob.bg_control_active()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.close_tabs(std::slice::from_ref(&tab_id_close), cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(openbot_browser_icon(
                        OpenBotBrowserIcon::CloseX,
                        11.5,
                        ob.text_secondary(),
                    ))
                    .into_any_element()
            } else if tab.loading {
                div()
                    .w(px(18.0))
                    .h(px(18.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(openbot_browser_icon(
                        OpenBotBrowserIcon::LoaderSpin,
                        13.0,
                        ob.text_muted(),
                    ))
                    .into_any_element()
            } else {
                render_browser_owner_avatar(
                    owner.or(Some(default_owner)),
                    show_acting,
                    16.0,
                    ob,
                )
            };

            let profile_underline = tab
                .profile_hue
                .map(|hue| browser_profile_hue_color(hue, ob));

            let chip_bg = if is_active {
                ob.bg_control_active()
            } else if is_hovered {
                ob.hover()
            } else {
                Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                }
            };

            let chip = div()
                .id(SharedString::from(format!("browser-tab-chip-{idx}")))
                .relative()
                .h(px(28.0))
                .min_w(px(BROWSER_TAB_MIN_WIDTH_PX))
                .max_w(px(BROWSER_TAB_MAX_WIDTH_PX))
                .px(px(8.0))
                .rounded(px(8.0))
                .bg(chip_bg)
                .border_1()
                .border_color(if is_active {
                    ob.border_strong_hairline()
                } else {
                    Rgba {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    }
                })
                .flex()
                .items_center()
                .gap(px(7.0))
                .cursor_pointer()
                .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                    if *hovering {
                        if this.hovered_tab_id.as_deref() != Some(tab_id_hover.as_str()) {
                            this.hovered_tab_id = Some(tab_id_hover.clone());
                            cx.notify();
                        }
                    } else if this.hovered_tab_id.as_deref() == Some(tab_id_hover.as_str()) {
                        this.hovered_tab_id = None;
                        cx.notify();
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.activate_tab(&tab_id_click, cx);
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, _, cx| {
                        this.close_tabs(std::slice::from_ref(&tab_id_mid), cx);
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, cx| {
                        this.page_menu_open = false;
                        this.address_suggestions_open = false;
                        this.tab_context_menu_tab_id = Some(tab_id_ctx.clone());
                        cx.notify();
                        cx.stop_propagation();
                    }),
                )
                .child(leading)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(12.5))
                        .font_weight(if is_active {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if is_active {
                            ob.text_primary()
                        } else {
                            ob.text_secondary()
                        })
                        .child(title),
                )
                .when_some(profile_underline, |el, col| {
                    el.child(
                        div()
                            .absolute()
                            .left(px(8.0))
                            .right(px(8.0))
                            .bottom(px(1.5))
                            .h(px(2.0))
                            .rounded_full()
                            .bg(col),
                    )
                });

            tab_strip = tab_strip.child(chip);
        }

        let new_tab_btn = div()
            .id("browser-full-new-tab-btn")
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(7.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .when(can_add, |el| {
                el.cursor_pointer()
                    .hover(|s| s.bg(ob.hover()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            let _ = this.open_new_tab(None, None, true, Some(window), cx);
                            cx.stop_propagation();
                        }),
                    )
            })
            .child(openbot_browser_icon(
                OpenBotBrowserIcon::Plus,
                14.0,
                if can_add {
                    ob.text_secondary()
                } else {
                    ob.text_dim()
                },
            ));

        let back_to_chat_btn = div()
            .id("browser-full-back-to-chat-btn")
            .h(px(26.0))
            .px(px(11.0))
            .mr(px(8.0))
            .rounded_full()
            .flex_shrink_0()
            .bg(ob.bg_control())
            .border_1()
            .border_color(ob.border_strong_hairline())
            .hover(|s| s.bg(ob.bg_control_hover()))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(5.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.request_back_to_chat(true, cx);
                    cx.stop_propagation();
                }),
            )
            .child(openbot_browser_icon(
                OpenBotBrowserIcon::ArrowLeft,
                12.5,
                ob.text_primary(),
            ))
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ob.text_primary())
                    .child(if is_zh { "返回聊天" } else { "Back to chat" }),
            );

        div()
            .id("browser-full-top-bar")
            .h(px(BROWSER_FULL_TOP_HEIGHT_PX))
            .w_full()
            .flex_shrink_0()
            .pl(px(left_inset))
            .pr(px(4.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(tab_strip)
            .child(new_tab_btn)
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w(px(24.0))
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(back_to_chat_btn)
    }

    /// Renders the 44px OpenBot `BrowserAddressRow` (`<`, `>`, `↻`/`×`, Address Capsule, badges, `...`).
    fn render_openbot_address_row(
        &self,
        ob: &OpenBotPalette,
        is_zh: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let addr_active = self.active_input == ActiveBrowserInput::AddressBar;
        let active_tab = self.session.active_tab();
        let can_back = self.can_go_back || active_tab.is_some_and(|t| t.can_go_back);
        let can_fwd = self.can_go_forward || active_tab.is_some_and(|t| t.can_go_forward);
        let is_loading = self.is_loading || active_tab.is_some_and(|t| t.loading);
        let is_recording = self.recorder.is_dom_recording
            || self.teach.is_recording()
            || active_tab.is_some_and(|t| t.recording);
        let is_acting = matches!(
            self.control_mode(),
            BrowserControlMode::AiControlling { .. }
        );

        let log_error_count = self
            .recorder
            .event_logs
            .iter()
            .filter(|l| l.level == BrowserLogLevel::Error)
            .count();
        let tab_diag_count = active_tab.map(|t| t.diagnostic_error_count).unwrap_or(0);
        let total_diag_errors = tab_diag_count.max(log_error_count);

        let completion_suffix = if addr_active {
            browser_completion(&self.url_input, &self.session.history).and_then(|entry| {
                let shown = browser_display_url(&entry.url);
                let typed_len = self.url_input.len();
                if shown.len() > typed_len
                    && shown
                        .to_ascii_lowercase()
                        .starts_with(&self.url_input.to_ascii_lowercase())
                {
                    Some(shown[typed_len..].to_string())
                } else {
                    None
                }
            })
        } else {
            None
        };

        let nav_btn = |id: &'static str, icon: OpenBotBrowserIcon, enabled: bool| {
            div()
                .id(id)
                .w(px(28.0))
                .h(px(28.0))
                .rounded(px(7.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .when(enabled, |el| el.cursor_pointer().hover(|s| s.bg(ob.hover())))
                .child(openbot_browser_icon(
                    icon,
                    15.0,
                    if enabled {
                        ob.text_secondary()
                    } else {
                        ob.text_dim()
                    },
                ))
        };

        let back_btn = nav_btn("browser-nav-back-btn", OpenBotBrowserIcon::ChevronLeft, can_back)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.go_back(cx);
                    cx.stop_propagation();
                }),
            );

        let fwd_btn = nav_btn(
            "browser-nav-forward-btn",
            OpenBotBrowserIcon::ChevronRight,
            can_fwd,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.go_forward(cx);
                cx.stop_propagation();
            }),
        );

        let reload_btn = nav_btn(
            "browser-nav-reload-btn",
            if is_loading {
                OpenBotBrowserIcon::StopX
            } else {
                OpenBotBrowserIcon::Reload
            },
            true,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                if this.is_loading {
                    this.stop_loading(cx);
                } else {
                    this.reload(cx);
                }
                cx.stop_propagation();
            }),
        );

        let placeholder = if is_zh {
            "搜索 Google 或输入网址"
        } else {
            "Search Google or enter address"
        };

        let capsule_content: AnyElement = if addr_active {
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .overflow_hidden()
                .text_size(px(13.0))
                .child(
                    div()
                        .text_color(if self.url_input.is_empty() {
                            ob.text_muted()
                        } else {
                            ob.text_primary()
                        })
                        .truncate()
                        .child(if self.url_input.is_empty() {
                            "|".to_string()
                        } else {
                            format!("{}|", self.url_input)
                        }),
                )
                .when_some(completion_suffix, |row, suffix| {
                    row.child(
                        div()
                            .text_color(ob.text_muted())
                            .truncate()
                            .child(suffix),
                    )
                })
                .into_any_element()
        } else if self.url_input.is_empty() {
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(13.0))
                .text_color(ob.text_muted())
                .child(placeholder)
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(13.0))
                .text_color(ob.text_primary())
                .child(self.url_input.clone())
                .into_any_element()
        };

        let capsule = div()
            .id("browser-address-bar-input")
            .flex_1()
            .min_w(px(140.0))
            .h(px(32.0))
            .mx(px(4.0))
            .px(px(10.0))
            .rounded(px(10.0))
            .bg(if addr_active {
                ob.bg_control_active()
            } else {
                ob.bg_control()
            })
            .border_1()
            .border_color(if addr_active {
                ob.border_strong_hairline()
            } else {
                ob.border_hairline()
            })
            .cursor_text()
            .flex()
            .items_center()
            .gap(px(8.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.page_menu_open = false;
                    this.tab_context_menu_tab_id = None;
                    this.address_suggestions_open = true;
                    this.address_suggestion_active_idx = None;
                    this.set_active_input(ActiveBrowserInput::AddressBar, Some(window), cx);
                    cx.stop_propagation();
                }),
            )
            .when(is_acting, |row| {
                row.child(render_browser_owner_avatar(
                    Some(&self.dock.active_agent),
                    true,
                    16.0,
                    ob,
                ))
            })
            .child(capsule_content);

        let show_profile_pill = self.active_profile_id != "default"
            || active_tab.is_some_and(|t| t.proxy_name.is_some());
        let profile_label = if let Some(proxy) = active_tab.and_then(|t| t.proxy_name.as_deref()) {
            format!("{} · {proxy}", self.active_profile_id)
        } else {
            self.active_profile_id.clone()
        };
        let profile_dot_color = active_tab
            .and_then(|t| t.profile_hue)
            .map(|h| browser_profile_hue_color(h, ob))
            .unwrap_or_else(|| ob.accent());

        let page_menu_btn = div()
            .id("browser-page-menu-btn")
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(7.0))
            .flex_shrink_0()
            .bg(if self.page_menu_open {
                ob.bg_control_active()
            } else {
                Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                }
            })
            .hover(|s| s.bg(ob.hover()))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let next = !this.page_menu_open;
                    this.close_all_browser_popovers();
                    this.page_menu_open = next;
                    cx.notify();
                    cx.stop_propagation();
                }),
            )
            .child(openbot_browser_icon(
                OpenBotBrowserIcon::Ellipsis,
                15.0,
                ob.text_secondary(),
            ));

        div()
            .id("browser-full-address-row")
            .h(px(BROWSER_ADDRESS_ROW_HEIGHT_PX))
            .w_full()
            .flex_shrink_0()
            .px(px(8.0))
            .border_b_1()
            .border_color(ob.border_hairline())
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(back_btn)
            .child(fwd_btn)
            .child(reload_btn)
            .child(capsule)
            // Optional Profile pill
            .when(show_profile_pill, |row| {
                row.child(
                    div()
                        .id("browser-profile-pill")
                        .h(px(26.0))
                        .px(px(9.0))
                        .mr(px(2.0))
                        .rounded_full()
                        .bg(ob.bg_control())
                        .border_1()
                        .border_color(ob.border_hairline())
                        .hover(|s| s.bg(ob.bg_control_hover()))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.inspector_open = true;
                                this.set_inspector_tab(BrowserInspectorTab::Session, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(
                            div()
                                .w(px(7.0))
                                .h(px(7.0))
                                .rounded_full()
                                .bg(profile_dot_color),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(ob.text_secondary())
                                .child(profile_label),
                        ),
                )
            })
            // Optional REC pill
            .when(is_recording, |row| {
                row.child(
                    div()
                        .id("browser-rec-pill")
                        .h(px(26.0))
                        .px(px(8.0))
                        .mr(px(2.0))
                        .rounded_full()
                        .bg(soft_rgba(ob.danger(), 0.16))
                        .border_1()
                        .border_color(soft_rgba(ob.danger(), 0.35))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                if !this.teach.is_active() {
                                    this.inspector_open = true;
                                    this.set_inspector_tab(BrowserInspectorTab::Recorder, cx);
                                }
                                cx.stop_propagation();
                            }),
                        )
                        .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(ob.danger()))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.danger())
                                .child("REC"),
                        ),
                )
            })
            // Optional Diagnostics Warning Pill (`⚠ 1` in `image_2.png`)
            .when(total_diag_errors > 0, |row| {
                row.child(
                    div()
                        .id("browser-diagnostics-pill")
                        .h(px(26.0))
                        .px(px(8.0))
                        .mr(px(2.0))
                        .rounded_full()
                        .bg(soft_rgba(ob.warning(), 0.15))
                        .border_1()
                        .border_color(soft_rgba(ob.warning(), 0.30))
                        .hover(|s| s.bg(soft_rgba(ob.warning(), 0.24)))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.inspector_open = !this.inspector_open;
                                if this.inspector_open {
                                    this.set_inspector_tab(BrowserInspectorTab::Copilot, cx);
                                }
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        )
                        .child(openbot_browser_icon(
                            OpenBotBrowserIcon::TriangleAlert,
                            13.0,
                            ob.warning(),
                        ))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.warning())
                                .child(total_diag_errors.to_string()),
                        ),
                )
            })
            // Design Mode indicator when active
            .when(self.design_mode.enabled, |row| {
                row.child(
                    div()
                        .id("browser-design-active-pill")
                        .h(px(26.0))
                        .px(px(8.0))
                        .mr(px(2.0))
                        .rounded_full()
                        .bg(ob.accent_soft())
                        .border_1()
                        .border_color(soft_rgba(ob.accent(), 0.35))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_design_mode(cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(openbot_browser_icon(
                            OpenBotBrowserIcon::Sparkles,
                            12.0,
                            ob.accent_text(),
                        ))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.accent_text())
                                .child(if is_zh { "设计模式" } else { "Design" }),
                        ),
                )
            })
            .child(page_menu_btn)
    }

    /// Renders OpenBot's omnibox suggestions popover below the Address Capsule.
    fn render_openbot_address_suggestions(
        &self,
        ob: &OpenBotPalette,
        is_zh: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.address_suggestions_open {
            return None;
        }
        let rows = self.current_address_suggestions();
        if rows.is_empty() {
            return None;
        }
        let active_idx = self.address_suggestion_active_idx;
        let top_offset = BROWSER_FULL_TOP_HEIGHT_PX + BROWSER_ADDRESS_ROW_HEIGHT_PX + 2.0;

        let mut list = div()
            .id("browser-address-suggestions-menu")
            .w_full()
            .max_w(px(720.0))
            .p(px(4.0))
            .rounded(px(10.0))
            .bg(ob.bg_surface())
            .border_1()
            .border_color(ob.border_strong_hairline())
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(px(2.0));

        for (idx, row) in rows.into_iter().enumerate() {
            let is_sel = active_idx == Some(idx);
            let row_clone = row.clone();
            let icon = match row.kind {
                BrowserSuggestionKind::Tab | BrowserSuggestionKind::Url => {
                    OpenBotBrowserIcon::Globe2
                }
                BrowserSuggestionKind::History => OpenBotBrowserIcon::Clock3,
                BrowserSuggestionKind::Search => OpenBotBrowserIcon::Search,
            };
            let is_tab = row.kind == BrowserSuggestionKind::Tab;

            list = list.child(
                div()
                    .id(SharedString::from(format!("browser-suggest-row-{idx}")))
                    .w_full()
                    .h(px(32.0))
                    .px(px(8.0))
                    .rounded(px(7.0))
                    .bg(if is_sel {
                        ob.bg_control_active()
                    } else {
                        Rgba {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }
                    })
                    .hover(|s| s.bg(ob.hover()))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.choose_address_suggestion(&row_clone, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(openbot_browser_icon(icon, 14.0, ob.text_muted()))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(ob.text_primary())
                            .child(row.title),
                    )
                    .when_some(row.detail, |el, detail| {
                        el.child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(px(12.0))
                                .text_color(ob.text_muted())
                                .child(detail),
                        )
                    })
                    .when(is_tab, |el| {
                        el.child(
                            div()
                                .flex_shrink_0()
                                .ml_auto()
                                .px(px(7.0))
                                .py(px(2.0))
                                .rounded_full()
                                .border_1()
                                .border_color(ob.border_strong_hairline())
                                .text_size(px(11.0))
                                .text_color(ob.text_muted())
                                .child(if is_zh { "切换到标签页" } else { "Switch to tab" }),
                        )
                    }),
            );
        }

        Some(
            deferred(
                div()
                    .id("browser-suggestions-backdrop")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.address_suggestions_open = false;
                            this.address_suggestion_active_idx = None;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(top_offset))
                            .left(px(102.0))
                            .right(px(52.0))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(list),
                    ),
            )
            .with_priority(60)
            .into_any_element(),
        )
    }

    /// Renders OpenBot's `BrowserPageMenu` (`...`) dropdown.
    fn render_openbot_page_menu(
        &self,
        ob: &OpenBotPalette,
        is_zh: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.page_menu_open {
            return None;
        }
        let platform = BrowserShortcutPlatform::current();
        let can_add = self.session.tabs.len() < BROWSER_MAX_TABS;
        let can_reopen = self.session.can_reopen();
        let next_engine_label = match self.active_engine {
            BrowserEngineKind::WkWebView => {
                if is_zh {
                    "切换到 Rust CDP 引擎"
                } else {
                    "Switch to Rust CDP engine"
                }
            }
            BrowserEngineKind::RustCdp => {
                if is_zh {
                    "切换到原生 WKWebView 引擎"
                } else {
                    "Switch to WKWebView engine"
                }
            }
        };

        let menu = div()
            .id("browser-page-menu-card")
            .w(px(248.0))
            .p(px(5.0))
            .rounded(px(10.0))
            .bg(ob.bg_surface())
            .border_1()
            .border_color(ob.border_strong_hairline())
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(px(1.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(menu_row_button(
                "browser-pm-new-tab",
                if is_zh {
                    "新建标签页".into()
                } else {
                    "New tab".into()
                },
                Some(browser_shortcut_glyph(
                    BrowserShortcutName::NewTab,
                    platform,
                )),
                !can_add,
                false,
                false,
                ob,
                cx.listener(|this, _, window, cx| {
                    let _ = this.open_new_tab(None, None, true, Some(window), cx);
                    cx.stop_propagation();
                }),
            ))
            .child(menu_row_button(
                "browser-pm-reopen",
                if is_zh {
                    "重新打开关闭的标签页".into()
                } else {
                    "Reopen closed tab".into()
                },
                Some(browser_shortcut_glyph(
                    BrowserShortcutName::Reopen,
                    platform,
                )),
                !can_reopen,
                false,
                false,
                ob,
                cx.listener(|this, _, _, cx| {
                    this.reopen_closed_tab(cx);
                    cx.stop_propagation();
                }),
            ))
            .child(menu_divider(ob))
            .child(menu_row_button(
                "browser-pm-teach",
                if is_zh {
                    "演示并录制任务".into()
                } else {
                    "Teach a task".into()
                },
                None,
                false,
                false,
                self.teach.is_active(),
                ob,
                cx.listener(|this, _, _, cx| {
                    this.open_teach_bar(cx);
                    cx.stop_propagation();
                }),
            ))
            .child(menu_row_button(
                "browser-pm-design",
                if is_zh {
                    "设计模式元素采集".into()
                } else {
                    "Design Mode picker".into()
                },
                Some(if cfg!(target_os = "macos") {
                    "⇧⌘I"
                } else {
                    "Ctrl+Shift+I"
                }),
                false,
                false,
                self.design_mode.enabled,
                ob,
                cx.listener(|this, _, _, cx| {
                    this.page_menu_open = false;
                    this.toggle_design_mode(cx);
                    cx.stop_propagation();
                }),
            ))
            .child(menu_row_button(
                "browser-pm-engine",
                next_engine_label.to_string(),
                None,
                false,
                false,
                false,
                ob,
                cx.listener(|this, _, _, cx| {
                    this.page_menu_open = false;
                    let next = match this.active_engine {
                        BrowserEngineKind::WkWebView => BrowserEngineKind::RustCdp,
                        BrowserEngineKind::RustCdp => BrowserEngineKind::WkWebView,
                    };
                    this.set_engine_kind(next, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(menu_row_button(
                "browser-pm-inspector",
                if is_zh {
                    "浏览器工具面板 (Copilot / 脚本 / 会话)".into()
                } else {
                    "Browser tools drawer".into()
                },
                None,
                false,
                false,
                self.inspector_open,
                ob,
                cx.listener(|this, _, _, cx| {
                    this.page_menu_open = false;
                    this.inspector_open = !this.inspector_open;
                    cx.notify();
                    cx.stop_propagation();
                }),
            ))
            .child(menu_divider(ob))
            .child(menu_row_button(
                "browser-pm-back-chat",
                if is_zh {
                    "返回聊天".into()
                } else {
                    "Back to chat".into()
                },
                None,
                false,
                false,
                false,
                ob,
                cx.listener(|this, _, _, cx| {
                    this.request_back_to_chat(true, cx);
                    cx.stop_propagation();
                }),
            ));

        Some(
            deferred(
                div()
                    .id("browser-page-menu-backdrop")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.page_menu_open = false;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(
                                BROWSER_FULL_TOP_HEIGHT_PX + BROWSER_ADDRESS_ROW_HEIGHT_PX + 2.0,
                            ))
                            .right(px(10.0))
                            .child(menu),
                    ),
            )
            .with_priority(70)
            .into_any_element(),
        )
    }

    /// Renders OpenBot's right-click `BrowserTabMenu`.
    fn render_openbot_tab_context_menu(
        &self,
        ob: &OpenBotPalette,
        is_zh: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let target_id = self.tab_context_menu_tab_id.clone()?;
        let idx = self.session.tabs.iter().position(|t| t.id == target_id)?;
        let len = self.session.tabs.len();
        let platform = BrowserShortcutPlatform::current();
        let can_add = len < BROWSER_MAX_TABS;
        let can_reopen = self.session.can_reopen();

        let id_new_right = target_id.clone();
        let id_reload = target_id.clone();
        let id_dup = target_id.clone();
        let id_move_l = target_id.clone();
        let id_move_r = target_id.clone();
        let id_close = target_id.clone();
        let id_close_l = target_id.clone();
        let id_close_r = target_id.clone();
        let id_close_o = target_id.clone();

        let left_offset = (if cfg!(target_os = "macos") {
            BROWSER_MAC_TRAFFIC_LIGHTS_INSET_PX
        } else {
            12.0
        }) + (idx as f32) * 140.0;

        let menu = div()
            .id("browser-tab-context-menu-card")
            .w(px(236.0))
            .p(px(5.0))
            .rounded(px(10.0))
            .bg(ob.bg_surface())
            .border_1()
            .border_color(ob.border_strong_hairline())
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(px(1.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(menu_row_button(
                "browser-tm-new-right",
                if is_zh {
                    "在右侧新建标签页".into()
                } else {
                    "New tab to the right".into()
                },
                Some(browser_shortcut_glyph(
                    BrowserShortcutName::NewTab,
                    platform,
                )),
                !can_add,
                false,
                false,
                ob,
                cx.listener(move |this, _, window, cx| {
                    let _ = this.open_new_tab(
                        None,
                        Some(id_new_right.clone()),
                        true,
                        Some(window),
                        cx,
                    );
                    cx.stop_propagation();
                }),
            ))
            .child(menu_row_button(
                "browser-tm-reload",
                if is_zh {
                    "重新加载".into()
                } else {
                    "Reload".into()
                },
                Some(browser_shortcut_glyph(
                    BrowserShortcutName::Reload,
                    platform,
                )),
                false,
                false,
                false,
                ob,
                cx.listener(move |this, _, _, cx| {
                    this.tab_context_menu_tab_id = None;
                    let _ = this.activate_tab(&id_reload, cx);
                    this.reload(cx);
                    cx.stop_propagation();
                }),
            ))
            .child(menu_row_button(
                "browser-tm-duplicate",
                if is_zh {
                    "复制标签页".into()
                } else {
                    "Duplicate tab".into()
                },
                None,
                !can_add,
                false,
                false,
                ob,
                cx.listener(move |this, _, _, cx| {
                    this.duplicate_tab(&id_dup, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(menu_divider(ob))
            .child(menu_row_button(
                "browser-tm-move-left",
                if is_zh {
                    "向左移动".into()
                } else {
                    "Move left".into()
                },
                Some(browser_shortcut_glyph(
                    BrowserShortcutName::MoveLeft,
                    platform,
                )),
                idx == 0,
                false,
                false,
                ob,
                cx.listener(move |this, _, _, cx| {
                    this.tab_context_menu_tab_id = None;
                    if idx > 0 {
                        this.move_tab(&id_move_l, idx - 1, cx);
                    }
                    cx.stop_propagation();
                }),
            ))
            .child(menu_row_button(
                "browser-tm-move-right",
                if is_zh {
                    "向右移动".into()
                } else {
                    "Move right".into()
                },
                Some(browser_shortcut_glyph(
                    BrowserShortcutName::MoveRight,
                    platform,
                )),
                idx + 1 >= len,
                false,
                false,
                ob,
                cx.listener(move |this, _, _, cx| {
                    this.tab_context_menu_tab_id = None;
                    this.move_tab(&id_move_r, idx + 1, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(menu_divider(ob))
            .child(menu_row_button(
                "browser-tm-close",
                if is_zh {
                    "关闭标签页".into()
                } else {
                    "Close tab".into()
                },
                Some(browser_shortcut_glyph(
                    BrowserShortcutName::Close,
                    platform,
                )),
                false,
                false,
                false,
                ob,
                cx.listener(move |this, _, _, cx| {
                    this.close_tabs(std::slice::from_ref(&id_close), cx);
                    cx.stop_propagation();
                }),
            ))
            .child(menu_row_button(
                "browser-tm-close-left",
                if is_zh {
                    "关闭左侧标签页".into()
                } else {
                    "Close tabs to the left".into()
                },
                None,
                idx == 0,
                false,
                false,
                ob,
                cx.listener(move |this, _, _, cx| {
                    this.close_tabs_left(&id_close_l, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(menu_row_button(
                "browser-tm-close-right",
                if is_zh {
                    "关闭右侧标签页".into()
                } else {
                    "Close tabs to the right".into()
                },
                None,
                idx + 1 >= len,
                false,
                false,
                ob,
                cx.listener(move |this, _, _, cx| {
                    this.close_tabs_right(&id_close_r, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(menu_row_button(
                "browser-tm-close-others",
                if is_zh {
                    "关闭其他标签页".into()
                } else {
                    "Close other tabs".into()
                },
                None,
                len <= 1,
                false,
                false,
                ob,
                cx.listener(move |this, _, _, cx| {
                    this.close_other_tabs(&id_close_o, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(menu_divider(ob))
            .child(menu_row_button(
                "browser-tm-reopen",
                if is_zh {
                    "重新打开关闭的标签页".into()
                } else {
                    "Reopen closed tab".into()
                },
                Some(browser_shortcut_glyph(
                    BrowserShortcutName::Reopen,
                    platform,
                )),
                !can_reopen,
                false,
                false,
                ob,
                cx.listener(|this, _, _, cx| {
                    this.reopen_closed_tab(cx);
                    cx.stop_propagation();
                }),
            ));

        Some(
            deferred(
                div()
                    .id("browser-tab-menu-backdrop")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.tab_context_menu_tab_id = None;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(BROWSER_FULL_TOP_HEIGHT_PX + 2.0))
                            .left(px(left_offset.min(540.0)))
                            .child(menu),
                    ),
            )
            .with_priority(75)
            .into_any_element(),
        )
    }

    /// Renders dismissible popup-blocked alert bar (`BrowserFullView.tsx`).
    fn render_openbot_popup_alert(
        &self,
        ob: &OpenBotPalette,
        _is_zh: bool,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let failure = self.session.active_tab()?.popup_failure.as_ref()?;
        if self.session.dismissed_popup_failures.contains(&failure.id) {
            return None;
        }
        let failure_id = failure.id.clone();
        let message = failure.message.clone();
        Some(
            div()
                .id("browser-popup-failure-alert")
                .flex_shrink_0()
                .mx(px(8.0))
                .mt(px(6.0))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(soft_rgba(ob.warning(), 0.14))
                .border_1()
                .border_color(soft_rgba(ob.warning(), 0.30))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(openbot_browser_icon(
                    OpenBotBrowserIcon::TriangleAlert,
                    14.0,
                    ob.warning(),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(12.0))
                        .text_color(ob.text_primary())
                        .truncate()
                        .child(message),
                )
                .child(
                    div()
                        .id("browser-popup-alert-dismiss")
                        .w(px(20.0))
                        .h(px(20.0))
                        .rounded(px(5.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(ob.hover()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.session
                                    .dismissed_popup_failures
                                    .insert(failure_id.clone());
                                cx.notify();
                            }),
                        )
                        .child(openbot_browser_icon(
                            OpenBotBrowserIcon::CloseX,
                            11.5,
                            ob.text_secondary(),
                        )),
                ),
        )
    }

    /// Renders OpenBot's `BrowserTeachBar` (ready state + red-tinted live recording state).
    fn render_openbot_teach_bar(
        &self,
        ob: &OpenBotPalette,
        is_zh: bool,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let name = &self.dock.active_agent.name;
        match &self.teach {
            BrowserTeachUiState::Inactive => None,
            BrowserTeachUiState::Ready => Some(
                div()
                    .id("browser-teach-bar")
                    .flex_shrink_0()
                    .w_full()
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(ob.border_hairline())
                    .bg(ob.bg_surface())
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.5))
                            .text_color(ob.text_primary())
                            .truncate()
                            .child(if is_zh {
                                format!("在浏览器中演示一次操作，让 {name} 学会这项任务")
                            } else {
                                format!("Show {name} how to complete a task in this browser")
                            }),
                    )
                    .child(
                        div()
                            .id("browser-teach-start-btn")
                            .h(px(28.0))
                            .px(px(10.0))
                            .rounded_full()
                            .bg(ob.bg_control_active())
                            .border_1()
                            .border_color(ob.border_strong_hairline())
                            .hover(|s| s.bg(ob.hover()))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.start_teach_recording(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(ob.danger()))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_primary())
                                    .child(if is_zh { "开始录制" } else { "Start recording" }),
                            ),
                    )
                    .child(
                        div()
                            .id("browser-teach-close-btn")
                            .w(px(26.0))
                            .h(px(26.0))
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(ob.hover()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.discard_teach_recording(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(openbot_browser_icon(
                                OpenBotBrowserIcon::CloseX,
                                13.0,
                                ob.text_muted(),
                            )),
                    ),
            ),
            BrowserTeachUiState::Recording { .. } => {
                let elapsed = self.teach.elapsed_label(Self::now_epoch_ms());
                Some(
                    div()
                        .id("browser-teach-bar-recording")
                        .flex_shrink_0()
                        .w_full()
                        .px(px(12.0))
                        .py(px(8.0))
                        .border_b_1()
                        .border_color(soft_rgba(ob.danger(), 0.40))
                        .bg(soft_rgba(ob.danger(), 0.14))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(ob.danger()))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_primary())
                                .truncate()
                                .child(if is_zh {
                                    format!("{name} 正在录制你的步骤")
                                } else {
                                    format!("{name} is recording your steps")
                                }),
                        )
                        .child(
                            div()
                                .id("browser-teach-stop-btn")
                                .h(px(28.0))
                                .px(px(10.0))
                                .rounded_full()
                                .bg(ob.danger())
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.save_teach_recording(cx);
                                        cx.stop_propagation();
                                    }),
                                )
                                .child(openbot_browser_icon(
                                    OpenBotBrowserIcon::StopSquare,
                                    10.0,
                                    rgb(0xFFFFFF),
                                ))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .child(elapsed),
                                ),
                        )
                        .child(
                            div()
                                .id("browser-teach-discard-btn")
                                .w(px(26.0))
                                .h(px(26.0))
                                .rounded(px(6.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(ob.hover()))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.discard_teach_recording(cx);
                                        cx.stop_propagation();
                                    }),
                                )
                                .child(openbot_browser_icon(
                                    OpenBotBrowserIcon::CloseX,
                                    13.0,
                                    ob.text_primary(),
                                )),
                        ),
                )
            }
        }
    }

    /// Renders OpenBot's bottom floating translucent glass `BrowserDock` + stacked `BrowserIsland` (`image_2.png`).
    fn render_openbot_browser_dock(
        &self,
        ob: &OpenBotPalette,
        is_zh: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let now_ms = Self::now_epoch_ms();
        let control_mode = self.control_mode();
        let island = self.dock.resolve_island(&control_mode, now_ms, is_zh);
        let glass_bg = dock_glass_bg(ob);
        let glass_border = dock_glass_border(ob);

        // Tucked compact pill mode (`BrowserDock.tsx` `.browser-dock-tab`)
        if self.dock.tucked {
            let label = self.dock.tucked_label(island.as_ref(), is_zh);
            let show_done = matches!(
                island,
                Some(BrowserIslandView::ActivityUser | BrowserIslandView::ActivityRequest { .. })
            );
            let has_unread = matches!(island, Some(BrowserIslandView::Reply { .. }));
            return div()
                .id("browser-dock-tucked-pill")
                .max_w(px(360.0))
                .h(px(36.0))
                .pl(px(8.0))
                .pr(px(if show_done { 5.0 } else { 10.0 }))
                .rounded_full()
                .bg(glass_bg)
                .border_1()
                .border_color(glass_border)
                .shadow_lg()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("browser-dock-tucked-open-btn")
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .min_w(px(0.0))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_dock_tucked(cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(
                            div()
                                .relative()
                                .child(render_browser_owner_avatar(
                                    Some(&self.dock.active_agent),
                                    self.dock.working,
                                    20.0,
                                    ob,
                                ))
                                .when(has_unread && !self.dock.working, |el| {
                                    el.child(
                                        div()
                                            .absolute()
                                            .top(px(-1.0))
                                            .right(px(-2.0))
                                            .w(px(7.0))
                                            .h(px(7.0))
                                            .rounded_full()
                                            .bg(ob.accent()),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(px(12.5))
                                .text_color(ob.text_primary())
                                .child(label),
                        )
                        .child(openbot_browser_icon(
                            OpenBotBrowserIcon::ChevronUp,
                            14.0,
                            ob.text_secondary(),
                        )),
                )
                .when(show_done, |pill| {
                    pill.child(
                        div()
                            .id("browser-dock-tucked-done-btn")
                            .h(px(26.0))
                            .px(px(10.0))
                            .rounded_full()
                            .bg(ob.surface_light())
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_on_light())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.user_resume_ai(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(if is_zh { "完成" } else { "Done" }),
                    )
                });
        }

        // Expanded floating BrowserDock stack (`max-w: 640px`)
        let mut stack = div()
            .id("browser-dock-expanded")
            .w_full()
            .max_w(px(BROWSER_DOCK_MAX_WIDTH_PX))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0));

        // 1. Optional stacked `BrowserIsland` above composer pill
        if let Some(island_view) = island {
            let island_card: AnyElement = match island_view {
                BrowserIslandView::ActivityAgent { label } => div()
                    .id("browser-island-activity-agent")
                    .h(px(34.0))
                    .pl(px(10.0))
                    .pr(px(5.0))
                    .rounded_full()
                    .bg(glass_bg)
                    .border_1()
                    .border_color(glass_border)
                    .shadow_md()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(openbot_browser_icon(
                        OpenBotBrowserIcon::LoaderSpin,
                        13.5,
                        ob.text_muted(),
                    ))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(ob.text_secondary())
                            .child(label),
                    )
                    .child(
                        div()
                            .id("browser-takeover-btn")
                            .h(px(24.0))
                            .px(px(10.0))
                            .rounded_full()
                            .bg(ob.bg_control_active())
                            .border_1()
                            .border_color(ob.border_strong_hairline())
                            .hover(|s| s.bg(ob.hover()))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_primary())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.user_take_over(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(if is_zh { "接管" } else { "Take over" }),
                    )
                    .into_any_element(),

                BrowserIslandView::ActivityUser => div()
                    .id("browser-island-activity-user")
                    .h(px(34.0))
                    .pl(px(10.0))
                    .pr(px(5.0))
                    .rounded_full()
                    .bg(glass_bg)
                    .border_1()
                    .border_color(glass_border)
                    .shadow_md()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(openbot_browser_icon(
                        OpenBotBrowserIcon::MousePointer2,
                        13.5,
                        ob.success(),
                    ))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(ob.text_primary())
                            .child(if is_zh {
                                "你正在控制该页面"
                            } else {
                                "You're controlling this page"
                            }),
                    )
                    .child(
                        div()
                            .id("browser-resume-ai-btn")
                            .h(px(24.0))
                            .px(px(10.0))
                            .rounded_full()
                            .bg(ob.surface_light())
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_on_light())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.user_resume_ai(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(if is_zh { "完成" } else { "Done" }),
                    )
                    .into_any_element(),

                BrowserIslandView::ActivityRequest { agent_name, reason } => div()
                    .id("browser-island-activity-request")
                    .min_h(px(34.0))
                    .pl(px(10.0))
                    .pr(px(5.0))
                    .py(px(4.0))
                    .rounded_full()
                    .bg(glass_bg)
                    .border_1()
                    .border_color(soft_rgba(ob.warning(), 0.40))
                    .shadow_md()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(openbot_browser_icon(
                        OpenBotBrowserIcon::MousePointer2,
                        13.5,
                        ob.warning(),
                    ))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(ob.text_primary())
                            .child(if reason.trim().is_empty() {
                                if is_zh {
                                    format!("{agent_name} 需要你在该页面操作")
                                } else {
                                    format!("{agent_name} needs you on this page")
                                }
                            } else if is_zh {
                                format!("{agent_name} 需要你在该页面操作：{reason}")
                            } else {
                                format!("{agent_name} needs you: {reason}")
                            }),
                    )
                    .child(
                        div()
                            .id("browser-request-done-btn")
                            .h(px(24.0))
                            .px(px(10.0))
                            .rounded_full()
                            .bg(ob.surface_light())
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_on_light())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.user_resume_ai(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(if is_zh { "完成" } else { "Done" }),
                    )
                    .into_any_element(),

                BrowserIslandView::Reply {
                    id,
                    body,
                    is_question,
                    ..
                } => {
                    let dismiss_id = id.clone();
                    div()
                        .id("browser-island-reply-card")
                        .w_full()
                        .p(px(12.0))
                        .rounded(px(14.0))
                        .bg(glass_bg)
                        .border_1()
                        .border_color(glass_border)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            this.dock.reply_hovered = *hovered;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(render_browser_owner_avatar(
                                    Some(&self.dock.active_agent),
                                    false,
                                    16.0,
                                    ob,
                                ))
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(ob.text_secondary())
                                        .child(self.dock.active_agent.name.clone()),
                                )
                                .child(
                                    div()
                                        .id("browser-island-reply-close")
                                        .ml_auto()
                                        .w(px(22.0))
                                        .h(px(22.0))
                                        .rounded(px(6.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(ob.hover()))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.dock.dismiss_reply(&dismiss_id);
                                                cx.notify();
                                                cx.stop_propagation();
                                            }),
                                        )
                                        .child(openbot_browser_icon(
                                            OpenBotBrowserIcon::CloseX,
                                            12.0,
                                            ob.text_muted(),
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(ob.text_primary())
                                .child(body),
                        )
                        .when(is_question, |card| {
                            card.child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .child(
                                        div()
                                            .id("browser-island-answer-in-chat")
                                            .h(px(26.0))
                                            .px(px(10.0))
                                            .rounded_full()
                                            .bg(ob.bg_control_active())
                                            .border_1()
                                            .border_color(ob.border_strong_hairline())
                                            .hover(|s| s.bg(ob.hover()))
                                            .cursor_pointer()
                                            .flex()
                                            .items_center()
                                            .text_size(px(12.0))
                                            .text_color(ob.text_primary())
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.request_back_to_chat(false, cx);
                                                    cx.stop_propagation();
                                                }),
                                            )
                                            .child(if is_zh {
                                                "在对话中回答"
                                            } else {
                                                "Answer in chat"
                                            }),
                                    ),
                            )
                        })
                        .into_any_element()
                }

                BrowserIslandView::Approval { title, detail, .. } => div()
                    .id("browser-island-approval-card")
                    .w_full()
                    .p(px(12.0))
                    .rounded(px(14.0))
                    .bg(glass_bg)
                    .border_1()
                    .border_color(soft_rgba(ob.accent(), 0.45))
                    .shadow_lg()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ob.text_primary())
                                    .child(title),
                            )
                            .when(!detail.is_empty(), |col| {
                                col.child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(ob.text_secondary())
                                        .truncate()
                                        .child(detail),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("browser-island-approval-open-chat")
                            .h(px(28.0))
                            .px(px(11.0))
                            .rounded_full()
                            .bg(ob.accent())
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(0xFFFFFF))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.request_back_to_chat(false, cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(if is_zh { "去对话审批" } else { "Review in chat" }),
                    )
                    .into_any_element(),
            };
            stack = stack.child(island_card);
        }

        // 2. Optional floating queued messages summary
        if !self.dock.queued_messages.is_empty() {
            let count = self.dock.queued_messages.len();
            let first = self
                .dock
                .queued_messages
                .first()
                .map(|(_, t)| t.as_str())
                .unwrap_or("");
            stack = stack.child(
                div()
                    .w_full()
                    .px(px(24.0))
                    .child(
                        div()
                            .w_full()
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded_t(px(12.0))
                            .bg(glass_bg)
                            .border_1()
                            .border_color(glass_border)
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_secondary())
                                    .child(if is_zh {
                                        format!("队列 ({count}):")
                                    } else {
                                        format!("Queued ({count}):")
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_size(px(11.5))
                                    .text_color(ob.text_muted())
                                    .child(first.to_string()),
                            ),
                    ),
            );
        }

        // 3. Main Floating Glass Composer Pill (`image_2.png`: `[avatar] 发消息给 New agent   ⌄  [↑]`)
        let dock_focused = self.active_input == ActiveBrowserInput::BrowserDockInput;
        let has_draft = !self.dock.draft.trim().is_empty();
        let show_stop = self.dock.working && !has_draft;
        let placeholder = if is_zh {
            format!("发消息给 {}", self.dock.active_agent.name)
        } else {
            format!("Message {}", self.dock.active_agent.name)
        };

        let send_btn_bg = if show_stop || has_draft {
            ob.surface_light()
        } else {
            soft_rgba(ob.surface_light(), 0.72)
        };

        let composer_pill = div()
            .id("browser-dock-composer-pill")
            .w_full()
            .h(px(44.0))
            .pl(px(10.0))
            .pr(px(6.0))
            .rounded_full()
            .bg(glass_bg)
            .border_1()
            .border_color(if dock_focused {
                ob.border_strong_hairline()
            } else {
                glass_border
            })
            .shadow_lg()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(render_browser_owner_avatar(
                Some(&self.dock.active_agent),
                self.dock.working,
                22.0,
                ob,
            ))
            .child(
                div()
                    .id("browser-dock-input")
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .flex()
                    .items_center()
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.close_all_browser_popovers();
                            this.set_active_input(
                                ActiveBrowserInput::BrowserDockInput,
                                Some(window),
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_size(px(13.5))
                            .text_color(if has_draft || dock_focused {
                                ob.text_primary()
                            } else {
                                ob.text_muted()
                            })
                            .child(if dock_focused {
                                if self.dock.draft.is_empty() {
                                    format!("| {placeholder}")
                                } else {
                                    format!("{}|", self.dock.draft)
                                }
                            } else if self.dock.draft.is_empty() {
                                placeholder
                            } else {
                                self.dock.draft.clone()
                            }),
                    ),
            )
            // Tuck chevron (`⌄`)
            .child(
                div()
                    .id("browser-dock-tuck-btn")
                    .w(px(28.0))
                    .h(px(28.0))
                    .rounded_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(ob.hover()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_dock_tucked(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(openbot_browser_icon(
                        OpenBotBrowserIcon::ChevronDown,
                        15.0,
                        ob.text_secondary(),
                    )),
            )
            // Send (`↑`) or Stop (`■`) circular white button
            .child(
                div()
                    .id("browser-dock-send-btn")
                    .w(px(32.0))
                    .h(px(32.0))
                    .rounded_full()
                    .flex_shrink_0()
                    .bg(send_btn_bg)
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if show_stop {
                                cx.emit(BrowserWorkspaceEvent::StopActiveTurn);
                            } else {
                                this.submit_dock_message(cx);
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .child(openbot_browser_icon(
                        if show_stop {
                            OpenBotBrowserIcon::StopSquare
                        } else {
                            OpenBotBrowserIcon::ArrowUp
                        },
                        if show_stop { 12.0 } else { 15.0 },
                        ob.text_on_light(),
                    )),
            );

        stack.child(composer_pill)
    }
}
