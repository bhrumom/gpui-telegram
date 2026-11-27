//! Apple Messages macOS & Telegram Desktop `.tdesktop-theme` color specifications
//! for `Night`, `Tinted`, `Day`, and `Classic`.

use super::palette_schemes::{
    TdesktopThemeMode, PEER_AVATAR_BOTTOM, PEER_AVATAR_TOP, PEER_NAME_COLORS_DAY,
    PEER_NAME_COLORS_NIGHT, PEER_NAME_COLORS_TINTED, adjust_rgb, mix_rgb,
};
use super::tdesktop_palette::TdesktopThemePalette;

struct ThemeSpec {
    window_bg: u32,
    rail_bg: u32,
    dialogs_bg: u32,
    dialogs_hover_bg: u32,
    dialogs_active_bg: u32,
    stage_bg: u32,
    topbar_bg: u32,
    composer_bg: u32,
    input_bg: u32,
    card_bg: u32,
    modal_bg: u32,
    tooltip_bg: u32,
    tooltip_fg: u32,
    menu_bg: u32,
    menu_hover_bg: u32,
    scrollbar_bg: u32,
    scrollbar_hover_bg: u32,
    msg_in_bg: u32,
    msg_out_bg: u32,
    msg_out_bg_top: u32,
    msg_out_bg_bottom: u32,
    msg_out_fg: u32,
    msg_out_secondary_fg: u32,
    msg_out_link_fg: u32,
    msg_in_selected_bg: u32,
    msg_out_selected_bg: u32,
    msg_in_date_fg: u32,
    msg_out_date_fg: u32,
    code_bg: u32,
    code_fg: u32,
    service_pill_bg: u32,
    service_pill_fg: u32,
    muted_badge_bg: u32,
    online_dot_fg: u32,
    check_read_fg: u32,
    check_sent_fg: u32,
    icon_fg: u32,
    border_fg: u32,
    text_primary: u32,
    text_secondary: u32,
    text_link: u32,
    accent_hover: u32,
    reaction_bg: u32,
    danger: u32,
    success: u32,
    wallpaper_top: u32,
    wallpaper_bottom: u32,
    wallpaper_pattern: u32,
    peer_name_colors: [u32; 7],
}

const NIGHT_SPEC: ThemeSpec = ThemeSpec {
    // Grok Bot `sand-dark` 4-step elevation ladder:
    // Sidebar `#0e0e0e` -> Chrome `#121212` -> Stage/Pane `#161616` -> Elevated `#1f1f1f` (hover `#2c2c2c`)
    window_bg: 0x121212,
    rail_bg: 0x0E0E0E,
    dialogs_bg: 0x0E0E0E,
    dialogs_hover_bg: 0x2C2C2C,
    dialogs_active_bg: 0x0A84FF,
    stage_bg: 0x161616,
    topbar_bg: 0x121212,
    composer_bg: 0x161616,
    input_bg: 0x1F1F1F,
    card_bg: 0x1F1F1F,
    modal_bg: 0x1F1F1F,
    tooltip_bg: 0x1F1F1F,
    tooltip_fg: 0xF5F5F5,
    menu_bg: 0x1F1F1F,
    menu_hover_bg: 0x2C2C2C,
    scrollbar_bg: 0x525256,
    scrollbar_hover_bg: 0x76767C,
    msg_in_bg: 0x1F1F1F,
    msg_out_bg: 0x116DD2,
    msg_out_bg_top: 0x1884FC,
    msg_out_bg_bottom: 0x116DD2,
    msg_out_fg: 0xFFFFFF,
    msg_out_secondary_fg: 0xD6EBFF,
    msg_out_link_fg: 0xFFFFFF,
    msg_in_selected_bg: 0x2C2C2C,
    msg_out_selected_bg: 0x0D59AD,
    msg_in_date_fg: 0x8A8A8E,
    msg_out_date_fg: 0xD6EBFF,
    code_bg: 0x242424,
    code_fg: 0xE8E8EC,
    service_pill_bg: 0x2C2C2C,
    service_pill_fg: 0x9E9EA4,
    muted_badge_bg: 0x48484A,
    online_dot_fg: 0x00BA69,
    check_read_fg: 0x4D9BFF,
    check_sent_fg: 0x8A8A8E,
    icon_fg: 0x9E9EA4,
    border_fg: 0x2C2C2C,
    text_primary: 0xF5F5F5,
    text_secondary: 0xA3A3A8,
    text_link: 0x4D9BFF,
    accent_hover: 0x1884FC,
    reaction_bg: 0x2C2C2C,
    danger: 0xFF6463,
    success: 0x00BA69,
    wallpaper_top: 0x161616,
    wallpaper_bottom: 0x161616,
    wallpaper_pattern: 0x1F1F1F,
    peer_name_colors: PEER_NAME_COLORS_NIGHT,
};

const TINTED_SPEC: ThemeSpec = ThemeSpec {
    window_bg: 0x181C22,
    rail_bg: 0x181C22,
    dialogs_bg: 0x181C22,
    dialogs_hover_bg: 0x242A33,
    dialogs_active_bg: 0x0A84FF,
    stage_bg: 0x181C22,
    topbar_bg: 0x181C22,
    composer_bg: 0x181C22,
    input_bg: 0x242A33,
    card_bg: 0x222831,
    modal_bg: 0x1E232B,
    tooltip_bg: 0x262D37,
    tooltip_fg: 0xF5F7FA,
    menu_bg: 0x212730,
    menu_hover_bg: 0x2F3744,
    scrollbar_bg: 0x5A6370,
    scrollbar_hover_bg: 0x7A8594,
    msg_in_bg: 0x242A33,
    msg_out_bg: 0x0A84FF,
    msg_out_bg_top: 0x1E8FFF,
    msg_out_bg_bottom: 0x0A84FF,
    msg_out_fg: 0xFFFFFF,
    msg_out_secondary_fg: 0xD6EBFF,
    msg_out_link_fg: 0xFFFFFF,
    msg_in_selected_bg: 0x313946,
    msg_out_selected_bg: 0x0070E0,
    msg_in_date_fg: 0x8E96A0,
    msg_out_date_fg: 0xD6EBFF,
    code_bg: 0x242A33,
    code_fg: 0xE5EAF0,
    service_pill_bg: 0x242A33,
    service_pill_fg: 0x8E96A0,
    muted_badge_bg: 0x444D5A,
    online_dot_fg: 0x30D158,
    check_read_fg: 0x0A84FF,
    check_sent_fg: 0x8E96A0,
    icon_fg: 0x949DA8,
    border_fg: 0x303844,
    text_primary: 0xF5F7FA,
    text_secondary: 0x949DA8,
    text_link: 0x409CFF,
    accent_hover: 0x2994FF,
    reaction_bg: 0x242A33,
    danger: 0xFF453A,
    success: 0x30D158,
    wallpaper_top: 0x181C22,
    wallpaper_bottom: 0x181C22,
    wallpaper_pattern: 0x242A33,
    peer_name_colors: PEER_NAME_COLORS_TINTED,
};

const DAY_SPEC: ThemeSpec = ThemeSpec {
    // Grok Bot `sand-light` 4-step elevation ladder:
    // Sidebar `#f5f5f5` -> Chrome `#fcfcfc` -> Pane `#ffffff` -> Elevated `#ffffff` (hover `#f1f1f1`)
    window_bg: 0xFCFCFC,
    rail_bg: 0xF5F5F5,
    dialogs_bg: 0xF5F5F5,
    dialogs_hover_bg: 0xF1F1F1,
    dialogs_active_bg: 0x1680F3,
    stage_bg: 0xFFFFFF,
    topbar_bg: 0xFCFCFC,
    composer_bg: 0xFFFFFF,
    input_bg: 0xF1F1F1,
    card_bg: 0xFFFFFF,
    modal_bg: 0xFFFFFF,
    tooltip_bg: 0x1C1C1C,
    tooltip_fg: 0xFFFFFF,
    menu_bg: 0xFFFFFF,
    menu_hover_bg: 0xF1F1F1,
    scrollbar_bg: 0xB0B0B5,
    scrollbar_hover_bg: 0x86868B,
    msg_in_bg: 0xF1F1F1,
    msg_out_bg: 0x126FD4,
    msg_out_bg_top: 0x1E86FC,
    msg_out_bg_bottom: 0x126FD4,
    msg_out_fg: 0xFFFFFF,
    msg_out_secondary_fg: 0xE5F2FF,
    msg_out_link_fg: 0xFFFFFF,
    msg_in_selected_bg: 0xE4E4E7,
    msg_out_selected_bg: 0x0B5DB8,
    msg_in_date_fg: 0x7A7A80,
    msg_out_date_fg: 0xE5F2FF,
    code_bg: 0xF1F1F4,
    code_fg: 0x1C1C1C,
    service_pill_bg: 0xF1F1F1,
    service_pill_fg: 0x7A7A80,
    muted_badge_bg: 0xC7C7CC,
    online_dot_fg: 0x10AB61,
    check_read_fg: 0x0B6DD3,
    check_sent_fg: 0x7A7A80,
    icon_fg: 0x6E6E73,
    border_fg: 0xE5E5E7,
    text_primary: 0x1C1C1C,
    text_secondary: 0x636366,
    text_link: 0x0B6DD3,
    accent_hover: 0x126FD4,
    reaction_bg: 0xF1F1F1,
    danger: 0xE72E3B,
    success: 0x10AB61,
    wallpaper_top: 0xFFFFFF,
    wallpaper_bottom: 0xFFFFFF,
    wallpaper_pattern: 0xEAEAEF,
    peer_name_colors: PEER_NAME_COLORS_DAY,
};

const CLASSIC_SPEC: ThemeSpec = ThemeSpec {
    window_bg: 0xFFFFFF,
    rail_bg: 0xEDEDED,
    dialogs_bg: 0xFFFFFF,
    dialogs_hover_bg: 0xF1F1F1,
    dialogs_active_bg: 0x419FD9,
    stage_bg: 0xD5E8C8,
    topbar_bg: 0xFFFFFF,
    composer_bg: 0xFFFFFF,
    input_bg: 0xF1F1F1,
    card_bg: 0xF6F8F5,
    modal_bg: 0xFFFFFF,
    tooltip_bg: 0xEEF2F5,
    tooltip_fg: 0x5D6C80,
    menu_bg: 0xFFFFFF,
    menu_hover_bg: 0xF1F1F1,
    scrollbar_bg: 0x8B998E,
    scrollbar_hover_bg: 0x6E7D72,
    msg_in_bg: 0xFFFFFF,
    msg_out_bg: 0xEFFDDE,
    msg_out_bg_top: 0xEFFDDE,
    msg_out_bg_bottom: 0xEFFDDE,
    msg_out_fg: 0x000000,
    msg_out_secondary_fg: 0x6FAB69,
    msg_out_link_fg: 0x168ACD,
    msg_in_selected_bg: 0xE8F3E5,
    msg_out_selected_bg: 0xDAEECC,
    msg_in_date_fg: 0xA0ACB6,
    msg_out_date_fg: 0x6FAB69,
    code_bg: 0xEFF5EC,
    code_fg: 0x459866,
    service_pill_bg: 0x527C41,
    service_pill_fg: 0xFFFFFF,
    muted_badge_bg: 0xBBBBBB,
    online_dot_fg: 0x4DC920,
    check_read_fg: 0x4B9432,
    check_sent_fg: 0x6FAB69,
    icon_fg: 0x8E97A0,
    border_fg: 0xE4E8E2,
    text_primary: 0x000000,
    text_secondary: 0x707579,
    text_link: 0x168ACD,
    accent_hover: 0x2E95D1,
    reaction_bg: 0xEAF4E4,
    danger: 0xE53935,
    success: 0x4B9432,
    wallpaper_top: 0xD5E8C8,
    wallpaper_bottom: 0xB8D8A3,
    wallpaper_pattern: 0x93BD7E,
    peer_name_colors: PEER_NAME_COLORS_DAY,
};

/// Darkens overly bright custom accent colors just enough so white text (`#FFFFFF`)
/// stays comfortably legible on outgoing bubbles and active sidebar rows.
fn ensure_white_text_contrast(hex: u32) -> u32 {
    let r = ((hex >> 16) & 0xFF) as f32;
    let g = ((hex >> 8) & 0xFF) as f32;
    let b = (hex & 0xFF) as f32;
    let luma = 0.299 * r + 0.587 * g + 0.114 * b;
    if luma <= 138.0 {
        hex
    } else {
        let darken_t = ((luma - 138.0) / 145.0).clamp(0.0, 0.36);
        mix_rgb(hex, 0x111111, darken_t)
    }
}

pub(super) fn preset_palette(
    mode: TdesktopThemeMode,
    accent: u32,
    font_size_px: u32,
    bubble_radius_px: u32,
) -> TdesktopThemePalette {
    let spec = match mode {
        TdesktopThemeMode::Night => &NIGHT_SPEC,
        TdesktopThemeMode::Tinted => &TINTED_SPEC,
        TdesktopThemeMode::Day => &DAY_SPEC,
        TdesktopThemeMode::Classic => &CLASSIC_SPEC,
    };
    let eff = if accent == 0 { mode.default_accent() } else { accent };
    let is_dark = mode.is_dark();
    let is_apple = mode.is_fabushi_style();
    let default_family = match mode {
        TdesktopThemeMode::Night => {
            matches!(
                eff,
                0x0A84FF | 0x007AFF | 0x116DD2 | 0x1680F3 | 0x1084FE | 0x1687FF | 0x2AABEE | 0x40A7E3 | 0x5288C1 | 0x3390EC
            )
        }
        TdesktopThemeMode::Tinted => {
            matches!(eff, 0x0A84FF | 0x007AFF | 0x116DD2 | 0x1687FF | 0x1084FE | 0x1ABC9C | 0x3FC1B0 | 0x009687)
        }
        TdesktopThemeMode::Day => {
            matches!(
                eff,
                0x007AFF | 0x0A84FF | 0x1680F3 | 0x126FD4 | 0x1084FE | 0x1687FF | 0x2AABEE | 0x40A7E3 | 0x419FD9 | 0x2481CC | 0x5288C1
            )
        }
        TdesktopThemeMode::Classic => matches!(eff, 0x2AABEE | 0x40A7E3 | 0x419FD9),
    };

    let custom_apple_fill = ensure_white_text_contrast(eff);

    let dialogs_active_bg = if is_apple {
        if default_family {
            spec.dialogs_active_bg
        } else {
            custom_apple_fill
        }
    } else if default_family {
        spec.dialogs_active_bg
    } else if is_dark {
        mix_rgb(spec.dialogs_bg, eff, 0.44)
    } else {
        eff
    };

    let (msg_out_bg, msg_out_bg_top, msg_out_bg_bottom, msg_out_selected_bg, msg_out_secondary_fg, msg_out_date_fg) =
        if mode == TdesktopThemeMode::Classic || default_family {
            (
                spec.msg_out_bg,
                spec.msg_out_bg_top,
                spec.msg_out_bg_bottom,
                spec.msg_out_selected_bg,
                spec.msg_out_secondary_fg,
                spec.msg_out_date_fg,
            )
        } else if is_apple {
            let top = mix_rgb(custom_apple_fill, 0xFFFFFF, if is_dark { 0.10 } else { 0.16 });
            let selected = mix_rgb(custom_apple_fill, 0x000000, 0.16);
            let secondary = mix_rgb(0xFFFFFF, custom_apple_fill, 0.16);
            (custom_apple_fill, top, custom_apple_fill, selected, secondary, secondary)
        } else {
            let base = if is_dark {
                mix_rgb(spec.msg_in_bg, eff, 0.36)
            } else {
                mix_rgb(0xFFFFFF, eff, 0.16)
            };
            let selected = if is_dark {
                mix_rgb(spec.msg_in_bg, eff, 0.48)
            } else {
                mix_rgb(0xFFFFFF, eff, 0.28)
            };
            (base, base, base, selected, spec.msg_out_secondary_fg, spec.msg_out_date_fg)
        };

    let text_link = if mode == TdesktopThemeMode::Classic || default_family {
        spec.text_link
    } else if is_dark {
        mix_rgb(0xFFFFFF, eff, 0.72)
    } else {
        adjust_rgb(eff, -22)
    };

    let check_read_fg = if mode == TdesktopThemeMode::Classic || default_family {
        spec.check_read_fg
    } else {
        eff
    };

    let accent_hover = if eff == mode.default_accent() {
        spec.accent_hover
    } else if is_dark {
        adjust_rgb(eff, 16)
    } else {
        adjust_rgb(eff, -18)
    };

    let unread_badge_bg = if is_apple && !default_family {
        custom_apple_fill
    } else {
        eff
    };

    TdesktopThemePalette {
        mode,
        window_bg: spec.window_bg,
        hover_bg: spec.dialogs_hover_bg,
        nav_rail_bg: spec.rail_bg,
        rail_bg: spec.rail_bg,
        dialogs_bg: spec.dialogs_bg,
        dialogs_hover_bg: spec.dialogs_hover_bg,
        dialogs_active_bg,
        history_bg: spec.stage_bg,
        stage_bg: spec.stage_bg,
        top_bar_bg: spec.topbar_bg,
        topbar_bg: spec.topbar_bg,
        compose_bg: spec.composer_bg,
        composer_bg: spec.composer_bg,
        input_bg: spec.input_bg,
        card_bg: spec.card_bg,
        modal_bg: spec.modal_bg,
        tooltip_bg: spec.tooltip_bg,
        tooltip_fg: spec.tooltip_fg,
        menu_bg: spec.menu_bg,
        menu_hover_bg: spec.menu_hover_bg,
        scrollbar_bg: spec.scrollbar_bg,
        scrollbar_hover_bg: spec.scrollbar_hover_bg,
        msg_in_bg: spec.msg_in_bg,
        msg_out_bg,
        msg_out_bg_top,
        msg_out_bg_bottom,
        msg_out_fg: spec.msg_out_fg,
        msg_out_secondary_fg,
        msg_out_link_fg: spec.msg_out_link_fg,
        msg_in_selected_bg: spec.msg_in_selected_bg,
        msg_out_selected_bg,
        msg_in_date_fg: spec.msg_in_date_fg,
        msg_out_date_fg,
        reply_bar_fg: eff,
        reply_bar_bg: eff,
        code_bg: spec.code_bg,
        code_fg: spec.code_fg,
        service_pill_bg: spec.service_pill_bg,
        service_pill_fg: spec.service_pill_fg,
        unread_badge_bg,
        unread_badge_fg: 0xFFFFFF,
        muted_badge_bg: spec.muted_badge_bg,
        online_dot_fg: spec.online_dot_fg,
        check_read_fg,
        check_sent_fg: spec.check_sent_fg,
        icon_fg: spec.icon_fg,
        icon_active: eff,
        border_fg: spec.border_fg,
        shadow_fg: 0x000000,
        text_primary: spec.text_primary,
        text_secondary: spec.text_secondary,
        text_link,
        accent: eff,
        accent_hover,
        active_button_fg: 0xFFFFFF,
        reaction_bg: spec.reaction_bg,
        reaction_active_bg: eff,
        danger: spec.danger,
        success: spec.success,
        wallpaper_top: spec.wallpaper_top,
        wallpaper_bottom: spec.wallpaper_bottom,
        wallpaper_pattern: spec.wallpaper_pattern,
        peer_name_colors: spec.peer_name_colors,
        peer_avatar_colors: PEER_AVATAR_TOP,
        peer_avatar_colors_bottom: PEER_AVATAR_BOTTOM,
        font_size_px: font_size_px.clamp(12, 20),
        bubble_radius_px: bubble_radius_px.clamp(6, 18),
        ui_scale_pct: 100,
        blur_wallpaper: false,
    }
}

/// Overlays OpenBot `383a35d2` monochrome dark/light surfaces onto `pal`, eliminating harsh
/// `#007AFF`/`#0A84FF`/`#116DD2`/`#1680F3` active/chrome fills while preserving `mode`,
/// `Classic` green bubbles/wallpaper, typography, scale, bubble radius, and wallpaper blur.
pub(super) fn apply_openbot_chrome(mut pal: TdesktopThemePalette) -> TdesktopThemePalette {
    if pal.mode.is_dark() {
        pal.window_bg = 0x0D0D0D;
        pal.nav_rail_bg = 0x0D0D0D;
        pal.rail_bg = 0x0D0D0D;
        pal.top_bar_bg = 0x0D0D0D;
        pal.topbar_bg = 0x0D0D0D;
        pal.dialogs_bg = 0x1A1A1A;
        pal.history_bg = 0x1A1A1A;
        pal.stage_bg = 0x1A1A1A;
        pal.hover_bg = 0x242424;
        pal.dialogs_hover_bg = 0x242424;
        pal.dialogs_active_bg = 0x212121;
        pal.compose_bg = 0x212121;
        pal.composer_bg = 0x212121;
        pal.input_bg = 0x212121;
        pal.card_bg = 0x212121;
        pal.modal_bg = 0x212121;
        pal.tooltip_bg = 0x212121;
        pal.tooltip_fg = 0xFFFFFF;
        pal.menu_bg = 0x212121;
        pal.menu_hover_bg = 0x242424;
        pal.msg_in_bg = 0x212121;
        pal.msg_in_selected_bg = 0x2A2A2A;
        pal.msg_out_bg = 0x242424;
        pal.msg_out_bg_top = 0x242424;
        pal.msg_out_bg_bottom = 0x242424;
        pal.msg_out_fg = 0xFFFFFF;
        pal.msg_out_secondary_fg = 0x979797;
        pal.msg_out_link_fg = 0x42A0FF;
        pal.msg_out_selected_bg = 0x2A2A2A;
        pal.msg_in_date_fg = 0x6A6A6A;
        pal.msg_out_date_fg = 0x979797;
        pal.code_bg = 0x141414;
        pal.code_fg = 0xDCDCDC;
        pal.service_pill_bg = 0x212121;
        pal.service_pill_fg = 0x979797;
        pal.unread_badge_bg = 0x705083;
        pal.unread_badge_fg = 0xF6EFE2;
        pal.muted_badge_bg = 0x3A3A3D;
        pal.online_dot_fg = 0x31CF76;
        pal.check_read_fg = 0x42A0FF;
        pal.check_sent_fg = 0x979797;
        pal.icon_fg = 0x979797;
        pal.icon_active = 0xFFFFFF;
        pal.border_fg = 0x262626;
        pal.text_primary = 0xFFFFFF;
        pal.text_secondary = 0x979797;
        pal.text_link = 0x42A0FF;
        pal.accent = 0x007CF7;
        pal.accent_hover = 0x1F8FFF;
        pal.active_button_fg = 0xFFFFFF;
        pal.reply_bar_fg = 0x42A0FF;
        pal.reply_bar_bg = 0x2A2A2A;
        pal.reaction_bg = 0x212121;
        pal.reaction_active_bg = 0x2A2A2A;
        pal.danger = 0xFD2F3B;
        pal.success = 0x31CF76;
        pal.wallpaper_top = 0x1A1A1A;
        pal.wallpaper_bottom = 0x1A1A1A;
        pal.wallpaper_pattern = 0x212121;
    } else if pal.mode == TdesktopThemeMode::Classic {
        pal.window_bg = 0xE8E8EB;
        pal.nav_rail_bg = 0xE8E8EB;
        pal.rail_bg = 0xE8E8EB;
        pal.top_bar_bg = 0xE8E8EB;
        pal.topbar_bg = 0xE8E8EB;
        pal.dialogs_bg = 0xF5F5F6;
        pal.hover_bg = 0xF0F0F2;
        pal.dialogs_hover_bg = 0xF0F0F2;
        pal.dialogs_active_bg = 0xE8E8EB;
        pal.compose_bg = 0xFFFFFF;
        pal.composer_bg = 0xFFFFFF;
        pal.input_bg = 0xFFFFFF;
        pal.card_bg = 0xFFFFFF;
        pal.modal_bg = 0xFFFFFF;
        pal.menu_bg = 0xFFFFFF;
        pal.menu_hover_bg = 0xF0F0F2;
        pal.unread_badge_bg = 0x705083;
        pal.unread_badge_fg = 0xF6EFE2;
        pal.icon_fg = 0x6B6B70;
        pal.icon_active = 0x141414;
        pal.border_fg = 0xE8E8EB;
        pal.text_primary = 0x141414;
        pal.text_secondary = 0x6B6B70;
        pal.accent = 0x007CF7;
        pal.accent_hover = 0x006AD4;
        pal.reaction_active_bg = 0xE8E8EB;
    } else {
        pal.window_bg = 0xE8E8EB;
        pal.nav_rail_bg = 0xE8E8EB;
        pal.rail_bg = 0xE8E8EB;
        pal.top_bar_bg = 0xE8E8EB;
        pal.topbar_bg = 0xE8E8EB;
        pal.dialogs_bg = 0xF5F5F6;
        pal.history_bg = 0xF5F5F6;
        pal.stage_bg = 0xF5F5F6;
        pal.hover_bg = 0xF0F0F2;
        pal.dialogs_hover_bg = 0xF0F0F2;
        pal.dialogs_active_bg = 0xE8E8EB;
        pal.compose_bg = 0xFFFFFF;
        pal.composer_bg = 0xFFFFFF;
        pal.input_bg = 0xFFFFFF;
        pal.card_bg = 0xFFFFFF;
        pal.modal_bg = 0xFFFFFF;
        pal.tooltip_bg = 0x1C1C1E;
        pal.tooltip_fg = 0xFFFFFF;
        pal.menu_bg = 0xFFFFFF;
        pal.menu_hover_bg = 0xF0F0F2;
        pal.msg_in_bg = 0xFFFFFF;
        pal.msg_in_selected_bg = 0xE8E8EB;
        pal.msg_out_bg = 0xFCFCFC;
        pal.msg_out_bg_top = 0xFCFCFC;
        pal.msg_out_bg_bottom = 0xFCFCFC;
        pal.msg_out_fg = 0x141414;
        pal.msg_out_secondary_fg = 0x6B6B70;
        pal.msg_out_link_fg = 0x0064C8;
        pal.msg_out_selected_bg = 0xE8E8EB;
        pal.msg_in_date_fg = 0x8E8E93;
        pal.msg_out_date_fg = 0x6B6B70;
        pal.code_bg = 0xF0F0F2;
        pal.code_fg = 0x141414;
        pal.service_pill_bg = 0xFFFFFF;
        pal.service_pill_fg = 0x6B6B70;
        pal.unread_badge_bg = 0x705083;
        pal.unread_badge_fg = 0xF6EFE2;
        pal.muted_badge_bg = 0xC7C7CC;
        pal.online_dot_fg = 0x1FA85A;
        pal.check_read_fg = 0x0064C8;
        pal.check_sent_fg = 0x8E8E93;
        pal.icon_fg = 0x6B6B70;
        pal.icon_active = 0x141414;
        pal.border_fg = 0xE8E8EB;
        pal.text_primary = 0x141414;
        pal.text_secondary = 0x6B6B70;
        pal.text_link = 0x0064C8;
        pal.accent = 0x007CF7;
        pal.accent_hover = 0x006AD4;
        pal.active_button_fg = 0xFFFFFF;
        pal.reply_bar_fg = 0x0064C8;
        pal.reply_bar_bg = 0xE8E8EB;
        pal.reaction_bg = 0xFFFFFF;
        pal.reaction_active_bg = 0xE8E8EB;
        pal.danger = 0xE8222D;
        pal.success = 0x1FA85A;
        pal.wallpaper_top = 0xF5F5F6;
        pal.wallpaper_bottom = 0xF5F5F6;
        pal.wallpaper_pattern = 0xE8E8EB;
    }
    pal
}

/// Builds a `TdesktopThemePalette` directly on OpenBot `383a35d2` monochrome chrome surfaces.
pub(super) fn preset_openbot_palette(
    mode: TdesktopThemeMode,
    accent: u32,
    font_size_px: u32,
    bubble_radius_px: u32,
) -> TdesktopThemePalette {
    apply_openbot_chrome(preset_palette(mode, accent, font_size_px, bubble_radius_px))
}
