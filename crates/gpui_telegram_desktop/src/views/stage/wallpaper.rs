//! Authentic Telegram Desktop Chat Wallpaper & Empty-State Rendering.

use gpui::*;
use gpui::prelude::FluentBuilder;
use crate::i18n::tr_key_or;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};

pub fn render_themed_wallpaper(palette: &TdesktopThemePalette) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let has_custom_gradient = palette.wallpaper_top != palette.wallpaper_bottom;
    let pattern_rgba = TdesktopThemePalette::rgba_with_alpha(
        palette.wallpaper_pattern,
        if palette.mode.is_dark() { 0.14 } else { 0.10 },
    );
    let pattern_opacity = if palette.blur_wallpaper { 0.16 } else { 0.45 };

    // Pure clean macOS Apple Messages stage canvas in Apple Light/Dark (respects custom gradient wallpapers if selected),
    // or classic Telegram patterned wallpaper in Classic mode.
    let mut canvas_div = div().absolute().inset_0().overflow_hidden();
    if is_apple && !has_custom_gradient {
        canvas_div = canvas_div.bg(rgb(palette.stage_bg));
    } else {
        let gradient_bg = linear_gradient(
            145.0,
            linear_color_stop(rgb(palette.wallpaper_top), 0.0),
            linear_color_stop(rgb(palette.wallpaper_bottom), 1.0),
        );
        canvas_div = canvas_div.bg(gradient_bg);
    }

    canvas_div
        .when(!is_apple, |el| {
            el.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .justify_around()
                    .px(px(28.0))
                    .py(px(24.0))
                    .children((0..5).map(move |row| {
                        let offset_left = if row % 2 == 0 { 0.0 } else { 36.0 };
                        div()
                            .flex()
                            .items_center()
                            .justify_around()
                            .pl(px(offset_left))
                            .opacity(pattern_opacity)
                            .child(
                                div()
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(pattern_rgba),
                            )
                            .child(
                                div()
                                    .w(px(14.0))
                                    .h(px(14.0))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(pattern_rgba),
                            )
                            .child(tg_icon(IconName::HeartReaction, 16.0, palette.wallpaper_pattern))
                            .child(tg_icon(IconName::SmileEmoji, 18.0, palette.wallpaper_pattern))
                            .child(
                                div()
                                    .w(px(8.0))
                                    .h(px(8.0))
                                    .rounded_full()
                                    .bg(pattern_rgba),
                            )
                            .child(tg_icon(IconName::PremiumStar, 14.0, palette.wallpaper_pattern))
                    })),
            )
        })
        .when(palette.blur_wallpaper, |el| {
            let veil = TdesktopThemePalette::rgba_with_alpha(
                palette.window_bg,
                if palette.mode.is_dark() { 0.42 } else { 0.34 },
            );
            el.child(
                div()
                    .id("stage-wallpaper-blur-veil")
                    .absolute()
                    .inset_0()
                    .bg(veil),
            )
        })
        .into_any_element()
}

pub fn render_empty_no_chat_pill(palette: &TdesktopThemePalette, lang_id: &str) -> AnyElement {
    let is_apple = palette.is_apple_style();
    if is_apple {
        return div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .child(
                div()
                    .w(px(48.0))
                    .h(px(48.0))
                    .rounded_full()
                    .bg(TdesktopThemePalette::rgba_with_alpha(palette.text_secondary, 0.10))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(tg_icon(IconName::FolderChat, 22.0, palette.text_secondary)),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(palette.text_secondary))
                    .child(tr_key_or(
                        lang_id,
                        "lng_chat_select_empty",
                        "Select a chat to start messaging",
                    )),
            )
            .into_any_element();
    }

    div()
        .size_full()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .child(render_themed_wallpaper(palette))
        .child(
            div()
                .relative()
                .px(px(18.0))
                .py(px(8.0))
                .rounded_full()
                .bg(palette.service_pill_rgba())
                .shadow_sm()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(palette.service_pill_fg))
                .child(tr_key_or(
                    lang_id,
                    "lng_chat_select_empty",
                    "Select a chat to start messaging",
                )),
        )
        .into_any_element()
}

pub fn render_empty_history_placeholder(
    palette: &TdesktopThemePalette,
    lang_id: &str,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    if is_apple {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .p(px(32.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.0))
                    .max_w(px(280.0))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(palette.text_primary))
                            .child(tr_key_or(
                                lang_id,
                                "lng_empty_history_title",
                                "No messages here yet...",
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(palette.text_secondary))
                            .child(tr_key_or(
                                lang_id,
                                "lng_empty_history_subtitle",
                                "Send a message below to start the conversation.",
                            )),
                    ),
            )
            .into_any_element();
    }

    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.0))
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.0))
                .max_w(px(260.0))
                .px(px(22.0))
                .py(px(20.0))
                .rounded(px(16.0))
                .bg(palette.service_pill_rgba())
                .child(tg_icon(IconName::FolderChat, 38.0, palette.accent))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(palette.service_pill_fg))
                        .child(tr_key_or(lang_id, "lng_empty_history_title", "No messages here yet...")),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(palette.text_secondary))
                        .child(tr_key_or(
                            lang_id,
                            "lng_empty_history_subtitle",
                            "Send a message below to start the conversation.",
                        )),
                ),
        )
        .into_any_element()
}

/// Calm Telegram-style placeholder shown in the stage while the initial dialog list is syncing
/// on a cold cache, preventing a misleading empty-state flash before conversations arrive.
pub fn render_syncing_workspace_placeholder(
    palette: &TdesktopThemePalette,
    lang_id: &str,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let label = if lang_id.starts_with("zh") {
        "正在同步消息…"
    } else {
        "Updating…"
    };

    let pill = if is_apple {
        div()
            .id("stage-syncing-workspace-pill")
            .px(px(14.0))
            .py(px(7.0))
            .rounded_full()
            .bg(TdesktopThemePalette::rgba_with_alpha(
                palette.text_secondary,
                if palette.mode.is_dark() { 0.14 } else { 0.08 },
            ))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .w(px(7.0))
                    .h(px(7.0))
                    .rounded_full()
                    .bg(rgb(palette.accent)),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(palette.text_secondary))
                    .child(label),
            )
    } else {
        div()
            .id("stage-syncing-workspace-pill")
            .px(px(16.0))
            .py(px(7.0))
            .rounded_full()
            .bg(palette.service_pill_rgba())
            .shadow_sm()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .w(px(7.0))
                    .h(px(7.0))
                    .rounded_full()
                    .bg(rgb(palette.accent)),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(palette.service_pill_fg))
                    .child(label),
            )
    };

    div()
        .id("stage-syncing-workspace-placeholder")
        .size_full()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .child(pill.with_animation(
            "stage-syncing-workspace-pulse",
            Animation::new(std::time::Duration::from_millis(1200))
                .repeat()
                .with_easing(pulsating_between(0.50, 1.0)),
            |el, delta| el.opacity(delta),
        ))
        .into_any_element()
}

/// Telegram-style animated skeleton chat bubbles displayed while an uncached chat's history
/// is loading from the server, preventing false "No messages here yet..." flashes.
pub fn render_loading_history_skeleton(
    palette: &TdesktopThemePalette,
    _lang_id: &str,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let is_dark = palette.mode.is_dark();
    let radius = if is_apple {
        17.0
    } else {
        (palette.bubble_radius_px as f32).clamp(10.0, 18.0)
    };

    let incoming_bg = if is_dark {
        TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.085)
    } else {
        TdesktopThemePalette::rgba_with_alpha(0x000000, 0.065)
    };
    let outgoing_bg = TdesktopThemePalette::rgba_with_alpha(
        palette.accent,
        if is_dark { 0.22 } else { 0.14 },
    );
    let line_in = if is_dark {
        TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.13)
    } else {
        TdesktopThemePalette::rgba_with_alpha(0x000000, 0.09)
    };
    let line_out = TdesktopThemePalette::rgba_with_alpha(
        palette.accent,
        if is_dark { 0.34 } else { 0.22 },
    );

    // (is_outgoing, bubble_width, line1_width, optional line2_width, fade)
    let bubbles: [(bool, f32, f32, Option<f32>, f32); 6] = [
        (false, 224.0, 152.0, Some(98.0), 0.36),
        (true, 176.0, 118.0, None, 0.50),
        (false, 258.0, 188.0, Some(134.0), 0.64),
        (true, 208.0, 146.0, Some(82.0), 0.78),
        (false, 186.0, 126.0, None, 0.90),
        (true, 236.0, 168.0, Some(112.0), 0.98),
    ];

    div()
        .id("stage-loading-history-skeleton")
        .flex_1()
        .w_full()
        .px(px(16.0))
        .pb(px(12.0))
        .flex()
        .flex_col()
        .justify_end()
        .gap(px(10.0))
        .children(bubbles.into_iter().map(move |(is_out, bw, l1, l2, fade)| {
            let bg = if is_out { outgoing_bg } else { incoming_bg };
            let line_col = if is_out { line_out } else { line_in };
            div()
                .w_full()
                .flex()
                .when(is_out, |r| r.justify_end())
                .when(!is_out, |r| r.justify_start())
                .opacity(fade)
                .child(
                    div()
                        .w(px(bw))
                        .px(px(14.0))
                        .py(px(12.0))
                        .rounded(px(radius))
                        .bg(bg)
                        .flex()
                        .flex_col()
                        .gap(px(7.0))
                        .child(
                            div()
                                .w(px(l1))
                                .h(px(10.0))
                                .rounded_full()
                                .bg(line_col),
                        )
                        .when_some(l2, |col, w2| {
                            col.child(
                                div()
                                    .w(px(w2))
                                    .h(px(9.5))
                                    .rounded_full()
                                    .bg(line_col)
                                    .opacity(0.75),
                            )
                        }),
                )
        }))
        .with_animation(
            "stage-history-skeleton-pulse",
            Animation::new(std::time::Duration::from_millis(1200))
                .repeat()
                .with_easing(pulsating_between(0.48, 1.0)),
            |el, delta| el.opacity(delta),
        )
        .into_any_element()
}
