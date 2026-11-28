//! Gemini-inspired unified collapsible left sidebar views (`288px` expanded / `64px` collapsed rail).

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::components::bloub_avatar::set_live_avatar_group_hover;
use crate::i18n::tr_str;
use crate::theme::{tg_icon, tg_icon_rgba, IconName, TdesktopThemePalette};
use crate::views::chat_sidebar::{
    DialogItemModel, COLLAPSED_WIDTH, OPENBOT_SIDEBAR_COMPACT_WIDTH,
};
use crate::views::nav_rail::{NavDestination, NavRail};
use crate::views::sidebar::dialogs_row_view::{
    render_dialog_avatar, render_openbot_dialog_avatar_with_badge_interactive,
    resolve_dialog_felt_badge_state, resolve_dialog_preview_priority, SidebarDialogMeta,
};
use crate::views::sidebar::openbot_sidebar_chrome::{
    openbot_vector_icon, OpenBotChromeIcon,
};

pub use super::gemini_expanded_sidebar::*;

/// Snapshot of workspace navigation and account state rendered inside the unified sidebar.
#[derive(Debug, Clone)]
pub struct UnifiedNavSnapshot {
    pub active_destination: NavDestination,
    pub active_folder_id: Option<i32>,
    pub chats_unread: u32,
    pub calls_unread: u32,
    pub tasks_unread: u32,
    pub mcp_unread: u32,
    pub saved_unread: u32,
    pub community_unread: u32,
    pub account_initials: String,
    pub account_name: String,
    pub is_connected: bool,
}

impl UnifiedNavSnapshot {
    /// Derives a live snapshot from an optional [`NavRail`] and dialog unread counts.
    pub fn from_rail_and_unread(
        rail: Option<&NavRail>,
        standalone_dest: NavDestination,
        total_dialog_unread: u32,
    ) -> Self {
        if let Some(r) = rail {
            let badge_for = |dest: NavDestination| -> u32 {
                r.items
                    .iter()
                    .find(|it| it.destination == dest)
                    .map(|it| it.unread_badge)
                    .unwrap_or(0)
            };
            Self {
                active_destination: r.active_destination,
                active_folder_id: r.active_folder_id,
                chats_unread: badge_for(NavDestination::Chats).max(total_dialog_unread),
                calls_unread: badge_for(NavDestination::Calls),
                tasks_unread: badge_for(NavDestination::TaskCenter),
                mcp_unread: badge_for(NavDestination::McpTools),
                saved_unread: badge_for(NavDestination::SavedMessages),
                community_unread: badge_for(NavDestination::Community),
                account_initials: r.account_initials.clone(),
                account_name: r.account_name.clone(),
                is_connected: r.is_connected,
            }
        } else {
            Self {
                active_destination: standalone_dest,
                active_folder_id: None,
                chats_unread: total_dialog_unread,
                calls_unread: 0,
                tasks_unread: 0,
                mcp_unread: 0,
                saved_unread: 0,
                community_unread: 0,
                account_initials: "TG".to_string(),
                account_name: String::new(),
                is_connected: false,
            }
        }
    }
}

/// User actions emitted by the unified Gemini-style sidebar chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnifiedSidebarAction {
    ToggleCollapsed,
    SelectDestination(NavDestination),
    SelectDialog(u64),
    OpenMainMenu,
    OpenAddFriend,
    ToggleTheme,
    Logout,
    FocusSearch,
    ExitSearch,
    ClearSearch,
    TogglePlusMenu,
    ToggleNavMore,
    OpenContacts,
    SetHover(Option<&'static str>),
    SetHoverDialog(Option<u64>),
    // OpenBot `383a35d2` sidebar chrome actions (`UI-F05`..`UI-F06`)
    NavigateBack,
    NavigateForward,
    ToggleOpenBotCompact,
    OpenGlobalSearch,
    ToggleFilterMenu,
    ToggleNewMenu,
    ToggleTabsEditMenu,
    CreateNewAgent,
    CreateNewGroup,
    CreateNewFolder,
    CloseOpenBotPopovers,
}

pub(super) fn format_compact_badge(count: u32) -> String {
    if count > 999 {
        "999+".to_string()
    } else {
        count.to_string()
    }
}

/// High-contrast floating pill tooltip painted in GPUI's deferred pass above adjacent panes.
pub fn render_floating_pill_tooltip(label: &str, palette: &TdesktopThemePalette) -> Deferred {
    render_floating_pill_tooltip_at(label, 54.0, 3.0, palette)
}

pub(super) fn render_floating_pill_tooltip_at(
    label: &str,
    left_px: f32,
    top_px: f32,
    palette: &TdesktopThemePalette,
) -> Deferred {
    let is_apple = palette.is_apple_style();
    let (pill_bg, pill_fg) = if palette.mode.is_dark() {
        (rgb(0xF1F3F4), rgb(0x1F1F1F))
    } else {
        (rgb(0x202124), rgb(0xFFFFFF))
    };

    deferred(
        div()
            .absolute()
            .left(px(left_px))
            .top(px(top_px))
            .px(px(12.0))
            .py(px(6.0))
            .rounded_full()
            .bg(pill_bg)
            .when(is_apple, |el| el.shadow(palette.popover_shadows()))
            .when(!is_apple, |el| el.shadow_md())
            .whitespace_nowrap()
            .text_size(px(12.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(pill_fg)
            .child(label.to_string()),
    )
    .with_priority(crate::theme::OPENBOT_LAYER_TOOLTIP)
}

fn render_collapsed_icon_btn<V: 'static>(
    id: &'static str,
    hover_key: &'static str,
    icon: IconName,
    tooltip_label: &str,
    is_active: bool,
    unread_badge: u32,
    hovered_item: Option<&'static str>,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, UnifiedSidebarAction, &mut Context<V>) + Clone + 'static,
    click_action: UnifiedSidebarAction,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let is_hovered = hovered_item == Some(hover_key);
    let active_bg = TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.16);
    let hover_bg = if is_apple {
        if palette.mode.is_dark() {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.08)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.06)
        }
    } else {
        rgb(palette.dialogs_hover_bg)
    };
    let icon_color = if is_active {
        palette.accent
    } else if is_hovered {
        palette.text_primary
    } else {
        palette.icon_fg
    };
    let on_hover_cb = on_action.clone();
    let on_click_cb = on_action;

    div()
        .id(id)
        .relative()
        .w(px(36.0))
        .h(px(36.0))
        .rounded_full()
        .cursor_pointer()
        .when(is_active, |el| el.bg(active_bg))
        .hover(move |s| if is_active { s } else { s.bg(hover_bg) })
        .flex()
        .items_center()
        .justify_center()
        .on_hover(cx.listener(move |this, hovered, _w, cx| {
            let next = if *hovered { Some(hover_key) } else { None };
            on_hover_cb(this, UnifiedSidebarAction::SetHover(next), cx);
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev, _w, cx| {
                on_click_cb(this, click_action, cx);
            }),
        )
        .child(tg_icon(icon, 18.0, icon_color))
        .when(unread_badge > 0, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(1.0))
                    .right(px(0.0))
                    .min_w(px(16.0))
                    .h(px(16.0))
                    .px(px(4.0))
                    .rounded_full()
                    .bg(rgb(palette.unread_badge_bg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(palette.unread_badge_fg))
                    .child(format_compact_badge(unread_badge)),
            )
        })
        .when(is_hovered, |el| {
            el.child(render_floating_pill_tooltip(tooltip_label, palette))
        })
        .into_any_element()
}

/// Renders OpenBot's clean `88px` compact avatar-only sidebar: centered `32px` search button
/// opening `GlobalSearch` (`Cmd+K`) + single centered column of `60x60px` avatar tiles (`48x48px`
/// 3D plush/cluster avatars + felt status badges) with ZERO duplicate compose/contacts/browser/account footer!
#[allow(clippy::too_many_arguments)]
pub fn render_openbot_compact_sidebar<V: 'static>(
    dialogs: &[(&DialogItemModel, SidebarDialogMeta)],
    is_loading_dialogs: bool,
    active_chat_id: Option<u64>,
    hovered_dialog_id: Option<u64>,
    external_topbar_hosted: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, UnifiedSidebarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let search_layout = crate::views::sidebar::openbot_sidebar_chrome::compute_openbot_sidebar_search_row_layout(
        true,
        false,
        dialogs.len(),
        false,
        palette,
        lang_id,
    );
    let compact_grid = crate::views::sidebar::dialogs_renderer::compute_openbot_pinned_grid_layout(
        dialogs.len().max(1),
        true,
        palette,
    );
    let ob = palette.openbot_palette();
    let is_dark = palette.mode.is_dark();
    let hover_bg = ob.hover();
    let active_bg = if is_dark { ob.bg_surface() } else { rgb(0xE8E8EB) };
    let border_col = if is_dark { ob.border_hairline() } else { ob.border() };
    let muted = ob.text_muted();

    let on_toggle = on_action.clone();
    let on_search = on_action.clone();

    div()
        .id("unified-sidebar-collapsed")
        .relative()
        .w(px(OPENBOT_SIDEBAR_COMPACT_WIDTH))
        .flex_shrink_0()
        .h_full()
        .bg(ob.bg_sidebar())
        .border_r_1()
        .border_color(border_col)
        .flex()
        .flex_col()
        .items_center()
        .pb(px(10.0))
        .when(!external_topbar_hosted, |el| {
            el.child(
                div()
                    .w_full()
                    .h(px(38.0))
                    .flex_shrink_0()
                    .pt(px(2.0))
                    .px(px(12.0))
                    .window_control_area(WindowControlArea::Drag)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id("sidebar-expand-btn")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover_bg))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    on_toggle(this, UnifiedSidebarAction::ToggleOpenBotCompact, cx);
                                }),
                            )
                            .child(openbot_vector_icon(OpenBotChromeIcon::PanelLeft, 16.0, muted)),
                    ),
            )
        })
        .when(search_layout.compact_search_visible, |el| {
            el.child(
                div()
                    .w_full()
                    .px(px(12.0))
                    .pt(px(if external_topbar_hosted { 11.0 } else { 6.0 }))
                    .pb(px(6.0))
                    .flex_shrink_0()
                    .child(
                        div()
                            .id("sidebar-compact-search-btn")
                            .w_full()
                            .h(px(search_layout.compact_search_size_px))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover_bg))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    on_search(this, UnifiedSidebarAction::OpenGlobalSearch, cx);
                                }),
                            )
                            .child(tg_icon_rgba(IconName::Search, 16.0, muted)),
                    ),
            )
        })
        .child(
            div()
                .id("rail-dialogs-scroll")
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_y_scroll()
                .px(px(12.0))
                .py(px(4.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .when(is_loading_dialogs && dialogs.is_empty(), |el| {
                    let skel_col = ob.bg_control_hover();
                    let fades: [f32; 5] = [0.95, 0.78, 0.62, 0.46, 0.30];
                    el.child(
                        div()
                            .id("rail-dialogs-loading-skeleton")
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(10.0))
                            .children(fades.into_iter().map(move |alpha| {
                                div()
                                    .w(px(48.0))
                                    .h(px(48.0))
                                    .rounded_full()
                                    .bg(skel_col)
                                    .opacity(alpha)
                            })),
                    )
                })
                .children(dialogs.iter().map(|(dialog, meta)| {
                    let dialog_id = dialog.id;
                    let is_active = active_chat_id == Some(dialog_id);
                    let is_hovered = hovered_dialog_id == Some(dialog_id);
                    let on_d_hover = on_action.clone();
                    let on_d_click = on_action.clone();
                    let tile_surface_hex = if is_active {
                        if is_dark { 0x212121 } else { 0xE8E8EB }
                    } else if is_dark {
                        0x1A1A1A
                    } else {
                        0xF5F5F6
                    };
                    let badge_state = resolve_dialog_felt_badge_state(
                        dialog,
                        meta,
                        &resolve_dialog_preview_priority(dialog, meta, lang_id),
                    );
                    let hover_key = format!("sidebar-rail:{dialog_id}");
                    let avatar = render_openbot_dialog_avatar_with_badge_interactive(
                        dialog,
                        meta,
                        palette,
                        compact_grid.avatar_px,
                        tile_surface_hex,
                        is_active,
                        badge_state,
                        "sidebar-rail",
                        Some(&hover_key),
                        is_hovered,
                        true,
                    );

                    div()
                        .id(("rail-dialog-btn", dialog_id as usize))
                        .relative()
                        .w(px(compact_grid.tile_w_px))
                        .h(px(compact_grid.tile_h_px))
                        .p(px(6.0))
                        .rounded(px(8.0))
                        .flex_shrink_0()
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(is_active, |el| el.bg(active_bg))
                        .when(!is_active && is_hovered, |el| el.bg(hover_bg))
                        .on_hover(cx.listener(move |this, hovered, _w, cx| {
                            set_live_avatar_group_hover(&hover_key, *hovered);
                            let next = if *hovered { Some(dialog_id) } else { None };
                            on_d_hover(this, UnifiedSidebarAction::SetHoverDialog(next), cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                on_d_click(this, UnifiedSidebarAction::SelectDialog(dialog_id), cx);
                            }),
                        )
                        .child(avatar)
                        .when(is_hovered, |el| {
                            el.child(render_floating_pill_tooltip_at(
                                &dialog.peer_name,
                                68.0,
                                16.0,
                                palette,
                            ))
                        })
                })),
        )
        .into_any_element()
}

/// Renders the `64px` collapsed rail with expand toggle, new-chat/contacts shortcuts, scrollable chat avatars, add-friend & account triggers.
#[allow(clippy::too_many_arguments)]
pub fn render_collapsed_gemini_sidebar<V: 'static>(
    nav: &UnifiedNavSnapshot,
    dialogs: &[(&DialogItemModel, SidebarDialogMeta)],
    is_loading_dialogs: bool,
    active_chat_id: Option<u64>,
    hovered_item: Option<&'static str>,
    hovered_dialog_id: Option<u64>,
    has_leading_app_sidebar: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, UnifiedSidebarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    if is_apple && has_leading_app_sidebar {
        return render_openbot_compact_sidebar(
            dialogs,
            is_loading_dialogs,
            active_chat_id,
            hovered_dialog_id,
            false,
            palette,
            lang_id,
            cx,
            on_action,
        );
    }
    let toggle_hovered = hovered_item == Some("toggle");
    let compose_hovered = hovered_item == Some("compose");
    let account_hovered = hovered_item == Some("account");
    let hover_bg = if is_apple {
        if palette.mode.is_dark() {
            TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.08)
        } else {
            TdesktopThemePalette::rgba_with_alpha(0x000000, 0.06)
        }
    } else {
        rgb(palette.dialogs_hover_bg)
    };
    let active_ring = rgb(palette.accent);
    let online_border = if is_apple {
        palette.sidebar_bg_rgba()
    } else {
        rgb(palette.dialogs_bg)
    };
    let avatar_grad = palette.peer_avatar_gradient(1);

    let on_toggle_hover = on_action.clone();
    let on_toggle_click = on_action.clone();
    let on_compose_hover = on_action.clone();
    let on_compose_click = on_action.clone();
    let on_account_hover = on_action.clone();
    let on_account_click = on_action.clone();

    div()
        .id("unified-sidebar-collapsed")
        .relative()
        .w(px(palette.scaled_px(COLLAPSED_WIDTH).min(96.0)))
        .flex_shrink_0()
        .h_full()
        .bg(if is_apple {
            palette.sidebar_bg_rgba()
        } else {
            rgb(palette.dialogs_bg)
        })
        .border_r_1()
        .border_color(if is_apple {
            palette.separator_rgba()
        } else {
            rgb(palette.border_fg)
        })
        .flex()
        .flex_col()
        .items_center()
        .justify_between()
        .when(!cfg!(target_os = "macos") || has_leading_app_sidebar, |el| el.pt(px(10.0)))
        .pb(px(10.0))
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .flex_shrink_0()
                .gap(px(6.0))
                .when(cfg!(target_os = "macos") && !has_leading_app_sidebar, |el| {
                    el.child(
                        div()
                            .w_full()
                            .h(px(52.0))
                            .flex_shrink_0()
                            .window_control_area(WindowControlArea::Drag),
                    )
                })
                .child(
                    div()
                        .id("sidebar-expand-btn")
                        .relative()
                        .w(px(36.0))
                        .h(px(36.0))
                        .rounded(px(if is_apple { 8.0 } else { 18.0 }))
                        .cursor_pointer()
                        .when(toggle_hovered, |el| el.bg(hover_bg))
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_hover(cx.listener(move |this, hovered, _w, cx| {
                            let next = if *hovered { Some("toggle") } else { None };
                            on_toggle_hover(this, UnifiedSidebarAction::SetHover(next), cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                on_toggle_click(this, UnifiedSidebarAction::ToggleCollapsed, cx);
                            }),
                        )
                        .child(tg_icon(
                            IconName::SidebarLeftOpen,
                            18.0,
                            if toggle_hovered {
                                palette.text_primary
                            } else {
                                palette.icon_fg
                            },
                        ))
                        .when(toggle_hovered, |el| {
                            el.child(render_floating_pill_tooltip(
                                tr_str(lang_id, "打开边栏", "Open sidebar"),
                                palette,
                            ))
                        }),
                )
                .child(
                    div()
                        .id("rail-compose-btn")
                        .relative()
                        .w(px(36.0))
                        .h(px(36.0))
                        .rounded(px(if is_apple { 8.0 } else { 18.0 }))
                        .cursor_pointer()
                        .when(!is_apple, |el| {
                            el.bg(rgb(palette.input_bg))
                                .border_1()
                                .border_color(rgb(palette.border_fg))
                        })
                        .when(compose_hovered, |el| el.bg(hover_bg))
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_hover(cx.listener(move |this, hovered, _w, cx| {
                            let next = if *hovered { Some("compose") } else { None };
                            on_compose_hover(this, UnifiedSidebarAction::SetHover(next), cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                on_compose_click(this, UnifiedSidebarAction::TogglePlusMenu, cx);
                            }),
                        )
                        .child(tg_icon(
                            IconName::ComposePen,
                            17.0,
                            if compose_hovered {
                                palette.text_primary
                            } else {
                                palette.icon_fg
                            },
                        ))
                        .when(compose_hovered, |el| {
                            el.child(render_floating_pill_tooltip(
                                tr_str(lang_id, "发起新对话", "New Chat"),
                                palette,
                            ))
                        }),
                )
                .child(render_collapsed_icon_btn(
                    "rail-contacts-btn",
                    "contacts",
                    IconName::ContactsUsers,
                    tr_str(lang_id, "联系人", "Contacts"),
                    false,
                    0,
                    hovered_item,
                    palette,
                    cx,
                    on_action.clone(),
                    UnifiedSidebarAction::OpenContacts,
                ))
                .child(render_collapsed_icon_btn(
                    "rail-browser-btn",
                    "browser",
                    IconName::ScreenShare,
                    tr_str(lang_id, "内置浏览器", "Browser"),
                    nav.active_destination == NavDestination::Browser,
                    0,
                    hovered_item,
                    palette,
                    cx,
                    on_action.clone(),
                    UnifiedSidebarAction::SelectDestination(
                        if nav.active_destination == NavDestination::Browser {
                            NavDestination::Chats
                        } else {
                            NavDestination::Browser
                        },
                    ),
                ))
                .when(!has_leading_app_sidebar, |col| {
                    col.child(render_collapsed_icon_btn(
                        "rail-community-btn",
                        "community",
                        IconName::CommunityGlobe,
                        tr_str(lang_id, "社区", "Community"),
                        nav.active_destination == NavDestination::Community,
                        nav.community_unread,
                        hovered_item,
                        palette,
                        cx,
                        on_action.clone(),
                        UnifiedSidebarAction::SelectDestination(
                            if nav.active_destination == NavDestination::Community {
                                NavDestination::Chats
                            } else {
                                NavDestination::Community
                            },
                        ),
                    ))
                })
                .child(
                    div()
                        .w(px(24.0))
                        .h(px(1.0))
                        .my(px(2.0))
                        .bg(if is_apple {
                            palette.separator_rgba()
                        } else {
                            rgb(palette.border_fg)
                        }),
                ),
        )
        .child(
            div()
                .id("rail-dialogs-scroll")
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_y_scroll()
                .py(px(4.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .when(is_loading_dialogs && dialogs.is_empty(), |el| {
                    let skel_col = if palette.mode.is_dark() {
                        TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.10)
                    } else {
                        TdesktopThemePalette::rgba_with_alpha(0x000000, 0.08)
                    };
                    let fades: [f32; 5] = [0.95, 0.78, 0.62, 0.46, 0.30];
                    el.child(
                        div()
                            .id("rail-dialogs-loading-skeleton")
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(10.0))
                            .children(fades.into_iter().map(move |alpha| {
                                div()
                                    .w(px(36.0))
                                    .h(px(36.0))
                                    .rounded_full()
                                    .bg(skel_col)
                                    .opacity(alpha)
                            }))
                            .with_animation(
                                "rail-dialogs-skeleton-pulse",
                                Animation::new(std::time::Duration::from_millis(1200))
                                    .repeat()
                                    .with_easing(pulsating_between(0.45, 1.0)),
                                |container, delta| container.opacity(delta),
                            ),
                    )
                })
                .children(dialogs.iter().map(|(dialog, meta)| {
                    let dialog_id = dialog.id;
                    let is_active = active_chat_id == Some(dialog_id);
                    let is_hovered = hovered_dialog_id == Some(dialog_id);
                    let unread_bg = if dialog.is_muted {
                        rgb(palette.muted_badge_bg)
                    } else {
                        rgb(palette.unread_badge_bg)
                    };
                    let on_d_hover = on_action.clone();
                    let on_d_click = on_action.clone();
                    let avatar = render_dialog_avatar(
                        dialog,
                        meta,
                        palette,
                        34.0,
                        palette.dialogs_bg,
                        online_border,
                    )
                    .when(dialog.unread_count > 0, |el| {
                        el.child(
                            div()
                                .absolute()
                                .top(px(-3.0))
                                .right(px(-4.0))
                                .min_w(px(16.0))
                                .h(px(16.0))
                                .px(px(4.0))
                                .rounded_full()
                                .bg(unread_bg)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(9.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(palette.unread_badge_fg))
                                .child(format_compact_badge(dialog.unread_count)),
                        )
                    })
                    .when(dialog.unread_count == 0 && meta.is_unread_marked, |el| {
                        el.child(
                            div()
                                .absolute()
                                .top(px(-1.0))
                                .right(px(-1.0))
                                .w(px(9.0))
                                .h(px(9.0))
                                .rounded_full()
                                .bg(unread_bg)
                                .border_1()
                                .border_color(online_border),
                        )
                    });

                    div()
                        .id(("rail-dialog-btn", dialog_id as usize))
                        .relative()
                        .w(px(42.0))
                        .h(px(42.0))
                        .rounded_full()
                        .flex_shrink_0()
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(is_active, |el| el.border_2().border_color(active_ring))
                        .when(!is_active && is_hovered, |el| el.bg(hover_bg))
                        .on_hover(cx.listener(move |this, hovered, _w, cx| {
                            let next = if *hovered { Some(dialog_id) } else { None };
                            on_d_hover(this, UnifiedSidebarAction::SetHoverDialog(next), cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                on_d_click(this, UnifiedSidebarAction::SelectDialog(dialog_id), cx);
                            }),
                        )
                        .child(avatar)
                        .when(is_hovered, |el| {
                            el.child(render_floating_pill_tooltip(&dialog.peer_name, palette))
                        })
                })),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .flex_shrink_0()
                .pt(px(6.0))
                .gap(px(6.0))
                .child(render_collapsed_icon_btn(
                    "rail-invite-btn",
                    "invite",
                    IconName::GroupUsers,
                    tr_str(lang_id, "添加好友", "Add"),
                    false,
                    0,
                    hovered_item,
                    palette,
                    cx,
                    on_action,
                    UnifiedSidebarAction::OpenAddFriend,
                ))
                .child(
                    div()
                        .id("rail-account-btn")
                        .relative()
                        .w(px(34.0))
                        .h(px(34.0))
                        .rounded_full()
                        .bg(rgb(avatar_grad.0))
                        .hover(|s| s.bg(rgb(palette.accent_hover)))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .on_hover(cx.listener(move |this, hovered, _w, cx| {
                            let next = if *hovered { Some("account") } else { None };
                            on_account_hover(this, UnifiedSidebarAction::SetHover(next), cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                on_account_click(this, UnifiedSidebarAction::OpenMainMenu, cx);
                            }),
                        )
                        .child(nav.account_initials.clone())
                        .child(
                            div()
                                .absolute()
                                .bottom_0()
                                .right_0()
                                .w(px(9.0))
                                .h(px(9.0))
                                .rounded_full()
                                .bg(if nav.is_connected {
                                    rgb(palette.online_dot_fg)
                                } else {
                                    rgb(palette.muted_badge_bg)
                                })
                                .border_1()
                                .border_color(rgb(palette.dialogs_bg)),
                        )
                        .when(account_hovered, |el| {
                            el.child(render_floating_pill_tooltip(
                                tr_str(lang_id, "账号与菜单", "Account & menu"),
                                palette,
                            ))
                        }),
                ),
        )
        .into_any_element()
}
