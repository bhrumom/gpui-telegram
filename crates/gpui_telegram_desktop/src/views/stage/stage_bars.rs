//! Top Bar (54px), Multi-Select Bar, In-Chat Search Bar, Pinned Banner (46px) & Scroll-to-Bottom Button.

use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_telegram_proto::miniapp_contracts::{
    resolve_grok_persona_color, resolve_grok_persona_shape, FabushiBotSummaryDto,
};

use crate::components::avatar::{
    render_channel_avatar, ChannelAgentBrief, ChannelAvatarLayout,
};
use crate::components::grok_bot_avatar::{
    render_grok_bot_avatar, GrokBotAvatarProps, GrokPersonaState,
};
use crate::i18n::{tr_fmt, tr_key_or, tr_str};
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use super::openbot_stage_chrome::{
    activity_str_to_openbot_mood, build_channel_avatar_inputs, openbot_stage_icon,
    render_fabushi_plush_avatar, render_tab_concave_fillet, OpenBotStageIcon,
};
use super::rich_meta::resolve_image_source;

pub const APPLE_STAGE_TOP_BAR_HEIGHT: f32 = 76.0;
pub const APPLE_STAGE_EMPTY_TOP_BAR_HEIGHT: f32 = 52.0;
pub const OPENBOT_HEADER_HEIGHT: f32 = 38.0;

#[derive(Debug, Clone)]
pub enum TopBarUiAction {
    ToggleInspector,
    ToggleSearch,
    FocusSearch,
    SearchStep(i32),
    SearchClear,
    StartCall { video: bool },
    ToggleTopMoreMenu,
    DismissTopMoreMenu,
    ToggleTtlSubmenu,
    SetMessageTtl(u32),
    ViewProfile,
    ChangeWallpaper,
    TogglePinChat,
    ToggleBlockPeer,
    DeleteOrLeaveChat,
    JumpPinned,
    UnpinCurrent,
    EnterSelectMode,
    ClearSelection,
    ForwardSelected,
    CopySelected,
    DeleteSelected,
    ScrollToBottom,
    ClearHistory,
    ExportChat,
    ToggleMuteChat,
    ToggleBotCompanion,
    // Additive M3 OpenBot ConversationHeader actions (UI-F09)
    OpenAssistantSettings,
    OpenChannelSettings,
    OpenContactProfile,
    ToggleSidePanel,
    NewThread,
    SelectThread(Option<String>),
    ToggleModelPicker,
    SelectModel {
        model_id: String,
        thinking_level: Option<String>,
    },
    TogglePinnedMessagesPopover,
    JumpPinnedMessage(u64),
    UnpinMessage(u64),
}

fn format_ttl_badge(ttl_seconds: u32, lang_id: &str) -> String {
    match ttl_seconds {
        0 => tr_str(lang_id, "关闭", "Off").to_string(),
        86_400 => tr_str(lang_id, "1 天", "1 day").to_string(),
        604_800 => tr_str(lang_id, "1 周", "1 week").to_string(),
        2_592_000 | 2_678_400 => tr_str(lang_id, "1 个月", "1 month").to_string(),
        secs if secs >= 86_400 => format!("{}d", secs / 86_400),
        secs if secs >= 3600 => format!("{}h", secs / 3600),
        secs => format!("{}s", secs),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_stage_top_bar<V: 'static>(
    chat_id: Option<u64>,
    title: &str,
    subtitle: &str,
    initials: &str,
    avatar_url: Option<&str>,
    active_fabushi_bot: Option<&FabushiBotSummaryDto>,
    is_bot_chat: bool,
    bot_activity_state: &str,
    select_mode_active: bool,
    selected_count: usize,
    message_ttl_seconds: u32,
    search_open: bool,
    search_query: &str,
    search_match_cursor: usize,
    search_match_total: usize,
    is_inspector_open: bool,
    more_menu_open: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    render_stage_top_bar_with_group(
        chat_id,
        title,
        subtitle,
        initials,
        avatar_url,
        active_fabushi_bot,
        is_bot_chat,
        false,
        None,
        bot_activity_state,
        select_mode_active,
        selected_count,
        message_ttl_seconds,
        search_open,
        search_query,
        search_match_cursor,
        search_match_total,
        is_inspector_open,
        false,
        more_menu_open,
        palette,
        lang_id,
        cx,
        on_action,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn render_stage_top_bar_with_group<V: 'static>(
    chat_id: Option<u64>,
    title: &str,
    subtitle: &str,
    initials: &str,
    avatar_url: Option<&str>,
    active_fabushi_bot: Option<&FabushiBotSummaryDto>,
    is_bot_chat: bool,
    is_group_chat: bool,
    group_member_tiles: Option<&[(String, u32)]>,
    bot_activity_state: &str,
    select_mode_active: bool,
    selected_count: usize,
    message_ttl_seconds: u32,
    search_open: bool,
    search_query: &str,
    search_match_cursor: usize,
    search_match_total: usize,
    is_inspector_open: bool,
    is_bot_companion_open: bool,
    more_menu_open: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let ob = palette.openbot_palette();
    let bar_h = px(if is_apple {
        OPENBOT_HEADER_HEIGHT
    } else {
        palette.scaled_px(54.0)
    });
    let bar_bg: Rgba = if is_apple {
        ob.bg_canvas()
    } else {
        rgb(palette.topbar_bg).into()
    };
    if select_mode_active || selected_count > 0 {
        let count_str = selected_count.to_string();
        let a_fwd = on_action.clone();
        let a_cpy = on_action.clone();
        let a_del = on_action.clone();
        let a_clr = on_action.clone();
        let pill_r = px(if is_apple { 8.0 } else { 6.0 });
        let primary_bg: Rgba = if is_apple {
            ob.surface_light()
        } else {
            rgb(palette.accent).into()
        };
        let primary_fg: Rgba = if is_apple {
            ob.text_on_light()
        } else {
            rgb(0xFFFFFF).into()
        };
        let secondary_bg: Rgba = if is_apple {
            ob.bg_control()
        } else {
            TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.14)
        };
        let secondary_fg: Rgba = if is_apple {
            ob.text_primary()
        } else {
            rgb(palette.accent).into()
        };
        let danger_bg: Rgba = if is_apple {
            ob.rgba("--openbot-danger-soft")
                .unwrap_or_else(|| TdesktopThemePalette::rgba_with_alpha(palette.danger, 0.14))
        } else {
            TdesktopThemePalette::rgba_with_alpha(palette.danger, 0.16)
        };
        let danger_fg: Rgba = if is_apple {
            ob.danger()
        } else {
            rgb(palette.danger).into()
        };
        let cancel_fg: Rgba = if is_apple {
            ob.text_muted()
        } else {
            rgb(palette.accent).into()
        };
        let ctl_hover = ob.bg_control_hover();
        let left_controls = if selected_count == 0 {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(openbot_stage_icon(
                    OpenBotStageIcon::Check,
                    14.0,
                    if is_apple { ob.text_primary() } else { rgb(palette.accent).into() },
                ))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if is_apple { ob.text_primary() } else { rgb(palette.text_primary).into() })
                        .child(tr_str(lang_id, "点击消息进行多选", "Click messages to select")),
                )
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .id("ms-fwd-btn")
                        .h(px(28.0))
                        .px(px(12.0))
                        .rounded(pill_r)
                        .bg(primary_bg)
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.88))
                        .on_click(cx.listener(move |this, _, _, cx| a_fwd(this, TopBarUiAction::ForwardSelected, cx)))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(primary_fg)
                                .child(format!("{} {}", tr_key_or(lang_id, "lng_context_forward_msg", "Forward"), count_str)),
                        ),
                )
                .child(
                    div()
                        .id("ms-copy-btn")
                        .h(px(28.0))
                        .px(px(10.0))
                        .rounded(pill_r)
                        .bg(secondary_bg)
                        .when(is_apple, |el| el.border_1().border_color(ob.border_hairline()))
                        .cursor_pointer()
                        .hover(move |s| s.bg(ctl_hover))
                        .on_click(cx.listener(move |this, _, _, cx| a_cpy(this, TopBarUiAction::CopySelected, cx)))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(openbot_stage_icon(OpenBotStageIcon::Copy, 13.0, secondary_fg))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(secondary_fg)
                                .child(tr_key_or(lang_id, "lng_copy", "Copy")),
                        ),
                )
                .child(
                    div()
                        .id("ms-del-btn")
                        .h(px(28.0))
                        .px(px(10.0))
                        .rounded(pill_r)
                        .bg(danger_bg)
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| a_del(this, TopBarUiAction::DeleteSelected, cx)))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(openbot_stage_icon(OpenBotStageIcon::Trash, 13.0, danger_fg))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(danger_fg)
                                .child(tr_key_or(lang_id, "lng_context_delete_msg", "Delete")),
                        ),
                )
        };
        let cancel_hover = if is_apple { ob.hover() } else { rgb(palette.menu_hover_bg).into() };
        return div()
            .id("stage-multiselect-bar")
            .h(bar_h)
            .w_full()
            .flex_shrink_0()
            .px(px(12.0))
            .bg(bar_bg)
            .border_b_1()
            .border_color(if is_apple {
                ob.border_hairline()
            } else {
                rgb(palette.border_fg).into()
            })
            .flex()
            .items_center()
            .justify_between()
            .child(left_controls)
            .child(div().flex_1().h_full().window_control_area(WindowControlArea::Drag))
            .child(
                div()
                    .id("ms-cancel-btn")
                    .h(px(28.0))
                    .px(px(10.0))
                    .rounded(pill_r)
                    .cursor_pointer()
                    .hover(move |s| s.bg(cancel_hover))
                    .on_click(cx.listener(move |this, _, _, cx| a_clr(this, TopBarUiAction::ClearSelection, cx)))
                    .flex()
                    .items_center()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cancel_fg)
                    .child(tr_key_or(lang_id, "lng_cancel", "Cancel")),
            )
            .into_any_element();
    }

    let pid = chat_id.unwrap_or(0) as i64;
    let (grad_top, grad_bot) = palette.peer_avatar_gradient(pid);
    let img_src = avatar_url.and_then(|u| resolve_image_source(u, None));
    let has_chat = chat_id.is_some();
    let is_dark = palette.mode.is_dark();
    let a_insp1 = on_action.clone();
    let a_insp2 = on_action.clone();
    let a_search = on_action.clone();
    let a_more = on_action.clone();
    let a_companion = on_action.clone();
    let ttl_header_label = (message_ttl_seconds > 0).then(|| format_ttl_badge(message_ttl_seconds, lang_id));
    let ttl_badge_el: Option<AnyElement> = ttl_header_label.map(|ttl_lbl| {
        div()
            .id("topbar-ttl-badge")
            .flex_shrink_0()
            .px(px(palette.scaled_px(6.0)))
            .py(px(palette.scaled_px(1.5)))
            .rounded_full()
            .bg(TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.16))
            .flex()
            .items_center()
            .gap(px(3.0))
            .child(tg_icon(IconName::ClockSending, palette.scaled_px(11.0), palette.accent))
            .child(
                div()
                    .text_size(px(palette.scaled_px(10.5)))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(palette.accent))
                    .child(ttl_lbl),
            )
            .into_any_element()
    });
    let sub_lower = subtitle.to_ascii_lowercase();
    let is_typing_or_active_bot = sub_lower.contains("typing")
        || subtitle.contains("输入")
        || subtitle.contains("思考")
        || (!bot_activity_state.trim().is_empty()
            && !bot_activity_state.eq_ignore_ascii_case("idle"));
    let is_online = sub_lower == "online" || subtitle == "在线";
    let is_group_sub = sub_lower.contains("member")
        || sub_lower.contains("subscriber")
        || subtitle.contains("成员")
        || subtitle.contains("订阅");
    let show_apple_subtitle = !subtitle.is_empty() && (is_typing_or_active_bot || is_online || is_group_sub);

    let avatar_el: Option<AnyElement> = has_chat.then(|| {
        let avatar_size_px = palette.scaled_px(40.0);
        let is_typing = is_typing_or_active_bot;
        let fab_non_classic = active_fabushi_bot
            .filter(|f| !f.avatar_shape.eq_ignore_ascii_case("classic"));
        let show_dynamic_bot_avatar =
            fab_non_classic.is_some() || (is_bot_chat && (img_src.is_none() || is_typing));
        let show_group_composite =
            (is_group_chat || is_group_sub) && img_src.is_none() && !show_dynamic_bot_avatar;
        let avatar_ring_col = palette.hairline_border_rgba();
        let avatar_shadow = if is_dark {
            Vec::new()
        } else {
            vec![BoxShadow {
                color: hsla(0.0, 0.0, 0.0, 0.10),
                offset: point(px(0.0), px(2.0)),
                blur_radius: px(8.0),
                spread_radius: px(0.0),
            }]
        };

        if show_group_composite {
            crate::views::sidebar::dialogs_row_view::render_composite_group_avatar(
                chat_id.unwrap_or(0),
                title,
                group_member_tiles,
                avatar_size_px,
                palette.stage_bg,
                is_dark,
            )
            .into_any_element()
        } else if show_dynamic_bot_avatar {
            if let Some(fab) = fab_non_classic {
                let effective_state = if !bot_activity_state.trim().is_empty()
                    && !bot_activity_state.eq_ignore_ascii_case("idle")
                {
                    bot_activity_state
                } else {
                    fab.execution_state.as_str()
                };
                render_grok_bot_avatar(
                    GrokBotAvatarProps::new(
                        &fab.agent_id,
                        &fab.avatar_shape,
                        &fab.avatar_color,
                        avatar_size_px,
                    )
                    .with_state(GrokPersonaState::from_activity_str(
                        effective_state,
                        is_typing,
                    ))
                    .with_follow_pointer(true)
                    .with_surface_bg(palette.topbar_bg),
                )
                .into_any_element()
            } else {
                let seed = format!("bot_{}_{}", chat_id.unwrap_or(0), title);
                let shape = resolve_grok_persona_shape(&seed, None);
                let color = resolve_grok_persona_color(&seed, None);
                let effective_state = if bot_activity_state.trim().is_empty() {
                    "idle"
                } else {
                    bot_activity_state
                };
                render_grok_bot_avatar(
                    GrokBotAvatarProps::new(&seed, &shape, &color, avatar_size_px)
                        .with_state(GrokPersonaState::from_activity_str(
                            effective_state,
                            is_typing,
                        ))
                        .with_follow_pointer(true)
                        .with_surface_bg(palette.topbar_bg),
                )
                .into_any_element()
            }
        } else if let Some(src) = img_src {
            div()
                .w(px(avatar_size_px))
                .h(px(avatar_size_px))
                .rounded_full()
                .overflow_hidden()
                .bg(rgb(grad_top))
                .when(is_apple, |el| {
                    el.border_1()
                        .border_color(avatar_ring_col)
                        .shadow(avatar_shadow.clone())
                })
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(img(src).size_full().object_fit(ObjectFit::Cover))
                .into_any_element()
        } else {
            div()
                .w(px(avatar_size_px))
                .h(px(avatar_size_px))
                .rounded_full()
                .overflow_hidden()
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(rgb(grad_top), 0.0),
                    linear_color_stop(rgb(grad_bot), 1.0),
                ))
                .when(is_apple, |el| {
                    el.border_1()
                        .border_color(avatar_ring_col)
                        .shadow(avatar_shadow)
                })
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(palette.scaled_px(if is_apple { 13.5 } else { 14.0 })))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .when(avatar_url.map_or(true, |u| u.trim().is_empty()), |av| {
                    av.child(initials.to_string())
                })
                .into_any_element()
        }
    });

    let bar = if is_apple {
        let props = OpenBotHeaderProps {
            chat_id,
            title,
            subtitle,
            initials,
            avatar_url,
            active_fabushi_bot,
            is_bot_chat,
            is_group_chat: is_group_chat || is_group_sub,
            group_member_tiles,
            group_member_briefs: &[],
            bot_activity_state,
            active_thread_id: None,
            thread_tabs: &[],
            active_model_label: "",
            active_thinking_effort: "",
            model_picker_open: false,
            pinned_count: 0,
            pinned_popover_open: false,
            search_open,
            more_menu_open,
            side_panel_open: is_inspector_open || is_bot_companion_open,
            right_panel_slot_open: false,
            external_hosted: false,
            available_width_px: 720.0,
            is_isolated_lock: false,
            is_archived: false,
            provider_connected: true,
            pending_model_badge: None,
            reduced_motion: false,
            window_glass: false,
            sidebar_closed: false,
            topbar_controls_right_edge_px: 0.0,
            message_ttl_seconds,
        };
        let _ = (show_apple_subtitle, is_online);
        render_openbot_conversation_header(&props, palette, lang_id, cx, on_action)
    } else {
        div()
            .relative()
            .h(bar_h)
            .w_full()
            .px(px(14.0))
            .bg(bar_bg)
            .border_b_1()
            .border_color(rgb(palette.border_fg))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .id("topbar-peer-header")
                    .flex()
                    .items_center()
                    .gap(px(palette.scaled_px(10.0)))
                    .flex_1()
                    .overflow_hidden()
                    .when(has_chat, |el| {
                        el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                            a_insp1(this, TopBarUiAction::ToggleInspector, cx);
                        }))
                    })
                    .when_some(avatar_el, |el, av| el.child(av))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .text_size(px(palette.scaled_px(14.0)))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(palette.text_primary))
                                            .truncate()
                                            .child(title.to_string()),
                                    )
                                    .when_some(ttl_badge_el, |row, badge| row.child(badge)),
                            )
                            .when(!subtitle.is_empty(), |el| {
                                let is_live = subtitle == "online" || subtitle.contains("typing");
                                el.child(
                                    div()
                                        .text_size(px(palette.scaled_px(12.0)))
                                        .text_color(rgb(if is_live {
                                            palette.online_dot_fg
                                        } else {
                                            palette.text_secondary
                                        }))
                                        .child(subtitle.to_string()),
                                )
                            }),
                    ),
            )
            .when(has_chat, |el| {
                let show_companion_btn = is_bot_chat || active_fabushi_bot.is_some();
                let a_comp_classic = a_companion.clone();
                let companion_btn = show_companion_btn.then(|| {
                    div()
                        .id("tb-bot-companion")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .bg(if is_bot_companion_open {
                            rgb(palette.accent).into()
                        } else {
                            TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.14)
                        })
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_comp_classic(this, TopBarUiAction::ToggleBotCompanion, cx);
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(palette.scaled_px(11.5)))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if is_bot_companion_open {
                                    0xFFFFFF
                                } else {
                                    palette.accent
                                }))
                                .child(tr_str(lang_id, "工作台", "Workbench")),
                        )
                });
                let search_btn = top_icon_btn("tb-search", IconName::Search, search_open, palette, cx, move |this, cx| {
                    a_search(this, TopBarUiAction::ToggleSearch, cx);
                });
                let info_btn = top_icon_btn("tb-info", IconName::SidebarPanel, is_inspector_open, palette, cx, move |this, cx| {
                    a_insp2(this, TopBarUiAction::ToggleInspector, cx);
                });
                let more_btn = top_icon_btn("tb-more", IconName::MoreVertical, more_menu_open, palette, cx, move |this, cx| {
                    a_more(this, TopBarUiAction::ToggleTopMoreMenu, cx);
                });
                let actions = div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .when_some(companion_btn, |row, btn| row.child(btn))
                    .child(search_btn)
                    .child(info_btn)
                    .child(more_btn);
                el.child(actions)
            })
            .into_any_element()
    };

    let _ = (search_query, search_match_cursor, search_match_total, tr_fmt, TopBarUiAction::StartCall { video: false });
    bar
}

pub const CHAT_SEARCH_WIDTH_PX: f32 = 308.0;
pub const CHAT_SEARCH_HEIGHT_PX: f32 = 34.0;
pub const CHAT_SEARCH_RIGHT_OFFSET_PX: f32 = 16.0;

#[derive(Debug, Clone, PartialEq)]
pub struct ChatSearchBarLayout {
    pub open: bool,
    pub width_px: f32,
    pub height_px: f32,
    pub right_offset_px: f32,
    pub match_counter: String,
    pub query: String,
}

pub fn compute_chat_search_bar_layout(
    open: bool,
    query: &str,
    cursor: usize,
    total: usize,
    side_panel_width_px: f32,
) -> ChatSearchBarLayout {
    let match_counter = if total == 0 {
        "0/0".to_string()
    } else {
        format!("{}/{}", cursor.saturating_add(1).min(total), total)
    };
    let safe_side_w = if side_panel_width_px.is_finite() {
        side_panel_width_px.max(0.0)
    } else {
        0.0
    };
    ChatSearchBarLayout {
        open,
        width_px: CHAT_SEARCH_WIDTH_PX,
        height_px: CHAT_SEARCH_HEIGHT_PX,
        right_offset_px: CHAT_SEARCH_RIGHT_OFFSET_PX + safe_side_w,
        match_counter,
        query: query.to_string(),
    }
}

pub fn render_stage_search_bar<V: 'static>(
    search_query: &str,
    search_match_cursor: usize,
    search_match_total: usize,
    side_panel_width_px: f32,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let search_layout = compute_chat_search_bar_layout(
        true,
        search_query,
        search_match_cursor,
        search_match_total,
        side_panel_width_px,
    );
    let safe_side_w = if side_panel_width_px.is_finite() && side_panel_width_px > 0.0 {
        side_panel_width_px
    } else {
        0.0
    };
    let stage_local_right_px =
        (search_layout.right_offset_px - safe_side_w).max(CHAT_SEARCH_RIGHT_OFFSET_PX);
    let a_focus = on_action.clone();
    let a_prev = on_action.clone();
    let a_next = on_action.clone();
    let a_close = on_action;
    let match_lbl = if search_query.is_empty() {
        tr_str(lang_id, "输入关键词搜索", "Type to search").to_string()
    } else if search_match_total == 0 {
        tr_str(lang_id, "无匹配结果", "0 / 0").to_string()
    } else {
        format!("{} / {}", search_match_cursor + 1, search_match_total)
    };
    let display_text = if search_query.is_empty() {
        tr_str(lang_id, "| 搜索当前对话消息…", "| Search messages in this chat…").to_string()
    } else {
        format!("{}|", search_query)
    };

    let is_apple = palette.is_apple_style();
    if is_apple {
        let ob = palette.openbot_palette();
        let hover_bg = ob.hover();
        return div()
            .id("stage-in-chat-search-bar")
            .absolute()
            .top(px(12.0))
            .right(px(stage_local_right_px))
            .w(px(search_layout.width_px))
            .h(px(search_layout.height_px))
            .px(px(10.0))
            .rounded(px(12.0))
            .bg(ob.bg_control())
            .border_1()
            .border_color(ob.border_strong_hairline())
            .shadow(palette.popover_shadows())
            .occlude()
            .flex()
            .items_center()
            .gap(px(6.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_focus(this, TopBarUiAction::FocusSearch, cx);
                }),
            )
            .child(openbot_stage_icon(OpenBotStageIcon::Search, 14.0, ob.text_muted()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(13.0))
                    .text_color(if search_query.is_empty() {
                        ob.text_dim()
                    } else {
                        ob.text_primary()
                    })
                    .child(if search_query.is_empty() {
                        tr_str(lang_id, "搜索消息…", "Find in chat…").to_string()
                    } else {
                        format!("{}|", search_query)
                    }),
            )
            .when(!search_query.is_empty(), |el| {
                el.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.0))
                        .text_color(ob.text_muted())
                        .child(match_lbl),
                )
            })
            .child(
                div()
                    .id("search-prev-btn")
                    .w(px(22.0))
                    .h(px(22.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_prev(this, TopBarUiAction::SearchStep(-1), cx);
                        }),
                    )
                    .child(openbot_stage_icon(OpenBotStageIcon::ChevronUp, 13.0, ob.text_muted())),
            )
            .child(
                div()
                    .id("search-next-btn")
                    .w(px(22.0))
                    .h(px(22.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_next(this, TopBarUiAction::SearchStep(1), cx);
                        }),
                    )
                    .child(openbot_stage_icon(OpenBotStageIcon::ChevronDown, 13.0, ob.text_muted())),
            )
            .child(
                div()
                    .id("search-close-btn")
                    .w(px(22.0))
                    .h(px(22.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_close(this, TopBarUiAction::SearchClear, cx);
                        }),
                    )
                    .child(openbot_stage_icon(OpenBotStageIcon::Close, 13.0, ob.text_muted())),
            )
            .into_any_element();
    }

    let bar_bg: Rgba = rgb(palette.topbar_bg).into();
    let div_col: Rgba = rgb(palette.border_fg).into();

    div()
        .id("stage-in-chat-search-bar")
        .h(px(44.0))
        .w_full()
        .flex_shrink_0()
        .px(px(16.0))
        .bg(bar_bg)
        .border_b_1()
        .border_color(div_col)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                a_focus(this, TopBarUiAction::FocusSearch, cx);
            }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .flex_1()
                .child(tg_icon(IconName::Search, 15.0, palette.accent))
                .child(
                    div()
                        .flex_1()
                        .px(px(12.0))
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .bg(rgb(palette.input_bg))
                        .border_1()
                        .border_color(rgb(palette.accent))
                        .text_size(px(13.0))
                        .text_color(if search_query.is_empty() {
                            rgb(palette.text_secondary)
                        } else {
                            rgb(palette.text_primary)
                        })
                        .child(display_text),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(palette.text_secondary))
                        .child(match_lbl),
                )
                .child(
                    div()
                        .id("search-prev-btn")
                        .px(px(6.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_prev(this, TopBarUiAction::SearchStep(-1), cx);
                            }),
                        )
                        .text_size(px(12.0))
                        .text_color(rgb(palette.icon_fg))
                        .child("▲"),
                )
                .child(
                    div()
                        .id("search-next-btn")
                        .px(px(6.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_next(this, TopBarUiAction::SearchStep(1), cx);
                            }),
                        )
                        .text_size(px(12.0))
                        .text_color(rgb(palette.icon_fg))
                        .child("▼"),
                )
                .child(
                    div()
                        .id("search-close-btn")
                        .p(px(5.0))
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_close(this, TopBarUiAction::SearchClear, cx);
                            }),
                        )
                        .child(tg_icon(IconName::Close, 14.0, palette.icon_fg)),
                ),
        )
        .into_any_element()
}

pub fn render_stage_more_menu_overlay<V: 'static>(
    is_group_chat: bool,
    can_block_peer: bool,
    is_muted_chat: bool,
    is_pinned_chat: bool,
    is_peer_blocked: bool,
    message_ttl_seconds: u32,
    ttl_submenu_open: bool,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let a_dismiss = on_action.clone();
    let a_mute = on_action.clone();
    let a_prof = on_action.clone();
    let a_srch = on_action.clone();
    let a_sel = on_action.clone();
    let a_ttl_toggle = on_action.clone();
    let a_wall = on_action.clone();
    let a_pin = on_action.clone();
    let a_exp = on_action.clone();
    let a_clr = on_action.clone();
    let a_blk = on_action.clone();
    let a_del = on_action.clone();

    let mute_label = if is_muted_chat {
        tr_str(lang_id, "开启通知", "Unmute notifications")
    } else {
        tr_str(lang_id, "静音通知", "Mute notifications")
    };
    let profile_label = if is_group_chat {
        tr_str(lang_id, "查看群组信息", "View group info")
    } else {
        tr_str(lang_id, "查看个人资料", "View profile")
    };
    let pin_label = if is_pinned_chat {
        tr_str(lang_id, "取消置顶对话", "Unpin chat")
    } else {
        tr_str(lang_id, "置顶对话", "Pin chat")
    };
    let block_label = if is_peer_blocked {
        tr_str(lang_id, "取消屏蔽用户", "Unblock user")
    } else {
        tr_str(lang_id, "屏蔽用户", "Block user")
    };
    let delete_label = if is_group_chat {
        tr_str(lang_id, "退出并删除群组", "Leave group")
    } else {
        tr_str(lang_id, "删除对话", "Delete chat")
    };
    let ttl_badge = format_ttl_badge(message_ttl_seconds, lang_id);
    let hover_bg = palette.menu_hover_bg;
    let pal = *palette;

    let ttl_options: [(u32, &'static str, &'static str); 4] = [
        (0, "关闭", "Off"),
        (86_400, "1 天", "1d"),
        (604_800, "1 周", "1w"),
        (2_592_000, "1 月", "1m"),
    ];

    let is_apple = pal.is_apple_style();
    let ob = pal.openbot_palette();
    let menu_bg: Rgba = if is_apple {
        ob.bg_surface()
    } else {
        rgb(pal.menu_bg).into()
    };
    let menu_border: Rgba = if is_apple {
        ob.border_strong_hairline()
    } else {
        rgb(pal.border_fg).into()
    };

    let mut dropdown = div()
        .id("topbar-more-dropdown")
        .absolute()
        .top(px(if is_apple { 8.0 } else { 50.0 }))
        .right(px(12.0))
        .w(px(232.0))
        .p(px(6.0))
        .rounded(px(if is_apple { 14.0 } else { 12.0 }))
        .bg(menu_bg)
        .border_1()
        .border_color(menu_border)
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }),
        )
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(more_menu_item(
            "tb-menu-mute",
            if is_muted_chat { IconName::Speaker } else { IconName::Mute },
            mute_label,
            false,
            palette,
            cx,
            move |this, cx| a_mute(this, TopBarUiAction::ToggleMuteChat, cx),
        ))
        .child(more_menu_item(
            "tb-menu-profile",
            IconName::ContactsUsers,
            profile_label,
            false,
            palette,
            cx,
            move |this, cx| a_prof(this, TopBarUiAction::ViewProfile, cx),
        ))
        .child(more_menu_item(
            "tb-menu-search",
            IconName::Search,
            tr_str(lang_id, "搜索聊天记录", "Search in chat"),
            false,
            palette,
            cx,
            move |this, cx| a_srch(this, TopBarUiAction::ToggleSearch, cx),
        ))
        .child(more_menu_item(
            "tb-menu-select",
            IconName::SelectCheck,
            tr_str(lang_id, "选择消息", "Select messages"),
            false,
            palette,
            cx,
            move |this, cx| a_sel(this, TopBarUiAction::EnterSelectMode, cx),
        ))
        .child(
            div()
                .id("tb-menu-ttl")
                .w_full()
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(if is_apple { 8.0 } else { 6.0 }))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(hover_bg)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_ttl_toggle(this, TopBarUiAction::ToggleTtlSubmenu, cx);
                    }),
                )
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(tg_icon(IconName::ClockSending, 15.0, pal.icon_fg))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(lang_id, "自动删除消息", "Auto-delete").to_string()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if is_apple { ob.text_muted() } else { rgb(pal.accent).into() })
                        .child(ttl_badge),
                ),
        )
        .when(ttl_submenu_open, |el| {
            el.child(
                div()
                    .w_full()
                    .px(px(6.0))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .bg(if is_apple { ob.bg_canvas() } else { rgb(pal.input_bg).into() })
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .children(ttl_options.into_iter().map(|(secs, zh_lbl, en_lbl)| {
                        let is_sel = message_ttl_seconds == secs;
                        let a_set = on_action.clone();
                        let lbl = tr_str(lang_id, zh_lbl, en_lbl).to_string();
                        div()
                            .cursor_pointer()
                            .flex_1()
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .bg(if is_sel {
                                if is_apple { ob.surface_light() } else { rgb(pal.accent).into() }
                            } else if is_apple {
                                ob.bg_control()
                            } else {
                                rgb(pal.card_bg).into()
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if is_sel {
                                if is_apple { ob.text_on_light() } else { rgb(pal.active_button_fg).into() }
                            } else {
                                rgb(pal.text_primary).into()
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_set(this, TopBarUiAction::SetMessageTtl(secs), cx);
                                }),
                            )
                            .child(lbl)
                    })),
            )
        })
        .child(more_menu_item(
            "tb-menu-wallpaper",
            IconName::SunDay,
            tr_str(lang_id, "更换聊天背景", "Change wallpaper"),
            false,
            palette,
            cx,
            move |this, cx| a_wall(this, TopBarUiAction::ChangeWallpaper, cx),
        ))
        .child(more_menu_item(
            "tb-menu-pin-chat",
            IconName::Pin,
            pin_label,
            false,
            palette,
            cx,
            move |this, cx| a_pin(this, TopBarUiAction::TogglePinChat, cx),
        ))
        .child(more_menu_item(
            "tb-menu-export",
            IconName::FileDoc,
            tr_str(lang_id, "导出聊天记录", "Export chat history"),
            false,
            palette,
            cx,
            move |this, cx| a_exp(this, TopBarUiAction::ExportChat, cx),
        ))
        .child(div().h(px(1.0)).my(px(3.0)).bg(menu_border))
        .child(more_menu_item(
            "tb-menu-clear",
            IconName::Edit,
            tr_str(lang_id, "清空聊天记录", "Clear history"),
            false,
            palette,
            cx,
            move |this, cx| a_clr(this, TopBarUiAction::ClearHistory, cx),
        ))
        .when(can_block_peer, |el| {
            el.child(more_menu_item(
                "tb-menu-block",
                IconName::Lock,
                block_label,
                !is_peer_blocked,
                palette,
                cx,
                move |this, cx| a_blk(this, TopBarUiAction::ToggleBlockPeer, cx),
            ))
        })
        .child(more_menu_item(
            "tb-menu-delete-chat",
            IconName::DeleteTrash,
            delete_label,
            true,
            palette,
            cx,
            move |this, cx| a_del(this, TopBarUiAction::DeleteOrLeaveChat, cx),
        ));
    dropdown = if is_apple {
        dropdown.shadow(pal.popover_shadows())
    } else {
        dropdown.shadow_xl()
    };

    div()
        .id("topbar-more-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                a_dismiss(this, TopBarUiAction::DismissTopMoreMenu, cx);
            }),
        )
        .child(deferred(dropdown).with_priority(40))
        .into_any_element()
}

fn more_menu_item<V: 'static>(
    id: &'static str,
    icon: IconName,
    label: &str,
    danger: bool,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let fg = if danger { palette.danger } else { palette.text_primary };
    let icon_fg = if danger { palette.danger } else { palette.icon_fg };
    let hover_bg = palette.menu_hover_bg;
    div()
        .id(id)
        .w_full()
        .px(px(10.0))
        .py(px(7.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(hover_bg)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                on_click(this, cx);
            }),
        )
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(tg_icon(icon, 15.0, icon_fg))
        .child(div().text_size(px(13.0)).text_color(rgb(fg)).child(label.to_string()))
        .into_any_element()
}

fn top_icon_btn<V: 'static>(
    id: &'static str,
    icon: IconName,
    active: bool,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let color = if active { palette.accent } else { palette.icon_fg };
    let hover_bg: Rgba = if is_apple {
        TdesktopThemePalette::rgba_with_alpha(
            palette.text_primary,
            if palette.mode.is_dark() { 0.10 } else { 0.06 },
        )
    } else {
        rgb(palette.menu_hover_bg).into()
    };
    let btn_sz = if is_apple { 30.0 } else { 34.0 };
    let icon_sz = if is_apple { 16.5 } else { 18.0 };
    div()
        .id(id)
        .w(px(btn_sz))
        .h(px(btn_sz))
        .rounded_full()
        .when(is_apple && active, |el| {
            el.bg(TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.12))
        })
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                on_click(this, cx);
            }),
        )
        .child(tg_icon(icon, icon_sz, color))
        .into_any_element()
}

pub fn render_pinned_banner_46px<V: 'static>(
    pinned_snippet: &str,
    pinned_sender: &str,
    pinned_thumb_url: Option<&str>,
    pin_count: usize,
    active_pin_idx: usize,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let a_jump = on_action.clone();
    let a_unpin = on_action.clone();
    let dashes = pin_count.clamp(1, 4);
    let active_dash = active_pin_idx % dashes;
    let dash_h = if dashes <= 1 { 26.0 } else if dashes == 2 { 12.0 } else if dashes == 3 { 8.0 } else { 5.5 };
    let thumb_src = pinned_thumb_url.and_then(|u| resolve_image_source(u, None));
    let header_title = if pin_count > 1 {
        format!(
            "{} #{}/{}",
            tr_key_or(lang_id, "lng_pinned_message", "Pinned Message"),
            active_pin_idx + 1,
            pin_count
        )
    } else if !pinned_sender.is_empty() {
        format!("{}: {}", tr_key_or(lang_id, "lng_pinned_message", "Pinned Message"), pinned_sender)
    } else {
        tr_key_or(lang_id, "lng_pinned_message", "Pinned Message").to_string()
    };

    let is_apple = palette.is_apple_style();
    let bar_bg: Rgba = if is_apple {
        TdesktopThemePalette::rgba_with_alpha(
            palette.topbar_bg,
            if palette.mode.is_dark() { 0.90 } else { 0.92 },
        )
    } else {
        rgb(palette.topbar_bg).into()
    };
    let div_col: Rgba = if is_apple {
        palette.separator_rgba()
    } else {
        rgb(palette.border_fg).into()
    };

    div()
        .h(px(46.0))
        .w_full()
        .px(px(if is_apple { 16.0 } else { 14.0 }))
        .bg(bar_bg)
        .border_b_1()
        .border_color(div_col)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .child(
            div()
                .id("pinned-banner-jump")
                .flex()
                .items_center()
                .gap(px(10.0))
                .flex_1()
                .overflow_hidden()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    a_jump(this, TopBarUiAction::JumpPinned, cx);
                }))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap(px(2.0))
                        .children((0..dashes).map(move |i| {
                            let is_active = i == active_dash;
                            div()
                                .w(px(2.5))
                                .h(px(dash_h))
                                .rounded_full()
                                .bg(if is_active {
                                    rgba((palette.accent << 8) | 0xFF)
                                } else {
                                    TdesktopThemePalette::rgba_with_alpha(palette.accent, 0.32)
                                })
                        })),
                )
                .when_some(thumb_src, |el, src| {
                    el.child(
                        img(src)
                            .w(px(30.0))
                            .h(px(30.0))
                            .rounded(px(4.0))
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(palette.accent))
                                .child(header_title),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(palette.text_secondary))
                                .child(pinned_snippet.to_string()),
                        ),
                ),
        )
        .child(
            div()
                .id("pinned-banner-close")
                .w(px(28.0))
                .h(px(28.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    a_unpin(this, TopBarUiAction::UnpinCurrent, cx);
                }))
                .child(tg_icon(IconName::Close, 14.0, palette.icon_fg)),
        )
        .into_any_element()
}

pub fn render_scroll_to_bottom_fab<V: 'static>(
    unread_count: u32,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let is_apple = palette.is_apple_style();
    let mut fab = div()
        .id("scroll-to-bottom-fab")
        .absolute()
        .bottom(px(16.0))
        .right(px(18.0))
        .w(px(if is_apple { 36.0 } else { 42.0 }))
        .h(px(if is_apple { 36.0 } else { 42.0 }))
        .rounded_full()
        .bg(if is_apple {
            TdesktopThemePalette::rgba_with_alpha(palette.menu_bg, 0.94)
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
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(palette.menu_hover_bg)))
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(tg_icon(IconName::ChevronDown, if is_apple { 17.0 } else { 20.0 }, palette.icon_fg))
        .when(unread_count > 0, |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(-8.0))
                    .right(px(-4.0))
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded_full()
                    .bg(rgb(palette.unread_badge_bg))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(palette.unread_badge_fg))
                    .child(unread_count.to_string()),
            )
        });
    fab = if is_apple {
        fab.shadow(palette.popover_shadows())
    } else {
        fab.shadow_md()
    };
    fab.into_any_element()
}

pub const OPENBOT_HEADER_TAB_FILLET_RADIUS_PX: f32 = 8.0;
pub const OPENBOT_HEADER_TAB_TRANSITION_MS: u32 = 240;
pub const OPENBOT_HEADER_TITLE_BTN_HEIGHT_PX: f32 = 28.0;
pub const OPENBOT_HEADER_TITLE_AVATAR_PX: f32 = 18.0;
pub const OPENBOT_HEADER_MODEL_PICKER_HEIGHT_PX: f32 = 28.0;
pub const OPENBOT_HEADER_PINNED_POPOVER_MAX_WIDTH_PX: f32 = 420.0;
pub const OPENBOT_HEADER_TOGGLE_SIZE_PX: f32 = 28.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderModelPickerFitMode {
    Full,
    Name,
    Icon,
}

impl HeaderModelPickerFitMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Name => "name",
            Self::Icon => "icon",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeaderRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Single-source-of-truth layout snapshot for OpenBot's `38px` non-overlapping `ConversationHeader` (`UI-F09`).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenBotHeaderLayoutSnapshot {
    pub height_px: f32,
    pub position_mode: &'static str,
    pub bg_strip_hex: String,
    pub header_rect: HeaderRect,
    pub timeline_rect: HeaderRect,
    pub overlaps_timeline: bool,
    pub tab_shape_visible: bool,
    pub tab_fillet_radius_px: f32,
    pub tab_fill_hex: String,
    pub tab_transition_ms: u32,
    pub tab_disabled_by_motion_or_glass: bool,
    pub title_btn_height_px: f32,
    pub title_avatar_px: f32,
    pub title_disabled: bool,
    pub title_subtitle_visible: bool,
    pub title_click_target_pane: &'static str,
    pub new_thread_btn_visible: bool,
    pub thread_tabs_count: usize,
    pub model_picker_visible: bool,
    pub model_picker_height_px: f32,
    pub model_picker_disabled: bool,
    pub model_picker_opacity: f32,
    pub model_picker_tooltip: Option<&'static str>,
    pub model_picker_fit_mode: HeaderModelPickerFitMode,
    pub pending_badge_visible: bool,
    pub pinned_popover_open: bool,
    pub pinned_popover_max_width_px: f32,
    pub pinned_count: usize,
    pub pinned_empty_state_visible: bool,
    pub side_panel_toggle_size_px: f32,
    pub side_panel_hairline_visible: bool,
    pub isolation_lock_visible: bool,
    pub group_avatar_layout: ChannelAvatarLayout,
    pub group_avatar_row_limit: usize,
    pub wrapped_to_multiline: bool,
    pub actions_clipped: bool,
}

/// Props for OpenBot `38px` `ConversationHeader` (`UI-F09`).
pub struct OpenBotHeaderProps<'a> {
    pub chat_id: Option<u64>,
    pub title: &'a str,
    pub subtitle: &'a str,
    pub initials: &'a str,
    pub avatar_url: Option<&'a str>,
    pub active_fabushi_bot: Option<&'a FabushiBotSummaryDto>,
    pub is_bot_chat: bool,
    pub is_group_chat: bool,
    pub group_member_tiles: Option<&'a [(String, u32)]>,
    pub group_member_briefs: &'a [ChannelAgentBrief],
    pub bot_activity_state: &'a str,
    pub active_thread_id: Option<&'a str>,
    pub thread_tabs: &'a [(String, String)],
    pub active_model_label: &'a str,
    pub active_thinking_effort: &'a str,
    pub model_picker_open: bool,
    pub pinned_count: usize,
    pub pinned_popover_open: bool,
    pub search_open: bool,
    pub more_menu_open: bool,
    pub side_panel_open: bool,
    pub right_panel_slot_open: bool,
    pub external_hosted: bool,
    pub available_width_px: f32,
    pub is_isolated_lock: bool,
    pub is_archived: bool,
    pub provider_connected: bool,
    pub pending_model_badge: Option<&'a str>,
    pub reduced_motion: bool,
    pub window_glass: bool,
    pub sidebar_closed: bool,
    pub topbar_controls_right_edge_px: f32,
    pub message_ttl_seconds: u32,
}

/// Computes the single-source-of-truth geometry, tab shape, and auto-fit decisions consumed
/// directly by [`render_openbot_conversation_header`].
pub fn compute_openbot_header_layout(
    props: &OpenBotHeaderProps<'_>,
    palette: &TdesktopThemePalette,
) -> OpenBotHeaderLayoutSnapshot {
    let ob = palette.openbot_palette();
    let has_chat = props.chat_id.is_some();
    let is_bot = props.is_bot_chat || props.active_fabushi_bot.is_some();
    let is_group = props.is_group_chat;
    let main_tab_active = props.active_thread_id.is_none();
    let shifted_start_px = if props.sidebar_closed {
        if props.topbar_controls_right_edge_px.is_finite() {
            props.topbar_controls_right_edge_px.clamp(44.0, 2000.0)
        } else {
            44.0
        }
    } else {
        0.0
    };
    let avail_w = if props.available_width_px.is_finite() && props.available_width_px > 0.0 {
        props.available_width_px
    } else {
        0.0
    };
    let header_rect = HeaderRect {
        x: shifted_start_px,
        y: 0.0,
        w: avail_w,
        h: OPENBOT_HEADER_HEIGHT,
    };
    let timeline_rect = HeaderRect {
        x: shifted_start_px,
        y: OPENBOT_HEADER_HEIGHT,
        w: avail_w,
        h: 682.0,
    };
    let overlaps_timeline = (header_rect.y + header_rect.h) > timeline_rect.y;
    let title_click_target_pane = if !main_tab_active {
        "main_chat"
    } else if is_bot && !is_group {
        "assistant_settings"
    } else if is_group {
        "channel_settings"
    } else {
        "contact_profile"
    };
    let model_picker_fit_mode = if avail_w >= 720.0 {
        HeaderModelPickerFitMode::Full
    } else if avail_w >= 520.0 {
        HeaderModelPickerFitMode::Name
    } else {
        HeaderModelPickerFitMode::Icon
    };
    let tab_disabled_by_motion_or_glass = props.reduced_motion || props.window_glass;
    let tab_shape_visible = has_chat && props.external_hosted && !props.window_glass;
    let model_picker_visible = has_chat && is_bot && !is_group;
    let min_title_w = if has_chat { 96.0 } else { 0.0 };
    let model_picker_w = if model_picker_visible {
        match model_picker_fit_mode {
            HeaderModelPickerFitMode::Full => 136.0,
            HeaderModelPickerFitMode::Name => 88.0,
            HeaderModelPickerFitMode::Icon => 32.0,
        }
    } else {
        0.0
    };
    let actions_w = if !has_chat {
        28.0
    } else if props.right_panel_slot_open {
        96.0
    } else {
        132.0
    };
    let required_row_w = min_title_w + model_picker_w + actions_w;
    let wrapped_to_multiline = false;
    let actions_clipped = avail_w <= 0.0 || avail_w < required_row_w;

    OpenBotHeaderLayoutSnapshot {
        height_px: OPENBOT_HEADER_HEIGHT,
        position_mode: "flex_flow",
        bg_strip_hex: ob
            .token("--openbot-bg-frame")
            .unwrap_or("#0d0d0d")
            .to_string(),
        header_rect,
        timeline_rect,
        overlaps_timeline,
        tab_shape_visible,
        tab_fillet_radius_px: OPENBOT_HEADER_TAB_FILLET_RADIUS_PX,
        tab_fill_hex: ob
            .token("--openbot-bg-canvas")
            .unwrap_or("#1a1a1a")
            .to_string(),
        tab_transition_ms: if props.reduced_motion {
            0
        } else {
            OPENBOT_HEADER_TAB_TRANSITION_MS
        },
        tab_disabled_by_motion_or_glass,
        title_btn_height_px: OPENBOT_HEADER_TITLE_BTN_HEIGHT_PX,
        title_avatar_px: OPENBOT_HEADER_TITLE_AVATAR_PX,
        title_disabled: is_group && props.is_archived,
        title_subtitle_visible: !props.subtitle.trim().is_empty() && avail_w >= 720.0,
        title_click_target_pane,
        new_thread_btn_visible: has_chat && is_bot && !is_group,
        thread_tabs_count: if has_chat {
            1 + props.thread_tabs.len()
        } else {
            0
        },
        model_picker_visible,
        model_picker_height_px: OPENBOT_HEADER_MODEL_PICKER_HEIGHT_PX,
        model_picker_disabled: !props.provider_connected,
        model_picker_opacity: if props.provider_connected { 1.0 } else { 0.48 },
        model_picker_tooltip: if props.provider_connected {
            None
        } else {
            Some("Provider disconnected — connect in Settings to change model")
        },
        model_picker_fit_mode,
        pending_badge_visible: props
            .pending_model_badge
            .is_some_and(|s| !s.trim().is_empty()),
        pinned_popover_open: props.pinned_popover_open,
        pinned_popover_max_width_px: OPENBOT_HEADER_PINNED_POPOVER_MAX_WIDTH_PX,
        pinned_count: props.pinned_count,
        pinned_empty_state_visible: props.pinned_popover_open && props.pinned_count == 0,
        side_panel_toggle_size_px: OPENBOT_HEADER_TOGGLE_SIZE_PX,
        side_panel_hairline_visible: !props.right_panel_slot_open,
        isolation_lock_visible: has_chat && props.is_isolated_lock,
        group_avatar_layout: ChannelAvatarLayout::Row,
        group_avatar_row_limit: crate::components::bloub_avatar::CHANNEL_ROW_LIMIT,
        wrapped_to_multiline,
        actions_clipped,
    }
}

fn openbot_header_icon_btn<V: 'static>(
    id: impl Into<ElementId>,
    icon: OpenBotStageIcon,
    active: bool,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let ob = palette.openbot_palette();
    let fg = if active { ob.text_primary() } else { ob.text_muted() };
    let hover_bg = ob.hover();
    let active_bg = ob.bg_control_active();
    div()
        .id(id)
        .w(px(OPENBOT_HEADER_TOGGLE_SIZE_PX))
        .h(px(OPENBOT_HEADER_TOGGLE_SIZE_PX))
        .flex_shrink_0()
        .rounded(px(6.0))
        .when(active, |el| el.bg(active_bg))
        .hover(move |s| s.bg(hover_bg))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                on_click(this, cx);
            }),
        )
        .child(openbot_stage_icon(icon, 15.0, fg))
        .into_any_element()
}

/// Renders OpenBot's exact `38px` non-overlapping `ConversationHeader` with Safari/Arc curved
/// active `TabShape` (`#1a1a1a` dark / `#f5f5f6` light over `#0d0d0d` / `#e8e8eb` frame).
pub fn render_openbot_conversation_header<V: 'static>(
    props: &OpenBotHeaderProps<'_>,
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let layout = compute_openbot_header_layout(props, palette);
    let ob = palette.openbot_palette();
    let is_dark = palette.mode.is_dark();
    let has_chat = props.chat_id.is_some();
    let is_bot = props.is_bot_chat || props.active_fabushi_bot.is_some();
    let is_group = props.is_group_chat;
    let is_human_dm = has_chat && !is_bot && !is_group;
    let frame_bg = if props.external_hosted {
        ob.bg_frame()
    } else {
        ob.bg_canvas()
    };
    let canvas_bg = ob.bg_canvas();
    let tab_border = ob.border_hairline();

    if !has_chat {
        return div()
            .id("openbot-conversation-header")
            .h(px(layout.height_px))
            .w_full()
            .flex_shrink_0()
            .bg(frame_bg)
            .when(!props.external_hosted, |el| {
                el.border_b_1().border_color(tab_border)
            })
            .window_control_area(WindowControlArea::Drag)
            .into_any_element();
    }

    let (mood, activity) = activity_str_to_openbot_mood(props.bot_activity_state);
    let sub_lower = props.subtitle.to_ascii_lowercase();
    let is_online = sub_lower == "online" || props.subtitle == "在线";
    let show_subtitle = layout.title_subtitle_visible;
    let main_tab_active = props.active_thread_id.is_none();

    let avatar_el: AnyElement = if is_group {
        let (members, agents) = build_channel_avatar_inputs(
            props.chat_id.unwrap_or(0),
            props.group_member_tiles,
            props.group_member_briefs,
        );
        let img_url = props.avatar_url.filter(|u| !u.trim().is_empty());
        let size = if img_url.is_some() { 20.0 } else { 18.0 };
        render_channel_avatar(
            &members,
            &agents,
            ChannelAvatarLayout::Row,
            None,
            img_url,
            false,
            size,
            None,
            is_dark,
            0.0,
        )
        .into_any_element()
    } else if is_bot {
        let fallback_seed = format!("bot_{}_{}", props.chat_id.unwrap_or(0), props.title);
        render_fabushi_plush_avatar(
            props.active_fabushi_bot,
            &fallback_seed,
            18.0,
            mood,
            activity,
        )
        .into_any_element()
    } else {
        let pid = props.chat_id.unwrap_or(0) as i64;
        let (grad_top, grad_bot) = palette.peer_avatar_gradient(pid);
        let img_src = props.avatar_url.and_then(|u| resolve_image_source(u, None));
        div()
            .relative()
            .w(px(20.0))
            .h(px(20.0))
            .flex_shrink_0()
            .child(
                div()
                    .w(px(20.0))
                    .h(px(20.0))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(linear_gradient(
                        180.0,
                        linear_color_stop(rgb(grad_top), 0.0),
                        linear_color_stop(rgb(grad_bot), 1.0),
                    ))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when_some(img_src, |el, src| {
                        el.child(img(src).size_full().object_fit(ObjectFit::Cover))
                    })
                    .when(props.avatar_url.map_or(true, |u| u.trim().is_empty()), |el| {
                        el.text_size(px(9.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xFFFFFF))
                            .child(props.initials.to_string())
                    }),
            )
            .when(is_online, |el| {
                el.child(
                    div()
                        .absolute()
                        .bottom(px(-0.5))
                        .right(px(-0.5))
                        .w(px(crate::views::conversation_stage::DIRECT_DM_ONLINE_DOT_PX))
                        .h(px(crate::views::conversation_stage::DIRECT_DM_ONLINE_DOT_PX))
                        .rounded_full()
                        .bg(ob.success())
                        .border_1()
                        .border_color(canvas_bg),
                )
            })
            .into_any_element()
    };

    let a_title = on_action.clone();
    let title_click_action = match layout.title_click_target_pane {
        "main_chat" => TopBarUiAction::SelectThread(None),
        "assistant_settings" => TopBarUiAction::OpenAssistantSettings,
        "channel_settings" => TopBarUiAction::OpenChannelSettings,
        _ => TopBarUiAction::OpenContactProfile,
    };
    let hover_bg = ob.hover();
    let title_inner = div()
        .id("topbar-peer-header")
        .relative()
        .h(px(layout.title_btn_height_px))
        .max_w(px(340.0))
        .px(px(10.0))
        .when(!layout.title_disabled, |el| el.cursor_pointer())
        .flex()
        .items_center()
        .gap(px(7.0))
        .when(layout.tab_shape_visible && main_tab_active, |el| {
            el.rounded_tl(px(layout.tab_fillet_radius_px))
                .rounded_tr(px(layout.tab_fillet_radius_px))
                .bg(canvas_bg)
                .border_t_1()
                .border_l_1()
                .border_r_1()
                .border_color(tab_border)
                // 2px bottom seam bridge covers workspace-card top border seamlessly
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(-1.5))
                        .h(px(2.5))
                        .bg(canvas_bg),
                )
        })
        .when(!layout.tab_shape_visible || !main_tab_active, |el| {
            el.mb(px(4.0))
                .rounded(px(6.0))
                .when(!layout.title_disabled, |s| s.hover(move |h| h.bg(hover_bg)))
        })
        .when(!layout.title_disabled, |el| {
            el.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_title(this, title_click_action.clone(), cx);
                }),
            )
        })
        .child(avatar_el)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(14.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ob.text_primary())
                .child(props.title.to_string()),
        )
        .when(show_subtitle, |el| {
            el.child(
                div()
                    .flex_shrink_0()
                    .max_w(px(120.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.0))
                    .text_color(ob.text_muted())
                    .child(format!("· {}", props.subtitle)),
            )
        })
        .when(layout.isolation_lock_visible, |el| {
            el.child(openbot_stage_icon(OpenBotStageIcon::Lock, 12.0, ob.text_muted()))
        })
        .when(props.message_ttl_seconds > 0, |el| {
            el.child(
                div()
                    .flex_shrink_0()
                    .px(px(5.0))
                    .py(px(1.0))
                    .rounded_full()
                    .bg(ob.bg_control_active())
                    .text_size(px(10.5))
                    .text_color(ob.text_muted())
                    .child(format_ttl_badge(props.message_ttl_seconds, lang_id)),
            )
        });

    let title_tab = if layout.tab_shape_visible && main_tab_active {
        div()
            .flex()
            .items_end()
            .h_full()
            .child(render_tab_concave_fillet(
                true,
                layout.tab_fillet_radius_px,
                canvas_bg,
                tab_border,
            ))
            .child(title_inner)
            .child(render_tab_concave_fillet(
                false,
                layout.tab_fillet_radius_px,
                canvas_bg,
                tab_border,
            ))
            .into_any_element()
    } else {
        div().flex().items_end().h_full().child(title_inner).into_any_element()
    };

    let mut heading_group = div()
        .id("conversation-heading-group")
        .flex()
        .items_end()
        .h_full()
        .gap(px(2.0))
        .min_w(px(0.0))
        .child(title_tab);

    if layout.new_thread_btn_visible {
        for (tid, tlabel) in props.thread_tabs.iter().take(4) {
            let is_sel = props.active_thread_id == Some(tid.as_str());
            let tid_owned = tid.clone();
            let a_sel = on_action.clone();
            let tab_hover = ob.hover();
            let tab_el = div()
                .id(SharedString::from(format!("hdr-thread-tab-{}", tid)))
                .h(px(if is_sel && layout.tab_shape_visible { 32.0 } else { 28.0 }))
                .max_w(px(140.0))
                .px(px(9.0))
                .when(!is_sel || !layout.tab_shape_visible, |el| {
                    el.mb(px(4.0)).rounded(px(6.0)).hover(move |s| s.bg(tab_hover))
                })
                .when(is_sel && layout.tab_shape_visible, |el| {
                    el.rounded_tl(px(8.0))
                        .rounded_tr(px(8.0))
                        .bg(canvas_bg)
                        .border_t_1()
                        .border_l_1()
                        .border_r_1()
                        .border_color(tab_border)
                })
                .cursor_pointer()
                .flex()
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_sel(this, TopBarUiAction::SelectThread(Some(tid_owned.clone())), cx);
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(12.5))
                        .text_color(if is_sel { ob.text_primary() } else { ob.text_muted() })
                        .child(tlabel.clone()),
                );
            heading_group = heading_group.child(tab_el);
        }
        let a_new_thread = on_action.clone();
        heading_group = heading_group.child(
            div()
                .mb(px(4.0))
                .child(openbot_header_icon_btn(
                    "hdr-new-thread-btn",
                    OpenBotStageIcon::Plus,
                    false,
                    palette,
                    cx,
                    move |this, cx| a_new_thread(this, TopBarUiAction::NewThread, cx),
                )),
        );
    }

    let mut right_actions = div()
        .id("conversation-header-actions")
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.0));

    if layout.model_picker_visible {
        let model_lbl = if props.active_model_label.trim().is_empty() {
            tr_str(lang_id, "默认模型", "Auto").to_string()
        } else {
            props.active_model_label.trim().to_string()
        };
        let show_model_text = layout.model_picker_fit_mode != HeaderModelPickerFitMode::Icon;
        let show_effort = !props.active_thinking_effort.trim().is_empty()
            && layout.model_picker_fit_mode == HeaderModelPickerFitMode::Full;
        let a_model = on_action.clone();
        let pill_bg = if props.model_picker_open {
            ob.bg_control_active()
        } else {
            ob.bg_surface()
        };
        let pill_hover = ob.bg_control_hover();
        let pending_badge = props.pending_model_badge.map(str::to_string);
        right_actions = right_actions.child(
            div()
                .id("hdr-model-picker-btn")
                .h(px(layout.model_picker_height_px))
                .px(px(8.0))
                .rounded_full()
                .bg(pill_bg)
                .border_1()
                .border_color(ob.border_hairline())
                .opacity(layout.model_picker_opacity)
                .when(!layout.model_picker_disabled, |el| {
                    el.hover(move |s| s.bg(pill_hover))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_model(this, TopBarUiAction::ToggleModelPicker, cx);
                            }),
                        )
                })
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(
                    div()
                        .w(px(7.0))
                        .h(px(7.0))
                        .rounded_full()
                        .flex_shrink_0()
                        .bg(if layout.model_picker_disabled {
                            ob.text_dim()
                        } else {
                            ob.success()
                        }),
                )
                .when(show_model_text, |el| {
                    el.child(
                        div()
                            .max_w(px(140.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_primary())
                            .child(model_lbl),
                    )
                })
                .when(show_effort, |el| {
                    el.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.5))
                            .text_color(ob.text_muted())
                            .child(format!("· {}", props.active_thinking_effort)),
                    )
                })
                .when(layout.pending_badge_visible, |el| {
                    el.when_some(pending_badge, |row, badge| {
                        row.child(
                            div()
                                .flex_shrink_0()
                                .px(px(5.0))
                                .py(px(1.0))
                                .rounded_full()
                                .bg(ob.bg_control_active())
                                .text_size(px(10.5))
                                .text_color(ob.text_secondary())
                                .child(badge),
                        )
                    })
                })
                .child(openbot_stage_icon(
                    OpenBotStageIcon::ChevronDown,
                    12.0,
                    ob.text_muted(),
                )),
        );
    } else if is_human_dm && props.available_width_px >= 520.0 {
        right_actions = right_actions.child(
            div()
                .h(px(24.0))
                .px(px(7.0))
                .rounded_full()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(openbot_stage_icon(OpenBotStageIcon::Lock, 11.5, ob.text_dim()))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(ob.text_dim())
                        .child(tr_str(lang_id, "私密", "Private")),
                ),
        );
    }

    let a_pin = on_action.clone();
    let pin_hover = ob.hover();
    let pin_active_bg = ob.bg_control_active();
    right_actions = right_actions.child(
        div()
            .id("hdr-pinned-btn")
            .relative()
            .w(px(28.0))
            .h(px(28.0))
            .flex_shrink_0()
            .rounded(px(6.0))
            .when(props.pinned_popover_open, |el| el.bg(pin_active_bg))
            .hover(move |s| s.bg(pin_hover))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    a_pin(this, TopBarUiAction::TogglePinnedMessagesPopover, cx);
                }),
            )
            .child(openbot_stage_icon(
                OpenBotStageIcon::Pin,
                14.5,
                if props.pinned_popover_open || props.pinned_count > 0 {
                    ob.text_primary()
                } else {
                    ob.text_muted()
                },
            ))
            .when(props.pinned_count > 0, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(4.0))
                        .right(px(4.0))
                        .w(px(5.0))
                        .h(px(5.0))
                        .rounded_full()
                        .bg(ob.text_primary()),
                )
            }),
    );

    let a_search = on_action.clone();
    right_actions = right_actions.child(openbot_header_icon_btn(
        "tb-search",
        OpenBotStageIcon::Search,
        props.search_open,
        palette,
        cx,
        move |this, cx| a_search(this, TopBarUiAction::ToggleSearch, cx),
    ));

    let a_more = on_action.clone();
    right_actions = right_actions.child(openbot_header_icon_btn(
        "tb-more",
        OpenBotStageIcon::MoreHorizontal,
        props.more_menu_open,
        palette,
        cx,
        move |this, cx| a_more(this, TopBarUiAction::ToggleTopMoreMenu, cx),
    ));

    if !props.right_panel_slot_open {
        let a_side = on_action.clone();
        right_actions = right_actions
            .child(
                div()
                    .w(px(1.0))
                    .h(px(20.0))
                    .mx(px(2.0))
                    .bg(ob.border_hairline()),
            )
            .child(openbot_header_icon_btn(
                "hdr-side-panel-toggle",
                OpenBotStageIcon::PanelRight,
                props.side_panel_open,
                palette,
                cx,
                move |this, cx| a_side(this, TopBarUiAction::ToggleSidePanel, cx),
            ));
    }

    div()
        .id("openbot-conversation-header")
        .h(px(layout.height_px))
        .w_full()
        .flex_shrink_0()
        .when(!layout.wrapped_to_multiline, |el| el.overflow_hidden())
        .px(px(8.0))
        .when(props.sidebar_closed && !props.external_hosted, |el| {
            el.pl(px(layout.header_rect.x.max(8.0)))
        })
        .bg(frame_bg)
        .when(!props.external_hosted, |el| {
            el.border_b_1().border_color(tab_border)
        })
        .flex()
        .items_center()
        .gap(px(if layout.actions_clipped { 2.0 } else { 6.0 }))
        .child(heading_group)
        .child(
            div()
                .flex_1()
                .min_w(px(if layout.actions_clipped { 4.0 } else { 16.0 }))
                .h_full()
                .window_control_area(WindowControlArea::Drag),
        )
        .child(right_actions)
        .into_any_element()
}

/// Renders the far-right `38px` top-chrome slot aligned over an open right `SidePanel` / `Inspector`.
pub fn render_openbot_right_panel_top_slot<V: 'static>(
    width_px: f32,
    palette: &TdesktopThemePalette,
    cx: &mut Context<V>,
    on_toggle_side_panel: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let ob = palette.openbot_palette();
    div()
        .id("openbot-right-panel-top-slot")
        .w(px(width_px))
        .h(px(OPENBOT_HEADER_HEIGHT))
        .flex_shrink_0()
        .bg(ob.bg_frame())
        .px(px(8.0))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex_1()
                .h_full()
                .window_control_area(WindowControlArea::Drag),
        )
        .child(openbot_header_icon_btn(
            "hdr-side-panel-toggle-right",
            OpenBotStageIcon::PanelRight,
            true,
            palette,
            cx,
            on_toggle_side_panel,
        ))
        .into_any_element()
}

/// Renders OpenBot's `420px`-max `PinnedMessages` popover anchored under the header `Pin` icon.
pub fn render_openbot_pinned_messages_popover<V: 'static>(
    entries: &[(u64, String, String)],
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let ob = palette.openbot_palette();
    let a_dismiss = on_action.clone();
    let hover_bg = ob.hover();

    let mut card = div()
        .id("openbot-pinned-messages-popover")
        .absolute()
        .top(px(8.0))
        .right(px(48.0))
        .w(px(360.0))
        .max_w(px(OPENBOT_HEADER_PINNED_POPOVER_MAX_WIDTH_PX))
        .max_h(px(340.0))
        .rounded(px(14.0))
        .bg(ob.bg_surface())
        .border_1()
        .border_color(ob.border_strong_hairline())
        .shadow(palette.popover_shadows())
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(12.0))
                .py(px(10.0))
                .border_b_1()
                .border_color(ob.border_hairline())
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(openbot_stage_icon(OpenBotStageIcon::Pin, 14.0, ob.text_muted()))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.text_primary())
                                .child(format!(
                                    "{} ({})",
                                    tr_str(lang_id, "置顶消息", "Pinned messages"),
                                    entries.len()
                                )),
                        ),
                ),
        );

    if entries.is_empty() {
        card = card.child(
            div()
                .px(px(16.0))
                .py(px(24.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.0))
                .child(openbot_stage_icon(OpenBotStageIcon::Pin, 20.0, ob.text_dim()))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(ob.text_muted())
                        .child(tr_str(lang_id, "暂无置顶消息", "No pinned messages yet")),
                ),
        );
    } else {
        let mut list = div()
            .id("openbot-pinned-popover-scroll")
            .flex_1()
            .max_h(px(280.0))
            .overflow_y_scroll()
            .p(px(6.0))
            .flex()
            .flex_col()
            .gap(px(2.0));
        for (msg_id, sender, snippet) in entries {
            let id = *msg_id;
            let a_jump = on_action.clone();
            let a_unpin = on_action.clone();
            let unpin_hover = ob.bg_control_active();
            list = list.child(
                div()
                    .id(SharedString::from(format!("pinned-pop-row-{}", id)))
                    .w_full()
                    .px(px(8.0))
                    .py(px(7.0))
                    .rounded(px(8.0))
                    .hover(move |s| s.bg(hover_bg))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            a_jump(this, TopBarUiAction::JumpPinnedMessage(id), cx);
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ob.text_primary())
                                    .child(sender.clone()),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(12.5))
                                    .text_color(ob.text_muted())
                                    .child(snippet.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("pinned-pop-unpin-{}", id)))
                            .w(px(24.0))
                            .h(px(24.0))
                            .flex_shrink_0()
                            .rounded(px(6.0))
                            .hover(move |s| s.bg(unpin_hover))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    a_unpin(this, TopBarUiAction::UnpinMessage(id), cx);
                                }),
                            )
                            .child(openbot_stage_icon(OpenBotStageIcon::Close, 12.0, ob.text_muted())),
                    ),
            );
        }
        card = card.child(list);
    }

    div()
        .id("openbot-pinned-popover-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                a_dismiss(this, TopBarUiAction::TogglePinnedMessagesPopover, cx);
            }),
        )
        .child(deferred(card).with_priority(42))
        .into_any_element()
}

/// Renders OpenBot's `ProviderModelPicker` dropdown popover anchored under the header model pill.
pub fn render_openbot_model_picker_popover<V: 'static>(
    active_model: &str,
    active_effort: &str,
    models: &[String],
    palette: &TdesktopThemePalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, TopBarUiAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let ob = palette.openbot_palette();
    let a_dismiss = on_action.clone();
    let hover_bg = ob.hover();
    let fallback_models: Vec<String> = if models.is_empty() {
        vec![
            "claude-sonnet-4-5".to_string(),
            "gpt-5".to_string(),
            "gemini-2.5-pro".to_string(),
        ]
    } else {
        models.to_vec()
    };
    let efforts = [
        ("low", tr_str(lang_id, "轻度", "Low")),
        ("medium", tr_str(lang_id, "标准", "Medium")),
        ("high", tr_str(lang_id, "深度", "High")),
    ];

    let mut list = div()
        .id("openbot-model-picker-scroll")
        .max_h(px(240.0))
        .overflow_y_scroll()
        .p(px(6.0))
        .flex()
        .flex_col()
        .gap(px(2.0));
    for m in fallback_models {
        let is_sel = !active_model.is_empty() && active_model.eq_ignore_ascii_case(&m);
        let m_clone = m.clone();
        let effort_clone = (!active_effort.is_empty()).then(|| active_effort.to_string());
        let a_sel = on_action.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!("model-pick-{}", m)))
                .h(px(30.0))
                .px(px(8.0))
                .rounded(px(8.0))
                .when(is_sel, |el| el.bg(ob.bg_control_active()))
                .hover(move |s| s.bg(hover_bg))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        a_sel(
                            this,
                            TopBarUiAction::SelectModel {
                                model_id: m_clone.clone(),
                                thinking_level: effort_clone.clone(),
                            },
                            cx,
                        );
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(13.0))
                        .text_color(ob.text_primary())
                        .child(m),
                )
                .when(is_sel, |el| {
                    el.child(openbot_stage_icon(OpenBotStageIcon::Check, 13.0, ob.text_primary()))
                }),
        );
    }

    let effort_row = div()
        .px(px(8.0))
        .py(px(8.0))
        .border_t_1()
        .border_color(ob.border_hairline())
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(11.5))
                .text_color(ob.text_muted())
                .child(tr_str(lang_id, "推理强度", "Effort")),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .children(efforts.into_iter().map(|(val, lbl)| {
                    let is_sel = active_effort.eq_ignore_ascii_case(val);
                    let model_id = active_model.to_string();
                    let eff = val.to_string();
                    let a_eff = on_action.clone();
                    div()
                        .id(SharedString::from(format!("model-effort-{}", val)))
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(if is_sel { ob.surface_light() } else { ob.bg_control() })
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if is_sel { ob.text_on_light() } else { ob.text_muted() })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                a_eff(
                                    this,
                                    TopBarUiAction::SelectModel {
                                        model_id: model_id.clone(),
                                        thinking_level: Some(eff.clone()),
                                    },
                                    cx,
                                );
                            }),
                        )
                        .child(lbl)
                })),
        );

    let card = div()
        .id("openbot-model-picker-popover")
        .absolute()
        .top(px(8.0))
        .right(px(84.0))
        .w(px(268.0))
        .rounded(px(14.0))
        .bg(ob.bg_surface())
        .border_1()
        .border_color(ob.border_strong_hairline())
        .shadow(palette.popover_shadows())
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .flex()
        .flex_col()
        .child(list)
        .child(effort_row);

    div()
        .id("openbot-model-picker-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                a_dismiss(this, TopBarUiAction::ToggleModelPicker, cx);
            }),
        )
        .child(deferred(card).with_priority(44))
        .into_any_element()
}
