//! Autocomplete Popup, Attachments Drawer, Formatting Toolbar & Send Options Menu (`popups_ui`).

use gpui::*;
use gpui::prelude::FluentBuilder;

use crate::components::avatar::render_plush_bot_avatar;
use crate::components::bots::bot_attach_web_view::peer_matches_types;
use crate::i18n::{tr_key_or, tr_str};
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::stage::openbot_stage_chrome::{openbot_stage_icon, OpenBotStageIcon};
use super::attachments_drawer::{AttachmentKind, ComposerAttachMiniApp};
use super::field_autocomplete::{AutocompleteType, FieldAutocomplete};
use super::formatting_bar::{FormattingBar, TextStyleKind};

/// OpenBot `+` `ComposerAddMenu` semantic actions (`UI-F13`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerOpenBotAddAction {
    AttachFiles,
    InsertSkillTrigger,
    TeachTask,
    SideQuestion,
    LiveVoice,
    Workflows,
    NewChat,
    Formatting,
    EmojiPicker,
}

#[derive(Debug, Clone)]
pub enum ComposerPopupAction {
    ChooseAutocomplete(String),
    ChooseAttachment(AttachmentKind),
    ChooseMiniApp(ComposerAttachMiniApp),
    ApplyFormat(TextStyleKind),
    SendSilent,
    ScheduleMessage,
    ClosePopups,
    OpenBotAdd(ComposerOpenBotAddAction),
}

pub const OPENBOT_PICKER_DARK_BG_HEX: &str = "#121212";
pub const OPENBOT_PICKER_DARK_BORDER_HEX: &str = "#222222";
pub const OPENBOT_PICKER_LIGHT_BG_HEX: &str = "#f8f8fa";
pub const OPENBOT_PICKER_LIGHT_BORDER_HEX: &str = "#e6e6ea";
pub const OPENBOT_PICKER_RADIUS_PX: f32 = 14.0;
pub const OPENBOT_PICKER_ROW_HEIGHT_PX: f32 = 29.0;
pub const OPENBOT_PICKER_MAX_VISIBLE_ROWS: usize = 7;
pub const OPENBOT_PICKER_MESSAGES_DEBOUNCE_MS: u64 = 150;
pub const OPENBOT_PICKER_MESSAGES_MIN_QUERY: usize = 3;
pub const OPENBOT_AT_PICKER_SECTIONS: [&str; 5] = [
    "agents", "files", "tabs", "threads", "messages",
];

#[derive(Debug, Clone, PartialEq)]
pub struct OpenBotPickerPopupLayout {
    pub open: bool,
    pub trigger: char,
    pub bg_hex: &'static str,
    pub border_hex: &'static str,
    pub radius_px: f32,
    pub row_height_px: f32,
    pub max_visible_rows: usize,
    pub visible_rows: usize,
    pub scroll_enabled: bool,
    pub fade_top: bool,
    pub fade_bottom: bool,
    pub sections: [&'static str; 5],
    pub messages_debounce_ms: u64,
    pub messages_min_query: usize,
    pub messages_query_active: bool,
}

impl OpenBotPickerPopupLayout {
    pub fn with_scroll_offset(mut self, total_rows: usize, scroll_offset: usize) -> Self {
        let scroll_enabled = total_rows > self.visible_rows;
        self.scroll_enabled = scroll_enabled;
        self.fade_top = scroll_enabled && scroll_offset > 0;
        self.fade_bottom = scroll_enabled && scroll_offset.saturating_add(self.visible_rows) < total_rows;
        self
    }
}

pub fn compute_openbot_picker_popup_layout(
    open: bool,
    trigger: char,
    total_rows: usize,
    query_len: usize,
    palette: &TdesktopThemePalette,
) -> OpenBotPickerPopupLayout {
    let is_dark = palette.mode.is_dark();
    let visible_rows = total_rows.min(OPENBOT_PICKER_MAX_VISIBLE_ROWS);
    let scroll_enabled = total_rows > visible_rows;
    OpenBotPickerPopupLayout {
        open,
        trigger,
        bg_hex: if is_dark {
            OPENBOT_PICKER_DARK_BG_HEX
        } else {
            OPENBOT_PICKER_LIGHT_BG_HEX
        },
        border_hex: if is_dark {
            OPENBOT_PICKER_DARK_BORDER_HEX
        } else {
            OPENBOT_PICKER_LIGHT_BORDER_HEX
        },
        radius_px: OPENBOT_PICKER_RADIUS_PX,
        row_height_px: OPENBOT_PICKER_ROW_HEIGHT_PX,
        max_visible_rows: OPENBOT_PICKER_MAX_VISIBLE_ROWS,
        visible_rows,
        scroll_enabled,
        fade_top: false,
        fade_bottom: scroll_enabled,
        sections: OPENBOT_AT_PICKER_SECTIONS,
        messages_debounce_ms: OPENBOT_PICKER_MESSAGES_DEBOUNCE_MS,
        messages_min_query: OPENBOT_PICKER_MESSAGES_MIN_QUERY,
        messages_query_active: trigger == '@' && query_len >= OPENBOT_PICKER_MESSAGES_MIN_QUERY,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerAddMenuLayout {
    pub items: Vec<crate::views::openbot::composer::ComposerAddMenuItem>,
    pub separator_after_indices: Vec<usize>,
    pub leading_or_trailing_separator: bool,
}

/// Computes the OpenBot `+` add-menu entries and separator layout for a peer (`UI-F12-06`).
/// Consumed directly by `render_composer_add_menu`.
pub fn compute_composer_add_menu_layout(is_bot: bool, is_group: bool) -> ComposerAddMenuLayout {
    let items = crate::views::openbot::composer::ComposerAddMenuItem::items_for_peer(is_bot, is_group);
    let mut separator_after_indices = Vec::new();
    // Place an internal group separator between attachment/skill actions and voice/new-chat
    // only when items exist strictly on both sides (never leading or trailing).
    if items.len() > 2 {
        if let Some(idx) = items.iter().position(|it| {
            matches!(
                it,
                crate::views::openbot::composer::ComposerAddMenuItem::VoiceConversation
                    | crate::views::openbot::composer::ComposerAddMenuItem::StartNewChat
            )
        }) {
            if idx > 0 && idx < items.len() {
                separator_after_indices.push(idx - 1);
            }
        }
    }
    let leading_or_trailing_separator = items.is_empty() && !separator_after_indices.is_empty()
        || separator_after_indices
            .iter()
            .any(|&sep_after| sep_after + 1 >= items.len());
    ComposerAddMenuLayout {
        items,
        separator_after_indices,
        leading_or_trailing_separator,
    }
}

const EMOJI_SHORTCODES: &[(&str, &str)] = &[
    (":thumbsup:", "👍"), (":+1:", "👍"), (":heart:", "❤️"), (":fire:", "🔥"),
    (":joy:", "😂"), (":smile:", "😄"), (":rocket:", "🚀"), (":party:", "🎉"),
    (":check:", "✅"), (":100:", "💯"), (":thinking:", "🤔"), (":eyes:", "👀"),
];

pub fn render_autocomplete_popup<V: 'static>(
    ac: &FieldAutocomplete,
    emoji_token: Option<&str>,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, ComposerPopupAction, &mut Context<V>) + Clone + 'static,
) -> Option<AnyElement> {
    let is_dark = palette.mode.is_dark();
    let is_apple = palette.is_apple_style();

    // OpenBot tucked `@mention` picker (`#121212` dark / `#f8f8fa` light, `29px` rows, max 7 rows = `213px`)
    if emoji_token.is_none()
        && ac.is_open
        && ac.autocomplete_type == AutocompleteType::Mentions
        && !ac.mention_rows.is_empty()
    {
        let sel_idx = ac.selected_index.max(0) as usize;
        let scroll_top = sel_idx.saturating_sub(OPENBOT_PICKER_MAX_VISIBLE_ROWS.saturating_sub(1));
        let ob = palette.openbot_palette();
        let picker_layout = compute_openbot_picker_popup_layout(
            true,
            '@',
            ac.mention_rows.len(),
            ac.filter.chars().count(),
            palette,
        )
        .with_scroll_offset(ac.mention_rows.len(), scroll_top);
        let hbg = if is_apple { ob.hover() } else { palette.row_hover_rgba() };
        let active_bg = if is_apple {
            ob.bg_control_active()
        } else {
            palette.row_active_rgba()
        };
        let tucked_bg: Rgba = if is_apple {
            if is_dark { rgb(0x121212).into() } else { rgb(0xF8F8FA).into() }
        } else {
            rgb(palette.menu_bg).into()
        };
        let tucked_border: Rgba = if is_apple {
            if is_dark { rgb(0x222222).into() } else { rgb(0xE6E6EA).into() }
        } else {
            palette.hairline_border_rgba()
        };
        let mut box_el = div()
            .id("composer-autocomplete-popup")
            .when(is_apple, |el| {
                el.w_full()
                    .max_h(px(
                        (picker_layout.max_visible_rows as f32) * picker_layout.row_height_px + 10.0,
                    ))
            })
            .when(!is_apple, |el| el.w(px(360.0)).max_h(px(320.0)))
            .when(picker_layout.scroll_enabled || !is_apple, |el| {
                el.overflow_y_scroll()
            })
            .mb(px(if is_apple { 4.0 } else { 6.0 }))
            .p(px(5.0))
            .rounded(px(picker_layout.radius_px))
            .bg(tucked_bg)
            .border_1()
            .border_color(tucked_border)
            .flex()
            .flex_col()
            .gap(px(1.0));
        if picker_layout.fade_top {
            box_el = box_el.child(div().h(px(1.0)).w_full().bg(ob.border_hairline()));
        }
        if !is_apple {
            box_el = box_el.shadow_lg();
        }

        for (idx, m) in ac.mention_rows.iter().take(12).enumerate() {
            let act = on_action.clone();
            let active = idx == sel_idx;
            let insert = crate::views::composer::composer_event_handlers::format_mention_insertion_token(
                m, is_apple,
            );
            let role_badge = ac.role_hint_for(m);
            let is_everyone = m.is_everyone();
            let hue = if is_everyone {
                "cyan"
            } else if m.is_bot {
                TdesktopThemePalette::bot_hue_name_for_id(m.user_id.wrapping_add(1))
            } else {
                TdesktopThemePalette::bot_hue_name_for_id(m.user_id)
            };
            let (avatar_fill, badge_bg, badge_fg) =
                TdesktopThemePalette::bot_hue_colors(hue, is_dark);
            let initials: String = m
                .name
                .chars()
                .filter(|c| !c.is_whitespace())
                .take(1)
                .collect::<String>()
                .to_uppercase();
            let avatar: AnyElement = if is_everyone {
                div()
                    .w(px(18.0))
                    .h(px(18.0))
                    .rounded_full()
                    .flex_shrink_0()
                    .bg(ob.bg_control_active())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(openbot_stage_icon(OpenBotStageIcon::UsersRound, 11.5, ob.text_primary()))
                    .into_any_element()
            } else if m.is_bot && is_apple {
                render_plush_bot_avatar(
                    &m.name,
                    None,
                    None,
                    None,
                    18.0,
                    openbot_contracts::avatar::AvatarMood::Idle,
                    None,
                )
                .into_any_element()
            } else if m.is_bot {
                div()
                    .w(px(24.0))
                    .h(px(24.0))
                    .rounded_full()
                    .flex_shrink_0()
                    .bg(rgb(avatar_fill))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(tg_icon(IconName::BotRobot, 13.0, 0xFFFFFF))
                    .into_any_element()
            } else {
                let sz = if is_apple { 18.0 } else { 24.0 };
                div()
                    .w(px(sz))
                    .h(px(sz))
                    .rounded_full()
                    .flex_shrink_0()
                    .bg(rgb(avatar_fill))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(if is_apple { 9.5 } else { 10.5 }))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xFFFFFF))
                    .child(if initials.is_empty() { "U".to_string() } else { initials })
                    .into_any_element()
            };

            let handle_label = if !m.username.trim().is_empty() {
                format!("@{}", m.username.trim().trim_start_matches('@'))
            } else {
                String::new()
            };
            let display_title = if m.name.trim().is_empty() {
                handle_label.clone()
            } else {
                m.name.clone()
            };

            box_el = box_el.child(
                div()
                    .id(SharedString::from(format!("ac-row-{}", idx)))
                    .h(px(if is_apple {
                        picker_layout.row_height_px
                    } else {
                        32.0
                    }))
                    .px(px(8.0))
                    .rounded(px(7.0))
                    .when(active, |el| el.bg(active_bg))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hbg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            act(this, ComposerPopupAction::ChooseAutocomplete(insert.clone()), cx);
                        }),
                    )
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(7.0))
                            .child(avatar)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .max_w(px(180.0))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_size(px(13.0))
                                            .font_weight(if active {
                                                FontWeight::MEDIUM
                                            } else {
                                                FontWeight::NORMAL
                                            })
                                            .text_color(if is_apple {
                                                ob.text_primary()
                                            } else {
                                                rgb(palette.text_primary).into()
                                            })
                                            .child(display_title),
                                    )
                                    .when(!handle_label.is_empty(), |row| {
                                        row.child(
                                            div()
                                                .flex_1()
                                                .min_w(px(0.0))
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .text_size(px(12.0))
                                                .text_color(if is_apple {
                                                    ob.text_dim()
                                                } else {
                                                    rgb(palette.text_secondary).into()
                                                })
                                                .child(handle_label),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(6.0))
                            .py(px(1.5))
                            .rounded(px(5.0))
                            .bg(if is_apple {
                                ob.bg_control()
                            } else if m.is_bot || is_everyone {
                                rgb(badge_bg).into()
                            } else {
                                TdesktopThemePalette::rgba_with_alpha(palette.text_secondary, 0.12)
                            })
                            .text_size(px(10.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if is_apple {
                                ob.text_muted()
                            } else if m.is_bot || is_everyone {
                                rgb(badge_fg).into()
                            } else {
                                rgb(palette.text_secondary).into()
                            })
                            .child(role_badge),
                    ),
            );
        }
        if picker_layout.fade_bottom {
            box_el = box_el.child(div().h(px(1.0)).w_full().bg(ob.border_hairline()));
        }

        return Some(box_el.into_any_element());
    }

    let mut rows: Vec<(String, String, String)> = Vec::new();
    if let Some(etok) = emoji_token {
        let q = etok.to_lowercase();
        for &(code, glyph) in EMOJI_SHORTCODES {
            if code.starts_with(&q) || code.contains(&q[1..]) {
                rows.push((glyph.to_string(), code.to_string(), glyph.to_string()));
            }
        }
    } else if ac.is_open {
        match ac.autocomplete_type {
            AutocompleteType::Mentions => {}
            AutocompleteType::Hashtags => {
                for h in &ac.hashtag_rows {
                    rows.push((format!("#{}", h.tag), "Hashtag".to_string(), format!("#{} ", h.tag)));
                }
            }
            AutocompleteType::BotCommands => {
                for c in &ac.bot_command_rows {
                    rows.push((format!("/{}", c.command), c.description.clone(), format!("/{} ", c.command)));
                }
            }
            AutocompleteType::Stickers => {
                for s in &ac.sticker_rows {
                    rows.push((s.emoji.clone(), "Sticker".to_string(), s.emoji.clone()));
                }
            }
        }
    }

    if rows.is_empty() {
        return None;
    }

    let sel_idx = ac.selected_index.max(0) as usize;
    let hbg = palette.menu_hover_bg;
    let mut box_el = div()
        .id("composer-autocomplete-popup")
        .w(px(340.0))
        .max_h(px(220.0))
        .overflow_y_scroll()
        .mb(px(6.0))
        .p(px(6.0))
        .rounded(px(if is_apple { 14.0 } else { 10.0 }))
        .bg(if is_apple {
            TdesktopThemePalette::rgba_with_alpha(palette.menu_bg, 0.96)
        } else {
            rgb(palette.menu_bg).into()
        })
        .border_1()
        .border_color(if is_apple {
            palette.hairline_border_rgba()
        } else {
            rgb(palette.border_fg).into()
        })
        .flex()
        .flex_col()
        .gap(px(2.0));
    box_el = if is_apple {
        box_el.shadow(palette.popover_shadows())
    } else {
        box_el.shadow_lg()
    };

    for (idx, (primary, secondary, insert)) in rows.into_iter().enumerate() {
        let act = on_action.clone();
        let active = idx == sel_idx;
        box_el = box_el.child(
            div()
                .id(SharedString::from(format!("ac-row-{}", idx)))
                .min_h(px(32.0))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .when(active, |el| el.bg(TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.20)))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(hbg)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    act(this, ComposerPopupAction::ChooseAutocomplete(insert.clone()), cx);
                }))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(div().text_size(px(13.0)).font_weight(FontWeight::BOLD).text_color(rgb(palette.accent)).child(primary))
                .child(div().text_size(px(12.0)).text_color(rgb(palette.text_secondary)).child(secondary)),
        );
    }

    Some(box_el.into_any_element())
}

pub fn render_attachments_drawer<V: 'static>(
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, ComposerPopupAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    render_attachments_drawer_with_apps(&[], 0, palette, lang_id, cx, on_action)
}

pub fn render_attachments_drawer_with_apps<V: 'static>(
    attach_apps: &[ComposerAttachMiniApp],
    current_peer_type_mask: u8,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, ComposerPopupAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let entries = [
        (AttachmentKind::PhotoOrVideo, IconName::Camera, 0x0A84FFu32, tr_key_or(lang_id, "lng_attach_photo_video", "Photo or Video")),
        (AttachmentKind::DocumentOrFile, IconName::FileDoc, 0x5E5CE6u32, tr_key_or(lang_id, "lng_attach_file", "File")),
        (AttachmentKind::CreatePoll, IconName::Poll, 0xFF9F0Au32, tr_key_or(lang_id, "lng_polls_create", "Poll")),
        (AttachmentKind::SendLocation, IconName::LocationPin, 0x30D158u32, tr_key_or(lang_id, "lng_attach_location", "Location")),
        (AttachmentKind::ShareContact, IconName::ContactsUsers, 0x64D2FFu32, tr_key_or(lang_id, "lng_attach_contact", "Contact")),
    ];
    let hbg = palette.menu_hover_bg;
    let filtered_apps: Vec<ComposerAttachMiniApp> = attach_apps
        .iter()
        .filter(|app| {
            current_peer_type_mask == 0
                || app.peer_types_mask == 0
                || peer_matches_types(current_peer_type_mask, app.peer_types_mask)
        })
        .cloned()
        .collect();

    let is_apple = palette.is_apple_style();
    if is_apple {
        let ob = palette.openbot_palette();
        let hover_bg = ob.hover();
        let is_group = (current_peer_type_mask
            & (crate::components::bots::bot_attach_web_view::peer_types::GROUP
                | crate::components::bots::bot_attach_web_view::peer_types::BROADCAST))
            != 0;
        let is_bot = current_peer_type_mask == 0
            || (current_peer_type_mask
                & (crate::components::bots::bot_attach_web_view::peer_types::BOT
                    | crate::components::bots::bot_attach_web_view::peer_types::SAME_BOT))
                != 0;
        let add_menu_layout = compute_composer_add_menu_layout(is_bot, is_group);
        let ob_entries: Vec<(ComposerOpenBotAddAction, OpenBotStageIcon, &str)> = add_menu_layout
            .items
            .into_iter()
            .map(|item| match item {
                crate::views::openbot::composer::ComposerAddMenuItem::AttachFiles => (
                    ComposerOpenBotAddAction::AttachFiles,
                    OpenBotStageIcon::FileText,
                    tr_str(lang_id, "附加文件或图片", "Attach files"),
                ),
                crate::views::openbot::composer::ComposerAddMenuItem::PickSkill => (
                    ComposerOpenBotAddAction::InsertSkillTrigger,
                    OpenBotStageIcon::Puzzle,
                    tr_str(lang_id, "使用技能 ($)", "Use skill ($)"),
                ),
                crate::views::openbot::composer::ComposerAddMenuItem::TeachBrowserTask => (
                    ComposerOpenBotAddAction::TeachTask,
                    OpenBotStageIcon::Globe,
                    tr_str(lang_id, "教任务（浏览器演示）", "Teach a task"),
                ),
                crate::views::openbot::composer::ComposerAddMenuItem::AskSideQuestion => (
                    ComposerOpenBotAddAction::SideQuestion,
                    OpenBotStageIcon::SideQuestion,
                    tr_str(lang_id, "旁问 (Side Question)", "Side question"),
                ),
                crate::views::openbot::composer::ComposerAddMenuItem::VoiceConversation => (
                    ComposerOpenBotAddAction::LiveVoice,
                    OpenBotStageIcon::AudioLines,
                    tr_str(lang_id, "语音对话", "Voice conversation"),
                ),
                crate::views::openbot::composer::ComposerAddMenuItem::StartNewChat => (
                    ComposerOpenBotAddAction::NewChat,
                    OpenBotStageIcon::NewChat,
                    tr_str(lang_id, "新对话 (/new)", "New chat"),
                ),
            })
            .collect();
        let mut box_el = div()
            .id("composer-attachments-popup")
            .w(px(252.0))
            .mb(px(6.0))
            .p(px(6.0))
            .rounded(px(14.0))
            .bg(ob.bg_surface())
            .border_1()
            .border_color(ob.border_strong_hairline())
            .shadow(palette.popover_shadows())
            .flex()
            .flex_col()
            .gap(px(1.0));
        for (idx, (act_kind, icon_kind, label)) in ob_entries.into_iter().enumerate() {
            let act = on_action.clone();
            let add_sep_after = add_menu_layout.separator_after_indices.contains(&idx);
            box_el = box_el.child(
                div()
                    .id(SharedString::from(format!("att-ob-opt-{}", idx)))
                    .h(px(32.0))
                    .px(px(9.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        act(this, ComposerPopupAction::OpenBotAdd(act_kind), cx);
                    }))
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .child(openbot_stage_icon(icon_kind, 14.5, ob.text_muted()))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(13.0))
                            .text_color(ob.text_primary())
                            .child(label.to_string()),
                    ),
            );
            if add_sep_after {
                box_el = box_el.child(div().w_full().h(px(1.0)).my(px(3.0)).bg(ob.border_hairline()));
            }
        }
        box_el = box_el.child(div().w_full().h(px(1.0)).my(px(4.0)).bg(ob.border_hairline()));
        let extra_tools: [(ComposerPopupAction, &str, &str); 5] = [
            (ComposerPopupAction::OpenBotAdd(ComposerOpenBotAddAction::EmojiPicker), "😊", tr_str(lang_id, "表情 / 贴纸", "Emoji & Stickers")),
            (ComposerPopupAction::OpenBotAdd(ComposerOpenBotAddAction::Formatting), "Aa", tr_str(lang_id, "文字格式", "Formatting")),
            (ComposerPopupAction::ChooseAttachment(AttachmentKind::CreatePoll), "📊", tr_key_or(lang_id, "lng_polls_create", "Poll")),
            (ComposerPopupAction::ChooseAttachment(AttachmentKind::SendLocation), "📍", tr_key_or(lang_id, "lng_attach_location", "Location")),
            (ComposerPopupAction::ChooseAttachment(AttachmentKind::ShareContact), "👤", tr_key_or(lang_id, "lng_attach_contact", "Contact")),
        ];
        for (idx, (popup_act, glyph, label)) in extra_tools.into_iter().enumerate() {
            let act = on_action.clone();
            box_el = box_el.child(
                div()
                    .id(SharedString::from(format!("att-opt-{}", idx)))
                    .h(px(30.0))
                    .px(px(9.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        act(this, popup_act.clone(), cx);
                    }))
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .child(
                        div()
                            .w(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.0))
                            .text_color(ob.text_muted())
                            .child(glyph),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.5))
                            .text_color(ob.text_secondary())
                            .child(label.to_string()),
                    ),
            );
        }
        if !filtered_apps.is_empty() {
            box_el = box_el
                .child(div().w_full().h(px(1.0)).my(px(4.0)).bg(ob.border_hairline()))
                .child(
                    div()
                        .px(px(9.0))
                        .py(px(2.0))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.text_dim())
                        .child(tr_str(lang_id, "小程序", "Mini Apps")),
                );
            for app in filtered_apps {
                let act = on_action.clone();
                let app_owned = app.clone();
                box_el = box_el.child(
                    div()
                        .id(SharedString::from(format!("att-miniapp-{}", app.short_name)))
                        .h(px(30.0))
                        .px(px(9.0))
                        .rounded(px(8.0))
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover_bg))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            act(this, ComposerPopupAction::ChooseMiniApp(app_owned.clone()), cx);
                        }))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.5))
                                .text_color(ob.text_primary())
                                .child(app.title),
                        ),
                );
            }
        }
        return box_el.into_any_element();
    }

    let sep_col: Rgba = rgb(palette.border_fg).into();
    let mut box_el = div()
        .id("composer-attachments-popup")
        .w(px(236.0))
        .mb(px(8.0))
        .p(px(6.0))
        .rounded(px(10.0))
        .bg(rgb(palette.menu_bg))
        .border_1()
        .border_color(sep_col)
        .shadow_lg()
        .flex()
        .flex_col()
        .gap(px(2.0));

    for (idx, (kind, icon, _badge_col, label)) in entries.into_iter().enumerate() {
        let act = on_action.clone();
        box_el = box_el.child(
            div()
                .id(SharedString::from(format!("att-opt-{}", idx)))
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(7.0))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(hbg)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    act(this, ComposerPopupAction::ChooseAttachment(kind), cx);
                }))
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(tg_icon(icon, 16.0, palette.accent))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(rgb(palette.text_primary))
                        .child(label.to_string()),
                ),
        );
    }

    if !filtered_apps.is_empty() {
        box_el = box_el
            .child(div().w_full().h(px(1.0)).my(px(4.0)).bg(sep_col))
            .child(
                div()
                    .px(px(10.0))
                    .py(px(2.0))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(palette.text_secondary))
                    .child(tr_str(lang_id, "已安装小程序", "Mini Apps")),
            );
        for app in filtered_apps {
            let act = on_action.clone();
            let app_owned = app.clone();
            let badge: String = app
                .title
                .chars()
                .take(1)
                .collect::<String>()
                .to_uppercase();
            let icon_col = if app.icon_color != 0 { app.icon_color } else { palette.accent };
            box_el = box_el.child(
                div()
                    .id(SharedString::from(format!("att-miniapp-{}", app.short_name)))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(hbg)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        act(this, ComposerPopupAction::ChooseMiniApp(app_owned.clone()), cx);
                    }))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded_full()
                            .bg(rgb(icon_col))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xFFFFFF))
                            .child(if badge.is_empty() { "M".to_string() } else { badge }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(rgb(palette.text_primary)).child(app.title))
                            .when(!app.bot_username.is_empty(), |col| {
                                col.child(div().text_size(px(10.5)).text_color(rgb(palette.text_secondary)).child(format!("@{}", app.bot_username.trim_start_matches('@'))))
                            }),
                    ),
            );
        }
    }

    box_el.into_any_element()
}

pub fn render_formatting_toolbar<V: 'static>(
    fmt: &FormattingBar,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, ComposerPopupAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let items = [
        (TextStyleKind::Bold, "B"),
        (TextStyleKind::Italic, "I"),
        (TextStyleKind::Underline, "U"),
        (TextStyleKind::Strikethrough, "S"),
        (TextStyleKind::Monospace, "</>"),
        (TextStyleKind::Spoiler, "||"),
        (TextStyleKind::Link, "Link"),
    ];
    let hbg = palette.menu_hover_bg;
    let is_apple = palette.is_apple_style();
    let mut row = div()
        .id("composer-formatting-bar")
        .px(px(8.0))
        .py(px(5.0))
        .mb(px(6.0))
        .rounded(px(if is_apple { 12.0 } else { 8.0 }))
        .bg(if is_apple {
            TdesktopThemePalette::rgba_with_alpha(palette.menu_bg, 0.96)
        } else {
            rgb(palette.card_bg).into()
        })
        .border_1()
        .border_color(if is_apple {
            palette.separator_rgba()
        } else {
            rgb(palette.border_fg).into()
        })
        .flex()
        .items_center()
        .gap(px(4.0));
    if is_apple {
        row = row.shadow(palette.popover_shadows());
    }

    for (idx, (kind, lbl)) in items.into_iter().enumerate() {
        let active = fmt.has_style(kind);
        let a = on_action.clone();
        row = row.child(
            div()
                .id(SharedString::from(format!("fmt-btn-{}", idx)))
                .px(px(9.0))
                .py(px(3.0))
                .rounded(px(6.0))
                .when(active, |el| el.bg(TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.22)))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(hbg)))
                .on_click(cx.listener(move |this, _, _, cx| a(this, ComposerPopupAction::ApplyFormat(kind), cx)))
                .text_size(px(12.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(if active { palette.accent } else { palette.text_primary }))
                .child(lbl),
        );
    }
    row.into_any_element()
}

pub fn render_send_options_popup<V: 'static>(
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, ComposerPopupAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let a_sil = on_action.clone();
    let a_sch = on_action;
    let hbg = palette.menu_hover_bg;
    let is_apple = palette.is_apple_style();
    let mut card = div()
        .id("composer-send-opts")
        .w(px(210.0))
        .p(px(6.0))
        .mb(px(6.0))
        .rounded(px(if is_apple { 14.0 } else { 10.0 }))
        .bg(if is_apple {
            TdesktopThemePalette::rgba_with_alpha(palette.menu_bg, 0.96)
        } else {
            rgb(palette.menu_bg).into()
        })
        .border_1()
        .border_color(if is_apple {
            palette.separator_rgba()
        } else {
            rgb(palette.border_fg).into()
        })
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .id("send-silent-opt")
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(if is_apple { 8.0 } else { 7.0 }))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(hbg)))
                .on_click(cx.listener(move |this, _, _, cx| a_sil(this, ComposerPopupAction::SendSilent, cx)))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(tg_icon(IconName::Mute, 15.0, palette.icon_fg))
                .child(div().text_size(px(13.0)).text_color(rgb(palette.text_primary)).child(tr_key_or(lang_id, "lng_send_silent_message", "Send Without Sound"))),
        )
        .child(
            div()
                .id("send-schedule-opt")
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(if is_apple { 8.0 } else { 7.0 }))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(hbg)))
                .on_click(cx.listener(move |this, _, _, cx| a_sch(this, ComposerPopupAction::ScheduleMessage, cx)))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(tg_icon(IconName::ClockSending, 15.0, palette.icon_fg))
                .child(div().text_size(px(13.0)).text_color(rgb(palette.text_primary)).child(tr_key_or(lang_id, "lng_schedule_message", "Schedule Message"))),
        );
    card = if is_apple {
        card.shadow(palette.popover_shadows())
    } else {
        card.shadow_lg()
    };
    card.into_any_element()
}
