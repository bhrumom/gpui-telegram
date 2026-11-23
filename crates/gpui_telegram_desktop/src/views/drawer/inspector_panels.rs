//! Inspector Drawer sub-panels: Apple Messages Contact Profile, Members,
//! Shared Media/Files/Links/Music/Voice, and Settings & Statistics.

use crate::i18n::tr_str;
use crate::theme::assets::{IconName, tg_icon};
use crate::theme::palette::TdesktopThemePalette;
use crate::views::context_inspector::{filter_add_member_candidates, AddMemberCandidate};
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

pub(crate) fn grouped_card(pal: TdesktopThemePalette) -> Div {
    div()
        .w_full()
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.separator_rgba())
        .overflow_hidden()
}

pub(crate) fn card_hairline(pal: TdesktopThemePalette) -> Div {
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
