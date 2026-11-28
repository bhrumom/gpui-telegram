//! 1:1 OpenBot `PageRail` GPUI `0.2.2` renderer (`OB-F14`).
//!
//! Mirrors `packages/ui/src/features/window/PageRail.tsx`:
//! - 11 canonical `OpenBotPage` destinations (`agents..settings`), 7 pinnable, 3 hideable
//! - `canvas(...)` dual concentric `UsageRings` (`34px`, stroke `2.25`, spacing `4.5`, dashed unavailable arc)
//! - Hover `UsageCard` popover, `RailMoreMenu` with per-row Pin checkboxes, `RailHelpMenu`,
//!   `CreditsRailChip`, notice dot on `Settings`, and non-macOS `AppLogo`.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::providers::AgentProviderId;

use crate::components::bloub_avatar::render_app_logo;
use crate::theme::openbot_palette::OpenBotPalette;
use crate::theme::{IconName, tg_icon_rgba};

use super::shell_state::{
    OpenBotPage, PAGE_RAIL_PINNABLE_PAGES, PAGE_RAIL_WIDTH_PX, PageRailHideableItem, PageRailState,
    UsageRingMetric, UsageRingSpec,
};

fn pal(palette: &OpenBotPalette, token: &str, fallback: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback))
}

/// Maps each `OpenBotPage` destination to its embedded desktop SVG icon.
pub const fn page_rail_icon(page: OpenBotPage) -> IconName {
    match page {
        OpenBotPage::Agents => IconName::BotRobot,
        OpenBotPage::Workflows => IconName::WorkflowWaypoints,
        OpenBotPage::Tasks => IconName::SelectCheck,
        OpenBotPage::Schedule => IconName::ClockSending,
        OpenBotPage::Usage => IconName::Poll,
        OpenBotPage::Library => IconName::FolderAll,
        OpenBotPage::Providers => IconName::ComputerDesktop,
        OpenBotPage::Crm => IconName::GroupUsers,
        OpenBotPage::Marketplace => IconName::FolderWork,
        OpenBotPage::Credits => IconName::WalletStars,
        OpenBotPage::Settings => IconName::SettingsGear,
    }
}

/// Canonical OpenBot provider accent color used in `UsageRings` and `UsageCard`.
pub const fn provider_usage_accent_rgb(provider: AgentProviderId, dark_scheme: bool) -> u32 {
    match provider {
        AgentProviderId::Codex => {
            if dark_scheme {
                0xD6ADF2
            } else {
                0x8E4FC0
            }
        }
        AgentProviderId::Claude => 0xD97757,
        AgentProviderId::Grok => {
            if dark_scheme {
                0x6BC7D9
            } else {
                0x16849B
            }
        }
        AgentProviderId::Antigravity => 0x8E75B2,
        AgentProviderId::Cursor => 0x42A0FF,
        AgentProviderId::DeepSeek => 0x4F7BFF,
        AgentProviderId::OpenRouter => 0x7C6FF7,
        AgentProviderId::Mistral => 0xF08A24,
        AgentProviderId::Copilot => 0x31CF76,
        _ => 0x007CF7,
    }
}

/// Compact 1-2 character monogram centered inside a `34×34px` `UsageRings` canvas.
pub const fn provider_usage_monogram(provider: AgentProviderId) -> &'static str {
    match provider {
        AgentProviderId::Codex => "Cx",
        AgentProviderId::Claude => "Cl",
        AgentProviderId::Grok => "Gk",
        AgentProviderId::OpenCode => "OC",
        AgentProviderId::Antigravity => "AG",
        AgentProviderId::Cursor => "Cu",
        AgentProviderId::Cline => "Cn",
        AgentProviderId::Pi => "Pi",
        AgentProviderId::Muse => "Mu",
        AgentProviderId::Inception => "In",
        AgentProviderId::DeepSeek => "DS",
        AgentProviderId::OpenRouter => "OR",
        AgentProviderId::Hermes => "Hm",
        AgentProviderId::Acp => "AC",
        AgentProviderId::Kimi => "Km",
        AgentProviderId::Devin => "Dv",
        AgentProviderId::Copilot => "Cp",
        AgentProviderId::Mistral => "Ms",
    }
}

fn stroke_arc(
    window: &mut Window,
    center: (f32, f32),
    radius: f32,
    stroke_px: f32,
    start_angle: f32,
    sweep_angle: f32,
    color: Rgba,
) {
    if radius <= 1.0 || sweep_angle <= 0.005 {
        return;
    }
    let steps = ((sweep_angle / TAU) * 48.0).ceil().clamp(4.0, 64.0) as usize;
    let mut builder = PathBuilder::stroke(px(stroke_px));
    for idx in 0..=steps {
        let t = (idx as f32) / (steps as f32);
        let theta = start_angle + sweep_angle * t;
        let x = center.0 + radius * theta.cos();
        let y = center.1 + radius * theta.sin();
        let pt = point(px(x), px(y));
        if idx == 0 {
            builder.move_to(pt);
        } else {
            builder.line_to(pt);
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn stroke_dashed_ring(
    window: &mut Window,
    center: (f32, f32),
    radius: f32,
    stroke_px: f32,
    color: Rgba,
) {
    // 10 dashes around pathLength=100 -> dash 4, gap 6 (`stroke-dasharray="4 6"`).
    let dash_count = 10usize;
    let step = TAU / (dash_count as f32);
    let dash_arc = step * 0.40;
    for idx in 0..dash_count {
        let start = -FRAC_PI_2 + (idx as f32) * step;
        stroke_arc(window, center, radius, stroke_px, start, dash_arc, color);
    }
}

/// Paints dual concentric `UsageRings` (`34px`, stroke `2.25px`, spacing `4.5px`) directly onto GPUI `Window`.
pub fn paint_usage_rings_canvas(
    bounds: Bounds<Pixels>,
    metric: &UsageRingMetric,
    spec: UsageRingSpec,
    track_rgba: Rgba,
    primary_rgba: Rgba,
    secondary_rgba: Rgba,
    window: &mut Window,
) {
    let bw = f32::from(bounds.size.width).max(1.0);
    let bh = f32::from(bounds.size.height).max(1.0);
    let side = bw.min(bh);
    let center = (
        f32::from(bounds.origin.x) + bw * 0.5,
        f32::from(bounds.origin.y) + bh * 0.5,
    );
    let outer_r = (side * 0.5 - spec.stroke_px * 0.5 - 1.0).max(4.0);
    let inner_r = (outer_r - spec.spacing_px).max(2.0);

    if metric.unavailable {
        stroke_dashed_ring(window, center, outer_r, spec.stroke_px, track_rgba);
        stroke_dashed_ring(window, center, inner_r, spec.stroke_px, track_rgba);
        return;
    }

    stroke_arc(window, center, outer_r, spec.stroke_px, -FRAC_PI_2, TAU, track_rgba);
    if let Some(primary) = metric.primary_ratio {
        stroke_arc(
            window,
            center,
            outer_r,
            spec.stroke_px,
            -FRAC_PI_2,
            TAU * primary.clamp(0.0, 1.0),
            primary_rgba,
        );
    }

    if let Some(secondary) = metric.secondary_ratio {
        stroke_arc(window, center, inner_r, spec.stroke_px, -FRAC_PI_2, TAU, track_rgba);
        stroke_arc(
            window,
            center,
            inner_r,
            spec.stroke_px,
            -FRAC_PI_2,
            TAU * secondary.clamp(0.0, 1.0),
            secondary_rgba,
        );
    }
}

/// Renders a single provider `UsageRings` rail button with dual concentric `canvas` rings + monogram.
pub fn render_usage_ring_button(
    metric: &UsageRingMetric,
    spec: UsageRingSpec,
    hovered: bool,
    palette: &OpenBotPalette,
) -> Stateful<Div> {
    let dark = palette.scheme.is_dark();
    let accent_u32 = provider_usage_accent_rgb(metric.provider, dark);
    let primary_rgba = rgb(accent_u32);
    let secondary_rgba = Rgba {
        a: 0.72,
        ..primary_rgba
    };
    let track_rgba = pal(palette, "--openbot-border-strong", 0x333335);
    let hover_bg = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let text_sec = pal(palette, "--openbot-text-secondary", 0xDCDCDC);
    let metric_for_paint = metric.clone();
    let size = spec.size_px;

    div()
        .id(SharedString::from(format!(
            "openbot-usage-ring-{}",
            metric.provider.as_str()
        )))
        .relative()
        .w(px(size))
        .h(px(size))
        .rounded_full()
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .when(hovered, |el| el.bg(hover_bg))
        .hover(move |style| style.bg(hover_bg))
        .child(
            canvas(
                |bounds, _, _| bounds,
                move |bounds, _, window, _cx| {
                    paint_usage_rings_canvas(
                        bounds,
                        &metric_for_paint,
                        spec,
                        track_rgba,
                        primary_rgba,
                        secondary_rgba,
                        window,
                    );
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .w(px(size))
            .h(px(size)),
        )
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .text_color(text_sec)
                .child(provider_usage_monogram(metric.provider)),
        )
}

/// Renders the hover `UsageCard` popover (`placement: "right-end"`) for an active usage ring.
pub fn render_usage_hover_card(metric: &UsageRingMetric, palette: &OpenBotPalette) -> Div {
    let bg = pal(palette, "--openbot-bg-overlay", 0x1A1A1A);
    let border = pal(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);
    let accent = rgb(provider_usage_accent_rgb(
        metric.provider,
        palette.scheme.is_dark(),
    ));

    let primary_pct = metric
        .primary_ratio
        .map(|v| format!("{}%", (v * 100.0).round() as u32))
        .unwrap_or_else(|| "—".to_string());
    let secondary_pct = metric
        .secondary_ratio
        .map(|v| format!("{}%", (v * 100.0).round() as u32))
        .unwrap_or_else(|| "—".to_string());

    div()
        .w(px(220.0))
        .p(px(12.0))
        .rounded(px(14.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child(metric.label.clone()),
                )
                .child(
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded_full()
                        .bg(if metric.unavailable { text_muted } else { accent }),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(12.0))
                .text_color(text_muted)
                .child(if metric.unavailable {
                    "Quota unavailable"
                } else {
                    "Session / Weekly"
                })
                .child(
                    div()
                        .text_color(text_pri)
                        .font_weight(FontWeight::MEDIUM)
                        .child(if metric.unavailable {
                            "Dashed".to_string()
                        } else {
                            format!("{primary_pct} · {secondary_pct}")
                        }),
                ),
        )
        .when_some(metric.reset_label.clone(), |el, reset| {
            el.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted)
                    .child(reset),
            )
        })
        .when_some(metric.detail_summary.clone(), |el, detail| {
            el.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted)
                    .child(detail),
            )
        })
}

/// Renders the `...` `RailMoreMenu` popover with per-row Pin checkboxes for all 7 pinnable pages.
pub fn render_rail_more_menu(
    state: &PageRailState,
    palette: &OpenBotPalette,
    on_select: Rc<dyn Fn(OpenBotPage)>,
) -> Div {
    let bg = pal(palette, "--openbot-bg-overlay", 0x1A1A1A);
    let border = pal(palette, "--openbot-border-grouped", 0x333335);
    let hover = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);
    let accent = pal(palette, "--openbot-accent", 0x007CF7);

    let mut menu = div()
        .w(px(212.0))
        .p(px(6.0))
        .rounded(px(14.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_muted)
                .child("PAGES & PINS"),
        );

    for page in PAGE_RAIL_PINNABLE_PAGES {
        let pinned = state.is_pinned(page);
        let active = state.active_page == page;
        let cb = Rc::clone(&on_select);
        menu = menu.child(
            div()
                .id(SharedString::from(format!(
                    "openbot-rail-more-{}",
                    page.as_str()
                )))
                .h(px(32.0))
                .px(px(8.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_between()
                .when(active, |el| el.bg(hover))
                .hover(move |style| style.bg(hover))
                .on_mouse_down(MouseButton::Left, move |_, _, _| cb(page))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(tg_icon_rgba(
                            page_rail_icon(page),
                            15.0,
                            if active { accent } else { text_muted },
                        ))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(text_pri)
                                .child(page.label()),
                        ),
                )
                .child(
                    div()
                        .w(px(16.0))
                        .h(px(16.0))
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(if pinned { accent } else { border })
                        .when(pinned, |el| el.bg(accent))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(pinned, |el| {
                            el.child(tg_icon_rgba(IconName::CheckSingle, 11.0, rgb(0xFFFFFF)))
                        }),
                ),
        );
    }

    menu
}

/// Renders the compact `CreditsRailChip` (`PageRail.tsx:L812`).
pub fn render_credits_rail_chip(
    balance_label: &str,
    active: bool,
    palette: &OpenBotPalette,
    on_select: Rc<dyn Fn(OpenBotPage)>,
) -> Stateful<Div> {
    let bg = if active {
        pal(palette, "--openbot-accent-soft", 0x163150)
    } else {
        pal(palette, "--openbot-bg-control", 0x212121)
    };
    let hover = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let border = pal(palette, "--openbot-border-grouped", 0x333335);
    let text = if active {
        pal(palette, "--openbot-accent-text", 0x42A0FF)
    } else {
        pal(palette, "--openbot-text-secondary", 0xDCDCDC)
    };

    div()
        .id("openbot-credits-rail-chip")
        .w(px(42.0))
        .py(px(4.0))
        .px(px(4.0))
        .rounded(px(10.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .cursor_pointer()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(2.0))
        .hover(move |style| style.bg(hover))
        .on_mouse_down(MouseButton::Left, move |_, _, _| {
            on_select(OpenBotPage::Credits)
        })
        .child(tg_icon_rgba(IconName::WalletStars, 14.0, text))
        .child(
            div()
                .text_size(px(9.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text)
                .truncate()
                .child(balance_label.to_string()),
        )
}

fn render_rail_icon_button(
    id: impl Into<SharedString>,
    icon: IconName,
    active: bool,
    notice_dot: bool,
    palette: &OpenBotPalette,
    on_click: Rc<dyn Fn()>,
) -> Stateful<Div> {
    let active_bg = pal(palette, "--openbot-bg-control-active", 0x2A2A2A);
    let hover_bg = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let icon_color = if active {
        pal(palette, "--openbot-text-primary", 0xFFFFFF)
    } else {
        pal(palette, "--openbot-text-muted", 0x979797)
    };
    let notice_color = pal(palette, "--openbot-accent", 0x007CF7);

    div()
        .id(id.into())
        .relative()
        .w(px(36.0))
        .h(px(36.0))
        .rounded(px(10.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .when(active, |el| el.bg(active_bg))
        .hover(move |style| style.bg(hover_bg))
        .on_mouse_down(MouseButton::Left, move |_, _, _| on_click())
        .child(tg_icon_rgba(icon, 18.0, icon_color))
        .when(notice_dot, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(5.0))
                    .right(px(5.0))
                    .w(px(7.0))
                    .h(px(7.0))
                    .rounded_full()
                    .bg(notice_color),
            )
        })
}

/// Renders the deferred `RailHelpMenu` popover (`PageRail.tsx:L835`).
pub fn render_rail_help_menu(palette: &OpenBotPalette) -> Div {
    let bg = pal(palette, "--openbot-bg-overlay", 0x1A1A1A);
    let border = pal(palette, "--openbot-border-grouped", 0x333335);
    let hover = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);

    let items = [
        ("shortcuts", "Keyboard shortcuts", "Cmd+/"),
        ("docs", "OpenBot docs", ""),
        ("diagnostics", "Export diagnostics", ""),
    ];

    let mut box_el = div()
        .w(px(200.0))
        .p(px(6.0))
        .rounded(px(14.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(px(2.0));

    for (key, label, hint) in items {
        box_el = box_el.child(
            div()
                .id(SharedString::from(format!("openbot-rail-help-{key}")))
                .h(px(30.0))
                .px(px(8.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_between()
                .hover(move |style| style.bg(hover))
                .child(div().text_size(px(13.0)).text_color(text_pri).child(label))
                .when(!hint.is_empty(), |el| {
                    el.child(div().text_size(px(11.0)).text_color(text_muted).child(hint))
                }),
        );
    }

    box_el
}

/// Renders the complete 1:1 OpenBot `PageRail` (`PageRail.tsx:L160-L868`).
pub fn render_page_rail<F>(
    state: &PageRailState,
    palette: &OpenBotPalette,
    usage_rings: &[UsageRingMetric],
    active_page: OpenBotPage,
    reduced_motion: bool,
    on_select: F,
) -> Div
where
    F: Fn(OpenBotPage) + 'static,
{
    let bg_frame = pal(palette, "--openbot-bg-frame", 0x0D0D0D);
    let border = pal(palette, "--openbot-border", 0x222222);
    let hover_bg = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);
    let on_select_rc: Rc<dyn Fn(OpenBotPage)> = Rc::new(on_select);

    // Top navigation stack: primary Agents + pinned pages + `...` RailMoreMenu.
    let mut top_stack = div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.0))
        .pt(px(10.0));

    if state.show_app_logo {
        let mut logo_state = state.app_logo.clone();
        logo_state.set_reduced_motion(reduced_motion);
        let cb_logo = Rc::clone(&on_select_rc);
        top_stack = top_stack.child(
            div()
                .id("openbot-page-rail-app-logo")
                .mb(px(6.0))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, _, _| {
                    cb_logo(OpenBotPage::Agents)
                })
                .child(render_app_logo(&logo_state, 0.0, 28.0)),
        );
    }

    let cb_agents = Rc::clone(&on_select_rc);
    top_stack = top_stack.child(render_rail_icon_button(
        "openbot-rail-page-agents",
        page_rail_icon(OpenBotPage::Agents),
        active_page == OpenBotPage::Agents,
        false,
        palette,
        Rc::new(move || cb_agents(OpenBotPage::Agents)),
    ));

    for &page in &state.pinned_pages {
        let cb = Rc::clone(&on_select_rc);
        top_stack = top_stack.child(render_rail_icon_button(
            format!("openbot-rail-page-{}", page.as_str()),
            page_rail_icon(page),
            active_page == page,
            false,
            palette,
            Rc::new(move || cb(page)),
        ));
    }

    let unpinned_count = state.unpinned_pinnable_pages().len();
    let cb_more = Rc::clone(&on_select_rc);
    let more_anchor = div()
        .relative()
        .child(
            div()
                .id("openbot-rail-more-trigger")
                .relative()
                .w(px(36.0))
                .h(px(36.0))
                .rounded(px(10.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .when(state.more_menu_open, |el| el.bg(hover_bg))
                .hover(move |style| style.bg(hover_bg))
                .child(tg_icon_rgba(IconName::MoreVertical, 18.0, text_muted))
                .when(unpinned_count > 0 && !state.more_menu_open, |el| {
                    el.child(
                        div()
                            .absolute()
                            .bottom(px(5.0))
                            .right(px(5.0))
                            .w(px(5.0))
                            .h(px(5.0))
                            .rounded_full()
                            .bg(text_muted),
                    )
                })
                .when(state.more_menu_open, |el| {
                    el.child(
                        deferred(
                            div()
                                .absolute()
                                .left(px(PAGE_RAIL_WIDTH_PX - 6.0))
                                .top(px(0.0))
                                .occlude()
                                .child(render_rail_more_menu(state, palette, cb_more)),
                        )
                        .with_priority(50),
                    )
                }),
        );
    top_stack = top_stack.child(more_anchor);

    // Bottom status stack: UsageRings (+ hover UsageCard + overflow), Credits, Marketplace, Help, Settings.
    let (visible_rings, overflow_rings) = state.partition_usage_rings(usage_rings, 176.0);
    let mut bottom_stack = div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(6.0))
        .pb(px(10.0));

    for metric in visible_rings {
        let is_hovered = state.active_usage_hover_provider == Some(metric.provider);
        let cb_usage = Rc::clone(&on_select_rc);
        let ring_btn = render_usage_ring_button(metric, state.usage_ring_spec, is_hovered, palette)
            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                cb_usage(OpenBotPage::Usage)
            });
        bottom_stack = bottom_stack.child(
            div()
                .relative()
                .child(ring_btn)
                .when(is_hovered, |el| {
                    el.child(
                        deferred(
                            div()
                                .absolute()
                                .left(px(PAGE_RAIL_WIDTH_PX - 6.0))
                                .bottom(px(0.0))
                                .occlude()
                                .child(render_usage_hover_card(metric, palette)),
                        )
                        .with_priority(55),
                    )
                }),
        );
    }

    if !overflow_rings.is_empty() {
        let cb_overflow = Rc::clone(&on_select_rc);
        bottom_stack = bottom_stack.child(
            div()
                .id("openbot-rail-usage-overflow")
                .w(px(34.0))
                .h(px(22.0))
                .rounded(px(999.0))
                .bg(pal(palette, "--openbot-bg-control", 0x212121))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_muted)
                .hover(move |style| style.bg(hover_bg))
                .on_mouse_down(MouseButton::Left, move |_, _, _| {
                    cb_overflow(OpenBotPage::Usage)
                })
                .child(format!("+{}", overflow_rings.len())),
        );
    }

    if let Some(ref media_label) = state.media_credits_label {
        let cb_media = Rc::clone(&on_select_rc);
        bottom_stack = bottom_stack.child(
            div()
                .id("openbot-rail-media-credits")
                .px(px(5.0))
                .py(px(2.0))
                .rounded(px(8.0))
                .bg(pal(palette, "--openbot-bg-control", 0x212121))
                .cursor_pointer()
                .text_size(px(9.5))
                .text_color(text_muted)
                .on_mouse_down(MouseButton::Left, move |_, _, _| {
                    cb_media(OpenBotPage::Credits)
                })
                .child(media_label.clone()),
        );
    }

    if !state.is_hidden(PageRailHideableItem::Credits) {
        let balance = state
            .credits_balance_label
            .as_deref()
            .unwrap_or("Credits");
        bottom_stack = bottom_stack.child(render_credits_rail_chip(
            balance,
            active_page == OpenBotPage::Credits,
            palette,
            Rc::clone(&on_select_rc),
        ));
    }

    if !state.is_hidden(PageRailHideableItem::Marketplace) {
        let cb_market = Rc::clone(&on_select_rc);
        bottom_stack = bottom_stack.child(render_rail_icon_button(
            "openbot-rail-page-marketplace",
            page_rail_icon(OpenBotPage::Marketplace),
            active_page == OpenBotPage::Marketplace,
            false,
            palette,
            Rc::new(move || cb_market(OpenBotPage::Marketplace)),
        ));
    }

    if !state.is_hidden(PageRailHideableItem::Help) {
        bottom_stack = bottom_stack.child(
            div()
                .relative()
                .child(
                    div()
                        .id("openbot-rail-help-trigger")
                        .w(px(36.0))
                        .h(px(36.0))
                        .rounded(px(10.0))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(text_muted)
                        .hover(move |style| style.bg(hover_bg))
                        .child("?"),
                )
                .when(state.help_menu_open, |el| {
                    el.child(
                        deferred(
                            div()
                                .absolute()
                                .left(px(PAGE_RAIL_WIDTH_PX - 6.0))
                                .bottom(px(0.0))
                                .occlude()
                                .child(render_rail_help_menu(palette)),
                        )
                        .with_priority(50),
                    )
                }),
        );
    }

    let cb_settings = Rc::clone(&on_select_rc);
    bottom_stack = bottom_stack.child(render_rail_icon_button(
        "openbot-rail-page-settings",
        page_rail_icon(OpenBotPage::Settings),
        active_page == OpenBotPage::Settings,
        state.update_notice,
        palette,
        Rc::new(move || cb_settings(OpenBotPage::Settings)),
    ));

    div()
        .w(px(PAGE_RAIL_WIDTH_PX))
        .h_full()
        .flex_shrink_0()
        .bg(bg_frame)
        .border_r_1()
        .border_color(border)
        .flex()
        .flex_col()
        .items_center()
        .justify_between()
        .child(top_stack)
        .child(bottom_stack)
}
