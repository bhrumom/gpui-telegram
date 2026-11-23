use crate::i18n::tr_str;
use crate::theme::{
    assets::{tg_icon, IconName},
    TdesktopThemePalette,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolApprovalDecision {
    AllowOnce,
    AllowAlways,
    Deny,
}

/// Grok Bot 4-tier tool approval policy badge (`自动审核` / `需要批准` / `已阻止` / `始终允许`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolApprovalPolicyTier {
    AutoReview,
    #[default]
    Ask,
    Blocked,
    AlwaysAllow,
}

impl ToolApprovalPolicyTier {
    pub fn label(&self, lang_id: &str) -> &'static str {
        match self {
            Self::AutoReview => tr_str(lang_id, "自动审核", "Auto Review"),
            Self::Ask => tr_str(lang_id, "需要批准", "Needs Approval"),
            Self::Blocked => tr_str(lang_id, "已阻止", "Blocked"),
            Self::AlwaysAllow => tr_str(lang_id, "始终允许", "Always Allow"),
        }
    }

    pub const fn hue_name(&self) -> &'static str {
        match self {
            Self::AutoReview => "cyan",
            Self::Ask => "yellow",
            Self::Blocked => "red",
            Self::AlwaysAllow => "green",
        }
    }

    pub fn from_policy_str(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto_review" | "auto-review" | "autoreview" | "smart_approve" => Self::AutoReview,
            "blocked" | "deny" | "denied" | "critical" => Self::Blocked,
            "always_allow" | "always-allow" | "alwaysallow" | "permanent_allow" | "yolo" => {
                Self::AlwaysAllow
            }
            _ => Self::Ask,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PendingToolApproval {
    pub id: String,
    pub tool_name: String,
    pub summary: String,
    pub arguments_preview: String,
    pub risk_level: String,
    pub created_at: String,
    pub target: String,
    pub cwd: String,
    pub policy_tier: ToolApprovalPolicyTier,
}

impl PendingToolApproval {
    pub fn new(
        id: impl Into<String>,
        tool_name: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            tool_name: tool_name.into(),
            summary: summary.into(),
            arguments_preview: String::new(),
            risk_level: "ask".to_string(),
            created_at: String::new(),
            target: String::new(),
            cwd: String::new(),
            policy_tier: ToolApprovalPolicyTier::Ask,
        }
    }

    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = target.into();
        self
    }

    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = cwd.into();
        self
    }

    pub fn with_policy_tier(mut self, tier: ToolApprovalPolicyTier) -> Self {
        self.policy_tier = tier;
        self
    }

    pub fn with_risk_level(mut self, risk: impl Into<String>) -> Self {
        self.risk_level = risk.into();
        self
    }

    pub fn effective_target(&self) -> &str {
        if !self.target.trim().is_empty() {
            self.target.trim()
        } else {
            self.arguments_preview.trim()
        }
    }
}

pub struct ToolPermissionDock {
    pub pending_count: usize,
    pub pending_requests: Vec<PendingToolApproval>,
    pub current_index: usize,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub last_decision: Option<(String, ToolApprovalDecision)>,
    pub on_decision: Option<Arc<dyn Fn(String, ToolApprovalDecision) + Send + Sync>>,
}

impl ToolPermissionDock {
    pub fn new(pending_count: usize) -> Self {
        Self {
            pending_count,
            pending_requests: Vec::new(),
            current_index: 0,
            palette: TdesktopThemePalette::default(),
            language_id: "en".to_string(),
            last_decision: None,
            on_decision: None,
        }
    }

    pub fn with_palette(
        mut self,
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
    ) -> Self {
        self.palette = palette;
        self.language_id = language_id.into();
        self
    }

    pub fn has_pending(&self) -> bool {
        !self.pending_requests.is_empty()
    }

    pub fn active_request(&self) -> Option<&PendingToolApproval> {
        if self.pending_requests.is_empty() {
            return None;
        }
        let idx = self.current_index.min(self.pending_requests.len() - 1);
        self.pending_requests.get(idx)
    }

    pub fn step_prev(&mut self) {
        let len = self.pending_requests.len();
        if len <= 1 {
            self.current_index = 0;
            return;
        }
        self.current_index = if self.current_index == 0 {
            len - 1
        } else {
            self.current_index - 1
        };
    }

    pub fn step_next(&mut self) {
        let len = self.pending_requests.len();
        if len <= 1 {
            self.current_index = 0;
            return;
        }
        self.current_index = (self.current_index + 1) % len;
    }

    pub fn sync_requests(&mut self, requests: Vec<PendingToolApproval>) {
        self.pending_requests = requests;
        self.pending_count = self.pending_requests.len();
        if self.pending_requests.is_empty() {
            self.current_index = 0;
        } else if self.current_index >= self.pending_requests.len() {
            self.current_index = self.pending_requests.len() - 1;
        }
    }

    pub fn push_request(&mut self, req: PendingToolApproval) {
        if !self.pending_requests.iter().any(|r| r.id == req.id) {
            self.pending_requests.push(req);
            self.pending_count = self.pending_requests.len();
        }
    }

    pub fn decide(&mut self, id: &str, decision: ToolApprovalDecision) {
        self.pending_requests.retain(|r| r.id != id);
        self.pending_count = self.pending_requests.len();
        if self.pending_requests.is_empty() {
            self.current_index = 0;
        } else if self.current_index >= self.pending_requests.len() {
            self.current_index = self.pending_requests.len() - 1;
        }
        self.last_decision = Some((id.to_string(), decision));
        if let Some(ref cb) = self.on_decision {
            cb(id.to_string(), decision);
        }
    }

    pub fn take_last_decision(&mut self) -> Option<(String, ToolApprovalDecision)> {
        self.last_decision.take()
    }
}

/// Renders Grok Bot's `"需要批准"` inline approval card docked directly above the composer
/// whenever pending tool approvals exist. Supports 4-tier policy badges, action/target/cwd
/// summary, prev/next approval stepping (`上一个审批` / `下一个审批`), and Allow Once / Always Allow / Deny.
#[allow(clippy::too_many_arguments)]
pub fn render_tool_permission_dock_inline<V: 'static>(
    dock: &ToolPermissionDock,
    pal: &TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<V>,
    on_prev: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_next: impl Fn(&mut V, &mut Context<V>) + 'static,
    on_decide: impl Fn(&mut V, String, ToolApprovalDecision, &mut Context<V>) + Clone + 'static,
) -> Option<AnyElement> {
    let req = dock.active_request()?;
    let total = dock.pending_requests.len();
    let idx = dock.current_index.min(total.saturating_sub(1));
    let is_dark = pal.mode.is_dark();

    let (_, tier_pill_bg, tier_pill_fg) =
        TdesktopThemePalette::bot_hue_colors(req.policy_tier.hue_name(), is_dark);
    let (_, _, warn_fg) = TdesktopThemePalette::bot_hue_colors("yellow", is_dark);
    let (allow_once_bg, _, _) = TdesktopThemePalette::bot_hue_colors("blue", is_dark);
    let (_, always_bg, always_fg) = TdesktopThemePalette::bot_hue_colors("green", is_dark);
    let (_, deny_bg, deny_fg) = TdesktopThemePalette::bot_hue_colors("red", is_dark);

    let id_once = req.id.clone();
    let id_always = req.id.clone();
    let id_deny = req.id.clone();
    let cb_once = on_decide.clone();
    let cb_always = on_decide.clone();
    let cb_deny = on_decide;

    let target_text = req.effective_target().to_string();
    let cwd_text = req.cwd.trim().to_string();
    let summary_text = req.summary.trim().to_string();
    let risk_text = req.risk_level.trim().to_string();
    let tier_label = req.policy_tier.label(lang);
    let row_hover = pal.row_hover_rgba();

    let card = div()
        .id("tool-permission-dock-card")
        .w_full()
        .max_w(px(740.0))
        .p(px(12.0))
        .rounded(px(14.0))
        .bg(rgb(pal.card_bg))
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .when(is_dark, |el| {
            el.border_t_1()
                .border_color(pal.specular_top_highlight_rgba())
        })
        .shadow(pal.popover_shadows())
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .min_w(px(0.0))
                        .child(tg_icon(IconName::TerminalMcp, 15.0, warn_fg))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(lang, "需要批准", "Needs Approval")),
                        )
                        .child(
                            div()
                                .px(px(7.0))
                                .py(px(1.5))
                                .rounded_full()
                                .bg(rgb(tier_pill_bg))
                                .text_size(px(10.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(tier_pill_fg))
                                .child(tier_label),
                        ),
                )
                .when(total > 1, |row| {
                    row.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .flex_shrink_0()
                            .child(
                                div()
                                    .id("tool-perm-prev-btn")
                                    .cursor_pointer()
                                    .px(px(7.0))
                                    .py(px(2.5))
                                    .rounded(px(7.0))
                                    .hover(move |s| s.bg(row_hover))
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(pal.text_secondary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            on_prev(this, cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "上一个审批", "Prev")),
                            )
                            .child(
                                div()
                                    .px(px(4.0))
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_secondary))
                                    .child(format!("{} / {}", idx + 1, total)),
                            )
                            .child(
                                div()
                                    .id("tool-perm-next-btn")
                                    .cursor_pointer()
                                    .px(px(7.0))
                                    .py(px(2.5))
                                    .rounded(px(7.0))
                                    .hover(move |s| s.bg(row_hover))
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(pal.text_secondary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            on_next(this, cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "下一个审批", "Next")),
                            ),
                    )
                }),
        )
        .child(
            div()
                .w_full()
                .px(px(10.0))
                .py(px(8.0))
                .rounded(px(10.0))
                .bg(rgb(pal.window_bg))
                .border_1()
                .border_color(pal.separator_rgba())
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .truncate()
                                .child(req.tool_name.clone()),
                        )
                        .when(!risk_text.is_empty(), |r| {
                            r.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(if req.created_at.trim().is_empty() {
                                        risk_text
                                    } else {
                                        format!("{} · {}", risk_text, req.created_at)
                                    }),
                            )
                        }),
                )
                .when(!summary_text.is_empty(), |col| {
                    col.child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(pal.text_secondary))
                            .child(summary_text),
                    )
                })
                .when(!target_text.is_empty(), |col| {
                    col.child(
                        div()
                            .px(px(7.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .bg(rgb(pal.code_bg))
                            .text_size(px(11.5))
                            .text_color(rgb(pal.code_fg))
                            .truncate()
                            .child(target_text),
                    )
                })
                .when(!cwd_text.is_empty(), |col| {
                    col.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(pal.msg_in_date_fg))
                            .truncate()
                            .child(format!("cwd: {cwd_text}")),
                    )
                }),
        )
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(
                    div()
                        .id("tool-perm-deny-btn")
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(6.0))
                        .rounded(px(8.0))
                        .bg(rgb(deny_bg))
                        .border_1()
                        .border_color(TdesktopThemePalette::rgba_with_alpha(deny_fg, 0.28))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(deny_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_deny(this, id_deny.clone(), ToolApprovalDecision::Deny, cx);
                            }),
                        )
                        .child(tr_str(lang, "拒绝", "Deny")),
                )
                .child(
                    div()
                        .id("tool-perm-always-btn")
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(6.0))
                        .rounded(px(8.0))
                        .bg(rgb(always_bg))
                        .border_1()
                        .border_color(TdesktopThemePalette::rgba_with_alpha(always_fg, 0.28))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(always_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_always(
                                    this,
                                    id_always.clone(),
                                    ToolApprovalDecision::AllowAlways,
                                    cx,
                                );
                            }),
                        )
                        .child(tr_str(lang, "始终允许", "Always Allow")),
                )
                .child(
                    div()
                        .id("tool-perm-once-btn")
                        .cursor_pointer()
                        .px(px(13.0))
                        .py(px(6.0))
                        .rounded(px(8.0))
                        .bg(rgb(allow_once_bg))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_once(
                                    this,
                                    id_once.clone(),
                                    ToolApprovalDecision::AllowOnce,
                                    cx,
                                );
                            }),
                        )
                        .child(tr_str(lang, "允许一次", "Allow Once")),
                ),
        );

    Some(
        div()
            .id("stage-tool-permission-dock-wrap")
            .w_full()
            .flex_shrink_0()
            .px(px(20.0))
            .pt(px(4.0))
            .pb(px(6.0))
            .bg(rgb(pal.stage_bg))
            .flex()
            .justify_center()
            .child(card)
            .into_any_element(),
    )
}

impl Render for ToolPermissionDock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let lang = self.language_id.clone();
        if let Some(el) = render_tool_permission_dock_inline(
            self,
            &pal,
            &lang,
            cx,
            |this, cx| {
                this.step_prev();
                cx.notify();
            },
            |this, cx| {
                this.step_next();
                cx.notify();
            },
            |this, id, decision, cx| {
                this.decide(&id, decision);
                cx.notify();
            },
        ) {
            return el;
        }

        div()
            .w_full()
            .p(px(12.0))
            .rounded(px(12.0))
            .bg(rgb(pal.card_bg))
            .border_1()
            .border_color(pal.hairline_border_rgba())
            .text_size(px(12.0))
            .text_color(rgb(pal.text_secondary))
            .child(tr_str(
                &lang,
                "暂无需要人工授权的工具调用请求。",
                "No tool calls currently awaiting approval.",
            ))
            .into_any_element()
    }
}
