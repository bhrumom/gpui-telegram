//! Interactive Orca Bots dialogs & rooms:
//! - 24-Template Gallery (6 categories) + 13 Bundled Skills (`Schedules install disabled`)
//! - Multi-Bot Group Rooms (sub-threads, `Bot|All|Mentions`, `@mention`, paused mail hold, remote asks)
//! - Bot Package Export/Import + 5-Class Scanner (`secret_blocked` & risk findings)
//! - E2EE Team Share (routing peer identity verification through `SecuritySurfaceModal`)
//! - Cross-App Import (`OpenClaw`/`Hermes`/`OpenBot`/`ExportPaste` + `BOT_IMPORT_EXPORT_PROMPT`)
//! - Transactional `"Hand to bot…"` Dialog (`suggest_hand_over_bot` + CAS rollback banner)

use gpui::prelude::FluentBuilder;
use gpui::*;
use orca_bots::{BotImportEngine, BotTemplateInstaller};
use orca_core::bot_ecosystem::{
    BotImportSource, BotTemplateCategory, HandOverOutcome,
};
use orca_core::bot_groups::BotGroupReplyMode;

use crate::i18n::tr_str;
use crate::theme::{IconName, TdesktopThemePalette, tg_icon};

use super::bots_state::{BotDialogState, BotsInputTarget};
use super::bots_workspace::OrcaBotsWorkspace;

/// Renders whichever `BotDialogState` overlay/panel is currently open, or `None`.
pub fn render_active_bot_dialog(
    ws: &OrcaBotsWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> Option<AnyElement> {
    match &ws.state.dialog {
        BotDialogState::None => None,
        BotDialogState::TemplateGallery { .. } => {
            Some(render_template_gallery_dialog(ws, pal, lang, cx))
        }
        BotDialogState::GroupRoom { .. } => Some(render_group_room_dialog(ws, pal, lang, cx)),
        BotDialogState::CreateGroup { .. } => Some(render_create_group_dialog(ws, pal, lang, cx)),
        BotDialogState::PackageManager { .. } => Some(render_package_dialog(ws, pal, lang, cx)),
        BotDialogState::TeamShare { .. } => Some(render_team_share_dialog(ws, pal, lang, cx)),
        BotDialogState::CrossAppImport { .. } => {
            Some(render_cross_app_import_dialog(ws, pal, lang, cx))
        }
        BotDialogState::HandOver { .. } => Some(render_hand_over_dialog(ws, pal, lang, cx)),
    }
}

fn dialog_shell(
    id: &'static str,
    title: String,
    subtitle: String,
    body: AnyElement,
    pal: TdesktopThemePalette,
    scroll: &ScrollHandle,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    div()
        .id(id)
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .occlude()
        .bg(rgba(0x00000099))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .w(px(740.0))
                .max_w(px(820.0))
                .max_h(px(640.0))
                .rounded(px(12.0))
                .bg(rgb(pal.modal_bg))
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .shadow(pal.popover_shadows())
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(16.0))
                        .py(px(12.0))
                        .bg(rgb(pal.top_bar_bg))
                        .border_b_1()
                        .border_color(pal.separator_rgba())
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(pal.text_secondary))
                                        .child(subtitle),
                                ),
                        )
                        .child(
                            div()
                                .id("orca-dialog-close-btn")
                                .cursor_pointer()
                                .p(px(5.0))
                                .rounded(px(6.0))
                                .bg(rgb(pal.input_bg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.close_dialog(cx);
                                    }),
                                )
                                .child(tg_icon(IconName::Close, 14.0, pal.icon_fg)),
                        ),
                )
                .child(
                    div()
                        .id("orca-dialog-body-scroll")
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .track_scroll(scroll)
                        .p(px(14.0))
                        .child(body),
                ),
        )
        .into_any_element()
}

fn render_template_gallery_dialog(
    ws: &OrcaBotsWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let (selected_cat, show_skills, custom_name, last_installed) = match &ws.state.dialog {
        BotDialogState::TemplateGallery {
            selected_category,
            show_skills,
            custom_name,
            last_installed,
        } => (*selected_category, *show_skills, custom_name.clone(), last_installed.clone()),
        _ => (None, false, String::new(), None),
    };
    let templates = ws.state.filter_templates(selected_cat);
    let skills = BotTemplateInstaller::list_bundled_skills();
    let name_active = ws.state.active_input == BotsInputTarget::TemplateCustomName;
    let name_text = if custom_name.is_empty() && !name_active {
        tr_str(lang, "可选自定义机器人名称…", "Optional custom bot name…").to_string()
    } else if name_active {
        format!("{custom_name}|")
    } else {
        custom_name
    };

    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .child(cat_tab_btn(
                    "tmpl-cat-all",
                    "All (24)",
                    !show_skills && selected_cat.is_none(),
                    pal,
                    cx.listener(|ws, _, _, cx| {
                        cx.stop_propagation();
                        if let BotDialogState::TemplateGallery {
                            ref mut selected_category,
                            ref mut show_skills,
                            ..
                        } = ws.state.dialog
                        {
                            *selected_category = None;
                            *show_skills = false;
                            cx.notify();
                        }
                    }),
                ))
                .children(BotTemplateCategory::ALL.into_iter().map(|cat| {
                    let active = !show_skills && selected_cat == Some(cat);
                    cat_tab_btn(
                        cat.as_str(),
                        cat.label(),
                        active,
                        pal,
                        cx.listener(move |ws, _, _, cx| {
                            cx.stop_propagation();
                            if let BotDialogState::TemplateGallery {
                                ref mut selected_category,
                                ref mut show_skills,
                                ..
                            } = ws.state.dialog
                            {
                                *selected_category = Some(cat);
                                *show_skills = false;
                                cx.notify();
                            }
                        }),
                    )
                }))
                .child(cat_tab_btn(
                    "tmpl-skills-tab",
                    "Skills (13)",
                    show_skills,
                    pal,
                    cx.listener(|ws, _, _, cx| {
                        cx.stop_propagation();
                        if let BotDialogState::TemplateGallery {
                            ref mut show_skills, ..
                        } = ws.state.dialog
                        {
                            *show_skills = true;
                            cx.notify();
                        }
                    }),
                )),
        )
        .child(
            div()
                .id("tmpl-custom-name-input")
                .cursor_text()
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if name_active {
                    rgb(pal.accent)
                } else {
                    pal.hairline_border_rgba()
                })
                .text_xs()
                .text_color(rgb(pal.text_primary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|ws, _, window, cx| {
                        cx.stop_propagation();
                        ws.set_active_input(BotsInputTarget::TemplateCustomName, window, cx);
                    }),
                )
                .child(name_text),
        )
        .when_some(last_installed, |el, res| {
            el.child(
                div()
                    .p(px(8.0))
                    .rounded(px(6.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.14))
                    .text_xs()
                    .text_color(rgb(pal.success))
                    .child(format!(
                        "Installed '{}' · {} skill(s) · {} suggested schedule(s) [all disabled]",
                        res.bot.name.as_str(),
                        res.installed_skills.len(),
                        res.suggested_schedules.len()
                    )),
            )
        })
        .when(!show_skills, |el| {
            el.children(templates.into_iter().map(|t| {
                let tid = t.id;
                div()
                    .id(SharedString::from(format!("tmpl-card-{tid}")))
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.card_bg))
                    .border_1()
                    .border_color(pal.hairline_border_rgba())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(format!("{} ({})", t.name, t.category.label())),
                                    )
                                    .child(
                                        div()
                                            .px(px(5.0))
                                            .py(px(1.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(pal.input_bg))
                                            .text_xs()
                                            .text_color(rgb(pal.text_secondary))
                                            .child("Schedules install disabled"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(pal.text_secondary))
                                    .child(t.tagline),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("tmpl-install-{tid}")))
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(5.0))
                            .bg(rgb(pal.accent))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.active_button_fg))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |ws, _, _, cx| {
                                    cx.stop_propagation();
                                    ws.install_template(tid, cx);
                                }),
                            )
                            .child(tr_str(lang, "安装", "Install")),
                    )
            }))
        })
        .when(show_skills, |el| {
            el.children(skills.iter().map(|sk| {
                let slug = sk.slug;
                div()
                    .id(SharedString::from(format!("skill-card-{slug}")))
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.card_bg))
                    .border_1()
                    .border_color(pal.hairline_border_rgba())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(format!("{} ({slug})", sk.title)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(pal.text_secondary))
                                    .child(sk.summary),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("skill-install-{slug}")))
                            .cursor_pointer()
                            .px(px(9.0))
                            .py(px(5.0))
                            .rounded(px(5.0))
                            .bg(rgb(pal.accent))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.active_button_fg))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |ws, _, _, cx| {
                                    cx.stop_propagation();
                                    ws.install_bundled_skill(slug, cx);
                                }),
                            )
                            .child(tr_str(lang, "安装技能", "Install Skill")),
                    )
            }))
        })
        .into_any_element();

    dialog_shell(
        "orca-template-gallery-dialog",
        tr_str(lang, "机器人模板库 (24) 与内置技能 (13)", "Bot Template Gallery (24) & Bundled Skills (13)").to_string(),
        "Safety invariant: all suggested schedules and Look Around presets install disabled.".to_string(),
        body,
        pal,
        &ws.dialog_scroll,
        cx,
    )
}

fn cat_tab_btn(
    id: &'static str,
    label: &'static str,
    active: bool,
    pal: TdesktopThemePalette,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .cursor_pointer()
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(5.0))
        .bg(if active {
            rgb(pal.accent)
        } else {
            rgb(pal.input_bg)
        })
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if active {
            rgb(pal.active_button_fg)
        } else {
            rgb(pal.text_primary)
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
        .into_any_element()
}

fn render_create_group_dialog(
    ws: &OrcaBotsWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let (name, selected_ids) = match &ws.state.dialog {
        BotDialogState::CreateGroup {
            name,
            selected_bot_ids,
        } => (name.clone(), selected_bot_ids.clone()),
        _ => (String::new(), Vec::new()),
    };
    let active = ws.state.active_input == BotsInputTarget::NewGroupName;
    let display = if name.is_empty() && !active {
        tr_str(lang, "输入多机器人群组名称…", "Multi-bot group room name…").to_string()
    } else if active {
        format!("{name}|")
    } else {
        name
    };

    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .id("orca-new-group-name")
                .cursor_text()
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(6.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if active {
                    rgb(pal.accent)
                } else {
                    pal.hairline_border_rgba()
                })
                .text_xs()
                .text_color(rgb(pal.text_primary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|ws, _, window, cx| {
                        cx.stop_propagation();
                        ws.set_active_input(BotsInputTarget::NewGroupName, window, cx);
                    }),
                )
                .child(display),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .children(ws.state.bots.iter().map(|b| {
                    let chosen = selected_ids.iter().any(|id| id == &b.id);
                    let bid = b.id.clone();
                    div()
                        .id(SharedString::from(format!("grp-pick-bot-{}", b.id)))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(5.0))
                        .bg(if chosen {
                            rgb(pal.accent)
                        } else {
                            rgb(pal.input_bg)
                        })
                        .text_xs()
                        .text_color(if chosen {
                            rgb(pal.active_button_fg)
                        } else {
                            rgb(pal.text_primary)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                if let BotDialogState::CreateGroup {
                                    ref mut selected_bot_ids,
                                    ..
                                } = ws.state.dialog
                                {
                                    if let Some(pos) =
                                        selected_bot_ids.iter().position(|x| x == &bid)
                                    {
                                        selected_bot_ids.remove(pos);
                                    } else {
                                        selected_bot_ids.push(bid.clone());
                                    }
                                    cx.notify();
                                }
                            }),
                        )
                        .child(b.name.as_str().to_string())
                })),
        )
        .child(
            div()
                .id("orca-create-group-submit")
                .cursor_pointer()
                .px(px(12.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .bg(rgb(pal.accent))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.active_button_fg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|ws, _, _, cx| {
                        cx.stop_propagation();
                        ws.submit_create_group(cx);
                    }),
                )
                .child(tr_str(lang, "创建群组房间", "Create Group Room")),
        )
        .into_any_element();

    dialog_shell(
        "orca-create-group-dialog",
        tr_str(lang, "新建多机器人群组房间", "Create Multi-Bot Group Room").to_string(),
        "SQLite WAL rollback-safe group room supporting sub-threads, @mentions, and remote members."
            .to_string(),
        body,
        pal,
        &ws.dialog_scroll,
        cx,
    )
}

fn render_group_room_dialog(
    ws: &OrcaBotsWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let (group_id, selected_thread_id, draft, new_thread_title) = match &ws.state.dialog {
        BotDialogState::GroupRoom {
            group_id,
            selected_thread_id,
            draft,
            new_thread_title,
        } => (
            group_id.clone(),
            selected_thread_id.clone(),
            draft.clone(),
            new_thread_title.clone(),
        ),
        _ => (String::new(), None, String::new(), String::new()),
    };
    let group_opt = ws.state.groups.iter().find(|g| g.id == group_id).cloned();
    let title = group_opt
        .as_ref()
        .map(|g| format!("Group Room: {}", g.name))
        .unwrap_or_else(|| "Group Room".to_string());
    let lead_bot_id = group_opt
        .as_ref()
        .and_then(|g| g.member_bot_ids.first().cloned())
        .unwrap_or_default();
    let current_mode = group_opt
        .as_ref()
        .map(|g| g.reply_mode.clone())
        .unwrap_or(BotGroupReplyMode::Mentions);
    let draft_active = ws.state.active_input == BotsInputTarget::GroupMessageDraft;
    let thread_active = ws.state.active_input == BotsInputTarget::NewThreadTitle;

    let gid_all = group_id.clone();
    let gid_men = group_id.clone();
    let gid_bot = group_id.clone();
    let gid_flush = group_id.clone();
    let gid_thread = group_id.clone();
    let gid_post = group_id.clone();

    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(cat_tab_btn(
                            "grp-mode-mentions",
                            "Mentions",
                            matches!(current_mode, BotGroupReplyMode::Mentions),
                            pal,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.set_group_reply_mode(&gid_men, BotGroupReplyMode::Mentions, cx);
                            }),
                        ))
                        .child(cat_tab_btn(
                            "grp-mode-all",
                            "All",
                            matches!(current_mode, BotGroupReplyMode::All),
                            pal,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.set_group_reply_mode(&gid_all, BotGroupReplyMode::All, cx);
                            }),
                        ))
                        .when(!lead_bot_id.is_empty(), |row| {
                            let lead = lead_bot_id.clone();
                            row.child(cat_tab_btn(
                                "grp-mode-bot",
                                "Lead Bot",
                                matches!(current_mode, BotGroupReplyMode::Bot { .. }),
                                pal,
                                cx.listener(move |ws, _, _, cx| {
                                    cx.stop_propagation();
                                    ws.set_group_reply_mode(
                                        &gid_bot,
                                        BotGroupReplyMode::Bot {
                                            bot_id: lead.clone(),
                                        },
                                        cx,
                                    );
                                }),
                            ))
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .child(format!(
                                    "Held mail: {} · Remote asks: {}",
                                    ws.state.group_pending_count,
                                    ws.state.group_remote_asks.len()
                                )),
                        )
                        .when(ws.state.group_pending_count > 0, |r| {
                            r.child(
                                div()
                                    .id("grp-flush-mail-btn")
                                    .cursor_pointer()
                                    .px(px(7.0))
                                    .py(px(3.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(pal.accent))
                                    .text_xs()
                                    .text_color(rgb(pal.active_button_fg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |ws, _, _, cx| {
                                            cx.stop_propagation();
                                            ws.flush_group_pending_mail(&gid_flush, cx);
                                        }),
                                    )
                                    .child("Flush Held"),
                            )
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(5.0))
                .child(cat_tab_btn(
                    "grp-thread-root",
                    "Main Room",
                    selected_thread_id.is_none(),
                    pal,
                    cx.listener(|ws, _, _, cx| {
                        cx.stop_propagation();
                        ws.select_group_thread(None, cx);
                    }),
                ))
                .children(ws.state.group_threads.iter().map(|th| {
                    let active = selected_thread_id.as_deref() == Some(th.id.as_str());
                    let tid = th.id.clone();
                    div()
                        .id(SharedString::from(format!("grp-thread-{}", th.id)))
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(if active {
                            rgb(pal.accent)
                        } else {
                            rgb(pal.input_bg)
                        })
                        .text_xs()
                        .text_color(if active {
                            rgb(pal.active_button_fg)
                        } else {
                            rgb(pal.text_primary)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.select_group_thread(Some(tid.clone()), cx);
                            }),
                        )
                        .child(th.title.clone())
                }))
                .child(
                    div()
                        .id("grp-new-thread-input")
                        .w(px(160.0))
                        .cursor_text()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if thread_active {
                            rgb(pal.accent)
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, window, cx| {
                                cx.stop_propagation();
                                ws.set_active_input(BotsInputTarget::NewThreadTitle, window, cx);
                            }),
                        )
                        .child(if new_thread_title.is_empty() && !thread_active {
                            "New thread title…".to_string()
                        } else if thread_active {
                            format!("{new_thread_title}|")
                        } else {
                            new_thread_title
                        }),
                )
                .child(
                    div()
                        .id("grp-new-thread-btn")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.create_group_thread(&gid_thread, cx);
                            }),
                        )
                        .child("+ Thread"),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(5.0))
                .children(ws.state.bots.iter().map(|b| {
                    let mention_token = format!("@{} ", b.name.as_str());
                    div()
                        .id(SharedString::from(format!("grp-mention-{}", b.id)))
                        .cursor_pointer()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                        .text_xs()
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                if let BotDialogState::GroupRoom { ref mut draft, .. } =
                                    ws.state.dialog
                                {
                                    draft.push_str(&mention_token);
                                    cx.notify();
                                }
                            }),
                        )
                        .child(format!("@{}", b.name.as_str()))
                })),
        )
        .children(ws.state.group_messages.iter().map(|m| {
            div()
                .p(px(8.0))
                .rounded(px(6.0))
                .bg(rgb(pal.card_bg))
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "#{} · {} · answered={} · handed_off={}",
                            m.sequence,
                            m.sender_kind,
                            m.answered_by.len(),
                            m.handed_off
                        )),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .child(m.text.clone()),
                )
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("grp-msg-draft-input")
                        .flex_1()
                        .cursor_text()
                        .px(px(9.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if draft_active {
                            rgb(pal.accent)
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, window, cx| {
                                cx.stop_propagation();
                                ws.set_active_input(BotsInputTarget::GroupMessageDraft, window, cx);
                            }),
                        )
                        .child(if draft.is_empty() && !draft_active {
                            tr_str(lang, "输入群消息并 @提及机器人…", "Post to group room (@mention bots)…").to_string()
                        } else if draft_active {
                            format!("{draft}|")
                        } else {
                            draft
                        }),
                )
                .child(
                    div()
                        .id("grp-msg-send-btn")
                        .cursor_pointer()
                        .px(px(11.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.post_group_draft(&gid_post, cx);
                            }),
                        )
                        .child(tr_str(lang, "发送", "Send")),
                ),
        )
        .into_any_element();

    dialog_shell(
        "orca-group-room-dialog",
        title,
        format!(
            "Members: {} local, {} remote",
            group_opt.as_ref().map(|g| g.member_bot_ids.len()).unwrap_or(0),
            group_opt.as_ref().map(|g| g.remote_members.len()).unwrap_or(0)
        ),
        body,
        pal,
        &ws.dialog_scroll,
        cx,
    )
}

fn render_package_dialog(
    ws: &OrcaBotsWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let (exported_b64, file_count, scan_report, runnable_warnings, import_b64, last_imported) =
        match &ws.state.dialog {
            BotDialogState::PackageManager {
                exported_zip_b64,
                exported_file_count,
                scan_report,
                runnable_warnings,
                import_zip_b64,
                last_imported_bot,
            } => (
                exported_zip_b64.clone(),
                *exported_file_count,
                scan_report.clone(),
                runnable_warnings.clone(),
                import_zip_b64.clone(),
                last_imported_bot.clone(),
            ),
            _ => (None, 0, None, Vec::new(), String::new(), None),
        };
    let active = ws.state.active_input == BotsInputTarget::PackageImportBase64;
    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("pkg-export-btn")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.export_selected_bot_package(cx);
                            }),
                        )
                        .child(tr_str(lang, "导出当前机器人 (.zip)", "Export Selected Bot (.zip)")),
                )
                .when_some(exported_b64.clone(), |row, b64| {
                    row.child(
                        div()
                            .id("pkg-copy-b64-btn")
                            .cursor_pointer()
                            .px(px(9.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .bg(rgb(pal.input_bg))
                            .text_xs()
                            .text_color(rgb(pal.text_primary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |ws, _, _, cx| {
                                    cx.stop_propagation();
                                    ws.copy_text_to_clipboard(
                                        b64.clone(),
                                        "Package base64 copied",
                                        cx,
                                    );
                                }),
                            )
                            .child(format!("Copy Base64 ({file_count} files)")),
                    )
                }),
        )
        .when_some(scan_report, |el, report| {
            let blocked = report.blocking;
            el.child(
                div()
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.card_bg))
                    .border_1()
                    .border_color(if blocked {
                        rgb(pal.danger)
                    } else {
                        rgb(pal.success)
                    })
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(if blocked {
                                rgb(pal.danger)
                            } else {
                                rgb(pal.success)
                            })
                            .child(if blocked {
                                format!(
                                    "5-Class Scanner: secret_blocked ({} finding(s) blocked export)",
                                    report.findings.len()
                                )
                            } else {
                                format!(
                                    "5-Class Scanner: CLEAN (0 blocking secrets · {} non-blocking risk warning(s))",
                                    report.risk_findings.len()
                                )
                            }),
                    )
                    .children(report.findings.iter().take(6).map(|f| {
                        div()
                            .text_xs()
                            .text_color(rgb(pal.danger))
                            .child(format!(
                                "[{}] line {}: {} ({})",
                                f.class.as_str(),
                                f.line,
                                f.label,
                                f.preview
                            ))
                    }))
                    .children(report.risk_findings.iter().take(6).map(|rf| {
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(format!("[{}] line {}: {}", rf.kind.as_str(), rf.line, rf.detail))
                    }))
                    .children(runnable_warnings.iter().map(|rw| {
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(format!("[runnable] {rw}"))
                    })),
            )
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("pkg-import-b64-input")
                        .flex_1()
                        .cursor_text()
                        .px(px(9.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if active {
                            rgb(pal.accent)
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, window, cx| {
                                cx.stop_propagation();
                                ws.set_active_input(
                                    BotsInputTarget::PackageImportBase64,
                                    window,
                                    cx,
                                );
                            }),
                        )
                        .child(if import_b64.is_empty() && !active {
                            "Paste Bot Package v1 base64 zip…".to_string()
                        } else if active {
                            format!("{}|", import_b64.chars().take(64).collect::<String>())
                        } else {
                            format!("{}… ({} chars)", import_b64.chars().take(40).collect::<String>(), import_b64.len())
                        }),
                )
                .child(
                    div()
                        .id("pkg-import-btn")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.import_bot_package_b64(cx);
                            }),
                        )
                        .child(tr_str(lang, "导入包", "Import Package")),
                ),
        )
        .when_some(last_imported, |el, name| {
            el.child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.success))
                    .child(format!("Imported bot: {name}")),
            )
        })
        .into_any_element();

    dialog_shell(
        "orca-package-dialog",
        tr_str(lang, "Bot 打包导出 / 导入与 5 类密钥扫描", "Bot Package v1 & 5-Class Security Scanner").to_string(),
        "Blocks export on any secret finding (`secret_blocked`) and excludes tools, webhooks, and credentials."
            .to_string(),
        body,
        pal,
        &ws.dialog_scroll,
        cx,
    )
}

fn render_team_share_dialog(
    ws: &OrcaBotsWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let my_id_json = serde_json::to_string_pretty(&ws.runtime.bots().team_share().public_identity())
        .unwrap_or_default();
    let (peer_json, sealed_json, incoming_json, state_label) = match &ws.state.dialog {
        BotDialogState::TeamShare {
            peer_identity_json,
            sealed_offer_json,
            incoming_offer_json,
            offer_state_label,
            ..
        } => (
            peer_identity_json.clone(),
            sealed_offer_json.clone(),
            incoming_offer_json.clone(),
            offer_state_label.clone(),
        ),
        _ => (String::new(), None, String::new(), None),
    };
    let peer_active = ws.state.active_input == BotsInputTarget::E2eePeerIdentityJson;
    let offer_active = ws.state.active_input == BotsInputTarget::E2eeOfferJson;
    let my_id_copy = my_id_json.clone();

    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .p(px(8.0))
                .rounded(px(6.0))
                .bg(rgb(pal.card_bg))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "Host E2EE Identity (X25519 + Ed25519): {}",
                            ws.runtime.bots().team_share().host_id()
                        )),
                )
                .child(
                    div()
                        .id("e2ee-copy-my-id")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.copy_text_to_clipboard(
                                    my_id_copy.clone(),
                                    "Host E2EE public identity copied",
                                    cx,
                                );
                            }),
                        )
                        .child("Copy Public Identity JSON"),
                ),
        )
        .child(
            div()
                .id("e2ee-peer-id-input")
                .cursor_text()
                .px(px(9.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if peer_active {
                    rgb(pal.accent)
                } else {
                    pal.hairline_border_rgba()
                })
                .text_xs()
                .text_color(rgb(pal.text_primary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|ws, _, window, cx| {
                        cx.stop_propagation();
                        ws.set_active_input(BotsInputTarget::E2eePeerIdentityJson, window, cx);
                    }),
                )
                .child(if peer_json.is_empty() && !peer_active {
                    "Paste peer TeamSharePublicIdentity JSON…".to_string()
                } else if peer_active {
                    format!("{peer_json}|")
                } else {
                    peer_json
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("e2ee-seal-btn")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.seal_team_share_offer(cx);
                            }),
                        )
                        .child(tr_str(lang, "封装 E2EE 邀请", "Seal E2EE Offer")),
                )
                .when_some(sealed_json, |row, sj| {
                    row.child(
                        div()
                            .id("e2ee-copy-offer")
                            .cursor_pointer()
                            .px(px(9.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .bg(rgb(pal.input_bg))
                            .text_xs()
                            .text_color(rgb(pal.success))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |ws, _, _, cx| {
                                    cx.stop_propagation();
                                    ws.copy_text_to_clipboard(
                                        sj.clone(),
                                        "Sealed E2EE offer copied",
                                        cx,
                                    );
                                }),
                            )
                            .child("Copy Sealed Offer JSON"),
                    )
                }),
        )
        .child(
            div()
                .id("e2ee-incoming-offer-input")
                .cursor_text()
                .px(px(9.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if offer_active {
                    rgb(pal.accent)
                } else {
                    pal.hairline_border_rgba()
                })
                .text_xs()
                .text_color(rgb(pal.text_primary))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|ws, _, window, cx| {
                        cx.stop_propagation();
                        ws.set_active_input(BotsInputTarget::E2eeOfferJson, window, cx);
                    }),
                )
                .child(if incoming_json.is_empty() && !offer_active {
                    "Paste incoming SealedTeamShareOffer JSON…".to_string()
                } else if offer_active {
                    format!("{incoming_json}|")
                } else {
                    incoming_json
                }),
        )
        .child(
            div()
                .id("e2ee-verify-open-btn")
                .cursor_pointer()
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .bg(rgb(pal.success))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.active_button_fg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|ws, _, _, cx| {
                        cx.stop_propagation();
                        ws.open_e2ee_verification_modal(cx);
                    }),
                )
                .child(tr_str(
                    lang,
                    "通过 SecuritySurface 验证对端并接收",
                    "Verify Peer via SecuritySurface & Open Offer",
                )),
        )
        .when_some(state_label, |el, lbl| {
            el.child(div().text_xs().text_color(rgb(pal.success)).child(lbl))
        })
        .into_any_element();

    dialog_shell(
        "orca-team-share-dialog",
        tr_str(lang, "端到端加密团队共享 (X25519 + Ed25519)", "E2EE Team Share (X25519 + Ed25519)").to_string(),
        "Peer identity verification ALWAYS routes through the non-themable Host SecuritySurface modal."
            .to_string(),
        body,
        pal,
        &ws.dialog_scroll,
        cx,
    )
}

fn render_cross_app_import_dialog(
    ws: &OrcaBotsWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let (selected_source, discovered, paste_text, last_imported) = match &ws.state.dialog {
        BotDialogState::CrossAppImport {
            selected_source,
            discovered,
            paste_text,
            last_imported_bot,
        } => (
            *selected_source,
            discovered.clone(),
            paste_text.clone(),
            last_imported_bot.clone(),
        ),
        _ => (None, Vec::new(), String::new(), None),
    };
    let paste_active = ws.state.active_input == BotsInputTarget::ImportPasteText;
    let filtered: Vec<_> = discovered
        .iter()
        .enumerate()
        .filter(|(_, c)| selected_source.is_none_or(|src| c.source == src))
        .collect();

    let body = div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .child(cat_tab_btn(
                    "imp-src-all",
                    "All Sources",
                    selected_source.is_none(),
                    pal,
                    cx.listener(|ws, _, _, cx| {
                        cx.stop_propagation();
                        if let BotDialogState::CrossAppImport {
                            ref mut selected_source,
                            ..
                        } = ws.state.dialog
                        {
                            *selected_source = None;
                            cx.notify();
                        }
                    }),
                ))
                .children(BotImportSource::ALL.into_iter().map(|src| {
                    let active = selected_source == Some(src);
                    cat_tab_btn(
                        src.as_str(),
                        src.as_str(),
                        active,
                        pal,
                        cx.listener(move |ws, _, _, cx| {
                            cx.stop_propagation();
                            if let BotDialogState::CrossAppImport {
                                ref mut selected_source,
                                ..
                            } = ws.state.dialog
                            {
                                *selected_source = Some(src);
                                cx.notify();
                            }
                        }),
                    )
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("imp-discover-btn")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.discover_cross_app_imports(cx);
                            }),
                        )
                        .child(tr_str(lang, "扫描本机 OpenClaw / Hermes / OpenBot", "Discover Local Bots")),
                )
                .child(
                    div()
                        .id("imp-copy-prompt-btn")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.copy_text_to_clipboard(
                                    BotImportEngine::export_prompt().to_string(),
                                    "BOT_IMPORT_EXPORT_PROMPT copied",
                                    cx,
                                );
                            }),
                        )
                        .child("Copy Export Prompt"),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("imp-paste-input")
                        .flex_1()
                        .cursor_text()
                        .px(px(9.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if paste_active {
                            rgb(pal.accent)
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, window, cx| {
                                cx.stop_propagation();
                                ws.set_active_input(BotsInputTarget::ImportPasteText, window, cx);
                            }),
                        )
                        .child(if paste_text.is_empty() && !paste_active {
                            "Paste foreign bot export markdown reply (<=200KB, auto-redacts secrets)…".to_string()
                        } else if paste_active {
                            format!("{paste_text}|")
                        } else {
                            paste_text
                        }),
                )
                .child(
                    div()
                        .id("imp-parse-paste-btn")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.parse_import_paste(cx);
                            }),
                        )
                        .child("Parse Paste"),
                ),
        )
        .when_some(last_imported, |el, name| {
            el.child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.success))
                    .child(format!("Imported bot: {name}")),
            )
        })
        .children(filtered.into_iter().map(|(idx, cand)| {
            div()
                .id(SharedString::from(format!("imp-cand-{idx}")))
                .p(px(9.0))
                .rounded(px(6.0))
                .bg(rgb(pal.card_bg))
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!(
                                    "{} [{} -> /{}]",
                                    cand.suggested_name.as_str(),
                                    cand.source.as_str(),
                                    cand.suggested_folder.as_str()
                                )),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .child(format!(
                                    "Redacted {} secret(s) · {} risk warning(s)",
                                    cand.redacted_secrets_count,
                                    cand.risk_warnings.len()
                                )),
                        ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("imp-cand-install-{idx}")))
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.import_discovered_candidate(idx, cx);
                            }),
                        )
                        .child(tr_str(lang, "导入", "Import")),
                )
        }))
        .into_any_element();

    dialog_shell(
        "orca-cross-app-import-dialog",
        tr_str(lang, "跨应用导入 (OpenClaw / Hermes / OpenBot)", "Cross-App Bot Import").to_string(),
        "Zero-parser safe discovery (never reads foreign DBs/private keys; redacts secrets before writing)."
            .to_string(),
        body,
        pal,
        &ws.dialog_scroll,
        cx,
    )
}

fn render_hand_over_dialog(
    ws: &OrcaBotsWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let (ws_name, repo_slug, recent_prompt, custom_name, target_existing, preview, last_outcome) =
        match &ws.state.dialog {
            BotDialogState::HandOver {
                workspace_name,
                repo_slug,
                recent_prompt,
                custom_name,
                target_existing_bot_id,
                preview,
                last_outcome,
                ..
            } => (
                workspace_name.clone(),
                repo_slug.clone(),
                recent_prompt.clone(),
                custom_name.clone(),
                target_existing_bot_id.clone(),
                preview.clone(),
                last_outcome.clone(),
            ),
            _ => (String::new(), String::new(), String::new(), String::new(), None, None, None),
        };

    let field_box = |id: &'static str,
                     target: BotsInputTarget,
                     val: String,
                     placeholder: &'static str,
                     cx: &mut Context<OrcaBotsWorkspace>| {
        let active = ws.state.active_input == target;
        div()
            .id(id)
            .cursor_text()
            .px(px(9.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .bg(rgb(pal.input_bg))
            .border_1()
            .border_color(if active {
                rgb(pal.accent)
            } else {
                pal.hairline_border_rgba()
            })
            .text_xs()
            .text_color(rgb(pal.text_primary))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |ws, _, window, cx| {
                    cx.stop_propagation();
                    ws.set_active_input(target, window, cx);
                }),
            )
            .child(if val.is_empty() && !active {
                placeholder.to_string()
            } else if active {
                format!("{val}|")
            } else {
                val
            })
    };

    let body = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(field_box(
            "ho-ws-name",
            BotsInputTarget::HandOverWorkspaceName,
            ws_name,
            "Branch / worktree name (e.g. fix-relay-reconnect)…",
            cx,
        ))
        .child(field_box(
            "ho-repo-slug",
            BotsInputTarget::HandOverRepoSlug,
            repo_slug,
            "Repository slug (optional, e.g. bhrumom/gpui-telegram)…",
            cx,
        ))
        .child(field_box(
            "ho-prompt",
            BotsInputTarget::HandOverPrompt,
            recent_prompt,
            "Recent supervision prompt / context (optional)…",
            cx,
        ))
        .child(field_box(
            "ho-custom-name",
            BotsInputTarget::HandOverCustomName,
            custom_name,
            "Custom bot name (auto-filled by suggestion)…",
            cx,
        ))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .child(cat_tab_btn(
                    "ho-target-new",
                    "Create New Bot",
                    target_existing.is_none(),
                    pal,
                    cx.listener(|ws, _, _, cx| {
                        cx.stop_propagation();
                        if let BotDialogState::HandOver {
                            ref mut target_existing_bot_id,
                            ..
                        } = ws.state.dialog
                        {
                            *target_existing_bot_id = None;
                            cx.notify();
                        }
                    }),
                ))
                .children(ws.state.bots.iter().map(|b| {
                    let active = target_existing.as_deref() == Some(b.id.as_str());
                    let bid = b.id.clone();
                    div()
                        .id(SharedString::from(format!("ho-target-{}", b.id)))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(5.0))
                        .bg(if active {
                            rgb(pal.accent)
                        } else {
                            rgb(pal.input_bg)
                        })
                        .text_xs()
                        .text_color(if active {
                            rgb(pal.active_button_fg)
                        } else {
                            rgb(pal.text_primary)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                if let BotDialogState::HandOver {
                                    ref mut target_existing_bot_id,
                                    ..
                                } = ws.state.dialog
                                {
                                    *target_existing_bot_id = Some(bid.clone());
                                    cx.notify();
                                }
                            }),
                        )
                        .child(format!("Attach: {}", b.name.as_str()))
                })),
        )
        .when_some(preview, |el, s| {
            el.child(
                div()
                    .p(px(8.0))
                    .rounded(px(6.0))
                    .bg(rgb(pal.card_bg))
                    .text_xs()
                    .text_color(rgb(pal.text_primary))
                    .child(format!(
                        "Suggested Bot: '{}' (/{}) · {}",
                        s.suggested_bot_name.as_str(),
                        s.folder.as_str(),
                        s.purpose
                    )),
            )
        })
        .when_some(last_outcome, |el, outcome| {
            let (is_err, text) = match outcome {
                HandOverOutcome::Created { name, folder, .. } => (
                    false,
                    format!("Created & handed over to '{}' (/{})", name.as_str(), folder.as_str()),
                ),
                HandOverOutcome::Attached { bot_id } => {
                    (false, format!("Attached workspace to bot '{bot_id}'"))
                }
                HandOverOutcome::RolledBack { placement, reason } => (
                    true,
                    format!("CAS Rollback ({placement:?}): {reason}"),
                ),
                HandOverOutcome::Declined | HandOverOutcome::Deferred => {
                    (false, "Hand-over deferred".to_string())
                }
            };
            el.child(
                div()
                    .p(px(8.0))
                    .rounded(px(6.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(
                        if is_err { pal.danger } else { pal.success },
                        0.15,
                    ))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if is_err { rgb(pal.danger) } else { rgb(pal.success) })
                    .child(text),
            )
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("ho-preview-btn")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.refresh_hand_over_preview(cx);
                            }),
                        )
                        .child("Preview Suggestion"),
                )
                .child(
                    div()
                        .id("ho-execute-btn")
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.execute_hand_over(cx);
                            }),
                        )
                        .child(tr_str(lang, "确认交接", "Hand to Bot")),
                ),
        )
        .into_any_element();

    dialog_shell(
        "orca-hand-over-dialog",
        tr_str(lang, "交接工作区给机器人 (Hand to bot…)", "Hand Workspace to Bot").to_string(),
        "Transactional move -> notify -> CAS rollback (surfaces Restored / MovedSince / Left on lineage conflict)."
            .to_string(),
        body,
        pal,
        &ws.dialog_scroll,
        cx,
    )
}
