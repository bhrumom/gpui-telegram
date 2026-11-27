//! Compile-time exhaustive GPUI `0.2.2` renderers for all 10 `NativeChatBotCardKind`
//! variants, interactive multi-decision `BotItemAsk` quick-reply cards (`--option` /
//! `--recommended` / `--multi`), `UsageLimitContinue` wait/auto-continue banner, and
//! `StreamingQuietMarkerFilter` (`NO_REPLY`) live preview.

use gpui::prelude::FluentBuilder;
use gpui::*;
use orca_core::bot_ecosystem::{
    BotCardPlacement, NativeChatBotCard, NativeChatBotCardKind, UsageLimitContinue,
};
use orca_core::bot_items::{AskSelection, BotItem, BotItemAsk, format_ask_reply};

use crate::i18n::tr_str;
use crate::theme::{IconName, TdesktopThemePalette, tg_icon};

use super::bots_workspace::OrcaBotsWorkspace;

/// Compile-time exhaustive metadata label for all 10 `NativeChatBotCardKind` variants
/// (zero `_` wildcard).
pub const fn card_kind_badge_label(kind: NativeChatBotCardKind) -> &'static str {
    match kind {
        NativeChatBotCardKind::MemoryUpdated => "Memory Updated",
        NativeChatBotCardKind::ConnectorOffer => "Connector Offer",
        NativeChatBotCardKind::ImportPlan => "Import Plan",
        NativeChatBotCardKind::ImportDone => "Import Done",
        NativeChatBotCardKind::BotRequest => "Bot Request",
        NativeChatBotCardKind::GoogleSetup => "Google Setup",
        NativeChatBotCardKind::ComposioSetup => "Composio Setup",
        NativeChatBotCardKind::CreateOffer => "Create Offer",
        NativeChatBotCardKind::ToolAdded => "Tool Added",
        NativeChatBotCardKind::Webhook => "Webhook Directive",
    }
}

pub const fn placement_badge_label(placement: BotCardPlacement) -> &'static str {
    match placement {
        BotCardPlacement::StatusLine => "status-line",
        BotCardPlacement::Message => "message",
        BotCardPlacement::Directive => "directive",
    }
}

/// Renders any of the 10 `NativeChatBotCardKind` variants with a compile-time exhaustive
/// `match` (zero `_` wildcard).
pub fn render_native_bot_card(
    card: &NativeChatBotCard,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let (icon, accent_hex, body_el) = match card.kind {
        NativeChatBotCardKind::MemoryUpdated => {
            let row_id = card
                .payload
                .get("historyRowId")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let el = div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(
                            card.summary
                                .clone()
                                .unwrap_or_else(|| "MEMORY.md updated with host provenance.".to_string()),
                        ),
                )
                .when_some(row_id, |row, rid| {
                    let rid_btn = rid.clone();
                    row.child(
                        div()
                            .id(SharedString::from(format!("card-mem-undo-{}", card.card_id)))
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
                                    ws.undo_memory_change(&rid_btn, cx);
                                }),
                            )
                            .child(tr_str(lang, "撤销", "Undo")),
                    )
                })
                .into_any_element();
            (IconName::Edit, pal.accent, el)
        }
        NativeChatBotCardKind::ConnectorOffer => {
            let offer_id = card
                .payload
                .get("offerId")
                .and_then(|v| v.as_str())
                .unwrap_or(&card.card_id)
                .to_string();
            let el = render_cas_offer_actions(
                &card.card_id,
                &offer_id,
                card.summary
                    .as_deref()
                    .unwrap_or("Connect external workspace via host-isolated OAuth relay."),
                tr_str(lang, "连接", "Connect"),
                pal,
                lang,
                cx,
            );
            (IconName::Forward, pal.accent, el)
        }
        NativeChatBotCardKind::ImportPlan => {
            let count = card
                .payload
                .get("candidateCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let el = div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(
                    card.summary
                        .clone()
                        .unwrap_or_else(|| format!("Prepared import plan ({count} candidate(s), secrets will be redacted).")),
                )
                .into_any_element();
            (IconName::FileDoc, pal.accent, el)
        }
        NativeChatBotCardKind::ImportDone => {
            let redacted = card
                .payload
                .get("redactedCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let el = div()
                .text_xs()
                .text_color(rgb(pal.success))
                .child(
                    card.summary
                        .clone()
                        .unwrap_or_else(|| format!("Import completed cleanly ({redacted} secret(s) redacted).")),
                )
                .into_any_element();
            (IconName::SelectCheck, pal.success, el)
        }
        NativeChatBotCardKind::BotRequest => {
            let requester = card
                .payload
                .get("requester")
                .and_then(|v| v.as_str())
                .unwrap_or("peer-bot");
            let el = div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(
                    card.summary
                        .clone()
                        .unwrap_or_else(|| format!("Inter-bot request from {requester} checked against 7-code policy.")),
                )
                .into_any_element();
            (IconName::BotRobot, pal.accent, el)
        }
        NativeChatBotCardKind::GoogleSetup => {
            let el = div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(
                    card.summary
                        .clone()
                        .unwrap_or_else(|| "Configure host-managed Google OAuth accounts (safe-empty default).".to_string()),
                )
                .into_any_element();
            (IconName::SettingsGear, pal.accent, el)
        }
        NativeChatBotCardKind::ComposioSetup => {
            let el = div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(
                    card.summary
                        .clone()
                        .unwrap_or_else(|| "Bind Composio toolkits through loopback credential-isolating relay.".to_string()),
                )
                .into_any_element();
            (IconName::TerminalMcp, pal.accent, el)
        }
        NativeChatBotCardKind::CreateOffer => {
            let offer_id = card
                .payload
                .get("offerId")
                .and_then(|v| v.as_str())
                .unwrap_or(&card.card_id)
                .to_string();
            let el = render_cas_offer_actions(
                &card.card_id,
                &offer_id,
                card.summary
                    .as_deref()
                    .unwrap_or("Create suggested companion bot (single-answer CAS guarded)."),
                tr_str(lang, "创建", "Create"),
                pal,
                lang,
                cx,
            );
            (IconName::SparkleAlgo, pal.success, el)
        }
        NativeChatBotCardKind::ToolAdded => {
            let tool_name = card
                .payload
                .get("tool")
                .and_then(|v| v.as_str())
                .unwrap_or("mcp-tool");
            let el = div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(
                    card.summary
                        .clone()
                        .unwrap_or_else(|| format!("Registered host-isolated tool '{tool_name}' on loopback relay.")),
                )
                .into_any_element();
            (IconName::TerminalMcp, pal.success, el)
        }
        NativeChatBotCardKind::Webhook => {
            // UI-only directive card: the ONLY place webhook ingest URL & secret copy buttons appear.
            let ingest_url = card
                .payload
                .get("ingestUrl")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let secret_key = card
                .payload
                .get("secretKey")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let url_copy = ingest_url.clone();
            let sec_copy = secret_key.clone();
            let el = div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!("Ingest URL: {ingest_url}")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("card-wh-copy-url-{}", card.card_id)))
                                .cursor_pointer()
                                .px(px(8.0))
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
                                        ws.copy_text_to_clipboard(url_copy.clone(), "Webhook URL copied", cx);
                                    }),
                                )
                                .child(tr_str(lang, "复制 Webhook URL", "Copy Webhook URL")),
                        )
                        .when(!secret_key.is_empty(), |row| {
                            row.child(
                                div()
                                    .id(SharedString::from(format!("card-wh-copy-sec-{}", card.card_id)))
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(5.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.16))
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.danger))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |ws, _, _, cx| {
                                            cx.stop_propagation();
                                            ws.copy_text_to_clipboard(sec_copy.clone(), "Webhook signing secret copied", cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "复制签名密钥", "Copy Signing Secret")),
                            )
                        }),
                )
                .into_any_element();
            (IconName::ShieldMod, pal.danger, el)
        }
    };

    div()
        .id(SharedString::from(format!("orca-bot-card-{}", card.card_id)))
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
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon(icon, 14.0, accent_hex))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(card.title.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(accent_hex, 0.14))
                                .text_xs()
                                .text_color(rgb(accent_hex))
                                .child(card_kind_badge_label(card.kind)),
                        )
                        .child(
                            div()
                                .px(px(5.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(rgb(pal.input_bg))
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .child(placement_badge_label(card.placement())),
                        ),
                ),
        )
        .child(body_el)
        .into_any_element()
}

fn render_cas_offer_actions(
    card_id: &str,
    offer_id: &str,
    summary: &str,
    accept_label: &str,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let accept_oid = offer_id.to_string();
    let dismiss_oid = offer_id.to_string();
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(summary.to_string()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id(SharedString::from(format!("card-offer-accept-{card_id}")))
                        .cursor_pointer()
                        .px(px(10.0))
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
                                ws.answer_offer_card(&accept_oid, true, cx);
                            }),
                        )
                        .child(accept_label.to_string()),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("card-offer-dismiss-{card_id}")))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                ws.answer_offer_card(&dismiss_oid, false, cx);
                            }),
                        )
                        .child(tr_str(lang, "忽略", "Dismiss")),
                ),
        )
        .into_any_element()
}

/// Renders an interactive multi-decision `BotItemAsk` card (`--option`, `--recommended`,
/// `--multi`) wired to `OrcaBotsSurfaceState::toggle_ask_option` and `answer_ask_item`.
pub fn render_bot_item_ask_card(
    ws: &OrcaBotsWorkspace,
    item: &BotItem,
    ask: &BotItemAsk,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<OrcaBotsWorkspace>,
) -> AnyElement {
    let mut selections: Vec<AskSelection> = Vec::new();
    for (q_idx, _q) in ask.questions.iter().enumerate() {
        if let Some(labels) = ws
            .state
            .ask_draft_selections
            .get(&(item.uid.clone(), q_idx))
            && !labels.is_empty()
        {
            selections.push(AskSelection {
                question_index: q_idx,
                labels: labels.clone(),
            });
        }
    }
    let formatted_preview = if !selections.is_empty() {
        format_ask_reply(ask, &selections).ok()
    } else {
        None
    };

    let item_uid_submit = item.uid.clone();
    let q_event = item.question_event.unwrap_or(item.last_event_id);
    let can_submit = !selections.is_empty();

    div()
        .id(SharedString::from(format!("ask-card-{}", item.uid)))
        .p(px(10.0))
        .rounded(px(8.0))
        .bg(rgb(pal.card_bg))
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.45))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .when_some(ask.context.as_deref().filter(|s| !s.trim().is_empty()), |el, ctx_text| {
            el.child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(ctx_text.to_string()),
            )
        })
        .children(ask.questions.iter().enumerate().map(|(q_idx, q)| {
            let selected_labels = ws
                .state
                .ask_draft_selections
                .get(&(item.uid.clone(), q_idx))
                .cloned()
                .unwrap_or_default();
            let multi = q.multi;
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(q.prompt.clone()),
                        )
                        .when(multi, |row| {
                            row.child(
                                div()
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(pal.input_bg))
                                    .text_xs()
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(lang, "多选", "multi")),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.0))
                        .children(q.options.iter().enumerate().map(|(o_idx, opt)| {
                            let is_selected = selected_labels.iter().any(|l| l == &opt.label);
                            let is_rec = q.recommended == Some(o_idx);
                            let uid_click = item.uid.clone();
                            let lbl_click = opt.label.clone();
                            let pill_text = if is_rec {
                                format!("★ {}", opt.label)
                            } else {
                                opt.label.clone()
                            };
                            div()
                                .id(SharedString::from(format!(
                                    "ask-opt-{}-{}-{}",
                                    item.uid, q_idx, o_idx
                                )))
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(4.0))
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(if is_selected {
                                    rgb(pal.accent)
                                } else if is_rec {
                                    rgb(pal.success)
                                } else {
                                    pal.hairline_border_rgba()
                                })
                                .bg(if is_selected {
                                    TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.22)
                                } else {
                                    rgb(pal.input_bg)
                                })
                                .text_xs()
                                .font_weight(if is_selected || is_rec {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |ws, _, _, cx| {
                                        cx.stop_propagation();
                                        ws.state.toggle_ask_option(&uid_click, q_idx, &lbl_click, multi);
                                        cx.notify();
                                    }),
                                )
                                .child(pill_text)
                        })),
                )
        }))
        .when_some(formatted_preview, |el, preview| {
            el.child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .bg(rgb(pal.code_bg))
                    .text_xs()
                    .text_color(rgb(pal.code_fg))
                    .child(preview),
            )
        })
        .child(
            div()
                .flex()
                .justify_end()
                .child(
                    div()
                        .id(SharedString::from(format!("ask-submit-{}", item.uid)))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(if can_submit {
                            rgb(pal.accent)
                        } else {
                            rgb(pal.muted_badge_bg)
                        })
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |ws, _, _, cx| {
                                cx.stop_propagation();
                                if can_submit {
                                    ws.submit_ask_answer(&item_uid_submit, q_event, selections.clone(), cx);
                                }
                            }),
                        )
                        .child(tr_str(lang, "提交决策", "Submit Decision")),
                ),
        )
        .into_any_element()
}

/// Renders the `UsageLimitContinue` wait / auto-continue status banner.
pub fn render_usage_limit_banner(
    limit: &UsageLimitContinue,
    now_ms: i64,
    pal: TdesktopThemePalette,
) -> AnyElement {
    let notice = limit.format_wait_notice(now_ms);
    let accent = if limit.auto_continue_enabled {
        pal.accent
    } else {
        pal.danger
    };
    div()
        .id("orca-usage-limit-banner")
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .bg(TdesktopThemePalette::rgba_with_alpha(accent, 0.14))
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(accent, 0.4))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(tg_icon(IconName::ClockSending, 14.0, accent))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!("{} · {notice}", limit.provider)),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(format!("attempts {}/{}", limit.attempts, limit.max_attempts)),
        )
        .into_any_element()
}

/// Renders the live `StreamingQuietMarkerFilter` preview (`NO_REPLY` suppressed).
pub fn render_streaming_reply_preview(
    visible_text: &str,
    pal: TdesktopThemePalette,
) -> Option<AnyElement> {
    if visible_text.trim().is_empty() {
        return None;
    }
    Some(
        div()
            .id("orca-streaming-preview")
            .p(px(10.0))
            .rounded(px(8.0))
            .bg(rgb(pal.msg_in_bg))
            .border_1()
            .border_color(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.35))
            .text_xs()
            .text_color(rgb(pal.text_primary))
            .child(format!("{visible_text} ▍"))
            .into_any_element(),
    )
}
