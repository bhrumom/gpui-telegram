//! 1:1 OpenBot `ServerRail` GPUI `0.2.2` renderer (`OB-F14`).
//!
//! Mirrors `packages/ui/src/features/servers/ServerRail.tsx`:
//! - Responsive width (`72px` macOS, `64px` desktop, `56px` narrow `<= 800px`, `0px` menu mode)
//! - `ServerPlushLogo` tiles (`render_server_plush_logo`), `ServerSleepDot` (`sleeping | waking`)
//! - Connection/mute/unread badges, local vs drag-reorderable remote server divider
//! - `HiddenServerFolder` 2×2 preview grid (up to `HIDDEN_FOLDER_PREVIEW_COUNT = 4`)

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::components::bloub_avatar::{ServerPlushLogo, render_server_plush_logo};
use crate::theme::openbot_palette::OpenBotPalette;
use crate::theme::{IconName, tg_icon_rgba};

use super::shell_state::{
    HIDDEN_FOLDER_PREVIEW_COUNT, ServerConnectionState, ServerEntry, ServerPlatform,
    ServerRailState, ServerSleepState,
};

fn pal(palette: &OpenBotPalette, token: &str, fallback: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback))
}

/// Renders the `ServerSleepDot` badge (`sleeping` | `waking`) anchored on a server tile.
pub fn render_server_sleep_dot(
    sleep_state: ServerSleepState,
    palette: &OpenBotPalette,
) -> Option<Div> {
    let bg_frame = pal(palette, "--openbot-bg-frame", 0x0D0D0D);
    let (dot_color, label) = match sleep_state {
        ServerSleepState::Awake => return None,
        ServerSleepState::Sleeping => (rgb(0x8E75B2), "z"),
        ServerSleepState::Waking => (pal(palette, "--openbot-warning", 0xFF9412), "·"),
    };

    Some(
        div()
            .absolute()
            .bottom(px(-2.0))
            .left(px(-2.0))
            .w(px(14.0))
            .h(px(14.0))
            .rounded_full()
            .bg(dot_color)
            .border_2()
            .border_color(bg_frame)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(8.5))
            .font_weight(FontWeight::BOLD)
            .text_color(rgb(0xFFFFFF))
            .child(label),
    )
}

/// Renders the server connection state, mute icon, or unread/waiting counter badge.
pub fn render_server_status_badge(
    entry: &ServerEntry,
    palette: &OpenBotPalette,
) -> Option<Div> {
    let bg_frame = pal(palette, "--openbot-bg-frame", 0x0D0D0D);
    let danger = pal(palette, "--openbot-danger", 0xFD2F3B);
    let warning = pal(palette, "--openbot-warning", 0xFF9412);
    let muted = pal(palette, "--openbot-text-muted", 0x979797);
    let surface = pal(palette, "--openbot-bg-surface-raised", 0x242424);

    match entry.connection {
        ServerConnectionState::Error | ServerConnectionState::Incompatible => {
            return Some(
                div()
                    .absolute()
                    .bottom(px(-2.0))
                    .right(px(-2.0))
                    .w(px(14.0))
                    .h(px(14.0))
                    .rounded_full()
                    .bg(danger)
                    .border_2()
                    .border_color(bg_frame)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xFFFFFF))
                    .child("!"),
            );
        }
        ServerConnectionState::Offline => {
            return Some(
                div()
                    .absolute()
                    .bottom(px(-2.0))
                    .right(px(-2.0))
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded_full()
                    .bg(muted)
                    .border_2()
                    .border_color(bg_frame),
            );
        }
        ServerConnectionState::Connecting => {
            return Some(
                div()
                    .absolute()
                    .bottom(px(-2.0))
                    .right(px(-2.0))
                    .w(px(12.0))
                    .h(px(12.0))
                    .rounded_full()
                    .bg(warning)
                    .border_2()
                    .border_color(bg_frame),
            );
        }
        ServerConnectionState::Online => {}
    }

    if entry.muted {
        return Some(
            div()
                .absolute()
                .bottom(px(-2.0))
                .right(px(-2.0))
                .w(px(15.0))
                .h(px(15.0))
                .rounded_full()
                .bg(surface)
                .border_1()
                .border_color(bg_frame)
                .flex()
                .items_center()
                .justify_center()
                .child(tg_icon_rgba(IconName::Mute, 9.0, muted)),
        );
    }

    if entry.waiting_count > 0 {
        return Some(
            div()
                .absolute()
                .top(px(-3.0))
                .right(px(-3.0))
                .min_w(px(16.0))
                .h(px(16.0))
                .px(px(4.0))
                .rounded_full()
                .bg(warning)
                .border_2()
                .border_color(bg_frame)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(9.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .child(entry.waiting_count.min(99).to_string()),
        );
    }

    if entry.unread_count > 0 {
        return Some(
            div()
                .absolute()
                .top(px(-3.0))
                .right(px(-3.0))
                .min_w(px(16.0))
                .h(px(16.0))
                .px(px(4.0))
                .rounded_full()
                .bg(rgb(0x5D3C70))
                .border_2()
                .border_color(bg_frame)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(9.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .child(entry.unread_count.min(99).to_string()),
        );
    }

    None
}

/// Renders the `HiddenServerFolder` 2×2 preview grid (up to `HIDDEN_FOLDER_PREVIEW_COUNT = 4`)
/// and optional deferred expanded popover listing all hidden servers.
pub fn render_hidden_server_folder(
    hidden_servers: &[&ServerEntry],
    open: bool,
    palette: &OpenBotPalette,
) -> Div {
    let bg_ctrl = pal(palette, "--openbot-bg-control", 0x212121);
    let hover_bg = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let bg_overlay = pal(palette, "--openbot-bg-overlay", 0x1A1A1A);
    let border = pal(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);

    let preview: Vec<&ServerEntry> = hidden_servers
        .iter()
        .copied()
        .take(HIDDEN_FOLDER_PREVIEW_COUNT)
        .collect();

    let mut row_top = div().flex().items_center().justify_center().gap(px(3.0));
    let mut row_bot = div().flex().items_center().justify_center().gap(px(3.0));
    for (idx, srv) in preview.iter().enumerate() {
        let mini = render_server_plush_logo(
            &ServerPlushLogo::new(&srv.id)
                .with_appearance(srv.appearance.clone())
                .with_size(14.0),
        );
        if idx < 2 {
            row_top = row_top.child(mini);
        } else {
            row_bot = row_bot.child(mini);
        }
    }

    let mut popover = div()
        .w(px(210.0))
        .p(px(8.0))
        .rounded(px(14.0))
        .bg(bg_overlay)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .px(px(6.0))
                .py(px(2.0))
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_muted)
                .child(format!("HIDDEN SERVERS ({})", hidden_servers.len())),
        );

    for srv in hidden_servers {
        let logo = render_server_plush_logo(
            &ServerPlushLogo::new(&srv.id)
                .with_appearance(srv.appearance.clone())
                .with_size(22.0),
        );
        popover = popover.child(
            div()
                .id(SharedString::from(format!("openbot-hidden-srv-{}", srv.id)))
                .h(px(34.0))
                .px(px(6.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(8.0))
                .hover(move |s| s.bg(hover_bg))
                .child(logo)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(13.0))
                        .text_color(text_pri)
                        .child(srv.name.clone()),
                ),
        );
    }

    div()
        .relative()
        .child(
            div()
                .id("openbot-hidden-server-folder")
                .w(px(40.0))
                .h(px(40.0))
                .rounded(px(12.0))
                .bg(bg_ctrl)
                .border_1()
                .border_color(border)
                .cursor_pointer()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(3.0))
                .hover(move |s| s.bg(hover_bg))
                .child(row_top)
                .when(preview.len() > 2, |el| el.child(row_bot)),
        )
        .when(open, |el| {
            el.child(
                deferred(
                    div()
                        .absolute()
                        .left(px(52.0))
                        .bottom(px(0.0))
                        .occlude()
                        .child(popover),
                )
                .with_priority(50),
            )
        })
}

/// Renders an individual server plush tile with active indicator pill, sleep dot, and status badge.
pub fn render_server_tile(
    entry: &ServerEntry,
    active: bool,
    tile_size_px: f32,
    dragging: bool,
    palette: &OpenBotPalette,
) -> Stateful<Div> {
    let active_pill = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let active_bg = pal(palette, "--openbot-bg-control-active", 0x2A2A2A);
    let hover_bg = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let accent = pal(palette, "--openbot-accent", 0x007CF7);
    let logo_size = (tile_size_px - 6.0).max(24.0);
    let logo = render_server_plush_logo(
        &ServerPlushLogo::new(&entry.id)
            .with_appearance(entry.appearance.clone())
            .with_size(logo_size),
    );

    div()
        .id(SharedString::from(format!("openbot-server-tile-{}", entry.id)))
        .relative()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .py(px(3.0))
        .cursor_pointer()
        .when(dragging, |el| el.opacity(0.65))
        .when(active, |el| {
            el.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .w(px(3.0))
                    .h(px(22.0))
                    .rounded_r(px(3.0))
                    .bg(active_pill),
            )
        })
        .child(
            div()
                .id(SharedString::from(format!(
                    "openbot-server-tile-btn-{}",
                    entry.id
                )))
                .relative()
                .w(px(tile_size_px))
                .h(px(tile_size_px))
                .rounded(px(12.0))
                .flex()
                .items_center()
                .justify_center()
                .when(active, |el| {
                    el.bg(active_bg).border_1().border_color(accent)
                })
                .hover(move |s| s.bg(hover_bg))
                .child(logo)
                .when_some(render_server_sleep_dot(entry.sleep_state, palette), |el, dot| {
                    el.child(dot)
                })
                .when_some(render_server_status_badge(entry, palette), |el, badge| {
                    el.child(badge)
                }),
        )
}

/// Renders the responsive 1:1 OpenBot `ServerRail` (`0px` menu, `56px` narrow, `64px` desktop, `72px` macOS).
pub fn render_server_rail(
    state: &ServerRailState,
    palette: &OpenBotPalette,
    active_server_id: &str,
    platform: ServerPlatform,
    window_width_px: u32,
) -> Div {
    let width = state.effective_width_px(platform, window_width_px);
    if width <= 0.0 {
        return div().w(px(0.0)).h_full().flex_shrink_0().overflow_hidden();
    }

    let bg_frame = pal(palette, "--openbot-bg-frame", 0x0D0D0D);
    let border = pal(palette, "--openbot-border", 0x222222);
    let divider = pal(palette, "--openbot-border-strong", 0x333335);
    let accent = pal(palette, "--openbot-accent", 0x007CF7);
    let ctrl_bg = pal(palette, "--openbot-bg-control", 0x212121);
    let hover_bg = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);

    let tile_size = if width <= 56.0 { 36.0 } else { 42.0 };
    let top_inset = if platform == ServerPlatform::MacOs {
        36.0
    } else {
        10.0
    };

    let locals = state.local_servers();
    let remotes = state.visible_remote_servers();
    let hidden = state.hidden_servers();

    let mut local_group = div().w_full().flex().flex_col().items_center().gap(px(4.0));
    for srv in &locals {
        local_group = local_group.child(render_server_tile(
            srv,
            srv.id == active_server_id,
            tile_size,
            state.dragging_server_id.as_deref() == Some(srv.id.as_str()),
            palette,
        ));
    }

    let mut remote_group = div()
        .id("openbot-server-rail-remotes")
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.0));

    for (idx, srv) in remotes.iter().enumerate() {
        if state.dragging_server_id.is_some() && state.drag_target_index == Some(idx) {
            remote_group = remote_group.child(div().w(px(28.0)).h(px(2.0)).rounded_full().bg(accent));
        }
        remote_group = remote_group.child(render_server_tile(
            srv,
            srv.id == active_server_id,
            tile_size,
            state.dragging_server_id.as_deref() == Some(srv.id.as_str()),
            palette,
        ));
    }

    div()
        .w(px(width))
        .h_full()
        .flex_shrink_0()
        .bg(bg_frame)
        .border_r_1()
        .border_color(border)
        .pt(px(top_inset))
        .pb(px(10.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0))
        .child(local_group)
        .when(!locals.is_empty() && !remotes.is_empty(), |el| {
            el.child(div().w(px(28.0)).h(px(1.0)).bg(divider))
        })
        .child(remote_group)
        .when(!hidden.is_empty(), |el| {
            el.child(render_hidden_server_folder(
                &hidden,
                state.hidden_folder_open,
                palette,
            ))
        })
        .child(
            div()
                .id("openbot-server-rail-add")
                .w(px(36.0))
                .h(px(36.0))
                .rounded(px(12.0))
                .bg(ctrl_bg)
                .border_1()
                .border_color(divider)
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(18.0))
                .text_color(text_muted)
                .hover(move |s| s.bg(hover_bg))
                .child("+"),
        )
}
