use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::miniapp_contracts::{FabushiBotSummaryDto, PatchFabushiBotRequest};
use gpui_telegram_proto::{
    BotComputerProfileDto, CodingAgentBotConfig, CodingPermissionMode, ComputerNodeDto,
    ComputerNodeKind, ComputerRunPolicy,
};

use crate::components::avatar::render_plush_bot_avatar;
use crate::components::bloub_avatar::{AgentAvatarProps, render_agent_avatar};
use crate::components::grok_bot_avatar::trigger_grok_avatar_bounce;
use crate::i18n::tr_str;
use crate::theme::assets::{IconName, tg_icon, tg_icon_rgba};
use crate::theme::openbot_palette::{
    CHAT_COLOR_PRESETS, OpenBotPalette, parse_css_color_to_rgba,
};
use crate::theme::palette::TdesktopThemePalette;
use crate::views::computer_workbench::{decode_data_url_image, render_ime_text_preview};
use crate::views::drawer::inspector_panels::{
    apple_switch_pill, muse_collapsible_header, muse_icon_row,
};
use crate::views::drawer::inspector_tabs::{
    BotCompanionTab, BotConfigFocusField, InspectorAction, InspectorAiModelCatalog,
    InspectorSubpage, OpenBotConfirmGuard, OpenBotInspectorAction, OpenBotInspectorTextField,
    OpenBotSettingsSubpage,
};
use crate::views::openbot::side_panel::SidePanelPaneKind;
use crate::views::stage::openbot_stage_chrome::{
    activity_str_to_openbot_mood, normalize_fabushi_plush_spec,
};
use crate::views::ContextInspector;
use openbot_contracts::avatar::{
    AVATAR_COLOR_CHOICES, AvatarColorChoice, AvatarMood, AvatarStyle, BloubExpression, BloubShape,
    avatar_seed_for_face,
};

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

/// Formats the effective model summary displayed in the Bot Profile panel,
/// staying consistent with live Settings AI provider state.
pub fn format_bot_effective_model_label(inspector: &ContextInspector, lang: &str) -> String {
    if let Some(override_model) = inspector.effective_bot_preferred_model() {
        if inspector.ai_models.synced
            && !inspector
                .ai_models
                .is_model_consistent_with_settings(&override_model)
        {
            if lang.starts_with("zh") {
                return format!("{override_model} (与设置不一致)");
            }
            return format!("{override_model} (differs from Settings)");
        }
        return override_model;
    }

    if !inspector.ai_models.needs_provider_selection && !inspector.ai_models.active_model.is_empty()
    {
        if lang.starts_with("zh") {
            format!("跟随设置 · {}", inspector.ai_models.active_model)
        } else {
            format!("Settings · {}", inspector.ai_models.active_model)
        }
    } else if inspector.ai_models.synced && inspector.ai_models.needs_provider_selection {
        tr_str(lang, "未配置模型", "No Model Configured").to_string()
    } else {
        tr_str(lang, "跟随设置默认", "Follow Settings").to_string()
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
                move |this, cx| {
                    this.show_copied_feedback(format!("{copied_prefix}: {cp_fb}"), cx);
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

/// Interactive OpenBot `284px` Avatar Editor popover toggled from the hero avatar's pencil badge:
/// Photo upload/remove, Plush/Flat style tabs + Reset + Shuffle, 5-column grid of all 13
/// `BloubShape` silhouettes, 4-column grid of 16 `BloubExpression` eyes, and 9×2 grid of 18 colors.
pub fn render_bot_avatar_editor_popover(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let Some(fab) = inspector.active_fabushi_bot.clone() else {
        return div().into_any_element();
    };
    let ob = pal.openbot_palette();
    let is_dark = pal.mode.is_dark();
    let target_bot_id = fab.bot_user_id.max(0) as u64;
    let (cur_shape_opt, cur_hue, cur_color_hex) =
        normalize_fabushi_plush_spec(&fab.avatar_shape, &fab.avatar_color);
    let cur_shape = cur_shape_opt.unwrap_or(BloubShape::Cercle);
    let cur_expr = inspector.bot_avatar_expression;
    let is_flat = inspector.bot_avatar_style_flat;
    let cur_style = if is_flat {
        AvatarStyle::Flat
    } else {
        AvatarStyle::Plush
    };
    let seed_base = if fab.agent_id.trim().is_empty() {
        format!("bot-{target_bot_id}")
    } else {
        fab.agent_id.clone()
    };

    let make_preview = |shape: BloubShape, expr: BloubExpression, sz: f32| {
        let seed = avatar_seed_for_face(&seed_base, shape, expr, cur_style)
            .unwrap_or_else(|_| format!("plush:{}", shape.as_str()));
        let mut props = AgentAvatarProps::new(seed)
            .with_size(sz)
            .with_mood(AvatarMood::Idle);
        props.shape_override = Some(shape);
        props.expression_override = Some(expr);
        props.hue = cur_hue;
        props.color = cur_color_hex.clone();
        props.dark_scheme = is_dark;
        render_agent_avatar(&props, None, 0.0)
    };

    let has_custom_photo = inspector.bot_avatar_custom_photo.is_some();
    let upload_photo_preview = inspector.bot_avatar_custom_photo.clone();
    let photo_section = div()
        .w_full()
        .px(px(10.0))
        .py(px(10.0))
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
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_secondary())
                        .child(tr_str(lang, "头像图片", "IMAGE")),
                )
                .when(has_custom_photo, |row| {
                    row.child(
                        div()
                            .id("bot-avatar-remove-photo")
                            .cursor_pointer()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .hover(|s| s.bg(ob.hover()))
                            .text_size(px(11.0))
                            .text_color(ob.danger())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.bot_avatar_custom_photo = None;
                                    this.profile.avatar_path = None;
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "移除", "Remove")),
                    )
                }),
        )
        .child(
            div()
                .id("bot-avatar-upload-photo")
                .cursor_pointer()
                .w_full()
                .min_h(px(58.0))
                .px(px(10.0))
                .py(px(8.0))
                .rounded(px(9.0))
                .bg(ob.bg_control())
                .border_1()
                .border_color(ob.border_strong_hairline())
                .hover(|s| s.bg(ob.bg_control_hover()))
                .flex()
                .items_center()
                .gap(px(10.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.pending_action = Some(InspectorAction::OpenEditPeer);
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .w(px(38.0))
                        .h(px(38.0))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(ob.bg_canvas())
                        .border_1()
                        .border_color(ob.border_hairline())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if let Some(path) = upload_photo_preview {
                            img(path)
                                .w(px(38.0))
                                .h(px(38.0))
                                .object_fit(ObjectFit::Cover)
                                .into_any_element()
                        } else {
                            make_preview(cur_shape, cur_expr, 34.0).into_any_element()
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
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_primary())
                                .child(if has_custom_photo {
                                    tr_str(lang, "替换头像图片", "Replace image")
                                } else {
                                    tr_str(lang, "上传自定义图片", "Upload image")
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(ob.text_muted())
                                .child("PNG / JPEG / WebP · 1:1"),
                        ),
                ),
        );

    let agent_reset = seed_base.clone();
    let agent_shuffle = seed_base.clone();
    let shape_header = div()
        .w_full()
        .px(px(10.0))
        .pt(px(8.0))
        .pb(px(4.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .p(px(2.0))
                .rounded(px(7.0))
                .bg(ob.bg_canvas())
                .border_1()
                .border_color(ob.border_hairline())
                .child(
                    div()
                        .id("bot-avatar-style-plush")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(5.0))
                        .bg(if !is_flat {
                            ob.bg_control_active()
                        } else {
                            Rgba::default()
                        })
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if !is_flat {
                            ob.text_primary()
                        } else {
                            ob.text_muted()
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.bot_avatar_style_flat = false;
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "毛绒", "Plush")),
                )
                .child(
                    div()
                        .id("bot-avatar-style-flat")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(5.0))
                        .bg(if is_flat {
                            ob.bg_control_active()
                        } else {
                            Rgba::default()
                        })
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if is_flat {
                            ob.text_primary()
                        } else {
                            ob.text_muted()
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.bot_avatar_style_flat = true;
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "扁平", "Flat")),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .id("bot-avatar-reset-btn")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(ob.bg_control())
                        .hover(|s| s.bg(ob.bg_control_hover()))
                        .text_size(px(11.0))
                        .text_color(ob.text_secondary())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                trigger_grok_avatar_bounce(&agent_reset);
                                this.bot_avatar_style_flat = false;
                                this.bot_avatar_expression = BloubExpression::Neutre;
                                this.bot_avatar_custom_photo = None;
                                this.pending_action = Some(InspectorAction::PatchFabushiBot {
                                    bot_id: target_bot_id,
                                    req: PatchFabushiBotRequest {
                                        avatar_shape: Some("plush:cercle".to_string()),
                                        avatar_color: Some(String::new()),
                                        ..Default::default()
                                    },
                                });
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "重置", "Reset")),
                )
                .child(
                    div()
                        .id("bot-avatar-shuffle-btn")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(ob.bg_control())
                        .hover(|s| s.bg(ob.bg_control_hover()))
                        .text_size(px(11.0))
                        .text_color(ob.text_secondary())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                trigger_grok_avatar_bounce(&agent_shuffle);
                                let tick = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.subsec_nanos() as usize)
                                    .unwrap_or(1);
                                let next_shape = BloubShape::ALL[tick % BloubShape::ALL.len()];
                                let next_expr =
                                    BloubExpression::ALL[(tick / 7) % BloubExpression::ALL.len()];
                                let next_choice = AVATAR_COLOR_CHOICES
                                    [(tick / 13) % AVATAR_COLOR_CHOICES.len()];
                                let color_val = match next_choice {
                                    AvatarColorChoice::Hue { hue, .. } => {
                                        hue.swatch().to_string()
                                    }
                                    AvatarColorChoice::Preset { color, .. } => color.to_string(),
                                };
                                this.bot_avatar_expression = next_expr;
                                let prefix = if this.bot_avatar_style_flat {
                                    "bloub"
                                } else {
                                    "plush"
                                };
                                this.pending_action = Some(InspectorAction::PatchFabushiBot {
                                    bot_id: target_bot_id,
                                    req: PatchFabushiBotRequest {
                                        avatar_shape: Some(format!(
                                            "{prefix}:{}",
                                            next_shape.as_str()
                                        )),
                                        avatar_color: Some(color_val),
                                        ..Default::default()
                                    },
                                });
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "随机", "Shuffle")),
                ),
        );

    // 5-column grid of all 13 BloubShape silhouettes
    let mut shape_grid = div()
        .w_full()
        .px(px(10.0))
        .pb(px(10.0))
        .flex()
        .flex_col()
        .gap(px(4.0));
    for (row_idx, chunk) in BloubShape::ALL.chunks(5).enumerate() {
        let mut row = div().w_full().flex().items_center().gap(px(4.0));
        for (col_idx, &shape) in chunk.iter().enumerate() {
            let idx = row_idx * 5 + col_idx;
            let selected = cur_shape == shape;
            let agent_bounce = seed_base.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("fabushi-insp-plush-{idx}")))
                    .flex_1()
                    .h(px(44.0))
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
                            trigger_grok_avatar_bounce(&agent_bounce);
                            let prefix = if this.bot_avatar_style_flat {
                                "bloub"
                            } else {
                                "plush"
                            };
                            this.pending_action = Some(InspectorAction::PatchFabushiBot {
                                bot_id: target_bot_id,
                                req: PatchFabushiBotRequest {
                                    avatar_shape: Some(format!("{prefix}:{}", shape.as_str())),
                                    ..Default::default()
                                },
                            });
                            cx.notify();
                        }),
                    )
                    .child(make_preview(shape, cur_expr, 30.0)),
            );
        }
        for _ in chunk.len()..5 {
            row = row.child(div().flex_1().h(px(44.0)));
        }
        shape_grid = shape_grid.child(row);
    }

    // 4-column grid of all 16 BloubExpression eyes/expressions
    let mut eyes_grid = div()
        .w_full()
        .px(px(10.0))
        .pb(px(10.0))
        .flex()
        .flex_col()
        .gap(px(4.0));
    for (row_idx, chunk) in BloubExpression::ALL.chunks(4).enumerate() {
        let mut row = div().w_full().flex().items_center().gap(px(4.0));
        for (col_idx, &expr) in chunk.iter().enumerate() {
            let idx = row_idx * 4 + col_idx;
            let selected = cur_expr == expr;
            row = row.child(
                div()
                    .id(SharedString::from(format!("fabushi-insp-eyes-{idx}")))
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
                            this.bot_avatar_expression = expr;
                            cx.notify();
                        }),
                    )
                    .child(make_preview(cur_shape, expr, 32.0)),
            );
        }
        eyes_grid = eyes_grid.child(row);
    }

    // 9-column × 2-row grid of 18 colors (Auto 'A' + 17 AVATAR_COLOR_CHOICES)
    let auto_selected = cur_hue.is_none() && cur_color_hex.is_none();
    let mut color_cells: Vec<AnyElement> = Vec::with_capacity(18);
    color_cells.push(
        div()
            .id("fabushi-insp-color-auto")
            .flex_1()
            .h(px(30.0))
            .cursor_pointer()
            .rounded(px(6.0))
            .bg(if auto_selected {
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
                    this.pending_action = Some(InspectorAction::PatchFabushiBot {
                        bot_id: target_bot_id,
                        req: PatchFabushiBotRequest {
                            avatar_color: Some(String::new()),
                            ..Default::default()
                        },
                    });
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(18.0))
                    .h(px(18.0))
                    .rounded_full()
                    .border_1()
                    .border_color(if auto_selected {
                        ob.text_primary()
                    } else {
                        ob.text_muted()
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ob.text_secondary())
                    .child("A"),
            )
            .into_any_element(),
    );
    for (idx, choice) in AVATAR_COLOR_CHOICES.iter().copied().enumerate() {
        let swatch_hex = match choice {
            AvatarColorChoice::Hue { swatch, .. } => swatch,
            AvatarColorChoice::Preset { swatch, .. } => swatch,
        };
        let is_selected = openbot_contracts::avatar::is_avatar_color_choice(
            choice,
            cur_hue,
            cur_color_hex.as_deref(),
        );
        let swatch_rgba =
            parse_css_color_to_rgba(swatch_hex).unwrap_or_else(|| rgb(0x4699FE));
        let patch_val = swatch_hex.to_string();
        color_cells.push(
            div()
                .id(SharedString::from(format!("fabushi-insp-color-{idx}")))
                .flex_1()
                .h(px(30.0))
                .cursor_pointer()
                .rounded(px(6.0))
                .bg(if is_selected {
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
                        this.pending_action = Some(InspectorAction::PatchFabushiBot {
                            bot_id: target_bot_id,
                            req: PatchFabushiBotRequest {
                                avatar_color: Some(patch_val.clone()),
                                ..Default::default()
                            },
                        });
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .w(px(18.0))
                        .h(px(18.0))
                        .rounded_full()
                        .bg(swatch_rgba)
                        .border_1()
                        .border_color(if is_selected {
                            ob.text_primary()
                        } else {
                            ob.border_strong_hairline()
                        }),
                )
                .into_any_element(),
        );
    }

    let mut color_row_1 = div().w_full().flex().items_center().gap(px(2.0));
    let mut color_row_2 = div().w_full().flex().items_center().gap(px(2.0));
    for (i, cell) in color_cells.into_iter().enumerate() {
        if i < 9 {
            color_row_1 = color_row_1.child(cell);
        } else {
            color_row_2 = color_row_2.child(cell);
        }
    }

    div()
        .id("bot-avatar-editor-popover")
        .mt(px(11.0))
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
                        .child(tr_str(lang, "自定义形象", "Customize Avatar")),
                )
                .child(
                    div()
                        .id("bot-avatar-editor-done")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(2.0))
                        .rounded(px(5.0))
                        .hover(|s| s.bg(ob.hover()))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.text_secondary())
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
        .child(photo_section)
        .child(div().w_full().h(px(1.0)).bg(ob.border_hairline()))
        .child(shape_header)
        .child(shape_grid)
        .child(div().w_full().h(px(1.0)).bg(ob.border_hairline()))
        .child(
            div()
                .px(px(10.0))
                .pt(px(8.0))
                .pb(px(4.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_secondary())
                .child(tr_str(lang, "表情", "EYES")),
        )
        .child(eyes_grid)
        .child(div().w_full().h(px(1.0)).bg(ob.border_hairline()))
        .child(
            div()
                .px(px(10.0))
                .pt(px(8.0))
                .pb(px(4.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_secondary())
                .child(tr_str(lang, "颜色", "COLOR")),
        )
        .child(
            div()
                .w_full()
                .px(px(10.0))
                .pb(px(10.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(color_row_1)
                .child(color_row_2),
        )
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
    let model_name = format_bot_effective_model_label(inspector, lang);
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

        out = out.child(render_bot_fusion_inspector_sections(
            inspector, pal, lang, cx,
        ));
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

const QUICK_BOT_SKILLS: [(&str, &str, &str, &str); 4] = [
    ("code-review", "代码审查", "Code Review", "自动审查 PR 变更与安全隐患"),
    ("web-research", "深度检索", "Web Research", "多来源交叉验证并输出结构化摘要"),
    ("daily-digest", "每日简报", "Daily Digest", "汇总当天重点议题与行动项"),
    ("release-notes", "发布说明", "Release Notes", "整理变更日志并生成发布摘要"),
];

/// Native OpenBot + Orca capabilities fused directly inside the Bot Inspector (`记忆 / 技能 / 定时任务 / 工作流`).
fn render_bot_fusion_inspector_sections(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let is_zh = lang.starts_with("zh");
    let snap = &inspector.bot_fusion;
    let mem_collapsed = inspector.is_section_collapsed("bot-sec-memory");
    let skills_collapsed = inspector.is_section_collapsed("bot-sec-skills");
    let routines_collapsed = inspector.is_section_collapsed("bot-sec-routines");
    let wf_collapsed = inspector.is_section_collapsed("bot-sec-workflows");

    let (mem_text, mem_preedit) = if let Some(ref ctrl) = inspector.memory_input {
        ctrl.read(cx).text_and_preedit(inspector.memory_draft.clone())
    } else {
        (inspector.memory_draft.clone(), None)
    };
    let mem_focused = inspector.bot_config_focus == BotConfigFocusField::MemoryDraft;

    // 1. Memory section
    let mem_title = format!(
        "{} ({})",
        tr_str(lang, "记忆与知识", "Memory & Knowledge"),
        snap.memories.len()
    );
    let mut mem_sec = div().w_full().flex().flex_col().gap(px(6.0)).child(
        muse_collapsible_header(
            "bot-sec-memory-hdr",
            "bot-sec-memory",
            &mem_title,
            mem_collapsed,
            pal,
            cx,
        ),
    );
    if !mem_collapsed {
        let input_row = div()
            .px(px(4.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .child(
                div()
                    .id("bot-fusion-memory-input")
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(28.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(if mem_focused {
                        rgb(pal.accent).into()
                    } else {
                        pal.separator_rgba()
                    })
                    .cursor_text()
                    .flex()
                    .items_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.ensure_bot_inputs(cx);
                            this.bot_config_focus = BotConfigFocusField::MemoryDraft;
                            cx.notify();
                        }),
                    )
                    .child(render_ime_text_preview(
                        &mem_text,
                        mem_preedit.as_deref(),
                        tr_str(lang, "输入一条记忆，回车记住…", "Add a memory, press Enter…"),
                        mem_focused,
                        false,
                        pal.text_primary,
                        pal,
                    )),
            )
            .child(
                div()
                    .id("bot-fusion-memory-add-btn")
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.accent))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xFFFFFF))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.submit_memory_draft() {
                                cx.notify();
                            }
                        }),
                    )
                    .child(tr_str(lang, "+ 记住", "+ Save")),
            );

        mem_sec = mem_sec.child(input_row);

        if snap.memories.is_empty() {
            mem_sec = mem_sec.child(
                div()
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_size(px(11.0))
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(
                        lang,
                        "暂无专属记忆 · 输入偏好或上下文让该 Bot 长期记住",
                        "No memories yet · Add preferences or facts for this bot",
                    )),
            );
        } else {
            for (idx, mem) in snap.memories.iter().take(6).enumerate() {
                let mem_id = mem.id.clone();
                mem_sec = mem_sec.child(
                    div()
                        .id(SharedString::from(format!("bot-fusion-mem-row-{idx}")))
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(8.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(pal.separator_rgba())
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(6.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(11.5))
                                .text_color(rgb(pal.text_primary))
                                .child(mem.content.clone()),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("bot-fusion-mem-del-{idx}")))
                                .cursor_pointer()
                                .px(px(4.0))
                                .rounded(px(4.0))
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_secondary))
                                .hover(|s| s.text_color(rgb(pal.danger)))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.pending_action =
                                            Some(InspectorAction::DeleteBotMemory(mem_id.clone()));
                                        cx.notify();
                                    }),
                                )
                                .child("×"),
                        ),
                );
            }
            mem_sec = mem_sec.child(
                div()
                    .px(px(4.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("bot-fusion-memory-tidy-btn")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(999.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                            .text_size(px(10.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(pal.accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.pending_action = Some(InspectorAction::TidyBotMemories);
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "去重整理记忆", "Tidy Memories")),
                    )
                    .child(
                        div()
                            .id("bot-fusion-memory-open-panel")
                            .cursor_pointer()
                            .text_size(px(10.5))
                            .text_color(rgb(pal.accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.pending_action = Some(InspectorAction::OpenBotCompanion(
                                        BotCompanionTab::Knowledge,
                                    ));
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "在工作台展开 →", "Open in Panel →")),
                    ),
            );
        }
    }

    // 2. Skills & MCP section
    let skills_title = format!(
        "{} ({})",
        tr_str(lang, "技能与 MCP", "Skills & MCP"),
        snap.skills.len()
    );
    let mut skills_sec = div().w_full().flex().flex_col().gap(px(6.0)).child(
        muse_collapsible_header(
            "bot-sec-skills-hdr",
            "bot-sec-skills",
            &skills_title,
            skills_collapsed,
            pal,
            cx,
        ),
    );
    if !skills_collapsed {
        for (idx, sk) in snap.skills.iter().take(6).enumerate() {
            let sid = sk.skill_id.clone();
            let next_enabled = !sk.enabled;
            skills_sec = skills_sec.child(
                div()
                    .id(SharedString::from(format!("bot-fusion-skill-row-{idx}")))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .truncate()
                                    .child(format!("${}", sk.slug)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .truncate()
                                    .child(sk.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("bot-fusion-skill-toggle-{idx}")))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.pending_action = Some(InspectorAction::ToggleBotSkill {
                                        skill_id: sid.clone(),
                                        enabled: next_enabled,
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(apple_switch_pill(sk.enabled, pal)),
                    ),
            );
        }

        let mut quick_chips = div().px(px(4.0)).flex().flex_wrap().gap(px(5.0));
        let mut has_uninstalled = false;
        for (idx, &(slug, zh, en, desc)) in QUICK_BOT_SKILLS.iter().enumerate() {
            if snap.skills.iter().any(|s| s.slug.eq_ignore_ascii_case(slug)) {
                continue;
            }
            has_uninstalled = true;
            let s_slug = slug.to_string();
            let s_name = if is_zh { zh } else { en }.to_string();
            let s_desc = desc.to_string();
            quick_chips = quick_chips.child(
                div()
                    .id(SharedString::from(format!("bot-fusion-skill-quick-{idx}")))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(999.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.10))
                    .border_1()
                    .border_color(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.25))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(pal.accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.pending_action = Some(InspectorAction::InstallBotSkill {
                                slug: s_slug.clone(),
                                name: s_name.clone(),
                                description: s_desc.clone(),
                            });
                            cx.notify();
                        }),
                    )
                    .child(format!("+ {}", if is_zh { zh } else { en })),
            );
        }
        if has_uninstalled {
            skills_sec = skills_sec.child(quick_chips);
        }
        if !snap.mcp_servers.is_empty() {
            skills_sec = skills_sec.child(
                div()
                    .px(px(8.0))
                    .text_size(px(10.5))
                    .text_color(rgb(pal.text_secondary))
                    .child(format!(
                        "MCP: {} {}",
                        snap.mcp_servers.len(),
                        tr_str(lang, "个已接入服务", "connected servers")
                    )),
            );
        }
    }

    // 3. Routines section
    let routines_title = format!(
        "{} ({})",
        tr_str(lang, "定时任务", "Routines"),
        snap.routines.len()
    );
    let mut routines_sec = div().w_full().flex().flex_col().gap(px(6.0)).child(
        muse_collapsible_header(
            "bot-sec-routines-hdr",
            "bot-sec-routines",
            &routines_title,
            routines_collapsed,
            pal,
            cx,
        ),
    );
    if !routines_collapsed {
        for (idx, rt) in snap.routines.iter().take(5).enumerate() {
            let rid_toggle = rt.id.clone();
            let rid_run = rt.id.clone();
            let next_active = !rt.active;
            routines_sec = routines_sec.child(
                div()
                    .id(SharedString::from(format!("bot-fusion-routine-row-{idx}")))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .truncate()
                                    .child(rt.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .truncate()
                                    .child(rt.schedule_label.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .id(SharedString::from(format!("bot-fusion-routine-run-{idx}")))
                                    .cursor_pointer()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(6.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.pending_action =
                                                Some(InspectorAction::RunBotRoutineNow {
                                                    routine_id: rid_run.clone(),
                                                });
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "运行", "Run")),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("bot-fusion-routine-tgl-{idx}")))
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.pending_action =
                                                Some(InspectorAction::ToggleBotRoutine {
                                                    routine_id: rid_toggle.clone(),
                                                    active: next_active,
                                                });
                                            cx.notify();
                                        }),
                                    )
                                    .child(apple_switch_pill(rt.active, pal)),
                            ),
                    ),
            );
        }

        routines_sec = routines_sec.child(
            div()
                .px(px(4.0))
                .flex()
                .flex_wrap()
                .gap(px(5.0))
                .child(
                    div()
                        .id("bot-fusion-routine-preset-daily")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(999.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.10))
                        .border_1()
                        .border_color(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.25))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.pending_action = Some(InspectorAction::CreatePresetRoutine {
                                    preset: "daily".to_string(),
                                });
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "+ 每日早报 09:00", "+ Daily 09:00")),
                )
                .child(
                    div()
                        .id("bot-fusion-routine-preset-hourly")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(999.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.10))
                        .border_1()
                        .border_color(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.25))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.pending_action = Some(InspectorAction::CreatePresetRoutine {
                                    preset: "hourly".to_string(),
                                });
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "+ 每小时巡检", "+ Hourly Check")),
                ),
        );
    }

    // 4. Workflows section
    let wf_title = format!(
        "{} ({})",
        tr_str(lang, "工作流与画布", "Workflows & Canvas"),
        snap.workflows.len()
    );
    let mut wf_sec = div().w_full().flex().flex_col().gap(px(6.0)).child(
        muse_collapsible_header(
            "bot-sec-workflows-hdr",
            "bot-sec-workflows",
            &wf_title,
            wf_collapsed,
            pal,
            cx,
        ),
    );
    if !wf_collapsed {
        for (idx, wf) in snap.workflows.iter().take(4).enumerate() {
            let wid = wf.id.clone();
            let status_txt = wf
                .last_run_status
                .clone()
                .unwrap_or_else(|| format!("{} steps", wf.step_count));
            wf_sec = wf_sec.child(
                div()
                    .id(SharedString::from(format!("bot-fusion-wf-row-{idx}")))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .truncate()
                                    .child(wf.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .truncate()
                                    .child(status_txt),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("bot-fusion-wf-run-{idx}")))
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(6.0))
                            .bg(rgb(pal.accent))
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xFFFFFF))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.pending_action = Some(InspectorAction::RunBotWorkflow {
                                        workflow_id: wid.clone(),
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "▶ 运行", "▶ Run")),
                    ),
            );
        }

        wf_sec = wf_sec.child(
            div()
                .px(px(4.0))
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .child(
                    div()
                        .id("bot-fusion-wf-create-starter")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(999.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                        .border_1()
                        .border_color(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.28))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.pending_action = Some(InspectorAction::CreateStarterWorkflow);
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "+ 新建审批工作流", "+ New Approval Flow")),
                )
                .child(
                    div()
                        .id("bot-fusion-wf-open-companion")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(999.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(pal.separator_rgba())
                        .text_size(px(10.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.pending_action = Some(InspectorAction::OpenBotCompanion(
                                    BotCompanionTab::Workflow,
                                ));
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "打开伴生工作台", "Open Workbench")),
                ),
        );
    }

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(mem_sec)
        .child(skills_sec)
        .child(routines_sec)
        .child(wf_sec)
        .into_any_element()
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

    let ai_models = &inspector.ai_models;
    let model_presets = ai_models.selectable_models();
    let draft_model_trimmed = inspector.bot_draft_model.trim().to_string();
    let follows_settings = draft_model_trimmed.is_empty();
    let model_mismatch = !follows_settings
        && ai_models.synced
        && !ai_models.is_model_consistent_with_settings(&draft_model_trimmed);
    let model_placeholder = if !ai_models.needs_provider_selection && !ai_models.active_model.is_empty()
    {
        if lang.starts_with("zh") {
            format!("留空跟随设置 ({})", ai_models.active_model)
        } else {
            format!("Blank to follow Settings ({})", ai_models.active_model)
        }
    } else {
        tr_str(lang, "留空跟随设置默认模型", "Blank to follow Settings default").to_string()
    };
    let settings_model_status = if let Some(summary) = ai_models.active_summary() {
        if lang.starts_with("zh") {
            format!("设置当前：{summary}")
        } else {
            format!("Settings active: {summary}")
        }
    } else if ai_models.synced && ai_models.needs_provider_selection {
        tr_str(
            lang,
            "设置中尚未配置可用模型，请先前往模型设置",
            "No model configured in Settings — open Model Settings",
        )
        .to_string()
    } else {
        tr_str(
            lang,
            "正在同步设置中的模型列表…",
            "Syncing available models from Settings…",
        )
        .to_string()
    };
    let follow_chip_label = if !ai_models.needs_provider_selection && !ai_models.active_model.is_empty()
    {
        if lang.starts_with("zh") {
            format!("跟随设置 ({})", ai_models.active_model)
        } else {
            format!("Follow Settings ({})", ai_models.active_model)
        }
    } else {
        tr_str(lang, "跟随设置", "Follow Settings").to_string()
    };
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
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(lang, "首选模型", "Preferred Model")),
                        )
                        .child(
                            div()
                                .id("bot-open-model-settings-btn")
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(2.5))
                                .rounded(px(6.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                                .hover(|s| {
                                    s.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.22))
                                })
                                .text_size(px(10.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.bot_config_focus = BotConfigFocusField::None;
                                        this.pending_action =
                                            Some(InspectorAction::OpenAiProvidersSettings);
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "模型设置", "Model Settings")),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(if ai_models.synced && ai_models.needs_provider_selection {
                            pal.danger
                        } else {
                            pal.text_secondary
                        }))
                        .child(settings_model_status),
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
                            } else if model_mismatch {
                                rgb(pal.danger).into()
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
                            &model_placeholder,
                            inspector.bot_config_focus == BotConfigFocusField::Model,
                            false,
                            pal.text_primary,
                            pal,
                        )),
                )
                .when(model_mismatch, |col| {
                    let mismatch_text = if lang.starts_with("zh") {
                        format!(
                            "当前模型「{draft_model_trimmed}」与设置中的可用模型不一致，建议点击下方「跟随设置」或选择设置中的模型"
                        )
                    } else {
                        format!(
                            "\"{draft_model_trimmed}\" is not in your active Settings models — follow Settings or pick an available model below"
                        )
                    };
                    col.child(
                        div()
                            .id("bot-model-mismatch-banner")
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(7.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.12))
                            .border_1()
                            .border_color(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.45))
                            .text_size(px(10.5))
                            .text_color(rgb(pal.danger))
                            .child(mismatch_text),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(5.0))
                        .child(
                            div()
                                .id("bot-model-follow-settings")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.5))
                                .rounded(px(999.0))
                                .bg(if follows_settings {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                                } else {
                                    rgb(pal.input_bg).into()
                                })
                                .border_1()
                                .border_color(if follows_settings {
                                    rgb(pal.accent).into()
                                } else {
                                    pal.separator_rgba()
                                })
                                .hover(|s| s.bg(rgb(pal.dialogs_hover_bg)))
                                .text_size(px(10.5))
                                .font_weight(if follows_settings {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(rgb(if follows_settings {
                                    pal.accent
                                } else {
                                    pal.text_primary
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(ref ctrl) = this.model_input {
                                            ctrl.update(cx, |c, _| c.clear_preedit());
                                        }
                                        this.bot_draft_model.clear();
                                        cx.notify();
                                    }),
                                )
                                .child(follow_chip_label),
                        )
                        .children(model_presets.into_iter().enumerate().map(|(i, preset)| {
                            let selected = !follows_settings
                                && draft_model_trimmed.eq_ignore_ascii_case(&preset);
                            let preset_click = preset.clone();
                            div()
                                .id(("bot-model-preset", i))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.5))
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
                                .hover(|s| s.bg(rgb(pal.dialogs_hover_bg)))
                                .text_size(px(10.5))
                                .font_weight(if selected {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(rgb(if selected {
                                    pal.accent
                                } else {
                                    pal.text_secondary
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        if let Some(ref ctrl) = this.model_input {
                                            ctrl.update(cx, |c, _| c.clear_preedit());
                                        }
                                        this.bot_draft_model = preset_click.clone();
                                        cx.notify();
                                    }),
                                )
                                .child(preset)
                        })),
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
                        let cleaned_model = {
                            let trimmed = this.bot_draft_model.trim();
                            if InspectorAiModelCatalog::is_placeholder_model(trimmed) {
                                String::new()
                            } else {
                                trimmed.to_string()
                            }
                        };
                        this.bot_draft_model = cleaned_model.clone();
                        this.pending_action = Some(InspectorAction::SaveBotComputerConfig {
                            bot_id,
                            persona_prompt: this.bot_draft_persona.clone(),
                            preferred_model: cleaned_model,
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

// ---------------------------------------------------------------------------
// M4 UI-F16: OpenBot `AgentSettingsPanel` (Bot Profile inside `打开 助手设置`)
// ---------------------------------------------------------------------------

pub(crate) fn ob_inset_group(
    title: &str,
    ob: &OpenBotPalette,
    rows: Vec<AnyElement>,
) -> AnyElement {
    let border = ob.border_hairline();
    let last_idx = rows.len().saturating_sub(1);
    let mut card = div()
        .w_full()
        .rounded(px(BOT_SETTINGS_INSET_CARD_RADIUS_PX))
        .bg(ob.bg_surface())
        .border_1()
        .border_color(border)
        .overflow_hidden()
        .flex()
        .flex_col();
    for (idx, row) in rows.into_iter().enumerate() {
        card = card.child(row);
        if idx < last_idx {
            card = card.child(div().w_full().h(px(1.0)).bg(border));
        }
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .when(!title.is_empty(), |col| {
            col.child(
                div()
                    .ml(px(4.0))
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ob.text_muted())
                    .child(title.to_string()),
            )
        })
        .child(card)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn ob_inset_row(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<String>,
    trailing_value: Option<String>,
    show_chevron: bool,
    danger: bool,
    ob: &OpenBotPalette,
    on_click: impl Fn(&mut ContextInspector, &mut Context<ContextInspector>) + 'static,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let hover = ob.hover();
    let fg = if danger { ob.danger() } else { ob.text_primary() };
    let icon_fg = if danger { ob.danger() } else { ob.text_muted() };
    let muted = ob.text_muted();
    div()
        .id(id)
        .w_full()
        .min_h(px(BOT_SETTINGS_INSET_ROW_HEIGHT_PX))
        .px(px(12.0))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                on_click(this, cx);
                cx.notify();
            }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(9.0))
                .child(tg_icon_rgba(icon, 14.0, icon_fg))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg)
                        .truncate()
                        .child(label.into()),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .when_some(trailing_value, |r, val| {
                    r.child(
                        div()
                            .max_w(px(140.0))
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(muted)
                            .child(val),
                    )
                })
                .when(show_chevron, |r| {
                    r.child(tg_icon_rgba(IconName::ChevronRight, 13.0, muted))
                }),
        )
        .into_any_element()
}

pub(crate) fn ob_switch_pill(enabled: bool, ob: &OpenBotPalette) -> Div {
    let track = if enabled {
        ob.success()
    } else {
        ob.bg_control_active()
    };
    div()
        .w(px(36.0))
        .h(px(20.0))
        .p(px(2.0))
        .rounded_full()
        .bg(track)
        .border_1()
        .border_color(if enabled {
            track
        } else {
            ob.border_strong_hairline()
        })
        .flex()
        .items_center()
        .when(enabled, |el| el.justify_end())
        .when(!enabled, |el| el.justify_start())
        .child(div().w(px(16.0)).h(px(16.0)).rounded_full().bg(rgb(0xFFFFFF)))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn ob_text_input_box(
    id: impl Into<ElementId>,
    label: &str,
    value: &str,
    preedit: Option<&str>,
    placeholder: &str,
    focused: bool,
    min_h_px: f32,
    disabled: bool,
    field: OpenBotInspectorTextField,
    pal: TdesktopThemePalette,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let border_col = if focused {
        ob.accent()
    } else {
        ob.border_strong_hairline()
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .when(!label.is_empty(), |col| {
            col.child(
                div()
                    .ml(px(8.0))
                    .text_size(px(13.0))
                    .text_color(ob.text_secondary())
                    .child(label.to_string()),
            )
        })
        .child(
            div()
                .id(id)
                .w_full()
                .min_h(px(min_h_px))
                .px(px(11.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .bg(ob.bg_surface())
                .border_1()
                .border_color(border_col)
                .when(!disabled, |el| {
                    el.cursor_text().on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.focus_openbot_text_field(field, cx);
                        }),
                    )
                })
                .when(disabled, |el| el.opacity(0.55))
                .child(render_ime_text_preview(
                    value,
                    if focused { preedit } else { None },
                    placeholder,
                    focused && !disabled,
                    min_h_px > 50.0,
                    pal.text_primary,
                    pal,
                )),
        )
        .into_any_element()
}

pub(crate) fn ob_confirm_guard_card(
    id_prefix: &'static str,
    title: &str,
    description: &str,
    confirm_label: &str,
    danger: bool,
    ob: &OpenBotPalette,
    lang: &str,
    on_confirm: impl Fn(&mut ContextInspector, &mut Context<ContextInspector>) + 'static,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let pal = TdesktopThemePalette::build_openbot(
        if ob.scheme.is_dark() {
            crate::theme::TdesktopThemeMode::Night
        } else {
            crate::theme::TdesktopThemeMode::Day
        },
        0x0A84FF,
        14,
        16,
    );
    let dlg_state = crate::views::boxes::modal_theme::ConfirmDialogState::open_dialog(danger);
    let dlg_layout =
        crate::views::boxes::modal_theme::compute_confirm_dialog_layout(&dlg_state, &pal);
    let cancel_focused = dlg_layout.initial_focus == "cancel";
    let btn_bg = if danger { ob.danger() } else { ob.accent() };
    div()
        .id(SharedString::from(format!("{id_prefix}-guard")))
        .w_full()
        .max_w(px(dlg_layout.max_width_px))
        .p(px(12.0))
        .rounded(px(8.0))
        .bg(ob.bg_surface())
        .border_1()
        .border_color(if danger { ob.danger() } else { ob.warning() })
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if danger { ob.danger() } else { ob.text_primary() })
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(ob.text_muted())
                .child(description.to_string()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(
                    div()
                        .id(SharedString::from(format!("{id_prefix}-cancel")))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(6.0))
                        .bg(ob.bg_control_hover())
                        .when(cancel_focused, |btn| {
                            btn.border_1().border_color(ob.text_primary())
                        })
                        .text_size(px(12.0))
                        .text_color(ob.text_secondary())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.pending_confirm_guard = None;
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "取消", "Cancel")),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{id_prefix}-confirm")))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(6.0))
                        .bg(btn_bg)
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.pending_confirm_guard = None;
                                on_confirm(this, cx);
                                cx.notify();
                            }),
                        )
                        .child(confirm_label.to_string()),
                ),
        )
        .into_any_element()
}

pub const BOT_SETTINGS_HERO_AVATAR_PX: f32 = 62.0;
pub const BOT_SETTINGS_SUBPAGE_HEADER_PX: f32 = 39.0;
pub const BOT_SETTINGS_INSET_CARD_RADIUS_PX: f32 = 8.0;
pub const BOT_SETTINGS_INSET_ROW_HEIGHT_PX: f32 = 40.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotCreationsCounts {
    pub books: usize,
    pub music: usize,
    pub games: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenBotBotSettingsLayout {
    pub mounted: bool,
    pub host: &'static str,
    pub hero_avatar_px: f32,
    pub pencil_edit_visible: bool,
    pub avatar_popover_open: bool,
    pub avatar_editor_width_px: f32,
    pub avatar_shapes_count: usize,
    pub avatar_eyes_count: usize,
    pub avatar_editor_cols: usize,
    pub avatar_editor_rows: usize,
    pub avatar_editor_total_cells: usize,
    pub online: bool,
    pub name: String,
    pub bio: String,
    pub display_handle: String,
    pub handle_copied: bool,
    pub autosave_debounce_ms: u64,
    pub inset_card_bg_hex: String,
    pub inset_card_radius_px: f32,
    pub inset_row_height_px: f32,
    pub profile_fields: [&'static str; 4],
    pub creations_counts: BotCreationsCounts,
    pub groups_present: [&'static str; 5],
    pub active_subpage: Option<&'static str>,
    pub subpage_header_px: f32,
    pub subpages: [&'static str; 6],
    pub permission_level: String,
    pub permission_modes: [&'static str; 3],
    pub destructive_confirm_dialogs: [&'static str; 3],
    pub backup_models_count: usize,
    pub backup_models_limit: usize,
    pub folders_count: usize,
    pub folders_limit: usize,
    pub delete_history_disabled: bool,
    pub context_inspector_live_reachable: bool,
}

pub fn compute_openbot_bot_settings_layout(
    inspector: &ContextInspector,
    pal: &TdesktopThemePalette,
) -> OpenBotBotSettingsLayout {
    let ob = pal.openbot_palette();
    let clean_username = inspector
        .profile
        .username
        .trim()
        .trim_start_matches('@');
    let display_handle = if clean_username.is_empty() {
        String::new()
    } else {
        format!("@{clean_username}")
    };
    let name = if !inspector.bot_draft_name.trim().is_empty() {
        inspector.bot_draft_name.clone()
    } else {
        inspector.profile.name.clone()
    };
    let bio = if !inspector.bot_draft_description.trim().is_empty() {
        inspector.bot_draft_description.clone()
    } else {
        inspector.profile.bio.clone()
    };
    let books = inspector
        .published_creations
        .iter()
        .filter(|c| c.kind_code == "book")
        .count();
    let music = inspector
        .published_creations
        .iter()
        .filter(|c| c.kind_code == "music")
        .count();
    let games = inspector
        .published_creations
        .iter()
        .filter(|c| c.kind_code == "game")
        .count();
    let permission_level = match inspector.bot_access_level.trim().to_ascii_lowercase().as_str() {
        "full" | "autonomous" => "full".to_string(),
        "workspace" | "strict" => "workspace".to_string(),
        _ => "ask".to_string(),
    };
    let route = inspector.resolve_context_inspector_render_route();
    let context_inspector_live_reachable = matches!(
        route,
        crate::views::context_inspector::ContextInspectorRenderRoute::OpenBotBotSettingsRoot
            | crate::views::context_inspector::ContextInspectorRenderRoute::OpenBotBotSubpage(_)
            | crate::views::context_inspector::ContextInspectorRenderRoute::OpenBotSharedMediaSubpage
    );
    let inset_bg_rgb = crate::views::content_hub::apple_primitives::rgba_to_rgb24(ob.bg_surface());

    OpenBotBotSettingsLayout {
        mounted: inspector.is_open && inspector.embedded_in_side_panel && inspector.profile.is_bot,
        host: if inspector.embedded_in_side_panel {
            "side_panel_assistant_settings"
        } else {
            "classic_drawer"
        },
        hero_avatar_px: BOT_SETTINGS_HERO_AVATAR_PX,
        pencil_edit_visible: inspector.profile.is_viewer_owner && inspector.active_fabushi_bot.is_some(),
        avatar_popover_open: inspector.avatar_editor_open,
        avatar_editor_width_px: crate::components::bloub_avatar::EDITOR_MAX_WIDTH_PX,
        avatar_shapes_count: openbot_contracts::avatar::SUPPORTED_AVATAR_SILHOUETTES.len(),
        avatar_eyes_count: openbot_contracts::avatar::AVATAR_EXPRESSIONS.len(),
        avatar_editor_cols: crate::components::bloub_avatar::EDITOR_COLOR_COLS,
        avatar_editor_rows: crate::components::bloub_avatar::EDITOR_COLOR_ROWS,
        avatar_editor_total_cells: crate::components::bloub_avatar::EDITOR_COLOR_TOTAL_CELLS,
        online: inspector.profile.is_bot
            && (inspector.active_fabushi_bot.is_some()
                || inspector.profile.status.trim().eq_ignore_ascii_case("online")
                || inspector.profile.status.trim() == "在线"
                || inspector.profile.status.trim().is_empty()),
        name,
        bio,
        display_handle,
        handle_copied: inspector.copied_feedback.is_some(),
        autosave_debounce_ms: inspector.bot_autosave.debounce_ms,
        inset_card_bg_hex: crate::views::content_hub::apple_primitives::rgb24_to_hex(inset_bg_rgb),
        inset_card_radius_px: BOT_SETTINGS_INSET_CARD_RADIUS_PX,
        inset_row_height_px: BOT_SETTINGS_INSET_ROW_HEIGHT_PX,
        profile_fields: [
            "creations_books_music_games",
            "add_to_group",
            "shared_media",
            "notify_me",
        ],
        creations_counts: BotCreationsCounts { books, music, games },
        groups_present: ["profile", "brain", "knows", "does", "rules"],
        active_subpage: inspector.openbot_subpage.map(|s| s.slug()),
        subpage_header_px: BOT_SETTINGS_SUBPAGE_HEADER_PX,
        subpages: [
            "permissions",
            "instructions",
            "skills",
            "memory",
            "routines",
            "workflows",
        ],
        permission_level,
        permission_modes: ["workspace", "ask", "full"],
        destructive_confirm_dialogs: ["new_chat", "delete_history", "delete_agent"],
        backup_models_count: inspector.bot_backup_models.len(),
        backup_models_limit: openbot_contracts::agents::AGENT_BACKUP_MODEL_LIMIT,
        folders_count: inspector.bot_additional_folders.len()
            + inspector.bot_excluded_folders.len(),
        folders_limit: openbot_contracts::agents::AGENT_FOLDER_LIMIT,
        delete_history_disabled: inspector.is_bot_busy(),
        context_inspector_live_reachable,
    }
}

/// Renders the embedded OpenBot `AgentSettingsPanel` root (`UI-F16`) inside `打开 助手设置`.
pub fn render_openbot_bot_settings_root(
    inspector: &ContextInspector,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let layout = compute_openbot_bot_settings_layout(inspector, &pal);
    let ob = pal.openbot_palette();
    let is_dark = pal.mode.is_dark();
    let model = &inspector.profile;
    let bot_id = model.bot_id;
    let is_owner_fabushi = layout.pencil_edit_visible;
    let snap = &inspector.bot_fusion;
    let preedit = inspector.openbot_input_preedit(cx);

    let clean_username = model.username.trim().trim_start_matches('@').to_string();
    let display_handle = layout.display_handle.clone();

    // Centered 62x62px 3D plush avatar + 24x24px bottom-right Pencil badge
    let hero_avatar: AnyElement = if let Some(ref custom) = inspector
        .bot_avatar_custom_photo
        .as_ref()
        .or(model.avatar_path.as_ref())
    {
        div()
            .w(px(layout.hero_avatar_px))
            .h(px(layout.hero_avatar_px))
            .rounded_full()
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
    } else if let Some(ref fab) = inspector.active_fabushi_bot {
        let (shape, hue, color_hex) =
            normalize_fabushi_plush_spec(&fab.avatar_shape, &fab.avatar_color);
        let (mood, activity) = activity_str_to_openbot_mood(&fab.execution_state);
        if inspector.bot_avatar_style_flat
            || inspector.bot_avatar_expression != BloubExpression::Neutre
        {
            let seed_base = if fab.agent_id.trim().is_empty() {
                format!("bot-{}", fab.bot_user_id)
            } else {
                fab.agent_id.clone()
            };
            let style = if inspector.bot_avatar_style_flat {
                AvatarStyle::Flat
            } else {
                AvatarStyle::Plush
            };
            let sh = shape.unwrap_or(BloubShape::Cercle);
            let seed = avatar_seed_for_face(&seed_base, sh, inspector.bot_avatar_expression, style)
                .unwrap_or(seed_base);
            let mut props = AgentAvatarProps::new(seed).with_size(62.0).with_mood(mood);
            props.shape_override = Some(sh);
            props.expression_override = Some(inspector.bot_avatar_expression);
            props.hue = hue;
            props.color = color_hex;
            props.activity = activity;
            props.dark_scheme = is_dark;
            render_agent_avatar(&props, None, 0.0).into_any_element()
        } else {
            let seed = if fab.agent_id.trim().is_empty() {
                model.name.as_str()
            } else {
                fab.agent_id.as_str()
            };
            render_plush_bot_avatar(seed, shape, hue, color_hex.as_deref(), 62.0, mood, activity)
                .into_any_element()
        }
    } else {
        let seed = if !clean_username.is_empty() {
            clean_username.as_str()
        } else {
            model.name.as_str()
        };
        render_plush_bot_avatar(seed, None, None, None, 62.0, AvatarMood::Idle, None)
            .into_any_element()
    };

    let hero_block = div()
        .w_full()
        .pt(px(4.0))
        .pb(px(4.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .relative()
                .w(px(62.0))
                .h(px(62.0))
                .child(hero_avatar)
                .when(is_owner_fabushi, |wrap| {
                    wrap.child(
                        div()
                            .id("inspector-hero-edit-btn")
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
                                    this.avatar_editor_open = !this.avatar_editor_open;
                                    cx.notify();
                                }),
                            )
                            .child(tg_icon_rgba(IconName::Edit, 11.0, ob.text_primary())),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .when(!display_handle.is_empty(), |row| {
                    row.child(
                        div()
                            .id("bot-copy-username")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(6.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .hover(|s| s.bg(ob.bg_control_hover()))
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .text_size(px(12.0))
                            .text_color(ob.text_secondary())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(msg) = this.trigger_copy_bot_handle() {
                                        this.show_copied_feedback(msg, cx);
                                    }
                                    cx.notify();
                                }),
                            )
                            .child(display_handle)
                            .child(tg_icon_rgba(IconName::Copy, 11.0, ob.text_muted())),
                    )
                })
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .bg(ob.bg_surface())
                        .border_1()
                        .border_color(ob.border_hairline())
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(ob.success()))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(ob.text_muted())
                                .child(tr_str(lang, "在线 · 已连接", "Online · Connected")),
                        ),
                ),
        )
        .when(inspector.avatar_editor_open && is_owner_fabushi, |col| {
            col.child(render_bot_avatar_editor_popover(inspector, pal, lang, cx))
        });

    let name_val = if inspector.bot_draft_name.is_empty() {
        model.name.clone()
    } else {
        inspector.bot_draft_name.clone()
    };
    let desc_val = if inspector.bot_draft_description.is_empty() {
        model.bio.clone()
    } else {
        inspector.bot_draft_description.clone()
    };
    let identity_section = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(ob_text_input_box(
            "openbot-bot-name-input",
            tr_str(lang, "名称", "Name"),
            &name_val,
            preedit.as_deref(),
            tr_str(lang, "输入助手名称", "Agent name"),
            inspector.openbot_text_focus == Some(OpenBotInspectorTextField::BotName),
            38.0,
            !is_owner_fabushi,
            OpenBotInspectorTextField::BotName,
            pal,
            cx,
        ))
        .child(ob_text_input_box(
            "openbot-bot-title-input",
            tr_str(lang, "称号", "Title"),
            &inspector.bot_draft_title,
            preedit.as_deref(),
            tr_str(lang, "角色定位 / 称号（可选）", "Role title (optional)"),
            inspector.openbot_text_focus == Some(OpenBotInspectorTextField::BotTitle),
            38.0,
            !is_owner_fabushi,
            OpenBotInspectorTextField::BotTitle,
            pal,
            cx,
        ))
        .child(ob_text_input_box(
            "openbot-bot-desc-input",
            tr_str(lang, "描述", "Description"),
            &desc_val,
            preedit.as_deref(),
            tr_str(lang, "简介与能力说明…", "Describe what this assistant does…"),
            inspector.openbot_text_focus == Some(OpenBotInspectorTextField::BotDescription),
            72.0,
            !is_owner_fabushi,
            OpenBotInspectorTextField::BotDescription,
            pal,
            cx,
        ))
        .when(is_owner_fabushi, |col| {
            col.child(
                div()
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .id("openbot-bot-save-identity-btn")
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
                                    this.commit_bot_identity_draft();
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "保存资料", "Save Profile")),
                    ),
            )
        });

    // Group 1: 资料与作品 (Bot Profile & Creations)
    let creations_count = inspector.published_creations.len();
    let media_total = inspector.shared_media.media_items.len()
        + inspector.shared_media.files.len()
        + inspector.shared_media.links.len();
    let target_bot_id = inspector
        .active_fabushi_bot
        .as_ref()
        .map(|b| b.bot_user_id.max(0) as u64)
        .filter(|&id| id > 0)
        .unwrap_or(bot_id);
    let notif_on = model.is_notifications_enabled;

    let mut profile_rows: Vec<AnyElement> = vec![
        ob_inset_row(
            "openbot-bot-creations-row",
            IconName::FolderAll,
            tr_str(lang, "作品集 (图书 / 音乐 / 游戏)", "Creations (Books / Music / Games)"),
            Some(if lang.starts_with("zh") {
                format!("{creations_count} 件")
            } else {
                format!("{creations_count}")
            }),
            true,
            false,
            &ob,
            |this, _| {
                this.openbot_subpage = Some(OpenBotSettingsSubpage::Creations);
            },
            cx,
        ),
        ob_inset_row(
            "bot-profile-add-to-group",
            IconName::GroupUsers,
            tr_str(lang, "添加到群聊", "Add to Group"),
            None,
            true,
            false,
            &ob,
            move |this, _| {
                this.pending_action = Some(InspectorAction::OpenAddBotToGroup(target_bot_id));
            },
            cx,
        ),
        ob_inset_row(
            "openbot-bot-shared-media-row",
            IconName::FileDoc,
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
            .id("bot-action-mute")
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
                            .child(tr_str(lang, "通知我", "Notify me")),
                    ),
            )
            .child(ob_switch_pill(notif_on, &ob))
            .into_any_element(),
    ];

    if model.bot_has_main_app || model.miniapp_url.is_some() {
        let launch_uri = model.miniapp_url.clone().unwrap_or_else(|| {
            if !clean_username.is_empty() {
                format!("gpui-miniapp://{clean_username}/app")
            } else {
                String::new()
            }
        });
        let app_title = model
            .miniapp_title
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| tr_str(lang, "打开应用", "Open App").to_string());
        profile_rows.push(ob_inset_row(
            "inspector-launch-miniapp-btn",
            IconName::FolderBots,
            app_title,
            None,
            true,
            false,
            &ob,
            move |this, _| {
                this.pending_action = Some(InspectorAction::LaunchMiniApp(launch_uri.clone()));
            },
            cx,
        ));
    }

    if !model.bot_commands.is_empty() {
        let first_cmd = model.bot_commands[0].0.clone();
        let slash = if first_cmd.starts_with('/') {
            first_cmd
        } else {
            format!("/{first_cmd}")
        };
        profile_rows.push(ob_inset_row(
            "openbot-bot-quick-cmds-row",
            IconName::TerminalMcp,
            tr_str(lang, "快捷指令", "Quick Commands"),
            Some(format!("{} ({})", slash, model.bot_commands.len())),
            true,
            false,
            &ob,
            move |this, _| {
                this.pending_action = Some(InspectorAction::RunBotCommand(slash.clone()));
            },
            cx,
        ));
    }

    let profile_group = ob_inset_group(tr_str(lang, "资料与作品", "Profile"), &ob, profile_rows);

    // Group 2: 核心 (Brain)
    let eff_model_label = format_bot_effective_model_label(inspector, lang);
    let cur_thinking = inspector
        .active_fabushi_bot
        .as_ref()
        .map(|b| b.thinking_level.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "medium".to_string());

    let mut reasoning_bar = div().w_full().px(px(12.0)).py(px(8.0)).flex().items_center().justify_between().gap(px(6.0))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(ob.text_muted())
                .child(tr_str(lang, "推理强度", "Reasoning")),
        );
    let mut effort_pills = div().flex().items_center().gap(px(3.0));
    for (idx, level) in ["off", "low", "medium", "high"].iter().enumerate() {
        let sel = cur_thinking == *level;
        let lvl_owned = (*level).to_string();
        let label_txt = match (*level, lang.starts_with("zh")) {
            ("off", true) => "关",
            ("low", true) => "低",
            ("medium", true) => "中",
            ("high", true) => "高",
            (other, _) => other,
        };
        effort_pills = effort_pills.child(
            div()
                .id(SharedString::from(format!("fabushi-insp-thinking-{idx}")))
                .cursor_pointer()
                .px(px(8.0))
                .py(px(3.0))
                .rounded(px(6.0))
                .bg(if sel { ob.bg_control_active() } else { ob.bg_canvas() })
                .border_1()
                .border_color(if sel { ob.text_muted() } else { ob.border_hairline() })
                .text_size(px(11.5))
                .font_weight(if sel { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .text_color(if sel { ob.text_primary() } else { ob.text_muted() })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(ref mut fab) = this.active_fabushi_bot {
                            fab.thinking_level = lvl_owned.clone();
                        }
                        if target_bot_id > 0 {
                            this.pending_action = Some(InspectorAction::PatchFabushiBot {
                                bot_id: target_bot_id,
                                req: PatchFabushiBotRequest {
                                    thinking_level: Some(lvl_owned.clone()),
                                    ..Default::default()
                                },
                            });
                        }
                        cx.notify();
                    }),
                )
                .child(label_txt),
        );
    }
    reasoning_bar = reasoning_bar.child(effort_pills);

    let mut bubble_swatches = div().flex().items_center().gap(px(5.0)).flex_wrap();
    bubble_swatches = bubble_swatches.child(
        div()
            .id("openbot-bot-bubble-default")
            .cursor_pointer()
            .w(px(18.0))
            .h(px(18.0))
            .rounded_full()
            .bg(ob.bg_canvas())
            .border_1()
            .border_color(if inspector.bot_chat_bubble_color.is_none() {
                ob.text_primary()
            } else {
                ob.border_strong_hairline()
            })
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(9.0))
            .text_color(ob.text_muted())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.bot_chat_bubble_color = None;
                    this.pending_openbot_action = Some(OpenBotInspectorAction::SetChatBubbleColor(None));
                    cx.notify();
                }),
            )
            .child("×"),
    );
    for (idx, preset) in CHAT_COLOR_PRESETS.iter().enumerate() {
        let sel = inspector.bot_chat_bubble_color.as_deref() == Some(preset.hex);
        let sw = parse_css_color_to_rgba(preset.hex).unwrap_or_else(|| rgb(0x4699FE));
        let hex_owned = preset.hex.to_string();
        bubble_swatches = bubble_swatches.child(
            div()
                .id(SharedString::from(format!("openbot-bot-bubble-color-{idx}")))
                .cursor_pointer()
                .w(px(18.0))
                .h(px(18.0))
                .rounded_full()
                .bg(sw)
                .border_1()
                .border_color(if sel {
                    ob.text_primary()
                } else {
                    ob.border_strong_hairline()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.bot_chat_bubble_color = Some(hex_owned.clone());
                        this.pending_openbot_action = Some(
                            OpenBotInspectorAction::SetChatBubbleColor(Some(hex_owned.clone())),
                        );
                        cx.notify();
                    }),
                ),
        );
    }

    let mut brain_rows: Vec<AnyElement> = vec![
        ob_inset_row(
            "bot-open-config-editor",
            IconName::BotRobot,
            tr_str(lang, "模型与推理", "Model & Reasoning"),
            Some(eff_model_label),
            true,
            false,
            &ob,
            move |this, cx| {
                if is_owner_fabushi {
                    this.open_bot_config_editor(cx);
                } else {
                    this.pending_action = Some(InspectorAction::OpenAiProvidersSettings);
                }
            },
            cx,
        ),
        reasoning_bar.into_any_element(),
        ob_inset_row(
            "openbot-bot-backup-models-row",
            IconName::Forward,
            tr_str(lang, "备用模型", "Backup Models"),
            Some(format!("{}/8", inspector.bot_backup_models.len())),
            true,
            false,
            &ob,
            |this, _| {
                let _ = this.open_bot_subpage(OpenBotSettingsSubpage::BackupModels);
            },
            cx,
        ),
        div()
            .w_full()
            .px(px(12.0))
            .py(px(9.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(ob.text_muted())
                    .child(tr_str(lang, "气泡颜色", "Chat Color")),
            )
            .child(bubble_swatches)
            .into_any_element(),
    ];
    if inspector
        .active_fabushi_bot
        .as_ref()
        .and_then(|b| b.coding_agent.as_ref())
        .is_some()
    {
        brain_rows.push(ob_inset_row(
            "openbot-bot-coding-session-row",
            IconName::TerminalMcp,
            tr_str(lang, "Coding Agent 工作区", "Coding Session"),
            Some(inspector.coding_draft_workspace.clone()),
            true,
            false,
            &ob,
            |this, _| {
                this.openbot_subpage = Some(OpenBotSettingsSubpage::Folders);
            },
            cx,
        ));
    }
    let brain_group = ob_inset_group(tr_str(lang, "核心", "Brain"), &ob, brain_rows);

    // Group 3: 已知 (Knows)
    let (explicit_comp, eff_comp) =
        resolve_bot_computer_binding(&inspector.computers, inspector.bot_computer_profile.as_ref());
    let (comp_badge, _) = computer_tier_badge(explicit_comp, eff_comp, lang);
    let knows_group = ob_inset_group(
        tr_str(lang, "已知", "Knows"),
        &ob,
        vec![
            ob_inset_row(
                "openbot-bot-instructions-row",
                IconName::FileDoc,
                tr_str(lang, "指令", "Instructions"),
                Some(format!("{} chars", inspector.bot_draft_persona.chars().count())),
                true,
                false,
                &ob,
                |this, _| {
                    let _ = this.open_bot_subpage(OpenBotSettingsSubpage::Instructions);
                },
                cx,
            ),
            ob_inset_row(
                "openbot-bot-memories-row",
                IconName::FolderAll,
                tr_str(lang, "记忆", "Memories"),
                Some(if lang.starts_with("zh") {
                    format!("已保存 {} 条", snap.memories.len())
                } else {
                    format!("{} saved", snap.memories.len())
                }),
                true,
                false,
                &ob,
                |this, _| {
                    let _ = this.open_bot_subpage(OpenBotSettingsSubpage::Memories);
                },
                cx,
            ),
            ob_inset_row(
                "openbot-bot-skills-row",
                IconName::TerminalMcp,
                tr_str(lang, "技能", "Skills"),
                Some(if lang.starts_with("zh") {
                    format!("已分配 {} 个", snap.skills.len())
                } else {
                    format!("{} installed", snap.skills.len())
                }),
                true,
                false,
                &ob,
                |this, _| {
                    let _ = this.open_bot_subpage(OpenBotSettingsSubpage::Skills);
                },
                cx,
            ),
            ob_inset_row(
                "openbot-bot-files-row",
                IconName::FileDoc,
                tr_str(lang, "文件", "Files"),
                Some(format!("{}", inspector.shared_media.files.len())),
                true,
                false,
                &ob,
                |this, _| {
                    this.pending_openbot_action =
                        Some(OpenBotInspectorAction::OpenSidePanelPane(SidePanelPaneKind::Files));
                },
                cx,
            ),
            ob_inset_row(
                "openbot-bot-tables-row",
                IconName::FolderAll,
                tr_str(lang, "数据表", "Data Tables"),
                Some(format!("{}", inspector.bot_data_tables.len())),
                true,
                false,
                &ob,
                |this, _| {
                    let _ = this.open_bot_subpage(OpenBotSettingsSubpage::DataTables);
                },
                cx,
            ),
            ob_inset_row(
                "bot-open-computers-list-row",
                IconName::ComputerDesktop,
                tr_str(lang, "远程电脑", "Remote Computer"),
                Some(comp_badge),
                true,
                false,
                &ob,
                |this, _| {
                    this.subpage = InspectorSubpage::ComputersList;
                    this.pending_action = Some(InspectorAction::OpenBotComputersSubpage);
                },
                cx,
            ),
        ],
    );

    // Group 4: 行为 (Does)
    let does_group = ob_inset_group(
        tr_str(lang, "行为", "Does"),
        &ob,
        vec![
            ob_inset_row(
                "openbot-bot-routines-row",
                IconName::Play,
                tr_str(lang, "自动化任务", "Routines"),
                Some(if lang.starts_with("zh") {
                    format!("已配置 {} 个", snap.routines.len())
                } else {
                    format!("{} configured", snap.routines.len())
                }),
                true,
                false,
                &ob,
                |this, _| {
                    let _ = this.open_bot_subpage(OpenBotSettingsSubpage::Routines);
                },
                cx,
            ),
            ob_inset_row(
                "openbot-bot-workflows-row",
                IconName::Forward,
                tr_str(lang, "工作流", "Workflows"),
                Some(format!("{}", snap.workflows.len())),
                true,
                false,
                &ob,
                |this, _| {
                    let _ = this.open_bot_subpage(OpenBotSettingsSubpage::Workflows);
                },
                cx,
            ),
            ob_inset_row(
                "openbot-bot-usage-row",
                IconName::SettingsGear,
                tr_str(lang, "用量", "Usage"),
                Some(format!("{} steps", snap.recent_steps.len())),
                true,
                false,
                &ob,
                |this, _| {
                    let _ = this.open_bot_subpage(OpenBotSettingsSubpage::Usage);
                },
                cx,
            ),
        ],
    );

    // Group 5: 规则 (Rules)
    let access_summary = match layout.permission_level.as_str() {
        "full" => tr_str(lang, "完全访问", "Full access"),
        "workspace" => tr_str(lang, "工作区仅限", "Workspace only"),
        _ => tr_str(lang, "先询问", "Ask first"),
    };
    let rules_group = ob_inset_group(
        tr_str(lang, "规则", "Rules"),
        &ob,
        vec![
            ob_inset_row(
                "openbot-bot-permissions-row",
                IconName::Lock,
                tr_str(lang, "权限", "Permissions"),
                Some(access_summary.to_string()),
                true,
                false,
                &ob,
                |this, _| {
                    let _ = this.open_bot_subpage(OpenBotSettingsSubpage::Permissions);
                },
                cx,
            ),
            ob_inset_row(
                "openbot-bot-folders-row",
                IconName::FolderAll,
                tr_str(lang, "文件夹", "Folders"),
                Some(format!(
                    "+{} / -{}",
                    inspector.bot_additional_folders.len(),
                    inspector.bot_excluded_folders.len()
                )),
                true,
                false,
                &ob,
                |this, _| {
                    let _ = this.open_bot_subpage(OpenBotSettingsSubpage::Folders);
                },
                cx,
            ),
            ob_inset_row(
                "openbot-bot-advanced-row",
                IconName::SettingsGear,
                tr_str(lang, "高级", "Advanced"),
                Some(if inspector.bot_busy_mode == "queue" {
                    "Queue".to_string()
                } else {
                    "Steer".to_string()
                }),
                true,
                false,
                &ob,
                |this, _| {
                    let _ = this.open_bot_subpage(OpenBotSettingsSubpage::Advanced);
                },
                cx,
            ),
        ],
    );

    let busy_banner = inspector.bot_busy_toast_message.clone().or_else(|| {
        layout.delete_history_disabled.then(|| {
            tr_str(
                lang,
                "助手正在执行任务，危险清理操作已暂时锁定",
                "Agent is currently busy; destructive history actions are disabled.",
            )
            .to_string()
        })
    });

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .when_some(copied_feedback_banner(inspector, pal), |col, b| col.child(b))
        .when_some(busy_banner, |col, msg| {
            col.child(
                div()
                    .id("openbot-bot-busy-banner")
                    .w_full()
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .bg(ob.bg_surface())
                    .border_1()
                    .border_color(ob.warning())
                    .text_size(px(12.0))
                    .text_color(ob.warning())
                    .child(msg),
            )
        })
        .child(hero_block)
        .child(identity_section)
        .child(profile_group)
        .child(brain_group)
        .child(knows_group)
        .child(does_group)
        .child(rules_group)
        .into_any_element()
}

/// Renders all drill-down subpages inside the embedded OpenBot `AgentSettingsPanel` (`UI-F16`).
pub fn render_openbot_bot_subpage(
    inspector: &ContextInspector,
    subpage: OpenBotSettingsSubpage,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContextInspector>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let snap = &inspector.bot_fusion;
    let preedit = inspector.openbot_input_preedit(cx);
    let target_bot_id = inspector
        .active_fabushi_bot
        .as_ref()
        .map(|b| b.bot_user_id.max(0) as u64)
        .filter(|&id| id > 0)
        .unwrap_or(inspector.profile.bot_id);
    let is_owner_fabushi =
        inspector.profile.is_viewer_owner && inspector.active_fabushi_bot.is_some();

    match subpage {
        OpenBotSettingsSubpage::Creations => {
            let mut col = div().w_full().flex().flex_col().gap(px(12.0));
            if inspector.published_creations.is_empty() {
                col = col.child(
                    div()
                        .w_full()
                        .p(px(14.0))
                        .rounded(px(8.0))
                        .bg(ob.bg_surface())
                        .border_1()
                        .border_color(ob.border_hairline())
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(ob.text_secondary())
                                .child(tr_str(
                                    lang,
                                    "该助手尚未发布图书、音乐或游戏作品。",
                                    "This assistant has not published any Books, Music, or Games yet.",
                                )),
                        )
                        .child(
                            div()
                                .id("openbot-creations-open-hub")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(ob.bg_control_hover())
                                .text_size(px(12.0))
                                .text_color(ob.text_primary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.pending_openbot_action =
                                            Some(OpenBotInspectorAction::OpenContentHubCreations);
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "前往我的创作中心", "Open My Creations")),
                        ),
                );
            } else {
                let rows = inspector
                    .published_creations
                    .iter()
                    .enumerate()
                    .map(|(idx, item)| {
                        ob_inset_row(
                            SharedString::from(format!("openbot-bot-creation-{idx}")),
                            IconName::FolderAll,
                            item.title.clone(),
                            Some(format!("{} · {}", item.kind_label, item.status_label)),
                            true,
                            false,
                            &ob,
                            move |this, _| {
                                let _ = this.open_published_creation_at(idx);
                            },
                            cx,
                        )
                    })
                    .collect();
                col = col.child(ob_inset_group(
                    tr_str(lang, "已发布 / 云端作品", "Published Creations"),
                    &ob,
                    rows,
                ));
            }
            col.into_any_element()
        }
        OpenBotSettingsSubpage::BackupModels => {
            let auto = inspector.bot_backup_models_auto;
            let mut list_rows: Vec<AnyElement> = Vec::new();
            for (idx, m) in inspector.bot_backup_models.iter().enumerate() {
                list_rows.push(
                    div()
                        .id(SharedString::from(format!("openbot-backup-model-{idx}")))
                        .w_full()
                        .min_h(px(40.0))
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(ob.text_primary())
                                .child(format!("{}. {m}", idx + 1)),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("openbot-backup-model-del-{idx}")))
                                .cursor_pointer()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(5.0))
                                .hover(|s| s.bg(ob.hover()))
                                .text_size(px(12.0))
                                .text_color(ob.danger())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        if idx < this.bot_backup_models.len() {
                                            this.bot_backup_models.remove(idx);
                                            cx.notify();
                                        }
                                    }),
                                )
                                .child("×"),
                        )
                        .into_any_element(),
                );
            }
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(ob_inset_group(
                    tr_str(lang, "自动故障转移", "Failover"),
                    &ob,
                    vec![
                        div()
                            .id("openbot-backup-auto-toggle")
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
                                    this.bot_backup_models_auto = !this.bot_backup_models_auto;
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .text_color(ob.text_primary())
                                    .child(tr_str(lang, "限流时自动切换备用模型", "Automatic Failover")),
                            )
                            .child(ob_switch_pill(auto, &ob))
                            .into_any_element(),
                    ],
                ))
                .when(!list_rows.is_empty(), |col| {
                    col.child(ob_inset_group(
                        &format!(
                            "{} ({}/8)",
                            tr_str(lang, "备用模型顺序", "Failover Order"),
                            inspector.bot_backup_models.len()
                        ),
                        &ob,
                        list_rows,
                    ))
                })
                .child(ob_text_input_box(
                    "openbot-backup-model-input",
                    tr_str(lang, "添加备用模型（最多 8 个）", "Add backup model (max 8)"),
                    &inspector.bot_backup_model_draft,
                    preedit.as_deref(),
                    if !inspector.ai_models.active_model.trim().is_empty() {
                        inspector.ai_models.active_model.as_str()
                    } else {
                        tr_str(lang, "输入设置中的模型 ID", "Enter model ID from Settings")
                    },
                    inspector.openbot_text_focus
                        == Some(OpenBotInspectorTextField::BotBackupModelDraft),
                    38.0,
                    inspector.bot_backup_models.len() >= 8,
                    OpenBotInspectorTextField::BotBackupModelDraft,
                    pal,
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .child(
                            div()
                                .id("openbot-backup-model-add-btn")
                                .cursor_pointer()
                                .px(px(12.0))
                                .py(px(6.0))
                                .rounded(px(7.0))
                                .bg(ob.bg_control_hover())
                                .border_1()
                                .border_color(ob.border_strong_hairline())
                                .text_size(px(12.5))
                                .text_color(ob.text_primary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.add_bot_backup_model_from_draft();
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "+ 添加备用模型", "+ Add Backup")),
                        ),
                )
                .into_any_element()
        }
        OpenBotSettingsSubpage::Instructions => div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(ob_text_input_box(
                "openbot-bot-instructions-box",
                tr_str(lang, "系统指令 (AGENTS.md)", "System Instructions (AGENTS.md)"),
                &inspector.bot_draft_persona,
                preedit.as_deref(),
                tr_str(lang, "定义助手的行为规范、语气和工作流…", "Define persona, rules, and workflows…"),
                inspector.openbot_text_focus == Some(OpenBotInspectorTextField::BotInstructions),
                160.0,
                !is_owner_fabushi,
                OpenBotInspectorTextField::BotInstructions,
                pal,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(ob.text_muted())
                            .child(format!("{} / 32,000", inspector.bot_draft_persona.chars().count())),
                    )
                    .child(
                        div()
                            .id("openbot-bot-instructions-save")
                            .cursor_pointer()
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(7.0))
                            .bg(ob.bg_control_hover())
                            .border_1()
                            .border_color(ob.border_strong_hairline())
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_primary())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.openbot_text_focus = None;
                                    this.profile.bot_persona_prompt = this.bot_draft_persona.clone();
                                    this.pending_action = Some(InspectorAction::SaveBotComputerConfig {
                                        bot_id: target_bot_id,
                                        persona_prompt: this.bot_draft_persona.clone(),
                                        preferred_model: this.bot_draft_model.clone(),
                                        approval_mode: this.bot_draft_approval_mode,
                                        allow_cross_device_control: this.bot_draft_allow_cross_device,
                                        allow_cloud_self_config: this.bot_draft_allow_cloud_self_config,
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "保存指令", "Save Instructions")),
                    ),
            )
            .into_any_element(),
        OpenBotSettingsSubpage::Memories => {
            let mem_preedit = inspector
                .memory_input
                .as_ref()
                .and_then(|c| c.read(cx).ime_preedit.clone());
            let focused = inspector.bot_config_focus == BotConfigFocusField::MemoryDraft;
            let mut rows: Vec<AnyElement> = Vec::new();
            for (idx, m) in snap.memories.iter().enumerate() {
                let mid = m.id.clone();
                rows.push(
                    div()
                        .id(SharedString::from(format!("bot-fusion-mem-row-{idx}")))
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
                                .min_w(px(0.0))
                                .text_size(px(12.5))
                                .text_color(ob.text_primary())
                                .child(m.content.clone()),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("bot-fusion-mem-del-{idx}")))
                                .cursor_pointer()
                                .px(px(5.0))
                                .text_size(px(12.0))
                                .text_color(ob.text_muted())
                                .hover(|s| s.text_color(ob.danger()))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.pending_action =
                                            Some(InspectorAction::DeleteBotMemory(mid.clone()));
                                        cx.notify();
                                    }),
                                )
                                .child("×"),
                        )
                        .into_any_element(),
                );
            }
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .id("bot-fusion-memory-input")
                        .cursor_text()
                        .w_full()
                        .min_h(px(38.0))
                        .px(px(11.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .bg(ob.bg_surface())
                        .border_1()
                        .border_color(if focused {
                            ob.accent()
                        } else {
                            ob.border_strong_hairline()
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.ensure_bot_inputs(cx);
                                this.openbot_text_focus = None;
                                this.bot_config_focus = BotConfigFocusField::MemoryDraft;
                                cx.notify();
                            }),
                        )
                        .child(render_ime_text_preview(
                            &inspector.memory_draft,
                            mem_preedit.as_deref(),
                            tr_str(lang, "输入长期记忆（回车保存）…", "Add long-term memory (Enter)…"),
                            focused,
                            false,
                            pal.text_primary,
                            pal,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("bot-fusion-memory-tidy-btn")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(6.0))
                                .bg(ob.bg_surface())
                                .border_1()
                                .border_color(ob.border_hairline())
                                .hover(|s| s.bg(ob.bg_control_hover()))
                                .text_size(px(12.0))
                                .text_color(ob.text_secondary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.pending_action = Some(InspectorAction::TidyBotMemories);
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "整理去重 (Tidy)", "Tidy Memories")),
                        )
                        .child(
                            div()
                                .id("bot-fusion-memory-add-btn")
                                .cursor_pointer()
                                .px(px(12.0))
                                .py(px(5.0))
                                .rounded(px(6.0))
                                .bg(ob.bg_control_hover())
                                .border_1()
                                .border_color(ob.border_strong_hairline())
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_primary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.submit_memory_draft();
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "+ 添加记忆", "+ Save")),
                        ),
                )
                .when(!rows.is_empty(), |col| {
                    col.child(ob_inset_group(
                        &format!(
                            "{} ({})",
                            tr_str(lang, "已保存记忆", "Saved Memories"),
                            snap.memories.len()
                        ),
                        &ob,
                        rows,
                    ))
                })
                .into_any_element()
        }
        OpenBotSettingsSubpage::Skills => {
            let mut skill_rows: Vec<AnyElement> = snap
                .skills
                .iter()
                .enumerate()
                .map(|(idx, sk)| {
                    let sid = sk.skill_id.clone();
                    let next_en = !sk.enabled;
                    div()
                        .id(SharedString::from(format!("bot-fusion-skill-toggle-{idx}")))
                        .cursor_pointer()
                        .w_full()
                        .min_h(px(40.0))
                        .px(px(12.0))
                        .hover(|s| s.bg(ob.hover()))
                        .flex()
                        .items_center()
                        .justify_between()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.pending_action = Some(InspectorAction::ToggleBotSkill {
                                    skill_id: sid.clone(),
                                    enabled: next_en,
                                });
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(13.0))
                                .text_color(ob.text_primary())
                                .truncate()
                                .child(format!("${} · {}", sk.slug, sk.name)),
                        )
                        .child(ob_switch_pill(sk.enabled, &ob))
                        .into_any_element()
                })
                .collect();
            if skill_rows.is_empty() {
                skill_rows.push(
                    div()
                        .px(px(12.0))
                        .py(px(10.0))
                        .text_size(px(12.5))
                        .text_color(ob.text_muted())
                        .child(tr_str(lang, "尚未安装技能，点击下方快速安装。", "No skills installed yet.")).into_any_element(),
                );
            }
            let quick_presets = [
                ("code-review", "代码审查", "Code Review"),
                ("web-research", "深度检索", "Web Research"),
                ("daily-digest", "每日简报", "Daily Digest"),
                ("release-notes", "发布说明", "Release Notes"),
            ];
            let mut quick_row = div().flex().flex_wrap().gap(px(6.0));
            for (idx, (slug, zh, en)) in quick_presets.into_iter().enumerate() {
                let s_owned = slug.to_string();
                let n_owned = tr_str(lang, zh, en).to_string();
                let d_owned = format!("Quick skill {slug}");
                quick_row = quick_row.child(
                    div()
                        .id(SharedString::from(format!("bot-fusion-skill-quick-{idx}")))
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(5.0))
                        .rounded(px(6.0))
                        .bg(ob.bg_surface())
                        .border_1()
                        .border_color(ob.border_hairline())
                        .hover(|s| s.bg(ob.bg_control_hover()))
                        .text_size(px(12.0))
                        .text_color(ob.text_primary())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.pending_action = Some(InspectorAction::InstallBotSkill {
                                    slug: s_owned.clone(),
                                    name: n_owned.clone(),
                                    description: d_owned.clone(),
                                });
                                cx.notify();
                            }),
                        )
                        .child(format!("+ {}", tr_str(lang, zh, en))),
                );
            }
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(ob_inset_group(tr_str(lang, "已分配技能", "Installed Skills"), &ob, skill_rows))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(
                            div()
                                .ml(px(4.0))
                                .text_size(px(12.5))
                                .text_color(ob.text_muted())
                                .child(tr_str(lang, "快速安装技能", "Quick Install")),
                        )
                        .child(quick_row),
                )
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(10.0))
                        .rounded(px(8.0))
                        .bg(ob.bg_surface())
                        .border_1()
                        .border_color(ob.border_hairline())
                        .text_size(px(12.5))
                        .text_color(ob.text_muted())
                        .child(format!(
                            "{}: {}",
                            tr_str(lang, "已连接 MCP 服务", "Connected MCP Servers"),
                            snap.mcp_servers.len()
                        )),
                )
                .into_any_element()
        }
        OpenBotSettingsSubpage::DataTables => {
            let rows: Vec<AnyElement> = if inspector.bot_data_tables.is_empty() {
                vec![
                    div()
                        .px(px(12.0))
                        .py(px(12.0))
                        .text_size(px(12.5))
                        .text_color(ob.text_muted())
                        .child(tr_str(
                            lang,
                            "暂无数据表。助手在执行结构化任务时会自动创建共享数据表。",
                            "No data tables yet.",
                        ))
                        .into_any_element(),
                ]
            } else {
                inspector
                    .bot_data_tables
                    .iter()
                    .enumerate()
                    .map(|(idx, (name, count))| {
                        ob_inset_row(
                            SharedString::from(format!("openbot-bot-table-{idx}")),
                            IconName::FileDoc,
                            name.clone(),
                            Some(format!("{count} rows")),
                            false,
                            false,
                            &ob,
                            |_, _| {},
                            cx,
                        )
                    })
                    .collect()
            };
            ob_inset_group(tr_str(lang, "共享数据表", "Data Tables"), &ob, rows)
        }
        OpenBotSettingsSubpage::Routines => {
            let mut rows: Vec<AnyElement> = snap
                .routines
                .iter()
                .enumerate()
                .map(|(idx, r)| {
                    let rid_run = r.id.clone();
                    let rid_tgl = r.id.clone();
                    let next_active = !r.active;
                    div()
                        .id(SharedString::from(format!("openbot-bot-routine-row-{idx}")))
                        .w_full()
                        .px(px(12.0))
                        .py(px(9.0))
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
                                        .text_size(px(13.0))
                                        .text_color(ob.text_primary())
                                        .truncate()
                                        .child(r.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(ob.text_muted())
                                        .child(r.schedule_label.clone()),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("bot-fusion-routine-run-{idx}")))
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
                                                this.pending_action =
                                                    Some(InspectorAction::RunBotRoutineNow {
                                                        routine_id: rid_run.clone(),
                                                    });
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(lang, "运行", "Run")),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("bot-fusion-routine-tgl-{idx}")))
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.pending_action =
                                                    Some(InspectorAction::ToggleBotRoutine {
                                                        routine_id: rid_tgl.clone(),
                                                        active: next_active,
                                                    });
                                                cx.notify();
                                            }),
                                        )
                                        .child(ob_switch_pill(r.active, &ob)),
                                ),
                        )
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
                        .child(tr_str(lang, "暂无定时任务，点击下方创建。", "No routines configured yet."))
                        .into_any_element(),
                );
            }
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(ob_inset_group(tr_str(lang, "自动化任务", "Routines"), &ob, rows))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("bot-fusion-routine-preset-daily")
                                .cursor_pointer()
                                .flex_1()
                                .py(px(7.0))
                                .rounded(px(7.0))
                                .bg(ob.bg_surface())
                                .border_1()
                                .border_color(ob.border_hairline())
                                .hover(|s| s.bg(ob.bg_control_hover()))
                                .flex()
                                .justify_center()
                                .text_size(px(12.0))
                                .text_color(ob.text_primary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.pending_action =
                                            Some(InspectorAction::CreatePresetRoutine {
                                                preset: "daily".to_string(),
                                            });
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "+ 每日任务", "+ Daily")),
                        )
                        .child(
                            div()
                                .id("bot-fusion-routine-preset-hourly")
                                .cursor_pointer()
                                .flex_1()
                                .py(px(7.0))
                                .rounded(px(7.0))
                                .bg(ob.bg_surface())
                                .border_1()
                                .border_color(ob.border_hairline())
                                .hover(|s| s.bg(ob.bg_control_hover()))
                                .flex()
                                .justify_center()
                                .text_size(px(12.0))
                                .text_color(ob.text_primary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.pending_action =
                                            Some(InspectorAction::CreatePresetRoutine {
                                                preset: "hourly".to_string(),
                                            });
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "+ 每小时任务", "+ Hourly")),
                        ),
                )
                .into_any_element()
        }
        OpenBotSettingsSubpage::Workflows => {
            let mut rows: Vec<AnyElement> = snap
                .workflows
                .iter()
                .enumerate()
                .map(|(idx, wf)| {
                    let wid = wf.id.clone();
                    div()
                        .id(SharedString::from(format!("openbot-bot-wf-{idx}")))
                        .w_full()
                        .px(px(12.0))
                        .py(px(9.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(13.0))
                                .text_color(ob.text_primary())
                                .truncate()
                                .child(wf.title.clone()),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("bot-fusion-wf-run-{idx}")))
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
                                        this.pending_action = Some(InspectorAction::RunBotWorkflow {
                                            workflow_id: wid.clone(),
                                        });
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "▶ 运行", "▶ Run")),
                        )
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
                        .child(tr_str(lang, "暂无审批工作流。", "No workflows yet."))
                        .into_any_element(),
                );
            }
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(ob_inset_group(tr_str(lang, "DAG 工作流", "Workflows"), &ob, rows))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("bot-fusion-wf-create-starter")
                                .cursor_pointer()
                                .flex_1()
                                .py(px(7.0))
                                .rounded(px(7.0))
                                .bg(ob.bg_control_hover())
                                .border_1()
                                .border_color(ob.border_strong_hairline())
                                .flex()
                                .justify_center()
                                .text_size(px(12.0))
                                .text_color(ob.text_primary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.pending_action =
                                            Some(InspectorAction::CreateStarterWorkflow);
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "+ 新建审批流", "+ New Flow")),
                        )
                        .child(
                            div()
                                .id("bot-fusion-wf-open-companion")
                                .cursor_pointer()
                                .flex_1()
                                .py(px(7.0))
                                .rounded(px(7.0))
                                .bg(ob.bg_surface())
                                .border_1()
                                .border_color(ob.border_hairline())
                                .flex()
                                .justify_center()
                                .text_size(px(12.0))
                                .text_color(ob.text_secondary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.pending_openbot_action = Some(
                                            OpenBotInspectorAction::OpenSidePanelPane(
                                                SidePanelPaneKind::Threads,
                                            ),
                                        );
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "在话题中打开", "Open in Threads")),
                        ),
                )
                .into_any_element()
        }
        OpenBotSettingsSubpage::Usage => ob_inset_group(
            tr_str(lang, "实时用量统计", "Usage Overview"),
            &ob,
            vec![
                ob_inset_row(
                    "openbot-usage-memories",
                    IconName::FolderAll,
                    tr_str(lang, "记忆条目", "Stored Memories"),
                    Some(format!("{}", snap.memories.len())),
                    false,
                    false,
                    &ob,
                    |_, _| {},
                    cx,
                ),
                ob_inset_row(
                    "openbot-usage-skills",
                    IconName::TerminalMcp,
                    tr_str(lang, "启用技能", "Active Skills"),
                    Some(format!("{}", snap.skills.iter().filter(|s| s.enabled).count())),
                    false,
                    false,
                    &ob,
                    |_, _| {},
                    cx,
                ),
                ob_inset_row(
                    "openbot-usage-steps",
                    IconName::Play,
                    tr_str(lang, "近期执行步骤", "Recent Steps"),
                    Some(format!("{}", snap.recent_steps.len())),
                    false,
                    false,
                    &ob,
                    |_, _| {},
                    cx,
                ),
                ob_inset_row(
                    "openbot-usage-queue",
                    IconName::Forward,
                    tr_str(lang, "队列消息", "Queued Messages"),
                    Some(format!("{}", snap.queue_items.len())),
                    false,
                    false,
                    &ob,
                    |_, _| {},
                    cx,
                ),
            ],
        ),
        OpenBotSettingsSubpage::Permissions => {
            let cur = inspector.bot_access_level.as_str();
            let show_widen_guard =
                inspector.pending_confirm_guard == Some(OpenBotConfirmGuard::WidenPermissionsFull);
            let mut mode_row = div().w_full().flex().items_center().gap(px(6.0));
            for (code, zh, en) in [
                ("workspace", "工作区仅限", "Workspace"),
                ("ask", "先询问", "Ask first"),
                ("full", "完全访问", "Full access"),
            ] {
                let sel = cur == code;
                let code_owned = code.to_string();
                mode_row = mode_row.child(
                    div()
                        .id(SharedString::from(format!("openbot-perm-mode-{code}")))
                        .cursor_pointer()
                        .flex_1()
                        .h(px(34.0))
                        .rounded(px(7.0))
                        .bg(if sel { ob.bg_control_active() } else { ob.bg_surface() })
                        .border_1()
                        .border_color(if sel { ob.text_muted() } else { ob.border_hairline() })
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .font_weight(if sel { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                        .text_color(if sel { ob.text_primary() } else { ob.text_muted() })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.request_bot_access_level_change(&code_owned);
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, zh, en)),
                );
            }
            let cua = inspector.bot_computer_use;
            let auto = inspector.bot_allow_automation;
            let pending_perm = inspector
                .bot_pending_access_level
                .as_deref()
                .unwrap_or("full");
            let confirm_title = if pending_perm == "ask" {
                tr_str(lang, "确认扩大访问权限？", "Confirm Wider Access?")
            } else {
                tr_str(lang, "确认授予完全访问权限？", "Confirm Full Access?")
            };
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(mode_row)
                .when(show_widen_guard, |col| {
                    col.child(ob_confirm_guard_card(
                        "openbot-perm-widen",
                        confirm_title,
                        tr_str(
                            lang,
                            "提高权限级别允许助手执行更广泛的系统或工作区操作，请确认信任此助手。",
                            "Widening permissions allows broader commands and file access. Confirm you trust this assistant.",
                        ),
                        tr_str(lang, "确认授予权限", "Confirm Permission Change"),
                        true,
                        &ob,
                        lang,
                        |this, _| {
                            this.confirm_pending_access_level_change();
                        },
                        cx,
                    ))
                })
                .child(ob_inset_group(
                    tr_str(lang, "系统能力开关", "Capabilities"),
                    &ob,
                    vec![
                        div()
                            .id("openbot-perm-cua-toggle")
                            .w_full()
                            .min_h(px(44.0))
                            .px(px(12.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(ob.hover()))
                            .flex()
                            .items_center()
                            .justify_between()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.bot_computer_use = !this.bot_computer_use;
                                    this.pending_openbot_action =
                                        Some(OpenBotInspectorAction::SetBotPermissions {
                                            access: this.bot_access_level.clone(),
                                            computer_use: this.bot_computer_use,
                                            allow_automation: this.bot_allow_automation,
                                        });
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(ob.text_primary())
                                    .child("Computer Use"),
                            )
                            .child(ob_switch_pill(cua, &ob))
                            .into_any_element(),
                        div()
                            .id("openbot-perm-automation-toggle")
                            .w_full()
                            .min_h(px(44.0))
                            .px(px(12.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(ob.hover()))
                            .flex()
                            .items_center()
                            .justify_between()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.bot_allow_automation = !this.bot_allow_automation;
                                    this.pending_openbot_action =
                                        Some(OpenBotInspectorAction::SetBotPermissions {
                                            access: this.bot_access_level.clone(),
                                            computer_use: this.bot_computer_use,
                                            allow_automation: this.bot_allow_automation,
                                        });
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(ob.text_primary())
                                    .child(tr_str(lang, "自动化与本地脚本", "Automation & Scripts")),
                            )
                            .child(ob_switch_pill(auto, &ob))
                            .into_any_element(),
                    ],
                ))
                .into_any_element()
        }
        OpenBotSettingsSubpage::Folders => {
            let mut add_rows: Vec<AnyElement> = inspector
                .bot_additional_folders
                .iter()
                .enumerate()
                .map(|(idx, path)| {
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
                                .child(path.clone()),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("openbot-add-folder-del-{idx}")))
                                .cursor_pointer()
                                .text_size(px(12.0))
                                .text_color(ob.danger())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        if idx < this.bot_additional_folders.len() {
                                            this.bot_additional_folders.remove(idx);
                                            cx.notify();
                                        }
                                    }),
                                )
                                .child("×"),
                        )
                        .into_any_element()
                })
                .collect();
            if add_rows.is_empty() {
                add_rows.push(
                    div()
                        .px(px(12.0))
                        .py(px(10.0))
                        .text_size(px(12.0))
                        .text_color(ob.text_muted())
                        .child(tr_str(lang, "未添加额外文件夹", "No additional folders"))
                        .into_any_element(),
                );
            }
            let mut excl_rows: Vec<AnyElement> = inspector
                .bot_excluded_folders
                .iter()
                .enumerate()
                .map(|(idx, path)| {
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
                                .child(path.clone()),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("openbot-excl-folder-del-{idx}")))
                                .cursor_pointer()
                                .text_size(px(12.0))
                                .text_color(ob.danger())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        if idx < this.bot_excluded_folders.len() {
                                            this.bot_excluded_folders.remove(idx);
                                            cx.notify();
                                        }
                                    }),
                                )
                                .child("×"),
                        )
                        .into_any_element()
                })
                .collect();
            if excl_rows.is_empty() {
                excl_rows.push(
                    div()
                        .px(px(12.0))
                        .py(px(10.0))
                        .text_size(px(12.0))
                        .text_color(ob.text_muted())
                        .child(tr_str(lang, "未配置排除目录", "No excluded folders"))
                        .into_any_element(),
                );
            }
            let is_excl = inspector.bot_folder_draft_excluded;
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(ob_inset_group(tr_str(lang, "允许访问的文件夹", "Additional Folders"), &ob, add_rows))
                .child(ob_inset_group(tr_str(lang, "排除文件夹", "Excluded Folders"), &ob, excl_rows))
                .child(ob_text_input_box(
                    "openbot-bot-folder-input",
                    tr_str(lang, "添加目录路径", "Add folder path"),
                    &inspector.bot_folder_draft,
                    preedit.as_deref(),
                    "/workspace/docs",
                    inspector.openbot_text_focus == Some(OpenBotInspectorTextField::BotFolderDraft),
                    38.0,
                    false,
                    OpenBotInspectorTextField::BotFolderDraft,
                    pal,
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("openbot-bot-folder-mode-tgl")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(5.0))
                                .rounded(px(6.0))
                                .bg(ob.bg_surface())
                                .border_1()
                                .border_color(ob.border_hairline())
                                .text_size(px(12.0))
                                .text_color(ob.text_secondary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.bot_folder_draft_excluded =
                                            !this.bot_folder_draft_excluded;
                                        cx.notify();
                                    }),
                                )
                                .child(if is_excl {
                                    tr_str(lang, "添加到: 排除", "Target: Excluded")
                                } else {
                                    tr_str(lang, "添加到: 允许", "Target: Additional")
                                }),
                        )
                        .child(
                            div()
                                .id("openbot-bot-folder-add-btn")
                                .cursor_pointer()
                                .px(px(12.0))
                                .py(px(5.0))
                                .rounded(px(6.0))
                                .bg(ob.bg_control_hover())
                                .border_1()
                                .border_color(ob.border_strong_hairline())
                                .text_size(px(12.0))
                                .text_color(ob.text_primary())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.add_bot_folder_from_draft();
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(lang, "+ 添加", "+ Add")),
                        ),
                )
                .into_any_element()
        }
        OpenBotSettingsSubpage::Advanced => {
            let layout = compute_openbot_bot_settings_layout(inspector, &pal);
            let steer_sel = inspector.bot_busy_mode != "queue";
            let ttl_val = inspector.settings.auto_delete_period.clone();
            let guard = inspector.pending_confirm_guard.clone();
            let bot_name = inspector.profile.name.clone();
            let busy_toast = inspector.bot_busy_toast_message.clone();
            let delete_disabled = layout.delete_history_disabled;
            let delete_history_row = div()
                .w_full()
                .when(delete_disabled, |el| el.opacity(0.45))
                .child(ob_inset_row(
                    "openbot-bot-delete-history",
                    IconName::DeleteTrash,
                    tr_str(lang, "删除历史记录", "Delete History"),
                    delete_disabled
                        .then(|| tr_str(lang, "忙碌中", "Busy").to_string()),
                    false,
                    true,
                    &ob,
                    |this, _| {
                        this.try_delete_bot_history_from_settings();
                    },
                    cx,
                ))
                .into_any_element();
            let mut danger_rows = vec![
                ob_inset_row(
                    "bot-action-new-session",
                    IconName::ComposePen,
                    tr_str(lang, "开启新对话 (/new)", "Start New Session (/new)"),
                    None,
                    false,
                    false,
                    &ob,
                    |this, _| {
                        this.try_start_new_chat_from_settings();
                    },
                    cx,
                ),
                delete_history_row,
            ];
            if is_owner_fabushi {
                danger_rows.push(ob_inset_row(
                    "openbot-bot-delete-agent",
                    IconName::DeleteTrash,
                    tr_str(lang, "删除助手", "Delete Assistant"),
                    None,
                    false,
                    true,
                    &ob,
                    |this, _| {
                        this.pending_confirm_guard = Some(OpenBotConfirmGuard::BotDeleteAgent);
                    },
                    cx,
                ));
            } else {
                danger_rows.push(ob_inset_row(
                    "bot-profile-report",
                    IconName::Close,
                    tr_str(lang, "举报助手", "Report Bot"),
                    None,
                    false,
                    true,
                    &ob,
                    move |this, _| {
                        this.pending_action = Some(InspectorAction::ReportBot(target_bot_id));
                    },
                    cx,
                ));
                danger_rows.push(ob_inset_row(
                    "bot-profile-block-toggle",
                    IconName::Close,
                    if inspector.is_contact_blocked {
                        tr_str(lang, "取消拉黑", "Unblock Bot")
                    } else {
                        tr_str(lang, "拉黑助手", "Block Bot")
                    },
                    None,
                    false,
                    true,
                    &ob,
                    move |this, _| {
                        this.pending_action = Some(InspectorAction::ToggleBlockBot(target_bot_id));
                    },
                    cx,
                ));
            }

            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .when_some(busy_toast, |col, msg| {
                    col.child(
                        div()
                            .id("openbot-bot-advanced-busy-toast")
                            .w_full()
                            .px(px(12.0))
                            .py(px(8.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.warning())
                            .text_size(px(12.0))
                            .text_color(ob.warning())
                            .child(msg),
                    )
                })
                .child(ob_inset_group(
                    tr_str(lang, "忙碌时新消息策略", "Busy Message Mode"),
                    &ob,
                    vec![
                        div()
                            .w_full()
                            .p(px(8.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id("openbot-busy-steer")
                                    .cursor_pointer()
                                    .flex_1()
                                    .h(px(32.0))
                                    .rounded(px(6.0))
                                    .bg(if steer_sel {
                                        ob.bg_control_active()
                                    } else {
                                        ob.bg_canvas()
                                    })
                                    .border_1()
                                    .border_color(if steer_sel {
                                        ob.text_muted()
                                    } else {
                                        ob.border_hairline()
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(12.5))
                                    .text_color(if steer_sel {
                                        ob.text_primary()
                                    } else {
                                        ob.text_muted()
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.bot_busy_mode = "steer".to_string();
                                            this.pending_openbot_action =
                                                Some(OpenBotInspectorAction::SetBotBusyMode(
                                                    "steer".to_string(),
                                                ));
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "立即转向 (Steer)", "Steer")),
                            )
                            .child(
                                div()
                                    .id("openbot-busy-queue")
                                    .cursor_pointer()
                                    .flex_1()
                                    .h(px(32.0))
                                    .rounded(px(6.0))
                                    .bg(if !steer_sel {
                                        ob.bg_control_active()
                                    } else {
                                        ob.bg_canvas()
                                    })
                                    .border_1()
                                    .border_color(if !steer_sel {
                                        ob.text_muted()
                                    } else {
                                        ob.border_hairline()
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(12.5))
                                    .text_color(if !steer_sel {
                                        ob.text_primary()
                                    } else {
                                        ob.text_muted()
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.bot_busy_mode = "queue".to_string();
                                            this.pending_openbot_action =
                                                Some(OpenBotInspectorAction::SetBotBusyMode(
                                                    "queue".to_string(),
                                                ));
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(lang, "排队等待 (Queue)", "Queue")),
                            )
                            .into_any_element(),
                        ob_inset_row(
                            "bot-settings-ttl-row",
                            IconName::SettingsGear,
                            tr_str(lang, "自动清理消息", "Auto-Delete TTL"),
                            Some(ttl_val),
                            true,
                            false,
                            &ob,
                            |this, _| {
                                let next = match this.settings.auto_delete_period.as_str() {
                                    "Off" => "24 Hours",
                                    "24 Hours" => "7 Days",
                                    "7 Days" => "30 Days",
                                    _ => "Off",
                                };
                                this.set_auto_delete_period(next);
                                this.pending_action =
                                    Some(InspectorAction::ChangeAutoDelete(next.to_string()));
                            },
                            cx,
                        ),
                    ],
                ))
                .child(ob_inset_group(tr_str(lang, "会话与危险操作", "Danger Zone"), &ob, danger_rows))
                .when_some(guard, |col, g| {
                    let is_danger = inspector
                        .active_confirm_dialog_state()
                        .map(|s| s.destructive)
                        .unwrap_or_else(|| g.is_destructive());
                    match g {
                        OpenBotConfirmGuard::BotNewSession => col.child(ob_confirm_guard_card(
                            "openbot-guard-bot-new",
                            tr_str(lang, "确认开启新对话？", "Start New Session?"),
                            tr_str(
                                lang,
                                "将向助手发送 /new 重置当前上下文窗口。",
                                "Sends /new to reset the current context window.",
                            ),
                            tr_str(lang, "确认开启", "Start New"),
                            is_danger,
                            &ob,
                            lang,
                            |this, _| {
                                this.pending_action =
                                    Some(InspectorAction::RunBotCommand("/new".to_string()));
                            },
                            cx,
                        )),
                        OpenBotConfirmGuard::BotDeleteHistory => col.child(ob_confirm_guard_card(
                            "openbot-guard-bot-delhist",
                            tr_str(lang, "确认删除历史记录？", "Delete Chat History?"),
                            tr_str(
                                lang,
                                "此操作会清空当前与该助手的全部消息历史，无法撤销。",
                                "Clears all messages in this conversation permanently.",
                            ),
                            tr_str(lang, "确认删除历史", "Delete History"),
                            is_danger,
                            &ob,
                            lang,
                            |this, _| {
                                this.pending_openbot_action =
                                    Some(OpenBotInspectorAction::DeleteChatHistory);
                            },
                            cx,
                        )),
                        OpenBotConfirmGuard::BotDeleteAgent => col.child(ob_confirm_guard_card(
                            "openbot-guard-bot-delete",
                            tr_str(lang, "确认删除该助手？", "Delete Assistant?"),
                            tr_str(
                                lang,
                                "删除后将移除该助手对话及其本地工作区状态，无法撤销。",
                                "Removes this assistant and its local workspace state permanently.",
                            ),
                            tr_str(lang, "确认删除助手", "Delete Assistant"),
                            is_danger,
                            &ob,
                            lang,
                            move |this, _| {
                                this.pending_openbot_action =
                                    Some(OpenBotInspectorAction::DeleteBot {
                                        bot_id: target_bot_id,
                                        bot_name: bot_name.clone(),
                                    });
                            },
                            cx,
                        )),
                        _ => col,
                    }
                })
                .into_any_element()
        }
        _ => render_openbot_bot_settings_root(inspector, pal, lang, cx),
    }
}
