//! 9-Provider Usage Popover & Multi-Account Hot-Swap (`F15`), AI Vault FTS5 Search (`F16`),
//! and 4-Tier Plugin System V2 + MCP Config Sync + Transactional Skills UI (`F19`, `F22`).

use gpui::prelude::FluentBuilder;
use gpui::*;
use orca_core::orchestration_protocol::{
    PLUGIN_ACTION_FALLBACK_TIMEOUT_MS, PLUGIN_APPROVAL_REQUIRED_EXIT_CODE, PluginHookErrorPolicy,
    PluginTier,
};
use orca_ide::{
    ACCOUNT_HOME_PROVIDERS_9, AccountHotSwapRequest, HotSwapDecision, MAX_IN_FLIGHT_RESTARTS,
    PluginContributionPoint, REASK_COOLDOWN_MS, SkillInstallState, USAGE_PROVIDERS_9,
    VaultSearchRequest, WARNING_CHIP_THRESHOLD_PERCENT,
};

use crate::i18n::tr_str;
use crate::theme::TdesktopThemePalette;

use super::ide_state::{IdeInputTarget, OrcaIdeAction};
use super::ide_workspace::OrcaIdeWorkspace;

pub fn format_countdown_ms(ms: Option<i64>) -> String {
    let Some(v) = ms else {
        return "—".to_string();
    };
    let secs = (v.max(0)) / 1_000;
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// Renders the 9-Provider Usage Popover, Multi-Account Hot-Swap & UsageLimitContinue drawer.
pub fn render_usage_popover_drawer(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let status_focused = ws.state.active_input == IdeInputTarget::ClaudeStatusLine;
    let acct_focused = ws.state.active_input == IdeInputTarget::HotSwapTargetAccount;
    let show_remaining = ws.state.usage_show_remaining;

    let mut roster_col = div().flex().flex_col().gap(px(5.0));
    if ws.state.usage_roster.is_empty() {
        roster_col = roster_col.child(
            div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(format!(
                    "Tracking all 9 providers ({}). Ingest a snapshot or zero-network Claude status line below.",
                    USAGE_PROVIDERS_9.join(", ")
                )),
        );
    }
    for entry in &ws.state.usage_roster {
        let pct_text = if show_remaining {
            format!("{:.1}% remaining", entry.percent_remaining)
        } else {
            format!("{:.1}% used", entry.percent_used)
        };
        let prov = entry.provider.clone();
        let acct = entry.account_id.clone();
        roster_col = roster_col.child(
            div()
                .p(px(8.0))
                .rounded(px(6.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if entry.warning_chip {
                    rgb(0xF59E0B)
                } else {
                    pal.hairline_border_rgba()
                })
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!("{} · {}", entry.provider, entry.account_id)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(5.0))
                                .when(entry.warning_chip, |el| {
                                    el.child(
                                        div()
                                            .px(px(6.0))
                                            .py(px(1.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(0xF59E0B))
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0x111827))
                                            .child(format!(">={WARNING_CHIP_THRESHOLD_PERCENT:.0}% WARN")),
                                    )
                                })
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(pct_text),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .justify_between()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "5h: {} · daily: {} · weekly: {} · est ${:.4}",
                            format_countdown_ms(entry.five_hour_reset_in_ms),
                            format_countdown_ms(entry.daily_reset_in_ms),
                            format_countdown_ms(entry.weekly_reset_in_ms),
                            entry.inferred_cost_usd
                        ))
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-swap-quick-{prov}-{acct}")))
                                .cursor_pointer()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(rgb(pal.accent))
                                .text_xs()
                                .text_color(rgb(pal.active_button_fg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        this.state.hotswap_provider = prov.clone();
                                        this.state.hotswap_target_account = acct.clone();
                                        cx.notify();
                                    }),
                                )
                                .child("Select for Hot-Swap"),
                        ),
                ),
        );
    }

    render_usage_popover_tail(ws, lang, now_ms, status_focused, acct_focused, roster_col, cx)
}

fn render_usage_popover_tail(
    ws: &OrcaIdeWorkspace,
    lang: &str,
    now_ms: i64,
    status_focused: bool,
    acct_focused: bool,
    roster_col: Div,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;

    let mut provider_pills = div().flex().flex_row().flex_wrap().gap(px(4.0));
    for prov in ACCOUNT_HOME_PROVIDERS_9 {
        let active = ws.state.hotswap_provider == prov;
        provider_pills = provider_pills.child(
            div()
                .id(SharedString::from(format!("orca-hs-prov-{prov}")))
                .cursor_pointer()
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(4.0))
                .bg(if active { rgb(pal.accent) } else { rgb(pal.input_bg) })
                .text_xs()
                .text_color(if active {
                    rgb(pal.active_button_fg)
                } else {
                    rgb(pal.text_secondary)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        this.state.hotswap_provider = prov.to_string();
                        cx.notify();
                    }),
                )
                .child(prov),
        );
    }

    let hotswap_status_el = ws.state.last_hotswap_decision.as_ref().map(|dec| match dec {
        HotSwapDecision::StartRestart {
            restart_id,
            target_account_id,
            target_home_dir,
        } => {
            let rid = restart_id.clone();
            div()
                .p(px(7.0))
                .rounded(px(6.0))
                .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.14))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "StartRestart -> {target_account_id} ({})",
                            target_home_dir.display()
                        )),
                )
                .child(
                    div()
                        .id("orca-hs-complete-btn")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.success))
                        .text_xs()
                        .text_color(rgb(0xFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let now = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                if let Err(e) =
                                    this.state.complete_account_hotswap(&rt, &rid, true, now)
                                {
                                    this.state.error_banner = Some(e);
                                } else {
                                    this.state.status_banner =
                                        Some("Account hot-swap restart completed".to_string());
                                }
                                cx.notify();
                            }),
                        )
                        .child("Complete Restart"),
                )
                .into_any_element()
        }
        HotSwapDecision::QueuedForTurnEnd { target_account_id } => div()
            .text_xs()
            .text_color(rgb(0xF59E0B))
            .child(format!("QueuedForTurnEnd -> {target_account_id}"))
            .into_any_element(),
        HotSwapDecision::DeferredConcurrencyCap {
            in_flight,
            max_in_flight,
        } => div()
            .text_xs()
            .text_color(rgb(pal.danger))
            .child(format!(
                "DeferredConcurrencyCap ({in_flight}/{max_in_flight} restarts in flight)"
            ))
            .into_any_element(),
        HotSwapDecision::DeferredCooldown { retry_after_ms } => div()
            .text_xs()
            .text_color(rgb(0xF59E0B))
            .child(format!(
                "DeferredCooldown ({}s remaining of {}s)",
                retry_after_ms / 1_000,
                REASK_COOLDOWN_MS / 1_000
            ))
            .into_any_element(),
    });

    let mut pauses_col = div().flex().flex_col().gap(px(4.0));
    for (sess_key, pause) in &ws.state.usage_limit_pauses {
        let sk = sess_key.clone();
        pauses_col = pauses_col.child(
            div()
                .p(px(6.0))
                .rounded(px(5.0))
                .bg(rgb(pal.input_bg))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "{sess_key} ({}) · {}",
                            pause.provider,
                            pause.format_wait_notice(now_ms)
                        )),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("orca-ul-resume-{sk}")))
                        .cursor_pointer()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let now = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                if let Err(e) =
                                    this.state.mark_due_auto_continue_resumed(&rt, &sk, now)
                                {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("Resume Now"),
                ),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(9.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(
                            lang,
                            "9 家厂商额度与多账号热切换",
                            "9-Provider Usage & Multi-Account Hot-Swap",
                        )),
                )
                .child(
                    div()
                        .id("orca-usage-pct-mode-toggle")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                this.state.usage_show_remaining = !this.state.usage_show_remaining;
                                cx.notify();
                            }),
                        )
                        .child(if ws.state.usage_show_remaining {
                            "Showing % remaining"
                        } else {
                            "Showing % used"
                        }),
                ),
        )
        .child(roster_col)
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-claude-statusline-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if status_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::ClaudeStatusLine, w, cx);
                            }),
                        )
                        .child(if ws.state.claude_status_line_draft.is_empty() {
                            "Zero-network Claude status line (e.g. 5h: 84% (resets in 2h 15m))…".to_string()
                        } else if status_focused {
                            format!("{}|", ws.state.claude_status_line_draft)
                        } else {
                            ws.state.claude_status_line_draft.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-claude-statusline-ingest-btn")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let now = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                let line = this.state.claude_status_line_draft.clone();
                                if let Err(e) =
                                    this.state.ingest_claude_status_line(&rt, "default", &line, now)
                                {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("Ingest"),
                ),
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_secondary))
                .child(format!(
                    "Multi-Account Hot-Swap (max {MAX_IN_FLIGHT_RESTARTS} in-flight · {}s cooldown · Codex credits: {})",
                    REASK_COOLDOWN_MS / 1_000,
                    ws.state.codex_reset_credits.len()
                )),
        )
        .child(provider_pills)
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-hotswap-acct-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if acct_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::HotSwapTargetAccount, w, cx);
                            }),
                        )
                        .child(if ws.state.hotswap_target_account.is_empty() {
                            "Target account slug (e.g. work-pro)…".to_string()
                        } else if acct_focused {
                            format!("{}|", ws.state.hotswap_target_account)
                        } else {
                            ws.state.hotswap_target_account.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-hotswap-trigger-btn")
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
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let now = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                let req = AccountHotSwapRequest {
                                    session_id: this
                                        .state
                                        .active_session_snapshot
                                        .as_ref()
                                        .map(|s| s.session.session_id.clone())
                                        .unwrap_or_else(|| "sess-default".to_string()),
                                    provider: this.state.hotswap_provider.clone(),
                                    target_account_id: this.state.hotswap_target_account.clone(),
                                    turn_in_progress: this.state.hotswap_turn_in_progress,
                                    force: false,
                                };
                                if let Err(e) = this.state.trigger_account_hotswap(&rt, &req, now) {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("Hot-Swap"),
                ),
        )
        .when_some(hotswap_status_el, |el, node| el.child(node))
        .when(!ws.state.usage_limit_pauses.is_empty(), |el| el.child(pauses_col))
        .into_any_element()
}

/// Renders the AI Vault FTS5 Search drawer (`bm25`, identifier splitting, typo repair banner,
/// cursor pagination `"Load more"`, `"Fork conversation"`, and `"Cross-agent handoff prompt"`).
pub fn render_ai_vault_drawer(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let q_focused = ws.state.active_input == IdeInputTarget::VaultQuery;
    let target_focused = ws.state.active_input == IdeInputTarget::VaultHandoffTargetAgent;

    let repaired = ws
        .state
        .vault_response
        .as_ref()
        .and_then(|r| r.fuzzy_repaired_query.clone());
    let next_cursor = ws
        .state
        .vault_response
        .as_ref()
        .and_then(|r| r.next_cursor.clone());
    let meta_line = ws
        .state
        .vault_response
        .as_ref()
        .map(|r| format!("gen={} · inc={} · {} hit(s)", r.generation, r.incarnation, ws.state.vault_accumulated_hits.len()));

    let mut hits_col = div().flex().flex_col().gap(px(6.0));
    if ws.state.vault_accumulated_hits.is_empty() {
        hits_col = hits_col.child(
            div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child(tr_str(
                    lang,
                    "输入代码标识符（支持 camelCase / snake_case 拆分与拼写自动修复）检索跨 Agent 历史。",
                    "Search cross-agent transcripts by keyword or camelCase/snake_case identifier (with typo repair).",
                )),
        );
    }
    for hit in &ws.state.vault_accumulated_hits {
        let sid_fork = hit.session_id.clone();
        let sid_handoff = hit.session_id.clone();
        let mut id_chips = div().flex().flex_row().flex_wrap().gap(px(4.0));
        for ident in hit.matched_identifiers.iter().take(6) {
            id_chips = id_chips.child(
                div()
                    .px(px(5.0))
                    .py(px(1.0))
                    .rounded(px(4.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16))
                    .text_xs()
                    .text_color(rgb(pal.accent))
                    .child(ident.clone()),
            );
        }
        hits_col = hits_col.child(
            div()
                .p(px(8.0))
                .rounded(px(6.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!(
                                    "{} ({}) · ws:{}",
                                    hit.session_id, hit.agent_id, hit.workspace_id
                                )),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("bm25 {:.3}", hit.rank)),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .child(hit.snippet.clone()),
                )
                .when(!hit.matched_identifiers.is_empty(), |el| el.child(id_chips))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(5.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-vault-fork-{sid_fork}")))
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .bg(rgb(pal.card_bg))
                                .text_xs()
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let now = chrono::Utc::now().timestamp_millis();
                                        let rt = this.runtime.clone();
                                        match this.state.fork_vault_session(&rt, &sid_fork, now) {
                                            Ok(new_sid) => {
                                                this.state.status_banner = Some(format!(
                                                    "Forked vault conversation -> {new_sid}"
                                                ));
                                            }
                                            Err(e) => this.state.error_banner = Some(e),
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("Fork conversation"),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-vault-handoff-{sid_handoff}")))
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .bg(rgb(pal.accent))
                                .text_xs()
                                .text_color(rgb(pal.active_button_fg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let rt = this.runtime.clone();
                                        match this.state.build_vault_handoff_prompt(&rt, &sid_handoff) {
                                            Ok(prompt) => {
                                                this.push_action(
                                                    OrcaIdeAction::SendPromptToActiveChat(prompt),
                                                );
                                                this.state.status_banner = Some(
                                                    "Cross-agent handoff prompt built".to_string(),
                                                );
                                            }
                                            Err(e) => this.state.error_banner = Some(e),
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("Cross-agent handoff prompt"),
                        ),
                ),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.text_primary))
                .child(tr_str(lang, "AI Vault FTS5 跨智能体检索", "AI Vault FTS5 Search")),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-vault-query-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if q_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::VaultQuery, w, cx);
                            }),
                        )
                        .child(if ws.state.vault_query.is_empty() {
                            "Search AI Vault (FTS5 + identifier split + typo repair)…".to_string()
                        } else if q_focused {
                            format!("{}|", ws.state.vault_query)
                        } else {
                            ws.state.vault_query.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-vault-handoff-agent-input")
                        .cursor_text()
                        .w(px(90.0))
                        .px(px(6.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if target_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::VaultHandoffTargetAgent, w, cx);
                            }),
                        )
                        .child(if target_focused {
                            format!("{}|", ws.state.vault_handoff_target_agent)
                        } else {
                            ws.state.vault_handoff_target_agent.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-vault-search-btn")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let rt = this.runtime.clone();
                                let req = VaultSearchRequest {
                                    query: this.state.vault_query.clone(),
                                    workspace_id: this.state.vault_workspace_filter.clone(),
                                    agent_id: this.state.vault_agent_filter.clone(),
                                    limit: 20,
                                    cursor: None,
                                };
                                if let Err(e) = this.state.search_ai_vault(&rt, req) {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("Search"),
                ),
        )
        .when_some(repaired, |el, repaired_q| {
            el.child(
                div()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(5.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(0xF59E0B, 0.16))
                    .border_1()
                    .border_color(rgb(0xF59E0B))
                    .text_xs()
                    .text_color(rgb(pal.text_primary))
                    .child(format!("Typo fuzzy repair applied -> \"{repaired_q}\"")),
            )
        })
        .when_some(meta_line, |el, meta| {
            el.child(div().text_xs().text_color(rgb(pal.text_secondary)).child(meta))
        })
        .child(hits_col)
        .when_some(next_cursor, |el, cursor| {
            el.child(
                div()
                    .id("orca-vault-load-more-btn")
                    .cursor_pointer()
                    .px(px(9.0))
                    .py(px(5.0))
                    .rounded(px(5.0))
                    .bg(rgb(pal.input_bg))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.accent))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            let rt = this.runtime.clone();
                            let req = VaultSearchRequest {
                                query: this.state.vault_query.clone(),
                                workspace_id: this.state.vault_workspace_filter.clone(),
                                agent_id: this.state.vault_agent_filter.clone(),
                                limit: 20,
                                cursor: Some(cursor.clone()),
                            };
                            if let Err(e) = this.state.search_ai_vault(&rt, req) {
                                this.state.error_banner = Some(e);
                            }
                            cx.notify();
                        }),
                    )
                    .child("Load more"),
            )
        })
        .when_some(ws.state.last_vault_handoff_prompt.clone(), |el, prompt| {
            el.child(
                div()
                    .p(px(7.0))
                    .rounded(px(5.0))
                    .bg(rgb(pal.code_bg))
                    .text_xs()
                    .text_color(rgb(pal.code_fg))
                    .child(prompt),
            )
        })
        .into_any_element()
}

/// Renders the 4-Tier Plugin System V2, MCP Config Sync & Transactional Skills drawer.
pub fn render_plugins_drawer(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let pid_focused = ws.state.active_input == IdeInputTarget::PluginIdDraft;
    let pub_focused = ws.state.active_input == IdeInputTarget::PluginPublisherDraft;

    let mut tiers_row = div().flex().flex_row().flex_wrap().gap(px(4.0));
    for tier in PluginTier::ALL {
        let active = ws.state.plugin_tier_draft == tier;
        tiers_row = tiers_row.child(
            div()
                .id(SharedString::from(format!("orca-plugin-tier-{}", tier.as_str())))
                .cursor_pointer()
                .px(px(7.0))
                .py(px(3.0))
                .rounded(px(4.0))
                .bg(if active { rgb(pal.accent) } else { rgb(pal.input_bg) })
                .text_xs()
                .text_color(if active {
                    rgb(pal.active_button_fg)
                } else {
                    rgb(pal.text_secondary)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        this.state.plugin_tier_draft = tier;
                        cx.notify();
                    }),
                )
                .child(format!(
                    "{}{}",
                    tier.as_str(),
                    if tier.requires_approval() { " *" } else { "" }
                )),
        );
    }

    let mut contrib_grid = div().flex().flex_row().flex_wrap().gap(px(4.0));
    for point in PluginContributionPoint::ALL {
        let selected = ws.state.plugin_selected_contributions.contains(&point);
        let allowed = point.allowed_for_tier(ws.state.plugin_tier_draft);
        contrib_grid = contrib_grid.child(
            div()
                .id(SharedString::from(format!("orca-plugin-cp-{}", point.as_str())))
                .cursor_pointer()
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(4.0))
                .bg(if selected {
                    rgb(pal.accent)
                } else {
                    rgb(pal.input_bg)
                })
                .border_1()
                .border_color(if allowed {
                    pal.hairline_border_rgba()
                } else {
                    rgb(pal.danger)
                })
                .text_xs()
                .text_color(if selected {
                    rgb(pal.active_button_fg)
                } else if allowed {
                    rgb(pal.text_secondary)
                } else {
                    rgb(pal.danger)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        if let Some(pos) = this
                            .state
                            .plugin_selected_contributions
                            .iter()
                            .position(|p| *p == point)
                        {
                            this.state.plugin_selected_contributions.remove(pos);
                        } else {
                            this.state.plugin_selected_contributions.push(point);
                        }
                        cx.notify();
                    }),
                )
                .child(point.as_str()),
        );
    }

    let mut installed_col = div().flex().flex_col().gap(px(5.0));
    for rec in &ws.state.installed_plugins {
        let pid = rec.manifest.id.clone();
        let eval = ws.state.plugin_decisions.get(&pid);
        let needs_approval = matches!(eval, Some(Err(_)));
        let status_lbl = match eval {
            Some(Ok(dec)) if dec.allowed => "APPROVED & ALLOWED".to_string(),
            Some(Err(err)) => err.clone(),
            _ => format!("exit {PLUGIN_APPROVAL_REQUIRED_EXIT_CODE} · approval_required"),
        };
        installed_col = installed_col.child(
            div()
                .p(px(7.0))
                .rounded(px(6.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if needs_approval {
                    rgb(0xF59E0B)
                } else {
                    rgb(pal.success)
                })
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!(
                                    "{} v{} [{}] · {}",
                                    rec.manifest.id,
                                    rec.manifest.version,
                                    rec.manifest.tier.as_str(),
                                    rec.audit_actor
                                )),
                        )
                        .when(needs_approval, |el| {
                            let pid_btn = pid.clone();
                            el.child(
                                div()
                                    .id(SharedString::from(format!("orca-plugin-approve-{pid}")))
                                    .cursor_pointer()
                                    .px(px(7.0))
                                    .py(px(3.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(0xF59E0B))
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0x111827))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _ev, _w, cx| {
                                            cx.stop_propagation();
                                            this.open_plugin_approval_security_modal(&pid_btn, cx);
                                        }),
                                    )
                                    .child("Approve (Security Surface)"),
                            )
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!("SHA-256: {} · {status_lbl}", rec.fingerprint)),
                ),
        );
    }

    render_plugins_drawer_tail(
        ws,
        lang,
        pid_focused,
        pub_focused,
        tiers_row,
        contrib_grid,
        installed_col,
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_plugins_drawer_tail(
    ws: &OrcaIdeWorkspace,
    lang: &str,
    pid_focused: bool,
    pub_focused: bool,
    tiers_row: Div,
    contrib_grid: Div,
    installed_col: Div,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let mcp_name_f = ws.state.active_input == IdeInputTarget::McpServerName;
    let mcp_cmd_f = ws.state.active_input == IdeInputTarget::McpServerCommand;
    let skill_name_f = ws.state.active_input == IdeInputTarget::SkillNameDraft;
    let skill_src_f = ws.state.active_input == IdeInputTarget::SkillSourceDirDraft;

    let mut skills_col = div().flex().flex_col().gap(px(4.0));
    for rec in &ws.state.skill_install_records {
        let state_col = match rec.state {
            SkillInstallState::Pending => 0x64748B,
            SkillInstallState::Staged => 0xF59E0B,
            SkillInstallState::Committed => 0x16A34A,
            SkillInstallState::RolledBack => 0xEF4444,
        };
        let iid = rec.install_id.clone();
        skills_col = skills_col.child(
            div()
                .p(px(6.0))
                .rounded(px(5.0))
                .bg(rgb(pal.input_bg))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .child(format!("{} [{}]", rec.skill_name, rec.state.as_str())),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .child(
                            div()
                                .px(px(5.0))
                                .py(px(1.0))
                                .rounded(px(4.0))
                                .bg(rgb(state_col))
                                .text_xs()
                                .text_color(rgb(0xFFFFFF))
                                .child(rec.state.as_str()),
                        )
                        .when(rec.state == SkillInstallState::Staged, |el| {
                            el.child(
                                div()
                                    .id(SharedString::from(format!("orca-skill-commit-{iid}")))
                                    .cursor_pointer()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(0x16A34A))
                                    .text_xs()
                                    .text_color(rgb(0xFFFFFF))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _ev, _w, cx| {
                                            cx.stop_propagation();
                                            let now = chrono::Utc::now().timestamp_millis();
                                            let rt = this.runtime.clone();
                                            if let Err(e) =
                                                this.state.commit_staged_skill(&rt, &iid, now)
                                            {
                                                this.state.error_banner = Some(e);
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child("Commit"),
                            )
                        }),
                ),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(tr_str(
                            lang,
                            "插件系统 V2、MCP 同步与事务技能",
                            "Plugin System V2, MCP Sync & Skills",
                        )),
                )
                .child(
                    div()
                        .id("orca-plugins-enabled-toggle")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(5.0))
                        .bg(if ws.state.plugins_enabled {
                            rgb(0x16A34A)
                        } else {
                            rgb(pal.danger)
                        })
                        .text_xs()
                        .text_color(rgb(0xFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let next = !this.state.plugins_enabled;
                                let _ = this.runtime.ide().plugin_host().set_enabled(next);
                                let rt = this.runtime.clone();
                                this.state.refresh_plugins(&rt);
                                cx.notify();
                            }),
                        )
                        .child(if ws.state.plugins_enabled {
                            "Plugins: ENABLED"
                        } else {
                            "Plugins: DISABLED"
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "Action fallback: {PLUGIN_ACTION_FALLBACK_TIMEOUT_MS}ms · Exit {PLUGIN_APPROVAL_REQUIRED_EXIT_CODE} parity"
                        )),
                )
                .child(
                    div()
                        .id("orca-plugin-hook-policy-toggle")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                this.state.plugin_hook_error_policy =
                                    match this.state.plugin_hook_error_policy {
                                        PluginHookErrorPolicy::Pass => PluginHookErrorPolicy::Deny,
                                        PluginHookErrorPolicy::Deny => PluginHookErrorPolicy::Pass,
                                    };
                                cx.notify();
                            }),
                        )
                        .child(match ws.state.plugin_hook_error_policy {
                            PluginHookErrorPolicy::Pass => "Hook onError: pass",
                            PluginHookErrorPolicy::Deny => "Hook onError: deny",
                        }),
                ),
        )
        .child(tiers_row)
        .child(contrib_grid)
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-plugin-id-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if pid_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::PluginIdDraft, w, cx);
                            }),
                        )
                        .child(if ws.state.plugin_id_draft.is_empty() {
                            "plugin-id".to_string()
                        } else if pid_focused {
                            format!("{}|", ws.state.plugin_id_draft)
                        } else {
                            ws.state.plugin_id_draft.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-plugin-pub-input")
                        .cursor_text()
                        .w(px(96.0))
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if pub_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::PluginPublisherDraft, w, cx);
                            }),
                        )
                        .child(if ws.state.plugin_publisher_draft.is_empty() {
                            "publisher".to_string()
                        } else if pub_focused {
                            format!("{}|", ws.state.plugin_publisher_draft)
                        } else {
                            ws.state.plugin_publisher_draft.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-plugin-validate-btn")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let rt = this.runtime.clone();
                                if let Err(e) = this.state.validate_draft_plugin(&rt) {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("Validate"),
                )
                .child(
                    div()
                        .id("orca-plugin-install-btn")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let now = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                match this.state.install_draft_plugin(&rt, now) {
                                    Ok(rec) => {
                                        if rec.manifest.tier.requires_approval() {
                                            this.open_plugin_approval_security_modal(
                                                &rec.manifest.id,
                                                cx,
                                            );
                                        }
                                    }
                                    Err(e) => this.state.error_banner = Some(e),
                                }
                                cx.notify();
                            }),
                        )
                        .child("Install"),
                ),
        )
        .when_some(ws.state.last_plugin_validation.clone(), |el, sum| {
            el.child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(format!(
                        "Validated {} [{}] · actor={} · exit={} · fp={}",
                        sum.plugin_id,
                        sum.tier.as_str(),
                        sum.audit_actor,
                        sum.approval_exit_code,
                        sum.fingerprint
                    )),
            )
        })
        .child(installed_col)
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_secondary))
                .child("MCP Config Sync (Bot Home Secret Stripping)"),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-mcp-name-input")
                        .cursor_text()
                        .w(px(100.0))
                        .px(px(6.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if mcp_name_f { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::McpServerName, w, cx);
                            }),
                        )
                        .child(if ws.state.mcp_server_name_draft.is_empty() {
                            "mcp name".to_string()
                        } else {
                            ws.state.mcp_server_name_draft.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-mcp-cmd-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(6.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if mcp_cmd_f { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::McpServerCommand, w, cx);
                            }),
                        )
                        .child(if ws.state.mcp_server_cmd_draft.is_empty() {
                            "command (e.g. npx mcp-server)".to_string()
                        } else {
                            ws.state.mcp_server_cmd_draft.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-mcp-sync-btn")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let rt = this.runtime.clone();
                                if let Err(e) = this.state.run_mcp_sync(&rt) {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("Sync MCP"),
                ),
        )
        .when_some(ws.state.last_mcp_sync_report.clone(), |el, rep| {
            el.child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.success))
                    .child(format!(
                        "MCP synced {} server(s) -> {} · secrets_stripped={}",
                        rep.merged_servers, rep.target_path, rep.secrets_stripped
                    )),
            )
        })
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_secondary))
                .child("Transactional Skill Installer (pending -> staged -> committed / recovery)"),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-skill-name-input")
                        .cursor_text()
                        .w(px(100.0))
                        .px(px(6.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if skill_name_f { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::SkillNameDraft, w, cx);
                            }),
                        )
                        .child(if ws.state.skill_name_draft.is_empty() {
                            "skill-slug".to_string()
                        } else {
                            ws.state.skill_name_draft.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-skill-src-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(6.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if skill_src_f { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::SkillSourceDirDraft, w, cx);
                            }),
                        )
                        .child(if ws.state.skill_source_dir_draft.is_empty() {
                            "/path/to/skill/dir".to_string()
                        } else {
                            ws.state.skill_source_dir_draft.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-skill-stage-btn")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let now = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                if let Err(e) = this.state.stage_or_install_skill(&rt, false, now) {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("Stage"),
                )
                .child(
                    div()
                        .id("orca-skill-recover-btn")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgb(0xF59E0B))
                        .text_xs()
                        .text_color(rgb(0x111827))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let now = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                if let Err(e) = this.state.recover_skills(&rt, now) {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("Recover"),
                ),
        )
        .when_some(ws.state.last_skill_recovery.clone(), |el, rep| {
            el.child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(format!(
                        "Skill recovery healed {} install(s), cleaned {} staging dir(s)",
                        rep.healed_install_ids.len(),
                        rep.cleaned_staging_dirs
                    )),
            )
        })
        .child(skills_col)
        .into_any_element()
}
