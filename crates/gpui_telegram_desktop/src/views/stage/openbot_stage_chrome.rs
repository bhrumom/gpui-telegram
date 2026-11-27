//! Shared OpenBot `383a35d2` stage & composer vector chrome:
//! - Lucide-proportioned GPUI `canvas` stroke-vector icons (preserves `IconName::ALL.len() == 64`),
//! - Safari/Arc concave bottom `TabShape` fillets for `ConversationHeader`,
//! - `24px` `ReferenceChip` token pills (`reference-chip.css`),
//! - Fabushi -> M1 `render_plush_bot_avatar` normalization helper.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto;
use openbot_contracts::avatar::{AgentStepActivity, AvatarHue, AvatarMood, BloubShape};

use crate::components::avatar::{
    legacy_shape_to_bloub_shape, parse_fabushi_hex_color, render_plush_bot_avatar,
};
use crate::theme::openbot_palette::file_reference_tone;
use crate::theme::TdesktopThemePalette;

/// Crisp stroke-vector icons shared across OpenBot Stage Header, Bubbles, Pill Composer & Stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenBotStageIcon {
    PanelRight,
    Pin,
    ChevronDown,
    ChevronRight,
    ChevronUp,
    Plus,
    Search,
    MoreHorizontal,
    Lock,
    Copy,
    Reply,
    Smile,
    Sparkles,
    Puzzle,
    Brain,
    AlertCircle,
    Check,
    Close,
    Mic,
    ArrowUp,
    StopSquare,
    AudioLines,
    FileText,
    Globe,
    UsersRound,
    SideQuestion,
    Workflow,
    NewChat,
    Pause,
    Play,
    SteerZap,
    Trash,
    Edit,
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

fn paint_stage_icon_part1(
    kind: OpenBotStageIcon,
    window: &mut Window,
    o: Point<Pixels>,
    s: f32,
    w: f32,
    c: Rgba,
) -> bool {
    match kind {
        OpenBotStageIcon::PanelRight => {
            stroke_poly(window, o, s, w, &[(5.0, 3.5), (19.0, 3.5), (20.5, 5.0), (20.5, 19.0), (19.0, 20.5), (5.0, 20.5), (3.5, 19.0), (3.5, 5.0), (5.0, 3.5)], c);
            stroke_poly(window, o, s, w, &[(14.5, 3.5), (14.5, 20.5)], c);
        }
        OpenBotStageIcon::Pin => {
            stroke_poly(window, o, s, w, &[(12.0, 17.0), (12.0, 22.0)], c);
            stroke_poly(window, o, s, w, &[(5.0, 17.0), (19.0, 17.0)], c);
            stroke_poly(window, o, s, w, &[(7.5, 17.0), (9.0, 11.0), (8.0, 5.0), (16.0, 5.0), (15.0, 11.0), (16.5, 17.0)], c);
        }
        OpenBotStageIcon::ChevronDown => {
            stroke_poly(window, o, s, w, &[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)], c);
        }
        OpenBotStageIcon::ChevronRight => {
            stroke_poly(window, o, s, w, &[(9.0, 6.0), (15.0, 12.0), (9.0, 18.0)], c);
        }
        OpenBotStageIcon::ChevronUp => {
            stroke_poly(window, o, s, w, &[(6.0, 15.0), (12.0, 9.0), (18.0, 15.0)], c);
        }
        OpenBotStageIcon::Plus => {
            stroke_poly(window, o, s, w, &[(12.0, 5.0), (12.0, 19.0)], c);
            stroke_poly(window, o, s, w, &[(5.0, 12.0), (19.0, 12.0)], c);
        }
        OpenBotStageIcon::Search => {
            stroke_poly(window, o, s, w, &[(11.0, 4.0), (15.5, 5.5), (17.5, 10.5), (15.5, 15.5), (11.0, 17.0), (6.5, 15.5), (4.5, 10.5), (6.5, 5.5), (11.0, 4.0)], c);
            stroke_poly(window, o, s, w, &[(16.0, 16.0), (20.5, 20.5)], c);
        }
        OpenBotStageIcon::MoreHorizontal => {
            for cx in [5.5_f32, 12.0, 18.5] {
                fill_poly(window, o, s, &[(cx - 1.3, 10.7), (cx + 1.3, 10.7), (cx + 1.3, 13.3), (cx - 1.3, 13.3)], c);
            }
        }
        OpenBotStageIcon::Lock => {
            stroke_poly(window, o, s, w, &[(6.0, 11.0), (18.0, 11.0), (19.0, 12.0), (19.0, 19.5), (18.0, 20.5), (6.0, 20.5), (5.0, 19.5), (5.0, 12.0), (6.0, 11.0)], c);
            stroke_poly(window, o, s, w, &[(8.0, 11.0), (8.0, 7.5), (9.5, 5.0), (12.0, 4.2), (14.5, 5.0), (16.0, 7.5), (16.0, 11.0)], c);
        }
        OpenBotStageIcon::Copy => {
            stroke_poly(window, o, s, w, &[(9.0, 9.0), (19.5, 9.0), (19.5, 19.5), (9.0, 19.5), (9.0, 9.0)], c);
            stroke_poly(window, o, s, w, &[(6.0, 15.0), (4.5, 15.0), (4.5, 4.5), (15.0, 4.5), (15.0, 6.0)], c);
        }
        OpenBotStageIcon::Reply => {
            stroke_poly(window, o, s, w, &[(9.0, 7.0), (4.0, 12.0), (9.0, 17.0)], c);
            stroke_poly(window, o, s, w, &[(4.5, 12.0), (15.5, 12.0), (18.5, 13.5), (20.0, 16.5), (20.0, 19.0)], c);
        }
        OpenBotStageIcon::Smile => {
            stroke_poly(window, o, s, w, &[(12.0, 3.0), (17.5, 5.0), (20.5, 10.0), (20.5, 14.0), (17.5, 19.0), (12.0, 21.0), (6.5, 19.0), (3.5, 14.0), (3.5, 10.0), (6.5, 5.0), (12.0, 3.0)], c);
            stroke_poly(window, o, s, w, &[(8.5, 14.5), (10.5, 16.5), (13.5, 16.5), (15.5, 14.5)], c);
            fill_poly(window, o, s, &[(8.2, 8.8), (9.8, 8.8), (9.8, 10.4), (8.2, 10.4)], c);
            fill_poly(window, o, s, &[(14.2, 8.8), (15.8, 8.8), (15.8, 10.4), (14.2, 10.4)], c);
        }
        OpenBotStageIcon::Sparkles => {
            stroke_poly(window, o, s, w, &[(12.0, 3.0), (14.2, 9.8), (21.0, 12.0), (14.2, 14.2), (12.0, 21.0), (9.8, 14.2), (3.0, 12.0), (9.8, 9.8), (12.0, 3.0)], c);
        }
        OpenBotStageIcon::Puzzle => {
            stroke_poly(window, o, s, w, &[(5.0, 9.0), (9.5, 9.0), (10.5, 6.0), (13.5, 6.0), (14.5, 9.0), (19.0, 9.0), (19.0, 19.0), (5.0, 19.0), (5.0, 9.0)], c);
        }
        OpenBotStageIcon::Brain => {
            stroke_poly(window, o, s, w, &[(12.0, 5.0), (12.0, 19.5)], c);
            stroke_poly(window, o, s, w, &[(12.0, 6.0), (8.0, 5.5), (5.0, 8.5), (5.5, 12.0), (5.0, 15.5), (8.0, 18.5), (12.0, 18.0)], c);
            stroke_poly(window, o, s, w, &[(12.0, 6.0), (16.0, 5.5), (19.0, 8.5), (18.5, 12.0), (19.0, 15.5), (16.0, 18.5), (12.0, 18.0)], c);
        }
        OpenBotStageIcon::AlertCircle => {
            stroke_poly(window, o, s, w, &[(12.0, 3.0), (18.0, 5.5), (21.0, 12.0), (18.0, 18.5), (12.0, 21.0), (6.0, 18.5), (3.0, 12.0), (6.0, 5.5), (12.0, 3.0)], c);
            stroke_poly(window, o, s, w, &[(12.0, 7.5), (12.0, 13.0)], c);
            fill_poly(window, o, s, &[(11.0, 15.5), (13.0, 15.5), (13.0, 17.5), (11.0, 17.5)], c);
        }
        _ => return false,
    }
    true
}

fn paint_stage_icon_part2(
    kind: OpenBotStageIcon,
    window: &mut Window,
    o: Point<Pixels>,
    s: f32,
    w: f32,
    c: Rgba,
) {
    match kind {
        OpenBotStageIcon::Check => {
            stroke_poly(window, o, s, w, &[(4.5, 12.5), (9.5, 17.5), (19.5, 6.5)], c);
        }
        OpenBotStageIcon::Close => {
            stroke_poly(window, o, s, w, &[(6.0, 6.0), (18.0, 18.0)], c);
            stroke_poly(window, o, s, w, &[(18.0, 6.0), (6.0, 18.0)], c);
        }
        OpenBotStageIcon::Mic => {
            stroke_poly(window, o, s, w, &[(9.5, 4.0), (14.5, 4.0), (15.0, 12.0), (13.5, 14.0), (10.5, 14.0), (9.0, 12.0), (9.5, 4.0)], c);
            stroke_poly(window, o, s, w, &[(6.0, 11.5), (6.5, 15.5), (9.5, 17.5), (14.5, 17.5), (17.5, 15.5), (18.0, 11.5)], c);
            stroke_poly(window, o, s, w, &[(12.0, 17.5), (12.0, 21.0)], c);
            stroke_poly(window, o, s, w, &[(9.0, 21.0), (15.0, 21.0)], c);
        }
        OpenBotStageIcon::ArrowUp => {
            stroke_poly(window, o, s, w, &[(12.0, 19.0), (12.0, 5.0)], c);
            stroke_poly(window, o, s, w, &[(6.0, 11.0), (12.0, 5.0), (18.0, 11.0)], c);
        }
        OpenBotStageIcon::StopSquare => {
            fill_poly(window, o, s, &[(6.5, 6.5), (17.5, 6.5), (17.5, 17.5), (6.5, 17.5)], c);
        }
        OpenBotStageIcon::AudioLines => {
            stroke_poly(window, o, s, w, &[(4.0, 10.0), (4.0, 14.0)], c);
            stroke_poly(window, o, s, w, &[(8.0, 6.5), (8.0, 17.5)], c);
            stroke_poly(window, o, s, w, &[(12.0, 4.0), (12.0, 20.0)], c);
            stroke_poly(window, o, s, w, &[(16.0, 7.5), (16.0, 16.5)], c);
            stroke_poly(window, o, s, w, &[(20.0, 10.0), (20.0, 14.0)], c);
        }
        OpenBotStageIcon::FileText => {
            stroke_poly(window, o, s, w, &[(6.0, 3.5), (14.0, 3.5), (18.5, 8.0), (18.5, 20.5), (6.0, 20.5), (6.0, 3.5)], c);
            stroke_poly(window, o, s, w, &[(14.0, 3.5), (14.0, 8.0), (18.5, 8.0)], c);
            stroke_poly(window, o, s, w, &[(9.0, 12.5), (15.5, 12.5)], c);
            stroke_poly(window, o, s, w, &[(9.0, 16.0), (15.5, 16.0)], c);
        }
        OpenBotStageIcon::Globe => {
            stroke_poly(window, o, s, w, &[(12.0, 3.0), (18.0, 5.5), (21.0, 12.0), (18.0, 18.5), (12.0, 21.0), (6.0, 18.5), (3.0, 12.0), (6.0, 5.5), (12.0, 3.0)], c);
            stroke_poly(window, o, s, w, &[(3.2, 12.0), (20.8, 12.0)], c);
            stroke_poly(window, o, s, w, &[(12.0, 3.0), (9.0, 12.0), (12.0, 21.0), (15.0, 12.0), (12.0, 3.0)], c);
        }
        OpenBotStageIcon::UsersRound => {
            stroke_poly(window, o, s, w, &[(10.0, 5.0), (13.0, 6.5), (13.0, 10.0), (10.0, 11.5), (7.0, 10.0), (7.0, 6.5), (10.0, 5.0)], c);
            stroke_poly(window, o, s, w, &[(4.0, 19.5), (5.5, 15.5), (10.0, 14.5), (14.5, 15.5), (16.0, 19.5)], c);
            stroke_poly(window, o, s, w, &[(15.5, 6.0), (18.0, 7.5), (18.0, 10.5), (15.5, 11.8)], c);
            stroke_poly(window, o, s, w, &[(17.0, 15.0), (19.5, 16.2), (20.5, 19.5)], c);
        }
        OpenBotStageIcon::SideQuestion => {
            stroke_poly(window, o, s, w, &[(12.0, 3.5), (18.0, 6.0), (20.5, 12.0), (18.0, 18.0), (12.0, 20.0), (7.5, 19.5), (4.0, 20.5), (5.0, 16.5), (3.5, 12.0), (6.0, 6.0), (12.0, 3.5)], c);
            stroke_poly(window, o, s, w, &[(9.5, 9.5), (10.5, 7.8), (13.2, 7.8), (14.2, 9.5), (12.0, 11.5), (12.0, 13.5)], c);
            fill_poly(window, o, s, &[(11.1, 15.4), (12.9, 15.4), (12.9, 17.2), (11.1, 17.2)], c);
        }
        OpenBotStageIcon::Workflow => {
            stroke_poly(window, o, s, w, &[(4.0, 4.0), (10.0, 4.0), (10.0, 10.0), (4.0, 10.0), (4.0, 4.0)], c);
            stroke_poly(window, o, s, w, &[(14.0, 14.0), (20.0, 14.0), (20.0, 20.0), (14.0, 20.0), (14.0, 14.0)], c);
            stroke_poly(window, o, s, w, &[(7.0, 10.0), (7.0, 17.0), (14.0, 17.0)], c);
        }
        OpenBotStageIcon::NewChat => {
            stroke_poly(window, o, s, w, &[(11.0, 5.0), (5.0, 5.0), (4.0, 6.0), (4.0, 19.0), (5.0, 20.0), (18.0, 20.0), (19.0, 19.0), (19.0, 13.0)], c);
            stroke_poly(window, o, s, w, &[(10.0, 14.0), (18.5, 5.5), (20.0, 7.0), (11.5, 15.5), (9.5, 16.0), (10.0, 14.0)], c);
        }
        OpenBotStageIcon::Pause => {
            fill_poly(window, o, s, &[(6.0, 5.0), (10.0, 5.0), (10.0, 19.0), (6.0, 19.0)], c);
            fill_poly(window, o, s, &[(14.0, 5.0), (18.0, 5.0), (18.0, 19.0), (14.0, 19.0)], c);
        }
        OpenBotStageIcon::Play => {
            fill_poly(window, o, s, &[(7.0, 5.0), (19.0, 12.0), (7.0, 19.0)], c);
        }
        OpenBotStageIcon::SteerZap => {
            stroke_poly(window, o, s, w, &[(13.0, 3.0), (5.0, 13.5), (11.5, 13.5), (10.5, 21.0), (19.0, 10.5), (12.5, 10.5), (13.0, 3.0)], c);
        }
        OpenBotStageIcon::Trash => {
            stroke_poly(window, o, s, w, &[(4.5, 6.5), (19.5, 6.5)], c);
            stroke_poly(window, o, s, w, &[(9.0, 6.5), (9.5, 4.0), (14.5, 4.0), (15.0, 6.5)], c);
            stroke_poly(window, o, s, w, &[(6.5, 6.5), (7.5, 20.0), (16.5, 20.0), (17.5, 6.5)], c);
        }
        OpenBotStageIcon::Edit => {
            stroke_poly(window, o, s, w, &[(4.5, 19.5), (6.0, 14.5), (16.5, 4.0), (20.0, 7.5), (9.5, 18.0), (4.5, 19.5)], c);
        }
        _ => {}
    }
}

/// Paints a crisp `24x24`-viewBox OpenBot stage/composer stroke icon via GPUI `canvas`.
pub fn openbot_stage_icon(kind: OpenBotStageIcon, size_px: f32, color: Rgba) -> AnyElement {
    let stroke_px = (size_px / 16.0 * 1.65).clamp(1.2, 2.1);
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
                    let o = bounds.origin;
                    if !paint_stage_icon_part1(kind, window, o, scale, stroke_px, color) {
                        paint_stage_icon_part2(kind, window, o, scale, stroke_px, color);
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// Paints Safari/Arc-style concave bottom fillet joining an active `ConversationHeader`
/// `TabShape` (`#1a1a1a` dark / `#f5f5f6` light) into `#app-shell-workspace-card`.
pub fn render_tab_concave_fillet(
    is_left: bool,
    radius_px: f32,
    fill: Rgba,
    border: Rgba,
) -> AnyElement {
    div()
        .w(px(radius_px))
        .h(px(radius_px))
        .flex_shrink_0()
        .child(
            canvas(
                |bounds, _, _| bounds,
                move |bounds, _, window, _| {
                    let x0 = f32::from(bounds.origin.x);
                    let y0 = f32::from(bounds.origin.y);
                    let w = f32::from(bounds.size.width).max(1.0);
                    let h = f32::from(bounds.size.height).max(1.0);
                    let k = 0.5522848_f32;
                    let mut fill_b = PathBuilder::fill();
                    let mut stroke_b = PathBuilder::stroke(px(1.0));
                    if is_left {
                        // Bottom-right corner of scoop fills tab color; curves upward to right edge.
                        fill_b.move_to(point(px(x0), px(y0 + h)));
                        fill_b.line_to(point(px(x0 + w), px(y0 + h)));
                        fill_b.line_to(point(px(x0 + w), px(y0)));
                        fill_b.cubic_bezier_to(
                            point(px(x0), px(y0 + h)),
                            point(px(x0 + w), px(y0 + h * k)),
                            point(px(x0 + w * k), px(y0 + h)),
                        );
                        fill_b.close();

                        stroke_b.move_to(point(px(x0 + w), px(y0)));
                        stroke_b.cubic_bezier_to(
                            point(px(x0), px(y0 + h)),
                            point(px(x0 + w), px(y0 + h * k)),
                            point(px(x0 + w * k), px(y0 + h)),
                        );
                    } else {
                        fill_b.move_to(point(px(x0), px(y0)));
                        fill_b.line_to(point(px(x0), px(y0 + h)));
                        fill_b.line_to(point(px(x0 + w), px(y0 + h)));
                        fill_b.cubic_bezier_to(
                            point(px(x0), px(y0)),
                            point(px(x0 + w * (1.0 - k)), px(y0 + h)),
                            point(px(x0), px(y0 + h * k)),
                        );
                        fill_b.close();

                        stroke_b.move_to(point(px(x0), px(y0)));
                        stroke_b.cubic_bezier_to(
                            point(px(x0 + w), px(y0 + h)),
                            point(px(x0), px(y0 + h * k)),
                            point(px(x0 + w * (1.0 - k)), px(y0 + h)),
                        );
                    }
                    if let Ok(path) = fill_b.build() {
                        window.paint_path(path, fill);
                    }
                    if let Ok(path) = stroke_b.build() {
                        window.paint_path(path, border);
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// Normalizes Fabushi `avatar_shape` and `avatar_color` strings into M1 `BloubShape`,
/// `AvatarHue`, and lowercase `#rrggbb` color string.
pub fn normalize_fabushi_plush_spec(
    raw_shape: &str,
    raw_color: &str,
) -> (Option<BloubShape>, Option<AvatarHue>, Option<String>) {
    let trimmed_shape = raw_shape.trim();
    let stripped = trimmed_shape
        .strip_prefix("plush:")
        .or_else(|| trimmed_shape.strip_prefix("bloub:"))
        .unwrap_or(trimmed_shape);
    let shape = BloubShape::parse(stripped)
        .or_else(|| legacy_shape_to_bloub_shape(stripped));

    let trimmed_color = raw_color.trim();
    let mut hue = None;
    let mut color_hex = None;
    if trimmed_color.starts_with('#') {
        let lower = trimmed_color.to_ascii_lowercase();
        if openbot_contracts::avatar::is_avatar_color(&lower) {
            color_hex = Some(lower);
        } else if let Some(rgb_u32) = parse_fabushi_hex_color(&lower) {
            let formatted = format!("#{:06x}", rgb_u32 & 0xFF_FFFF);
            if openbot_contracts::avatar::is_avatar_color(&formatted) {
                color_hex = Some(formatted);
            }
        }
    } else if !trimmed_color.is_empty() {
        let lower = trimmed_color.to_ascii_lowercase();
        hue = match lower.as_str() {
            "red" | "coral" | "crimson" => Some(AvatarHue::Red),
            "orange" | "amber" | "peach" => Some(AvatarHue::Orange),
            "yellow" | "gold" => Some(AvatarHue::Yellow),
            "lime" => Some(AvatarHue::Lime),
            "green" | "emerald" | "mint" => Some(AvatarHue::Green),
            "cyan" | "teal" | "aqua" => Some(AvatarHue::Cyan),
            "blue" | "sky" | "azure" => Some(AvatarHue::Blue),
            "indigo" => Some(AvatarHue::Indigo),
            "violet" | "purple" | "lavender" => Some(AvatarHue::Violet),
            "magenta" | "pink" | "rose" => Some(AvatarHue::Magenta),
            _ => None,
        };
        if hue.is_none() {
            if let Some(rgb_u32) = parse_fabushi_hex_color(&lower) {
                let formatted = format!("#{:06x}", rgb_u32 & 0xFF_FFFF);
                if openbot_contracts::avatar::is_avatar_color(&formatted) {
                    color_hex = Some(formatted);
                }
            }
        }
    }
    (shape, hue, color_hex)
}

/// Renders a Fabushi/OpenBot assistant using M1's 3D tactile felt `render_plush_bot_avatar`.
pub fn render_fabushi_plush_avatar(
    fab: Option<&FabushiBotSummaryDto>,
    fallback_seed: &str,
    size_px: f32,
    mood: AvatarMood,
    activity: Option<AgentStepActivity>,
) -> Div {
    if let Some(f) = fab {
        let (shape, hue, color) = normalize_fabushi_plush_spec(&f.avatar_shape, &f.avatar_color);
        let seed = if f.agent_id.trim().is_empty() {
            fallback_seed
        } else {
            f.agent_id.as_str()
        };
        render_plush_bot_avatar(seed, shape, hue, color.as_deref(), size_px, mood, activity)
    } else {
        render_plush_bot_avatar(fallback_seed, None, None, None, size_px, mood, activity)
    }
}

/// Maps `bot_activity_state` string to M1 `AvatarMood` & optional `AgentStepActivity`.
pub fn activity_str_to_openbot_mood(
    state: &str,
) -> (AvatarMood, Option<AgentStepActivity>) {
    match state.trim().to_ascii_lowercase().as_str() {
        "" | "idle" => (AvatarMood::Idle, None),
        "tool" | "searching" | "search" => (AvatarMood::Working, Some(AgentStepActivity::Searching)),
        "writing" | "editing" => (AvatarMood::Working, Some(AgentStepActivity::Editing)),
        "thinking" | "working" | "running" => {
            (AvatarMood::Working, Some(AgentStepActivity::Working))
        }
        "waiting" | "approval" => (AvatarMood::Waiting, None),
        "failed" | "error" => (AvatarMood::Failed, None),
        "done" | "responded" => (AvatarMood::Responded, None),
        _ => (AvatarMood::Working, Some(AgentStepActivity::Working)),
    }
}

/// Builds `(Vec<ChannelMember>, Vec<ChannelAgentBrief>)` for M1 `render_channel_avatar`
/// from either rich briefs or legacy `group_member_tiles` (`(initials, color_rgb)`).
pub fn build_channel_avatar_inputs(
    chat_id: u64,
    group_member_tiles: Option<&[(String, u32)]>,
    rich_briefs: &[crate::components::avatar::ChannelAgentBrief],
) -> (
    Vec<openbot_contracts::channels::ChannelMember>,
    Vec<crate::components::avatar::ChannelAgentBrief>,
) {
    use openbot_contracts::channels::ChannelMember;
    if !rich_briefs.is_empty() {
        let members = rich_briefs
            .iter()
            .map(|b| ChannelMember {
                agent_id: b.id.clone(),
            })
            .collect();
        return (members, rich_briefs.to_vec());
    }
    let Some(tiles) = group_member_tiles.filter(|t| !t.is_empty()) else {
        return (Vec::new(), Vec::new());
    };
    let mut members = Vec::with_capacity(tiles.len());
    let mut agents = Vec::with_capacity(tiles.len());
    for (idx, (initials, rgb_val)) in tiles.iter().enumerate() {
        let id = format!("ch_{chat_id}_m_{idx}_{initials}");
        let hex = format!("#{:06x}", rgb_val & 0xFF_FFFF);
        let color = openbot_contracts::avatar::is_avatar_color(&hex).then_some(hex);
        members.push(ChannelMember {
            agent_id: id.clone(),
        });
        agents.push(crate::components::avatar::ChannelAgentBrief {
            id,
            avatar_seed: Some(format!("{initials}_{chat_id}_{idx}")),
            avatar_hue: None,
            avatar_color: color,
            avatar_url: None,
        });
    }
    (members, agents)
}

/// Visual tone for a `24px` OpenBot `ReferenceChip` token (`reference-chip.css`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceChipTone {
    Default,
    SkillOrMcp,
    File,
}

/// Renders an OpenBot `24px` `ReferenceChip` pill (`h: 24px`, `rounded(6px)`, `px: 6px`, `12px` medium).
pub fn render_openbot_reference_chip(
    id: impl Into<ElementId>,
    kind: &str,
    label: &str,
    palette: &TdesktopThemePalette,
) -> Stateful<Div> {
    let ob = palette.openbot_palette();
    let is_dark = palette.mode.is_dark();
    let kind_lc = kind.trim().to_ascii_lowercase();
    let tone = match kind_lc.as_str() {
        "skill" | "mcp" | "workflow" | "routine" | "command" => ReferenceChipTone::SkillOrMcp,
        "file" | "path" | "quote" => ReferenceChipTone::File,
        _ => ReferenceChipTone::Default,
    };

    let (chip_bg, chip_fg, icon_kind) = match tone {
        ReferenceChipTone::SkillOrMcp => (
            ob.accent_soft(),
            ob.accent_text(),
            if kind_lc == "workflow" {
                OpenBotStageIcon::Workflow
            } else {
                OpenBotStageIcon::Puzzle
            },
        ),
        ReferenceChipTone::File => {
            let (fg_str, bg_str) = file_reference_tone(label).colors(ob.scheme);
            let bg = crate::theme::openbot_palette::parse_css_color_to_rgba(bg_str)
                .unwrap_or_else(|| ob.bg_control_active());
            let fg = crate::theme::openbot_palette::parse_css_color_to_rgba(fg_str)
                .unwrap_or_else(|| ob.text_primary());
            (bg, fg, OpenBotStageIcon::FileText)
        }
        ReferenceChipTone::Default => {
            let bg: Rgba = if is_dark {
                rgb(0x2A2A2C).into()
            } else {
                rgb(0xE8E8EB).into()
            };
            let icon = match kind_lc.as_str() {
                "everyone" => OpenBotStageIcon::UsersRound,
                "tab" | "url" => OpenBotStageIcon::Globe,
                _ => OpenBotStageIcon::Sparkles,
            };
            (bg, ob.text_primary(), icon)
        }
    };
    let hover_bg = ob.bg_control_active();
    let show_mini_plush = kind_lc == "agent" || kind_lc == "bot";

    div()
        .id(id)
        .h(px(24.0))
        .max_w(px(220.0))
        .px(px(6.0))
        .rounded(px(6.0))
        .bg(chip_bg)
        .hover(move |s| s.bg(hover_bg))
        .cursor_pointer()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.0))
        .when(show_mini_plush, |el| {
            el.child(render_plush_bot_avatar(
                label,
                None,
                None,
                None,
                16.0,
                AvatarMood::Idle,
                None,
            ))
        })
        .when(!show_mini_plush, |el| {
            el.child(openbot_stage_icon(icon_kind, 13.0, chip_fg))
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(12.0))
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(chip_fg)
                .child(label.to_string()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbot_contracts::avatar::{AvatarHue, AvatarMood, BloubShape};

    #[std::prelude::v1::test]
    fn normalizes_fabushi_plush_specs_and_activity_moods() {
        let (shape, hue, hex) = normalize_fabushi_plush_spec("plush:cercle", "#3f7ee8");
        assert_eq!(shape, Some(BloubShape::Cercle));
        assert_eq!(hue, None);
        assert_eq!(hex.as_deref(), Some("#3f7ee8"));

        let (shape2, named_hue, _) = normalize_fabushi_plush_spec("squircle", "violet");
        assert_eq!(shape2, Some(BloubShape::Squircle));
        assert_eq!(named_hue, Some(AvatarHue::Violet));

        let (mood, act) = activity_str_to_openbot_mood("tool");
        assert_eq!(mood, AvatarMood::Working);
        assert!(act.is_some());

        let tiles = vec![("研".to_string(), 0x3F7EE8), ("设".to_string(), 0x26C473)];
        let (members, agents) = build_channel_avatar_inputs(7, Some(&tiles), &[]);
        assert_eq!(members.len(), 2);
        assert_eq!(agents.len(), 2);
    }
}
