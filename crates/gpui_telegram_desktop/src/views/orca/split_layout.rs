//! Recursive GPUI `0.2.2` Split Anything workspace layout renderer (`F22`).
//!
//! Renders `SplitPaneNode` trees across ALL 10 `TabContentType::ALL` variants using a
//! compile-time exhaustive `match` (zero `_` wildcard), with interactive H/V splits,
//! pinned split boundaries (`set_split_ratio` + `rebalance_splits`), 30-day close
//! tombstones + `Cmd+Shift+T` reopen, `Cmd+Shift+J` Chat<->Terminal flip/fork/resume
//! with `active_cli_owner` protection, and `HibernationController::build_rehydration_plan`
//! wake controls.

use gpui::prelude::FluentBuilder;
use gpui::*;
use orca_core::workspace_layout::{
    SplitDirection, SplitPaneNode, TabContentType, TabLeafState, TabViewMode, WorkspaceTab,
};

use crate::i18n::tr_str;
use crate::theme::{IconName, TdesktopThemePalette, tg_icon};

use super::ide_state::{IdeInputTarget, OrcaIdeAction, OrcaIdeDrawer};
use super::ide_workspace::OrcaIdeWorkspace;

/// Compile-time exhaustive icon mapping across all 10 `TabContentType::ALL` variants.
pub fn tab_content_icon(content_type: TabContentType) -> IconName {
    match content_type {
        TabContentType::Terminal => IconName::TerminalMcp,
        TabContentType::Editor => IconName::FileDoc,
        TabContentType::Diff => IconName::Edit,
        TabContentType::ConflictReview => IconName::ShieldMod,
        TabContentType::CheckDetails => IconName::SelectCheck,
        TabContentType::GitGraph => IconName::Forward,
        TabContentType::AgentSession => IconName::BotRobot,
        TabContentType::Browser => IconName::ScreenShare,
        TabContentType::Simulator => IconName::ComputerDesktop,
        TabContentType::Room => IconName::GroupUsers,
    }
}

/// Compile-time exhaustive accent badge color across all 10 `TabContentType::ALL` variants.
pub const fn tab_content_badge_color(content_type: TabContentType) -> u32 {
    match content_type {
        TabContentType::Terminal => 0x0EA5E9,
        TabContentType::Editor => 0x6366F1,
        TabContentType::Diff => 0xF59E0B,
        TabContentType::ConflictReview => 0xEF4444,
        TabContentType::CheckDetails => 0x10B981,
        TabContentType::GitGraph => 0x8B5CF6,
        TabContentType::AgentSession => 0x14B8A6,
        TabContentType::Browser => 0x3B82F6,
        TabContentType::Simulator => 0xEC4899,
        TabContentType::Room => 0xF97316,
    }
}

/// Top-level entry rendering the workspace's `SplitPaneNode` tree plus tombstone reopen strip.
pub fn render_split_layout(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let tombstone_count = ws.state.layout.closed_tombstones.len();
    let pane_count = ws.state.layout.pane_count();
    let tab_count = ws.state.layout.tab_count();
    let root = ws.state.layout.root.clone();

    div()
        .id("orca-ide-split-layout-root")
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .p(px(8.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .bg(rgb(pal.top_bar_bg))
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "ws:{} · {} pane(s) · {} tab(s) · rev {}",
                            ws.state.layout.workspace_id,
                            pane_count,
                            tab_count,
                            ws.state.layout.revision
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("orca-ide-rebalance-splits-btn")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(5.0))
                                .bg(rgb(pal.input_bg))
                                .text_xs()
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let rt = this.runtime.clone();
                                        this.state.rebalance_splits_and_sync(&rt);
                                        this.state.status_banner = Some(
                                            "Rebalanced unpinned split boundaries".to_string(),
                                        );
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "均衡未锁定分屏", "Rebalance Splits")),
                        )
                        .when(tombstone_count > 0, |row| {
                            row.child(
                                div()
                                    .id("orca-ide-reopen-closed-tab-btn")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.0))
                                    .rounded(px(5.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev, _w, cx| {
                                            cx.stop_propagation();
                                            let now_ms = chrono::Utc::now().timestamp_millis();
                                            match this.state.reopen_last_closed_tab(now_ms) {
                                                Ok(tab) => {
                                                    let rt = this.runtime.clone();
                                                    this.state.persist_layout_to_runtime(&rt);
                                                    this.state.status_banner = Some(format!(
                                                        "Reopened tombstoned tab '{}'",
                                                        tab.title
                                                    ));
                                                }
                                                Err(e) => {
                                                    this.state.error_banner = Some(e.to_string());
                                                }
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child(format!(
                                        "{} ({tombstone_count}) · Cmd+Shift+T",
                                        tr_str(lang, "恢复关闭标签", "Reopen Closed")
                                    )),
                            )
                        }),
                ),
        )
        .when_some(ws.state.active_cli_owner_banner.clone(), |el, msg| {
            el.child(
                div()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.16))
                    .border_1()
                    .border_color(rgb(pal.danger))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.danger))
                    .child(format!("active_cli_owner guard: {msg}")),
            )
        })
        .child(render_split_node(ws, &root, Vec::new(), cx))
        .into_any_element()
}

fn render_split_node(
    ws: &OrcaIdeWorkspace,
    node: &SplitPaneNode,
    path: Vec<bool>,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    match node {
        SplitPaneNode::Leaf {
            pane_id,
            tabs,
            active_tab_id,
        } => render_leaf_pane(ws, pane_id, tabs, active_tab_id.as_deref(), cx),
        SplitPaneNode::Split {
            direction,
            ratio_bp,
            pinned_boundary,
            first,
            second,
        } => {
            let mut left_path = path.clone();
            left_path.push(false);
            let mut right_path = path.clone();
            right_path.push(true);

            let first_el = render_split_node(ws, first, left_path, cx);
            let second_el = render_split_node(ws, second, right_path, cx);

            let pct = (*ratio_bp as f32) / 100.0;
            let first_frac = (*ratio_bp as f32) / 10_000.0;
            let second_frac = ((10_000u16.saturating_sub(*ratio_bp)) as f32) / 10_000.0;
            let path_key: String = path
                .iter()
                .map(|b| if *b { 'R' } else { 'L' })
                .collect();

            let pin_path = path.clone();
            let current_ratio = *ratio_bp;
            let next_pinned = !*pinned_boundary;

            let dec_path = path.clone();
            let inc_path = path.clone();
            let pin_state = *pinned_boundary;

            let divider = div()
                .id(SharedString::from(format!("orca-split-div-{path_key}")))
                .flex()
                .when(*direction == SplitDirection::Horizontal, |d| {
                    d.flex_col().w(px(26.0)).items_center().justify_center()
                })
                .when(*direction == SplitDirection::Vertical, |d| {
                    d.flex_row().h(px(24.0)).items_center().justify_center()
                })
                .gap(px(4.0))
                .bg(rgb(pal.top_bar_bg))
                .border_1()
                .border_color(if *pinned_boundary {
                    rgb(pal.accent)
                } else {
                    pal.hairline_border_rgba()
                })
                .rounded(px(4.0))
                .child(
                    div()
                        .id(SharedString::from(format!("orca-split-pin-{path_key}")))
                        .cursor_pointer()
                        .px(px(3.0))
                        .py(px(1.0))
                        .rounded(px(3.0))
                        .bg(if *pinned_boundary {
                            rgb(pal.accent)
                        } else {
                            rgb(pal.input_bg)
                        })
                        .text_xs()
                        .text_color(if *pinned_boundary {
                            rgb(pal.active_button_fg)
                        } else {
                            rgb(pal.text_secondary)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                if let Err(e) = this.state.toggle_pin_boundary(
                                    &pin_path,
                                    current_ratio,
                                    next_pinned,
                                ) {
                                    this.state.error_banner = Some(e.to_string());
                                } else {
                                    let rt = this.runtime.clone();
                                    this.state.persist_layout_to_runtime(&rt);
                                }
                                cx.notify();
                            }),
                        )
                        .child(if *pinned_boundary { "PIN" } else { "pin" }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!("{pct:.0}%")),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("orca-split-dec-{path_key}")))
                        .cursor_pointer()
                        .px(px(3.0))
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let next_ratio = current_ratio.saturating_sub(500).max(1_000);
                                let _ = this.state.toggle_pin_boundary(
                                    &dec_path,
                                    next_ratio,
                                    pin_state,
                                );
                                let rt = this.runtime.clone();
                                this.state.persist_layout_to_runtime(&rt);
                                cx.notify();
                            }),
                        )
                        .child("-"),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("orca-split-inc-{path_key}")))
                        .cursor_pointer()
                        .px(px(3.0))
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let next_ratio = current_ratio.saturating_add(500).min(9_000);
                                let _ = this.state.toggle_pin_boundary(
                                    &inc_path,
                                    next_ratio,
                                    pin_state,
                                );
                                let rt = this.runtime.clone();
                                this.state.persist_layout_to_runtime(&rt);
                                cx.notify();
                            }),
                        )
                        .child("+"),
                );

            div()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .flex()
                .when(*direction == SplitDirection::Horizontal, |d| d.flex_row())
                .when(*direction == SplitDirection::Vertical, |d| d.flex_col())
                .gap(px(4.0))
                .child(
                    div()
                        .min_w(px(0.0))
                        .min_h(px(0.0))
                        .flex()
                        .flex_col()
                        .when(*direction == SplitDirection::Horizontal, |el| {
                            el.w(relative(first_frac)).h_full()
                        })
                        .when(*direction == SplitDirection::Vertical, |el| {
                            el.h(relative(first_frac)).w_full()
                        })
                        .child(first_el),
                )
                .child(divider)
                .child(
                    div()
                        .min_w(px(0.0))
                        .min_h(px(0.0))
                        .flex()
                        .flex_col()
                        .flex_1()
                        .when(*direction == SplitDirection::Horizontal, |el| {
                            el.w(relative(second_frac)).h_full()
                        })
                        .when(*direction == SplitDirection::Vertical, |el| {
                            el.h(relative(second_frac)).w_full()
                        })
                        .child(second_el),
                )
                .into_any_element()
        }
    }
}

fn render_leaf_pane(
    ws: &OrcaIdeWorkspace,
    pane_id: &str,
    tabs: &[WorkspaceTab],
    active_tab_id: Option<&str>,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let is_focused = ws.state.layout.focused_pane_id == pane_id;
    let active_tab = active_tab_id
        .and_then(|id| tabs.iter().find(|t| t.tab_id == id))
        .or_else(|| tabs.first());

    let mut tab_strip = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(4.0))
        .flex_1()
        .min_w(px(0.0));

    for tab in tabs {
        let is_active = active_tab.is_some_and(|a| a.tab_id == tab.tab_id);
        let tab_id_focus = tab.tab_id.clone();
        let tab_id_close = tab.tab_id.clone();
        let accent_col = tab_content_badge_color(tab.content_type);
        let icon = tab_content_icon(tab.content_type);
        let sleep_badge = match &tab.leaf_state {
            TabLeafState::Live => None,
            TabLeafState::Sleeping { .. } => Some("SLEEP"),
            TabLeafState::Resume { .. } => Some("RESUME"),
        };

        tab_strip = tab_strip.child(
            div()
                .id(SharedString::from(format!("orca-tab-{}", tab.tab_id)))
                .cursor_pointer()
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .bg(if is_active {
                    pal.row_active_rgba()
                } else {
                    rgb(pal.input_bg)
                })
                .border_1()
                .border_color(if is_active {
                    rgb(accent_col)
                } else {
                    pal.hairline_border_rgba()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let _ = this.state.layout.focus_tab(&tab_id_focus, now_ms);
                        let rt = this.runtime.clone();
                        this.state.persist_layout_to_runtime(&rt);
                        let _ = this.state.refresh_sessions(&rt);
                        cx.notify();
                    }),
                )
                .child(tg_icon(icon, 12.0, accent_col))
                .child(
                    div()
                        .text_xs()
                        .font_weight(if is_active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(rgb(pal.text_primary))
                        .child(tab.title.clone()),
                )
                .when(tab.content_type.traits().supports_dual_view, |el| {
                    let mode_lbl = match tab.view_mode {
                        TabViewMode::Chat => "CHAT",
                        TabViewMode::Terminal => "TERM",
                    };
                    el.child(
                        div()
                            .px(px(4.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .bg(rgb(accent_col))
                            .text_xs()
                            .text_color(rgb(0xFFFFFF))
                            .child(mode_lbl),
                    )
                })
                .when_some(sleep_badge, |el, lbl| {
                    el.child(
                        div()
                            .px(px(4.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .bg(rgb(0x78350F))
                            .text_xs()
                            .text_color(rgb(0xFEF3C7))
                            .child(lbl),
                    )
                })
                .child(
                    div()
                        .id(SharedString::from(format!("orca-tab-close-{}", tab.tab_id)))
                        .cursor_pointer()
                        .px(px(3.0))
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                match this.state.close_and_tombstone_tab(&tab_id_close, now_ms) {
                                    Ok(tomb) => {
                                        let rt = this.runtime.clone();
                                        this.state.persist_layout_to_runtime(&rt);
                                        this.state.status_banner = Some(format!(
                                            "Closed '{}' (30d tombstone recorded)",
                                            tomb.tab.title
                                        ));
                                    }
                                    Err(e) => {
                                        this.state.error_banner = Some(e.to_string());
                                    }
                                }
                                cx.notify();
                            }),
                        )
                        .child("×"),
                ),
        );
    }

    let pane_h = pane_id.to_string();
    let pane_v = pane_id.to_string();
    let pane_focus = pane_id.to_string();

    let pane_actions = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .child(
            div()
                .id(SharedString::from(format!("orca-split-h-{pane_id}")))
                .cursor_pointer()
                .px(px(6.0))
                .py(px(3.0))
                .rounded(px(4.0))
                .bg(rgb(pal.input_bg))
                .text_xs()
                .text_color(rgb(pal.text_primary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let rt = this.runtime.clone();
                        if let Err(e) = this.state.split_pane_and_sync(
                            &rt,
                            &pane_h,
                            SplitDirection::Horizontal,
                            TabContentType::Terminal,
                            now_ms,
                        ) {
                            this.state.error_banner = Some(e.to_string());
                        }
                        cx.notify();
                    }),
                )
                .child("Split H"),
        )
        .child(
            div()
                .id(SharedString::from(format!("orca-split-v-{pane_id}")))
                .cursor_pointer()
                .px(px(6.0))
                .py(px(3.0))
                .rounded(px(4.0))
                .bg(rgb(pal.input_bg))
                .text_xs()
                .text_color(rgb(pal.text_primary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let rt = this.runtime.clone();
                        if let Err(e) = this.state.split_pane_and_sync(
                            &rt,
                            &pane_v,
                            SplitDirection::Vertical,
                            TabContentType::Diff,
                            now_ms,
                        ) {
                            this.state.error_banner = Some(e.to_string());
                        }
                        cx.notify();
                    }),
                )
                .child("Split V"),
        );

    let body = match active_tab {
        Some(tab) => render_active_tab_content(ws, pane_id, tab, cx),
        None => render_empty_pane_launcher(ws, pane_id, cx),
    };

    div()
        .id(SharedString::from(format!("orca-pane-{pane_id}")))
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .rounded(px(8.0))
        .bg(rgb(pal.card_bg))
        .border_1()
        .border_color(if is_focused {
            rgb(pal.accent)
        } else {
            pal.hairline_border_rgba()
        })
        .flex()
        .flex_col()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev, _w, cx| {
                this.state.layout.focused_pane_id = pane_focus.clone();
                cx.notify();
            }),
        )
        .child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .border_b_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(tab_strip)
                .child(pane_actions),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .p(px(10.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(body)
                .when(tabs.is_empty(), |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(
                                lang,
                                "支持 10 种原生标签类型任意分屏",
                                "Split Anything supports all 10 native TabContentType variants",
                            )),
                    )
                }),
        )
        .into_any_element()
}

fn render_empty_pane_launcher(
    ws: &OrcaIdeWorkspace,
    pane_id: &str,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let mut grid = div().flex().flex_row().flex_wrap().gap(px(6.0));
    for content_type in TabContentType::ALL {
        let pane_target = pane_id.to_string();
        let accent = tab_content_badge_color(content_type);
        let icon = tab_content_icon(content_type);
        grid = grid.child(
            div()
                .id(SharedString::from(format!(
                    "orca-open-tab-{}-{}",
                    pane_id,
                    content_type.as_str()
                )))
                .cursor_pointer()
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        let now_ms = chrono::Utc::now().timestamp_millis();
                        let tab = this.state.make_tab(content_type, now_ms);
                        let rt = this.runtime.clone();
                        if let Err(e) = this
                            .state
                            .open_tab_and_sync(&rt, &pane_target, tab, now_ms)
                        {
                            this.state.error_banner = Some(e.to_string());
                        }
                        cx.notify();
                    }),
                )
                .child(tg_icon(icon, 13.0, accent))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.text_primary))
                        .child(content_type.traits().label),
                ),
        );
    }
    grid.into_any_element()
}

fn render_active_tab_content(
    ws: &OrcaIdeWorkspace,
    pane_id: &str,
    tab: &WorkspaceTab,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();

    // If tab is Sleeping (hibernated), show Wake / Rehydration control first.
    if let TabLeafState::Sleeping {
        hibernated_at_ms,
        reason,
    } = &tab.leaf_state
    {
        let wake_tab_id = tab.tab_id.clone();
        return div()
            .flex_1()
            .flex()
            .flex_col()
            .items_start()
            .justify_center()
            .gap(px(8.0))
            .p(px(12.0))
            .rounded(px(8.0))
            .bg(rgb(pal.input_bg))
            .border_1()
            .border_color(rgb(0xF59E0B))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(pal.text_primary))
                    .child(format!(
                        "{} · {}",
                        tr_str(lang, "标签页已休眠", "Tab Hibernated (Sleeping)"),
                        tab.title
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(format!("hibernated_at={hibernated_at_ms} · {reason}")),
            )
            .child(
                div()
                    .id(SharedString::from(format!("orca-wake-tab-{}", tab.tab_id)))
                    .cursor_pointer()
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .bg(rgb(pal.accent))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.active_button_fg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            let now_ms = chrono::Utc::now().timestamp_millis();
                            let rt = this.runtime.clone();
                            match this.state.wake_hibernated_tab(&rt, &wake_tab_id, now_ms) {
                                Ok(plan) => {
                                    this.state.status_banner = Some(format!(
                                        "Woke tab '{}': {} {}",
                                        plan.tab_id,
                                        plan.binary,
                                        plan.args.join(" ")
                                    ));
                                }
                                Err(e) => {
                                    this.state.error_banner = Some(e);
                                }
                            }
                            cx.notify();
                        }),
                    )
                    .child(tr_str(
                        lang,
                        "唤醒并透明恢复 (--resume)",
                        "Wake & Rehydrate (--resume)",
                    )),
            )
            .into_any_element();
    }

    // Compile-time exhaustive match across ALL 10 TabContentType::ALL variants (zero `_` wildcard!).
    match tab.content_type {
        TabContentType::Terminal | TabContentType::AgentSession => {
            render_dual_view_chat_terminal_body(ws, tab, cx)
        }
        TabContentType::Editor => {
            let wt_summary = ws
                .state
                .worktrees
                .first()
                .map(|w| format!("{} ({})", w.branch_name, w.worktree_path))
                .unwrap_or_else(|| ws.state.active_workspace_id.clone());
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!("Editor · {} ({pane_id})", ws.state.diff_file_path)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "Workspace: {wt_summary} · Local AI-attributed lines: {}",
                            ws.state.file_attributions.len()
                        )),
                )
                .into_any_element()
        }
        TabContentType::Diff => render_diff_tab_summary(ws, cx),
        TabContentType::ConflictReview => div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.text_primary))
                    .child("Conflict Review"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(format!(
                        "Active annotations: {} · Tracked gates: {}",
                        ws.state.diff_annotations.len(),
                        ws.state.tracked_gates.len()
                    )),
            )
            .into_any_element(),
        TabContentType::CheckDetails => div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.text_primary))
                    .child("CI & Check Details"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(format!(
                        "Runs: {} · Ready DAG tasks: {} · Review/Merge gates: {}",
                        ws.state.runs.len(),
                        ws.state.ready_tasks.len(),
                        ws.state.tracked_gates.len()
                    )),
            )
            .into_any_element(),
        TabContentType::GitGraph => {
            let mut list = div().flex().flex_col().gap(px(4.0));
            for wt in ws.state.worktrees.iter().take(6) {
                list = list.child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .child(format!("{} · {} ({})", wt.branch_name, wt.title, wt.repo_root)),
                );
            }
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!("Git Graph · {} worktree(s)", ws.state.worktrees.len())),
                )
                .child(list)
                .into_any_element()
        }
        TabContentType::Browser => div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.text_primary))
                    .child("Embedded Browser & Design Mode"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(format!(
                        "Attributed *.orca.localhost ports: {}",
                        ws.state.worktree_ports.len()
                    )),
            )
            .child(
                div()
                    .id(SharedString::from(format!("orca-tab-browser-dm-{}", tab.tab_id)))
                    .cursor_pointer()
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .bg(rgb(pal.accent))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.active_button_fg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev, _w, cx| {
                            cx.stop_propagation();
                            this.push_action(OrcaIdeAction::OpenBrowserDesignMode);
                            cx.notify();
                        }),
                    )
                    .child("Open Browser Design Mode (Cmd+Shift+I)"),
            )
            .into_any_element(),
        TabContentType::Simulator => div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.text_primary))
                    .child("Mobile Simulator & Emulator Bridge"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child("Normalized coordinate (0..=10000 bp) & AX bridge ready via MobileEmulatorService"),
            )
            .into_any_element(),
        TabContentType::Room => div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.text_primary))
                    .child("Multi-Bot & Agent Room"),
            )
            .child(
                div()
                    .id(SharedString::from(format!("orca-tab-open-bots-{}", tab.tab_id)))
                    .cursor_pointer()
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .bg(rgb(pal.accent))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.active_button_fg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev, _w, cx| {
                            cx.stop_propagation();
                            this.push_action(OrcaIdeAction::OpenBotsWorkspace);
                            cx.notify();
                        }),
                    )
                    .child("Switch to Orca Bots Workspace"),
            )
            .into_any_element(),
    }
}

fn render_diff_tab_summary(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!("Diff Review · {}", ws.state.diff_file_path)),
                )
                .child(
                    div()
                        .id("orca-diff-tab-open-drawer")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                this.state.active_drawer = OrcaIdeDrawer::DiffTasks;
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "打开 Diff 审阅面板", "Open Annotate Diff Panel")),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(format!(
                    "{} annotation(s) · {} attributed line(s) (local-only)",
                    ws.state.diff_annotations.len(),
                    ws.state.file_attributions.len()
                )),
        )
        .into_any_element()
}

fn render_dual_view_chat_terminal_body(
    ws: &OrcaIdeWorkspace,
    tab: &WorkspaceTab,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let tab_id = tab.tab_id.clone();
    let bound_sid = ws.state.tab_session_bindings.get(&tab.tab_id).cloned();
    let snap = ws
        .state
        .active_session_snapshot
        .as_ref()
        .filter(|s| bound_sid.as_deref() == Some(s.session.session_id.as_str()))
        .or(ws.state.active_session_snapshot.as_ref());

    let cli_owner_active = snap.is_some_and(|s| s.session.active_cli_owner);
    let chain_len = snap.map_or(0, |s| s.session.provider_handle_chain.len());
    let mode_label = match tab.view_mode {
        TabViewMode::Chat => "Native Chat View",
        TabViewMode::Terminal => "PTY Terminal View",
    };

    let tab_id_flip = tab_id.clone();
    let sid_flip = bound_sid.clone();
    let tab_id_fork = tab_id.clone();
    let tab_id_resume = tab_id.clone();
    let next_resume_mode = match tab.view_mode {
        TabViewMode::Chat => TabViewMode::Terminal,
        TabViewMode::Terminal => TabViewMode::Chat,
    };
    let tab_id_cli_owner = tab_id.clone();
    let tab_id_send = tab_id.clone();

    let mut items_col = div().flex().flex_col().gap(px(4.0));
    if ws.state.active_session_items.is_empty() {
        items_col = items_col.child(
            div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(tr_str(
                    lang,
                    "暂无会话日志条目，发送提示词或按 Cmd+Shift+J 切换 Chat <-> Terminal。",
                    "No journal items yet. Submit a prompt or press Cmd+Shift+J to flip Chat <-> Terminal.",
                )),
        );
    } else {
        for item in ws.state.active_session_items.iter().rev().take(12).rev() {
            let text_preview = item
                .payload_json
                .get("text")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| item.payload_json.to_string());
            items_col = items_col.child(
                div()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(5.0))
                    .bg(rgb(if tab.view_mode == TabViewMode::Terminal {
                        pal.code_bg
                    } else {
                        pal.input_bg
                    }))
                    .text_xs()
                    .text_color(rgb(if tab.view_mode == TabViewMode::Terminal {
                        pal.code_fg
                    } else {
                        pal.text_primary
                    }))
                    .child(format!("#{} [{}] {}", item.sequence, item.item_kind, text_preview)),
            );
        }
    }

    let draft_active = ws.state.active_input == IdeInputTarget::SessionPromptDraft;
    let draft_text = if ws.state.session_prompt_draft.is_empty() {
        if draft_active {
            "|".to_string()
        } else {
            tr_str(
                lang,
                "输入提示词或终端指令 (点击聚焦)…",
                "Enter agent prompt or terminal input (click to focus)…",
            )
            .to_string()
        }
    } else if draft_active {
        format!("{}|", ws.state.session_prompt_draft)
    } else {
        ws.state.session_prompt_draft.clone()
    };

    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "{mode_label} · handles={chain_len} · cli_owner={cli_owner_active}"
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap(px(5.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-flip-mode-{tab_id}")))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(5.0))
                                .bg(rgb(pal.accent))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.active_button_fg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let now_ms = chrono::Utc::now().timestamp_millis();
                                        let rt = this.runtime.clone();
                                        match this.state.flip_chat_terminal(
                                            &rt,
                                            &tab_id_flip,
                                            sid_flip.as_deref(),
                                            now_ms,
                                        ) {
                                            Ok(mode) => {
                                                this.state.status_banner = Some(format!(
                                                    "Flipped '{}' to {:?}",
                                                    tab_id_flip, mode
                                                ));
                                            }
                                            Err(e) => this.state.error_banner = Some(e),
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("Flip Chat<->Term (Cmd+Shift+J)"),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-fork-sess-{tab_id}")))
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(3.0))
                                .rounded(px(5.0))
                                .bg(rgb(pal.input_bg))
                                .text_xs()
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let now_ms = chrono::Utc::now().timestamp_millis();
                                        let rt = this.runtime.clone();
                                        match this
                                            .state
                                            .ensure_session_for_tab(&rt, &tab_id_fork, now_ms)
                                            .and_then(|sid| {
                                                this.state.fork_session_into_tab(&rt, &sid, now_ms)
                                            }) {
                                            Ok(forked) => {
                                                this.state.status_banner = Some(format!(
                                                    "Forked session -> {}",
                                                    forked.session.session_id
                                                ));
                                            }
                                            Err(e) => this.state.error_banner = Some(e),
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("Fork"),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-resume-sess-{tab_id}")))
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(3.0))
                                .rounded(px(5.0))
                                .bg(rgb(pal.input_bg))
                                .text_xs()
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let now_ms = chrono::Utc::now().timestamp_millis();
                                        let rt = this.runtime.clone();
                                        match this
                                            .state
                                            .ensure_session_for_tab(&rt, &tab_id_resume, now_ms)
                                            .and_then(|sid| {
                                                this.state.resume_session_in_new_tab(
                                                    &rt,
                                                    &sid,
                                                    next_resume_mode,
                                                    now_ms,
                                                )
                                            }) {
                                            Ok(res) => {
                                                this.state.status_banner = Some(format!(
                                                    "Resumed in new {:?} session {}",
                                                    next_resume_mode, res.session.session_id
                                                ));
                                            }
                                            Err(e) => this.state.error_banner = Some(e),
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child(format!("Resume in {:?}", next_resume_mode)),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-cli-owner-{tab_id}")))
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(3.0))
                                .rounded(px(5.0))
                                .bg(if cli_owner_active {
                                    TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.2)
                                } else {
                                    rgb(pal.input_bg)
                                })
                                .text_xs()
                                .text_color(if cli_owner_active {
                                    rgb(pal.danger)
                                } else {
                                    rgb(pal.text_secondary)
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let now_ms = chrono::Utc::now().timestamp_millis();
                                        let rt = this.runtime.clone();
                                        if let Ok(sid) = this.state.ensure_session_for_tab(
                                            &rt,
                                            &tab_id_cli_owner,
                                            now_ms,
                                        ) {
                                            let _ = this.state.set_session_cli_owner(
                                                &rt,
                                                &sid,
                                                !cli_owner_active,
                                                now_ms,
                                            );
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child(if cli_owner_active {
                                    "CLI Owner: ON"
                                } else {
                                    "CLI Owner: OFF"
                                }),
                        ),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(items_col),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .id(SharedString::from(format!("orca-session-input-{tab_id}")))
                        .cursor_text()
                        .flex_1()
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if draft_active {
                            rgb(pal.accent)
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .text_xs()
                        .text_color(if ws.state.session_prompt_draft.is_empty() {
                            rgb(pal.text_secondary)
                        } else {
                            rgb(pal.text_primary)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, window, cx| {
                                cx.stop_propagation();
                                this.set_active_input(
                                    IdeInputTarget::SessionPromptDraft,
                                    window,
                                    cx,
                                );
                            }),
                        )
                        .child(draft_text),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("orca-session-send-{tab_id}")))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                let prompt = this.state.session_prompt_draft.clone();
                                match this
                                    .state
                                    .ensure_session_for_tab(&rt, &tab_id_send, now_ms)
                                    .and_then(|sid| {
                                        this.state
                                            .submit_session_prompt(&rt, &sid, &prompt, now_ms)
                                    }) {
                                    Ok(()) => {}
                                    Err(e) => this.state.error_banner = Some(e),
                                }
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "发送", "Send")),
                ),
        )
        .into_any_element()
}
