//! Complete 1:1 Telegram Desktop Message Row & Bubble Renderer (`stage_bubble`).

use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::components::avatar::render_plush_bot_avatar;
use crate::components::grok_bot_avatar::{render_grok_persona_avatar, GrokPersonaState};
use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::openbot::conversation::compute_message_preview;
use crate::views::openbot::steps_and_trace::{AgentStepEntry, AgentStepKind, STEPS_MAX_HEIGHT};
use super::cards::bot_assist_card::{
    render_bot_assist_card, BotAssistClickDraft, BotAssistLoginDraft, BotAssistLoginField,
};
use super::cards::models::{
    DeliveryStatus, MessageContent, MessageDirection, MessageItemModel,
};
use super::cards_interactive::{
    render_call_card, render_inline_bot_keyboard, render_invoice_card, render_location_card,
    render_poll_card, render_story_card, InteractiveCardAction,
};
use super::cards_media::{
    render_album_mosaic_card, render_media_bubble_card, render_voice_waveform_card, MediaCardAction,
};
use super::document_contact_cards::{
    render_contact_card, render_document_card, render_webpage_preview_card,
};
use super::entities::{parse_rich_blocks, push_sanitized_no_breaks, RichBlock, StyledSegment};
use super::entities_render::{render_rich_message_body, RichTextAction};
use super::history_element::{MSG_MAX_WIDTH, MSG_MIN_WIDTH, MSG_PHOTO_SIZE, MSG_PHOTO_SKIP};
use super::openbot_stage_chrome::{
    normalize_fabushi_plush_spec, openbot_stage_icon, OpenBotStageIcon,
};
use super::reactions::button::{sort_reaction_pills, ReactionKey, ReactionPill};
use super::rich_meta::{format_view_count, resolve_image_source, MessageRichMeta};
use super::stage_context_menu::{
    render_hover_reaction_overlay, ContextMenuAction, ReactionOverlayLayout, ReactionOverlayMode,
};

/// Compact minimum bubble width in Apple Messages style so short messages like "好" / "OK"
/// hug tightly as sleek pills without altering `history_element::MSG_MIN_WIDTH` (`160.0`).
pub const APPLE_MSG_MIN_WIDTH: f32 = 38.0;
pub const APPLE_AVATAR_SIZE: f32 = 32.0;
pub const APPLE_AVATAR_SKIP: f32 = 36.0;
pub const OPENBOT_ENTRY_GAP_PX: f32 = 12.0;
pub const OPENBOT_GROUPED_GAP_PX: f32 = 4.0;
pub const OPENBOT_BUBBLE_RADIUS_OUTER_PX: f32 = 22.0;
pub const OPENBOT_BUBBLE_RADIUS_GROUPED_PX: f32 = 6.0;
pub const OPENBOT_ERROR_PRE_MAX_HEIGHT_PX: f32 = 160.0;
pub const OPENBOT_REPLY_QUOTE_BORDER_PX: f32 = 2.0;
pub const OPENBOT_REPLY_QUOTE_MAX_HEIGHT_PX: f32 = 60.0;

/// Single-source-of-truth error drawer button label (`"Show error"` / `"Hide error"`).
pub fn openbot_error_drawer_btn_label(expanded: bool, lang_id: &str) -> &'static str {
    if expanded {
        tr_str(lang_id, "隐藏错误详情", "Hide error")
    } else {
        tr_str(lang_id, "显示错误详情", "Show error")
    }
}

/// Single-source-of-truth layout snapshot for OpenBot 1:1 chat message bubbles (`UI-F10`).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenBotBubbleLayoutSnapshot {
    pub author: &'static str,
    pub entry_gap_px: f32,
    pub grouped_gap_px: f32,
    pub bg_hex: String,
    pub resolved_text_hex: String,
    pub uses_legacy_blue: bool,
    pub radius_outer_px: f32,
    pub radius_grouped_px: f32,
    pub actions_opacity_rest: f32,
    pub time_opacity_rest: f32,
    pub actions_opacity_hover: f32,
    pub structured_error_quarantined: bool,
    pub contains_raw_json_wall: bool,
    pub error_drawer_open: bool,
    pub error_drawer_btn_label: &'static str,
    pub error_pre_max_height_px: f32,
    pub wide_table_or_code: bool,
    pub pending_error_retry: bool,
}

pub fn compute_openbot_bubble_layout(
    msg: &MessageItemModel,
    _group: BubbleGroupLayout,
    _is_row_hovered: bool,
    error_expanded: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
) -> OpenBotBubbleLayoutSnapshot {
    let is_outgoing = msg.direction == MessageDirection::Outgoing;
    let bg_u32 = if is_outgoing {
        palette.msg_out_bg
    } else {
        palette.msg_in_bg
    };
    let fg_u32 = if is_outgoing {
        palette.msg_out_fg
    } else {
        palette.text_primary
    };
    let bg_hex = format!("#{:06x}", bg_u32 & 0x00FF_FFFF);
    let resolved_text_hex = format!("#{:06x}", fg_u32 & 0x00FF_FFFF);
    let uses_legacy_blue = bg_u32 == 0x007AFF || bg_u32 == 0x0A84FF || bg_u32 == 0x3390EC;
    let raw_text = match &msg.content {
        MessageContent::Text { text, .. } => text.as_str(),
        _ => "",
    };
    let structured_error = if !is_outgoing {
        extract_structured_error_payload(raw_text)
    } else {
        None
    };
    let structured_error_quarantined = structured_error.is_some();
    let contains_raw_json_wall = if structured_error_quarantined {
        false
    } else {
        let trimmed = raw_text.trim();
        trimmed.starts_with("{\"error\"") || trimmed.contains("\"stack_trace\"")
    };
    let wide_table_or_code =
        raw_text.contains("```") || (raw_text.contains('|') && raw_text.contains("---"));
    let pending_error_retry = is_outgoing && msg.delivery_status == DeliveryStatus::Failed;

    OpenBotBubbleLayoutSnapshot {
        author: if is_outgoing { "user" } else { "assistant" },
        entry_gap_px: OPENBOT_ENTRY_GAP_PX,
        grouped_gap_px: OPENBOT_GROUPED_GAP_PX,
        bg_hex,
        resolved_text_hex,
        uses_legacy_blue,
        radius_outer_px: OPENBOT_BUBBLE_RADIUS_OUTER_PX,
        radius_grouped_px: OPENBOT_BUBBLE_RADIUS_GROUPED_PX,
        actions_opacity_rest: 0.0,
        time_opacity_rest: 0.0,
        actions_opacity_hover: 1.0,
        structured_error_quarantined,
        contains_raw_json_wall,
        error_drawer_open: error_expanded,
        error_drawer_btn_label: openbot_error_drawer_btn_label(error_expanded, lang_id),
        error_pre_max_height_px: OPENBOT_ERROR_PRE_MAX_HEIGHT_PX,
        wide_table_or_code,
        pending_error_retry,
    }
}

/// Single-source-of-truth layout snapshot for multi-author OpenBot group (Channel) messages (`UI-F11`).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenBotGroupMessageLayout {
    pub show_author: bool,
    pub bubble_style: &'static str,
    pub author_gutter_px: f32,
    pub author_avatar_px: f32,
    pub author_avatar_clickable: bool,
    pub author_name_clickable: bool,
    pub author_label: String,
    pub author_name_color: String,
    pub consecutive_header_collapsed: bool,
}

pub fn compute_group_message_layout(
    messages: &[MessageItemModel],
    idx: usize,
    sender_hue: Option<openbot_contracts::avatar::AvatarHue>,
    sender_color_override: Option<&str>,
) -> OpenBotGroupMessageLayout {
    let Some(msg) = messages.get(idx) else {
        return OpenBotGroupMessageLayout {
            show_author: true,
            bubble_style: "classic",
            author_gutter_px: APPLE_AVATAR_SKIP,
            author_avatar_px: APPLE_AVATAR_SIZE,
            author_avatar_clickable: false,
            author_name_clickable: false,
            author_label: crate::views::openbot::channel_view::UNAVAILABLE_CHANNEL_AUTHOR_LABEL
                .to_string(),
            author_name_color: openbot_contracts::avatar::avatar_head_color(
                "unavailable",
                sender_hue,
                sender_color_override,
            ),
            consecutive_header_collapsed: false,
        };
    };
    let (attached_prev, _) = crate::views::conversation_stage::compute_attachment(messages, idx);
    let is_outgoing = msg.direction == MessageDirection::Outgoing;
    let author_label = crate::views::openbot::channel_view::resolve_channel_author_label(
        Some(&msg.sender_name),
    )
    .to_string();
    let seed = if msg.sender_name.is_empty() {
        format!("sender-{}", msg.sender_id)
    } else {
        msg.sender_name.clone()
    };
    let author_name_color = openbot_contracts::avatar::avatar_head_color(
        &seed,
        sender_hue,
        sender_color_override,
    );
    OpenBotGroupMessageLayout {
        show_author: !is_outgoing,
        bubble_style: "classic",
        author_gutter_px: APPLE_AVATAR_SKIP,
        author_avatar_px: APPLE_AVATAR_SIZE,
        author_avatar_clickable: !is_outgoing && msg.sender_id > 0,
        author_name_clickable: !is_outgoing && msg.sender_id > 0,
        author_label,
        author_name_color,
        consecutive_header_collapsed: attached_prev,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BubbleGroupLayout {
    pub attached_to_prev: bool,
    pub attached_to_next: bool,
    pub show_sender_name: bool,
    pub show_bottom_avatar: bool,
    pub show_tail: bool,
    pub reserve_avatar_column: bool,
}

#[derive(Debug, Clone)]
pub enum BubbleUiAction {
    ClickReplyQuote(u64),
    ToggleReaction(u64, String),
    ToggleReactionPicker(u64),
    ToggleSelect(u64),
    OpenContextMenu(u64, Point<Pixels>),
    SetHover(Option<u64>),
    SetBubbleHover(u64, bool),
    SetBubbleGutterHover(u64, bool),
    SetReactionBarHover(u64, bool),
    SetReactionBridgeHover(u64, bool),
    Media(MediaCardAction),
    Interactive(InteractiveCardAction),
    Context(ContextMenuAction),
    RichText(RichTextAction),
    DownloadDoc(u64, String, String),
    OpenContact(u64),
    OpenWebUrl(String),
    OpenInstantView(String, String),
    // Additive M3 OpenBot actions (UI-F10 / UI-F11)
    ToggleErrorDetails(u64),
    ToggleAgentSteps(u64),
    ToggleLongMessage(u64),
    CopyMessageText(u64),
    CopyRawText(String),
    OpenSenderDm {
        user_id: u64,
        sender_name: String,
    },
    RetryFailedMessage(u64),
}

/// Optional OpenBot-specific per-row context passed into `render_message_row_openbot`.
#[derive(Clone, Copy, Default)]
pub struct OpenBotBubbleExtra<'a> {
    pub is_group_chat: bool,
    pub is_direct_human: bool,
    pub sender_hue_name: Option<&'a str>,
    pub sender_is_bot: bool,
    pub error_expanded: bool,
    pub steps_expanded: bool,
    pub long_expanded: bool,
    pub turn_steps: &'a [AgentStepEntry],
}

/// Parsed provider / tool / raw JSON error payload rendered as a collapsible OpenBot failed step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredErrorCard {
    pub summary_title: String,
    pub concise_reason: String,
    pub raw_payload: String,
    pub is_json: bool,
}

fn make_row_height_probe_canvas(
    msg_id: u64,
    probe: Rc<RefCell<HashMap<u64, f32>>>,
) -> impl IntoElement {
    canvas(
        move |bounds, _window, _cx| {
            let h = f32::from(bounds.size.height);
            if h > 1.0 {
                probe.borrow_mut().insert(msg_id, h);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn measure_single_line_width_px(
    text: &str,
    font_px: f32,
    weight: FontWeight,
    window: &Window,
) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let mut clean = String::new();
    push_sanitized_no_breaks(&mut clean, text);
    if clean.is_empty() {
        return 0.0;
    }
    let mut style = window.text_style();
    style.font_weight = weight;
    let run = style.to_run(clean.len());
    let shaped = window
        .text_system()
        .shape_line(SharedString::from(clean), px(font_px), &[run], None);
    f32::from(shaped.width).ceil()
}

fn measure_rich_line_width_px(
    segs: &[StyledSegment],
    font_px: f32,
    default_weight: FontWeight,
    revealed_spoilers: &HashSet<(u64, usize)>,
    msg_id: u64,
    window: &Window,
) -> f32 {
    if segs.is_empty() {
        return 0.0;
    }
    let base_style = window.text_style();
    let mut full = String::new();
    let mut runs = Vec::new();
    for (seg_idx, seg) in segs.iter().enumerate() {
        let start = full.len();
        if seg.flags.spoiler {
            let sid = seg.spoiler_id.unwrap_or(seg_idx);
            if revealed_spoilers.contains(&(msg_id, sid)) {
                push_sanitized_no_breaks(&mut full, &seg.text);
            } else {
                let mask_len = seg.text.chars().count().clamp(4, 18);
                for _ in 0..mask_len {
                    full.push('░');
                }
            }
        } else if seg.flags.code {
            let mut clean = String::new();
            push_sanitized_no_breaks(&mut clean, &seg.text);
            if !clean.is_empty() {
                full.push(' ');
                full.push_str(&clean);
                full.push(' ');
            }
        } else {
            push_sanitized_no_breaks(&mut full, &seg.text);
        }
        let seg_len = full.len() - start;
        if seg_len > 0 {
            let mut style = base_style.clone();
            style.font_weight = if seg.flags.bold {
                FontWeight::BOLD
            } else if seg.flags.code && default_weight == FontWeight::NORMAL {
                FontWeight::MEDIUM
            } else {
                default_weight
            };
            if seg.flags.italic {
                style.font_style = FontStyle::Italic;
            }
            runs.push(style.to_run(seg_len));
        }
    }
    if full.is_empty() || runs.is_empty() {
        return 0.0;
    }
    let shaped = window
        .text_system()
        .shape_line(SharedString::from(full), px(font_px), &runs, None);
    f32::from(shaped.width).ceil()
}

#[allow(clippy::too_many_arguments)]
fn compute_definite_bubble_width(
    msg: &MessageItemModel,
    meta: Option<&MessageRichMeta>,
    group: BubbleGroupLayout,
    is_outgoing: bool,
    revealed_spoilers: &HashSet<(u64, usize)>,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    eff_min_w: f32,
    eff_max_w: f32,
    window: &Window,
) -> Option<f32> {
    if meta.is_some_and(|m| m.contact.is_some() || m.document.is_some()) {
        return None;
    }
    match &msg.content {
        MessageContent::Text { .. } | MessageContent::BotKeyboard { .. } => {}
        _ => return None,
    }

    let is_apple = palette.is_apple_style();
    let pad_base = if is_apple { 26.0 } else { 22.0 };
    let horiz_pad = palette.scaled_px(pad_base) + if is_apple { 4.0 } else { 6.0 };
    let mut needed_w = eff_min_w;

    let is_zh = lang_id.starts_with("zh");
    if !is_apple {
        let meta_font = palette.scaled_px(10.5);
        let is_edited = matches!(msg.content, MessageContent::Text { is_edited: true, .. })
            || meta.and_then(|m| m.edit_date.as_ref()).is_some();
        let views = meta.and_then(|m| m.views_count);
        let mut meta_w = measure_single_line_width_px(&msg.timestamp, meta_font, FontWeight::NORMAL, window)
            + horiz_pad
            + 6.0;
        if is_outgoing {
            meta_w += palette.scaled_px(18.0);
        }
        if is_edited {
            let elabel = if is_zh { "已编辑" } else { "edited" };
            meta_w += measure_single_line_width_px(elabel, meta_font, FontWeight::NORMAL, window)
                + palette.scaled_px(5.0);
        }
        if let Some(vc) = views {
            let vlabel = if is_zh {
                format!("{} 次浏览", format_view_count(vc))
            } else {
                format!("{} views", format_view_count(vc))
            };
            meta_w += measure_single_line_width_px(&vlabel, meta_font, FontWeight::NORMAL, window)
                + palette.scaled_px(5.0);
        }
        needed_w = needed_w.max(meta_w);

        if group.show_sender_name && !is_outgoing {
            needed_w = needed_w.max(
                measure_single_line_width_px(&msg.sender_name, 12.0, FontWeight::BOLD, window) + horiz_pad,
            );
        }
    }
    if let Some(fwd) = meta.and_then(|m| m.forward_from.as_ref()) {
        let fwd_label = if is_zh {
            format!("转发自 {}", fwd.sender_name)
        } else {
            format!("Forwarded from {}", fwd.sender_name)
        };
        let fwd_w = measure_single_line_width_px(
            &fwd_label,
            palette.scaled_px(11.5),
            FontWeight::SEMIBOLD,
            window,
        ) + palette.scaled_px(16.0)
            + horiz_pad;
        needed_w = needed_w.max(fwd_w);
    }
    if let Some(reply) = &msg.reply_to {
        let reply_w = measure_single_line_width_px(&reply.sender_name, 11.5, FontWeight::BOLD, window)
            .max(measure_single_line_width_px(&reply.snippet, 11.5, FontWeight::NORMAL, window))
            + 20.0
            + horiz_pad;
        needed_w = needed_w.max(reply_w);
    }
    if !is_apple && !msg.reactions.is_empty() {
        let rx_w = (msg.reactions.len() as f32) * 52.0 + horiz_pad;
        needed_w = needed_w.max(rx_w);
    }

    let kb_rows_opt = match &msg.content {
        MessageContent::BotKeyboard { rows } => Some(rows.as_slice()),
        _ => msg.bot_keyboard.as_deref(),
    };
    if let Some(kb_rows) = kb_rows_opt {
        for r in kb_rows {
            if r.is_empty() {
                continue;
            }
            let row_sum: f32 = r
                .iter()
                .map(|btn| {
                    measure_single_line_width_px(&btn.text, 12.0, FontWeight::SEMIBOLD, window)
                        + 28.0
                })
                .sum::<f32>()
                + ((r.len().saturating_sub(1)) as f32 * 4.0);
            needed_w = needed_w.max(row_sum);
            if needed_w >= eff_max_w {
                return Some(eff_max_w);
            }
        }
    }

    if let MessageContent::Text { text, .. } = &msg.content {
        if meta.is_some_and(|m| m.webpage.is_some() || m.show_translation) {
            return Some(eff_max_w);
        }
        let explicit_entities = meta.map(|m| m.entities.as_slice()).unwrap_or(&[]);
        let blocks = parse_rich_blocks(text, explicit_entities);
        let font_px = palette.scaled_px(palette.font_size_px as f32);
        let q_font = palette.scaled_px((palette.font_size_px as f32 - 0.5).max(12.0));

        for block in blocks {
            match block {
                RichBlock::PreCode { .. } | RichBlock::Divider | RichBlock::Table { .. } => {
                    return Some(eff_max_w)
                }
                RichBlock::Paragraph(segs) => {
                    let line_w = measure_rich_line_width_px(
                        &segs,
                        font_px,
                        FontWeight::NORMAL,
                        revealed_spoilers,
                        msg.id,
                        window,
                    ) + horiz_pad;
                    needed_w = needed_w.max(line_w);
                }
                RichBlock::Heading { level, segments } => {
                    let scale = match level {
                        1 => 1.24,
                        2 => 1.16,
                        3 => 1.09,
                        _ => 1.03,
                    };
                    let h_px = (font_px * scale).round();
                    let line_w = measure_rich_line_width_px(
                        &segments,
                        h_px,
                        FontWeight::BOLD,
                        revealed_spoilers,
                        msg.id,
                        window,
                    ) + horiz_pad;
                    needed_w = needed_w.max(line_w);
                }
                RichBlock::ListItem { bullet, indent, segments } => {
                    let b_w = measure_single_line_width_px(&bullet, font_px, FontWeight::SEMIBOLD, window);
                    let s_w = measure_rich_line_width_px(
                        &segments,
                        font_px,
                        FontWeight::NORMAL,
                        revealed_spoilers,
                        msg.id,
                        window,
                    );
                    let line_w = ((indent as f32) * 12.0) + b_w + 6.0 + s_w + horiz_pad + 4.0;
                    needed_w = needed_w.max(line_w);
                }
                RichBlock::Blockquote { lines, collapsed_default, .. } => {
                    let can_collapse = lines.len() > 4;
                    let extra = palette.scaled_px(22.0) + if can_collapse { 20.0 } else { 0.0 };
                    let visible = if collapsed_default && can_collapse {
                        &lines[..4]
                    } else {
                        &lines[..]
                    };
                    for q_line in visible {
                        let line_w = measure_rich_line_width_px(
                            q_line,
                            q_font,
                            FontWeight::NORMAL,
                            revealed_spoilers,
                            msg.id,
                            window,
                        ) + extra
                            + horiz_pad;
                        needed_w = needed_w.max(line_w);
                        if needed_w >= eff_max_w {
                            return Some(eff_max_w);
                        }
                    }
                }
            }
            if needed_w >= eff_max_w {
                return Some(eff_max_w);
            }
        }
    }

    Some(needed_w.clamp(eff_min_w, eff_max_w).ceil())
}

#[derive(Clone, Copy)]
pub struct BotAssistRenderContext<'a> {
    pub login_drafts: &'a HashMap<String, BotAssistLoginDraft>,
    pub click_drafts: &'a HashMap<String, BotAssistClickDraft>,
    pub active_field: Option<&'a (String, BotAssistLoginField)>,
    pub ime_preedit: Option<&'a str>,
}

#[allow(clippy::too_many_arguments)]
pub fn render_message_row_1to1<V: 'static>(
    msg: &MessageItemModel,
    meta: Option<&MessageRichMeta>,
    active_fabushi_bot: Option<&FabushiBotSummaryDto>,
    group: BubbleGroupLayout,
    stage_w: f32,
    is_selected: bool,
    has_any_selection: bool,
    reaction_overlay_mode: Option<ReactionOverlayMode>,
    reaction_overlay_layout: ReactionOverlayLayout,
    bubble_widths_probe: Option<Rc<RefCell<HashMap<u64, f32>>>>,
    bubble_bounds_probe: Option<Rc<RefCell<HashMap<u64, Bounds<Pixels>>>>>,
    row_heights_probe: Option<Rc<RefCell<HashMap<u64, f32>>>>,
    highlight_alpha: f32,
    is_unread_anchor: bool,
    date_divider_label: Option<&str>,
    revealed_spoilers: &HashSet<(u64, usize)>,
    expanded_quotes: &HashSet<(u64, usize)>,
    copied_code_msg: Option<u64>,
    bot_assist: Option<BotAssistRenderContext<'_>>,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    window: &Window,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, BubbleUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    render_message_row_apple(
        msg,
        meta,
        active_fabushi_bot,
        group,
        stage_w,
        is_selected,
        has_any_selection,
        false,
        false,
        reaction_overlay_mode,
        reaction_overlay_layout,
        bubble_widths_probe,
        bubble_bounds_probe,
        row_heights_probe,
        highlight_alpha,
        is_unread_anchor,
        date_divider_label,
        revealed_spoilers,
        expanded_quotes,
        copied_code_msg,
        bot_assist,
        palette,
        lang_id,
        window,
        cx,
        on_action,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_message_row_apple<V: 'static>(
    msg: &MessageItemModel,
    meta: Option<&MessageRichMeta>,
    active_fabushi_bot: Option<&FabushiBotSummaryDto>,
    group: BubbleGroupLayout,
    stage_w: f32,
    is_selected: bool,
    has_any_selection: bool,
    is_last_outgoing: bool,
    is_row_hovered: bool,
    reaction_overlay_mode: Option<ReactionOverlayMode>,
    reaction_overlay_layout: ReactionOverlayLayout,
    bubble_widths_probe: Option<Rc<RefCell<HashMap<u64, f32>>>>,
    bubble_bounds_probe: Option<Rc<RefCell<HashMap<u64, Bounds<Pixels>>>>>,
    row_heights_probe: Option<Rc<RefCell<HashMap<u64, f32>>>>,
    highlight_alpha: f32,
    is_unread_anchor: bool,
    date_divider_label: Option<&str>,
    revealed_spoilers: &HashSet<(u64, usize)>,
    expanded_quotes: &HashSet<(u64, usize)>,
    copied_code_msg: Option<u64>,
    bot_assist: Option<BotAssistRenderContext<'_>>,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    window: &Window,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, BubbleUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    render_message_row_openbot(
        msg,
        meta,
        active_fabushi_bot,
        group,
        stage_w,
        is_selected,
        has_any_selection,
        is_last_outgoing,
        is_row_hovered,
        reaction_overlay_mode,
        reaction_overlay_layout,
        bubble_widths_probe,
        bubble_bounds_probe,
        row_heights_probe,
        highlight_alpha,
        is_unread_anchor,
        date_divider_label,
        revealed_spoilers,
        expanded_quotes,
        copied_code_msg,
        bot_assist,
        OpenBotBubbleExtra::default(),
        palette,
        lang_id,
        window,
        cx,
        on_action,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_message_row_openbot<V: 'static>(
    msg: &MessageItemModel,
    meta: Option<&MessageRichMeta>,
    active_fabushi_bot: Option<&FabushiBotSummaryDto>,
    group: BubbleGroupLayout,
    stage_w: f32,
    is_selected: bool,
    has_any_selection: bool,
    is_last_outgoing: bool,
    is_row_hovered: bool,
    reaction_overlay_mode: Option<ReactionOverlayMode>,
    reaction_overlay_layout: ReactionOverlayLayout,
    bubble_widths_probe: Option<Rc<RefCell<HashMap<u64, f32>>>>,
    bubble_bounds_probe: Option<Rc<RefCell<HashMap<u64, Bounds<Pixels>>>>>,
    row_heights_probe: Option<Rc<RefCell<HashMap<u64, f32>>>>,
    highlight_alpha: f32,
    is_unread_anchor: bool,
    date_divider_label: Option<&str>,
    revealed_spoilers: &HashSet<(u64, usize)>,
    expanded_quotes: &HashSet<(u64, usize)>,
    copied_code_msg: Option<u64>,
    bot_assist: Option<BotAssistRenderContext<'_>>,
    extra: OpenBotBubbleExtra<'_>,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    window: &Window,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, BubbleUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let msg_id = msg.id;
    let is_outgoing = msg.direction == MessageDirection::Outgoing;
    let is_apple = palette.is_apple_style();
    let ob = palette.openbot_palette();
    let is_dark = palette.mode.is_dark();

    if let MessageContent::Service { text, .. } = &msg.content {
        if is_apple {
            let act_dm = on_action.clone();
            let sender_uid = msg.sender_id;
            let sender_name = msg.sender_name.clone();
            let has_agent_pill = sender_uid > 0
                && !sender_name.trim().is_empty()
                && (text.contains("Assigned")
                    || text.contains("分配")
                    || text.contains("转交")
                    || text.contains("joined")
                    || text.contains("加入")
                    || text.contains("asked")
                    || text.contains("已询问"));
            let pill_hover = ob.hover();
            return div()
                .id(SharedString::from(format!("svc-row-{}", msg_id)))
                .relative()
                .w_full()
                .py(px(10.0))
                .px(px(16.0))
                .flex_shrink_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .max_w(px(680.0))
                        .min_h(px(22.0))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_muted())
                                .child(text.clone()),
                        )
                        .when(has_agent_pill, |el| {
                            let click_name = sender_name.clone();
                            el.child(
                                div()
                                    .id(SharedString::from(format!("svc-agent-pill-{}", msg_id)))
                                    .h(px(22.0))
                                    .px(px(7.0))
                                    .rounded_full()
                                    .bg(ob.bg_surface())
                                    .border_1()
                                    .border_color(ob.border_hairline())
                                    .hover(move |s| s.bg(pill_hover))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            act_dm(
                                                this,
                                                BubbleUiAction::OpenSenderDm {
                                                    user_id: sender_uid,
                                                    sender_name: click_name.clone(),
                                                },
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(render_plush_bot_avatar(
                                        &sender_name,
                                        None,
                                        None,
                                        None,
                                        15.0,
                                        openbot_contracts::avatar::AvatarMood::Idle,
                                        None,
                                    ))
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(ob.text_primary())
                                            .child(sender_name),
                                    ),
                            )
                        }),
                )
                .when_some(row_heights_probe, |el, probe| {
                    el.child(make_row_height_probe_canvas(msg_id, probe))
                })
                .into_any_element();
        }
        return div()
            .id(SharedString::from(format!("svc-row-{}", msg_id)))
            .relative()
            .w_full()
            .py(px(8.0))
            .flex_shrink_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .px(px(14.0))
                    .py(px(4.0))
                    .rounded_full()
                    .bg(palette.service_pill_rgba())
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(palette.service_pill_fg))
                    .child(text.clone()),
            )
            .when_some(row_heights_probe, |el, probe| {
                el.child(make_row_height_probe_canvas(msg_id, probe))
            })
            .into_any_element();
    }

    let bubble_layout =
        compute_openbot_bubble_layout(msg, group, is_row_hovered, extra.error_expanded, palette, lang_id);
    let is_apple_out = is_apple && is_outgoing;
    let eff_r = if is_apple {
        bubble_layout.radius_outer_px
    } else {
        palette.bubble_radius_px as f32
    };
    let r_lg = if is_apple { eff_r } else { palette.scaled_px(eff_r) };
    let r_sm = if is_apple {
        bubble_layout.radius_grouped_px
    } else {
        palette.scaled_px(4.0)
    };
    let (tl, tr, br, bl) = if is_outgoing {
        (
            r_lg,
            if group.attached_to_prev { r_sm } else { r_lg },
            if group.attached_to_next {
                r_sm
            } else if group.show_tail && !is_apple {
                2.0
            } else {
                r_lg
            },
            r_lg,
        )
    } else {
        (
            if group.attached_to_prev { r_sm } else { r_lg },
            r_lg,
            r_lg,
            if group.attached_to_next {
                r_sm
            } else if group.show_tail && !is_apple {
                2.0
            } else {
                r_lg
            },
        )
    };

    let is_local_bot_card = matches!(msg.content, MessageContent::LocalBotAssistCard(_));
    let structured_error = if is_apple && !is_outgoing {
        match &msg.content {
            MessageContent::Text { text, .. } => extract_structured_error_payload(text),
            _ => None,
        }
    } else {
        None
    };
    let is_apple_fullbleed_media = is_apple
        && !is_local_bot_card
        && structured_error.is_none()
        && meta.map_or(true, |m| {
            m.document.is_none() && m.contact.is_none() && m.forward_from.is_none()
        })
        && msg.reply_to.is_none()
        && matches!(
            &msg.content,
            MessageContent::Media { .. } | MessageContent::Album { .. }
        );
    let is_apple_media_no_caption = is_apple_fullbleed_media
        && match &msg.content {
            MessageContent::Media { caption, .. } | MessageContent::Album { caption, .. } => {
                caption.as_deref().map_or(true, |c| c.trim().is_empty())
            }
            _ => false,
        };

    let bubble_hex = if is_local_bot_card {
        if is_selected {
            0x1E293B
        } else {
            0x151922
        }
    } else {
        match (is_outgoing, is_selected) {
            (true, true) => palette.msg_out_selected_bg,
            (true, false) => palette.msg_out_bg,
            (false, true) => palette.msg_in_selected_bg,
            (false, false) => palette.msg_in_bg,
        }
    };

    let body_el = if let Some(ref err_card) = structured_error {
        render_structured_error_step_card(
            msg_id,
            err_card,
            extra.error_expanded,
            palette,
            lang_id,
            cx,
            on_action.clone(),
        )
    } else {
        render_bubble_inner_content_ext(
            msg,
            meta,
            is_outgoing,
            extra.long_expanded,
            revealed_spoilers,
            expanded_quotes,
            copied_code_msg,
            bot_assist,
            palette,
            lang_id,
            cx,
            on_action.clone(),
        )
    };

    let eff_min_w = palette.scaled_px(if is_apple {
        APPLE_MSG_MIN_WIDTH
    } else {
        MSG_MIN_WIDTH
    });
    let avatar_skip = if !is_outgoing && group.reserve_avatar_column {
        if is_apple {
            compute_group_message_layout(std::slice::from_ref(msg), 0, None, None).author_gutter_px
        } else {
            MSG_PHOTO_SKIP
        }
    } else {
        0.0
    };
    let sel_skip = if has_any_selection { 32.0 } else { 0.0 };
    let stream_container_w = if is_apple && extra.is_direct_human {
        stage_w.min(760.0)
    } else if is_apple {
        stage_w.clamp(360.0, 1440.0)
    } else {
        stage_w
    };
    let avail_w = (stream_container_w - 32.0 - avatar_skip - sel_skip).max(eff_min_w);
    let eff_max_w = if is_apple {
        if extra.is_direct_human {
            (avail_w * 0.78).min(560.0).max(eff_min_w)
        } else if is_outgoing {
            (avail_w * 0.92)
                .min((avail_w * 0.50).max(420.0))
                .min(800.0)
                .max(eff_min_w)
        } else {
            avail_w
                .min((avail_w * 0.70).max(720.0))
                .min(1200.0)
                .max(eff_min_w)
        }
    } else {
        let classic_avail = (stage_w - 24.0 - 32.0 - 56.0 - avatar_skip - sel_skip).max(eff_min_w);
        palette.scaled_px(MSG_MAX_WIDTH).min(classic_avail).max(eff_min_w)
    };
    let definite_w = if structured_error.is_some() {
        Some(eff_max_w.min(640.0).max(320.0))
    } else {
        compute_definite_bubble_width(
            msg,
            meta,
            group,
            is_outgoing,
            revealed_spoilers,
            palette,
            lang_id,
            eff_min_w,
            eff_max_w,
            window,
        )
    };

    let act_rclick = on_action.clone();
    let act_dbl = on_action.clone();
    let act_sel = on_action.clone();
    let act_hover = on_action.clone();
    let mut bubble = div()
        .id(SharedString::from(format!("bubble-{}", msg_id)))
        .relative()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .when_some(definite_w, |el, bw| {
            el.w(px(bw)).min_w(px(bw)).max_w(px(bw))
        })
        .when(definite_w.is_none(), |el| {
            el.min_w(px(eff_min_w)).max_w(px(eff_max_w))
        })
        .when(is_apple_media_no_caption || structured_error.is_some(), |el| {
            el.px(px(0.0)).py(px(0.0))
        })
        .when(is_apple_fullbleed_media && !is_apple_media_no_caption, |el| {
            el.px(px(0.0)).pt(px(0.0)).pb(px(palette.scaled_px(8.0)))
        })
        .when(!is_apple_fullbleed_media && structured_error.is_none(), |el| {
            el.px(px(if is_apple { 12.0 } else { palette.scaled_px(11.0) }))
                .py(px(if is_apple { 8.0 } else { palette.scaled_px(7.0) }))
        })
        .rounded_tl(px(tl))
        .rounded_tr(px(tr))
        .rounded_br(px(br))
        .rounded_bl(px(bl));

    bubble = if (is_apple_media_no_caption || structured_error.is_some()) && !is_selected {
        bubble
    } else if is_apple && !is_local_bot_card {
        bubble
            .bg(rgb(bubble_hex))
            .when(!is_dark && !is_selected, |el| {
                el.border_1().border_color(ob.border_hairline())
            })
    } else {
        bubble.bg(rgb(bubble_hex))
    };

    bubble = bubble
        .when(is_local_bot_card && !is_selected && highlight_alpha <= 0.01, |el| {
            el.border_1().border_color(rgb(0x2A3140))
        })
        .when(is_selected, |el| el.border_1().border_color(rgb(palette.accent)))
        .when(highlight_alpha > 0.01, |el| {
            el.border_2().border_color(TdesktopThemePalette::rgba_with_alpha(palette.accent, highlight_alpha))
        })
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
            if has_any_selection {
                cx.stop_propagation();
                act_dbl(this, BubbleUiAction::ToggleSelect(msg_id), cx);
            } else if ev.click_count == 2 {
                cx.stop_propagation();
                act_dbl(this, BubbleUiAction::ToggleReaction(msg_id, "❤️".to_string()), cx);
            }
        }))
        .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
            act_rclick(this, BubbleUiAction::OpenContextMenu(msg_id, ev.position), cx);
        }))
        .on_hover(cx.listener(move |this, hovered, _, cx| {
            act_hover(this, BubbleUiAction::SetBubbleHover(msg_id, *hovered), cx);
        }));

    if !is_apple && group.show_sender_name && !is_outgoing {
        let name_hex = palette.peer_name_color(msg.sender_id as i64);
        bubble = bubble.child(
            div()
                .mb(px(2.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(name_hex))
                .child(msg.sender_name.clone()),
        );
    }

    if let Some(fwd) = meta.and_then(|m| m.forward_from.as_ref()) {
        let fwd_label = if lang_id.starts_with("zh") {
            format!("转发自 {}", fwd.sender_name)
        } else {
            format!("Forwarded from {}", fwd.sender_name)
        };
        let fwd_color = if is_apple_out {
            palette.msg_out_secondary_fg
        } else if is_apple {
            palette.text_secondary
        } else {
            palette.accent
        };
        let fwd_weight = if is_apple { FontWeight::MEDIUM } else { FontWeight::SEMIBOLD };
        bubble = bubble.child(
            div()
                .mb(px(palette.scaled_px(3.0)))
                .flex()
                .items_center()
                .gap(px(palette.scaled_px(4.0)))
                .child(tg_icon(IconName::Forward, palette.scaled_px(12.0), fwd_color))
                .child(
                    div()
                        .text_size(px(palette.scaled_px(11.5)))
                        .font_weight(fwd_weight)
                        .text_color(rgb(fwd_color))
                        .child(fwd_label),
                ),
        );
    }

    if let Some(reply) = &msg.reply_to {
        let target_id = reply.message_id;
        let act_reply = on_action.clone();
        let reply_bg = if is_apple {
            ob.hover()
        } else {
            TdesktopThemePalette::rgba_with_alpha(palette.reply_bar_bg, 0.30)
        };
        let reply_bar = if is_apple {
            if is_dark { 0x6A6A6A } else { 0x8E8E93 }
        } else {
            palette.reply_bar_fg
        };
        let reply_sender_fg = if is_apple {
            palette.text_primary
        } else {
            palette.reply_bar_fg
        };
        let reply_snippet_fg = if is_apple {
            palette.text_secondary
        } else {
            palette.text_secondary
        };
        bubble = bubble.child(
            div()
                .id(SharedString::from(format!("reply-hdr-{}", msg_id)))
                .w_full()
                .min_w(px(0.0))
                .when(is_apple, |el| el.max_h(px(OPENBOT_REPLY_QUOTE_MAX_HEIGHT_PX)))
                .overflow_hidden()
                .mb(px(5.0))
                .pl(px(8.0))
                .pr(px(8.0))
                .py(px(4.0))
                .rounded(px(if is_apple { 8.0 } else { 6.0 }))
                .bg(reply_bg)
                .border_l_2()
                .border_color(rgb(reply_bar))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    act_reply(this, BubbleUiAction::ClickReplyQuote(target_id), cx);
                }))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(if is_apple { FontWeight::SEMIBOLD } else { FontWeight::BOLD })
                        .text_color(rgb(reply_sender_fg))
                        .truncate()
                        .child(reply.sender_name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(reply_snippet_fg))
                        .truncate()
                        .child(reply.snippet.clone()),
                ),
        );
    }

    bubble = bubble.child(body_el);
    if !is_apple {
        bubble = bubble.child(render_bottom_meta_strip(msg, meta, is_outgoing, palette, lang_id));
    }

    if has_any_selection {
        let act_shield = on_action.clone();
        bubble = bubble.child(
            div()
                .id(SharedString::from(format!("bubble-select-shield-{}", msg_id)))
                .absolute()
                .inset_0()
                .size_full()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        act_shield(this, BubbleUiAction::ToggleSelect(msg_id), cx);
                    }),
                ),
        );
    }

    if !msg.reactions.is_empty() {
        // Sort reaction pills per upstream `history_view_reactions.cpp:L177-L232`
        let raw_pills: Vec<ReactionPill> = msg
            .reactions
            .iter()
            .enumerate()
            .map(|(catalog_index, rx)| {
                let key = if rx.emoji == "⭐" {
                    ReactionKey::Paid
                } else {
                    ReactionKey::Emoji(rx.emoji.clone())
                };
                let mut pill = ReactionPill::new(key, rx.count as i32);
                pill.chosen = rx.user_reacted;
                pill.catalog_index = catalog_index;
                pill
            })
            .collect();
        let sorted_pills = sort_reaction_pills(raw_pills);

        if is_apple {
            let mut tapbacks = div()
                .absolute()
                .top(px(-12.0))
                .when(is_outgoing, |el| el.left(px(-10.0)))
                .when(!is_outgoing, |el| el.right(px(-10.0)))
                .flex()
                .items_center()
                .gap(px(3.0));
            for pill in sorted_pills {
                let em = pill.id.display_label();
                let act_rx = on_action.clone();
                let is_chosen = pill.chosen;
                let is_paid = pill.paid;
                let bg_hex = if is_paid {
                    0xFF9F0A
                } else if is_chosen {
                    if is_dark { 0x333336 } else { 0x1C1C1E }
                } else if is_dark {
                    0x242424
                } else {
                    0xEFEFF2
                };
                let count_fg = if is_chosen || is_paid {
                    0xFFFFFF
                } else {
                    palette.text_primary
                };
                let show_count = pill.count > 1;
                tapbacks = tapbacks.child(
                    div()
                        .id(SharedString::from(format!("rx-pill-{}-{}", msg_id, em)))
                        .h(px(26.0))
                        .min_w(px(26.0))
                        .px(px(if show_count { 7.0 } else { 5.0 }))
                        .rounded_full()
                        .bg(rgb(bg_hex))
                        .border_2()
                        .border_color(rgb(palette.stage_bg))
                        .shadow(palette.popover_shadows())
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(3.0))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                act_rx(this, BubbleUiAction::ToggleReaction(msg_id, em.clone()), cx);
                            }),
                        )
                        .text_size(px(12.5))
                        .child(pill.id.display_label())
                        .when(show_count, |el| {
                            el.child(
                                div()
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(count_fg))
                                    .child(format!("{}", pill.count)),
                            )
                        }),
                );
            }
            bubble = bubble.child(tapbacks);
        } else {
            let mut rx_bar = div().flex().flex_wrap().gap(px(4.0)).mt(px(5.0));
            for pill in sorted_pills {
                let em = pill.id.display_label();
                let act_rx = on_action.clone();
                let is_chosen = pill.chosen;
                let is_paid = pill.paid;
                let bg_color: Rgba = if is_paid {
                    rgba(0xFFA5002E).into()
                } else if is_chosen {
                    rgb(palette.reaction_active_bg).into()
                } else {
                    rgb(palette.reaction_bg).into()
                };
                let fg_hex = if is_chosen && !is_paid {
                    0xFFFFFF
                } else if is_paid {
                    0xD97706
                } else {
                    palette.text_primary
                };
                rx_bar = rx_bar.child(
                    div()
                        .id(SharedString::from(format!("rx-pill-{}-{}", msg_id, em)))
                        .h(px(24.0))
                        .px(px(8.0))
                        .rounded_full()
                        .bg(bg_color)
                        .when(is_chosen, |el| el.border_1().border_color(rgb(palette.accent)))
                        .when(is_paid && !is_chosen, |el| el.border_1().border_color(rgb(0xFFA500)))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                act_rx(this, BubbleUiAction::ToggleReaction(msg_id, em.clone()), cx);
                            }),
                        )
                        .text_size(px(12.0))
                        .child(pill.id.display_label())
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(fg_hex))
                                .child(format!("{}", pill.count)),
                        ),
                );
            }
            bubble = bubble.child(rx_bar);
        }
    }

    if group.show_tail && !is_apple {
        let tail_el = if is_outgoing {
            div().absolute().bottom(px(0.0)).right(px(-6.0)).child(tg_icon(IconName::BubbleTailOut, 12.0, bubble_hex))
        } else {
            div().absolute().bottom(px(0.0)).left(px(-6.0)).child(tg_icon(IconName::BubbleTailIn, 12.0, bubble_hex))
        };
        bubble = bubble.child(tail_el);
    }

    if bubble_widths_probe.is_some() || bubble_bounds_probe.is_some() {
        bubble = bubble.child(
            canvas(
                move |bounds, _window, _cx| {
                    let w = f32::from(bounds.size.width);
                    if w > 1.0 {
                        if let Some(ref probe) = bubble_widths_probe {
                            probe.borrow_mut().insert(msg_id, w);
                        }
                        if let Some(ref bounds_probe) = bubble_bounds_probe {
                            bounds_probe.borrow_mut().insert(msg_id, bounds);
                        }
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
            .size_full(),
        );
    }

    // Full-height invisible outer side corridor (`[W, W+38] × [0, H]` incoming / `[-38, 0] × [0, H]`
    // outgoing, strictly within `stack`'s `56px` side margin and `[0, H]` row height) so diagonal
    // cursor movement from the upper/middle part of multi-line messages toward `CornerButton` never
    // drops hover (`history_view_reactions_button.cpp`).
    if !has_any_selection {
        let act_gutter = on_action.clone();
        bubble = bubble.child(
            div()
                .id(SharedString::from(format!("bubble-gutter-{}", msg_id)))
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(38.0))
                .when(is_outgoing, |el| el.left(px(-38.0)))
                .when(!is_outgoing, |el| el.right(px(-38.0)))
                .on_hover(cx.listener(move |this, hovered, _, cx| {
                    act_gutter(this, BubbleUiAction::SetBubbleGutterHover(msg_id, *hovered), cx);
                })),
        );
    }

    let show_hover_rx_overlay = reaction_overlay_mode
        .filter(|m| {
            !has_any_selection
                && !msg.is_local_only()
                && (!is_apple || !matches!(m, ReactionOverlayMode::CornerButton))
        });
    if let Some(overlay_mode) = show_hover_rx_overlay {
        let chosen_emojis: Vec<String> = msg
            .reactions
            .iter()
            .filter(|r| r.user_reacted)
            .map(|r| r.emoji.clone())
            .collect();
        let a_zone = on_action.clone();
        let a_bridge = on_action.clone();
        let a_exp = on_action.clone();
        let a_react = on_action.clone();
        bubble = bubble.child(render_hover_reaction_overlay(
            msg_id,
            is_outgoing,
            reaction_overlay_layout,
            overlay_mode,
            &chosen_emojis,
            palette,
            lang_id,
            cx,
            move |this, hovered, cx| {
                a_zone(this, BubbleUiAction::SetReactionBarHover(msg_id, hovered), cx);
            },
            move |this, hovered, cx| {
                a_bridge(this, BubbleUiAction::SetReactionBridgeHover(msg_id, hovered), cx);
            },
            move |this, cx| {
                a_exp(this, BubbleUiAction::ToggleReactionPicker(msg_id), cx);
            },
            move |this, emoji, cx| {
                a_react(this, BubbleUiAction::ToggleReaction(msg_id, emoji), cx);
            },
        ));
    }

    let bubble_element: AnyElement = bubble.into_any_element();

    let mut stack = div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .when(!is_apple, |el| {
            el.when_some(definite_w, |b, bw| b.w(px(bw)).min_w(px(bw)).max_w(px(bw)))
                .when(definite_w.is_none(), |b| b.min_w(px(0.0)).max_w(px(eff_max_w)))
        })
        .when(is_apple, |el| {
            el.min_w(px(0.0)).max_w(px(eff_max_w))
        })
        .when(is_outgoing, |el| el.items_end().ml(px(if is_apple { 28.0 } else { 56.0 })))
        .when(!is_outgoing, |el| el.items_start().mr(px(if is_apple { 28.0 } else { 56.0 })));

    if is_apple
        && !is_outgoing
        && !group.attached_to_prev
        && (extra.is_group_chat || (group.show_sender_name && !msg.sender_name.trim().is_empty()))
    {
        let sender_uid = msg.sender_id;
        let sender_name = crate::views::openbot::channel_view::resolve_channel_author_label(
            Some(&msg.sender_name),
        )
        .to_string();
        let hue_name = extra
            .sender_hue_name
            .filter(|h| TdesktopThemePalette::is_tinted_bot_hue(h))
            .unwrap_or_else(|| TdesktopThemePalette::bot_hue_name_for_id(sender_uid));
        let (_, _, author_fg_hex) = TdesktopThemePalette::bot_hue_colors(hue_name, is_dark);
        let act_dm = on_action.clone();
        let click_name = sender_name.clone();
        stack = stack.child(
            div()
                .id(SharedString::from(format!("openbot-group-author-{}", msg_id)))
                .pl(px(4.0))
                .mb(px(if !msg.reactions.is_empty() { 10.0 } else { 3.0 }))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .id(SharedString::from(format!("openbot-author-link-{}", msg_id)))
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(author_fg_hex))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                act_dm(
                                    this,
                                    BubbleUiAction::OpenSenderDm {
                                        user_id: sender_uid,
                                        sender_name: click_name.clone(),
                                    },
                                    cx,
                                );
                            }),
                        )
                        .child(sender_name),
                )
                .when(is_row_hovered && !msg.timestamp.is_empty(), |row| {
                    row.child(
                        div()
                            .text_size(px(11.5))
                            .text_color(ob.text_dim())
                            .child(msg.timestamp.clone()),
                    )
                }),
        );
    }

    stack = stack.child(bubble_element);

    if let Some(kb) = &msg.bot_keyboard {
        let a_bot = on_action.clone();
        stack = stack.child(render_inline_bot_keyboard(msg_id, kb, palette, cx, move |this, i, cx| {
            a_bot(this, BubbleUiAction::Interactive(i), cx);
        }));
    }

    if is_apple && !has_any_selection && bubble_layout.pending_error_retry {
        let a_retry = on_action.clone();
        stack = stack.child(
            div()
                .id(SharedString::from(format!("bubble-retry-row-{}", msg_id)))
                .mt(px(4.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(ob.danger())
                        .child(tr_str(lang_id, "发送失败", "Failed to send")),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("bubble-retry-btn-{}", msg_id)))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded(px(999.0))
                        .bg(ob.bg_control())
                        .border_1()
                        .border_color(ob.danger())
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.danger())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_retry(this, BubbleUiAction::RetryFailedMessage(msg_id), cx);
                            }),
                        )
                        .child(tr_str(lang_id, "重试", "Retry")),
                ),
        );
    }

    if is_apple && !has_any_selection {
        let has_steps = !is_outgoing && !extra.turn_steps.is_empty();
        let show_action_row = is_row_hovered
            || show_hover_rx_overlay.is_some()
            || extra.steps_expanded
            || has_steps;
        if show_action_row {
            let show_icons = is_row_hovered || show_hover_rx_overlay.is_some();
            let a_copy = on_action.clone();
            let a_reply = on_action.clone();
            let a_react = on_action.clone();
            let a_pin = on_action.clone();
            let a_more = on_action.clone();
            let a_steps = on_action.clone();
            let hover_bg = ob.hover();
            let icon_c = ob.text_muted();

            let mut action_row = div()
                .id(SharedString::from(format!("message-action-row-{}", msg_id)))
                .mt(px(3.0))
                .min_h(px(24.0))
                .px(px(2.0))
                .flex()
                .items_center()
                .gap(px(4.0))
                .when(is_outgoing, |el| el.justify_end())
                .when(!is_outgoing, |el| el.justify_start());

            if show_icons {
                let make_btn = |id_suffix: &str, icon: OpenBotStageIcon, act: BubbleUiAction| {
                    let cb = on_action.clone();
                    div()
                        .id(SharedString::from(format!("msg-act-{}-{}", id_suffix, msg_id)))
                        .w(px(24.0))
                        .h(px(24.0))
                        .rounded(px(6.0))
                        .hover(move |s| s.bg(hover_bg))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb(this, act.clone(), cx);
                            }),
                        )
                        .child(openbot_stage_icon(icon, 13.0, icon_c))
                };
                let _ = (a_copy, a_reply, a_react, a_pin);
                action_row = action_row
                    .child(make_btn("copy", OpenBotStageIcon::Copy, BubbleUiAction::CopyMessageText(msg_id)))
                    .child(make_btn("reply", OpenBotStageIcon::Reply, BubbleUiAction::Context(ContextMenuAction::Reply(msg_id))))
                    .when(!msg.is_local_only(), |el| {
                        el.child(make_btn("react", OpenBotStageIcon::Smile, BubbleUiAction::ToggleReactionPicker(msg_id)))
                            .child(make_btn("pin", OpenBotStageIcon::Pin, BubbleUiAction::Context(ContextMenuAction::TogglePin(msg_id))))
                            .child(
                                div()
                                    .id(SharedString::from(format!("msg-act-more-{}", msg_id)))
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .rounded(px(6.0))
                                    .hover(move |s| s.bg(hover_bg))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            a_more(this, BubbleUiAction::OpenContextMenu(msg_id, ev.position), cx);
                                        }),
                                    )
                                    .child(openbot_stage_icon(OpenBotStageIcon::MoreHorizontal, 13.0, icon_c)),
                            )
                    });
                if !msg.timestamp.is_empty() {
                    let is_edited = matches!(msg.content, MessageContent::Text { is_edited: true, .. })
                        || meta.and_then(|m| m.edit_date.as_ref()).is_some();
                    let time_str = if is_edited {
                        format!(
                            "{} · {}",
                            msg.timestamp,
                            tr_str(lang_id, "已编辑", "Edited")
                        )
                    } else {
                        msg.timestamp.clone()
                    };
                    action_row = action_row.child(
                        div()
                            .px(px(4.0))
                            .text_size(px(12.5))
                            .text_color(ob.text_dim())
                            .whitespace_nowrap()
                            .child(time_str),
                    );
                }
            }

            if has_steps {
                let total_ms: u64 = extra
                    .turn_steps
                    .iter()
                    .filter_map(|s| s.duration_ms)
                    .sum();
                let count = extra.turn_steps.len();
                let steps_lbl = if total_ms >= 1000 {
                    format!(
                        "{} {} · {}s",
                        count,
                        tr_str(lang_id, "步", "steps"),
                        total_ms / 1000
                    )
                } else {
                    format!("{} {}", count, tr_str(lang_id, "步", "steps"))
                };
                action_row = action_row.child(
                    div()
                        .id(SharedString::from(format!("msg-steps-toggle-{}", msg_id)))
                        .h(px(24.0))
                        .px(px(7.0))
                        .rounded(px(6.0))
                        .hover(move |s| s.bg(hover_bg))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_steps(this, BubbleUiAction::ToggleAgentSteps(msg_id), cx);
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(ob.text_muted())
                                .child(steps_lbl),
                        )
                        .child(openbot_stage_icon(
                            if extra.steps_expanded {
                                OpenBotStageIcon::ChevronDown
                            } else {
                                OpenBotStageIcon::ChevronRight
                            },
                            12.0,
                            ob.text_muted(),
                        )),
                );
            }

            stack = stack.child(action_row);

            if has_steps && extra.steps_expanded {
                stack = stack.child(render_openbot_agent_step_list(
                    msg_id,
                    extra.turn_steps,
                    palette,
                    lang_id,
                ));
            }
        }
    }
    let _ = is_last_outgoing;

    let base_pt: f32 = if group.attached_to_prev {
        if is_apple {
            bubble_layout.grouped_gap_px
        } else {
            2.0
        }
    } else if is_apple {
        bubble_layout.entry_gap_px
    } else {
        7.0
    };
    let row_pt: f32 = if is_apple && !msg.reactions.is_empty() && !group.show_sender_name {
        base_pt.max(16.0)
    } else {
        base_pt
    };

    let mut row = div()
        .id(SharedString::from(format!("msg-row-{}", msg_id)))
        .relative()
        .w_full()
        .px(px(16.0))
        .pt(px(row_pt))
        .flex_shrink_0()
        .flex()
        .items_end()
        .when(has_any_selection, |el| {
            el.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    act_sel(this, BubbleUiAction::ToggleSelect(msg_id), cx);
                }),
            )
        })
        .when(highlight_alpha > 0.01, |el| {
            el.bg(TdesktopThemePalette::rgba_with_alpha(palette.accent, highlight_alpha * 0.14))
        });

    if has_any_selection {
        row = row.child(
            div()
                .w(px(22.0))
                .h(px(22.0))
                .mr(px(10.0))
                .mb(px(8.0))
                .rounded_full()
                .border_2()
                .border_color(rgb(if is_selected { palette.accent } else { palette.text_secondary }))
                .when(is_selected, |el| {
                    el.bg(rgb(palette.accent))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(tg_icon(IconName::SelectCheck, 12.0, 0xFFFFFF))
                }),
        );
    }

    if !is_outgoing && group.reserve_avatar_column {
        let av_src = meta.and_then(|m| m.avatar_url.as_deref()).and_then(|u| resolve_image_source(u, None));
        let av_size = if is_apple { APPLE_AVATAR_SIZE } else { MSG_PHOTO_SIZE };
        let av_skip = if is_apple { APPLE_AVATAR_SKIP } else { MSG_PHOTO_SKIP };
        let sender_uid = msg.sender_id;
        let sender_name = msg.sender_name.clone();
        let act_av_dm = on_action.clone();
        row = row.child(
            div()
                .w(px(av_skip))
                .pr(px(if is_apple { 4.0 } else { 6.0 }))
                .flex_shrink_0()
                .flex()
                .justify_start()
                .when(group.show_bottom_avatar, |el| {
                    let av_inner: AnyElement = if let Some(src) = av_src {
                        div()
                            .w(px(av_size))
                            .h(px(av_size))
                            .rounded_full()
                            .overflow_hidden()
                            .bg(rgb(palette.peer_avatar_color(msg.sender_id as i64)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(img(src).size_full().object_fit(ObjectFit::Cover))
                            .into_any_element()
                    } else if is_apple && (extra.sender_is_bot || extra.sender_hue_name.is_some()) {
                        let (_, hue, color_hex) = normalize_fabushi_plush_spec(
                            "",
                            extra.sender_hue_name.unwrap_or(""),
                        );
                        let seed = format!("sender_{}_{}", msg.sender_id, msg.sender_name);
                        render_plush_bot_avatar(
                            &seed,
                            None,
                            hue,
                            color_hex.as_deref(),
                            av_size,
                            openbot_contracts::avatar::AvatarMood::Idle,
                            None,
                        )
                        .into_any_element()
                    } else if let Some(fab) = active_fabushi_bot
                        .filter(|f| !extra.is_group_chat && !f.avatar_shape.eq_ignore_ascii_case("classic"))
                    {
                        if is_apple {
                            let (shape, hue, color) =
                                normalize_fabushi_plush_spec(&fab.avatar_shape, &fab.avatar_color);
                            render_plush_bot_avatar(
                                &fab.agent_id,
                                shape,
                                hue,
                                color.as_deref(),
                                av_size,
                                openbot_contracts::avatar::AvatarMood::Idle,
                                None,
                            )
                            .into_any_element()
                        } else {
                            render_grok_persona_avatar(
                                &fab.agent_id,
                                &fab.avatar_shape,
                                &fab.avatar_color,
                                &msg.avatar_initials,
                                av_size,
                                GrokPersonaState::from_activity_str(&fab.execution_state, false),
                                palette.stage_bg,
                                false,
                            )
                            .into_any_element()
                        }
                    } else {
                        div()
                            .w(px(av_size))
                            .h(px(av_size))
                            .rounded_full()
                            .overflow_hidden()
                            .bg(rgb(palette.peer_avatar_color(msg.sender_id as i64)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(if is_apple { 11.5 } else { 12.0 }))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xFFFFFF))
                            .when(meta.and_then(|m| m.avatar_url.as_ref()).is_none(), |a| {
                                a.child(msg.avatar_initials.clone())
                            })
                            .into_any_element()
                    };

                    el.child(
                        div()
                            .id(SharedString::from(format!("group-sender-avatar-{}", msg_id)))
                            .when(is_apple && extra.is_group_chat && sender_uid > 0, |btn| {
                                let click_name = sender_name.clone();
                                btn.cursor_pointer().on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        act_av_dm(
                                            this,
                                            BubbleUiAction::OpenSenderDm {
                                                user_id: sender_uid,
                                                sender_name: click_name.clone(),
                                            },
                                            cx,
                                        );
                                    }),
                                )
                            })
                            .child(av_inner),
                    )
                }),
        );
    }

    let aligned_row = if is_outgoing {
        row.justify_end().child(stack)
    } else {
        row.justify_start().child(stack)
    };
    let wrapped_row = if is_apple && extra.is_direct_human {
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(div().w_full().max_w(px(760.0)).child(aligned_row))
            .into_any_element()
    } else {
        aligned_row.into_any_element()
    };

    if date_divider_label.is_none() && !is_unread_anchor {
        return div()
            .relative()
            .w_full()
            .flex_shrink_0()
            .child(wrapped_row)
            .when_some(row_heights_probe, |el, probe| {
                el.child(make_row_height_probe_canvas(msg_id, probe))
            })
            .into_any_element();
    }

    div()
        .relative()
        .flex()
        .flex_col()
        .w_full()
        .flex_shrink_0()
        .when_some(date_divider_label, |col, label| {
            if is_apple {
                col.child(
                    div()
                        .w_full()
                        .pt(px(14.0))
                        .pb(px(6.0))
                        .flex_shrink_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(palette.text_secondary))
                                .child(label.to_string()),
                        ),
                )
            } else {
                col.child(
                    div().w_full().py(px(10.0)).flex_shrink_0().flex().justify_center().child(
                        div()
                            .px(px(12.0))
                            .py(px(3.0))
                            .rounded_full()
                            .bg(palette.service_pill_rgba())
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(palette.service_pill_fg))
                            .child(label.to_string()),
                    ),
                )
            }
        })
        .when(is_unread_anchor, |col| {
            col.child(
                div().w_full().py(px(8.0)).flex_shrink_0().child(
                    div()
                        .w_full()
                        .py(px(4.0))
                        .when(is_apple, |el| el.bg(TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.08)))
                        .when(!is_apple, |el| el.bg(palette.service_pill_rgba()))
                        .flex()
                        .justify_center()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(palette.accent))
                        .child(crate::i18n::tr_key_or(lang_id, "lng_unread_bar", "Unread Messages")),
                ),
            )
        })
        .child(wrapped_row)
        .when_some(row_heights_probe, |el, probe| {
            el.child(make_row_height_probe_canvas(msg_id, probe))
        })
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_bubble_inner_content_ext<V: 'static>(
    msg: &MessageItemModel,
    meta: Option<&MessageRichMeta>,
    is_outgoing: bool,
    long_expanded: bool,
    revealed_spoilers: &HashSet<(u64, usize)>,
    expanded_quotes: &HashSet<(u64, usize)>,
    copied_code_msg: Option<u64>,
    bot_assist: Option<BotAssistRenderContext<'_>>,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, BubbleUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let msg_id = msg.id;
    let explicit_entities = meta.map(|m| m.entities.as_slice()).unwrap_or(&[]);

    if let Some(doc) = meta.and_then(|m| m.document.as_ref()) {
        let a_dl = on_action.clone();
        let mut col = div().flex().flex_col().w_full().min_w(px(0.0)).gap(px(6.0)).child(render_document_card(
            msg_id, doc, palette, cx, move |this, mid, u, n, cx| {
                a_dl(this, BubbleUiAction::DownloadDoc(mid, u, n), cx);
            },
        ));
        if let MessageContent::Text { text, .. } = &msg.content {
            if !text.is_empty() && text != &doc.file_name {
                let a_rt = on_action.clone();
                col = col.child(render_rich_message_body(
                    msg_id, text, explicit_entities, revealed_spoilers, expanded_quotes,
                    copied_code_msg, palette, lang_id, is_outgoing, cx,
                    move |this, r, cx| a_rt(this, BubbleUiAction::RichText(r), cx),
                ));
            }
        }
        return col.into_any_element();
    }

    if let Some(contact) = meta.and_then(|m| m.contact.as_ref()) {
        let a_c = on_action.clone();
        return render_contact_card(msg_id, contact, palette, lang_id, cx, move |this, uid, cx| {
            a_c(this, BubbleUiAction::OpenContact(uid), cx);
        });
    }

    match &msg.content {
        MessageContent::Text { text, .. } => {
            let a_rt = on_action.clone();
            let preview = if palette.is_apple_style() {
                Some(compute_message_preview(text, false, long_expanded))
            } else {
                None
            };
            let display_text = preview
                .as_ref()
                .map(|p| p.preview_text.as_str())
                .unwrap_or(text.as_str());
            let show_more_avail = preview.as_ref().is_some_and(|p| p.show_more_available);
            let is_collapsed = preview.as_ref().is_some_and(|p| p.collapsed);
            let mut col = div().flex().flex_col().w_full().min_w(px(0.0)).gap(px(4.0)).child(render_rich_message_body(
                msg_id, display_text, explicit_entities, revealed_spoilers, expanded_quotes,
                copied_code_msg, palette, lang_id, is_outgoing, cx,
                move |this, r, cx| a_rt(this, BubbleUiAction::RichText(r), cx),
            ));
            if show_more_avail {
                let ob = palette.openbot_palette();
                let a_long = on_action.clone();
                let hover_bg = ob.hover();
                col = col.child(
                    div()
                        .id(SharedString::from(format!("msg-long-toggle-{}", msg_id)))
                        .mt(px(4.0))
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .bg(ob.bg_control())
                        .border_1()
                        .border_color(ob.border_hairline())
                        .hover(move |s| s.bg(hover_bg))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.text_muted())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_long(this, BubbleUiAction::ToggleLongMessage(msg_id), cx);
                            }),
                        )
                        .child(if is_collapsed {
                            tr_str(lang_id, "显示全部", "Show full")
                        } else {
                            tr_str(lang_id, "收起长消息", "Show less")
                        }),
                );
            }
            if let Some(m) = meta {
                if m.show_translation {
                    if let Some(ref tr_txt) = m.translated_text {
                        let is_apple_out = palette.is_apple_style() && is_outgoing;
                        col = col.child(
                            div()
                                .mt(px(4.0))
                                .pt(px(5.0))
                                .border_t_1()
                                .border_color(if is_apple_out {
                                    TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.25)
                                } else {
                                    TdesktopThemePalette::rgba_with_alpha(palette.border_fg, 0.45)
                                })
                                .text_size(px(12.5))
                                .text_color(rgb(if is_apple_out {
                                    palette.msg_out_secondary_fg
                                } else {
                                    palette.accent
                                }))
                                .child(format!("🌐 {}", tr_txt)),
                        );
                    }
                }
                if let Some(ref wp) = m.webpage {
                    let a_url = on_action.clone();
                    let a_iv = on_action.clone();
                    col = col.child(render_webpage_preview_card(
                        msg_id, wp, palette, lang_id, cx,
                        move |this, u, cx| a_url(this, BubbleUiAction::OpenWebUrl(u), cx),
                        move |this, u, t, cx| a_iv(this, BubbleUiAction::OpenInstantView(u, t), cx),
                    ));
                }
            }
            col.into_any_element()
        }
        MessageContent::Media {
            media_type, url, width, height, duration_secs, file_size_bytes, caption, is_streaming,
        } => {
            let local_path = meta.and_then(|m| m.local_media_path.as_deref());
            let is_spoiler = meta.map_or(false, |m| m.is_spoiler_media);
            let a_m = on_action.clone();
            render_media_bubble_card(
                msg_id, *media_type, url, local_path, *width, *height, *duration_secs,
                *file_size_bytes, caption.as_deref(), *is_streaming, is_spoiler,
                revealed_spoilers, expanded_quotes, copied_code_msg, explicit_entities,
                palette, lang_id, is_outgoing, cx,
                move |this, ma, cx| a_m(this, BubbleUiAction::Media(ma), cx),
            )
        }
        MessageContent::Album { items, caption } => {
            let a_m = on_action.clone();
            render_album_mosaic_card(
                msg_id, items, caption.as_deref(), revealed_spoilers, expanded_quotes,
                copied_code_msg, explicit_entities, palette, lang_id, is_outgoing, cx,
                move |this, ma, cx| a_m(this, BubbleUiAction::Media(ma), cx),
            )
        }
        MessageContent::Voice {
            duration_secs, waveform_5bit, playback_position_secs, is_playing, playback_speed,
        } => {
            let a_m = on_action.clone();
            render_voice_waveform_card(
                msg_id, *duration_secs, waveform_5bit, *playback_position_secs, *is_playing,
                *playback_speed, palette, is_outgoing, cx,
                move |this, ma, cx| a_m(this, BubbleUiAction::Media(ma), cx),
            )
        }
        MessageContent::Poll {
            question, poll_type, options, total_votes, is_closed, selected_option_id,
        } => {
            let expl = meta.and_then(|m| m.quiz_explanation.as_deref());
            let a_i = on_action.clone();
            render_poll_card(
                msg_id, question, *poll_type, options, *total_votes, *is_closed,
                *selected_option_id, expl, palette, lang_id, cx,
                move |this, ia, cx| a_i(this, BubbleUiAction::Interactive(ia), cx),
            )
        }
        MessageContent::Location { latitude, longitude, title, address } => {
            render_location_card(*latitude, *longitude, title, address, palette)
        }
        MessageContent::Call { call_type, direction, duration_secs, status_text } => {
            let a_i = on_action.clone();
            render_call_card(msg_id, *call_type, *direction, *duration_secs, status_text, palette, cx, move |this, ia, cx| {
                a_i(this, BubbleUiAction::Interactive(ia), cx);
            })
        }
        MessageContent::Story { user_name, avatar_initials, caption, preview_color, expires_in } => {
            let a_i = on_action.clone();
            render_story_card(
                msg_id, user_name, avatar_initials, caption, *preview_color, expires_in,
                palette, cx, move |this, ia, cx| {
                    a_i(this, BubbleUiAction::Interactive(ia), cx);
                },
            )
        }
        MessageContent::Invoice { title, description, amount_stars, currency, status, receipt_id } => {
            let a_i = on_action.clone();
            render_invoice_card(
                msg_id, title, description, *amount_stars, currency, *status,
                receipt_id.as_deref(), palette, cx,
                move |this, ia, cx| a_i(this, BubbleUiAction::Interactive(ia), cx),
            )
        }
        MessageContent::BotKeyboard { rows } => {
            let a_i = on_action.clone();
            render_inline_bot_keyboard(msg_id, rows, palette, cx, move |this, ia, cx| {
                a_i(this, BubbleUiAction::Interactive(ia), cx);
            })
        }
        MessageContent::GenerativeCard(card) => {
            let a_i = on_action.clone();
            super::cards::render_generative_card(msg_id, card, palette, cx, move |this, ia, cx| {
                a_i(this, BubbleUiAction::Interactive(ia), cx);
            })
        }
        MessageContent::LocalBotAssistCard(card) => {
            let cid = card.card_id();
            let login_draft = bot_assist.as_ref().and_then(|ba| ba.login_drafts.get(cid));
            let click_draft = bot_assist.as_ref().and_then(|ba| ba.click_drafts.get(cid));
            let active_field = bot_assist
                .as_ref()
                .and_then(|ba| ba.active_field)
                .and_then(|(active_cid, field)| (active_cid == cid).then_some(*field));
            let ime_preedit = bot_assist
                .as_ref()
                .filter(|_| active_field.is_some())
                .and_then(|ba| ba.ime_preedit);
            let a_i = on_action.clone();
            render_bot_assist_card(
                card,
                login_draft,
                active_field,
                ime_preedit,
                click_draft,
                palette,
                cx,
                move |this, ia, cx| a_i(this, BubbleUiAction::Interactive(ia), cx),
            )
        }
        MessageContent::Service { text, .. } => div().text_size(px(12.0)).child(text.clone()).into_any_element(),
    }
}

fn render_bottom_meta_strip(
    msg: &MessageItemModel,
    meta: Option<&MessageRichMeta>,
    is_outgoing: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
) -> AnyElement {
    let fg = if is_outgoing { palette.msg_out_date_fg } else { palette.msg_in_date_fg };
    let is_edited = matches!(msg.content, MessageContent::Text { is_edited: true, .. })
        || meta.and_then(|m| m.edit_date.as_ref()).is_some();
    let views = meta.and_then(|m| m.views_count);
    let is_zh = lang_id.starts_with("zh");
    let meta_font = px(palette.scaled_px(10.5));

    div()
        .mt(px(palette.scaled_px(3.0)))
        .w_full()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(palette.scaled_px(5.0)))
        .when_some(views, |el, vc| {
            let vlabel = if is_zh {
                format!("{} 次浏览", format_view_count(vc))
            } else {
                format!("{} views", format_view_count(vc))
            };
            el.child(div().text_size(meta_font).text_color(rgb(fg)).child(vlabel))
        })
        .when(is_edited, |el| {
            let elabel = if is_zh { "已编辑" } else { "edited" };
            el.child(div().text_size(meta_font).italic().text_color(rgb(fg)).child(elabel))
        })
        .child(div().text_size(meta_font).text_color(rgb(fg)).child(msg.timestamp.clone()))
        .when(is_outgoing, |el| {
            let (icon, tint) = match msg.delivery_status {
                DeliveryStatus::Sending => (IconName::ClockSending, fg),
                DeliveryStatus::Sent => (IconName::CheckSingle, palette.check_sent_fg),
                DeliveryStatus::Read => (IconName::CheckDouble, palette.check_read_fg),
                DeliveryStatus::Failed => (IconName::Close, 0xFF453A),
            };
            el.child(tg_icon(icon, palette.scaled_px(13.0), tint))
        })
        .into_any_element()
}

/// Detects raw JSON error walls (e.g. Gemini/OpenAI HTTP 400 tool-schema dumps) or prefixed
/// provider/tool failure cards and extracts a concise human summary + raw payload for OpenBot's
/// collapsible `AgentSteps` failed-step drawer (`UI-F10`).
pub fn extract_structured_error_payload(text: &str) -> Option<StructuredErrorCard> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    const ERROR_PREFIXES: [&str; 9] = [
        "⚠️ **执行失败",
        "⚠️ **AI 服务请求失败",
        "⚠️ **本地 AI 执行失败",
        "⚠️ **无法启动 AI 回复",
        "⚠️ **执行异常",
        "⚠️ **AI Provider Request Failed",
        "⚠️ **Local AI Turn Failed",
        "PROVIDER_ERROR:",
        "tool_error:",
    ];

    let matched_prefix = ERROR_PREFIXES
        .iter()
        .find(|p| trimmed.starts_with(**p))
        .copied();

    let extract_json_reason = |val: &serde_json::Value| -> Option<String> {
        if let Some(msg) = val
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|v| v.as_str())
        {
            return Some(msg.lines().next().unwrap_or(msg).trim().to_string());
        }
        if let Some(msg) = val.get("message").and_then(|v| v.as_str()) {
            return Some(msg.lines().next().unwrap_or(msg).trim().to_string());
        }
        if let Some(arr) = val
            .get("error")
            .and_then(|e| e.get("details"))
            .and_then(|d| d.as_array())
        {
            for item in arr {
                if let Some(viols) = item.get("fieldViolations").and_then(|v| v.as_array()) {
                    if let Some(desc) = viols
                        .first()
                        .and_then(|v| v.get("description"))
                        .and_then(|v| v.as_str())
                    {
                        return Some(desc.trim().to_string());
                    }
                }
            }
        }
        if let Some(desc) = val.get("description").and_then(|v| v.as_str()) {
            return Some(desc.trim().to_string());
        }
        None
    };

    // Case 1: Pure raw JSON error object or array
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            let has_err_shape = val.get("error").is_some()
                || val.get("fieldViolations").is_some()
                || val.get("violations").is_some()
                || val.get("status").and_then(|v| v.as_str()).is_some_and(|s| {
                    s.eq_ignore_ascii_case("INVALID_ARGUMENT")
                        || s.eq_ignore_ascii_case("ERROR")
                        || s.eq_ignore_ascii_case("FAILED_PRECONDITION")
                })
                || trimmed.contains("function_declarations")
                || trimmed.contains("invalid_request_error");
            if has_err_shape {
                let reason = extract_json_reason(&val)
                    .unwrap_or_else(|| "Model provider returned a structured error payload".to_string());
                let short_reason: String = reason.chars().take(120).collect();
                return Some(StructuredErrorCard {
                    summary_title: "执行失败".to_string(),
                    concise_reason: short_reason,
                    raw_payload: trimmed.to_string(),
                    is_json: true,
                });
            }
        } else {
            let line_count = trimmed.lines().count();
            let key_hits = [
                "\"error\"",
                "\"violations\"",
                "\"fieldViolations\"",
                "\"function_declarations\"",
                "\"invalid_request_error\"",
                "\"description\"",
                "\"status\"",
            ]
            .iter()
            .filter(|k| trimmed.contains(**k))
            .count();
            if line_count >= 4 && key_hits >= 2 {
                return Some(StructuredErrorCard {
                    summary_title: "执行失败".to_string(),
                    concise_reason: "Model provider returned an invalid request payload".to_string(),
                    raw_payload: trimmed.to_string(),
                    is_json: true,
                });
            }
        }
    }

    // Case 2: Prefixed provider/tool failure card containing JSON / HTTP error traces / long wall
    if let Some(prefix) = matched_prefix {
        let has_embedded_json = trimmed.contains('{') && trimmed.contains('}');
        let has_http_trace = trimmed.contains("HTTP 4")
            || trimmed.contains("HTTP 5")
            || trimmed.contains("status=")
            || trimmed.contains("fieldViolations")
            || trimmed.contains("function_declarations");
        if has_embedded_json || has_http_trace || trimmed.chars().count() > 180 {
            let title_line = trimmed
                .lines()
                .next()
                .unwrap_or(prefix)
                .replace("⚠️", "")
                .replace("**", "")
                .trim()
                .to_string();
            let summary_title = title_line
                .split(&['：', ':'][..])
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("执行失败")
                .to_string();

            let mut concise = String::new();
            if let (Some(j_start), Some(j_end)) = (trimmed.find('{'), trimmed.rfind('}')) {
                if j_start < j_end {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&trimmed[j_start..=j_end]) {
                        if let Some(r) = extract_json_reason(&val) {
                            concise = r;
                        }
                    }
                }
            }
            if concise.is_empty() {
                let after_colon = title_line
                    .split_once('：')
                    .or_else(|| title_line.split_once(':'))
                    .map(|(_, rest)| rest.trim())
                    .unwrap_or("");
                if !after_colon.is_empty() && !after_colon.starts_with('{') {
                    concise = after_colon.to_string();
                } else {
                    concise = "Provider or tool call returned an error response".to_string();
                }
            }
            let concise_reason: String = concise.chars().take(120).collect();
            return Some(StructuredErrorCard {
                summary_title,
                concise_reason,
                raw_payload: trimmed.to_string(),
                is_json: has_embedded_json,
            });
        }
    }

    None
}

fn render_structured_error_step_card<V: 'static>(
    msg_id: u64,
    err: &StructuredErrorCard,
    expanded: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, BubbleUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let ob = palette.openbot_palette();
    let a_toggle = on_action.clone();
    let a_copy = on_action;
    let raw_copy = err.raw_payload.clone();
    let hover_bg = ob.hover();
    let danger_bg = ob
        .rgba("--openbot-danger-soft")
        .unwrap_or_else(|| TdesktopThemePalette::rgba_with_alpha(palette.danger, 0.14));
    let danger_border = ob
        .rgba("--openbot-danger-border")
        .unwrap_or_else(|| ob.danger());

    div()
        .id(SharedString::from(format!("agent-step-error-card-{}", msg_id)))
        .w_full()
        .min_w(px(0.0))
        .rounded(px(14.0))
        .bg(ob.bg_surface())
        .border_1()
        .border_color(danger_border)
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_shrink_0()
                        .px(px(7.0))
                        .py(px(2.0))
                        .rounded_full()
                        .bg(danger_bg)
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(openbot_stage_icon(OpenBotStageIcon::AlertCircle, 12.0, ob.danger()))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.danger())
                                .child(tr_str(lang_id, "失败", "Failed")),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(err.summary_title.clone()),
                ),
        )
        .child(
            div()
                .w_full()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(12.5))
                .text_color(ob.text_muted())
                .child(err.concise_reason.clone()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .id(("agent-step-error-toggle", msg_id))
                        .h(px(24.0))
                        .px(px(7.0))
                        .rounded(px(6.0))
                        .hover(move |s| s.bg(hover_bg))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_toggle(this, BubbleUiAction::ToggleErrorDetails(msg_id), cx);
                            }),
                        )
                        .child(openbot_stage_icon(
                            if expanded {
                                OpenBotStageIcon::ChevronDown
                            } else {
                                OpenBotStageIcon::ChevronRight
                            },
                            12.0,
                            ob.text_muted(),
                        ))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_muted())
                                .child(openbot_error_drawer_btn_label(expanded, lang_id)),
                        ),
                )
                .when(expanded, |row| {
                    row.child(
                        div()
                            .id(("agent-step-error-copy", msg_id))
                            .h(px(24.0))
                            .px(px(7.0))
                            .rounded(px(6.0))
                            .hover(move |s| s.bg(hover_bg))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_copy(this, BubbleUiAction::CopyRawText(raw_copy.clone()), cx);
                                }),
                            )
                            .child(openbot_stage_icon(OpenBotStageIcon::Copy, 12.0, ob.text_muted()))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(ob.text_muted())
                                    .child(tr_str(lang_id, "复制", "Copy")),
                            ),
                    )
                }),
        )
        .when(expanded, |col| {
            col.child(
                div()
                    .id(SharedString::from(format!("agent-step-error-pre-{}", msg_id)))
                    .w_full()
                    .max_h(px(OPENBOT_ERROR_PRE_MAX_HEIGHT_PX))
                    .overflow_y_scroll()
                    .rounded(px(4.0))
                    .bg(ob.bg_control())
                    .border_1()
                    .border_color(ob.border_hairline())
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_size(px(12.0))
                    .line_height(px(16.5))
                    .text_color(ob.text_secondary())
                    .child(err.raw_payload.clone()),
            )
        })
        .into_any_element()
}

/// Renders an OpenBot collapsible `AgentStepList` (`max_h: 220px`, Sparkles/Puzzle/Brain/Alert icons).
pub fn render_openbot_agent_step_list(
    scope_id: u64,
    steps: &[AgentStepEntry],
    palette: &TdesktopThemePalette,
    _lang_id: &str,
) -> AnyElement {
    let ob = palette.openbot_palette();
    let mut list = div()
        .id(SharedString::from(format!("openbot-agent-steps-{}", scope_id)))
        .mt(px(4.0))
        .w_full()
        .max_w(px(640.0))
        .max_h(px(STEPS_MAX_HEIGHT))
        .overflow_y_scroll()
        .rounded(px(10.0))
        .bg(ob.bg_surface())
        .border_1()
        .border_color(ob.border_hairline())
        .p(px(8.0))
        .flex()
        .flex_col()
        .gap(px(5.0));

    for (step_idx, step) in steps.iter().enumerate() {
        let (icon_kind, icon_color) = if step.failed {
            (OpenBotStageIcon::AlertCircle, ob.danger())
        } else {
            match step.kind {
                AgentStepKind::Thinking => (OpenBotStageIcon::Sparkles, ob.text_muted()),
                AgentStepKind::Progress => (OpenBotStageIcon::Puzzle, ob.text_muted()),
                AgentStepKind::Memory | AgentStepKind::MemorySummary => {
                    (OpenBotStageIcon::Brain, ob.text_muted())
                }
            }
        };
        let is_thinking = step.kind == AgentStepKind::Thinking;
        let is_strike = step.is_strikethrough_memory();
        list = list.child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(openbot_stage_icon(icon_kind, 12.5, icon_color))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.0))
                                .when(is_thinking, |el| el.italic())
                                .when(is_strike, |el| el.line_through())
                                .text_color(if step.failed { ob.danger() } else { ob.text_secondary() })
                                .child(step.label.clone()),
                        )
                        .when_some(step.duration_ms.filter(|&ms| ms >= 100), |row, ms| {
                            let d_lbl = if ms >= 1000 {
                                format!("{:.1}s", ms as f32 / 1000.0)
                            } else {
                                format!("{ms}ms")
                            };
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(11.0))
                                    .text_color(ob.text_dim())
                                    .child(d_lbl),
                            )
                        }),
                )
                .when_some(step.stderr.as_ref().filter(|s| !s.trim().is_empty()), |col, stderr| {
                    col.child(
                        div()
                            .id(SharedString::from(format!("agent-step-stderr-{}-{}", scope_id, step_idx)))
                            .ml(px(18.0))
                            .max_h(px(120.0))
                            .overflow_y_scroll()
                            .rounded(px(4.0))
                            .bg(ob.bg_control())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .px(px(6.0))
                            .py(px(4.0))
                            .text_size(px(11.0))
                            .text_color(ob.danger())
                            .child(stderr.clone()),
                    )
                }),
        );
    }

    list.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn extracts_raw_gemini_json_error_wall_into_collapsible_card() {
        let raw = r#"{
  "error": {
    "code": 400,
    "message": "Invalid JSON payload received. Unknown name \"additionalProperties\" at 'tools[0].function_declarations[0].parameters'",
    "status": "INVALID_ARGUMENT",
    "details": [{
      "fieldViolations": [{
        "field": "tools[0].function_declarations[0].parameters",
        "description": "Invalid JSON payload received."
      }]
    }]
  }
}"#;
        let card = extract_structured_error_payload(raw).expect("must detect raw JSON error wall");
        assert!(card.is_json);
        assert!(card.concise_reason.contains("Invalid JSON payload"));
        assert_eq!(card.raw_payload, raw);
    }

    #[std::prelude::v1::test]
    fn extracts_prefixed_hermes_provider_error_card() {
        let raw = "⚠️ **执行失败 (400)**：{\"error\":{\"message\":\"Bad request on tool schema\",\"status\":\"INVALID_ARGUMENT\"}}";
        let card = extract_structured_error_payload(raw).expect("must detect prefixed provider error");
        assert!(card.is_json);
        assert_eq!(card.summary_title, "执行失败 (400)");
        assert_eq!(card.concise_reason, "Bad request on tool schema");
    }

    #[std::prelude::v1::test]
    fn ignores_normal_assistant_prose_and_short_json() {
        assert!(extract_structured_error_payload("Hello! Here is your summary.").is_none());
        assert!(extract_structured_error_payload("{\"hello\": \"world\"}").is_none());
    }

    #[std::prelude::v1::test]
    fn group_message_layout_guards_empty_and_oob_index_and_bubble_derives_retry() {
        let fallback = compute_group_message_layout(&[], 0, None, None);
        assert_eq!(
            fallback.author_label,
            crate::views::openbot::channel_view::UNAVAILABLE_CHANNEL_AUTHOR_LABEL
        );
        assert!(!fallback.author_avatar_clickable);
        assert!(!fallback.author_name_clickable);

        let pal = TdesktopThemePalette::build_openbot(
            crate::theme::TdesktopThemeMode::Night,
            0x007CF7,
            14,
            16,
        );
        let failed_msg = MessageItemModel {
            id: 42,
            sender_id: 1,
            sender_name: "Me".to_string(),
            avatar_initials: "ME".to_string(),
            avatar_color: 0x007CF7,
            direction: MessageDirection::Outgoing,
            timestamp: "10:02".to_string(),
            content: MessageContent::Text {
                text: "Retry me".to_string(),
                is_edited: false,
            },
            delivery_status: DeliveryStatus::Failed,
            is_pinned: false,
            reply_to: None,
            reactions: Vec::new(),
            bot_keyboard: None,
        };
        let oob = compute_group_message_layout(&[failed_msg.clone()], 9, None, None);
        assert_eq!(
            oob.author_label,
            crate::views::openbot::channel_view::UNAVAILABLE_CHANNEL_AUTHOR_LABEL
        );
        let grp = BubbleGroupLayout {
            attached_to_prev: false,
            attached_to_next: false,
            show_sender_name: false,
            show_bottom_avatar: false,
            show_tail: false,
            reserve_avatar_column: false,
        };
        let layout_failed = compute_openbot_bubble_layout(&failed_msg, grp, false, false, &pal, "en");
        assert!(layout_failed.pending_error_retry);

        let mut sent_msg = failed_msg;
        sent_msg.delivery_status = DeliveryStatus::Read;
        let layout_sent = compute_openbot_bubble_layout(&sent_msg, grp, false, false, &pal, "en");
        assert!(!layout_sent.pending_error_retry);
    }
}
