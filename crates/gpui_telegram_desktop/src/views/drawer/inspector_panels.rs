//! Inspector Drawer sub-panels: Apple Messages Contact Profile, Members,
//! Shared Media/Files/Links/Music/Voice, and Settings & Statistics.

use crate::i18n::tr_str;
use crate::theme::assets::{IconName, tg_icon};
use crate::theme::palette::TdesktopThemePalette;
use crate::views::drawer::inspector_tabs::{
    InspectorAction, InspectorTab, MembersTabModel, ProfileTabModel, SettingsTabModel,
    SharedFileItem, SharedMediaItem, SharedMediaSubTab, SharedMediaTabModel,
};
use crate::views::drawer::statistics::info_statistics_inner_widget::ChartDataPoint;
use crate::views::ContextInspector;
use gpui::prelude::FluentBuilder;
use gpui::*;

pub(crate) fn segmented_active_bg(pal: TdesktopThemePalette) -> Rgba {
    if pal.mode.is_dark() {
        rgb(pal.menu_hover_bg)
    } else {
        rgb(pal.modal_bg)
    }
}

pub(crate) fn segmented_pill_shadow(pal: TdesktopThemePalette) -> Vec<BoxShadow> {
    if pal.mode.is_dark() {
        vec![BoxShadow {
            color: hsla(0.0, 0.0, 0.0, 0.34),
            offset: point(px(0.0), px(1.0)),
            blur_radius: px(2.5),
            spread_radius: px(0.0),
        }]
    } else {
        vec![BoxShadow {
            color: hsla(0.0, 0.0, 0.0, 0.12),
            offset: point(px(0.0), px(1.0)),
            blur_radius: px(2.5),
            spread_radius: px(0.0),
        }]
    }
}

fn grouped_card(pal: TdesktopThemePalette) -> Div {
    div()
        .w_full()
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.separator_rgba())
        .overflow_hidden()
}

fn card_hairline(pal: TdesktopThemePalette) -> Div {
    div()
        .mx(px(12.0))
        .h(px(1.0))
        .bg(pal.separator_rgba())
}

fn apple_quick_action_tile(
    id: &'static str,
    icon: IconName,
    icon_color: u32,
    label: &str,
    label_color: u32,
    pal: TdesktopThemePalette,
    on_tap: impl Fn(&mut ContextInspector) + 'static,
    cx: &mut Context<ContextInspector>,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .flex_1()
        .min_w(px(0.0))
        .py(px(10.0))
        .px(px(4.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.separator_rgba())
        .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(5.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _e, _w, cx| {
                on_tap(this);
                cx.notify();
            }),
        )
        .child(tg_icon(icon, 17.0, icon_color))
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(label_color))
                .truncate()
                .child(label.to_string()),
        )
}

fn apple_switch_pill(enabled: bool, pal: TdesktopThemePalette) -> Div {
    let track_bg = if enabled { pal.success } else { pal.input_bg };
    div()
        .w(px(34.0))
        .h(px(20.0))
        .rounded_full()
        .bg(rgb(track_bg))
        .border_1()
        .border_color(if enabled {
            rgb(track_bg).into()
        } else {
            pal.separator_rgba()
        })
        .px(px(2.0))
        .flex()
        .items_center()
        .justify_end()
        .when(!enabled, |el| el.justify_start())
        .child(
            div()
                .w(px(14.0))
                .h(px(14.0))
                .rounded_full()
                .bg(rgb(0xFFFFFF))
                .shadow(vec![BoxShadow {
                    color: hsla(0.0, 0.0, 0.0, 0.22),
                    offset: point(px(0.0), px(1.0)),
                    blur_radius: px(2.0),
                    spread_radius: px(0.0),
                }]),
        )
}

fn render_media_thumb_tile(
    item: &SharedMediaItem,
    tile_px: f32,
    pal: TdesktopThemePalette,
    cx: &mut Context<ContextInspector>,
) -> Div {
    let url = item.url.clone();
    let is_vid = item.is_video;
    let thumb = item
        .local_path
        .as_ref()
        .map(|p| {
            img(p.clone())
                .w(px(tile_px))
                .h(px(tile_px))
                .object_fit(ObjectFit::Cover)
                .into_any_element()
        })
        .unwrap_or_else(|| {
            tg_icon(
                if is_vid {
                    IconName::VideoCall
                } else {
                    IconName::Camera
                },
                22.0,
                pal.accent,
            )
            .into_any_element()
        });

    div()
        .relative()
        .cursor_pointer()
        .w(px(tile_px))
        .h(px(tile_px))
        .rounded(px(8.0))
        .overflow_hidden()
        .bg(rgb(pal.input_bg))
        .border_1()
        .border_color(pal.separator_rgba())
        .hover(move |s| s.border_color(rgb(pal.accent)))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _e, _w, cx| {
                this.pending_media_open = Some((url.clone(), is_vid));
                this.pending_action = Some(InspectorAction::OpenMedia {
                    url: url.clone(),
                    is_video: is_vid,
                });
                cx.notify();
            }),
        )
        .child(thumb)
        .when(is_vid, |tile| {
            tile.child(
                div()
                    .absolute()
                    .bottom(px(5.0))
                    .right(px(5.0))
                    .w(px(20.0))
                    .h(px(20.0))
                    .rounded_full()
                    .bg(TdesktopThemePalette::rgba_with_alpha(0x000000, 0.58))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(tg_icon(IconName::Play, 10.0, 0xFFFFFF)),
            )
        })
}

/// Renders the flagship Apple Messages Contact Info profile panel:
/// centered 74px hero avatar, 4-tile quick action strip, grouped contact facts card,
/// optional 3-column shared media preview card, privacy toggle, and red destructive card.
#[allow(clippy::too_many_arguments)]
pub fn render_contact_profile_panel(
    model: &ProfileTabModel,
    shared_media: &SharedMediaTabModel,
    is_secret_chat_active: bool,
    is_contact_blocked: bool,
    copied_feedback: Option<&String>,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let peer_name = model.name.clone();
    let peer_for_vid = model.name.clone();
    let avatar_color = if model.avatar_color != 0 {
        model.avatar_color
    } else {
        pal.accent
    };
    let avatar_px = if pal.is_apple_style() { 74.0 } else { 80.0 };

    let fallback_initials = {
        let trimmed_init = model.avatar_initials.trim();
        if !trimmed_init.is_empty() && trimmed_init != "?" {
            trimmed_init.to_string()
        } else {
            let derived = crate::views::app_shell::shell_truth::derive_initials(&model.name);
            if derived.is_empty() {
                "?".to_string()
            } else {
                derived
            }
        }
    };

    let avatar_shadow = vec![BoxShadow {
        color: hsla(0.0, 0.0, 0.0, if pal.mode.is_dark() { 0.36 } else { 0.14 }),
        offset: point(px(0.0), px(4.0)),
        blur_radius: px(12.0),
        spread_radius: px(0.0),
    }];

    let avatar = if let Some(ref path) = model.avatar_path {
        div()
            .w(px(avatar_px))
            .h(px(avatar_px))
            .rounded_full()
            .overflow_hidden()
            .border_1()
            .border_color(pal.separator_rgba())
            .shadow(avatar_shadow)
            .child(
                img(path.clone())
                    .w(px(avatar_px))
                    .h(px(avatar_px))
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element()
    } else {
        div()
            .w(px(avatar_px))
            .h(px(avatar_px))
            .rounded_full()
            .bg(rgb(avatar_color))
            .border_1()
            .border_color(pal.separator_rgba())
            .shadow(avatar_shadow)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(25.0))
            .font_weight(FontWeight::BOLD)
            .text_color(rgb(pal.active_button_fg))
            .child(fallback_initials)
            .into_any_element()
    };

    let status_trimmed = model.status.trim();
    let is_online = status_trimmed.eq_ignore_ascii_case("online") || status_trimmed == "在线";
    let clean_handle = model.username.trim();
    let subtitle_text = if !status_trimmed.is_empty() && !clean_handle.is_empty() {
        format!("{clean_handle} · {status_trimmed}")
    } else if !status_trimmed.is_empty() {
        status_trimmed.to_string()
    } else {
        clean_handle.to_string()
    };

    let mut rows: Vec<(String, String, bool)> = Vec::new();
    if !model.phone.is_empty() {
        rows.push((
            tr_str(lang, "手机号", "Mobile").into(),
            model.phone.clone(),
            true,
        ));
    }
    if !model.username.is_empty() {
        rows.push((
            tr_str(lang, "用户名", "Username").into(),
            model.username.clone(),
            true,
        ));
    }
    if !model.bio.is_empty() {
        rows.push((
            tr_str(lang, "简介", "Bio").into(),
            model.bio.clone(),
            false,
        ));
    }
    for (label, val) in &model.facts {
        rows.push((label.clone(), val.clone(), false));
    }

    let mute_icon_color = if model.is_notifications_enabled {
        pal.accent
    } else {
        pal.text_secondary
    };
    let mute_label = if model.is_notifications_enabled {
        tr_str(lang, "静音", "Mute")
    } else {
        tr_str(lang, "已静音", "Unmute")
    };

    let facts_card = if rows.is_empty() {
        None
    } else {
        let mut card = grouped_card(pal).flex().flex_col();
        let last_idx = rows.len().saturating_sub(1);
        for (idx, (label, val, is_link)) in rows.into_iter().enumerate() {
            let cp = val.clone();
            let copied_prefix = tr_str(lang, "已复制", "Copied").to_string();
            card = card.child(
                div()
                    .cursor_pointer()
                    .w_full()
                    .px(px(12.0))
                    .py(px(10.0))
                    .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _e, _w, cx| {
                            this.copied_feedback = Some(format!("{copied_prefix}: {cp}"));
                            this.pending_action = Some(InspectorAction::CopyText(cp.clone()));
                            cx.notify();
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
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(pal.text_secondary))
                                    .child(label),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(rgb(if is_link {
                                        pal.text_link
                                    } else {
                                        pal.text_primary
                                    }))
                                    .child(val),
                            ),
                    )
                    .child(tg_icon(IconName::Copy, 13.0, pal.text_secondary)),
            );
            if idx < last_idx {
                card = card.child(card_hairline(pal));
            }
        }
        Some(card)
    };

    let media_preview_card = if shared_media.media_items.is_empty() {
        None
    } else {
        Some(
            grouped_card(pal)
                .p(px(12.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(lang, "照片与媒体", "Photos & Media")),
                        )
                        .child(
                            div()
                                .id("inspector-media-see-all")
                                .cursor_pointer()
                                .text_size(px(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(pal.accent))
                                .hover(|s| s.underline())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _e, _w, cx| {
                                        this.set_tab(InspectorTab::SharedMedia);
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "查看全部", "See All")),
                        ),
                )
                .child(
                    div().flex().flex_wrap().gap(px(6.0)).children(
                        shared_media
                            .media_items
                            .iter()
                            .take(6)
                            .map(|item| render_media_thumb_tile(item, 84.0, pal, cx)),
                    ),
                ),
        )
    };

    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        // Hero Profile Section
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .pt(px(4.0))
                .pb(px(4.0))
                .gap(px(6.0))
                .child(avatar)
                .child(
                    div()
                        .pt(px(4.0))
                        .text_size(px(18.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .text_center()
                        .child(if model.name.is_empty() {
                            tr_str(lang, "未选择对话", "No chat selected").to_string()
                        } else {
                            model.name.clone()
                        }),
                )
                .when(!subtitle_text.is_empty(), |col| {
                    col.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(5.0))
                            .when(is_online, |row| {
                                row.child(
                                    div()
                                        .w(px(7.0))
                                        .h(px(7.0))
                                        .rounded_full()
                                        .bg(rgb(pal.online_dot_fg)),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(subtitle_text),
                            ),
                    )
                }),
        )
        // 4-Tile Apple Quick Action Strip
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(apple_quick_action_tile(
                    "inspector-quick-audio",
                    IconName::PhoneCall,
                    pal.accent,
                    tr_str(lang, "语音", "Audio"),
                    pal.accent,
                    pal,
                    move |this| {
                        this.pending_action =
                            Some(InspectorAction::StartVoiceCall(peer_name.clone()));
                    },
                    cx,
                ))
                .child(apple_quick_action_tile(
                    "inspector-quick-video",
                    IconName::VideoCall,
                    pal.accent,
                    tr_str(lang, "视频", "Video"),
                    pal.accent,
                    pal,
                    move |this| {
                        this.pending_action =
                            Some(InspectorAction::StartVideoCall(peer_for_vid.clone()));
                    },
                    cx,
                ))
                .child(apple_quick_action_tile(
                    "inspector-quick-mute",
                    IconName::Mute,
                    mute_icon_color,
                    mute_label,
                    mute_icon_color,
                    pal,
                    |this| {
                        this.profile.is_notifications_enabled =
                            !this.profile.is_notifications_enabled;
                        this.pending_action = Some(InspectorAction::ToggleNotifications(
                            this.profile.is_notifications_enabled,
                        ));
                    },
                    cx,
                ))
                .child(apple_quick_action_tile(
                    "inspector-quick-info",
                    IconName::Edit,
                    pal.accent,
                    tr_str(lang, "信息", "Info"),
                    pal.accent,
                    pal,
                    |this| {
                        this.pending_action = Some(InspectorAction::OpenEditPeer);
                    },
                    cx,
                )),
        )
        // Bio & Contact Facts Grouped Card
        .when_some(facts_card, |col, card| col.child(card))
        .when_some(copied_feedback.cloned(), |col, hint| {
            col.child(
                div()
                    .w_full()
                    .px(px(12.0))
                    .py(px(7.0))
                    .rounded(px(10.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.12))
                    .border_1()
                    .border_color(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.28))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(pal.success))
                    .child(hint),
            )
        })
        // Shared Media 3-Column Preview Card
        .when_some(media_preview_card, |col, card| col.child(card))
        // Privacy & Secret Chat Card
        .child(
            grouped_card(pal).child(
                div()
                    .id("inspector-secret-chat-toggle")
                    .cursor_pointer()
                    .w_full()
                    .px(px(12.0))
                    .py(px(11.0))
                    .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _e, _w, cx| {
                            this.is_secret_chat_active = !this.is_secret_chat_active;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(9.0))
                            .child(tg_icon(
                                IconName::Lock,
                                15.0,
                                if is_secret_chat_active {
                                    pal.success
                                } else {
                                    pal.accent
                                },
                            ))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(pal.text_primary))
                                    .child(if is_secret_chat_active {
                                        tr_str(lang, "端到端加密已开启", "Secret Chat Active")
                                    } else {
                                        tr_str(lang, "开启加密聊天", "Start Secret Chat")
                                    }),
                            ),
                    )
                    .child(apple_switch_pill(is_secret_chat_active, pal)),
            ),
        )
        // Cleanly Separated Red Destructive Action Card
        .child(
            grouped_card(pal).child(
                div()
                    .id("inspector-block-contact-row")
                    .cursor_pointer()
                    .w_full()
                    .px(px(12.0))
                    .py(px(11.0))
                    .hover(move |s| {
                        s.bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.10))
                    })
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _e, _w, cx| {
                            this.is_contact_blocked = !this.is_contact_blocked;
                            cx.notify();
                        }),
                    )
                    .child(tg_icon(IconName::Close, 15.0, pal.danger))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(pal.danger))
                            .child(if is_contact_blocked {
                                tr_str(lang, "取消屏蔽联系人", "Unblock Contact")
                            } else {
                                tr_str(lang, "屏蔽此联系人", "Block Contact")
                            }),
                    ),
            ),
        )
        .into_any_element()
}

pub fn render_members_panel(
    model: &MembersTabModel,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let invite = model.invite_link.clone();
    let members_card = if model.members.is_empty() {
        grouped_card(pal)
            .child(empty_panel(
                IconName::GroupUsers,
                tr_str(lang, "暂无成员列表", "No members loaded"),
                pal,
            ))
            .into_any_element()
    } else {
        let mut card = grouped_card(pal).flex().flex_col();
        let last_idx = model.members.len().saturating_sub(1);
        for (idx, m) in model.members.iter().enumerate() {
            let mid = m.id;
            card = card.child(
                div()
                    .cursor_pointer()
                    .w_full()
                    .h(px(52.0))
                    .px(px(12.0))
                    .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                    .flex()
                    .items_center()
                    .justify_between()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _e, _w, cx| {
                            this.pending_action = Some(InspectorAction::CycleMemberRole(mid));
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .w(px(34.0))
                                    .h(px(34.0))
                                    .rounded_full()
                                    .bg(rgb(m.color))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .flex_shrink_0()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.active_button_fg))
                                    .child(m.initials.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w(px(0.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(pal.text_primary))
                                            .truncate()
                                            .child(m.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .when(m.is_online, |row| {
                                                row.child(
                                                    div()
                                                        .w(px(6.0))
                                                        .h(px(6.0))
                                                        .rounded_full()
                                                        .bg(rgb(pal.online_dot_fg)),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(if m.is_online {
                                                        rgb(pal.online_dot_fg)
                                                    } else {
                                                        rgb(pal.text_secondary)
                                                    })
                                                    .child(if m.is_online {
                                                        "online"
                                                    } else {
                                                        "offline"
                                                    }),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(2.5))
                            .rounded(px(999.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.accent))
                            .child(m.role.clone()),
                    ),
            );
            if idx < last_idx {
                card = card.child(card_hairline(pal));
            }
        }
        card.into_any_element()
    };

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(2.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "{} {}",
                            model.members.len(),
                            tr_str(lang, "位成员", "MEMBERS")
                        )),
                )
                .child(
                    div()
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(999.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                        .hover(move |s| {
                            s.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.22))
                        })
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _e, _w, cx| {
                                this.pending_action = Some(InspectorAction::OpenAddMember);
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "+ 添加成员", "+ Add Member").to_string()),
                ),
        )
        .when(!invite.is_empty(), |el| {
            let cp = invite.clone();
            el.child(
                grouped_card(pal)
                    .p(px(12.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(lang, "邀请链接", "Invite Link")),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(rgb(pal.text_link))
                                    .truncate()
                                    .child(invite),
                            ),
                    )
                    .child(
                        div()
                            .cursor_pointer()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded_full()
                            .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _e, _w, cx| {
                                    this.pending_action =
                                        Some(InspectorAction::CopyText(cp.clone()));
                                    cx.notify();
                                }),
                            )
                            .child(tg_icon(IconName::Copy, 14.0, pal.icon_fg)),
                    ),
            )
        })
        .child(members_card)
        .into_any_element()
}

pub fn render_shared_media_panel(
    model: &SharedMediaTabModel,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let tabs = [
        (tr_str(lang, "媒体", "Media"), SharedMediaSubTab::Media),
        (tr_str(lang, "文件", "Files"), SharedMediaSubTab::Files),
        (tr_str(lang, "链接", "Links"), SharedMediaSubTab::Links),
        (tr_str(lang, "音乐", "Music"), SharedMediaSubTab::Music),
        (tr_str(lang, "语音", "Voice"), SharedMediaSubTab::Voice),
    ];

    let body = match model.active_subtab {
        SharedMediaSubTab::Media if model.media_items.is_empty() => grouped_card(pal)
            .child(empty_panel(
                IconName::Camera,
                tr_str(lang, "暂无共享媒体", "No shared media yet"),
                pal,
            ))
            .into_any_element(),
        SharedMediaSubTab::Media => grouped_card(pal)
            .p(px(10.0))
            .child(
                div().flex().flex_wrap().gap(px(6.0)).children(
                    model
                        .media_items
                        .iter()
                        .map(|item| render_media_thumb_tile(item, 85.0, pal, cx)),
                ),
            )
            .into_any_element(),
        SharedMediaSubTab::Files if model.files.is_empty() => grouped_card(pal)
            .child(empty_panel(
                IconName::FileDoc,
                tr_str(lang, "暂无共享文件", "No shared files yet"),
                pal,
            ))
            .into_any_element(),
        SharedMediaSubTab::Files => render_file_list(&model.files, pal, cx),
        SharedMediaSubTab::Music if model.music_items.is_empty() => grouped_card(pal)
            .child(empty_panel(
                IconName::Speaker,
                tr_str(lang, "暂无共享音乐", "No shared music yet"),
                pal,
            ))
            .into_any_element(),
        SharedMediaSubTab::Music => render_file_list(&model.music_items, pal, cx),
        SharedMediaSubTab::Links if model.links.is_empty() => grouped_card(pal)
            .child(empty_panel(
                IconName::Forward,
                tr_str(lang, "暂无共享链接", "No shared links yet"),
                pal,
            ))
            .into_any_element(),
        SharedMediaSubTab::Links => {
            let mut card = grouped_card(pal).flex().flex_col();
            let last_idx = model.links.len().saturating_sub(1);
            for (idx, l) in model.links.iter().enumerate() {
                let u = l.url.clone();
                card = card.child(
                    div()
                        .cursor_pointer()
                        .w_full()
                        .px(px(12.0))
                        .py(px(10.0))
                        .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _e, _w, cx| {
                                this.pending_media_open = Some((u.clone(), false));
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_link))
                                .child(l.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_secondary))
                                .truncate()
                                .child(l.url.clone()),
                        ),
                );
                if idx < last_idx {
                    card = card.child(card_hairline(pal));
                }
            }
            card.into_any_element()
        }
        SharedMediaSubTab::Voice if model.voice_notes.is_empty() => grouped_card(pal)
            .child(empty_panel(
                IconName::Mic,
                tr_str(lang, "暂无语音消息", "No voice messages yet"),
                pal,
            ))
            .into_any_element(),
        SharedMediaSubTab::Voice => {
            let mut card = grouped_card(pal).flex().flex_col();
            let last_idx = model.voice_notes.len().saturating_sub(1);
            for (idx, v) in model.voice_notes.iter().enumerate() {
                let t = v.title.clone();
                card = card.child(
                    div()
                        .cursor_pointer()
                        .w_full()
                        .px(px(12.0))
                        .py(px(10.0))
                        .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _e, _w, cx| {
                                this.pending_media_open = Some((t.clone(), true));
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .w(px(28.0))
                                .h(px(28.0))
                                .rounded_full()
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(tg_icon(IconName::Play, 12.0, pal.accent)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(1.0))
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(pal.text_primary))
                                        .truncate()
                                        .child(v.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("{}s · {}", v.duration_sec, v.date)),
                                ),
                        ),
                );
                if idx < last_idx {
                    card = card.child(card_hairline(pal));
                }
            }
            card.into_any_element()
        }
    };

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .w_full()
                .p(px(2.5))
                .rounded(px(9.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(pal.separator_rgba())
                .flex()
                .items_center()
                .gap(px(2.0))
                .children(tabs.into_iter().map(|(lbl, st)| {
                    let act = model.active_subtab == st;
                    div()
                        .cursor_pointer()
                        .flex_1()
                        .py(px(4.5))
                        .rounded(px(7.0))
                        .when(act, |el| {
                            el.bg(segmented_active_bg(pal))
                                .shadow(segmented_pill_shadow(pal))
                        })
                        .when(!act, |el| {
                            el.hover(move |s| {
                                s.bg(TdesktopThemePalette::rgba_with_alpha(
                                    if pal.mode.is_dark() { 0xFFFFFF } else { 0x000000 },
                                    0.04,
                                ))
                            })
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(11.0))
                        .font_weight(if act {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(rgb(if act {
                            pal.text_primary
                        } else {
                            pal.text_secondary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _e, _w, cx| {
                                this.shared_media.active_subtab = st;
                                cx.notify();
                            }),
                        )
                        .child(lbl.to_string())
                })),
        )
        .child(body)
        .into_any_element()
}

pub fn render_settings_and_stats_panel(
    model: &SettingsTabModel,
    boost_lvl: u32,
    boost_cur: u32,
    boost_tgt: u32,
    stats_bars: &[ChartDataPoint],
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let max_val = stats_bars
        .iter()
        .map(|b| b.value)
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .when(!model.e2ee_fingerprint.is_empty(), |el| {
            el.child(
                grouped_card(pal)
                    .p(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(tg_icon(IconName::Lock, 14.0, pal.success))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.success))
                                    .child(tr_str(lang, "加密指纹", "Encryption Key")),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(18.0))
                            .child(model.e2ee_fingerprint.clone()),
                    ),
            )
        })
        .child(
            grouped_card(pal).child(
                div()
                    .cursor_pointer()
                    .w_full()
                    .px(px(12.0))
                    .py(px(11.0))
                    .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                    .flex()
                    .items_center()
                    .justify_between()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _e, _w, cx| {
                            let next = match this.settings.auto_delete_period.as_str() {
                                "Off" => "24 Hours",
                                "24 Hours" => "7 Days",
                                "7 Days" => "30 Days",
                                _ => "Off",
                            };
                            this.set_auto_delete_period(next);
                            this.pending_action =
                                Some(InspectorAction::ChangeAutoDelete(next.to_string()));
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(pal.text_primary))
                            .child(tr_str(lang, "自动删除消息", "Auto-Delete")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .child(model.auto_delete_period.clone()),
                            )
                            .child(tg_icon(IconName::ChevronRight, 12.0, pal.text_secondary)),
                    ),
            ),
        )
        .child(
            grouped_card(pal)
                .p(px(12.0))
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
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!("Level {}", boost_lvl)),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("{}/{} boosts", boost_cur, boost_tgt)),
                        ),
                )
                .when(!stats_bars.is_empty(), |el| {
                    el.child(
                        div()
                            .h(px(64.0))
                            .pt(px(6.0))
                            .flex()
                            .items_end()
                            .gap(px(4.0))
                            .children(stats_bars.iter().take(12).map(|pt| {
                                let h = ((pt.value as f32 / max_val) * 52.0).max(4.0);
                                div()
                                    .flex_1()
                                    .h(px(h))
                                    .rounded(px(3.0))
                                    .bg(rgb(pal.accent))
                            })),
                    )
                }),
        )
        .into_any_element()
}

fn render_file_list(
    items: &[SharedFileItem],
    pal: TdesktopThemePalette,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let mut card = grouped_card(pal).flex().flex_col();
    let last_idx = items.len().saturating_sub(1);
    for (idx, f) in items.iter().enumerate() {
        let name = f.name.clone();
        card = card.child(
            div()
                .cursor_pointer()
                .w_full()
                .px(px(12.0))
                .py(px(10.0))
                .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
                .flex()
                .items_center()
                .gap(px(10.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _e, _w, cx| {
                        this.pending_media_open = Some((name.clone(), false));
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .w(px(32.0))
                        .h(px(32.0))
                        .rounded(px(8.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_shrink_0()
                        .child(tg_icon(IconName::FileDoc, 16.0, pal.accent)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(pal.text_primary))
                                .truncate()
                                .child(f.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("{} · {}", f.size_text, f.date)),
                        ),
                ),
        );
        if idx < last_idx {
            card = card.child(card_hairline(pal));
        }
    }
    card.into_any_element()
}

fn empty_panel(icon: IconName, text: &str, pal: TdesktopThemePalette) -> AnyElement {
    div()
        .py(px(28.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0))
        .child(tg_icon(icon, 26.0, pal.text_secondary))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(pal.text_secondary))
                .child(text.to_string()),
        )
        .into_any_element()
}
