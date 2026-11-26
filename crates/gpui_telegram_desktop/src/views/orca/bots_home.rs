//! 4-Tab Bot Home (`Chat` / `Items` / `Up Next` / `Settings`) and Priority-Folded
//! `"Needs You"` Inbox GPUI `0.2.2` renderers.

use gpui::prelude::FluentBuilder;
use gpui::*;
use orca_bots::TidyUpSchedule;
use orca_core::bot_items::{BotItem, BotItemStatus, bot_item_links_one_per_chain, is_snooze_active};
use orca_core::bot_look_around::LookAroundEvery;
use orca_core::bot_memory::is_protected_owner_line;
use orca_core::bot_types::{
    BOT_AVATAR_COLORS, BotAvatarOutfit, BotMascotId, OrcaBotRecord,
};
use orca_core::triggers_webhooks::WebhookAdapterId;

use crate::i18n::tr_str;
use crate::theme::{IconName, TdesktopThemePalette, tg_icon};

use super::bots_cards::{
    render_bot_item_ask_card, render_native_bot_card, render_streaming_reply_preview,
    render_usage_limit_banner,
};
use super::bots_state::{BotHomeTab, BotsInputTarget};
use super::bots_workspace::OrcaBotsWorkspace;

/// Renders the priority-folded `"Needs You"` inbox: approvals are NEVER hidden even when
/// exceeding `NEEDS_YOU_FOLD_CAP`, focused item is preserved, and `"Show N more"` expands.
pub fn render_needs_you_inbox(
    ws: &OrcaBotsWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let fold = &ws.state.needs_you_fold;
    let total = fold.visible.len() + fold.folded.len();
    let shown_items: Vec<&BotItem> = if ws.state.needs_you_expanded {
        fold.visible.iter().chain(fold.folded.iter()).collect()
    } else {
        fold.visible.iter().collect()
    };

    div()
        .id("orca-needs-you-inbox")
        .p(px(10.0))
        .rounded(px(8.0))
        .bg(rgb(pal.card_bg))
        .border_1()
        .border_color(if total > 0 {
            TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.45)
        } else {
            pal.hairline_border_rgba()
        })
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
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon(IconName::ShieldMod, 15.0, pal.danger))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!(
                                    "{} ({total})",
                                    tr_str(lang, "需要你处理", "Needs You")
                                )),
                        )
                        .when(fold.approval_count > 0, |row| {
                            row.child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.16))
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.danger))
                                    .child(format!("{} approval(s)", fold.approval_count)),
                            )
                        }),
                )
                .when(fold.hidden_count > 0, |row| {
                    let expanded = ws.state.needs_you_expanded;
                    let label = if expanded {
                        tr_str(lang, "收起", "Fold").to_string()
                    } else {
                        format!("Show {} more", fold.hidden_count)
                    };
                    row.child(
                        div()
                            .id("orca-needs-you-toggle-fold")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(5.0))
                            .bg(rgb(pal.input_bg))
                            .text_xs()
                            .text_color(rgb(pal.accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|ws, _, _, cx| {
                                    cx.stop_propagation();
                                    ws.state.needs_you_expanded = !ws.state.needs_you_expanded;
                                    cx.notify();
                                }),
                            )
                            .child(label),
                    )
                }),
        )
        .when(shown_items.is_empty(), |el| {
            el.child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(lang, "当前没有待处理事项。", "All caught up — no pending decisions.")),
            )
        })
        .children(shown_items.into_iter().map(|item| {
            let uid_focus = item.uid.clone();
            div()
                .id(SharedString::from(format!("needs-you-item-{}", item.uid)))
                .p(px(8.0))
                .rounded(px(6.0))
                .bg(rgb(pal.window_bg))
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!("#{} · {}", item.number, item.title)),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("needs-you-pin-{}", item.uid)))
                                .cursor_pointer()
                                .text_xs()
                                .text_color(rgb(pal.text_link))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.focus_needs_you_item(&uid_focus, cx);
                                    }),
                                )
                                .child(tr_str(lang, "聚焦", "Focus")),
                        ),
                )
                .when_some(item.ask.as_ref(), |el, ask| {
                    el.child(render_bot_item_ask_card(ws, item, ask, pal, lang, cx))
                })
        }))
        .into_any_element()
}

/// Renders the main 4-tab Bot Home panel for the currently selected `OrcaBotRecord`.
pub fn render_bot_home_panel(
    ws: &OrcaBotsWorkspace,
    bot: &OrcaBotRecord,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let tab = ws.state.active_tab;
    let body = match tab {
        BotHomeTab::Chat => render_chat_tab(ws, bot, pal, lang, cx),
        BotHomeTab::Items => render_items_tab(ws, pal, lang, cx),
        BotHomeTab::UpNext => render_up_next_tab(ws, bot, pal, lang, cx),
        BotHomeTab::Settings => render_settings_tab(ws, bot, pal, lang, cx),
    };

    div()
        .id("orca-bot-home-panel")
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .bg(rgb(pal.stage_bg))
        .child(
            div()
                .px(px(14.0))
                .py(px(10.0))
                .bg(rgb(pal.top_bar_bg))
                .border_b_1()
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
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(tg_icon(IconName::BotRobot, 18.0, pal.accent))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(bot.name.as_str().to_string()),
                                )
                                .child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(pal.input_bg))
                                        .text_xs()
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("{} · /{}", bot.agent.as_str(), bot.folder.as_str())),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(action_chip(
                                    "home-pause-btn",
                                    if bot.paused {
                                        tr_str(lang, "恢复", "Resume")
                                    } else {
                                        tr_str(lang, "暂停", "Pause")
                                    },
                                    pal,
                                    cx.listener(|ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.toggle_selected_bot_paused(cx);
                                    }),
                                ))
                                .child(action_chip(
                                    "home-handover-btn",
                                    tr_str(lang, "交接工作区…", "Hand to bot…"),
                                    pal,
                                    cx.listener(|ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.open_hand_over_dialog(cx);
                                    }),
                                ))
                                .child(action_chip(
                                    "home-pkg-btn",
                                    tr_str(lang, "打包/导出", "Package"),
                                    pal,
                                    cx.listener(|ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.open_package_dialog(cx);
                                    }),
                                ))
                                .child(action_chip(
                                    "home-share-btn",
                                    tr_str(lang, "E2EE 共享", "Team Share"),
                                    pal,
                                    cx.listener(|ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.open_team_share_dialog(cx);
                                    }),
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .children(BotHomeTab::ALL.into_iter().map(|t| {
                            let active = t == tab;
                            let lbl = match t {
                                BotHomeTab::Chat => tr_str(lang, "对话", "Chat"),
                                BotHomeTab::Items => tr_str(lang, "事项", "Items"),
                                BotHomeTab::UpNext => tr_str(lang, "接下来", "Up Next"),
                                BotHomeTab::Settings => tr_str(lang, "设置", "Settings"),
                            };
                            div()
                                .id(SharedString::from(format!("bot-home-tab-{}", t.as_str())))
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(6.0))
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
                                    rgb(pal.text_secondary)
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.state.select_home_tab(t);
                                        cx.notify();
                                    }),
                                )
                                .child(lbl)
                        })),
                ),
        )
        .child(body)
        .into_any_element()
}

fn action_chip(
    id: &'static str,
    label: &'static str,
    pal: TdesktopThemePalette,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .cursor_pointer()
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(5.0))
        .bg(rgb(pal.input_bg))
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .text_xs()
        .text_color(rgb(pal.text_primary))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
        .into_any_element()
}

fn render_chat_tab(
    ws: &OrcaBotsWorkspace,
    bot: &OrcaBotRecord,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let session_id = ws
        .state
        .selected_chat_session_id
        .clone()
        .unwrap_or_else(|| format!("home:{}", bot.id));
    let messages = ws
        .state
        .chat_messages
        .get(&session_id)
        .cloned()
        .unwrap_or_default();
    let cards: Vec<_> = ws
        .state
        .chat_cards
        .iter()
        .filter(|c| c.bot_id == bot.id)
        .cloned()
        .collect();
    let draft_active = ws.state.active_input == BotsInputTarget::BotChatDraft;
    let draft_text = if ws.state.chat_draft.is_empty() && !draft_active {
        tr_str(lang, "向此机器人发送指令或粘贴设计抓取…", "Message this bot or attach Design Mode capture…").to_string()
    } else if draft_active {
        format!("{}|", ws.state.chat_draft)
    } else {
        ws.state.chat_draft.clone()
    };

    div()
        .id("orca-bot-chat-tab")
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .p(px(12.0))
        .child(render_needs_you_inbox(ws, pal, lang, cx))
        .when_some(ws.state.usage_limit.as_ref(), |el, lim| {
            el.child(render_usage_limit_banner(lim, now_ms, pal))
        })
        .when_some(ws.state.pending_design_handoff.as_ref(), |el, handoff| {
            el.child(
                div()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.14))
                    .border_1()
                    .border_color(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.4))
                    .text_xs()
                    .text_color(rgb(pal.text_primary))
                    .child(format!(
                        "Design Mode capture ready: {} ({})",
                        handoff.selector, handoff.contract.url
                    )),
            )
        })
        .child(
            div()
                .id("orca-bot-chat-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .track_scroll(&ws.chat_scroll)
                .flex()
                .flex_col()
                .gap(px(8.0))
                .when(messages.is_empty() && cards.is_empty(), |el| {
                    el.child(
                        div()
                            .p(px(14.0))
                            .rounded(px(8.0))
                            .bg(rgb(pal.card_bg))
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(
                                lang,
                                "会话已就绪（按权威顺序加载 Tools -> SOUL.md -> MEMORY.md，自动抑制 NO_REPLY）。",
                                "Session ready (authority-ordered Tools -> SOUL.md -> MEMORY.md preamble; streaming NO_REPLY suppressed).",
                            )),
                    )
                })
                .children(messages.into_iter().map(|m| {
                    div()
                        .id(SharedString::from(format!("bot-msg-{}", m.id)))
                        .p(px(10.0))
                        .rounded(px(8.0))
                        .bg(if m.is_owner {
                            rgb(pal.msg_out_bg)
                        } else {
                            rgb(pal.msg_in_bg)
                        })
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if m.is_owner {
                                    rgb(pal.msg_out_secondary_fg)
                                } else {
                                    rgb(pal.text_secondary)
                                })
                                .child(m.sender_label),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(if m.is_owner {
                                    rgb(pal.msg_out_fg)
                                } else {
                                    rgb(pal.text_primary)
                                })
                                .child(m.text),
                        )
                }))
                .children(
                    cards
                        .iter()
                        .map(|c| render_native_bot_card(c, pal, lang, cx)),
                )
                .when_some(
                    render_streaming_reply_preview(&ws.state.streaming_visible_preview, pal),
                    |el, preview| el.child(preview),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("orca-bot-chat-input")
                        .flex_1()
                        .cursor_text()
                        .px(px(10.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if draft_active {
                            rgb(pal.accent)
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .text_xs()
                        .text_color(if ws.state.chat_draft.is_empty() && !draft_active {
                            rgb(pal.text_secondary)
                        } else {
                            rgb(pal.text_primary)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, window, cx| {
                                cx.stop_propagation();
                                ws.set_active_input(BotsInputTarget::BotChatDraft, window, cx);
                            }),
                        )
                        .child(draft_text),
                )
                .child(
                    div()
                        .id("orca-bot-chat-send")
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.send_chat_draft(cx);
                            }),
                        )
                        .child(tr_str(lang, "发送", "Send")),
                ),
        )
        .into_any_element()
}

fn render_items_tab(
    ws: &OrcaBotsWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let input_active = ws.state.active_input == BotsInputTarget::NewItemTitle;
    let title_display = if ws.state.new_item_title.is_empty() && !input_active {
        tr_str(lang, "新建 Bot 事项标题…", "New bot item title…").to_string()
    } else if input_active {
        format!("{}|", ws.state.new_item_title)
    } else {
        ws.state.new_item_title.clone()
    };

    div()
        .id("orca-bot-items-tab")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .track_scroll(&ws.items_scroll)
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .when(ws.state.items_read_only, |el| {
            el.child(
                div()
                    .p(px(8.0))
                    .rounded(px(6.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.16))
                    .text_xs()
                    .text_color(rgb(pal.danger))
                    .child("Items store opened in read-only mode (newer forward-only schema version)."),
            )
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("orca-new-item-input")
                        .flex_1()
                        .cursor_text()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if input_active {
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
                                ws.set_active_input(BotsInputTarget::NewItemTitle, window, cx);
                            }),
                        )
                        .child(title_display),
                )
                .child(
                    div()
                        .id("orca-new-item-add")
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
                                ws.add_new_item(cx);
                            }),
                        )
                        .child(tr_str(lang, "添加事项", "Add Item")),
                ),
        )
        .children(ws.state.items.iter().map(|item| {
            let dedup_links = bot_item_links_one_per_chain(&item.links);
            let snoozed = is_snooze_active(item.snoozed_until_ms, now_ms);
            let uid_work = item.uid.clone();
            let uid_done = item.uid.clone();
            let uid_cancel = item.uid.clone();
            let uid_snooze = item.uid.clone();
            let ver = item.version;
            div()
                .id(SharedString::from(format!("orca-item-row-{}", item.uid)))
                .p(px(10.0))
                .rounded(px(8.0))
                .bg(rgb(pal.card_bg))
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!("#{} · {}", item.number, item.title)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(pal.input_bg))
                                        .text_xs()
                                        .text_color(rgb(pal.accent))
                                        .child(item.status.as_str()),
                                )
                                .when(snoozed, |r| {
                                    r.child(
                                        div()
                                            .px(px(6.0))
                                            .py(px(2.0))
                                            .rounded(px(4.0))
                                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                            .text_xs()
                                            .text_color(rgb(pal.text_secondary))
                                            .child("snoozed"),
                                    )
                                }),
                        ),
                )
                .when(!dedup_links.is_empty(), |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(format!("{} chain-deduped link(s)", dedup_links.len())),
                    )
                })
                .when_some(item.ask.as_ref(), |el, ask| {
                    el.child(render_bot_item_ask_card(ws, item, ask, pal, lang, cx))
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(item_state_btn(
                            format!("item-work-{}", item.uid),
                            "Working",
                            pal,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.transition_item(&uid_work, BotItemStatus::Working, ver, cx);
                            }),
                        ))
                        .child(item_state_btn(
                            format!("item-done-{}", item.uid),
                            "Done",
                            pal,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.transition_item(&uid_done, BotItemStatus::Done, ver, cx);
                            }),
                        ))
                        .child(item_state_btn(
                            format!("item-cancel-{}", item.uid),
                            "Cancel",
                            pal,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.transition_item(&uid_cancel, BotItemStatus::Canceled, ver, cx);
                            }),
                        ))
                        .child(item_state_btn(
                            format!("item-snooze-{}", item.uid),
                            if snoozed { "Unsnooze" } else { "Snooze 4h" },
                            pal,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.toggle_item_snooze(&uid_snooze, snoozed, cx);
                            }),
                        )),
                )
        }))
        .into_any_element()
}

fn item_state_btn(
    id: String,
    label: &'static str,
    pal: TdesktopThemePalette,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(SharedString::from(id))
        .cursor_pointer()
        .px(px(7.0))
        .py(px(3.0))
        .rounded(px(4.0))
        .bg(rgb(pal.input_bg))
        .text_xs()
        .text_color(rgb(pal.text_primary))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
        .into_any_element()
}

fn render_up_next_tab(
    ws: &OrcaBotsWorkspace,
    bot: &OrcaBotRecord,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let la = &ws.state.look_around_state;
    let supported = bot.agent.supports_look_around();
    let due_text = ws
        .state
        .look_around_due
        .map(|ev| if ev.due_now { "due now" } else { "scheduled" })
        .unwrap_or("idle");

    div()
        .id("orca-bot-up-next-tab")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .track_scroll(&ws.up_next_scroll)
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .p(px(10.0))
                .rounded(px(8.0))
                .bg(rgb(pal.card_bg))
                .border_1()
                .border_color(pal.hairline_border_rgba())
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
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!(
                                    "Look Around (Read-Only Proactive Scan) · {due_text}"
                                )),
                        )
                        .child(
                            div()
                                .id("look-around-toggle")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.0))
                                .rounded(px(5.0))
                                .bg(if la.on {
                                    rgb(pal.success)
                                } else {
                                    rgb(pal.input_bg)
                                })
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if la.on {
                                    rgb(pal.active_button_fg)
                                } else {
                                    rgb(pal.text_primary)
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|ws, _, _, cx| {
                                        cx.stop_propagation();
                                        let next_on = !ws.state.look_around_state.on;
                                        ws.patch_look_around(Some(next_on), None, None, cx);
                                    }),
                                )
                                .child(if la.on { "ON" } else { "OFF" }),
                        ),
                )
                .when(!supported, |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.danger))
                            .child("Look Around requires a read-only Claude or Codex bot."),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.0))
                        .children(LookAroundEvery::ALL.into_iter().map(|preset| {
                            let active = la.schedule.every == preset;
                            div()
                                .id(SharedString::from(format!("la-preset-{}", preset.as_str())))
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
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
                                        ws.patch_look_around(None, Some(preset), None, cx);
                                    }),
                                )
                                .child(preset.as_str())
                        }))
                        .child(
                            div()
                                .id("la-weekdays-toggle")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(5.0))
                                .bg(if la.schedule.weekdays_only {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.2)
                                } else {
                                    rgb(pal.input_bg)
                                })
                                .text_xs()
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|ws, _, _, cx| {
                                        cx.stop_propagation();
                                        let next = !ws.state.look_around_state.schedule.weekdays_only;
                                        ws.patch_look_around(None, None, Some(next), cx);
                                    }),
                                )
                                .child(if la.schedule.weekdays_only {
                                    "Weekdays only: ON"
                                } else {
                                    "Weekdays only: OFF"
                                }),
                        ),
                ),
        )
        .when(ws.state.up_next.suggestions.is_empty(), |el| {
            el.child(
                div()
                    .p(px(12.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.card_bg))
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(
                        lang,
                        "暂无 Up Next 建议（最多 5 条去重建议，自动剥离含凭据 URL）。",
                        "No Up Next suggestions yet (capped at 5 deduped items; credential URLs stripped).",
                    )),
            )
        })
        .children(ws.state.up_next.suggestions.iter().map(|s| {
            let sid_start = s.id.clone();
            let sid_dismiss = s.id.clone();
            div()
                .id(SharedString::from(format!("up-next-{}", s.id)))
                .p(px(10.0))
                .rounded(px(8.0))
                .bg(rgb(pal.card_bg))
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(s.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(s.why.clone()),
                )
                .when_some(s.link.as_ref(), |el, lnk| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_link))
                            .child(lnk.clone()),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("up-next-start-{}", s.id)))
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(4.0))
                                .rounded(px(5.0))
                                .bg(rgb(pal.accent))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.active_button_fg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.start_up_next_suggestion(&sid_start, cx);
                                    }),
                                )
                                .child(tr_str(lang, "开始", "Start")),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("up-next-dismiss-{}", s.id)))
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(4.0))
                                .rounded(px(5.0))
                                .bg(rgb(pal.input_bg))
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.dismiss_up_next_suggestion(&sid_dismiss, cx);
                                    }),
                                )
                                .child(tr_str(lang, "忽略", "Dismiss")),
                        ),
                )
        }))
        .into_any_element()
}

fn render_settings_tab(
    ws: &OrcaBotsWorkspace,
    bot: &OrcaBotRecord,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    div()
        .id("orca-bot-settings-tab")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .track_scroll(&ws.settings_scroll)
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(render_memory_settings_card(ws, pal, lang, cx))
        .child(render_avatar_settings_card(bot, pal, lang, cx))
        .child(render_connectors_security_card(ws, bot, pal, lang, cx))
        .into_any_element()
}

fn render_memory_settings_card(
    ws: &OrcaBotsWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let forget_active = ws.state.active_input == BotsInputTarget::MemoryForgetQuery;
    let forget_text = if ws.state.memory_forget_query.is_empty() && !forget_active {
        tr_str(lang, "输入要遗忘的记忆关键词…", "Query to forget from MEMORY.md…").to_string()
    } else if forget_active {
        format!("{}|", ws.state.memory_forget_query)
    } else {
        ws.state.memory_forget_query.clone()
    };
    let tidy = &ws.state.tidy_up_state;
    let latest_undoable = ws.state.memory_history.iter().rev().find(|r| r.undoable);

    div()
        .p(px(10.0))
        .rounded(px(8.0))
        .bg(rgb(pal.card_bg))
        .border_1()
        .border_color(pal.hairline_border_rgba())
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
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "MEMORY.md Governance ({} facts · 14d Re-Add Guard)",
                            ws.state.memory_facts.len()
                        )),
                )
                .when_some(latest_undoable, |row, hrow| {
                    let row_id = hrow.id.clone();
                    row.child(
                        div()
                            .id("orca-mem-undo-latest")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                            .text_xs()
                            .text_color(rgb(pal.accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |ws, _, _, cx| {
                                    cx.stop_propagation();
                                    ws.undo_memory_change(&row_id, cx);
                                }),
                            )
                            .child(tr_str(lang, "撤销上次变更", "Undo Last Change")),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .id("orca-mem-forget-input")
                        .flex_1()
                        .cursor_text()
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if forget_active {
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
                                ws.set_active_input(BotsInputTarget::MemoryForgetQuery, window, cx);
                            }),
                        )
                        .child(forget_text),
                )
                .child(
                    div()
                        .id("orca-mem-forget-btn")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.danger))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.forget_memory_fact(cx);
                            }),
                        )
                        .child(tr_str(lang, "遗忘", "Forget")),
                ),
        )
        .children(ws.state.memory_facts.iter().take(12).map(|fact| {
            let protected = is_protected_owner_line(&fact.text);
            div()
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(5.0))
                .bg(rgb(pal.window_bg))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .when(protected, |r| r.child(tg_icon(IconName::Lock, 12.0, pal.accent)))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.text_primary))
                                .child(fact.body.clone()),
                        ),
                )
                .when_some(fact.source.as_ref(), |r, src| {
                    r.child(
                        div()
                            .px(px(5.0))
                            .py(px(1.0))
                            .rounded(px(4.0))
                            .bg(rgb(pal.input_bg))
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(format!("· from: {src}")),
                    )
                })
        }))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pt(px(4.0))
                .border_t_1()
                .border_color(pal.separator_rgba())
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "Overnight Tidy-Up (3–8 AM local, tool-less): {} ({:?})",
                            if tidy.enabled { "ON" } else { "OFF" },
                            tidy.schedule
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("orca-tidy-night-btn")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .bg(rgb(pal.input_bg))
                                .text_xs()
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|ws, _, _, cx| {
                                        cx.stop_propagation();
                                        let next = !ws.state.tidy_up_state.enabled;
                                        ws.configure_tidy_up(next, TidyUpSchedule::Night, cx);
                                    }),
                                )
                                .child(if tidy.enabled { "Disable Tidy-Up" } else { "Enable Nightly" }),
                        )
                        .child(
                            div()
                                .id("orca-tidy-week-btn")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .bg(rgb(pal.input_bg))
                                .text_xs()
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.configure_tidy_up(true, TidyUpSchedule::Week, cx);
                                    }),
                                )
                                .child("Weekly"),
                        ),
                ),
        )
        .into_any_element()
}

fn parse_hex_color(hex: &str, fallback: u32) -> u32 {
    u32::from_str_radix(hex.trim().trim_start_matches('#'), 16).unwrap_or(fallback)
}

fn render_avatar_settings_card(
    bot: &OrcaBotRecord,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let spec = &bot.avatar_spec;
    let current_avatar = spec.avatar.as_deref().unwrap_or("");
    let current_color = spec.avatar_color.as_deref().unwrap_or("");
    let current_outfit = spec.avatar_outfit.as_deref().unwrap_or("");

    div()
        .p(px(10.0))
        .rounded(px(8.0))
        .bg(rgb(pal.card_bg))
        .border_1()
        .border_color(pal.hairline_border_rgba())
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
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(
                            lang,
                            "CAS 头像选择器（19 吉祥物 · 9 服饰 · 11 配色）",
                            "CAS Avatar Picker (19 Mascots · 9 Outfits · 11 Colors)",
                        )),
                )
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!("rev {}", spec.avatar_revision)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.0))
                .children(BotMascotId::PICKABLE.into_iter().map(|m| {
                    let active = current_avatar == format!("mascot:{}", m.as_str());
                    div()
                        .id(SharedString::from(format!("avatar-mascot-{}", m.as_str())))
                        .cursor_pointer()
                        .px(px(6.0))
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
                                ws.update_avatar_style(Some(m), None, None, cx);
                            }),
                        )
                        .child(m.as_str())
                })),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.0))
                .children(BotAvatarOutfit::ALL.into_iter().map(|o| {
                    let active = current_outfit == o.as_str();
                    div()
                        .id(SharedString::from(format!("avatar-outfit-{}", o.as_str())))
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
                                ws.update_avatar_style(None, None, Some(o), cx);
                            }),
                        )
                        .child(o.as_str())
                })),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(5.0))
                .children(BOT_AVATAR_COLORS.into_iter().enumerate().map(|(i, hex_str)| {
                    let swatch = parse_hex_color(hex_str, pal.accent);
                    let active = current_color.eq_ignore_ascii_case(hex_str);
                    div()
                        .id(SharedString::from(format!("avatar-color-{i}")))
                        .cursor_pointer()
                        .w(px(20.0))
                        .h(px(20.0))
                        .rounded_full()
                        .bg(rgb(swatch))
                        .border_2()
                        .border_color(if active {
                            rgb(pal.text_primary)
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.update_avatar_style(None, Some(hex_str.to_string()), None, cx);
                            }),
                        )
                })),
        )
        .into_any_element()
}

fn render_connectors_security_card(
    ws: &OrcaBotsWorkspace,
    bot: &OrcaBotRecord,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let host_mcp = bot.connectors.uses_host_mcp_servers();
    let relay_label = ws
        .state
        .relay_base_url
        .as_deref()
        .unwrap_or("stopped (127.0.0.1 loopback)");

    div()
        .p(px(10.0))
        .rounded(px(8.0))
        .bg(rgb(pal.card_bg))
        .border_1()
        .border_color(pal.hairline_border_rgba())
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
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(
                            lang,
                            "连接器、主机 MCP、访问审批与 Webhook",
                            "Connectors, Host-Only MCP, Access Control & Webhooks",
                        )),
                )
                .child(
                    div()
                        .id("orca-host-mcp-toggle")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(5.0))
                        .bg(if host_mcp {
                            rgb(pal.success)
                        } else {
                            rgb(pal.input_bg)
                        })
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if host_mcp {
                            rgb(pal.active_button_fg)
                        } else {
                            rgb(pal.text_primary)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.set_host_mcp_servers(!host_mcp, cx);
                            }),
                        )
                        .child(if host_mcp {
                            "Host MCP: ON"
                        } else {
                            "Host MCP: OFF"
                        }),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(format!(
                    "Tool Relay: {relay_label} · Webhooks: {} · Automations: {}",
                    ws.state.webhooks.len(),
                    ws.state.automations_count
                )),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .child(
                    div()
                        .id("orca-create-gh-webhook")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.create_webhook(WebhookAdapterId::Github, cx);
                            }),
                        )
                        .child("+ GitHub Webhook"),
                )
                .child(
                    div()
                        .id("orca-create-linear-webhook")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.create_webhook(WebhookAdapterId::Linear, cx);
                            }),
                        )
                        .child("+ Linear Webhook"),
                )
                .child(
                    div()
                        .id("orca-delete-bot-btn")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(5.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.16))
                        .text_xs()
                        .text_color(rgb(pal.danger))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.delete_selected_bot(cx);
                            }),
                        )
                        .child(tr_str(lang, "删除机器人", "Delete Bot")),
                ),
        )
        .children(ws.state.access_requests.iter().map(|req| {
            let rid = req.request_id.clone();
            div()
                .id(SharedString::from(format!("access-req-{}", req.request_id)))
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(5.0))
                .bg(rgb(pal.window_bg))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "Who Can Ask: {} ({}) · {:?}",
                            req.member.name,
                            req.member.source.as_deref().unwrap_or("external"),
                            req.status
                        )),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("access-review-{}", req.request_id)))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.open_access_security_modal(&rid, cx);
                            }),
                        )
                        .child(tr_str(lang, "安全审批", "Review in SecuritySurface")),
                )
        }))
        .into_any_element()
}
