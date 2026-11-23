//! 1:1 Telegram Desktop dialog row and forum topics tree renderer.

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::components::grok_bot_avatar::{
    render_grok_bot_avatar, GrokBotAvatarProps, GrokPersonaState,
};
use crate::i18n::tr_str;
use crate::theme::{tg_icon, tg_icon_rgba, IconName, TdesktopThemePalette};
use crate::views::chat_sidebar::{BotGrokAvatarVisual, DialogDeliveryStatus, DialogItemModel};
use crate::views::sidebar::dialogs_entry::DialogsEntry;
use crate::views::sidebar::dialogs_row::DialogsRow;
use crate::views::sidebar::ui::dialogs_layout::DialogsLayoutUi;
use crate::views::sidebar::ui::dialogs_message_view::{
    DialogMessageMediaBadge, DialogsMessageViewUi,
};
use crate::views::sidebar::ui::dialogs_pill::DialogsPillUi;
use crate::views::sidebar::ui::dialogs_topics_view::DialogsTopicsViewUi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarContextAction {
    TogglePin(u64),
    ToggleMute(u64),
    ToggleArchive(u64),
    ToggleUnread(u64),
    ClearHistory(u64),
    DeleteChat(u64),
}

#[derive(Debug, Clone, Default)]
pub struct SidebarDialogMeta {
    pub is_verified: bool,
    pub is_premium: bool,
    pub is_bot: bool,
    pub fabushi_shape: Option<String>,
    pub fabushi_color: Option<u32>,
    pub grok_avatar: Option<BotGrokAvatarVisual>,
    pub is_saved_messages: bool,
    pub is_unread_marked: bool,
    pub is_archived: bool,
    pub draft_text: Option<String>,
    pub media_badge: DialogMessageMediaBadge,
    pub topics: Option<DialogsTopicsViewUi>,
    pub is_topics_expanded: bool,
    pub folder_tags: Vec<String>,
    pub miniapp_open_uri: Option<String>,
}

fn delivery_svg(
    status: DialogDeliveryStatus,
    palette: &TdesktopThemePalette,
    is_selected: bool,
) -> AnyElement {
    let icon = match status {
        DialogDeliveryStatus::Sending => IconName::ClockSending,
        DialogDeliveryStatus::Sent => IconName::CheckSingle,
        DialogDeliveryStatus::Read => IconName::CheckDouble,
    };
    let use_inverted_sel = is_selected && !palette.mode.is_fabushi_style();
    let color = if use_inverted_sel {
        palette.active_button_fg
    } else if status == DialogDeliveryStatus::Read {
        palette.check_read_fg
    } else {
        palette.check_sent_fg
    };
    tg_icon(icon, 13.0, color).into_any_element()
}

fn media_badge_svg_rgba(badge: DialogMessageMediaBadge, color: Rgba) -> Option<AnyElement> {
    let icon = match badge {
        DialogMessageMediaBadge::None => return None,
        DialogMessageMediaBadge::Photo => IconName::Camera,
        DialogMessageMediaBadge::Video => IconName::VideoRound,
        DialogMessageMediaBadge::Document => IconName::FileDoc,
        DialogMessageMediaBadge::Audio => IconName::Play,
        DialogMessageMediaBadge::Voice => IconName::Mic,
        DialogMessageMediaBadge::Location => IconName::LocationPin,
        DialogMessageMediaBadge::Poll => IconName::Poll,
        DialogMessageMediaBadge::Contact => IconName::ContactsUsers,
        DialogMessageMediaBadge::Game => IconName::BotRobot,
    };
    Some(tg_icon_rgba(icon, 12.0, color).into_any_element())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogPreviewPriority {
    BlockedApproval(String),
    Draft(String),
    LiveActivity(String),
    Message(String),
}

fn strip_known_prefix<'a>(text: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    let trimmed = text.trim();
    for p in prefixes {
        if let Some(rest) = trimmed.strip_prefix(p) {
            return Some(rest.trim());
        }
    }
    None
}

/// Resolves Grok Bot's 4-tier sidebar preview priority order:
/// (1) pending approval / blocked ("等你处理：…"),
/// (2) unsent draft ("草稿：…"),
/// (3) live agent activity / typing indicator,
/// (4) last message snippet.
pub fn resolve_dialog_preview_priority(
    item: &DialogItemModel,
    meta: &SidebarDialogMeta,
    lang_id: &str,
) -> DialogPreviewPriority {
    const BLOCKED_PREFIXES: &[&str] = &[
        "等你处理：",
        "等你处理:",
        "需要批准：",
        "需要批准:",
        "[需要批准]",
        "approval:",
        "blocked:",
        "Needs approval:",
        "Approval required:",
        "Waiting for approval:",
    ];

    let is_grok_blocked = meta.grok_avatar.as_ref().is_some_and(|g| {
        let s = g.execution_state.trim().to_ascii_lowercase();
        matches!(s.as_str(), "blocked" | "awaiting_approval" | "approval" | "needs_approval")
    });
    if let Some(ref typing) = item.typing_indicator {
        if let Some(rest) = strip_known_prefix(typing, BLOCKED_PREFIXES) {
            let detail = if rest.is_empty() {
                tr_str(lang_id, "工具操作待确认", "Action awaiting approval").to_string()
            } else {
                rest.to_string()
            };
            return DialogPreviewPriority::BlockedApproval(detail);
        }
    }
    if let Some(rest) = strip_known_prefix(&item.last_message, BLOCKED_PREFIXES) {
        let detail = if rest.is_empty() {
            tr_str(lang_id, "工具操作待确认", "Action awaiting approval").to_string()
        } else {
            rest.to_string()
        };
        return DialogPreviewPriority::BlockedApproval(detail);
    }
    if is_grok_blocked {
        let detail = if item.last_message.trim().is_empty() {
            tr_str(lang_id, "需要你的批准", "Needs your approval").to_string()
        } else {
            item.last_message.trim().to_string()
        };
        return DialogPreviewPriority::BlockedApproval(detail);
    }

    if let Some(draft) = meta.draft_text.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return DialogPreviewPriority::Draft(draft.to_string());
    }

    if let Some(typing) = item
        .typing_indicator
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return DialogPreviewPriority::LiveActivity(typing.to_string());
    }

    if let Some(ref ga) = meta.grok_avatar {
        let st = ga.execution_state.trim().to_ascii_lowercase();
        if !st.is_empty() && st != "idle" {
            let label = match st.as_str() {
                "tool" | "working" | "coding" => tr_str(lang_id, "正在执行工具…", "Running tools…"),
                "writing" => tr_str(lang_id, "正在组织回复…", "Writing reply…"),
                _ => tr_str(lang_id, "正在思考…", "Thinking…"),
            };
            return DialogPreviewPriority::LiveActivity(label.to_string());
        }
    }

    DialogPreviewPriority::Message(item.last_message.clone())
}

fn derive_cluster_tiles(
    seed_id: u64,
    title: &str,
    member_tiles: Option<&[(String, u32)]>,
    is_dark: bool,
) -> Vec<(String, u32)> {
    if let Some(tiles) = member_tiles {
        let valid: Vec<(String, u32)> = tiles
            .iter()
            .filter(|(lbl, _)| !lbl.trim().is_empty())
            .take(3)
            .cloned()
            .collect();
        if valid.len() >= 2 {
            return valid;
        }
    }

    let mut parts: Vec<String> = title
        .split(|c: char| matches!(c, '、' | ',' | '&' | '·' | '/' | '|'))
        .flat_map(|seg| seg.split("和"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().next().unwrap_or('G').to_uppercase().to_string())
        .take(3)
        .collect();

    if parts.len() < 2 {
        let chars: Vec<char> = title.chars().filter(|c| !c.is_whitespace()).collect();
        parts.clear();
        if chars.len() >= 2 {
            parts.push(chars[0].to_uppercase().to_string());
            parts.push(chars[1].to_uppercase().to_string());
            if chars.len() >= 4 {
                parts.push(chars[chars.len() / 2].to_uppercase().to_string());
            }
        } else if let Some(&first) = chars.first() {
            parts.push(first.to_uppercase().to_string());
            parts.push("AI".to_string());
        } else {
            parts.push("G".to_string());
            parts.push("B".to_string());
        }
    }

    parts
        .into_iter()
        .enumerate()
        .map(|(idx, glyph)| {
            let hue = TdesktopThemePalette::bot_hue_name_for_id(seed_id.wrapping_add(idx as u64 * 3 + 1));
            let (fill, _, _) = TdesktopThemePalette::bot_hue_colors(hue, is_dark);
            (glyph, fill)
        })
        .collect()
}

/// Renders Grok Bot's overlapping composite group avatar (2–3 mini circular tiles with
/// surface-matched cutout rings) for group chats without a custom photo.
pub fn render_composite_group_avatar(
    seed_id: u64,
    title: &str,
    member_tiles: Option<&[(String, u32)]>,
    avatar_size_px: f32,
    surface_bg_hex: u32,
    is_dark: bool,
) -> Div {
    let tiles = derive_cluster_tiles(seed_id, title, member_tiles, is_dark);
    let ring = rgb(surface_bg_hex);
    let container = div()
        .relative()
        .w(px(avatar_size_px))
        .h(px(avatar_size_px))
        .flex_shrink_0();

    if tiles.len() >= 3 {
        let tile_d = (avatar_size_px * 0.56).round();
        let font_px = (tile_d * 0.44).clamp(8.5, 13.0);
        let positions = [
            ((avatar_size_px - tile_d) * 0.5, 0.0),
            (0.0, avatar_size_px - tile_d),
            (avatar_size_px - tile_d, avatar_size_px - tile_d),
        ];
        tiles
            .into_iter()
            .zip(positions)
            .fold(container, |acc, ((glyph, color), (left_px, top_px))| {
                acc.child(
                    div()
                        .absolute()
                        .left(px(left_px))
                        .top(px(top_px))
                        .w(px(tile_d))
                        .h(px(tile_d))
                        .rounded_full()
                        .bg(rgb(color))
                        .border_2()
                        .border_color(ring)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(font_px))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .child(glyph),
                )
            })
    } else {
        let tile_d = (avatar_size_px * 0.66).round();
        let font_px = (tile_d * 0.44).clamp(9.0, 14.0);
        let positions = [
            (0.0, 0.0),
            (avatar_size_px - tile_d, avatar_size_px - tile_d),
        ];
        tiles
            .into_iter()
            .zip(positions)
            .fold(container, |acc, ((glyph, color), (left_px, top_px))| {
                acc.child(
                    div()
                        .absolute()
                        .left(px(left_px))
                        .top(px(top_px))
                        .w(px(tile_d))
                        .h(px(tile_d))
                        .rounded_full()
                        .bg(rgb(color))
                        .border_2()
                        .border_color(ring)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(font_px))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .child(glyph),
                )
            })
    }
}

/// Shared circular / animated bot avatar renderer used by both dialog rows and the Apple Messages
/// 3-column Pinned Chats avatar grid.
pub(crate) fn render_dialog_avatar(
    item: &DialogItemModel,
    meta: &SidebarDialogMeta,
    palette: &TdesktopThemePalette,
    avatar_size_px: f32,
    surface_bg_hex: u32,
    online_dot_border: Rgba,
) -> Div {
    let avatar_bg = if meta.is_saved_messages {
        palette.accent
    } else {
        let base = meta.fabushi_color.unwrap_or(item.avatar_color);
        if base == 0 {
            palette.peer_avatar_gradient(item.id as i64).0
        } else {
            base
        }
    };
    let avatar_size = px(avatar_size_px);
    let initials_font_px = (avatar_size_px * 0.35).clamp(12.0, 19.0);
    let saved_icon_px = (avatar_size_px * 0.48).clamp(17.0, 26.0);
    let bot_icon_px = (avatar_size_px * 0.44).clamp(17.0, 24.0);
    let is_group_dialog = !meta.is_saved_messages
        && !meta.is_bot
        && meta.grok_avatar.is_none()
        && (item.folder == "Groups"
            || meta.topics.as_ref().is_some_and(|t| !t.topics.is_empty()));

    let render_classic_circle = || -> Div {
        let mut el = div()
            .relative()
            .w(avatar_size)
            .h(avatar_size)
            .flex_shrink_0()
            .rounded_full()
            .bg(rgb(avatar_bg))
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(0xFFFFFF))
            .font_weight(FontWeight::BOLD)
            .text_size(px(initials_font_px));
        el = if meta.is_saved_messages {
            el.child(tg_icon(IconName::SavedMessages, saved_icon_px, 0xFFFFFF))
        } else if meta.is_bot
            && item
                .avatar_initials
                .chars()
                .all(|c| !c.is_ascii_alphanumeric())
        {
            el.child(tg_icon(IconName::BotRobot, bot_icon_px, 0xFFFFFF))
        } else {
            el.child(item.avatar_initials.clone())
        };
        el
    };

    let mut avatar = if !meta.is_saved_messages {
        if is_group_dialog {
            render_composite_group_avatar(
                item.id,
                &item.peer_name,
                None,
                avatar_size_px,
                surface_bg_hex,
                palette.mode.is_dark(),
            )
        } else if let Some(ref ga) = meta.grok_avatar {
            if !ga.shape.eq_ignore_ascii_case("classic") {
                render_grok_bot_avatar(
                    GrokBotAvatarProps::new(&ga.agent_id, &ga.shape, &ga.color, avatar_size_px)
                        .with_state(GrokPersonaState::from_activity_str(
                            &ga.execution_state,
                            item.typing_indicator.is_some(),
                        ))
                        .with_follow_pointer(true)
                        .with_paused(false)
                        .with_surface_bg(surface_bg_hex),
                )
            } else {
                render_classic_circle()
            }
        } else if let Some(ref shape) = meta.fabushi_shape {
            if !shape.eq_ignore_ascii_case("classic") {
                crate::components::avatar::render_fabushi_shape_avatar(
                    shape,
                    avatar_bg,
                    &item.peer_name,
                    avatar_size_px,
                )
            } else {
                render_classic_circle()
            }
        } else if meta.is_bot {
            const DEFAULT_BOT_SHAPES: [&str; 6] = [
                "circle", "squircle", "hexagon", "shield", "cloud", "diamond",
            ];
            const DEFAULT_BOT_COLORS: [&str; 6] = [
                "blue", "cyan", "purple", "emerald", "amber", "rose",
            ];
            let idx = (item.id as usize) % DEFAULT_BOT_SHAPES.len();
            render_grok_bot_avatar(
                GrokBotAvatarProps::new(
                    &format!("bot-{}", item.id),
                    DEFAULT_BOT_SHAPES[idx],
                    DEFAULT_BOT_COLORS[idx],
                    avatar_size_px,
                )
                .with_state(if item.typing_indicator.is_some() {
                    GrokPersonaState::Thinking
                } else {
                    GrokPersonaState::Idle
                })
                .with_follow_pointer(true)
                .with_paused(false)
                .with_surface_bg(surface_bg_hex),
            )
        } else {
            render_classic_circle()
        }
    } else {
        render_classic_circle()
    };

    let is_agent_running = item.typing_indicator.is_some()
        || meta.grok_avatar.as_ref().is_some_and(|g| {
            let s = g.execution_state.trim().to_ascii_lowercase();
            !s.is_empty() && s != "idle" && s != "blocked" && s != "awaiting_approval"
        });
    if (item.is_online || is_agent_running) && !meta.is_saved_messages {
        let dot_px = (avatar_size_px * 0.24).clamp(8.5, 11.5);
        let dot_color = if is_agent_running {
            palette.accent
        } else {
            palette.online_dot_fg
        };
        avatar = avatar.relative().child(
            div()
                .absolute()
                .bottom_0()
                .right_0()
                .w(px(dot_px))
                .h(px(dot_px))
                .rounded_full()
                .bg(rgb(dot_color))
                .border_2()
                .border_color(online_dot_border),
        );
    }

    avatar
}

pub fn render_dialog_row_item<V: 'static>(
    item: &DialogItemModel,
    index: usize,
    is_selected: bool,
    is_narrow: bool,
    meta: &SidebarDialogMeta,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_select: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_context: impl Fn(&mut V, Point<Pixels>, &mut Context<V>) + 'static,
    on_toggle_topics: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_open_miniapp: impl Fn(&mut V, String, &mut Context<V>) + 'static,
) -> AnyElement {
    let mut entry = DialogsEntry::new(item.id as i64, item.peer_name.clone());
    entry.data.is_forum = meta.topics.as_ref().is_some_and(|t| !t.topics.is_empty());
    entry.set_active(is_selected);
    let mut row_model = DialogsRow::new(entry, index, 0);
    row_model.set_selected(is_selected);
    row_model.recount_height(if is_narrow { 1.0 } else { 0.0 });
    let _ = (DialogsLayoutUi::AVATAR_SIZE, DialogsPillUi::HEIGHT, DialogsPillUi::PADDING);

    let mut preview_ui = if let Some(ref draft) = meta.draft_text {
        DialogsMessageViewUi::as_draft(draft.clone())
    } else {
        DialogsMessageViewUi::new(item.last_message.clone())
    };
    preview_ui.media_badge = meta.media_badge;
    let mut pill = DialogsPillUi::new(String::new(), item.unread_count);
    pill.is_active = is_selected;
    pill.is_muted = item.is_muted;

    if palette.is_apple_style() {
        // Grok Bot `dC` sidebar row rhythm: 54px height, 10px pill radius, 36px avatar, 8px gap,
        // translucent 6% hover / 8% active fills, and 4-tier preview priority order.
        let is_dark = palette.mode.is_dark();
        let row_height = palette.scaled_px(54.0);
        let row_hover_bg = palette.row_hover_rgba();
        let row_active_bg = palette.row_active_rgba();
        let row_surface_hex = if is_selected {
            if is_dark { 0x1F1F1F } else { 0xE5E5E5 }
        } else {
            palette.dialogs_bg
        };
        let name_fg: Rgba = rgb(palette.text_primary);
        let sub_fg: Rgba = rgb(palette.text_secondary);
        let time_fg: Rgba = rgb(palette.msg_in_date_fg);
        let avatar_size_px = palette.scaled_px(36.0);
        let online_border = rgb(row_surface_hex);
        let avatar = render_dialog_avatar(
            item,
            meta,
            palette,
            avatar_size_px,
            row_surface_hex,
            online_border,
        );

        let preview_priority = resolve_dialog_preview_priority(item, meta, lang_id);
        let is_blocked = matches!(preview_priority, DialogPreviewPriority::BlockedApproval(_));
        let has_unread = item.unread_count > 0 || meta.is_unread_marked;
        let (_, _, warn_text_hex) = TdesktopThemePalette::bot_hue_colors("yellow", is_dark);

        let mut title_row = div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(px(palette.scaled_px(4.0)))
            .overflow_hidden()
            .child(
                div()
                    .truncate()
                    .text_size(px(palette.scaled_px(13.5)))
                    .font_weight(if is_selected || has_unread {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(name_fg)
                    .child(item.peer_name.clone()),
            );
        if meta.is_verified {
            title_row = title_row.child(tg_icon(
                IconName::Verified,
                palette.scaled_px(12.5),
                palette.accent,
            ));
        }
        if meta.is_premium {
            title_row = title_row.child(tg_icon(
                IconName::PremiumStar,
                palette.scaled_px(11.5),
                palette.accent,
            ));
        }
        if item.is_muted {
            title_row = title_row.child(tg_icon_rgba(
                IconName::Mute,
                palette.scaled_px(11.0),
                TdesktopThemePalette::rgba_with_alpha(palette.text_secondary, 0.72),
            ));
        }

        let top_row = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .child(title_row)
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(palette.scaled_px(11.5)))
                    .text_color(time_fg)
                    .child(item.timestamp.clone()),
            );

        let preview_font = px(palette.scaled_px(12.5));
        let preview_line: AnyElement = match preview_priority {
            DialogPreviewPriority::BlockedApproval(detail) => div()
                .w_full()
                .overflow_hidden()
                .flex()
                .items_center()
                .gap(px(3.0))
                .text_size(preview_font)
                .child(
                    div()
                        .flex_shrink_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(warn_text_hex))
                        .child(tr_str(lang_id, "等你处理：", "Needs you: ")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_color(sub_fg)
                        .child(detail),
                )
                .into_any_element(),
            DialogPreviewPriority::Draft(draft) => div()
                .w_full()
                .overflow_hidden()
                .flex()
                .items_center()
                .gap(px(3.0))
                .text_size(preview_font)
                .child(
                    div()
                        .flex_shrink_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(palette.danger))
                        .child(tr_str(lang_id, "草稿：", "Draft: ")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_color(sub_fg)
                        .child(draft),
                )
                .into_any_element(),
            DialogPreviewPriority::LiveActivity(activity) => div()
                .w_full()
                .overflow_hidden()
                .truncate()
                .text_size(preview_font)
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(palette.accent))
                .child(activity)
                .into_any_element(),
            DialogPreviewPriority::Message(text) => {
                if let Some(icon) = media_badge_svg_rgba(preview_ui.media_badge, sub_fg) {
                    div()
                        .w_full()
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(div().flex_shrink_0().child(icon))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(preview_font)
                                .text_color(sub_fg)
                                .child(text),
                        )
                        .into_any_element()
                } else {
                    div()
                        .w_full()
                        .overflow_hidden()
                        .truncate()
                        .text_size(preview_font)
                        .text_color(sub_fg)
                        .child(text)
                        .into_any_element()
                }
            }
        };

        let mut right_aux = div()
            .flex()
            .items_center()
            .gap(px(5.0))
            .flex_shrink_0();
        let mut has_aux = false;

        if let Some(ref open_uri) = meta.miniapp_open_uri {
            has_aux = true;
            let uri_owned = open_uri.clone();
            right_aux = right_aux.child(
                div()
                    .id(("dialog-row-open-miniapp", item.id))
                    .cursor_pointer()
                    .px(px(palette.scaled_px(8.0)))
                    .py(px(palette.scaled_px(2.0)))
                    .rounded_full()
                    .bg(rgb(palette.accent))
                    .text_size(px(palette.scaled_px(10.5)))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xFFFFFF))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            on_open_miniapp(this, uri_owned.clone(), cx);
                        }),
                    )
                    .child(tr_str(lang_id, "打开", "Open")),
            );
        }

        // Trailing attention slot priority (Grok Bot §2.5): blocked amber dot > unread dot/pill
        if is_blocked {
            has_aux = true;
            right_aux = right_aux.child(
                div()
                    .w(px(palette.scaled_px(8.0)))
                    .h(px(palette.scaled_px(8.0)))
                    .rounded_full()
                    .bg(rgb(warn_text_hex)),
            );
        } else if item.unread_count > 1 {
            has_aux = true;
            let (count_bg, count_fg) = if item.is_muted {
                (
                    TdesktopThemePalette::rgba_with_alpha(palette.muted_badge_bg, 0.22),
                    rgb(palette.text_secondary),
                )
            } else {
                (
                    TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.16),
                    rgb(palette.accent),
                )
            };
            right_aux = right_aux.child(
                div()
                    .h(px(palette.scaled_px(16.0)))
                    .min_w(px(palette.scaled_px(16.0)))
                    .px(px(palette.scaled_px(5.0)))
                    .rounded_full()
                    .bg(count_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(palette.scaled_px(10.5)))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(count_fg)
                    .child(format!("{}", pill.count)),
            );
        } else if has_unread {
            has_aux = true;
            let dot_bg = if item.is_muted {
                rgb(palette.muted_badge_bg)
            } else {
                rgb(palette.accent)
            };
            right_aux = right_aux.child(
                div()
                    .w(px(palette.scaled_px(7.5)))
                    .h(px(palette.scaled_px(7.5)))
                    .rounded_full()
                    .bg(dot_bg),
            );
        }

        if meta.topics.as_ref().is_some_and(|t| !t.topics.is_empty()) {
            has_aux = true;
            let chev = if meta.is_topics_expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            right_aux = right_aux.child(
                div()
                    .id(("forum-topics-toggle", item.id))
                    .cursor_pointer()
                    .px(px(4.0))
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.12))
                    .flex()
                    .items_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            on_toggle_topics(this, cx);
                        }),
                    )
                    .child(tg_icon(chev, 10.0, palette.accent)),
            );
        }

        let bottom_row = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .child(div().flex_1().min_w(px(0.0)).child(preview_line))
            .when(has_aux, |el| el.child(right_aux));

        return div()
            .id(("sidebar-dialog-row", item.id))
            .cursor_pointer()
            .h(px(row_height))
            .flex_shrink_0()
            .w_full()
            .px(px(palette.scaled_px(8.0)))
            .py(px(palette.scaled_px(6.0)))
            .rounded(px(palette.scaled_px(10.0)))
            .when(is_selected, |el| el.bg(row_active_bg))
            .hover(move |s| if is_selected { s } else { s.bg(row_hover_bg) })
            .flex()
            .items_center()
            .gap(px(palette.scaled_px(8.0)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev, _w, cx| {
                    on_select(this, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                    on_context(this, ev.position, cx);
                }),
            )
            .child(avatar)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(palette.scaled_px(1.5)))
                    .child(top_row)
                    .child(bottom_row),
            )
            .into_any_element();
    }

    // Legacy Telegram Desktop Classic style path
    let row_height = palette.scaled_px(row_model.height.max(62) as f32);
    let use_inverted_sel = is_selected;
    let row_bg_hex = if is_selected {
        palette.dialogs_active_bg
    } else {
        palette.dialogs_bg
    };
    let row_bg = rgb(row_bg_hex);
    let hover_bg = rgb(palette.dialogs_hover_bg);
    let name_fg = if use_inverted_sel {
        rgb(palette.active_button_fg)
    } else {
        rgb(palette.text_primary)
    };
    let sub_fg_hex = if use_inverted_sel {
        palette.active_button_fg
    } else {
        palette.text_secondary
    };
    let time_fg = if use_inverted_sel {
        rgb(palette.active_button_fg)
    } else {
        rgb(palette.msg_in_date_fg)
    };
    let icon_hex = if use_inverted_sel {
        palette.active_button_fg
    } else {
        palette.icon_fg
    };
    let badge_bg = if use_inverted_sel {
        rgb(palette.active_button_fg)
    } else if item.is_muted {
        rgb(palette.muted_badge_bg)
    } else {
        rgb(palette.unread_badge_bg)
    };
    let badge_fg = if use_inverted_sel {
        rgb(palette.accent)
    } else {
        rgb(palette.unread_badge_fg)
    };
    let avatar_size_px = palette.scaled_px(46.0);
    let avatar = render_dialog_avatar(item, meta, palette, avatar_size_px, row_bg_hex, row_bg);

    let root = div()
        .id(("sidebar-dialog-row", item.id))
        .cursor_pointer()
        .h(px(row_height))
        .flex_shrink_0()
        .w_full()
        .px(px(palette.scaled_px(10.0)))
        .py(px(palette.scaled_px(8.0)))
        .bg(row_bg)
        .hover(move |s| if is_selected { s } else { s.bg(hover_bg) })
        .flex()
        .items_center()
        .gap(px(palette.scaled_px(12.0)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _ev, _w, cx| {
                on_select(this, cx);
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                on_context(this, ev.position, cx);
            }),
        );

    let mut title_row = div()
        .flex()
        .items_center()
        .gap(px(palette.scaled_px(4.0)))
        .overflow_hidden();
    title_row = title_row.child(
        div()
            .text_size(px(palette.scaled_px(14.0)))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(name_fg)
            .child(item.peer_name.clone()),
    );
    if meta.is_verified {
        title_row = title_row.child(tg_icon(
            IconName::Verified,
            palette.scaled_px(14.0),
            if use_inverted_sel {
                palette.active_button_fg
            } else {
                palette.accent
            },
        ));
    }
    if meta.is_premium {
        title_row = title_row.child(tg_icon(
            IconName::PremiumStar,
            palette.scaled_px(13.0),
            if use_inverted_sel {
                palette.active_button_fg
            } else {
                palette.accent
            },
        ));
    }
    if item.is_muted {
        title_row = title_row.child(tg_icon(IconName::Mute, palette.scaled_px(12.0), icon_hex));
    }

    let preview_font = px(palette.scaled_px(13.0));
    let preview_line: AnyElement = if preview_ui.is_draft {
        div()
            .flex()
            .items_center()
            .gap(px(3.0))
            .overflow_hidden()
            .child(
                div()
                    .text_size(preview_font)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(palette.danger))
                    .child(tr_str(lang_id, "草稿:", "Draft:")),
            )
            .child(
                div()
                    .text_size(preview_font)
                    .text_color(rgb(sub_fg_hex))
                    .child(preview_ui.text),
            )
            .into_any_element()
    } else if let Some(ref typing) = item.typing_indicator {
        div()
            .text_size(preview_font)
            .text_color(if use_inverted_sel {
                rgb(palette.active_button_fg)
            } else {
                rgb(palette.accent)
            })
            .child(typing.clone())
            .into_any_element()
    } else {
        let mut line = div().flex().items_center().gap(px(4.0)).overflow_hidden();
        if let Some(icon) = media_badge_svg_rgba(preview_ui.media_badge, rgb(sub_fg_hex)) {
            line = line.child(icon);
        }
        line.child(
            div()
                .text_size(preview_font)
                .text_color(rgb(sub_fg_hex))
                .child(preview_ui.text),
        )
        .into_any_element()
    };

    let mut right_badges = div()
        .flex()
        .items_center()
        .gap(px(palette.scaled_px(5.0)))
        .flex_shrink_0();
    if let Some(ref open_uri) = meta.miniapp_open_uri {
        let uri_owned = open_uri.clone();
        let pill_bg = if use_inverted_sel {
            rgb(palette.active_button_fg)
        } else {
            rgb(palette.accent)
        };
        let pill_fg = if use_inverted_sel {
            rgb(palette.accent)
        } else {
            rgb(0xFFFFFF)
        };
        right_badges = right_badges.child(
            div()
                .id(("dialog-row-open-miniapp", item.id))
                .cursor_pointer()
                .px(px(palette.scaled_px(8.0)))
                .py(px(palette.scaled_px(2.0)))
                .rounded_full()
                .bg(pill_bg)
                .text_size(px(palette.scaled_px(11.0)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(pill_fg)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        on_open_miniapp(this, uri_owned.clone(), cx);
                    }),
                )
                .child(tr_str(lang_id, "打开", "Open")),
        );
    }
    for (tag_idx, tag_label) in meta.folder_tags.iter().enumerate() {
        let display_tag =
            crate::views::sidebar::dialogs_overlays::localize_folder_label(tag_label, lang_id);
        let tag_bg = if use_inverted_sel {
            TdesktopThemePalette::rgba_with_alpha(palette.active_button_fg, 0.22)
        } else {
            TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.16)
        };
        let tag_fg = if use_inverted_sel {
            rgb(palette.active_button_fg)
        } else {
            rgb(palette.accent)
        };
        right_badges = right_badges.child(
            div()
                .id((
                    "dialog-folder-tag",
                    item.id.saturating_mul(10) + tag_idx as u64,
                ))
                .px(px(palette.scaled_px(5.0)))
                .py(px(palette.scaled_px(1.5)))
                .rounded(px(palette.scaled_px(4.0)))
                .bg(tag_bg)
                .text_size(px(palette.scaled_px(10.0)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tag_fg)
                .child(display_tag),
        );
    }
    if item.is_pinned && item.unread_count == 0 && !meta.is_unread_marked {
        right_badges =
            right_badges.child(tg_icon(IconName::Pin, palette.scaled_px(14.0), icon_hex));
    }
    if item.unread_count > 0 {
        let pill_h = px(palette.scaled_px(19.0));
        let pill_pad = px(palette.scaled_px(5.0));
        right_badges = right_badges.child(
            div()
                .h(pill_h)
                .min_w(pill_h)
                .px(pill_pad)
                .rounded_full()
                .bg(badge_bg)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(palette.scaled_px(11.5)))
                .font_weight(FontWeight::BOLD)
                .text_color(badge_fg)
                .child(format!("{}", pill.count)),
        );
    } else if meta.is_unread_marked {
        right_badges = right_badges.child(
            div()
                .w(px(palette.scaled_px(10.0)))
                .h(px(palette.scaled_px(10.0)))
                .rounded_full()
                .bg(badge_bg),
        );
    }

    let top_row = div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.0))
        .child(title_row)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(3.0))
                .flex_shrink_0()
                .when(item.unread_count == 0 && meta.draft_text.is_none(), |el| {
                    el.child(delivery_svg(item.delivery_status, palette, is_selected))
                })
                .child(
                    div()
                        .text_size(px(palette.scaled_px(12.0)))
                        .text_color(time_fg)
                        .child(item.timestamp.clone()),
                ),
        );

    let mut bottom_row = div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.0))
        .child(div().flex_1().overflow_hidden().child(preview_line))
        .child(right_badges);
    if meta.topics.as_ref().is_some_and(|t| !t.topics.is_empty()) {
        let chev = if meta.is_topics_expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };
        bottom_row = bottom_row.child(
            div()
                .id(("forum-topics-toggle", item.id))
                .cursor_pointer()
                .px(px(4.0))
                .py(px(1.0))
                .rounded(px(4.0))
                .bg(rgb(palette.composer_bg))
                .flex()
                .items_center()
                .gap(px(2.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        on_toggle_topics(this, cx);
                    }),
                )
                .child(tg_icon(chev, 10.0, palette.accent)),
        );
    }

    root.child(avatar)
        .child(
            div()
                .flex_1()
                .h_full()
                .overflow_hidden()
                .flex()
                .flex_col()
                .justify_between()
                .child(top_row)
                .child(bottom_row),
        )
        .into_any_element()
}
