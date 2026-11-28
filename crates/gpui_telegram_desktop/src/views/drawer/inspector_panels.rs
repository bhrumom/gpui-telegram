//! Inspector Drawer sub-panels: Apple Messages Contact Profile, Members,
//! Shared Media/Files/Links/Music/Voice, and Settings & Statistics.

use crate::components::avatar::render_plush_bot_avatar;
use crate::components::bloub_avatar::{
    ChannelAgentBrief, ChannelAvatarLayout, KnownChannelIcon, render_channel_avatar,
    render_channel_icon_tile,
};
use crate::i18n::tr_str;
use crate::theme::assets::{IconName, tg_icon, tg_icon_rgba};
use crate::theme::openbot_palette::parse_css_color_to_rgba;
use crate::theme::palette::TdesktopThemePalette;
use crate::views::context_inspector::{filter_add_member_candidates, AddMemberCandidate};
use crate::views::drawer::bot_computer_inspector::{
    ob_confirm_guard_card, ob_inset_group, ob_inset_row, ob_switch_pill, ob_text_input_box,
};
use crate::views::drawer::inspector_tabs::{
    ChannelBrowserAccess, InspectorAction, InspectorTab, MembersTabModel, OpenBotConfirmGuard,
    OpenBotInspectorAction, OpenBotInspectorTextField, OpenBotSettingsSubpage, ProfileTabModel,
    SettingsTabModel, SharedFileItem, SharedMediaItem, SharedMediaSubTab, SharedMediaTabModel,
};
use crate::views::drawer::statistics::info_statistics_inner_widget::ChartDataPoint;
use crate::views::openbot::channel_editor::{CHANNEL_ICON_CATALOG, CHANNEL_ICON_HUES};
use crate::views::ContextInspector;
use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::{AvatarHue, AvatarMood};
use openbot_contracts::channels::{ChannelIcon, ChannelIconName, ChannelMember};

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

pub(crate) fn grouped_card(pal: TdesktopThemePalette) -> Div {
    if pal.is_apple_style() {
        let ob = pal.openbot_palette();
        return div()
            .w_full()
            .rounded(px(8.0))
            .bg(ob.bg_surface())
            .border_1()
            .border_color(ob.border_hairline())
            .overflow_hidden();
    }
    div()
        .w_full()
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.separator_rgba())
        .overflow_hidden()
}

pub(crate) fn card_hairline(pal: TdesktopThemePalette) -> Div {
    if pal.is_apple_style() {
        return div()
            .w_full()
            .h(px(1.0))
            .bg(pal.openbot_palette().border_hairline());
    }
    div()
        .mx(px(12.0))
        .h(px(1.0))
        .bg(pal.separator_rgba())
}

/// Borderless Muse-style collapsible section header (`13px` medium/semibold title + right chevron).
pub(crate) fn muse_collapsible_header(
    id: &'static str,
    section_key: &'static str,
    title: &str,
    collapsed: bool,
    pal: TdesktopThemePalette,
    cx: &mut Context<ContextInspector>,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .w_full()
        .px(px(6.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
        .flex()
        .items_center()
        .justify_between()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.toggle_section(section_key);
                cx.notify();
            }),
        )
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_primary))
                .child(title.to_string()),
        )
        .child(tg_icon(
            if collapsed {
                IconName::ChevronRight
            } else {
                IconName::ChevronDown
            },
            12.0,
            pal.text_secondary,
        ))
}

/// Clean borderless Muse-style list item with a `30px` circular hairline icon badge on the left,
/// primary title (`13px`) + optional muted subtitle (`11.5px`), and optional trailing accessory.
#[allow(clippy::too_many_arguments)]
pub(crate) fn muse_icon_row(
    id: impl Into<ElementId>,
    icon: IconName,
    icon_color: u32,
    title: impl Into<String>,
    subtitle: Option<String>,
    title_color: Option<u32>,
    trailing: Option<AnyElement>,
    pal: TdesktopThemePalette,
    on_tap: impl Fn(&mut ContextInspector, &mut Context<ContextInspector>) + 'static,
    cx: &mut Context<ContextInspector>,
) -> Stateful<Div> {
    let fg = title_color.unwrap_or(pal.text_primary);
    let badge_bg = TdesktopThemePalette::rgba_with_alpha(
        icon_color,
        if pal.mode.is_dark() { 0.14 } else { 0.08 },
    );
    let badge_border = TdesktopThemePalette::rgba_with_alpha(
        icon_color,
        if pal.mode.is_dark() { 0.24 } else { 0.14 },
    );
    div()
        .id(id)
        .cursor_pointer()
        .w_full()
        .px(px(6.0))
        .py(px(7.0))
        .rounded(px(10.0))
        .hover(move |s| s.bg(rgb(pal.dialogs_hover_bg)))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                on_tap(this, cx);
                cx.notify();
            }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .w(px(30.0))
                        .h(px(30.0))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(badge_bg)
                        .border_1()
                        .border_color(badge_border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(tg_icon(icon, 14.0, icon_color)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(1.5))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(fg))
                                .truncate()
                                .child(title.into()),
                        )
                        .when_some(subtitle, |col, sub| {
                            col.child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(sub),
                            )
                        }),
                ),
        )
        .when_some(trailing, |row, acc| row.child(acc))
}

pub(crate) fn apple_switch_pill(enabled: bool, pal: TdesktopThemePalette) -> Div {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactProfileInfoRow {
    pub icon: IconName,
    pub icon_color: u32,
    pub value: String,
    pub subtitle: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactProfileLayoutSpec {
    pub is_group_or_channel: bool,
    pub is_channel: bool,
    pub info_section_title: String,
    pub info_rows: Vec<ContactProfileInfoRow>,
    pub privacy_section_title: String,
    pub show_secret_chat: bool,
    pub show_members_row: bool,
    pub members_row_label: Option<String>,
    pub edit_row_label: String,
    pub leave_row_label: Option<String>,
    pub show_block_contact: bool,
}

fn is_bio_fact_label(label: &str) -> bool {
    let trimmed = label.trim();
    trimmed == "简介" || trimmed.eq_ignore_ascii_case("Bio")
}

fn is_members_fact_label(label: &str) -> bool {
    let trimmed = label.trim();
    trimmed == "成员" || trimmed.eq_ignore_ascii_case("Members")
}

/// Computes the strict 3-way peer layout specification for non-Bot inspector profiles
/// (`Group / Supergroup / Channel` vs `1v1 User`).
pub fn build_contact_profile_layout(
    model: &ProfileTabModel,
    members: &MembersTabModel,
    pal: TdesktopThemePalette,
    lang: &str,
) -> ContactProfileLayoutSpec {
    let is_group_or_channel = members.is_group_or_channel;
    let is_channel = is_group_or_channel
        && model.facts.iter().any(|(k, v)| {
            (k == "类型" || k.eq_ignore_ascii_case("Type"))
                && (v == "频道" || v.eq_ignore_ascii_case("Channel"))
        });

    let mut info_rows: Vec<ContactProfileInfoRow> = Vec::new();
    if is_group_or_channel {
        if !model.username.trim().is_empty() {
            info_rows.push(ContactProfileInfoRow {
                icon: IconName::ContactsUsers,
                icon_color: pal.accent,
                value: model.username.trim().to_string(),
                subtitle: tr_str(lang, "公开链接 · 点击复制", "Public Link · Click to copy").into(),
            });
        }
        if !members.invite_link.trim().is_empty() {
            info_rows.push(ContactProfileInfoRow {
                icon: IconName::GroupUsers,
                icon_color: pal.accent,
                value: members.invite_link.trim().to_string(),
                subtitle: tr_str(lang, "邀请链接 · 点击复制", "Invite Link · Click to copy").into(),
            });
        }
        if !model.bio.trim().is_empty() {
            info_rows.push(ContactProfileInfoRow {
                icon: IconName::FolderAll,
                icon_color: pal.text_secondary,
                value: model.bio.trim().to_string(),
                subtitle: if is_channel {
                    tr_str(lang, "频道简介", "Channel Description")
                } else {
                    tr_str(lang, "群组简介", "Group Description")
                }
                .into(),
            });
        }
        let mut has_members_fact = false;
        for (label, val) in &model.facts {
            if is_bio_fact_label(label) {
                continue;
            }
            if is_members_fact_label(label) {
                has_members_fact = true;
            }
            info_rows.push(ContactProfileInfoRow {
                icon: IconName::FileDoc,
                icon_color: pal.text_secondary,
                value: val.clone(),
                subtitle: label.clone(),
            });
        }
        if !has_members_fact && !members.members.is_empty() {
            info_rows.push(ContactProfileInfoRow {
                icon: IconName::GroupUsers,
                icon_color: pal.text_secondary,
                value: members.members.len().to_string(),
                subtitle: tr_str(lang, "成员", "Members").into(),
            });
        }
    } else {
        if !model.phone.trim().is_empty() {
            info_rows.push(ContactProfileInfoRow {
                icon: IconName::PhoneCall,
                icon_color: pal.accent,
                value: model.phone.clone(),
                subtitle: tr_str(lang, "手机号 · 点击复制", "Mobile · Click to copy").into(),
            });
        }
        if !model.username.trim().is_empty() {
            info_rows.push(ContactProfileInfoRow {
                icon: IconName::ContactsUsers,
                icon_color: pal.accent,
                value: model.username.clone(),
                subtitle: tr_str(lang, "用户名 · 点击复制", "Username · Click to copy").into(),
            });
        }
        if !model.bio.trim().is_empty() {
            info_rows.push(ContactProfileInfoRow {
                icon: IconName::FolderAll,
                icon_color: pal.text_secondary,
                value: model.bio.clone(),
                subtitle: tr_str(lang, "简介", "Bio").into(),
            });
        }
        for (label, val) in &model.facts {
            if !model.bio.trim().is_empty() && is_bio_fact_label(label) {
                continue;
            }
            info_rows.push(ContactProfileInfoRow {
                icon: IconName::FileDoc,
                icon_color: pal.text_secondary,
                value: val.clone(),
                subtitle: label.clone(),
            });
        }
    }

    let info_section_title = if is_channel {
        tr_str(lang, "频道信息", "Channel Info").to_string()
    } else if is_group_or_channel {
        tr_str(lang, "群组信息", "Group Info").to_string()
    } else {
        tr_str(lang, "联系信息", "Contact Info").to_string()
    };

    let privacy_section_title = if is_group_or_channel {
        tr_str(lang, "通知与管理", "Group Settings").to_string()
    } else {
        tr_str(lang, "通知与隐私", "Notifications & Privacy").to_string()
    };

    let edit_row_label = if is_channel {
        tr_str(lang, "编辑频道信息", "Edit Channel Info").to_string()
    } else if is_group_or_channel {
        tr_str(lang, "编辑群组信息", "Edit Group Info").to_string()
    } else {
        tr_str(lang, "编辑联系人信息", "Edit Contact Info").to_string()
    };

    let leave_row_label = if is_channel {
        Some(tr_str(lang, "离开频道", "Leave Channel").to_string())
    } else if is_group_or_channel {
        Some(tr_str(lang, "退出群组", "Leave Group").to_string())
    } else {
        None
    };

    let members_row_label = is_group_or_channel.then(|| {
        if !members.members.is_empty() {
            if lang.starts_with("zh") {
                format!("查看群成员 ({})", members.members.len())
            } else {
                format!("Members ({})", members.members.len())
            }
        } else {
            tr_str(lang, "群成员列表", "View Members").to_string()
        }
    });

    ContactProfileLayoutSpec {
        is_group_or_channel,
        is_channel,
        info_section_title,
        info_rows,
        privacy_section_title,
        show_secret_chat: !is_group_or_channel,
        show_members_row: is_group_or_channel,
        members_row_label,
        edit_row_label,
        leave_row_label,
        show_block_contact: !is_group_or_channel,
    }
}

/// Renders the Muse-inspired Profile panel (Hero & segmented bar live in `ContextInspector::render`):
/// strictly branched for `Group/Channel` vs `1v1 User`, with collapsible facts rows,
/// optional 3-column shared media preview, notifications/management rows, and clean destructive row.
#[allow(clippy::too_many_arguments)]
pub fn render_contact_profile_panel(
    model: &ProfileTabModel,
    members: &MembersTabModel,
    shared_media: &SharedMediaTabModel,
    collapsed_sections: &std::collections::HashSet<&'static str>,
    is_secret_chat_active: bool,
    is_contact_blocked: bool,
    copied_feedback: Option<&String>,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let layout = build_contact_profile_layout(model, members, pal, lang);

    let info_collapsed = collapsed_sections.contains("contact-sec-info");
    let media_collapsed = collapsed_sections.contains("contact-sec-media");
    let privacy_collapsed = collapsed_sections.contains("contact-sec-privacy");

    let info_section = (!layout.info_rows.is_empty()).then(|| {
        let mut sec = div().w_full().flex().flex_col().gap(px(2.0)).child(
            muse_collapsible_header(
                "contact-sec-info-hdr",
                "contact-sec-info",
                &layout.info_section_title,
                info_collapsed,
                pal,
                cx,
            ),
        );
        if !info_collapsed {
            for (idx, row) in layout.info_rows.into_iter().enumerate() {
                let cp = row.value.clone();
                let copied_prefix = tr_str(lang, "已复制", "Copied").to_string();
                sec = sec.child(muse_icon_row(
                    SharedString::from(format!("contact-info-row-{idx}")),
                    row.icon,
                    row.icon_color,
                    row.value,
                    Some(row.subtitle),
                    None,
                    Some(tg_icon(IconName::Copy, 12.5, pal.text_secondary).into_any_element()),
                    pal,
                    move |this, cx| {
                        this.show_copied_feedback(format!("{copied_prefix}: {cp}"), cx);
                        this.pending_action = Some(InspectorAction::CopyText(cp.clone()));
                    },
                    cx,
                ));
            }
        }
        sec
    });

    let media_section = (!shared_media.media_items.is_empty()).then(|| {
        let mut sec = div().w_full().flex().flex_col().gap(px(6.0)).child(
            muse_collapsible_header(
                "contact-sec-media-hdr",
                "contact-sec-media",
                tr_str(lang, "照片与媒体", "Photos & Media"),
                media_collapsed,
                pal,
                cx,
            ),
        );
        if !media_collapsed {
            sec = sec
                .child(
                    div()
                        .px(px(6.0))
                        .flex()
                        .flex_wrap()
                        .gap(px(6.0))
                        .children(
                            shared_media
                                .media_items
                                .iter()
                                .take(6)
                                .map(|item| render_media_thumb_tile(item, 88.0, pal, cx)),
                        ),
                )
                .child(
                    div()
                        .id("inspector-media-see-all")
                        .cursor_pointer()
                        .py(px(4.0))
                        .flex()
                        .justify_center()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.text_secondary))
                        .hover(|s| s.text_color(rgb(pal.text_primary)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _e, _w, cx| {
                                this.set_tab(InspectorTab::SharedMedia);
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "查看所有照片与媒体", "See all shared media")),
                );
        }
        sec
    });

    let mut privacy_sec = div().w_full().flex().flex_col().gap(px(2.0)).child(
        muse_collapsible_header(
            "contact-sec-privacy-hdr",
            "contact-sec-privacy",
            &layout.privacy_section_title,
            privacy_collapsed,
            pal,
            cx,
        ),
    );
    if !privacy_collapsed {
        let mute_on = model.is_notifications_enabled;
        privacy_sec = privacy_sec.child(muse_icon_row(
            "inspector-quick-mute",
            if mute_on { IconName::Speaker } else { IconName::Mute },
            if mute_on { pal.accent } else { pal.text_secondary },
            tr_str(lang, "消息通知", "Notifications"),
            Some(
                if mute_on {
                    tr_str(lang, "已开启声音与横幅提醒", "Alerts enabled")
                } else {
                    tr_str(lang, "已静音通知", "Muted")
                }
                .to_string(),
            ),
            None,
            Some(apple_switch_pill(mute_on, pal).into_any_element()),
            pal,
            |this, _| {
                this.profile.is_notifications_enabled = !this.profile.is_notifications_enabled;
                this.pending_action = Some(InspectorAction::ToggleNotifications(
                    this.profile.is_notifications_enabled,
                ));
            },
            cx,
        ));
        if layout.show_members_row {
            if let Some(ref members_label) = layout.members_row_label {
                privacy_sec = privacy_sec.child(muse_icon_row(
                    "inspector-group-members-row",
                    IconName::GroupUsers,
                    pal.accent,
                    members_label.clone(),
                    None,
                    None,
                    Some(tg_icon(IconName::ChevronRight, 12.0, pal.text_secondary).into_any_element()),
                    pal,
                    |this, _| {
                        this.set_tab(InspectorTab::Members);
                    },
                    cx,
                ));
            }
        }
        if layout.show_secret_chat {
            privacy_sec = privacy_sec.child(muse_icon_row(
                "inspector-secret-chat-toggle",
                IconName::Lock,
                if is_secret_chat_active { pal.success } else { pal.accent },
                if is_secret_chat_active {
                    tr_str(lang, "端到端加密已开启", "Secret Chat Active")
                } else {
                    tr_str(lang, "开启加密聊天", "Start Secret Chat")
                },
                None,
                None,
                Some(apple_switch_pill(is_secret_chat_active, pal).into_any_element()),
                pal,
                |this, _| {
                    this.is_secret_chat_active = !this.is_secret_chat_active;
                },
                cx,
            ));
        }
        privacy_sec = privacy_sec.child(muse_icon_row(
            "inspector-edit-peer-row",
            IconName::Edit,
            pal.icon_fg,
            layout.edit_row_label,
            None,
            None,
            Some(tg_icon(IconName::ChevronRight, 12.0, pal.text_secondary).into_any_element()),
            pal,
            |this, _| {
                this.pending_action = Some(InspectorAction::OpenEditPeer);
            },
            cx,
        ));
        if let Some(leave_label) = layout.leave_row_label {
            privacy_sec = privacy_sec.child(muse_icon_row(
                "inspector-leave-group-row",
                IconName::DeleteTrash,
                pal.danger,
                leave_label,
                None,
                Some(pal.danger),
                None,
                pal,
                |this, _| {
                    this.pending_leave_chat = true;
                },
                cx,
            ));
        } else if layout.show_block_contact {
            privacy_sec = privacy_sec.child(muse_icon_row(
                "inspector-block-contact-row",
                IconName::Close,
                pal.danger,
                if is_contact_blocked {
                    tr_str(lang, "取消屏蔽联系人", "Unblock Contact")
                } else {
                    tr_str(lang, "屏蔽此联系人", "Block Contact")
                },
                None,
                Some(pal.danger),
                None,
                pal,
                |this, _| {
                    this.is_contact_blocked = !this.is_contact_blocked;
                },
                cx,
            ));
        }
    }

    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .when_some(copied_feedback.cloned(), |col, hint| {
            col.child(
                div()
                    .w_full()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(9.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.12))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(pal.success))
                    .child(hint),
            )
        })
        .when_some(info_section, |col, sec| col.child(sec))
        .when_some(media_section, |col, sec| col.child(sec))
        .child(privacy_sec)
        .into_any_element()
}

pub fn render_members_panel(
    model: &MembersTabModel,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    render_members_panel_inner(model, false, false, "", None, &[], pal, lang, cx)
}

/// Renders the Group Members inspector tab along with Grok Bot's inline searchable
/// `"+ 添加成员"` picker (contacts + Fabushi AI Bots) when open.
pub fn render_members_panel_with_picker(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let preedit = inspector
        .add_member_search_input
        .as_ref()
        .and_then(|ctrl| ctrl.read(cx).ime_preedit.clone());
    render_members_panel_inner(
        &inspector.members,
        inspector.add_member_picker_open,
        inspector.add_member_search_focused,
        &inspector.add_member_query,
        preedit.as_deref(),
        &inspector.add_member_candidates,
        pal,
        lang,
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_members_panel_inner(
    model: &MembersTabModel,
    picker_open: bool,
    search_focused: bool,
    search_query: &str,
    search_preedit: Option<&str>,
    candidates: &[AddMemberCandidate],
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let is_dark = pal.mode.is_dark();
    let invite = model.invite_link.clone();
    let picker_panel: Option<AnyElement> = picker_open.then(|| {
        let filtered = filter_add_member_candidates(candidates, search_query);
        let row_hover = pal.row_hover_rgba();
        let combined_empty = search_query.is_empty() && search_preedit.map_or(true, |p| p.is_empty());
        let search_display = if combined_empty {
            if search_focused {
                format!("| {}", tr_str(lang, "搜索联系人或 AI Bot…", "Search contacts or AI Bots…"))
            } else {
                tr_str(lang, "搜索联系人或 AI Bot…", "Search contacts or AI Bots…").to_string()
            }
        } else if let Some(pre) = search_preedit.filter(|p| !p.is_empty()) {
            format!("{search_query}{pre}|")
        } else if search_focused {
            format!("{search_query}|")
        } else {
            search_query.to_string()
        };

        let (_, bot_badge_bg, bot_badge_fg) =
            TdesktopThemePalette::bot_hue_colors("purple", is_dark);

        let mut list = div()
            .id("inspector-add-member-list")
            .w_full()
            .max_h(px(240.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.0));

        if filtered.is_empty() {
            list = list.child(
                div()
                    .w_full()
                    .py(px(14.0))
                    .px(px(10.0))
                    .text_center()
                    .text_size(px(12.0))
                    .text_color(rgb(pal.text_secondary))
                    .child(if candidates.is_empty() {
                        tr_str(
                            lang,
                            "所有联系人与 AI Bot 均已在群内",
                            "All contacts and AI Bots are already in this group",
                        )
                    } else {
                        tr_str(lang, "未找到匹配的联系人或 Bot", "No matching contacts or bots")
                    }),
            );
        } else {
            for cand in filtered {
                let uid = cand.user_id;
                let handle = cand
                    .username
                    .as_deref()
                    .map(|u| format!("@{u}"))
                    .unwrap_or_default();
                let avatar_bg = cand.avatar_color;
                let avatar_el = if cand.is_bot {
                    div()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded_full()
                        .flex_shrink_0()
                        .bg(rgb(avatar_bg))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(tg_icon(IconName::BotRobot, 14.0, 0xFFFFFF))
                } else {
                    div()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded_full()
                        .flex_shrink_0()
                        .bg(rgb(avatar_bg))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .child(cand.initials.clone())
                };

                list = list.child(
                    div()
                        .id(("inspector-add-cand-row", uid))
                        .w_full()
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(8.0))
                        .hover(move |s| s.bg(row_hover))
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
                                .gap(px(8.0))
                                .child(avatar_el)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(1.0))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(5.0))
                                                .min_w(px(0.0))
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_size(px(12.5))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(rgb(pal.text_primary))
                                                        .child(cand.display_name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .flex_shrink_0()
                                                        .px(px(5.0))
                                                        .py(px(1.0))
                                                        .rounded_full()
                                                        .bg(if cand.is_bot {
                                                            rgb(bot_badge_bg).into()
                                                        } else {
                                                            TdesktopThemePalette::rgba_with_alpha(
                                                                pal.text_secondary,
                                                                0.12,
                                                            )
                                                        })
                                                        .text_size(px(9.5))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(if cand.is_bot {
                                                            rgb(bot_badge_fg)
                                                        } else {
                                                            rgb(pal.text_secondary)
                                                        })
                                                        .child(if cand.is_bot {
                                                            "AI Bot"
                                                        } else {
                                                            tr_str(lang, "联系人", "Human")
                                                        }),
                                                ),
                                        )
                                        .when(!handle.is_empty(), |col| {
                                            col.child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(pal.text_secondary))
                                                    .child(handle),
                                            )
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .id(("inspector-add-member-btn", uid))
                                .cursor_pointer()
                                .flex_shrink_0()
                                .px(px(8.0))
                                .py(px(3.5))
                                .rounded_full()
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                .hover(move |s| {
                                    s.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.24))
                                })
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.request_add_member_to_active_group(uid);
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "+ 添加", "+ Add")),
                        ),
                );
            }
        }

        grouped_card(pal)
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(tr_str(lang, "添加成员 / AI Bot", "Add Member / AI Bot")),
                    )
                    .child(
                        div()
                            .id("inspector-add-member-picker-close")
                            .cursor_pointer()
                            .w(px(22.0))
                            .h(px(22.0))
                            .rounded_full()
                            .hover(move |s| s.bg(row_hover))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_add_member_picker();
                                    cx.notify();
                                }),
                            )
                            .child(tg_icon(IconName::Close, 11.5, pal.icon_fg)),
                    ),
            )
            .child(
                div()
                    .id("inspector-add-member-search")
                    .cursor_text()
                    .w_full()
                    .h(px(32.0))
                    .px(px(9.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.window_bg))
                    .border_1()
                    .border_color(if search_focused {
                        TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.72)
                    } else {
                        pal.hairline_border_rgba()
                    })
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.add_member_search_focused = true;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(tg_icon(IconName::Search, 12.5, pal.text_secondary))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_size(px(12.0))
                                    .text_color(if combined_empty {
                                        rgb(pal.text_secondary)
                                    } else {
                                        rgb(pal.text_primary)
                                    })
                                    .child(search_display),
                            ),
                    )
                    .when(!search_query.is_empty(), |row| {
                        row.child(
                            div()
                                .id("inspector-add-member-search-clear")
                                .cursor_pointer()
                                .w(px(18.0))
                                .h(px(18.0))
                                .rounded_full()
                                .hover(move |s| s.bg(row_hover))
                                .flex()
                                .items_center()
                                .justify_center()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.add_member_query.clear();
                                        cx.notify();
                                    }),
                                )
                                .child(tg_icon(IconName::Close, 10.0, pal.icon_fg)),
                        )
                    }),
            )
            .child(list)
            .into_any_element()
    });

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
                        .id("inspector-toggle-add-member-picker")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(999.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(
                            pal.accent,
                            if picker_open { 0.22 } else { 0.12 },
                        ))
                        .hover(move |s| {
                            s.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.22))
                        })
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _e, _w, cx| {
                                if this.add_member_picker_open {
                                    this.close_add_member_picker();
                                } else {
                                    this.pending_action = Some(InspectorAction::OpenAddMember);
                                }
                                cx.notify();
                            }),
                        )
                        .child(if picker_open {
                            tr_str(lang, "收起添加", "Done").to_string()
                        } else {
                            tr_str(lang, "+ 添加成员", "+ Add Member").to_string()
                        }),
                ),
        )
        .when_some(picker_panel, |col, panel| col.child(panel))
        .when(!invite.is_empty(), |el| {
            let cp = invite.clone();
            let copied_prefix = tr_str(lang, "已复制", "Copied").to_string();
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
                                    this.show_copied_feedback(format!("{copied_prefix}: {cp}"), cx);
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

// ---------------------------------------------------------------------------
// M4 UI-F17: OpenBot `ChannelSettingsPane` (`打开 群聊设置`) & 1:1 `打开 联系人资料`
// ---------------------------------------------------------------------------

fn render_channel_picture_editor_popover(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let is_dark = pal.mode.is_dark();
    let active_hue = AvatarHue::from_degrees(inspector.channel_icon_hue).unwrap_or(AvatarHue::Blue);
    let chosen_name = inspector
        .channel_icon_name
        .as_deref()
        .and_then(ChannelIconName::parse);
    let has_img = inspector.channel_custom_image.is_some();

    // 5-column × 4-row grid of all 20 CHANNEL_ICON_CATALOG icons
    let mut icon_grid = div()
        .w_full()
        .px(px(10.0))
        .pb(px(10.0))
        .flex()
        .flex_col()
        .gap(px(4.0));
    for (row_idx, chunk) in CHANNEL_ICON_CATALOG.chunks(5).enumerate() {
        let mut row = div().w_full().flex().items_center().gap(px(4.0));
        for (col_idx, &icon_name_str) in chunk.iter().enumerate() {
            let idx = row_idx * 5 + col_idx;
            let parsed = ChannelIconName::parse(icon_name_str).unwrap_or(ChannelIconName::Star);
            let selected = chosen_name == Some(parsed);
            let icon_owned = icon_name_str.to_string();
            let hue_deg = active_hue.degrees();
            row = row.child(
                div()
                    .id(SharedString::from(format!("channel-pic-icon-{idx}")))
                    .flex_1()
                    .h(px(42.0))
                    .cursor_pointer()
                    .rounded(px(8.0))
                    .bg(if selected {
                        ob.bg_control_active()
                    } else {
                        ob.bg_control()
                    })
                    .border_1()
                    .border_color(if selected {
                        ob.text_muted()
                    } else {
                        ob.border_hairline()
                    })
                    .hover(|s| s.bg(ob.bg_control_hover()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.channel_custom_image = None;
                            this.channel_icon_name = Some(icon_owned.clone());
                            this.pending_openbot_action =
                                Some(OpenBotInspectorAction::SetChannelIcon {
                                    icon_name: Some(icon_owned.clone()),
                                    hue: Some(hue_deg),
                                });
                            cx.notify();
                        }),
                    )
                    .child(render_channel_icon_tile(
                        KnownChannelIcon {
                            name: parsed,
                            hue: active_hue,
                        },
                        30.0,
                        8.0,
                        is_dark,
                    )),
            );
        }
        icon_grid = icon_grid.child(row);
    }

    // 8-column color swatch grid across CHANNEL_ICON_HUES (10 hues)
    let mut hue_grid = div()
        .w_full()
        .px(px(10.0))
        .pb(px(10.0))
        .flex()
        .flex_col()
        .gap(px(4.0));
    for (row_idx, chunk) in CHANNEL_ICON_HUES.chunks(8).enumerate() {
        let mut row = div().w_full().flex().items_center().gap(px(4.0));
        for (col_idx, &hue) in chunk.iter().enumerate() {
            let idx = row_idx * 8 + col_idx;
            let selected = active_hue == hue;
            let deg = hue.degrees();
            let sw = parse_css_color_to_rgba(hue.swatch()).unwrap_or_else(|| rgb(0x4699FE));
            row = row.child(
                div()
                    .id(SharedString::from(format!("channel-pic-hue-{idx}")))
                    .flex_1()
                    .h(px(28.0))
                    .cursor_pointer()
                    .rounded(px(6.0))
                    .bg(if selected {
                        ob.bg_control_active()
                    } else {
                        Rgba::default()
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.channel_icon_hue = deg;
                            if this.channel_icon_name.is_none() {
                                this.channel_icon_name = Some("star".to_string());
                            }
                            this.pending_openbot_action =
                                Some(OpenBotInspectorAction::SetChannelIcon {
                                    icon_name: this.channel_icon_name.clone(),
                                    hue: Some(deg),
                                });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .w(px(18.0))
                            .h(px(18.0))
                            .rounded_full()
                            .bg(sw)
                            .border_1()
                            .border_color(if selected {
                                ob.text_primary()
                            } else {
                                ob.border_strong_hairline()
                            }),
                    ),
            );
        }
        for _ in chunk.len()..8 {
            row = row.child(div().flex_1().h(px(28.0)));
        }
        hue_grid = hue_grid.child(row);
    }

    div()
        .id("channel-picture-editor-popover")
        .mt(px(10.0))
        .w_full()
        .max_w(px(284.0))
        .rounded(px(12.0))
        .bg(ob.bg_surface())
        .border_1()
        .border_color(ob.border_strong_hairline())
        .shadow_md()
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(36.0))
                .px(px(10.0))
                .border_b_1()
                .border_color(ob.border_hairline())
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(tr_str(lang, "频道头像与图标", "Channel Picture")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("channel-pic-reset-cluster")
                                .cursor_pointer()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(5.0))
                                .hover(|s| s.bg(ob.hover()))
                                .text_size(px(11.0))
                                .text_color(ob.text_muted())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.channel_icon_name = None;
                                        this.channel_custom_image = None;
                                        this.pending_openbot_action =
                                            Some(OpenBotInspectorAction::SetChannelIcon {
                                                icon_name: None,
                                                hue: None,
                                            });
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "恢复拼贴", "Reset")),
                        )
                        .child(
                            div()
                                .id("channel-pic-done")
                                .cursor_pointer()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(5.0))
                                .hover(|s| s.bg(ob.hover()))
                                .text_size(px(11.5))
                                .text_color(ob.text_secondary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.channel_picture_editor_open = false;
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "完成", "Done")),
                        ),
                ),
        )
        .child(
            div()
                .px(px(10.0))
                .py(px(8.0))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .id("channel-pic-upload-btn")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(7.0))
                        .bg(ob.bg_control())
                        .border_1()
                        .border_color(ob.border_strong_hairline())
                        .hover(|s| s.bg(ob.bg_control_hover()))
                        .text_size(px(12.0))
                        .text_color(ob.text_primary())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.pending_action = Some(InspectorAction::OpenEditPeer);
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "上传群聊图片", "Upload Photo")),
                )
                .when(has_img, |r| {
                    r.child(
                        div()
                            .id("channel-pic-remove-img")
                            .cursor_pointer()
                            .text_size(px(11.5))
                            .text_color(ob.danger())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.channel_custom_image = None;
                                    this.profile.avatar_path = None;
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "移除图片", "Remove")),
                    )
                }),
        )
        .child(div().w_full().h(px(1.0)).bg(ob.border_hairline()))
        .child(
            div()
                .px(px(10.0))
                .pt(px(8.0))
                .pb(px(4.0))
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_secondary())
                .child(tr_str(lang, "图标 (20)", "ICONS (20)")),
        )
        .child(icon_grid)
        .child(div().w_full().h(px(1.0)).bg(ob.border_hairline()))
        .child(
            div()
                .px(px(10.0))
                .pt(px(8.0))
                .pb(px(4.0))
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_secondary())
                .child(tr_str(lang, "色调", "COLOR")),
        )
        .child(hue_grid)
        .into_any_element()
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenBotChannelSettingsLayout {
    pub mounted: bool,
    pub avatar_px: f32,
    pub picture_popover_width_px: f32,
    pub icon_catalog_count: usize,
    pub icon_hues_count: usize,
    pub name: String,
    pub read_only: bool,
    pub members_count: usize,
    pub browser_policy: String,
    pub context_isolated: bool,
    pub subpages_present: [&'static str; 3],
    pub memories_count: usize,
    pub routines_count: usize,
    pub folders_count: usize,
    pub context_actions_visible: bool,
    pub compact_disabled: bool,
    pub context_actions: [&'static str; 3],
}

pub fn compute_openbot_channel_settings_layout(
    inspector: &ContextInspector,
) -> OpenBotChannelSettingsLayout {
    let name = if !inspector.channel_draft_name.trim().is_empty() {
        inspector.channel_draft_name.clone()
    } else {
        inspector.profile.name.clone()
    };
    OpenBotChannelSettingsLayout {
        mounted: inspector.is_open
            && inspector.embedded_in_side_panel
            && inspector.members.is_group_or_channel,
        avatar_px: 62.0,
        picture_popover_width_px: crate::components::bloub_avatar::EDITOR_MAX_WIDTH_PX,
        icon_catalog_count: CHANNEL_ICON_CATALOG.len(),
        icon_hues_count: CHANNEL_ICON_HUES.len(),
        name,
        read_only: inspector.channel_archived,
        members_count: inspector.members.members.len(),
        browser_policy: inspector.channel_browser_access.as_policy_str(),
        context_isolated: inspector.channel_context_isolated,
        subpages_present: ["memories", "routines", "folders"],
        memories_count: inspector.channel_memories.len(),
        routines_count: inspector.bot_fusion.routines.len(),
        folders_count: inspector.channel_folders.len(),
        context_actions_visible: !inspector.channel_archived,
        compact_disabled: inspector.channel_archived || !inspector.channel_provider_supports_compact,
        context_actions: ["compact", "new_conversation", "delete_history"],
    }
}

/// Renders OpenBot's `ChannelSettingsPane` (`UI-F17`) inside `打开 群聊设置`.
pub fn render_openbot_channel_settings_root(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let ch_layout = compute_openbot_channel_settings_layout(inspector);
    let ob = pal.openbot_palette();
    let is_dark = pal.mode.is_dark();
    let read_only = ch_layout.read_only;
    let preedit = inspector.openbot_input_preedit(cx);
    let copied_prefix = tr_str(lang, "已复制", "Copied").to_string();

    // Build ChannelAvatar cluster inputs from live group members
    let member_entries: Vec<ChannelMember> = inspector
        .members
        .members
        .iter()
        .take(4)
        .map(|m| ChannelMember {
            agent_id: format!("member-{}", m.id),
        })
        .collect();
    let briefs: Vec<ChannelAgentBrief> = inspector
        .members
        .members
        .iter()
        .take(4)
        .map(|m| ChannelAgentBrief {
            id: format!("member-{}", m.id),
            avatar_seed: Some(m.name.clone()),
            avatar_hue: Some(AvatarHue::ALL[(m.id as usize) % AvatarHue::ALL.len()]),
            avatar_color: None,
            avatar_url: None,
        })
        .collect();
    let channel_icon_opt = inspector.channel_icon_name.as_ref().map(|name| ChannelIcon {
        name: name.clone(),
        hue: inspector.channel_icon_hue,
    });
    let channel_avatar_el: AnyElement = if let Some(ref custom) = inspector
        .channel_custom_image
        .as_ref()
        .or(inspector.profile.avatar_path.as_ref())
    {
        div()
            .w(px(ch_layout.avatar_px))
            .h(px(ch_layout.avatar_px))
            .rounded(px(20.0))
            .overflow_hidden()
            .border_1()
            .border_color(ob.border_hairline())
            .child(
                img((*custom).clone())
                    .w(px(62.0))
                    .h(px(62.0))
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element()
    } else {
        render_channel_avatar(
            &member_entries,
            &briefs,
            ChannelAvatarLayout::Cluster,
            channel_icon_opt.as_ref(),
            None,
            false,
            62.0,
            None,
            is_dark,
            0.0,
        )
        .into_any_element()
    };

    let hero = div()
        .w_full()
        .pt(px(4.0))
        .pb(px(4.0))
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .relative()
                .w(px(62.0))
                .h(px(62.0))
                .child(channel_avatar_el)
                .when(!read_only, |wrap| {
                    wrap.child(
                        div()
                            .id("channel-picture-edit-badge")
                            .cursor_pointer()
                            .absolute()
                            .right(px(-4.0))
                            .bottom(px(-4.0))
                            .w(px(24.0))
                            .h(px(24.0))
                            .rounded_full()
                            .bg(ob.bg_control())
                            .border_2()
                            .border_color(ob.bg_canvas())
                            .hover(|s| s.bg(ob.bg_control_hover()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.channel_picture_editor_open =
                                        !this.channel_picture_editor_open;
                                    cx.notify();
                                }),
                            )
                            .child(tg_icon_rgba(IconName::Edit, 11.0, ob.text_primary())),
                    )
                }),
        )
        .when(inspector.channel_picture_editor_open && !read_only, |col| {
            col.child(render_channel_picture_editor_popover(inspector, pal, lang, cx))
        });

    let ch_name = if inspector.channel_draft_name.trim().is_empty() {
        inspector.profile.name.clone()
    } else {
        inspector.channel_draft_name.clone()
    };
    let ch_instr = if inspector.channel_draft_instructions.is_empty() {
        inspector.profile.bio.clone()
    } else {
        inspector.channel_draft_instructions.clone()
    };

    let fields = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(ob_text_input_box(
            "openbot-ch-name-input",
            tr_str(lang, "名称", "Name"),
            &ch_name,
            preedit.as_deref(),
            tr_str(lang, "群聊 / 频道名称", "Channel name"),
            inspector.openbot_text_focus == Some(OpenBotInspectorTextField::ChannelName),
            38.0,
            read_only,
            OpenBotInspectorTextField::ChannelName,
            pal,
            cx,
        ))
        .child(ob_text_input_box(
            "openbot-ch-title-input",
            tr_str(lang, "称号", "Title"),
            &inspector.channel_draft_title,
            preedit.as_deref(),
            tr_str(lang, "简短副标题（可选）", "Subtitle (optional)"),
            inspector.openbot_text_focus == Some(OpenBotInspectorTextField::ChannelTitle),
            38.0,
            read_only,
            OpenBotInspectorTextField::ChannelTitle,
            pal,
            cx,
        ))
        .child(ob_text_input_box(
            "openbot-ch-instr-input",
            tr_str(lang, "指令", "Instructions"),
            &ch_instr,
            preedit.as_deref(),
            tr_str(lang, "为群内所有成员和助手设定共享规则…", "Shared instructions for members…"),
            inspector.openbot_text_focus == Some(OpenBotInspectorTextField::ChannelInstructions),
            120.0,
            read_only,
            OpenBotInspectorTextField::ChannelInstructions,
            pal,
            cx,
        ))
        .when(!read_only, |col| {
            col.child(
                div()
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .id("openbot-ch-save-info-btn")
                            .cursor_pointer()
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(7.0))
                            .bg(ob.bg_control_hover())
                            .border_1()
                            .border_color(ob.border_strong_hairline())
                            .hover(|s| s.bg(ob.bg_control_active()))
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_primary())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.commit_channel_identity_draft();
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "保存群聊设置", "Save Channel Info")),
                    ),
            )
        })
        .when_some(inspector.channel_validation_error.clone(), |col, err| {
            col.child(
                div()
                    .id("openbot-ch-validation-error")
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .bg(ob.bg_control())
                    .border_1()
                    .border_color(ob.danger())
                    .text_size(px(11.5))
                    .text_color(ob.danger())
                    .child(err),
            )
        });

    // Browser Access Selector (`所有成员` / specific member / `无人`)
    let mut browser_opts = div().w_full().p(px(8.0)).flex().flex_wrap().gap(px(5.0));
    let is_everyone = inspector.channel_browser_access == ChannelBrowserAccess::Everyone;
    let is_none = inspector.channel_browser_access == ChannelBrowserAccess::None;
    browser_opts = browser_opts.child(
        div()
            .id("openbot-ch-browser-everyone")
            .cursor_pointer()
            .px(px(9.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .bg(if is_everyone {
                ob.bg_control_active()
            } else {
                ob.bg_canvas()
            })
            .border_1()
            .border_color(if is_everyone {
                ob.text_muted()
            } else {
                ob.border_hairline()
            })
            .text_size(px(12.0))
            .text_color(if is_everyone {
                ob.text_primary()
            } else {
                ob.text_muted()
            })
            .when(!read_only, |el| {
                el.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.channel_browser_access = ChannelBrowserAccess::Everyone;
                        this.pending_openbot_action =
                            Some(OpenBotInspectorAction::SetChannelBrowserPolicy(
                                "everyone".to_string(),
                            ));
                        cx.notify();
                    }),
                )
            })
            .child(tr_str(lang, "所有成员", "Everyone")),
    );
    for m in inspector.members.members.iter().take(4) {
        let uid = m.id;
        let sel = inspector.channel_browser_access == ChannelBrowserAccess::Member(uid);
        browser_opts = browser_opts.child(
            div()
                .id(("openbot-ch-browser-member", uid))
                .cursor_pointer()
                .px(px(9.0))
                .py(px(5.0))
                .rounded(px(6.0))
                .bg(if sel { ob.bg_control_active() } else { ob.bg_canvas() })
                .border_1()
                .border_color(if sel { ob.text_muted() } else { ob.border_hairline() })
                .text_size(px(12.0))
                .text_color(if sel { ob.text_primary() } else { ob.text_muted() })
                .when(!read_only, |el| {
                    el.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.channel_browser_access = ChannelBrowserAccess::Member(uid);
                            this.pending_openbot_action =
                                Some(OpenBotInspectorAction::SetChannelBrowserPolicy(format!(
                                    "member:{uid}"
                                )));
                            cx.notify();
                        }),
                    )
                })
                .child(m.name.clone()),
        );
    }
    browser_opts = browser_opts.child(
        div()
            .id("openbot-ch-browser-none")
            .cursor_pointer()
            .px(px(9.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .bg(if is_none {
                ob.bg_control_active()
            } else {
                ob.bg_canvas()
            })
            .border_1()
            .border_color(if is_none {
                ob.text_muted()
            } else {
                ob.border_hairline()
            })
            .text_size(px(12.0))
            .text_color(if is_none {
                ob.text_primary()
            } else {
                ob.text_muted()
            })
            .when(!read_only, |el| {
                el.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.channel_browser_access = ChannelBrowserAccess::None;
                        this.pending_openbot_action =
                            Some(OpenBotInspectorAction::SetChannelBrowserPolicy(
                                "none".to_string(),
                            ));
                        cx.notify();
                    }),
                )
            })
            .child(tr_str(lang, "无人", "None")),
    );

    let isolated = inspector.channel_context_isolated;
    let browser_and_isolation = ob_inset_group(
        tr_str(lang, "浏览器与上下文", "Browser & Context"),
        &ob,
        vec![
            browser_opts.into_any_element(),
            div()
                .id("openbot-ch-isolation-toggle")
                .w_full()
                .px(px(12.0))
                .py(px(10.0))
                .when(!read_only, |el| {
                    el.cursor_pointer()
                        .hover(|s| s.bg(ob.hover()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.channel_context_isolated = !this.channel_context_isolated;
                                this.pending_openbot_action =
                                    Some(OpenBotInspectorAction::SetChannelContextIsolation(
                                        this.channel_context_isolated,
                                    ));
                                cx.notify();
                            }),
                        )
                })
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_primary())
                                .child(tr_str(
                                    lang,
                                    "将记忆保留在此频道",
                                    "Keep memories in this channel",
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(ob.text_muted())
                                .child(tr_str(
                                    lang,
                                    "智能体在此学到的内容仅保留于当前群聊，不串入其他对话。",
                                    "Memories learned here stay isolated to this channel.",
                                )),
                        ),
                )
                .child(ob_switch_pill(isolated, &ob))
                .into_any_element(),
        ],
    );

    // Inset Link Group: 记忆, 自动化任务, 文件夹, 共享媒体与文件, 邀请链接, 通知
    let notif_on = inspector.profile.is_notifications_enabled;
    let media_total = inspector.shared_media.media_items.len()
        + inspector.shared_media.files.len()
        + inspector.shared_media.links.len();
    let mut link_rows: Vec<AnyElement> = vec![
        ob_inset_row(
            "openbot-ch-memories-row",
            IconName::FolderAll,
            tr_str(lang, "记忆", "Memories"),
            Some(if lang.starts_with("zh") {
                format!("已保存 {} 条", inspector.channel_memories.len())
            } else {
                format!("{} saved", inspector.channel_memories.len())
            }),
            true,
            false,
            &ob,
            |this, _| {
                this.openbot_subpage = Some(OpenBotSettingsSubpage::ChannelMemories);
            },
            cx,
        ),
        ob_inset_row(
            "openbot-ch-routines-row",
            IconName::Play,
            tr_str(lang, "自动化任务", "Routines"),
            Some(format!("{}", inspector.bot_fusion.routines.len())),
            true,
            false,
            &ob,
            |this, _| {
                this.openbot_subpage = Some(OpenBotSettingsSubpage::ChannelRoutines);
            },
            cx,
        ),
        ob_inset_row(
            "openbot-ch-folders-row",
            IconName::FileDoc,
            tr_str(lang, "文件夹", "Folders"),
            Some(format!("{}", inspector.channel_folders.len())),
            true,
            false,
            &ob,
            |this, _| {
                this.openbot_subpage = Some(OpenBotSettingsSubpage::ChannelFolders);
            },
            cx,
        ),
        ob_inset_row(
            "openbot-ch-media-row",
            IconName::Camera,
            tr_str(lang, "共享媒体与文件", "Shared Media & Files"),
            Some(format!("{media_total}")),
            true,
            false,
            &ob,
            |this, _| {
                this.openbot_subpage = Some(OpenBotSettingsSubpage::SharedMedia);
            },
            cx,
        ),
        div()
            .id("inspector-quick-mute")
            .w_full()
            .min_h(px(40.0))
            .px(px(12.0))
            .cursor_pointer()
            .hover(|s| s.bg(ob.hover()))
            .flex()
            .items_center()
            .justify_between()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.profile.is_notifications_enabled = !this.profile.is_notifications_enabled;
                    this.pending_action = Some(InspectorAction::ToggleNotifications(
                        this.profile.is_notifications_enabled,
                    ));
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .child(tg_icon_rgba(
                        if notif_on { IconName::Speaker } else { IconName::Mute },
                        14.0,
                        ob.text_muted(),
                    ))
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_primary())
                            .child(tr_str(lang, "消息通知", "Notifications")),
                    ),
            )
            .child(ob_switch_pill(notif_on, &ob))
            .into_any_element(),
    ];

    if !inspector.members.invite_link.trim().is_empty() {
        let inv = inspector.members.invite_link.trim().to_string();
        let inv_cp = inv.clone();
        let prefix_cp = copied_prefix.clone();
        link_rows.push(
            div()
                .w_full()
                .min_h(px(40.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .id("openbot-ch-copy-invite")
                        .cursor_pointer()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.show_copied_feedback(format!("{prefix_cp}: {inv_cp}"), cx);
                                this.pending_action = Some(InspectorAction::CopyText(inv_cp.clone()));
                                cx.notify();
                            }),
                        )
                        .child(tg_icon_rgba(IconName::Copy, 13.5, ob.text_muted()))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.5))
                                .text_color(ob.text_secondary())
                                .child(inv),
                        ),
                )
                .when(!read_only, |r| {
                    r.child(
                        div()
                            .id("openbot-ch-revoke-invite")
                            .cursor_pointer()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .hover(|s| s.bg(ob.hover()))
                            .text_size(px(11.5))
                            .text_color(ob.danger())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.pending_action = Some(InspectorAction::RevokeInviteLink);
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "重置", "Revoke")),
                    )
                })
                .into_any_element(),
        );
    }
    let links_group = ob_inset_group(tr_str(lang, "频道资源", "Channel Resources"), &ob, link_rows);

    // ChannelContextActions (hidden when archived)
    let compact_disabled = ch_layout.compact_disabled;
    let context_actions = (!read_only).then(|| {
        ob_inset_group(
            tr_str(lang, "上下文操作", "Conversation Context"),
            &ob,
            vec![
                div()
                    .w_full()
                    .when(compact_disabled, |el| el.opacity(0.45))
                    .child(ob_inset_row(
                        "openbot-ch-compact-ctx",
                        IconName::FolderAll,
                        tr_str(lang, "压缩对话", "Compact context"),
                        Some(if compact_disabled {
                            tr_str(lang, "不可用", "Unavailable").to_string()
                        } else {
                            tr_str(lang, "压缩", "Compact").to_string()
                        }),
                        false,
                        false,
                        &ob,
                        |this, _| {
                            if !this.channel_archived && this.channel_provider_supports_compact {
                                this.pending_confirm_guard =
                                    Some(OpenBotConfirmGuard::ChannelCompactContext);
                            }
                        },
                        cx,
                    ))
                    .into_any_element(),
                ob_inset_row(
                    "openbot-ch-new-conv",
                    IconName::ComposePen,
                    tr_str(lang, "开启新对话", "New conversation"),
                    Some(tr_str(lang, "开始", "Start").to_string()),
                    false,
                    false,
                    &ob,
                    |this, _| {
                        this.pending_confirm_guard =
                            Some(OpenBotConfirmGuard::ChannelNewConversation);
                    },
                    cx,
                ),
                ob_inset_row(
                    "openbot-ch-delete-hist",
                    IconName::DeleteTrash,
                    tr_str(lang, "删除历史记录", "Delete history"),
                    Some(tr_str(lang, "删除", "Delete").to_string()),
                    false,
                    true,
                    &ob,
                    |this, _| {
                        this.pending_confirm_guard =
                            Some(OpenBotConfirmGuard::ChannelDeleteHistory);
                    },
                    cx,
                ),
            ],
        )
    });

    // Members Roster (36px rows, 24x24px avatar, Lead Crown always visible when lead, Remove + Add Member inline picker)
    let lead_id = inspector.channel_lead_user_id;
    let mut member_rows: Vec<AnyElement> = Vec::new();
    for m in &inspector.members.members {
        let mid = m.id;
        let mname = m.name.clone();
        let mname_rm = m.name.clone();
        let is_lead = lead_id == Some(mid)
            || m.role.eq_ignore_ascii_case("owner")
            || m.role.eq_ignore_ascii_case("lead")
            || m.role == "群主"
            || m.role == "主理人";
        let is_bot_role = m.role.to_ascii_lowercase().contains("bot");
        let av_el: AnyElement = if is_bot_role {
            render_plush_bot_avatar(
                &m.name,
                None,
                Some(AvatarHue::ALL[(mid as usize) % AvatarHue::ALL.len()]),
                None,
                24.0,
                AvatarMood::Idle,
                None,
            )
            .into_any_element()
        } else {
            div()
                .w(px(24.0))
                .h(px(24.0))
                .rounded_full()
                .bg(rgb(if m.color != 0 { m.color } else { 0x3A3A3C }))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
                .child(m.initials.clone())
                .into_any_element()
        };

        member_rows.push(
            div()
                .id(("openbot-ch-member-row", mid))
                .w_full()
                .min_h(px(36.0))
                .px(px(10.0))
                .py(px(4.0))
                .hover(|s| s.bg(ob.hover()))
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
                        .gap(px(8.0))
                        .child(av_el)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(ob.text_primary())
                                        .child(m.name.clone()),
                                )
                                .when(!m.role.is_empty(), |r| {
                                    r.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_size(px(11.0))
                                            .text_color(ob.text_muted())
                                            .child(m.role.clone()),
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .id(("openbot-ch-member-crown", mid))
                                .cursor_pointer()
                                .w(px(24.0))
                                .h(px(24.0))
                                .rounded(px(6.0))
                                .bg(if is_lead {
                                    TdesktopThemePalette::rgba_with_alpha(0xF5A623, 0.16)
                                } else {
                                    Rgba::default()
                                })
                                .hover(|s| s.bg(ob.bg_control_hover()))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(12.0))
                                .text_color(if is_lead {
                                    rgb(0xF5A623)
                                } else {
                                    ob.text_dim()
                                })
                                .when(!read_only, |el| {
                                    el.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.channel_lead_user_id = Some(mid);
                                            for mem in &mut this.members.members {
                                                if mem.id == mid {
                                                    mem.role = "Lead".to_string();
                                                } else if mem.role == "Lead" {
                                                    mem.role = "Member".to_string();
                                                }
                                            }
                                            this.pending_openbot_action = Some(
                                                OpenBotInspectorAction::PromoteChannelLead {
                                                    user_id: mid,
                                                    member_name: mname.clone(),
                                                },
                                            );
                                            cx.notify();
                                        }),
                                    )
                                })
                                .child("♛"),
                        )
                        .when(!read_only, |acts| {
                            acts.child(
                                div()
                                    .id(("openbot-ch-member-rm", mid))
                                    .cursor_pointer()
                                    .h(px(24.0))
                                    .px(px(7.0))
                                    .rounded(px(6.0))
                                    .hover(move |s| {
                                        s.bg(TdesktopThemePalette::rgba_with_alpha(0xFD2F3B, 0.14))
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(11.0))
                                    .text_color(ob.danger())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.pending_confirm_guard =
                                                Some(OpenBotConfirmGuard::ChannelRemoveMember {
                                                    user_id: mid,
                                                    name: mname_rm.clone(),
                                                });
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "移除", "Remove")),
                            )
                        }),
                )
                .into_any_element(),
        );
    }

    if !read_only {
        member_rows.push(
            div()
                .id("inspector-toggle-add-member-picker")
                .cursor_pointer()
                .w_full()
                .min_h(px(36.0))
                .px(px(12.0))
                .hover(|s| s.bg(ob.hover()))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ob.text_secondary())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if this.add_member_picker_open {
                            this.close_add_member_picker();
                        } else {
                            this.pending_action = Some(InspectorAction::OpenAddMember);
                        }
                        cx.notify();
                    }),
                )
                .child(tg_icon_rgba(IconName::GroupUsers, 14.0, ob.text_muted()))
                .child(if inspector.add_member_picker_open {
                    tr_str(lang, "收起添加列表", "Hide Member Picker")
                } else {
                    tr_str(lang, "+ 添加助手 / 成员", "+ Add Agent / Member")
                })
                .into_any_element(),
        );
    }

    let members_group = ob_inset_group(
        &format!("{} ({})", tr_str(lang, "成员", "Members"), inspector.members.members.len()),
        &ob,
        member_rows,
    );

    // Inline searchable member picker when open
    let inline_picker: Option<AnyElement> = (inspector.add_member_picker_open && !read_only)
        .then(|| {
            let search_pre = inspector
                .add_member_search_input
                .as_ref()
                .and_then(|c| c.read(cx).ime_preedit.clone());
            let filtered = filter_add_member_candidates(
                &inspector.add_member_candidates,
                &inspector.add_member_query,
            );
            let mut list = div()
                .id("inspector-add-member-list")
                .w_full()
                .max_h(px(220.0))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(2.0));
            if filtered.is_empty() {
                list = list.child(
                    div()
                        .py(px(10.0))
                        .text_center()
                        .text_size(px(12.0))
                        .text_color(ob.text_muted())
                        .child(tr_str(lang, "暂无可添加的联系人或助手", "No candidates found")),
                );
            } else {
                for cand in filtered {
                    let uid = cand.user_id;
                    list = list.child(
                        div()
                            .id(("openbot-ch-add-cand", uid))
                            .w_full()
                            .min_h(px(32.0))
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .hover(|s| s.bg(ob.hover()))
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
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(12.5))
                                            .text_color(ob.text_primary())
                                            .child(cand.display_name.clone()),
                                    )
                                    .when(cand.is_bot, |r| {
                                        r.child(
                                            div()
                                                .px(px(5.0))
                                                .py(px(1.0))
                                                .rounded(px(4.0))
                                                .bg(ob.bg_control_active())
                                                .text_size(px(10.0))
                                                .text_color(ob.text_muted())
                                                .child("Bot"),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .id(("inspector-add-member-btn", uid))
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(3.0))
                                    .rounded(px(6.0))
                                    .bg(ob.bg_control_hover())
                                    .text_size(px(11.5))
                                    .text_color(ob.text_primary())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.request_add_member_to_active_group(uid);
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "+ 添加", "+ Add")),
                            ),
                    );
                }
            }

            let disp = if inspector.add_member_query.is_empty() && search_pre.is_none() {
                tr_str(lang, "搜索助手或联系人…", "Search agents or contacts…").to_string()
            } else if let Some(pre) = search_pre {
                format!("{}{}|", inspector.add_member_query, pre)
            } else {
                format!("{}|", inspector.add_member_query)
            };

            div()
                .w_full()
                .p(px(10.0))
                .rounded(px(8.0))
                .bg(ob.bg_surface())
                .border_1()
                .border_color(ob.border_strong_hairline())
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .id("inspector-add-member-search")
                        .cursor_text()
                        .w_full()
                        .h(px(32.0))
                        .px(px(9.0))
                        .rounded(px(7.0))
                        .bg(ob.bg_canvas())
                        .border_1()
                        .border_color(if inspector.add_member_search_focused {
                            ob.accent()
                        } else {
                            ob.border_hairline()
                        })
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.openbot_text_focus = None;
                                this.add_member_search_focused = true;
                                cx.notify();
                            }),
                        )
                        .child(tg_icon_rgba(IconName::Search, 13.0, ob.text_muted()))
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_size(px(12.0))
                                .text_color(if inspector.add_member_query.is_empty() {
                                    ob.text_muted()
                                } else {
                                    ob.text_primary()
                                })
                                .child(disp),
                        ),
                )
                .child(list)
                .into_any_element()
        });

    let guard = inspector.pending_confirm_guard.clone();
    let leave_group = ob_inset_group(
        "",
        &ob,
        vec![ob_inset_row(
            "inspector-leave-group-row",
            IconName::DeleteTrash,
            tr_str(lang, "退出群聊", "Leave Group"),
            None,
            false,
            true,
            &ob,
            |this, _| {
                this.pending_leave_chat = true;
            },
            cx,
        )],
    );

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .when(read_only, |col| {
            col.child(
                div()
                    .w_full()
                    .px(px(12.0))
                    .py(px(9.0))
                    .rounded(px(8.0))
                    .bg(ob.bg_surface())
                    .border_1()
                    .border_color(ob.warning())
                    .text_size(px(12.0))
                    .text_color(ob.warning())
                    .child(tr_str(
                        lang,
                        "此群聊已归档，当前为只读锁定状态。",
                        "This channel is archived and locked read-only.",
                    )),
            )
        })
        .when_some(inspector.copied_feedback.clone(), |col, hint| {
            col.child(
                div()
                    .w_full()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.14))
                    .text_size(px(12.0))
                    .text_color(ob.success())
                    .child(hint),
            )
        })
        .child(hero)
        .child(fields)
        .child(browser_and_isolation)
        .child(links_group)
        .when_some(context_actions, |col, acts| col.child(acts))
        .when_some(guard, |col, g| {
            let is_danger = inspector
                .active_confirm_dialog_state()
                .map(|s| s.destructive)
                .unwrap_or_else(|| g.is_destructive());
            match g {
                OpenBotConfirmGuard::ChannelCompactContext => col.child(ob_confirm_guard_card(
                    "openbot-guard-ch-compact",
                    tr_str(lang, "确认压缩频道上下文？", "Compact Channel Context?"),
                    tr_str(
                        lang,
                        "将总结早期对话以释放上下文窗口空间。",
                        "Summarizes earlier turns to free up context budget.",
                    ),
                    tr_str(lang, "确认压缩", "Compact"),
                    is_danger,
                    &ob,
                    lang,
                    |this, _| {
                        this.pending_openbot_action =
                            Some(OpenBotInspectorAction::CompactChannelContext);
                    },
                    cx,
                )),
                OpenBotConfirmGuard::ChannelNewConversation => col.child(ob_confirm_guard_card(
                    "openbot-guard-ch-new",
                    tr_str(lang, "确认开启新对话？", "Start New Conversation?"),
                    tr_str(
                        lang,
                        "将开启新的群聊上下文线程。",
                        "Starts a fresh conversation thread in this channel.",
                    ),
                    tr_str(lang, "确认开启", "Start"),
                    is_danger,
                    &ob,
                    lang,
                    |this, _| {
                        this.pending_openbot_action =
                            Some(OpenBotInspectorAction::StartNewChannelConversation);
                    },
                    cx,
                )),
                OpenBotConfirmGuard::ChannelDeleteHistory => col.child(ob_confirm_guard_card(
                    "openbot-guard-ch-delhist",
                    tr_str(lang, "确认删除群聊历史记录？", "Delete Channel History?"),
                    tr_str(
                        lang,
                        "将永久清空该群聊中的所有消息记录，无法撤销。",
                        "Permanently clears all messages in this channel.",
                    ),
                    tr_str(lang, "确认删除", "Delete"),
                    is_danger,
                    &ob,
                    lang,
                    |this, _| {
                        this.pending_openbot_action = Some(OpenBotInspectorAction::DeleteChatHistory);
                    },
                    cx,
                )),
                OpenBotConfirmGuard::ChannelRemoveMember { user_id, name } => {
                    let nm = name.clone();
                    col.child(ob_confirm_guard_card(
                        "openbot-guard-ch-rm-mem",
                        &format!("{} {name}?", tr_str(lang, "确认移除成员", "Remove member")),
                        tr_str(
                            lang,
                            "移除后该成员将不再接收本群消息。",
                            "This member will be removed from the channel.",
                        ),
                        tr_str(lang, "确认移除", "Remove"),
                        is_danger,
                        &ob,
                        lang,
                        move |this, _| {
                            this.members.members.retain(|m| m.id != user_id);
                            if this.channel_lead_user_id == Some(user_id) {
                                this.channel_lead_user_id = this.members.members.first().map(|m| m.id);
                            }
                            if this.channel_browser_access == ChannelBrowserAccess::Member(user_id) {
                                this.channel_browser_access = ChannelBrowserAccess::None;
                            }
                            this.pending_openbot_action =
                                Some(OpenBotInspectorAction::RemoveChannelMember {
                                    user_id,
                                    member_name: nm.clone(),
                                });
                        },
                        cx,
                    ))
                }
                _ => col,
            }
        })
        .child(members_group)
        .when_some(inline_picker, |col, p| col.child(p))
        .child(leave_group)
        .into_any_element()
}

/// Renders drill-down subpages inside `ChannelSettingsPane` (`ChannelMemories`, `ChannelRoutines`, `ChannelFolders`).
pub fn render_openbot_channel_subpage(
    inspector: &ContextInspector,
    subpage: OpenBotSettingsSubpage,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let preedit = inspector.openbot_input_preedit(cx);
    let read_only = inspector.channel_archived;
    match subpage {
        OpenBotSettingsSubpage::ChannelMemories => {
            let mut rows: Vec<AnyElement> = inspector
                .channel_memories
                .iter()
                .enumerate()
                .map(|(idx, txt)| {
                    div()
                        .w_full()
                        .px(px(12.0))
                        .py(px(9.0))
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.5))
                                .text_color(ob.text_primary())
                                .child(txt.clone()),
                        )
                        .when(!read_only, |r| {
                            r.child(
                                div()
                                    .id(SharedString::from(format!("openbot-ch-mem-del-{idx}")))
                                    .cursor_pointer()
                                    .text_size(px(12.0))
                                    .text_color(ob.danger())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            if idx < this.channel_memories.len() {
                                                this.channel_memories.remove(idx);
                                                cx.notify();
                                            }
                                        }),
                                    )
                                    .child("×"),
                            )
                        })
                        .into_any_element()
                })
                .collect();
            if rows.is_empty() {
                rows.push(
                    div()
                        .px(px(12.0))
                        .py(px(10.0))
                        .text_size(px(12.5))
                        .text_color(ob.text_muted())
                        .child(tr_str(lang, "暂无频道记忆。", "No channel memories yet."))
                        .into_any_element(),
                );
            }
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(ob_inset_group(tr_str(lang, "频道记忆 (<=32)", "Channel Memories"), &ob, rows))
                .when(!read_only, |col| {
                    col.child(ob_text_input_box(
                        "openbot-ch-mem-input",
                        tr_str(lang, "添加频道共享记忆", "Add channel memory"),
                        &inspector.channel_memory_draft,
                        preedit.as_deref(),
                        tr_str(lang, "例如：周会纪要统一按模板输出…", "Add shared channel rule…"),
                        inspector.openbot_text_focus
                            == Some(OpenBotInspectorTextField::ChannelMemoryDraft),
                        38.0,
                        false,
                        OpenBotInspectorTextField::ChannelMemoryDraft,
                        pal,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .child(
                                div()
                                    .id("openbot-ch-mem-add")
                                    .cursor_pointer()
                                    .px(px(12.0))
                                    .py(px(5.0))
                                    .rounded(px(6.0))
                                    .bg(ob.bg_control_hover())
                                    .text_size(px(12.0))
                                    .text_color(ob.text_primary())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.add_channel_memory_from_draft();
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "+ 添加", "+ Add")),
                            ),
                    )
                })
                .into_any_element()
        }
        OpenBotSettingsSubpage::ChannelFolders => {
            let mut rows: Vec<AnyElement> = inspector
                .channel_folders
                .iter()
                .enumerate()
                .map(|(idx, p)| {
                    div()
                        .w_full()
                        .min_h(px(36.0))
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(ob.text_primary())
                                .truncate()
                                .child(p.clone()),
                        )
                        .when(!read_only, |r| {
                            r.child(
                                div()
                                    .id(SharedString::from(format!("openbot-ch-folder-del-{idx}")))
                                    .cursor_pointer()
                                    .text_size(px(12.0))
                                    .text_color(ob.danger())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            if idx < this.channel_folders.len() {
                                                this.channel_folders.remove(idx);
                                                cx.notify();
                                            }
                                        }),
                                    )
                                    .child("×"),
                            )
                        })
                        .into_any_element()
                })
                .collect();
            if rows.is_empty() {
                rows.push(
                    div()
                        .px(px(12.0))
                        .py(px(10.0))
                        .text_size(px(12.5))
                        .text_color(ob.text_muted())
                        .child(tr_str(lang, "暂无绑定目录。", "No folders linked."))
                        .into_any_element(),
                );
            }
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(ob_inset_group(tr_str(lang, "频道目录 (<=8)", "Channel Folders"), &ob, rows))
                .when(!read_only, |col| {
                    col.child(ob_text_input_box(
                        "openbot-ch-folder-input",
                        tr_str(lang, "添加共享目录", "Add folder path"),
                        &inspector.channel_folder_draft,
                        preedit.as_deref(),
                        "/workspace/shared",
                        inspector.openbot_text_focus
                            == Some(OpenBotInspectorTextField::ChannelFolderDraft),
                        38.0,
                        false,
                        OpenBotInspectorTextField::ChannelFolderDraft,
                        pal,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .child(
                                div()
                                    .id("openbot-ch-folder-add")
                                    .cursor_pointer()
                                    .px(px(12.0))
                                    .py(px(5.0))
                                    .rounded(px(6.0))
                                    .bg(ob.bg_control_hover())
                                    .text_size(px(12.0))
                                    .text_color(ob.text_primary())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.add_channel_folder_from_draft();
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "+ 添加", "+ Add")),
                            ),
                    )
                })
                .into_any_element()
        }
        OpenBotSettingsSubpage::ChannelRoutines => ob_inset_group(
            tr_str(lang, "频道自动化任务", "Channel Routines"),
            &ob,
            vec![
                div()
                    .px(px(12.0))
                    .py(px(12.0))
                    .text_size(px(12.5))
                    .text_color(ob.text_muted())
                    .child(tr_str(
                        lang,
                        "在群内添加助手后，可在「自动化任务」中为本群配置定时汇总与巡检。",
                        "Configure scheduled digests and checks for this channel.",
                    ))
                    .into_any_element(),
            ],
        ),
        _ => render_openbot_channel_settings_root(inspector, pal, lang, cx),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenBotContactProfileLayout {
    pub mounted: bool,
    pub host: &'static str,
    pub avatar_px: f32,
    pub handle: String,
    pub online: bool,
    pub online_dot_hex: String,
    pub block_confirm_open: bool,
    pub blocked: bool,
    pub destructive_confirm_gated: bool,
}

pub fn compute_openbot_contact_profile_layout(
    inspector: &ContextInspector,
    pal: &TdesktopThemePalette,
) -> OpenBotContactProfileLayout {
    let ob = pal.openbot_palette();
    let status = inspector.profile.status.trim();
    let clean_handle = inspector
        .profile
        .username
        .trim()
        .trim_start_matches('@');
    let handle = if clean_handle.is_empty() {
        String::new()
    } else {
        format!("@{clean_handle}")
    };
    let success_rgb = crate::views::content_hub::apple_primitives::rgba_to_rgb24(ob.success());
    OpenBotContactProfileLayout {
        mounted: inspector.is_open
            && inspector.embedded_in_side_panel
            && !inspector.profile.is_bot
            && !inspector.members.is_group_or_channel,
        host: if inspector.embedded_in_side_panel {
            "side_panel_contact_profile"
        } else {
            "classic_drawer"
        },
        avatar_px: 62.0,
        handle,
        online: status.eq_ignore_ascii_case("online") || status == "在线",
        online_dot_hex: crate::views::content_hub::apple_primitives::rgb24_to_hex(success_rgb),
        block_confirm_open: inspector.pending_confirm_guard
            == Some(OpenBotConfirmGuard::ContactBlock),
        blocked: inspector.is_contact_blocked,
        destructive_confirm_gated: pal.is_apple_style(),
    }
}

/// Renders the OpenBot-styled 1:1 Human Contact Profile (`UI-F17`) inside `打开 联系人资料`.
pub fn render_openbot_contact_profile_root(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let contact_layout = compute_openbot_contact_profile_layout(inspector, &pal);
    let ob = pal.openbot_palette();
    let model = &inspector.profile;
    let layout = build_contact_profile_layout(model, &inspector.members, pal, lang);
    let copied_prefix = tr_str(lang, "已复制", "Copied").to_string();

    let avatar_el: AnyElement = if let Some(ref path) = model.avatar_path {
        div()
            .w(px(contact_layout.avatar_px))
            .h(px(contact_layout.avatar_px))
            .rounded_full()
            .overflow_hidden()
            .border_1()
            .border_color(ob.border_hairline())
            .child(
                img(path.clone())
                    .w(px(contact_layout.avatar_px))
                    .h(px(contact_layout.avatar_px))
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element()
    } else {
        let init = if model.avatar_initials.trim().is_empty() {
            "?".to_string()
        } else {
            model.avatar_initials.clone()
        };
        div()
            .w(px(contact_layout.avatar_px))
            .h(px(contact_layout.avatar_px))
            .rounded_full()
            .bg(rgb(if model.avatar_color != 0 {
                model.avatar_color
            } else {
                0x3A3A3C
            }))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(22.0))
            .font_weight(FontWeight::BOLD)
            .text_color(rgb(0xFFFFFF))
            .child(init)
            .into_any_element()
    };

    let online = contact_layout.online;
    let online_dot_rgb = crate::views::content_hub::apple_primitives::parse_hex_color(
        &contact_layout.online_dot_hex,
        0x31CF76,
    );
    let status_txt = if online {
        tr_str(lang, "在线", "Online").to_string()
    } else if !model.status.trim().is_empty() {
        model.status.clone()
    } else {
        contact_layout.handle.clone()
    };

    let hero = div()
        .w_full()
        .pt(px(6.0))
        .pb(px(4.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(6.0))
        .child(avatar_el)
        .child(
            div()
                .text_size(px(16.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_primary())
                .child(model.name.clone()),
        )
        .when(!status_txt.is_empty(), |col| {
            col.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .when(online, |r| {
                        r.child(
                            div()
                                .w(px(7.0))
                                .h(px(7.0))
                                .rounded_full()
                                .bg(rgb(online_dot_rgb)),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(ob.text_muted())
                            .child(status_txt),
                    ),
            )
        });

    let info_group = (!layout.info_rows.is_empty()).then(|| {
        let rows: Vec<AnyElement> = layout
            .info_rows
            .into_iter()
            .enumerate()
            .map(|(idx, r)| {
                let cp = r.value.clone();
                let pfx = copied_prefix.clone();
                ob_inset_row(
                    SharedString::from(format!("contact-info-row-{idx}")),
                    r.icon,
                    r.value,
                    Some(r.subtitle),
                    false,
                    false,
                    &ob,
                    move |this, cx| {
                        this.show_copied_feedback(format!("{pfx}: {cp}"), cx);
                        this.pending_action = Some(InspectorAction::CopyText(cp.clone()));
                    },
                    cx,
                )
            })
            .collect();
        ob_inset_group(&layout.info_section_title, &ob, rows)
    });

    let media_total = inspector.shared_media.media_items.len()
        + inspector.shared_media.files.len()
        + inspector.shared_media.links.len();
    let media_card = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .when(!inspector.shared_media.media_items.is_empty(), |col| {
            col.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .children(
                        inspector
                            .shared_media
                            .media_items
                            .iter()
                            .take(6)
                            .map(|item| render_media_thumb_tile(item, 86.0, pal, cx)),
                    ),
            )
        })
        .child(ob_inset_group(
            tr_str(lang, "共享内容", "Shared Content"),
            &ob,
            vec![ob_inset_row(
                "inspector-media-see-all",
                IconName::Camera,
                tr_str(lang, "共享媒体 / 文件 / 链接", "Shared Media / Files / Links"),
                Some(format!("{media_total}")),
                true,
                false,
                &ob,
                |this, _| {
                    this.openbot_subpage = Some(OpenBotSettingsSubpage::SharedMedia);
                },
                cx,
            )],
        ));

    let notif_on = model.is_notifications_enabled;
    let secret_on = inspector.is_secret_chat_active;
    let blocked = inspector.is_contact_blocked;
    let actions_group = ob_inset_group(
        &layout.privacy_section_title,
        &ob,
        vec![
            div()
                .id("inspector-quick-mute")
                .w_full()
                .min_h(px(40.0))
                .px(px(12.0))
                .cursor_pointer()
                .hover(|s| s.bg(ob.hover()))
                .flex()
                .items_center()
                .justify_between()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.profile.is_notifications_enabled = !this.profile.is_notifications_enabled;
                        this.pending_action = Some(InspectorAction::ToggleNotifications(
                            this.profile.is_notifications_enabled,
                        ));
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(9.0))
                        .child(tg_icon_rgba(
                            if notif_on { IconName::Speaker } else { IconName::Mute },
                            14.0,
                            ob.text_muted(),
                        ))
                        .child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_primary())
                                .child(tr_str(lang, "消息通知", "Notifications")),
                        ),
                )
                .child(ob_switch_pill(notif_on, &ob))
                .into_any_element(),
            ob_inset_row(
                "inspector-edit-peer-row",
                IconName::Edit,
                layout.edit_row_label,
                None,
                true,
                false,
                &ob,
                |this, _| {
                    this.pending_action = Some(InspectorAction::OpenEditPeer);
                },
                cx,
            ),
            div()
                .id("inspector-secret-chat-toggle")
                .w_full()
                .min_h(px(40.0))
                .px(px(12.0))
                .cursor_pointer()
                .hover(|s| s.bg(ob.hover()))
                .flex()
                .items_center()
                .justify_between()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.is_secret_chat_active = !this.is_secret_chat_active;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(9.0))
                        .child(tg_icon_rgba(IconName::Lock, 14.0, ob.text_muted()))
                        .child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_primary())
                                .child(tr_str(lang, "发起私密聊天", "Start Secret Chat")),
                        ),
                )
                .child(ob_switch_pill(secret_on, &ob))
                .into_any_element(),
            ob_inset_row(
                "inspector-block-contact-row",
                IconName::Close,
                if blocked {
                    tr_str(lang, "取消拉黑联系人", "Unblock Contact")
                } else {
                    tr_str(lang, "拉黑联系人", "Block Contact")
                },
                None,
                false,
                true,
                &ob,
                move |this, _| {
                    if blocked {
                        this.is_contact_blocked = false;
                        this.pending_confirm_guard = None;
                    } else {
                        this.pending_confirm_guard = Some(OpenBotConfirmGuard::ContactBlock);
                    }
                },
                cx,
            ),
        ],
    );

    let confirm_block_card = contact_layout.block_confirm_open.then(|| {
        ob_confirm_guard_card(
            "inspector-confirm-block-contact",
            tr_str(lang, "拉黑该联系人？", "Block this contact?"),
            tr_str(
                lang,
                "拉黑后对方将无法向你发送消息，你可以随时取消拉黑。",
                "Blocked contacts cannot message you until you unblock them.",
            ),
            tr_str(lang, "确认拉黑", "Block"),
            true,
            &ob,
            lang,
            |this, _| {
                this.is_contact_blocked = true;
                this.pending_confirm_guard = None;
            },
            cx,
        )
    });

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .when_some(inspector.copied_feedback.clone(), |col, hint| {
            col.child(
                div()
                    .w_full()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.14))
                    .text_size(px(12.0))
                    .text_color(ob.success())
                    .child(hint),
            )
        })
        .child(hero)
        .when_some(info_group, |col, g| col.child(g))
        .child(media_card)
        .child(actions_group)
        .when_some(confirm_block_card, |col, card| col.child(card))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn openbot_channel_icon_catalog_and_browser_policy_contracts() {
        assert_eq!(CHANNEL_ICON_CATALOG.len(), 20);
        assert_eq!(CHANNEL_ICON_HUES.len(), 10);
        assert_eq!(ChannelBrowserAccess::Everyone.as_policy_str(), "everyone");
        assert_eq!(ChannelBrowserAccess::Member(42).as_policy_str(), "member:42");
        assert_eq!(ChannelBrowserAccess::None.as_policy_str(), "none");
    }
}
