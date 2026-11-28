//! OpenBot 1:1 Browser Vector Icons & Reusable GPUI Chrome Primitives.
//!
//! Mirrors upstream `nightly-labs/openbot`:
//! - `packages/ui/src/features/conversation/BrowserPreviewSidebar.tsx` (`image.png` / `image_3.png`)
//! - `packages/ui/src/features/browser/BrowserFullView.tsx` (`image_2.png`)
//! - `packages/ui/src/features/browser/BrowserTabStrip.tsx` & `BrowserTabMenu.tsx`
//! - `packages/ui/src/features/browser/BrowserAddressRow.tsx` & `BrowserPageMenu.tsx`
//! - `packages/ui/src/features/browser/BrowserTeachBar.tsx`
//! - `packages/ui/src/features/browser/BrowserDock.tsx` & `BrowserIsland.tsx`

use gpui::*;
use openbot_contracts::avatar::AvatarMood;

use crate::components::avatar::render_plush_bot_avatar;
use crate::theme::openbot_palette::OpenBotPalette;
use crate::views::browser::openbot_session::{
    browser_display_url, BrowserTabEntry, BrowserTabOwner,
};

/// Crisp 24x24-viewBox Lucide-proportioned GPUI `canvas` stroke icons for OpenBot Browser chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenBotBrowserIcon {
    ArrowLeft,
    ChevronLeft,
    ChevronRight,
    ChevronDown,
    ChevronUp,
    Reload,
    StopX,
    CloseX,
    Plus,
    Ellipsis,
    Maximize2,
    Monitor,
    Globe2,
    TriangleAlert,
    Clock3,
    Search,
    MousePointer2,
    LoaderSpin,
    ArrowUp,
    StopSquare,
    CircleDot,
    UserRound,
    Ungroup,
    Group,
    Wrench,
    Sparkles,
}

fn stroke_poly(
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

fn fill_poly(
    window: &mut Window,
    origin: Point<Pixels>,
    scale: f32,
    pts: &[(f32, f32)],
    color: Rgba,
) {
    if pts.len() < 3 {
        return;
    }
    let ox = f32::from(origin.x);
    let oy = f32::from(origin.y);
    let mut b = PathBuilder::fill();
    for (idx, (x, y)) in pts.iter().enumerate() {
        let p = point(px(ox + x * scale), px(oy + y * scale));
        if idx == 0 {
            b.move_to(p);
        } else {
            b.line_to(p);
        }
    }
    b.close();
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

fn paint_browser_icon(
    kind: OpenBotBrowserIcon,
    window: &mut Window,
    o: Point<Pixels>,
    s: f32,
    w: f32,
    c: Rgba,
) {
    match kind {
        OpenBotBrowserIcon::ArrowLeft => {
            stroke_poly(window, o, s, w, &[(19.0, 12.0), (5.0, 12.0)], c);
            stroke_poly(window, o, s, w, &[(11.5, 5.5), (5.0, 12.0), (11.5, 18.5)], c);
        }
        OpenBotBrowserIcon::ChevronLeft => {
            stroke_poly(window, o, s, w, &[(15.0, 6.0), (9.0, 12.0), (15.0, 18.0)], c);
        }
        OpenBotBrowserIcon::ChevronRight => {
            stroke_poly(window, o, s, w, &[(9.0, 6.0), (15.0, 12.0), (9.0, 18.0)], c);
        }
        OpenBotBrowserIcon::ChevronDown => {
            stroke_poly(window, o, s, w, &[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)], c);
        }
        OpenBotBrowserIcon::ChevronUp => {
            stroke_poly(window, o, s, w, &[(6.0, 15.0), (12.0, 9.0), (18.0, 15.0)], c);
        }
        OpenBotBrowserIcon::Reload => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[
                    (20.0, 12.0),
                    (19.0, 16.5),
                    (15.5, 19.5),
                    (10.5, 19.8),
                    (6.5, 17.5),
                    (4.5, 13.0),
                    (5.5, 8.0),
                    (9.0, 5.0),
                    (14.0, 4.5),
                    (18.5, 7.0),
                ],
                c,
            );
            stroke_poly(window, o, s, w, &[(18.5, 3.5), (18.8, 7.5), (14.8, 7.8)], c);
        }
        OpenBotBrowserIcon::StopX | OpenBotBrowserIcon::CloseX => {
            stroke_poly(window, o, s, w, &[(6.0, 6.0), (18.0, 18.0)], c);
            stroke_poly(window, o, s, w, &[(18.0, 6.0), (6.0, 18.0)], c);
        }
        OpenBotBrowserIcon::Plus => {
            stroke_poly(window, o, s, w, &[(12.0, 5.0), (12.0, 19.0)], c);
            stroke_poly(window, o, s, w, &[(5.0, 12.0), (19.0, 12.0)], c);
        }
        OpenBotBrowserIcon::Ellipsis => {
            for cx in [5.5_f32, 12.0, 18.5] {
                fill_poly(
                    window,
                    o,
                    s,
                    &[
                        (cx - 1.3, 10.7),
                        (cx + 1.3, 10.7),
                        (cx + 1.3, 13.3),
                        (cx - 1.3, 13.3),
                    ],
                    c,
                );
            }
        }
        OpenBotBrowserIcon::Maximize2 => {
            stroke_poly(window, o, s, w, &[(15.0, 4.0), (20.0, 4.0), (20.0, 9.0)], c);
            stroke_poly(window, o, s, w, &[(20.0, 4.0), (13.5, 10.5)], c);
            stroke_poly(window, o, s, w, &[(9.0, 20.0), (4.0, 20.0), (4.0, 15.0)], c);
            stroke_poly(window, o, s, w, &[(4.0, 20.0), (10.5, 13.5)], c);
        }
        OpenBotBrowserIcon::Monitor => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[(3.0, 4.5), (21.0, 4.5), (21.0, 15.5), (3.0, 15.5), (3.0, 4.5)],
                c,
            );
            stroke_poly(window, o, s, w, &[(8.5, 19.5), (15.5, 19.5)], c);
            stroke_poly(window, o, s, w, &[(12.0, 15.5), (12.0, 19.5)], c);
        }
        OpenBotBrowserIcon::Globe2 => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[
                    (12.0, 3.0),
                    (17.8, 5.2),
                    (21.0, 12.0),
                    (17.8, 18.8),
                    (12.0, 21.0),
                    (6.2, 18.8),
                    (3.0, 12.0),
                    (6.2, 5.2),
                    (12.0, 3.0),
                ],
                c,
            );
            stroke_poly(window, o, s, w, &[(3.2, 12.0), (20.8, 12.0)], c);
            stroke_poly(
                window,
                o,
                s,
                w,
                &[(12.0, 3.0), (15.2, 8.0), (15.2, 16.0), (12.0, 21.0)],
                c,
            );
            stroke_poly(
                window,
                o,
                s,
                w,
                &[(12.0, 3.0), (8.8, 8.0), (8.8, 16.0), (12.0, 21.0)],
                c,
            );
        }
        OpenBotBrowserIcon::TriangleAlert => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[(12.0, 3.5), (21.0, 19.5), (3.0, 19.5), (12.0, 3.5)],
                c,
            );
            stroke_poly(window, o, s, w, &[(12.0, 9.0), (12.0, 14.0)], c);
            fill_poly(
                window,
                o,
                s,
                &[(11.0, 16.0), (13.0, 16.0), (13.0, 17.8), (11.0, 17.8)],
                c,
            );
        }
        OpenBotBrowserIcon::Clock3 => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[
                    (12.0, 3.5),
                    (18.0, 6.0),
                    (20.5, 12.0),
                    (18.0, 18.0),
                    (12.0, 20.5),
                    (6.0, 18.0),
                    (3.5, 12.0),
                    (6.0, 6.0),
                    (12.0, 3.5),
                ],
                c,
            );
            stroke_poly(window, o, s, w, &[(12.0, 7.0), (12.0, 12.0), (16.5, 12.0)], c);
        }
        OpenBotBrowserIcon::Search => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[
                    (11.0, 4.0),
                    (15.5, 5.5),
                    (17.5, 10.5),
                    (15.5, 15.5),
                    (11.0, 17.0),
                    (6.5, 15.5),
                    (4.5, 10.5),
                    (6.5, 5.5),
                    (11.0, 4.0),
                ],
                c,
            );
            stroke_poly(window, o, s, w, &[(16.0, 16.0), (20.5, 20.5)], c);
        }
        OpenBotBrowserIcon::MousePointer2 => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[(5.0, 4.0), (10.5, 19.5), (13.0, 13.5), (19.5, 10.5), (5.0, 4.0)],
                c,
            );
        }
        OpenBotBrowserIcon::LoaderSpin => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[
                    (12.0, 4.0),
                    (17.5, 6.0),
                    (20.0, 12.0),
                    (17.5, 18.0),
                    (12.0, 20.0),
                ],
                c,
            );
        }
        OpenBotBrowserIcon::ArrowUp => {
            stroke_poly(window, o, s, w, &[(12.0, 18.5), (12.0, 5.5)], c);
            stroke_poly(window, o, s, w, &[(6.5, 11.0), (12.0, 5.5), (17.5, 11.0)], c);
        }
        OpenBotBrowserIcon::StopSquare => {
            fill_poly(
                window,
                o,
                s,
                &[(7.0, 7.0), (17.0, 7.0), (17.0, 17.0), (7.0, 17.0)],
                c,
            );
        }
        OpenBotBrowserIcon::CircleDot => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[
                    (12.0, 4.0),
                    (18.0, 6.5),
                    (20.0, 12.0),
                    (18.0, 17.5),
                    (12.0, 20.0),
                    (6.0, 17.5),
                    (4.0, 12.0),
                    (6.0, 6.5),
                    (12.0, 4.0),
                ],
                c,
            );
            fill_poly(
                window,
                o,
                s,
                &[(9.8, 9.8), (14.2, 9.8), (14.2, 14.2), (9.8, 14.2)],
                c,
            );
        }
        OpenBotBrowserIcon::UserRound => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[
                    (12.0, 4.5),
                    (15.2, 6.0),
                    (15.2, 10.0),
                    (12.0, 11.5),
                    (8.8, 10.0),
                    (8.8, 6.0),
                    (12.0, 4.5),
                ],
                c,
            );
            stroke_poly(
                window,
                o,
                s,
                w,
                &[(5.0, 19.5), (7.0, 15.5), (12.0, 14.2), (17.0, 15.5), (19.0, 19.5)],
                c,
            );
        }
        OpenBotBrowserIcon::Ungroup => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[(4.0, 4.0), (13.0, 4.0), (13.0, 12.0), (4.0, 12.0), (4.0, 4.0)],
                c,
            );
            stroke_poly(
                window,
                o,
                s,
                w,
                &[(11.0, 12.0), (20.0, 12.0), (20.0, 20.0), (11.0, 20.0), (11.0, 12.0)],
                c,
            );
        }
        OpenBotBrowserIcon::Group => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[(4.0, 5.0), (20.0, 5.0), (20.0, 19.0), (4.0, 19.0), (4.0, 5.0)],
                c,
            );
            stroke_poly(window, o, s, w, &[(8.0, 9.5), (16.0, 9.5)], c);
            stroke_poly(window, o, s, w, &[(8.0, 14.5), (14.0, 14.5)], c);
        }
        OpenBotBrowserIcon::Wrench => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[(14.5, 5.5), (18.5, 9.5), (9.5, 18.5), (5.5, 14.5), (14.5, 5.5)],
                c,
            );
            stroke_poly(window, o, s, w, &[(16.0, 4.0), (20.0, 8.0)], c);
        }
        OpenBotBrowserIcon::Sparkles => {
            stroke_poly(
                window,
                o,
                s,
                w,
                &[
                    (12.0, 3.0),
                    (14.2, 9.8),
                    (21.0, 12.0),
                    (14.2, 14.2),
                    (12.0, 21.0),
                    (9.8, 14.2),
                    (3.0, 12.0),
                    (9.8, 9.8),
                    (12.0, 3.0),
                ],
                c,
            );
        }
    }
}

/// Renders a crisp vector icon for OpenBot browser chrome.
pub fn openbot_browser_icon(kind: OpenBotBrowserIcon, size_px: f32, color: Rgba) -> AnyElement {
    let stroke_w = (size_px / 11.5).clamp(1.25, 2.0);
    div()
        .w(px(size_px))
        .h(px(size_px))
        .flex_shrink_0()
        .child(
            canvas(
                |bounds, _, _| bounds,
                move |bounds, _, window, _| {
                    let s = f32::from(bounds.size.width).min(f32::from(bounds.size.height)) / 24.0;
                    paint_browser_icon(kind, window, bounds.origin, s, stroke_w, color);
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// Renders a tab owner's 3D felt plush avatar (or fallback `Globe2` icon) at `size_px`.
pub fn render_browser_owner_avatar(
    owner: Option<&BrowserTabOwner>,
    acting: bool,
    size_px: f32,
    ob: &OpenBotPalette,
) -> AnyElement {
    if let Some(agent) = owner {
        let mood = if acting {
            AvatarMood::Working
        } else {
            AvatarMood::Idle
        };
        let avatar = render_plush_bot_avatar(
            &agent.avatar_seed,
            agent.avatar_shape,
            agent.avatar_hue,
            agent.avatar_color.as_deref(),
            size_px,
            mood,
            None,
        );
        if acting {
            div()
                .w(px(size_px + 3.0))
                .h(px(size_px + 3.0))
                .rounded_full()
                .border_1()
                .border_color(ob.accent())
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(avatar)
                .into_any_element()
        } else {
            div()
                .w(px(size_px))
                .h(px(size_px))
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .child(avatar)
                .into_any_element()
        }
    } else {
        openbot_browser_icon(OpenBotBrowserIcon::Globe2, size_px, ob.text_muted())
    }
}

/// Maps a profile hue degree (0..360) to a clean OpenBot profile indicator `Rgba`.
pub fn browser_profile_hue_color(hue: u16, ob: &OpenBotPalette) -> Rgba {
    let h = (hue % 360) as f32;
    let s = 0.72_f32;
    let l = if ob.scheme.is_dark() { 0.62_f32 } else { 0.46_f32 };
    hsl_to_rgba(h, s, l)
}

fn hsl_to_rgba(h: f32, s: f32, l: f32) -> Rgba {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match h as u16 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Rgba {
        r: (r1 + m).clamp(0.0, 1.0),
        g: (g1 + m).clamp(0.0, 1.0),
        b: (b1 + m).clamp(0.0, 1.0),
        a: 1.0,
    }
}

/// Renders the visual body of a `16:10` OpenBot `BrowserPreviewCard` (`image.png` / `image_3.png`),
/// displaying either the captured live `RenderImage` thumbnail or a sleek dark page preview fallback.
pub fn render_browser_preview_thumbnail_content(
    tab: &BrowserTabEntry,
    hovered: bool,
    ob: &OpenBotPalette,
    is_zh: bool,
) -> Div {
    let bg_surface = ob.bg_surface();
    let bg_canvas = ob.bg_canvas();
    let border = ob.border_strong_hairline();
    let text_primary = ob.text_primary();
    let text_muted = ob.text_muted();
    let host = browser_display_url(&tab.url);
    let is_google = host.eq_ignore_ascii_case("google.com") || host.starts_with("google.com/");

    let mut frame = div()
        .relative()
        .w_full()
        .h(px(196.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(border)
        .bg(if ob.scheme.is_dark() {
            rgb(0x202124)
        } else {
            bg_surface
        })
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center();

    if let Some(ref render_img) = tab.preview.image {
        frame = frame.child(
            img(render_img.clone())
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Cover),
        );
    } else if tab.preview.failed {
        frame = frame.child(openbot_browser_icon(
            OpenBotBrowserIcon::Monitor,
            26.0,
            text_muted,
        ));
    } else if is_google {
        // Refined Google dark/light placeholder matching `image.png` before first live snapshot lands
        let google_fg = if ob.scheme.is_dark() {
            rgb(0xe8eaed)
        } else {
            rgb(0x202124)
        };
        let search_bg = if ob.scheme.is_dark() {
            rgb(0x303134)
        } else {
            rgb(0xffffff)
        };
        frame = frame.child(
            div()
                .w_full()
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.0))
                .px(px(28.0))
                .child(
                    div()
                        .text_size(px(24.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(google_fg)
                        .child("Google"),
                )
                .child(
                    div()
                        .w_full()
                        .max_w(px(210.0))
                        .h(px(20.0))
                        .rounded_full()
                        .bg(search_bg)
                        .border_1()
                        .border_color(border)
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(openbot_browser_icon(
                            OpenBotBrowserIcon::Search,
                            9.0,
                            text_muted,
                        ))
                        .child(div().w(px(26.0)).h(px(8.0)).rounded_full().bg(ob.hover())),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .w(px(44.0))
                                .h(px(12.0))
                                .rounded(px(3.0))
                                .bg(search_bg),
                        )
                        .child(
                            div()
                                .w(px(44.0))
                                .h(px(12.0))
                                .rounded(px(3.0))
                                .bg(search_bg),
                        ),
                ),
        );
    } else {
        // Generic sleek webpage skeleton preview before first screenshot capture
        frame = frame.child(
            div()
                .w_full()
                .h_full()
                .p(px(14.0))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .child(openbot_browser_icon(
                    if tab.loading {
                        OpenBotBrowserIcon::LoaderSpin
                    } else {
                        OpenBotBrowserIcon::Globe2
                    },
                    22.0,
                    text_muted,
                ))
                .child(
                    div()
                        .max_w(px(220.0))
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(text_muted)
                        .child(if host.is_empty() {
                            tab.preview_title(is_zh)
                        } else {
                            host
                        }),
                ),
        );
    }

    // Centered hover pill (`[Maximize2] 打开` — exact 1:1 `image_3.png`)
    if hovered {
        frame = frame.child(
            div()
                .absolute()
                .inset_0()
                .bg(Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.22,
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(7.0))
                        .rounded_full()
                        .bg(bg_canvas)
                        .border_1()
                        .border_color(border)
                        .shadow_sm()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(openbot_browser_icon(
                            OpenBotBrowserIcon::Maximize2,
                            12.0,
                            text_primary,
                        ))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_primary)
                                .child(if is_zh { "打开" } else { "Open" }),
                        ),
                ),
        );
    }

    frame
}
