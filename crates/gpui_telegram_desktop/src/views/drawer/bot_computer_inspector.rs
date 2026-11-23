use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::miniapp_contracts::{
    FabushiBotSummaryDto, CLASSIC_INITIALS_AVATAR_SHAPE, GROK_AVATAR_SHAPES,
    PatchFabushiBotRequest,
};
use gpui_telegram_proto::{
    BotComputerProfileDto, CodingAgentBotConfig, CodingPermissionMode, ComputerNodeDto,
    ComputerNodeKind, ComputerRunPolicy,
};

use crate::components::grok_bot_avatar::{
    render_grok_bot_avatar, trigger_grok_avatar_bounce, GrokBotAvatarProps, GrokPersonaState,
    GROK_COLOR_PALETTE,
};
use crate::i18n::tr_str;
use crate::theme::assets::{IconName, tg_icon};
use crate::theme::palette::TdesktopThemePalette;
use crate::views::computer_workbench::{decode_data_url_image, render_ime_text_preview};
use crate::views::drawer::inspector_panels::{
    apple_switch_pill, muse_collapsible_header, muse_icon_row,
};
use crate::views::drawer::inspector_tabs::{
    BotConfigFocusField, InspectorAction, InspectorSubpage,
};
use crate::views::ContextInspector;

fn node_endpoint(node: &ComputerNodeDto) -> &str {
    node.host
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&node.ip_address)
}

pub fn resolve_bot_computer_binding<'a>(
    computers: &'a [ComputerNodeDto],
    profile: Option<&BotComputerProfileDto>,
) -> (Option<&'a ComputerNodeDto>, Option<&'a ComputerNodeDto>) {
    let explicit = profile
        .and_then(|p| p.bound_node_id.as_deref())
        .and_then(|id| computers.iter().find(|c| c.node_id == id));
    let effective = explicit
        .or_else(|| computers.iter().find(|c| c.is_current_device))
        .or_else(|| computers.first());
    (explicit, effective)
}

fn computer_tier_badge(
    explicit: Option<&ComputerNodeDto>,
    effective: Option<&ComputerNodeDto>,
    lang: &str,
) -> (String, bool) {
    match explicit.or(effective) {
        Some(node) if !node.tier_badge.is_empty() => (node.tier_badge.clone(), true),
        Some(node) if node.kind == ComputerNodeKind::CloudServer => {
            (tr_str(lang, "云电脑", "Cloud").to_string(), true)
        }
        Some(node) if node.kind == ComputerNodeKind::LocalDevice => {
            (tr_str(lang, "本地", "Local").to_string(), true)
        }
        _ => (tr_str(lang, "免费", "Free").to_string(), false),
    }
}

fn format_policy_label(policy: ComputerRunPolicy, lang: &str) -> &'static str {
    if lang.starts_with("zh") {
        policy.label_zh()
    } else {
        policy.label_en()
    }
}

/// Formats a monthly active users count with thousands separators (e.g. `12,345`).
pub fn format_mau_count(mau: i64) -> String {
    let n = mau.max(0) as u64;
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

/// Formats the Telegram bot subtitle (`"{mau} 名每月活跃用户"` or `"机器人"`).
pub fn format_bot_mau_subtitle(mau: i64, lang: &str) -> String {
    if mau > 0 {
        let formatted = format_mau_count(mau);
        if lang.starts_with("zh") {
            format!("{formatted} 名每月活跃用户")
        } else {
            format!("{formatted} monthly users")
        }
    } else {
        tr_str(lang, "机器人", "bot").to_string()
    }
}

/// Renders the Muse-inspired Bot Profile overview (`InspectorTab::Profile`):
/// collapsible 基本信息, 快捷指令, and 常用操作 sections (hero & 4-icon segmented bar live in `ContextInspector`).
pub fn render_bot_profile_main(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let model = &inspector.profile;
    let bot_id = model.bot_id;
    let is_owner_fabushi = model.is_viewer_owner && inspector.active_fabushi_bot.is_some();

    let clean_username = model.username.trim().trim_start_matches('@').to_string();
    let display_username = if clean_username.is_empty() {
        String::new()
    } else {
        format!("@{clean_username}")
    };

    let has_main_app = model.bot_has_main_app || model.miniapp_url.is_some();
    let launch_uri = model.miniapp_url.clone().or_else(|| {
        (model.bot_has_main_app && !clean_username.is_empty())
            .then(|| format!("gpui-miniapp://{clean_username}/app"))
    });
    let copied_prefix = tr_str(lang, "已复制", "Copied").to_string();

    let info_collapsed = inspector.is_section_collapsed("bot-sec-info");
    let cmds_collapsed = inspector.is_section_collapsed("bot-sec-cmds");
    let actions_collapsed = inspector.is_section_collapsed("bot-sec-actions");

    let mut info_sec = div().w_full().flex().flex_col().gap(px(2.0)).child(
        muse_collapsible_header(
            "bot-sec-info-hdr",
            "bot-sec-info",
            tr_str(lang, "基本信息", "Overview"),
            info_collapsed,
            pal,
            cx,
        ),
    );
    if !info_collapsed {
        if !display_username.is_empty() {
            let cp = display_username.clone();
            let cp_fb = display_username.clone();
            info_sec = info_sec.child(muse_icon_row(
                "bot-copy-username",
                IconName::BotRobot,
                pal.accent,
                display_username,
                Some(tr_str(lang, "账号 · 点击复制", "Handle · Click to copy").into()),
                None,
                Some(tg_icon(IconName::Copy, 12.5, pal.text_secondary).into_any_element()),
                pal,
                move |this, _| {
                    this.copied_feedback = Some(format!("{copied_prefix}: {cp_fb}"));
                    this.pending_action = Some(InspectorAction::CopyText(cp.clone()));
                },
                cx,
            ));
        }
        if !model.bio.trim().is_empty() {
            info_sec = info_sec.child(muse_icon_row(
                "bot-info-bio-row",
                IconName::FolderAll,
                pal.text_secondary,
                tr_str(lang, "简介", "About"),
                Some(model.bio.trim().to_string()),
                None,
                None,
                pal,
                |_, _| {},
                cx,
            ));
        }
        if is_owner_fabushi {
            info_sec = info_sec.child(muse_icon_row(
                "bot-action-new-session",
                IconName::ComposePen,
                pal.accent,
                tr_str(lang, "开启新对话 (/new)", "New Session (/new)"),
                Some(tr_str(lang, "重置当前智能体上下文", "Reset conversation context").into()),
                None,
                Some(tg_icon(IconName::ChevronRight, 12.0, pal.text_secondary).into_any_element()),
                pal,
                |this, _| {
                    this.pending_action = Some(InspectorAction::RunBotCommand("/new".to_string()));
                },
                cx,
            ));
        }
        if has_main_app {
            let url = launch_uri.clone().unwrap_or_default();
            let app_title = model
                .miniapp_title
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| tr_str(lang, "打开小程序", "Open Mini App").to_string());
            info_sec = info_sec
                .child(muse_icon_row(
                    "inspector-launch-miniapp-btn",
                    IconName::FolderBots,
                    pal.accent,
                    app_title,
                    Some(tr_str(lang, "启动嵌入式交互应用", "Launch embedded app").into()),
                    Some(pal.accent),
                    Some(tg_icon(IconName::ChevronRight, 12.0, pal.accent).into_any_element()),
                    pal,
                    move |this, _| {
                        this.pending_action = Some(InspectorAction::LaunchMiniApp(url.clone()));
                    },
                    cx,
                ))
                .child(
                    div()
                        .id("inspector-miniapp-tos-link")
                        .cursor_pointer()
                        .px(px(6.0))
                        .py(px(2.0))
                        .text_size(px(11.0))
                        .text_color(rgb(pal.text_secondary))
                        .hover(|s| s.text_color(rgb(pal.accent)).underline())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.pending_action = Some(InspectorAction::OpenMiniAppTos);
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "查看小程序服务条款", "Mini App Terms of Service")),
                );
        }
    }

    let cmds_sec = (!model.bot_commands.is_empty()).then(|| {
        let mut sec = div().w_full().flex().flex_col().gap(px(2.0)).child(
            muse_collapsible_header(
                "bot-sec-cmds-hdr",
                "bot-sec-cmds",
                tr_str(lang, "快捷指令", "Quick Commands"),
                cmds_collapsed,
                pal,
                cx,
            ),
        );
        if !cmds_collapsed {
            for (idx, (cmd, desc)) in model.bot_commands.iter().enumerate() {
                let slash = if cmd.starts_with('/') {
                    cmd.clone()
                } else {
                    format!("/{cmd}")
                };
                let run_cmd = slash.clone();
                let sub = (!desc.trim().is_empty()).then(|| desc.trim().to_string());
                sec = sec.child(muse_icon_row(
                    SharedString::from(format!("bot-cmd-row-{idx}")),
                    IconName::TerminalMcp,
                    pal.text_secondary,
                    slash,
                    sub,
                    None,
                    None,
                    pal,
                    move |this, _| {
                        this.pending_action =
                            Some(InspectorAction::RunBotCommand(run_cmd.clone()));
                    },
                    cx,
                ));
            }
        }
        sec
    });

    let mut actions_sec = div().w_full().flex().flex_col().gap(px(2.0)).child(
        muse_collapsible_header(
            "bot-sec-actions-hdr",
            "bot-sec-actions",
            tr_str(lang, "常用操作", "Actions"),
            actions_collapsed,
            pal,
            cx,
        ),
    );
    if !actions_collapsed {
        let notif_on = model.is_notifications_enabled;
        actions_sec = actions_sec.child(muse_icon_row(
            "bot-action-mute",
            if notif_on { IconName::Speaker } else { IconName::Mute },
            if notif_on { pal.accent } else { pal.text_secondary },
            tr_str(lang, "消息通知", "Notifications"),
            Some(
                if notif_on {
                    tr_str(lang, "已开启消息提醒", "Alerts enabled")
                } else {
                    tr_str(lang, "已静音通知", "Muted")
                }
                .to_string(),
            ),
            None,
            Some(apple_switch_pill(notif_on, pal).into_any_element()),
            pal,
            |this, _| {
                this.profile.is_notifications_enabled = !this.profile.is_notifications_enabled;
                this.pending_action = Some(InspectorAction::ToggleNotifications(
                    this.profile.is_notifications_enabled,
                ));
            },
            cx,
        ));
        if model.bot_can_join_groups || is_owner_fabushi {
            let target_bot_id = inspector
                .active_fabushi_bot
                .as_ref()
                .map(|b| b.bot_user_id.max(0) as u64)
                .filter(|&id| id > 0)
                .unwrap_or(bot_id);
            actions_sec = actions_sec.child(muse_icon_row(
                "bot-profile-add-to-group",
                IconName::GroupUsers,
                pal.accent,
                tr_str(lang, "添加到群聊", "Add to Group"),
                Some(tr_str(lang, "拉入群聊并添加其他成员或 Bot", "Start a group with this bot").into()),
                None,
                Some(tg_icon(IconName::ChevronRight, 12.0, pal.text_secondary).into_any_element()),
                pal,
                move |this, _| {
                    this.pending_action = Some(InspectorAction::OpenAddBotToGroup(target_bot_id));
                },
                cx,
            ));
        }
    }

    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .when_some(copied_feedback_banner(inspector, pal), |col, banner| {
            col.child(banner)
        })
        .child(info_sec)
        .when_some(cmds_sec, |col, sec| col.child(sec))
        .child(actions_sec)
        .into_any_element()
}

/// Compact inline shape & color picker toggled from the hero avatar's edit pencil badge.
pub fn render_bot_avatar_editor_popover(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let Some(fab) = inspector.active_fabushi_bot.clone() else {
        return div().into_any_element();
    };
    let target_bot_id = fab.bot_user_id.max(0) as u64;
    let fab_bg =
        crate::components::avatar::parse_fabushi_hex_color(&fab.avatar_color).unwrap_or(pal.accent);
    let highlight_fg = if fab.avatar_color.eq_ignore_ascii_case("black") {
        pal.accent
    } else {
        fab_bg
    };

    let mut shape_row = div().flex().flex_wrap().items_center().gap(px(5.0));
    for (idx, &shape) in GROK_AVATAR_SHAPES.iter().enumerate() {
        let selected = fab.avatar_shape.eq_ignore_ascii_case(shape);
        let shape_owned = shape.to_string();
        let agent_for_bounce = fab.agent_id.clone();
        shape_row = shape_row.child(
            div()
                .id(SharedString::from(format!("fabushi-insp-shape-{idx}")))
                .cursor_pointer()
                .p(px(4.0))
                .rounded(px(9.0))
                .bg(if selected {
                    TdesktopThemePalette::rgba_with_alpha(highlight_fg, 0.18)
                } else {
                    rgb(pal.input_bg).into()
                })
                .border_1()
                .border_color(if selected {
                    rgb(highlight_fg).into()
                } else {
                    pal.separator_rgba()
                })
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        trigger_grok_avatar_bounce(&agent_for_bounce);
                        this.pending_action = Some(InspectorAction::PatchFabushiBot {
                            bot_id: target_bot_id,
                            req: PatchFabushiBotRequest {
                                avatar_shape: Some(shape_owned.clone()),
                                coding_agent: None,
                                ..Default::default()
                            },
                        });
                        cx.notify();
                    }),
                )
                .child(render_grok_bot_avatar(
                    GrokBotAvatarProps::new(
                        format!("insp-shape-{shape}"),
                        shape,
                        &fab.avatar_color,
                        20.0,
                    )
                    .with_state(GrokPersonaState::Idle)
                    .with_paused(true)
                    .with_surface_bg(pal.card_bg),
                )),
        );
    }

    let classic_selected = fab
        .avatar_shape
        .eq_ignore_ascii_case(CLASSIC_INITIALS_AVATAR_SHAPE);
    let agent_for_classic = fab.agent_id.clone();
    shape_row = shape_row.child(
        div()
            .id("fabushi-insp-shape-classic")
            .cursor_pointer()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(9.0))
            .bg(if classic_selected {
                TdesktopThemePalette::rgba_with_alpha(highlight_fg, 0.18)
            } else {
                rgb(pal.input_bg).into()
            })
            .border_1()
            .border_color(if classic_selected {
                rgb(highlight_fg).into()
            } else {
                pal.separator_rgba()
            })
            .text_size(px(10.5))
            .font_weight(if classic_selected {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            })
            .text_color(rgb(if classic_selected {
                highlight_fg
            } else {
                pal.text_primary
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    trigger_grok_avatar_bounce(&agent_for_classic);
                    this.pending_action = Some(InspectorAction::PatchFabushiBot {
                        bot_id: target_bot_id,
                        req: PatchFabushiBotRequest {
                            avatar_shape: Some(CLASSIC_INITIALS_AVATAR_SHAPE.to_string()),
                            coding_agent: None,
                            ..Default::default()
                        },
                    });
                    cx.notify();
                }),
            )
            .child("classic"),
    );

    let mut color_row = div().flex().flex_wrap().items_center().gap(px(6.0));
    for (idx, spec) in GROK_COLOR_PALETTE.iter().enumerate() {
        let selected = fab.avatar_color.eq_ignore_ascii_case(spec.id)
            || crate::components::avatar::parse_fabushi_hex_color(&fab.avatar_color)
                == Some(spec.swatch_rgb);
        let color_id = spec.id.to_string();
        let agent_for_bounce = fab.agent_id.clone();
        color_row = color_row.child(
            div()
                .id(SharedString::from(format!("fabushi-insp-color-{idx}")))
                .cursor_pointer()
                .w(px(18.0))
                .h(px(18.0))
                .rounded_full()
                .bg(rgb(spec.swatch_rgb))
                .border_2()
                .border_color(rgb(if selected {
                    pal.text_primary
                } else {
                    pal.border_fg
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        trigger_grok_avatar_bounce(&agent_for_bounce);
                        this.pending_action = Some(InspectorAction::PatchFabushiBot {
                            bot_id: target_bot_id,
                            req: PatchFabushiBotRequest {
                                avatar_color: Some(color_id.clone()),
                                coding_agent: None,
                                ..Default::default()
                            },
                        });
                        cx.notify();
                    }),
                ),
        );
    }

    div()
        .id("bot-avatar-editor-popover")
        .mx(px(14.0))
        .mt(px(8.0))
        .p(px(10.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.separator_rgba())
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
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(lang, "自定义智能体形象", "Customize Avatar")),
                )
                .child(
                    div()
                        .id("bot-avatar-editor-done")
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.avatar_editor_open = false;
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "完成", "Done")),
                ),
        )
        .child(shape_row)
        .child(color_row)
        .into_any_element()
}

fn copied_feedback_banner(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
) -> Option<AnyElement> {
    inspector.copied_feedback.as_ref().map(|hint| {
        div()
            .w_full()
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(9.0))
            .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.12))
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(pal.success))
            .child(hint.clone())
            .into_any_element()
    })
}

/// Tab 2 (`InspectorTab::Members` on a bot): clean collapsible Agent Config, Coding Agent, and Computers sections.
pub fn render_bot_agent_and_computer_tab(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let model = &inspector.profile;
    let bot_id = model.bot_id;
    let is_owner_fabushi = model.is_viewer_owner && inspector.active_fabushi_bot.is_some();

    if !is_owner_fabushi {
        let caps_collapsed = inspector.is_section_collapsed("bot-sec-caps");
        let has_help_cmd = model
            .bot_commands
            .iter()
            .any(|(c, _)| c.trim_start_matches('/').eq_ignore_ascii_case("help"));
        let has_settings_row = model.bot_has_settings
            || model
                .bot_commands
                .iter()
                .any(|(c, _)| c.trim_start_matches('/').eq_ignore_ascii_case("settings"));

        let mut sec = div().w_full().flex().flex_col().gap(px(2.0)).child(
            muse_collapsible_header(
                "bot-sec-caps-hdr",
                "bot-sec-caps",
                tr_str(lang, "机器人能力", "Bot Capabilities"),
                caps_collapsed,
                pal,
                cx,
            ),
        );
        if !caps_collapsed {
            if has_help_cmd {
                sec = sec.child(muse_icon_row(
                    "bot-profile-help",
                    IconName::FolderBots,
                    pal.accent,
                    tr_str(lang, "机器人帮助 (/help)", "Bot Help (/help)"),
                    None,
                    None,
                    Some(tg_icon(IconName::ChevronRight, 12.0, pal.text_secondary).into_any_element()),
                    pal,
                    |this, _| {
                        this.pending_action =
                            Some(InspectorAction::RunBotCommand("/help".to_string()));
                    },
                    cx,
                ));
            }
            if has_settings_row {
                sec = sec.child(muse_icon_row(
                    "bot-profile-settings",
                    IconName::SettingsGear,
                    pal.accent,
                    tr_str(lang, "机器人设置 (/settings)", "Bot Settings"),
                    None,
                    None,
                    Some(tg_icon(IconName::ChevronRight, 12.0, pal.text_secondary).into_any_element()),
                    pal,
                    move |this, _| {
                        this.pending_action = Some(InspectorAction::OpenBotSettings(bot_id));
                    },
                    cx,
                ));
            }
            if !has_help_cmd && !has_settings_row {
                sec = sec.child(
                    div()
                        .px(px(8.0))
                        .py(px(12.0))
                        .text_size(px(12.0))
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(
                            lang,
                            "第三方机器人 · 使用聊天输入框发送消息即可交互",
                            "External bot · Send a message below to interact",
                        )),
                );
            }
        }
        return sec.into_any_element();
    }

    let profile_dto = inspector.bot_computer_profile.as_ref();
    let (explicit_node, effective_node) =
        resolve_bot_computer_binding(&inspector.computers, profile_dto);
    let (tier_label, _has_bound_style) =
        computer_tier_badge(explicit_node, effective_node, lang);

    let bound_name = explicit_node
        .map(|n| n.display_name.clone())
        .or_else(|| {
            effective_node.map(|n| {
                format!(
                    "{} ({})",
                    n.display_name,
                    tr_str(lang, "当前设备", "Current")
                )
            })
        })
        .unwrap_or_else(|| tr_str(lang, "未绑定", "Unbound").to_string());
    let model_name = profile_dto
        .map(|p| p.preferred_model.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| tr_str(lang, "默认模型", "Default Model").to_string());
    let approval_mode = profile_dto
        .map(|p| p.approval_mode)
        .unwrap_or(ComputerRunPolicy::AskEveryTime);

    let effective_node_id = effective_node.map(|n| n.node_id.clone());
    let preview_title = if lang.starts_with("zh") {
        format!(
            "{}的电脑",
            if model.name.is_empty() { "Agent" } else { model.name.as_str() }
        )
    } else {
        format!(
            "{}'s Computer",
            if model.name.is_empty() { "Agent" } else { model.name.as_str() }
        )
    };

    let agent_collapsed = inspector.is_section_collapsed("bot-sec-agent");
    let comp_collapsed = inspector.is_section_collapsed("bot-sec-computer");
    let in_attach = model.in_attach_menu;

    let mut out = div().w_full().flex().flex_col().gap(px(14.0));

    if let Some(fab) = inspector.active_fabushi_bot.clone() {
        let target_bot_id = fab.bot_user_id.max(0) as u64;
        let mut agent_sec = div().w_full().flex().flex_col().gap(px(4.0)).child(
            muse_collapsible_header(
                "bot-sec-agent-hdr",
                "bot-sec-agent",
                tr_str(lang, "智能体配置", "Agent Configuration"),
                agent_collapsed,
                pal,
                cx,
            ),
        );
        if !agent_collapsed {
            let mut thinking_row = div().flex().flex_wrap().gap(px(5.0));
            for (idx, &lvl) in ["off", "low", "medium", "high"].iter().enumerate() {
                let selected = fab.thinking_level.eq_ignore_ascii_case(lvl);
                let lvl_owned = lvl.to_string();
                thinking_row = thinking_row.child(
                    div()
                        .id(SharedString::from(format!("fabushi-insp-thinking-{idx}")))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(999.0))
                        .bg(if selected {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                        } else {
                            rgb(pal.input_bg).into()
                        })
                        .border_1()
                        .border_color(if selected {
                            rgb(pal.accent).into()
                        } else {
                            pal.separator_rgba()
                        })
                        .text_size(px(11.0))
                        .font_weight(if selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(rgb(if selected {
                            pal.accent
                        } else {
                            pal.text_primary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.pending_action = Some(InspectorAction::PatchFabushiBot {
                                    bot_id: target_bot_id,
                                    req: PatchFabushiBotRequest {
                                        thinking_level: Some(lvl_owned.clone()),
                                        coding_agent: None,
                                        ..Default::default()
                                    },
                                });
                                cx.notify();
                            }),
                        )
                        .child(lvl),
                );
            }

            let mut approval_row = div().flex().flex_wrap().gap(px(5.0));
            for (idx, &mode) in ["prompt", "autonomous", "strict"].iter().enumerate() {
                let selected = fab.approval_mode.eq_ignore_ascii_case(mode);
                let mode_owned = mode.to_string();
                approval_row = approval_row.child(
                    div()
                        .id(SharedString::from(format!("fabushi-insp-approval-{idx}")))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(999.0))
                        .bg(if selected {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                        } else {
                            rgb(pal.input_bg).into()
                        })
                        .border_1()
                        .border_color(if selected {
                            rgb(pal.accent).into()
                        } else {
                            pal.separator_rgba()
                        })
                        .text_size(px(11.0))
                        .font_weight(if selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(rgb(if selected {
                            pal.accent
                        } else {
                            pal.text_primary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.pending_action = Some(InspectorAction::PatchFabushiBot {
                                    bot_id: target_bot_id,
                                    req: PatchFabushiBotRequest {
                                        approval_mode: Some(mode_owned.clone()),
                                        coding_agent: None,
                                        ..Default::default()
                                    },
                                });
                                cx.notify();
                            }),
                        )
                        .child(mode),
                );
            }

            agent_sec = agent_sec
                .child(muse_icon_row(
                    "bot-agent-status-row",
                    IconName::BotRobot,
                    pal.accent,
                    tr_str(lang, "运行状态", "Runtime"),
                    Some(format!(
                        "isolated · Memory {} · Skills {} · {}",
                        fab.memory_entries_count, fab.skills_count, fab.execution_state
                    )),
                    None,
                    None,
                    pal,
                    |_, _| {},
                    cx,
                ))
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(6.0))
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(pal.text_secondary))
                                .child(tr_str(lang, "思考深度", "Thinking Level")),
                        )
                        .child(thinking_row),
                )
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(6.0))
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(pal.text_secondary))
                                .child(tr_str(lang, "工具审批策略", "Approval Policy")),
                        )
                        .child(approval_row),
                )
                .child(muse_icon_row(
                    "bot-open-config-editor",
                    IconName::SettingsGear,
                    pal.accent,
                    tr_str(lang, "编辑 AI Bot 人设与模型", "Edit Persona & Model"),
                    Some(format!(
                        "{model_name} · {}",
                        format_policy_label(approval_mode, lang)
                    )),
                    None,
                    Some(tg_icon(IconName::ChevronRight, 12.0, pal.text_secondary).into_any_element()),
                    pal,
                    |this, cx| {
                        this.open_bot_config_editor(cx);
                    },
                    cx,
                ));

            if bot_id > 0 {
                agent_sec = agent_sec.child(muse_icon_row(
                    "inspector-toggle-attach-btn",
                    IconName::Paperclip,
                    if in_attach { pal.success } else { pal.text_secondary },
                    tr_str(lang, "附件菜单快捷入口", "Attach Menu Shortcut"),
                    Some(
                        if in_attach {
                            tr_str(lang, "已添加到输入栏 + 菜单", "Shown in + menu")
                        } else {
                            tr_str(lang, "添加后可从 + 菜单直接呼出", "Pin to + menu")
                        }
                        .to_string(),
                    ),
                    None,
                    Some(apple_switch_pill(in_attach, pal).into_any_element()),
                    pal,
                    move |this, _| {
                        this.pending_action = Some(InspectorAction::ToggleAttachMenuBot(bot_id));
                    },
                    cx,
                ));
            }
        }
        out = out.child(agent_sec);

        if let Some(coding_cfg) = fab.coding_agent.clone() {
            out = out.child(render_coding_agent_inspector_card(
                &fab,
                &coding_cfg,
                inspector,
                pal,
                lang,
                cx,
            ));
        }
    }

    let mut comp_sec = div().w_full().flex().flex_col().gap(px(6.0)).child(
        muse_collapsible_header(
            "bot-sec-computer-hdr",
            "bot-sec-computer",
            tr_str(lang, "关联电脑", "Bound Computer"),
            comp_collapsed,
            pal,
            cx,
        ),
    );
    if !comp_collapsed {
        let mini_preview_card = render_miniature_desktop_card(
            effective_node,
            &preview_title,
            pal,
            lang,
            cx.listener(move |this, _, _, cx| {
                if let Some(ref id) = effective_node_id {
                    this.pending_action = Some(InspectorAction::OpenRemoteWorkbench {
                        node_id: id.clone(),
                    });
                } else {
                    this.subpage = InspectorSubpage::ComputersList;
                    this.pending_action = Some(InspectorAction::OpenBotComputersSubpage);
                }
                cx.notify();
            }),
        );
        comp_sec = comp_sec
            .child(muse_icon_row(
                "bot-open-computers-list-row",
                IconName::ComputerDesktop,
                pal.accent,
                bound_name,
                Some(format!("{tier_label} · {}", tr_str(lang, "查看与切换电脑", "Manage computers"))),
                None,
                Some(tg_icon(IconName::ChevronRight, 12.0, pal.text_secondary).into_any_element()),
                pal,
                |this, _| {
                    this.subpage = InspectorSubpage::ComputersList;
                    this.pending_action = Some(InspectorAction::OpenBotComputersSubpage);
                },
                cx,
            ))
            .child(mini_preview_card);
    }

    out.child(comp_sec).into_any_element()
}

/// Tab 4 (`InspectorTab::Settings` on a bot): clean notification, auto-delete, and safety actions.
pub fn render_bot_settings_tab(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let model = &inspector.profile;
    let bot_id = model.bot_id;
    let is_blocked = model.is_blocked || inspector.is_contact_blocked;
    let notif_on = model.is_notifications_enabled;

    let settings_collapsed = inspector.is_section_collapsed("bot-sec-settings");
    let safety_collapsed = inspector.is_section_collapsed("bot-sec-safety");

    let mut settings_sec = div().w_full().flex().flex_col().gap(px(2.0)).child(
        muse_collapsible_header(
            "bot-sec-settings-hdr",
            "bot-sec-settings",
            tr_str(lang, "通知与消息", "Notifications & Messages"),
            settings_collapsed,
            pal,
            cx,
        ),
    );
    if !settings_collapsed {
        settings_sec = settings_sec
            .child(muse_icon_row(
                "bot-settings-mute-row",
                if notif_on { IconName::Speaker } else { IconName::Mute },
                if notif_on { pal.accent } else { pal.text_secondary },
                tr_str(lang, "消息通知", "Notifications"),
                None,
                None,
                Some(apple_switch_pill(notif_on, pal).into_any_element()),
                pal,
                |this, _| {
                    this.profile.is_notifications_enabled = !this.profile.is_notifications_enabled;
                    this.pending_action = Some(InspectorAction::ToggleNotifications(
                        this.profile.is_notifications_enabled,
                    ));
                },
                cx,
            ))
            .child(muse_icon_row(
                "bot-settings-ttl-row",
                IconName::ClockSending,
                pal.accent,
                tr_str(lang, "自动删除消息", "Auto-Delete Messages"),
                Some(inspector.settings.auto_delete_period.clone()),
                None,
                Some(tg_icon(IconName::ChevronRight, 12.0, pal.text_secondary).into_any_element()),
                pal,
                |this, _| {
                    let next = match this.settings.auto_delete_period.as_str() {
                        "Off" => "24 Hours",
                        "24 Hours" => "7 Days",
                        "7 Days" => "30 Days",
                        _ => "Off",
                    };
                    this.set_auto_delete_period(next);
                    this.pending_action = Some(InspectorAction::ChangeAutoDelete(next.to_string()));
                },
                cx,
            ));
    }

    let mut safety_sec = div().w_full().flex().flex_col().gap(px(2.0)).child(
        muse_collapsible_header(
            "bot-sec-safety-hdr",
            "bot-sec-safety",
            tr_str(lang, "安全与管理", "Safety"),
            safety_collapsed,
            pal,
            cx,
        ),
    );
    if !safety_collapsed {
        safety_sec = safety_sec
            .child(muse_icon_row(
                "bot-profile-report",
                IconName::DeleteTrash,
                pal.danger,
                tr_str(lang, "举报此机器人", "Report Bot"),
                None,
                Some(pal.danger),
                None,
                pal,
                move |this, _| {
                    this.pending_action = Some(InspectorAction::ReportBot(bot_id));
                },
                cx,
            ))
            .child(muse_icon_row(
                "bot-profile-block-toggle",
                IconName::Close,
                pal.danger,
                if is_blocked {
                    tr_str(lang, "取消屏蔽并重启机器人", "Restart & Unblock Bot")
                } else {
                    tr_str(lang, "停用并屏蔽机器人", "Stop & Block Bot")
                },
                None,
                Some(pal.danger),
                None,
                pal,
                move |this, _| {
                    let next = !(this.profile.is_blocked || this.is_contact_blocked);
                    this.profile.is_blocked = next;
                    this.is_contact_blocked = next;
                    this.pending_action = Some(InspectorAction::ToggleBlockBot(bot_id));
                },
                cx,
            ));
    }

    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(settings_sec)
        .child(safety_sec)
        .into_any_element()
}

fn render_miniature_desktop_card(
    node: Option<&ComputerNodeDto>,
    title: &str,
    pal: TdesktopThemePalette,
    lang: &str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let live_img = node
        .and_then(|n| n.latest_frame_data_url.as_deref())
        .and_then(decode_data_url_image);
    let node_online = node.is_some_and(|n| n.online);
    let label_text = node
        .map(|n| format!("{} · {}", n.display_name, node_endpoint(n)))
        .unwrap_or_else(|| tr_str(lang, "点击手动打开工作台", "Click to open computer").to_string());

    let is_dark = pal.mode.is_dark();
    let frame_bg = if is_dark { pal.input_bg } else { 0xEFF3F8 };
    let bar_bg = if is_dark { pal.card_bg } else { 0xFFFFFF };
    let bar_sep = if is_dark { 0x38383A } else { 0xE2E8F0 };
    let title_col = if is_dark { pal.text_primary } else { 0x475569 };
    let doc_col = if is_dark { pal.text_secondary } else { 0x64748B };

    div()
        .id("bot-miniature-desktop-card")
        .cursor_pointer()
        .w_full()
        .h(px(158.0))
        .rounded(px(12.0))
        .overflow_hidden()
        .bg(rgb(frame_bg))
        .border_1()
        .border_color(pal.separator_rgba())
        .hover(|s| s.border_color(rgb(pal.accent)))
        .flex()
        .flex_col()
        .justify_between()
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .px(px(10.0))
                .py(px(6.0))
                .bg(rgb(bar_bg))
                .border_b_1()
                .border_color(rgb(bar_sep))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(rgb(0xFF5F56)))
                        .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(rgb(0xFFBD2E)))
                        .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(rgb(0x27C93F))),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(title_col))
                        .truncate()
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .w(px(6.0))
                                .h(px(6.0))
                                .rounded_full()
                                .bg(rgb(if node_online { 0x16A34A } else { 0x94A3B8 })),
                        )
                        .child(
                            div()
                                .text_size(px(9.5))
                                .text_color(rgb(doc_col))
                                .child(if node_online {
                                    tr_str(lang, "在线", "Online")
                                } else {
                                    tr_str(lang, "待机", "Ready")
                                }),
                        ),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .p(px(8.0))
                .child(if let Some(arc_img) = live_img {
                    img(arc_img)
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .rounded(px(6.0))
                        .into_any_element()
                } else {
                    div()
                        .w_full()
                        .h_full()
                        .rounded(px(8.0))
                        .bg(rgb(bar_bg))
                        .border_1()
                        .border_color(rgb(bar_sep))
                        .p(px(8.0))
                        .flex()
                        .flex_col()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(tg_icon(IconName::TerminalMcp, 13.0, pal.accent))
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(if is_dark { pal.text_primary } else { 0x1E293B }))
                                        .truncate()
                                        .child(label_text),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(div().w(px(120.0)).h(px(5.0)).rounded(px(3.0)).bg(rgb(bar_sep)))
                                .child(div().w(px(86.0)).h(px(5.0)).rounded(px(3.0)).bg(rgb(bar_sep))),
                        )
                        .into_any_element()
                }),
        )
        .child(
            div()
                .w_full()
                .pb(px(6.0))
                .flex()
                .justify_center()
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(4.0))
                        .rounded_full()
                        .bg(TdesktopThemePalette::rgba_with_alpha(bar_bg, 0.90))
                        .border_1()
                        .border_color(rgb(bar_sep))
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(tg_icon(IconName::ComputerDesktop, 13.0, pal.accent))
                        .child(tg_icon(IconName::FolderAll, 13.0, 0x3B82F6))
                        .child(tg_icon(IconName::TerminalMcp, 13.0, title_col))
                        .child(tg_icon(IconName::FileDoc, 13.0, doc_col))
                        .child(tg_icon(IconName::SettingsGear, 13.0, title_col)),
                ),
        )
        .into_any_element()
}

pub fn render_bot_computers_subpage(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let bot_id = inspector.profile.bot_id;
    let bound_id = inspector
        .bot_computer_profile
        .as_ref()
        .and_then(|p| p.bound_node_id.as_deref());

    let locals: Vec<&ComputerNodeDto> = inspector
        .computers
        .iter()
        .filter(|c| c.kind == ComputerNodeKind::LocalDevice)
        .collect();
    let clouds: Vec<&ComputerNodeDto> = inspector
        .computers
        .iter()
        .filter(|c| c.kind == ComputerNodeKind::CloudServer)
        .collect();

    let mut local_section = div().flex().flex_col().gap(px(8.0)).child(
        div()
            .text_size(px(12.5))
            .font_weight(FontWeight::BOLD)
            .text_color(rgb(pal.text_primary))
            .child(tr_str(lang, "本地电脑", "Local Computers")),
    );
    if locals.is_empty() {
        local_section = local_section.child(
            div()
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.separator_rgba())
                .text_size(px(12.0))
                .text_color(rgb(pal.text_secondary))
                .child(tr_str(
                    lang,
                    "暂未检测到已登录账号的本地电脑",
                    "No local computers registered on this account yet.",
                )),
        );
    } else {
        for (idx, node) in locals.into_iter().enumerate() {
            let is_bound = bound_id == Some(node.node_id.as_str());
            local_section = local_section.child(render_computer_item_card(
                node,
                is_bound,
                bot_id,
                idx,
                true,
                pal,
                lang,
                cx,
            ));
        }
    }

    let mut cloud_section = div().flex().flex_col().gap(px(8.0)).child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(pal.text_primary))
                    .child(tr_str(lang, "云电脑", "Cloud Computers")),
            )
            .child(
                div()
                    .id("bot-create-cloud-btn")
                    .cursor_pointer()
                    .px(px(9.0))
                    .py(px(4.0))
                    .rounded(px(999.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                    .hover(|s| s.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.25)))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.pending_action =
                                Some(InspectorAction::OpenCreateCloudComputerModal {
                                    bind_bot_id: (bot_id > 0).then_some(bot_id),
                                });
                            cx.notify();
                        }),
                    )
                    .child(tr_str(lang, "+ 创建", "+ Create")),
            ),
    );

    if clouds.is_empty() {
        cloud_section = cloud_section.child(
            div()
                .w_full()
                .p(px(18.0))
                .rounded(px(12.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.separator_rgba())
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .w(px(40.0))
                        .h(px(40.0))
                        .rounded(px(12.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(tg_icon(IconName::ComputerDesktop, 20.0, pal.accent)),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .text_center()
                        .child(tr_str(
                            lang,
                            "每个 Agent 都拥有自己的电脑",
                            "Every Agent gets its own computer",
                        )),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(pal.text_secondary))
                        .text_center()
                        .child(tr_str(
                            lang,
                            "只有在你的 Agent 需要保持 24/7 运行时，才添加一台常驻云电脑。",
                            "Add an always-on cloud computer only when your Agent needs 24/7 execution.",
                        )),
                ),
        );
    } else {
        for (idx, node) in clouds.into_iter().enumerate() {
            let is_bound = bound_id == Some(node.node_id.as_str());
            cloud_section = cloud_section.child(render_computer_item_card(
                node,
                is_bound,
                bot_id,
                idx,
                false,
                pal,
                lang,
                cx,
            ));
        }
    }

    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(local_section)
        .child(cloud_section)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_computer_item_card(
    node: &ComputerNodeDto,
    is_bound: bool,
    bot_id: u64,
    idx: usize,
    is_local: bool,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let node_id_bind = node.node_id.clone();
    let node_id_open = node.node_id.clone();
    let node_id_detail = node.node_id.clone();
    let badge_text = if is_local {
        if node.is_current_device {
            tr_str(lang, "当前设备", "Current Device").to_string()
        } else if node.online {
            tr_str(lang, "在线 · 同账号登录设备", "Online · Same Account").to_string()
        } else {
            tr_str(lang, "离线", "Offline").to_string()
        }
    } else if node.online {
        tr_str(lang, "在线", "Online").to_string()
    } else {
        tr_str(lang, "离线", "Offline").to_string()
    };

    let endpoint = node_endpoint(node);
    let subtitle = if is_local {
        endpoint.to_string()
    } else {
        format!("{}:{}", endpoint, node.ssh_port.unwrap_or(22))
    };

    div()
        .w_full()
        .p(px(12.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(if is_bound {
            rgb(pal.accent).into()
        } else {
            pal.separator_rgba()
        })
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .min_w(px(0.0))
                        .flex_1()
                        .child(
                            div()
                                .w(px(32.0))
                                .h(px(32.0))
                                .rounded(px(9.0))
                                .bg(rgb(pal.input_bg))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(tg_icon(IconName::ComputerDesktop, 16.0, pal.accent)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w(px(0.0))
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .truncate()
                                        .child(node.display_name.clone()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.0))
                                        .child(
                                            div()
                                                .w(px(6.0))
                                                .h(px(6.0))
                                                .rounded_full()
                                                .bg(rgb(if node.online {
                                                    pal.success
                                                } else {
                                                    pal.text_secondary
                                                })),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(rgb(pal.text_secondary))
                                                .truncate()
                                                .child(format!("{subtitle} · {badge_text}")),
                                        ),
                                ),
                        ),
                )
                .child(
                    div()
                        .id((if is_local { "local-detail" } else { "cloud-detail" }, idx))
                        .cursor_pointer()
                        .w(px(26.0))
                        .h(px(26.0))
                        .rounded_full()
                        .hover(|s| s.bg(rgb(pal.dialogs_hover_bg)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.pending_action =
                                    Some(InspectorAction::OpenComputerDetailInSettings {
                                        node_id: node_id_detail.clone(),
                                    });
                                cx.notify();
                            }),
                        )
                        .child(tg_icon(IconName::ChevronRight, 13.0, pal.icon_fg)),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .id((if is_local { "local-bind" } else { "cloud-bind" }, idx))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(7.0))
                        .bg(if is_bound {
                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                        } else {
                            rgb(pal.input_bg).into()
                        })
                        .border_1()
                        .border_color(rgb(if is_bound { pal.accent } else { pal.border_fg }))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if is_bound {
                            pal.accent
                        } else {
                            pal.text_primary
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                let target = if is_bound {
                                    None
                                } else {
                                    Some(node_id_bind.clone())
                                };
                                this.pending_action = Some(InspectorAction::BindBotToComputer {
                                    bot_id,
                                    node_id: target,
                                });
                                cx.notify();
                            }),
                        )
                        .child(if is_bound {
                            tr_str(lang, "已设为默认", "Default")
                        } else if is_local {
                            tr_str(lang, "设为默认", "Set Default")
                        } else {
                            tr_str(lang, "绑定此 Bot", "Bind Bot")
                        }),
                )
                .child(
                    div()
                        .id((if is_local { "local-control" } else { "cloud-control" }, idx))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(7.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(rgb(pal.border_fg))
                        .hover(|s| s.bg(rgb(pal.dialogs_hover_bg)))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.pending_action = Some(InspectorAction::OpenRemoteWorkbench {
                                    node_id: node_id_open.clone(),
                                });
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "远程操控", "Remote Control")),
                ),
        )
        .into_any_element()
}

pub fn render_bot_config_editor(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let bot_id = inspector.profile.bot_id;
    let bound_id = inspector
        .bot_computer_profile
        .as_ref()
        .and_then(|p| p.bound_node_id.as_deref());

    let (persona_text, persona_preedit) = inspector
        .persona_input
        .as_ref()
        .map(|c| c.read(cx).text_and_preedit(inspector.bot_draft_persona.clone()))
        .unwrap_or_else(|| (inspector.bot_draft_persona.clone(), None));
    let (model_text, model_preedit) = inspector
        .model_input
        .as_ref()
        .map(|c| c.read(cx).text_and_preedit(inspector.bot_draft_model.clone()))
        .unwrap_or_else(|| (inspector.bot_draft_model.clone(), None));

    let model_presets = ["gpt-5.4", "claude-sonnet-4.6", "gemini-2.5-pro", "deepseek-reasoner"];
    let policy_choices = [
        ComputerRunPolicy::AskEveryTime,
        ComputerRunPolicy::AllowReadOnly,
        ComputerRunPolicy::AlwaysAllowAuthorized,
        ComputerRunPolicy::Disabled,
    ];

    let mut target_chips = div().flex().flex_wrap().gap(px(6.0)).child(
        div()
            .id("bot-cfg-bind-none")
            .cursor_pointer()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(999.0))
            .bg(if bound_id.is_none() {
                TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
            } else {
                rgb(pal.input_bg).into()
            })
            .border_1()
            .border_color(rgb(if bound_id.is_none() {
                pal.accent
            } else {
                pal.border_fg
            }))
            .text_size(px(11.0))
            .text_color(rgb(if bound_id.is_none() {
                pal.accent
            } else {
                pal.text_primary
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.pending_action = Some(InspectorAction::BindBotToComputer {
                        bot_id,
                        node_id: None,
                    });
                    cx.notify();
                }),
            )
            .child(tr_str(lang, "跟随当前设备", "Follow Current")),
    );
    for (idx, node) in inspector.computers.iter().enumerate() {
        let selected = bound_id == Some(node.node_id.as_str());
        let nid = node.node_id.clone();
        target_chips = target_chips.child(
            div()
                .id(("bot-cfg-bind-node", idx))
                .cursor_pointer()
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(999.0))
                .bg(if selected {
                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                } else {
                    rgb(pal.input_bg).into()
                })
                .border_1()
                .border_color(rgb(if selected { pal.accent } else { pal.border_fg }))
                .text_size(px(11.0))
                .text_color(rgb(if selected {
                    pal.accent
                } else {
                    pal.text_primary
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.pending_action = Some(InspectorAction::BindBotToComputer {
                            bot_id,
                            node_id: Some(nid.clone()),
                        });
                        cx.notify();
                    }),
                )
                .child(node.display_name.clone()),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.separator_rgba())
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(lang, "绑定默认运行电脑", "Bound Computer")),
                )
                .child(target_chips),
        )
        .child(
            div()
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.separator_rgba())
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(lang, "系统角色提示词", "System Persona Prompt")),
                )
                .child(
                    div()
                        .id("bot-persona-input-box")
                        .cursor_text()
                        .min_h(px(74.0))
                        .p(px(8.0))
                        .rounded(px(10.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(
                            if inspector.bot_config_focus == BotConfigFocusField::Persona {
                                rgb(pal.accent).into()
                            } else {
                                pal.separator_rgba()
                            },
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.ensure_bot_inputs(cx);
                                this.bot_config_focus = BotConfigFocusField::Persona;
                                cx.notify();
                            }),
                        )
                        .child(render_ime_text_preview(
                            &persona_text,
                            persona_preedit.as_deref(),
                            tr_str(
                                lang,
                                "描述此 Bot 的专长、语气与安全边界...",
                                "Describe persona, skills, and safety rules...",
                            ),
                            inspector.bot_config_focus == BotConfigFocusField::Persona,
                            false,
                            pal.text_primary,
                            pal,
                        )),
                ),
        )
        .child(
            div()
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.separator_rgba())
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(lang, "首选模型", "Preferred Model")),
                )
                .child(
                    div()
                        .id("bot-model-input-box")
                        .cursor_text()
                        .min_h(px(34.0))
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(10.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(
                            if inspector.bot_config_focus == BotConfigFocusField::Model {
                                rgb(pal.accent).into()
                            } else {
                                pal.separator_rgba()
                            },
                        )
                        .flex()
                        .items_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.ensure_bot_inputs(cx);
                                this.bot_config_focus = BotConfigFocusField::Model;
                                cx.notify();
                            }),
                        )
                        .child(render_ime_text_preview(
                            &model_text,
                            model_preedit.as_deref(),
                            "gpt-5.4 / claude-sonnet-4.6",
                            inspector.bot_config_focus == BotConfigFocusField::Model,
                            false,
                            pal.text_primary,
                            pal,
                        )),
                )
                .child(
                    div().flex().flex_wrap().gap(px(5.0)).children(
                        model_presets.into_iter().enumerate().map(|(i, preset)| {
                            div()
                                .id(("bot-model-preset", i))
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .bg(rgb(pal.input_bg))
                                .hover(|s| s.bg(rgb(pal.dialogs_hover_bg)))
                                .text_size(px(10.5))
                                .text_color(rgb(pal.text_secondary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.bot_draft_model = preset.to_string();
                                        cx.notify();
                                    }),
                                )
                                .child(preset)
                        }),
                    ),
                ),
        )
        .child(
            div()
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.separator_rgba())
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(lang, "默认执行审批策略", "Execution Approval Policy")),
                )
                .child(
                    div().flex().flex_wrap().gap(px(6.0)).children(
                        policy_choices.into_iter().enumerate().map(|(idx, pol)| {
                            let active = inspector.bot_draft_approval_mode == pol;
                            div()
                                .id(("bot-cfg-policy", idx))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(5.0))
                                .rounded(px(8.0))
                                .bg(if active {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                                } else {
                                    rgb(pal.input_bg).into()
                                })
                                .border_1()
                                .border_color(if active {
                                    rgb(pal.accent).into()
                                } else {
                                    pal.separator_rgba()
                                })
                                .text_size(px(11.0))
                                .font_weight(if active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(rgb(if active {
                                    pal.accent
                                } else {
                                    pal.text_primary
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.bot_draft_approval_mode = pol;
                                        cx.notify();
                                    }),
                                )
                                .child(format_policy_label(pol, lang))
                        }),
                    ),
                ),
        )
        .child(
            div()
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.separator_rgba())
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(bot_toggle_row(
                    "bot-toggle-cross-device",
                    tr_str(lang, "允许跨设备协同控制", "Allow Cross-Device Control"),
                    inspector.bot_draft_allow_cross_device,
                    pal,
                    cx.listener(|this, _, _, cx| {
                        this.bot_draft_allow_cross_device = !this.bot_draft_allow_cross_device;
                        cx.notify();
                    }),
                ))
                .child(bot_toggle_row(
                    "bot-toggle-cloud-self-cfg",
                    tr_str(lang, "允许 Bot 自主配置云电脑", "Allow Cloud Self-Configuration"),
                    inspector.bot_draft_allow_cloud_self_config,
                    pal,
                    cx.listener(|this, _, _, cx| {
                        this.bot_draft_allow_cloud_self_config =
                            !this.bot_draft_allow_cloud_self_config;
                        cx.notify();
                    }),
                )),
        )
        .child(
            div()
                .id("bot-save-config-btn")
                .cursor_pointer()
                .w_full()
                .h(px(36.0))
                .rounded(px(9.0))
                .bg(rgb(pal.accent))
                .hover(|s| s.bg(rgb(pal.accent_hover)))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.active_button_fg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.bot_config_focus = BotConfigFocusField::None;
                        this.pending_action = Some(InspectorAction::SaveBotComputerConfig {
                            bot_id,
                            persona_prompt: this.bot_draft_persona.clone(),
                            preferred_model: this.bot_draft_model.clone(),
                            approval_mode: this.bot_draft_approval_mode,
                            allow_cross_device_control: this.bot_draft_allow_cross_device,
                            allow_cloud_self_config: this.bot_draft_allow_cloud_self_config,
                        });
                        cx.notify();
                    }),
                )
                .child(tr_str(lang, "保存 AI Bot 配置", "Save AI Bot Config")),
        )
        .into_any_element()
}

fn config_summary_row(label: &str, value: &str, pal: TdesktopThemePalette) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(pal.text_secondary))
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(pal.text_primary))
                .truncate()
                .child(value.to_string()),
        )
}

fn bot_toggle_row(
    id: &'static str,
    label: &str,
    enabled: bool,
    pal: TdesktopThemePalette,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .py(px(4.0))
        .flex()
        .items_center()
        .justify_between()
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(pal.text_primary))
                .child(label.to_string()),
        )
        .child(apple_switch_pill(enabled, pal))
}

fn render_coding_agent_inspector_card(
    _fab: &FabushiBotSummaryDto,
    coding_cfg: &CodingAgentBotConfig,
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let provider_label = coding_cfg.provider_kind.display_name();
    let supports_acp = coding_cfg.provider_kind.supports_acp();
    let (ws_text, ws_preedit) = inspector
        .coding_workspace_input
        .as_ref()
        .map(|c| {
            c.read(cx)
                .text_and_preedit(inspector.coding_draft_workspace.clone())
        })
        .unwrap_or_else(|| (inspector.coding_draft_workspace.clone(), None));
    let (model_text, model_preedit) = inspector
        .coding_model_input
        .as_ref()
        .map(|c| {
            c.read(cx)
                .text_and_preedit(inspector.coding_draft_model_override.clone())
        })
        .unwrap_or_else(|| (inspector.coding_draft_model_override.clone(), None));

    let perm_choices = [
        CodingPermissionMode::Ask,
        CodingPermissionMode::AutoEdit,
        CodingPermissionMode::FullAccess,
    ];
    let perm_row = div().flex().flex_wrap().gap(px(5.0)).children(
        perm_choices.into_iter().enumerate().map(|(idx, mode)| {
            let selected = inspector.coding_draft_permission_mode == mode;
            let label = if lang.starts_with("zh") {
                mode.label_zh()
            } else {
                mode.label_en()
            };
            div()
                .id(SharedString::from(format!("coding-insp-perm-{idx}")))
                .cursor_pointer()
                .px(px(7.0))
                .py(px(3.0))
                .rounded(px(999.0))
                .bg(if selected {
                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                } else {
                    rgb(pal.input_bg).into()
                })
                .border_1()
                .border_color(rgb(if selected { pal.accent } else { pal.border_fg }))
                .text_size(px(10.5))
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(rgb(if selected {
                    pal.accent
                } else {
                    pal.text_primary
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.coding_draft_permission_mode = mode;
                        cx.notify();
                    }),
                )
                .child(label)
        }),
    );

    div()
        .id("coding-agent-inspector-card")
        .w_full()
        .p(px(12.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.separator_rgba())
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(lang, "Coding Agent 配置", "Coding Agent Config")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .px(px(7.0))
                                .py(px(2.0))
                                .rounded(px(999.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                .text_size(px(10.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.accent))
                                .child(provider_label),
                        )
                        .when(supports_acp, |row| {
                            row.child(
                                div()
                                    .px(px(5.0))
                                    .py(px(2.0))
                                    .rounded(px(999.0))
                                    .bg(rgb(pal.input_bg))
                                    .text_size(px(9.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_secondary))
                                    .child("ACP"),
                            )
                        }),
                ),
        )
        .when_some(coding_cfg.bound_device_id.clone(), |col, dev_id| {
            col.child(config_summary_row(
                tr_str(lang, "绑定主机", "Bound Host"),
                &dev_id,
                pal,
            ))
        })
        .child(
            div()
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_secondary))
                .child(tr_str(lang, "工作区路径", "Workspace Path")),
        )
        .child(
            div()
                .id("coding-insp-workspace-box")
                .cursor_text()
                .min_h(px(32.0))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(
                    if inspector.bot_config_focus == BotConfigFocusField::CodingWorkspace {
                        rgb(pal.accent).into()
                    } else {
                        pal.separator_rgba()
                    },
                )
                .flex()
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.ensure_bot_inputs(cx);
                        this.bot_config_focus = BotConfigFocusField::CodingWorkspace;
                        cx.notify();
                    }),
                )
                .child(render_ime_text_preview(
                    &ws_text,
                    ws_preedit.as_deref(),
                    "/path/to/repo",
                    inspector.bot_config_focus == BotConfigFocusField::CodingWorkspace,
                    false,
                    pal.text_primary,
                    pal,
                )),
        )
        .child(bot_toggle_row(
            "coding-insp-worktree-toggle",
            tr_str(lang, "Git Worktree 隔离", "Git Worktree Isolation"),
            inspector.coding_draft_use_worktree,
            pal,
            cx.listener(|this, _, _, cx| {
                this.coding_draft_use_worktree = !this.coding_draft_use_worktree;
                cx.notify();
            }),
        ))
        .child(
            div()
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_secondary))
                .child(tr_str(lang, "权限模式", "Permission Mode")),
        )
        .child(perm_row)
        .child(bot_toggle_row(
            "coding-insp-group-toggle",
            tr_str(lang, "允许群成员触发", "Allow Group Members"),
            inspector.coding_draft_allow_group_members,
            pal,
            cx.listener(|this, _, _, cx| {
                this.coding_draft_allow_group_members = !this.coding_draft_allow_group_members;
                cx.notify();
            }),
        ))
        .child(
            div()
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_secondary))
                .child(tr_str(lang, "模型覆盖（可选）", "Model Override (optional)")),
        )
        .child(
            div()
                .id("coding-insp-model-box")
                .cursor_text()
                .min_h(px(32.0))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(
                    if inspector.bot_config_focus == BotConfigFocusField::CodingModel {
                        rgb(pal.accent).into()
                    } else {
                        pal.separator_rgba()
                    },
                )
                .flex()
                .items_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.ensure_bot_inputs(cx);
                        this.bot_config_focus = BotConfigFocusField::CodingModel;
                        cx.notify();
                    }),
                )
                .child(render_ime_text_preview(
                    &model_text,
                    model_preedit.as_deref(),
                    tr_str(lang, "默认模型", "Provider default"),
                    inspector.bot_config_focus == BotConfigFocusField::CodingModel,
                    false,
                    pal.text_primary,
                    pal,
                )),
        )
        .child(
            div()
                .id("coding-insp-save-btn")
                .cursor_pointer()
                .w_full()
                .h(px(34.0))
                .rounded(px(9.0))
                .bg(rgb(pal.accent))
                .hover(|s| s.bg(rgb(pal.accent_hover)))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.active_button_fg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.save_coding_agent_config_action();
                        cx.notify();
                    }),
                )
                .child(tr_str(lang, "保存 Coding Agent 配置", "Save Coding Agent Config")),
        )
        .into_any_element()
}
