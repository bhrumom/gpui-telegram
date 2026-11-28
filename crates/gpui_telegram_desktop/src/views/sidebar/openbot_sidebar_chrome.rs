//! OpenBot `383a35d2` sidebar chrome: crisp vector icons, `38px` `SidebarTopbar`,
//! pill `SidebarSearch`, inset `SidebarFilterMenu`, and circular `#f0f0f0` `+` menu (`UI-F05`).

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::i18n::tr_str;
use crate::theme::{tg_icon_rgba, IconName, TdesktopThemePalette};
use crate::views::chat_sidebar::{
    SidebarChatSortOrder, SidebarListFilterState, SidebarListShowFilter,
    OPENBOT_SEARCH_HEIGHT, OPENBOT_TOPBAR_HEIGHT,
};
use crate::views::sidebar::gemini_unified_sidebar::UnifiedSidebarAction;

/// Crisp stroke-vector icons needed by OpenBot sidebar chrome (keeps `IconName::ALL.len() == 64`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenBotChromeIcon {
    ArrowLeft,
    ArrowRight,
    PanelLeft,
    Filter,
    SlidersHorizontal,
    Plus,
    Check,
}

fn stroke_polyline(
    window: &mut Window,
    origin: Point<Pixels>,
    scale: f32,
    stroke_px: f32,
    pts: &[(f32, f32)],
    color: Rgba,
) {
    if pts.len() < 2 {
        return;
    }
    let ox = f32::from(origin.x);
    let oy = f32::from(origin.y);
    let mut b = PathBuilder::stroke(px(stroke_px));
    for (idx, (x, y)) in pts.iter().enumerate() {
        let p = point(px(ox + x * scale), px(oy + y * scale));
        if idx == 0 {
            b.move_to(p);
        } else {
            b.line_to(p);
        }
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

/// Paints crisp Lucide-proportioned (`24x24` viewBox, `1.75px` stroke) icons via GPUI `canvas`.
pub fn openbot_vector_icon(kind: OpenBotChromeIcon, size_px: f32, color: Rgba) -> AnyElement {
    let stroke_px = (size_px / 16.0 * 1.65).clamp(1.25, 2.0);
    div()
        .w(px(size_px))
        .h(px(size_px))
        .flex_shrink_0()
        .child(
            canvas(
                |bounds, _, _| bounds,
                move |bounds, _, window, _| {
                    let side = f32::from(bounds.size.width)
                        .min(f32::from(bounds.size.height))
                        .max(1.0);
                    let scale = side / 24.0;
                    let origin = bounds.origin;
                    match kind {
                        OpenBotChromeIcon::ArrowLeft => {
                            stroke_polyline(window, origin, scale, stroke_px, &[(19.0, 12.0), (5.0, 12.0)], color);
                            stroke_polyline(
                                window,
                                origin,
                                scale,
                                stroke_px,
                                &[(12.0, 19.0), (5.0, 12.0), (12.0, 5.0)],
                                color,
                            );
                        }
                        OpenBotChromeIcon::ArrowRight => {
                            stroke_polyline(window, origin, scale, stroke_px, &[(5.0, 12.0), (19.0, 12.0)], color);
                            stroke_polyline(
                                window,
                                origin,
                                scale,
                                stroke_px,
                                &[(12.0, 5.0), (19.0, 12.0), (12.0, 19.0)],
                                color,
                            );
                        }
                        OpenBotChromeIcon::PanelLeft => {
                            // Outer rounded box approximated cleanly with crisp bevel corners
                            stroke_polyline(
                                window,
                                origin,
                                scale,
                                stroke_px,
                                &[
                                    (5.0, 3.5),
                                    (19.0, 3.5),
                                    (20.5, 5.0),
                                    (20.5, 19.0),
                                    (19.0, 20.5),
                                    (5.0, 20.5),
                                    (3.5, 19.0),
                                    (3.5, 5.0),
                                    (5.0, 3.5),
                                ],
                                color,
                            );
                            stroke_polyline(window, origin, scale, stroke_px, &[(9.5, 3.5), (9.5, 20.5)], color);
                        }
                        OpenBotChromeIcon::Filter => {
                            stroke_polyline(
                                window,
                                origin,
                                scale,
                                stroke_px,
                                &[
                                    (3.0, 4.5),
                                    (21.0, 4.5),
                                    (14.0, 12.5),
                                    (14.0, 19.5),
                                    (10.0, 17.5),
                                    (10.0, 12.5),
                                    (3.0, 4.5),
                                ],
                                color,
                            );
                        }
                        OpenBotChromeIcon::SlidersHorizontal => {
                            stroke_polyline(window, origin, scale, stroke_px, &[(21.0, 5.0), (14.0, 5.0)], color);
                            stroke_polyline(window, origin, scale, stroke_px, &[(10.0, 5.0), (3.0, 5.0)], color);
                            stroke_polyline(window, origin, scale, stroke_px, &[(14.0, 2.5), (14.0, 7.5)], color);
                            stroke_polyline(window, origin, scale, stroke_px, &[(21.0, 12.0), (12.0, 12.0)], color);
                            stroke_polyline(window, origin, scale, stroke_px, &[(8.0, 12.0), (3.0, 12.0)], color);
                            stroke_polyline(window, origin, scale, stroke_px, &[(8.0, 9.5), (8.0, 14.5)], color);
                            stroke_polyline(window, origin, scale, stroke_px, &[(21.0, 19.0), (16.0, 19.0)], color);
                            stroke_polyline(window, origin, scale, stroke_px, &[(12.0, 19.0), (3.0, 19.0)], color);
                            stroke_polyline(window, origin, scale, stroke_px, &[(16.0, 16.5), (16.0, 21.5)], color);
                        }
                        OpenBotChromeIcon::Plus => {
                            stroke_polyline(window, origin, scale, stroke_px, &[(12.0, 5.0), (12.0, 19.0)], color);
                            stroke_polyline(window, origin, scale, stroke_px, &[(5.0, 12.0), (19.0, 12.0)], color);
                        }
                        OpenBotChromeIcon::Check => {
                            stroke_polyline(
                                window,
                                origin,
                                scale,
                                stroke_px,
                                &[(4.5, 12.5), (9.5, 17.5), (19.5, 6.5)],
                                color,
                            );
                        }
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// Single-source-of-truth layout snapshot for `SidebarTopbar` (`38px`, `←`/`→` opacities, mac clearance).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenBotSidebarTopbarLayout {
    pub height_px: f32,
    pub toggle_visible: bool,
    pub back_enabled: bool,
    pub forward_enabled: bool,
    pub back_opacity: f32,
    pub forward_opacity: f32,
    pub mac_clearance_px: f32,
}

pub fn compute_openbot_sidebar_topbar_layout(
    can_go_back: bool,
    can_go_forward: bool,
    is_agents_page: bool,
    has_page_rail: bool,
) -> OpenBotSidebarTopbarLayout {
    OpenBotSidebarTopbarLayout {
        height_px: OPENBOT_TOPBAR_HEIGHT,
        toggle_visible: is_agents_page,
        back_enabled: can_go_back,
        forward_enabled: can_go_forward,
        back_opacity: if can_go_back { 1.0 } else { 0.4 },
        forward_opacity: if can_go_forward { 1.0 } else { 0.4 },
        mac_clearance_px: if has_page_rail { 24.0 } else { 78.0 },
    }
}

/// Renders OpenBot's exact `38px` `SidebarTopbar` (`← →` history nav + right-aligned `PanelLeft`).
/// Works both inside `#app-shell-top-chrome-strip` (`external_topbar_hosted == true`) and standalone.
#[allow(clippy::too_many_arguments)]
pub fn render_openbot_sidebar_topbar<V: 'static>(
    can_go_back: bool,
    can_go_forward: bool,
    is_compact: bool,
    is_closed: bool,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_back: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_forward: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_toggle_panel: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let layout = compute_openbot_sidebar_topbar_layout(can_go_back, can_go_forward, true, true);
    let ob = palette.openbot_palette();
    let hover_bg = ob.hover();
    let icon_muted = ob.text_muted();
    let icon_primary = ob.text_primary();

    let panel_btn = div()
        .id("openbot-sidebar-panel-toggle-btn")
        .w(px(28.0))
        .h(px(28.0))
        .flex_shrink_0()
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
                on_toggle_panel(this, cx);
            }),
        )
        .child(openbot_vector_icon(
            OpenBotChromeIcon::PanelLeft,
            16.0,
            if is_closed { icon_primary } else { icon_muted },
        ));

    if is_closed || is_compact {
        return div()
            .id("openbot-sidebar-topbar-compact")
            .h(px(layout.height_px))
            .w_full()
            .flex_shrink_0()
            .px(px(if is_closed { 8.0 } else { 12.0 }))
            .pt(px(2.0))
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .items_center()
            .justify_center()
            .when(layout.toggle_visible, |el| el.child(panel_btn))
            .into_any_element();
    }

    let back_btn = div()
        .id("openbot-sidebar-nav-back-btn")
        .w(px(28.0))
        .h(px(28.0))
        .flex_shrink_0()
        .rounded(px(6.0))
        .when(layout.back_enabled, |el| {
            el.cursor_pointer().hover(move |s| s.bg(hover_bg))
        })
        .opacity(layout.back_opacity)
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if can_go_back {
                    on_back(this, cx);
                }
            }),
        )
        .child(openbot_vector_icon(OpenBotChromeIcon::ArrowLeft, 15.0, icon_muted));

    let forward_btn = div()
        .id("openbot-sidebar-nav-forward-btn")
        .w(px(28.0))
        .h(px(28.0))
        .flex_shrink_0()
        .rounded(px(6.0))
        .when(layout.forward_enabled, |el| {
            el.cursor_pointer().hover(move |s| s.bg(hover_bg))
        })
        .opacity(layout.forward_opacity)
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if can_go_forward {
                    on_forward(this, cx);
                }
            }),
        )
        .child(openbot_vector_icon(OpenBotChromeIcon::ArrowRight, 15.0, icon_muted));

    let pl_px = if cfg!(target_os = "macos") {
        layout.mac_clearance_px
    } else {
        10.0
    };

    div()
        .id("openbot-sidebar-topbar")
        .h(px(layout.height_px))
        .w_full()
        .flex_shrink_0()
        .pt(px(2.0))
        .pl(px(pl_px))
        .pr(px(3.0))
        .window_control_area(WindowControlArea::Drag)
        .flex()
        .items_center()
        .gap(px(3.0))
        .child(back_btn)
        .child(forward_btn)
        .child(div().flex_1().h_full())
        .when(layout.toggle_visible, |el| el.child(panel_btn))
        .into_any_element()
}

/// Single-source-of-truth layout snapshot for `SidebarSearch` pill, inset funnel, and circular `+` button.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenBotSidebarSearchRowLayout {
    pub height_px: f32,
    pub radius_px: f32,
    pub bg_hex: String,
    pub funnel_size_px: f32,
    pub filter_active: bool,
    pub compact_search_visible: bool,
    pub compact_search_size_px: f32,
    pub filter_empty_visible: bool,
    pub clear_filter_visible: bool,
    pub add_button_size_px: f32,
    pub add_button_bg_hex: String,
    pub add_button_fg_hex: String,
    pub add_menu_open: bool,
    pub add_menu_items: [&'static str; 3],
}

pub fn compute_openbot_sidebar_search_row_layout(
    is_compact: bool,
    filter_active: bool,
    filtered_count: usize,
    is_new_menu_open: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
) -> OpenBotSidebarSearchRowLayout {
    let ob = palette.openbot_palette();
    let bg_hex = ob
        .token("--openbot-bg-control")
        .unwrap_or("#212121")
        .to_string();
    let add_button_bg_hex = ob
        .token("--openbot-surface-light")
        .unwrap_or("#f0f0f0")
        .to_string();
    let add_button_fg_hex = ob
        .token("--openbot-text-on-light")
        .unwrap_or("#141414")
        .to_string();
    OpenBotSidebarSearchRowLayout {
        height_px: OPENBOT_SEARCH_HEIGHT,
        radius_px: 9999.0,
        bg_hex,
        funnel_size_px: 28.0,
        filter_active,
        compact_search_visible: is_compact,
        compact_search_size_px: OPENBOT_SEARCH_HEIGHT,
        filter_empty_visible: filter_active && filtered_count == 0,
        clear_filter_visible: filter_active,
        add_button_size_px: 32.0,
        add_button_bg_hex,
        add_button_fg_hex,
        add_menu_open: is_new_menu_open,
        add_menu_items: [
            tr_str(lang_id, "新建助手", "New Agent"),
            tr_str(lang_id, "新建群聊", "New Group"),
            tr_str(lang_id, "新建分组", "New Section"),
        ],
    }
}

/// Renders OpenBot's `SidebarSearch` pill trigger (`Q 搜索` -> `GlobalSearch`), inset `28x28`
/// filter funnel (`SidebarFilterMenu`), and circular `32x32` `#f0f0f0` `+` button.
#[allow(clippy::too_many_arguments)]
pub fn render_openbot_sidebar_search_row<V: 'static>(
    external_topbar_hosted: bool,
    has_active_filter: bool,
    is_filter_open: bool,
    is_new_menu_open: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, UnifiedSidebarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let layout = compute_openbot_sidebar_search_row_layout(
        false,
        has_active_filter,
        1,
        is_new_menu_open,
        palette,
        lang_id,
    );
    let ob = palette.openbot_palette();
    let pill_bg = ob.bg_control();
    let pill_hover = ob.bg_control_hover();
    let filter_active_bg = ob.bg_control_active();
    let filter_hover_bg = ob.hover();
    let muted = ob.text_muted();
    let primary = ob.text_primary();
    let plus_bg = ob.surface_light();
    let plus_hover = ob.text_secondary();
    let plus_fg = ob.text_on_light();

    let on_search = on_action.clone();
    let on_filter = on_action.clone();
    let on_plus = on_action;

    div()
        .id("openbot-sidebar-search-wrap")
        .relative()
        .w_full()
        .flex_shrink_0()
        .pt(px(if external_topbar_hosted { 11.0 } else { 8.0 }))
        .px(px(8.0))
        .pb(px(10.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            div()
                .id("sidebar-search-box")
                .relative()
                .flex_1()
                .min_w(px(0.0))
                .h(px(layout.height_px))
                .rounded(px(layout.radius_px))
                .bg(pill_bg)
                .hover(move |s| s.bg(pill_hover))
                .cursor_pointer()
                .pl(px(12.0))
                .pr(px(34.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        on_search(this, UnifiedSidebarAction::OpenGlobalSearch, cx);
                    }),
                )
                .child(tg_icon_rgba(IconName::Search, 14.5, muted))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(13.5))
                        .text_color(muted)
                        .child(tr_str(lang_id, "搜索", "Search")),
                )
                .child(
                    div()
                        .id("openbot-sidebar-filter-btn")
                        .absolute()
                        .right(px(2.0))
                        .top(px(2.0))
                        .w(px(layout.funnel_size_px))
                        .h(px(layout.funnel_size_px))
                        .rounded_full()
                        .cursor_pointer()
                        .when(is_filter_open || layout.filter_active, |el| el.bg(filter_active_bg))
                        .hover(move |s| s.bg(filter_hover_bg))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                on_filter(this, UnifiedSidebarAction::ToggleFilterMenu, cx);
                            }),
                        )
                        .child(openbot_vector_icon(
                            OpenBotChromeIcon::Filter,
                            13.5,
                            if is_filter_open || layout.filter_active { primary } else { muted },
                        ))
                        .when(layout.filter_active, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .top(px(3.0))
                                    .right(px(3.0))
                                    .w(px(6.0))
                                    .h(px(6.0))
                                    .rounded_full()
                                    .bg(primary),
                            )
                        }),
                ),
        )
        .child(
            div()
                .id("openbot-sidebar-plus-btn")
                .w(px(layout.add_button_size_px))
                .h(px(layout.add_button_size_px))
                .flex_shrink_0()
                .rounded_full()
                .cursor_pointer()
                .bg(if layout.add_menu_open { plus_hover } else { plus_bg })
                .hover(move |s| s.bg(plus_hover))
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        on_plus(this, UnifiedSidebarAction::ToggleNewMenu, cx);
                    }),
                )
                .child(openbot_vector_icon(OpenBotChromeIcon::Plus, 18.0, plus_fg)),
        )
        .into_any_element()
}

/// Renders OpenBot's circular `+` floating dropdown (`新建助手`, `新建群聊`, `新建分组`).
pub fn render_openbot_new_menu_popover<V: 'static>(
    external_topbar_hosted: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, UnifiedSidebarAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let layout = compute_openbot_sidebar_search_row_layout(false, false, 1, true, palette, lang_id);
    let ob = palette.openbot_palette();
    let surface = ob.bg_surface();
    let border = ob.border_strong_hairline();
    let hover = ob.bg_control_hover();
    let text_primary = ob.text_primary();
    let icon_muted = ob.text_muted();
    let top_px = if external_topbar_hosted { 48.0 } else { 84.0 };

    let entries: [(&'static str, IconName, &'static str, UnifiedSidebarAction); 3] = [
        (
            "openbot-new-menu-agent",
            IconName::BotRobot,
            layout.add_menu_items[0],
            UnifiedSidebarAction::CreateNewAgent,
        ),
        (
            "openbot-new-menu-group",
            IconName::GroupUsers,
            layout.add_menu_items[1],
            UnifiedSidebarAction::CreateNewGroup,
        ),
        (
            "openbot-new-menu-folder",
            IconName::FolderWork,
            layout.add_menu_items[2],
            UnifiedSidebarAction::CreateNewFolder,
        ),
    ];

    let dismiss_cb = on_action.clone();
    let rows: Vec<AnyElement> = entries
        .into_iter()
        .map(|(id, icon, label, act)| {
            let cb = on_action.clone();
            div()
                .id(id)
                .cursor_pointer()
                .h(px(32.0))
                .px(px(10.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(hover))
                .flex()
                .items_center()
                .gap(px(9.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        cb(this, act, cx);
                    }),
                )
                .child(tg_icon_rgba(icon, 15.0, icon_muted))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text_primary)
                        .child(label.to_string()),
                )
                .into_any_element()
        })
        .collect();

    div()
        .id("openbot-new-menu-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                dismiss_cb(this, UnifiedSidebarAction::CloseOpenBotPopovers, cx);
            }),
        )
        .child(
            deferred(
                div()
                    .id("openbot-new-menu-popover")
                    .absolute()
                    .right(px(8.0))
                    .top(px(top_px))
                    .w(px(184.0))
                    .p(px(5.0))
                    .rounded(px(12.0))
                    .bg(surface)
                    .border_1()
                    .border_color(border)
                    .shadow(palette.popover_shadows())
                    .occlude()
                    .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .children(rows),
            )
            .with_priority(50),
        )
        .into_any_element()
}

/// Counts displayed inside OpenBot's `SidebarFilterMenu` popover.
#[derive(Debug, Clone, Copy, Default)]
pub struct SidebarFilterCounts {
    pub all: usize,
    pub agents: usize,
    pub channels: usize,
    pub unread: usize,
    pub working: usize,
    pub muted: usize,
}

/// Renders OpenBot's `SidebarFilterMenu` floating dropdown (show radio, status toggles, sort, clear).
#[allow(clippy::too_many_arguments)]
pub fn render_openbot_filter_menu_popover<V: 'static>(
    filter: &SidebarListFilterState,
    counts: SidebarFilterCounts,
    external_topbar_hosted: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_set_show: impl Fn(&mut V, SidebarListShowFilter, &mut Context<V>) + Clone + 'static,
    on_toggle_flag: impl Fn(&mut V, &'static str, &mut Context<V>) + Clone + 'static,
    on_set_sort: impl Fn(&mut V, SidebarChatSortOrder, &mut Context<V>) + Clone + 'static,
    on_clear: impl Fn(&mut V, &mut Context<V>) + Clone + 'static,
    on_dismiss: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let ob = palette.openbot_palette();
    let surface = ob.bg_surface();
    let border = ob.border_strong_hairline();
    let sep = ob.border_hairline();
    let hover = ob.bg_control_hover();
    let text_pri = ob.text_primary();
    let text_muted = ob.text_muted();
    let top_px = if external_topbar_hosted { 48.0 } else { 84.0 };

    let mut rows: Vec<AnyElement> = Vec::new();
    let show_opts = [
        (SidebarListShowFilter::All, tr_str(lang_id, "全部对话", "All chats"), counts.all),
        (SidebarListShowFilter::Agents, tr_str(lang_id, "助手", "Agents"), counts.agents),
        (SidebarListShowFilter::Channels, tr_str(lang_id, "群聊", "Groups"), counts.channels),
    ];
    for (idx, (mode, label, count)) in show_opts.into_iter().enumerate() {
        let active = filter.show == mode;
        let cb = on_set_show.clone();
        rows.push(
            div()
                .id(("openbot-filter-show", idx as u64))
                .cursor_pointer()
                .h(px(30.0))
                .px(px(8.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(hover))
                .flex()
                .items_center()
                .justify_between()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        cb(this, mode, cx);
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(div().w(px(14.0)).flex().items_center().justify_center().when(active, |el| {
                            el.child(openbot_vector_icon(OpenBotChromeIcon::Check, 13.0, text_pri))
                        }))
                        .child(div().text_size(px(12.5)).text_color(text_pri).child(label)),
                )
                .child(div().text_size(px(11.5)).text_color(text_muted).child(count.to_string()))
                .into_any_element(),
        );
    }

    rows.push(div().h(px(1.0)).mx(px(6.0)).my(px(4.0)).bg(sep).into_any_element());

    let toggles = [
        ("unread", tr_str(lang_id, "未读", "Unread"), filter.unread_only, counts.unread),
        ("working", tr_str(lang_id, "处理中", "Working"), filter.working_only, counts.working),
        ("muted", tr_str(lang_id, "已静音", "Muted"), filter.muted_only, counts.muted),
    ];
    for (idx, (key, label, active, count)) in toggles.into_iter().enumerate() {
        let cb = on_toggle_flag.clone();
        rows.push(
            div()
                .id(("openbot-filter-toggle", idx as u64))
                .cursor_pointer()
                .h(px(30.0))
                .px(px(8.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(hover))
                .flex()
                .items_center()
                .justify_between()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        cb(this, key, cx);
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(div().w(px(14.0)).flex().items_center().justify_center().when(active, |el| {
                            el.child(openbot_vector_icon(OpenBotChromeIcon::Check, 13.0, text_pri))
                        }))
                        .child(div().text_size(px(12.5)).text_color(text_pri).child(label)),
                )
                .child(div().text_size(px(11.5)).text_color(text_muted).child(count.to_string()))
                .into_any_element(),
        );
    }

    rows.push(div().h(px(1.0)).mx(px(6.0)).my(px(4.0)).bg(sep).into_any_element());
    rows.push(
        div()
            .px(px(8.0))
            .py(px(2.0))
            .text_size(px(11.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(text_muted)
            .child(tr_str(lang_id, "排序方式", "Sort by"))
            .into_any_element(),
    );

    let sorts = [
        (SidebarChatSortOrder::Manual, tr_str(lang_id, "默认", "Default")),
        (SidebarChatSortOrder::Recent, tr_str(lang_id, "最近活动", "Recent")),
        (SidebarChatSortOrder::UnreadFirst, tr_str(lang_id, "未读优先", "Unread first")),
        (SidebarChatSortOrder::UnreadGroup, tr_str(lang_id, "未读分组", "Unread group")),
    ];
    for (idx, (order, label)) in sorts.into_iter().enumerate() {
        let active = filter.sort_order == order;
        let cb = on_set_sort.clone();
        rows.push(
            div()
                .id(("openbot-filter-sort", idx as u64))
                .cursor_pointer()
                .h(px(28.0))
                .px(px(8.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(hover))
                .flex()
                .items_center()
                .gap(px(8.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        cb(this, order, cx);
                    }),
                )
                .child(div().w(px(14.0)).flex().items_center().justify_center().when(active, |el| {
                    el.child(openbot_vector_icon(OpenBotChromeIcon::Check, 13.0, text_pri))
                }))
                .child(div().text_size(px(12.5)).text_color(text_pri).child(label))
                .into_any_element(),
        );
    }

    if filter.is_active() {
        rows.push(div().h(px(1.0)).mx(px(6.0)).my(px(4.0)).bg(sep).into_any_element());
        rows.push(
            div()
                .id("openbot-filter-clear")
                .cursor_pointer()
                .h(px(28.0))
                .px(px(8.0))
                .rounded(px(7.0))
                .hover(move |s| s.bg(hover))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(text_muted)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        on_clear(this, cx);
                    }),
                )
                .child(tr_str(lang_id, "清除筛选", "Clear filters"))
                .into_any_element(),
        );
    }

    div()
        .id("openbot-filter-menu-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                on_dismiss(this, cx);
            }),
        )
        .child(
            deferred(
                div()
                    .id("openbot-filter-menu-popover")
                    .absolute()
                    .right(px(44.0))
                    .top(px(top_px))
                    .w(px(216.0))
                    .p(px(5.0))
                    .rounded(px(12.0))
                    .bg(surface)
                    .border_1()
                    .border_color(border)
                    .shadow(palette.popover_shadows())
                    .occlude()
                    .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.stop_propagation()))
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .children(rows),
            )
            .with_priority(48),
        )
        .into_any_element()
}
