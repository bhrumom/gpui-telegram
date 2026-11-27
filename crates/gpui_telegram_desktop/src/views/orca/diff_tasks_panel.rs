//! Annotate AI Diff, Local Per-Line AI-vs-Human Gutter Attribution, Conventional Commit /
//! Stacked PR Draft Composer, and 4-Forge Tasks Hub (`F17`, `F22`).

use gpui::prelude::FluentBuilder;
use gpui::*;
use orca_core::orchestration_protocol::{DiffAnnotation, LineAttribution};
use orca_ide::TaskForge;

use crate::i18n::tr_str;
use crate::theme::{TdesktopThemePalette};

use super::ide_state::{DiffDisplayMode, DiffTasksSubTab, IdeInputTarget, OrcaIdeAction};
use super::ide_workspace::OrcaIdeWorkspace;

pub fn attribution_gutter_meta(attr: &LineAttribution) -> (&'static str, u32) {
    match attr {
        LineAttribution::Human => ("HUMAN", 0x16A34A),
        LineAttribution::Bot { .. } => ("BOT", 0x8B5CF6),
        LineAttribution::Worker { .. } => ("WORKER", 0x0EA5E9),
        LineAttribution::Unknown => ("—", 0x64748B),
    }
}

/// Renders the Diff / Attribution / Commit-PR / Tasks Hub slide-over drawer.
pub fn render_diff_tasks_drawer(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();

    let mut subtabs = div().flex().flex_row().gap(px(5.0));
    for (tab, lbl) in [
        (DiffTasksSubTab::AnnotateDiff, "Annotate Diff & Attribution"),
        (DiffTasksSubTab::CommitPr, "Commit & Stacked PR"),
        (DiffTasksSubTab::TasksHub, "Tasks Hub (4 Forges)"),
    ] {
        let active = ws.state.diff_tab == tab;
        subtabs = subtabs.child(
            div()
                .id(SharedString::from(format!("orca-diff-subtab-{tab:?}")))
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
                    rgb(pal.text_secondary)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        this.state.diff_tab = tab;
                        let rt = this.runtime.clone();
                        let _ = this.state.refresh_diff_and_tasks(&rt);
                        cx.notify();
                    }),
                )
                .child(lbl),
        );
    }

    let body = match ws.state.diff_tab {
        DiffTasksSubTab::AnnotateDiff => render_annotate_diff_section(ws, cx),
        DiffTasksSubTab::CommitPr => render_commit_pr_section(ws, cx),
        DiffTasksSubTab::TasksHub => render_tasks_hub_section(ws, cx),
    };

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.text_primary))
                .child(tr_str(
                    lang,
                    "AI Diff 审阅、本地归属与任务中心",
                    "Annotate AI Diff, Attribution & Tasks Hub",
                )),
        )
        .child(subtabs)
        .child(body)
        .into_any_element()
}

fn render_annotate_diff_section(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let fp_focused = ws.state.active_input == IdeInputTarget::DiffFilePath;
    let lstart_focused = ws.state.active_input == IdeInputTarget::DiffLineStart;
    let lend_focused = ws.state.active_input == IdeInputTarget::DiffLineEnd;
    let body_focused = ws.state.active_input == IdeInputTarget::DiffAnnotationBody;

    let export_summary = ws.state.attribution_export.as_ref().map(|ex| {
        format!(
            "Local-only gutter attribution (never written to git commits) · Human: {} · AI: {} · Unknown: {}",
            ex.human_lines, ex.ai_lines, ex.unknown_lines
        )
    });

    // Build interactive gutter rows from live attributed & annotated lines (minimum 1..=8 window)
    let max_line = ws
        .state
        .diff_annotations
        .iter()
        .map(|a| a.line_end)
        .chain(ws.state.file_attributions.keys().copied())
        .max()
        .unwrap_or(6)
        .clamp(6, 40);

    let mut gutter_rows = div().flex().flex_col().gap(px(3.0));
    for line_no in 1..=max_line {
        let attr = ws
            .state
            .file_attributions
            .get(&line_no)
            .cloned()
            .unwrap_or(LineAttribution::Unknown);
        let (attr_lbl, attr_col) = attribution_gutter_meta(&attr);
        let line_anns = ws
            .state
            .diff_annotations
            .iter()
            .filter(|a| line_no >= a.line_start && line_no <= a.line_end)
            .count();

        gutter_rows = gutter_rows.child(
            div()
                .px(px(6.0))
                .py(px(3.0))
                .rounded(px(4.0))
                .bg(rgb(pal.code_bg))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id(("orca-diff-gutter-plus", line_no as usize))
                                .cursor_pointer()
                                .px(px(5.0))
                                .rounded(px(3.0))
                                .bg(rgb(pal.accent))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.active_button_fg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, w, cx| {
                                        cx.stop_propagation();
                                        this.state.diff_line_start_draft = line_no.to_string();
                                        this.state.diff_line_end_draft = line_no.to_string();
                                        this.set_active_input(
                                            IdeInputTarget::DiffAnnotationBody,
                                            w,
                                            cx,
                                        );
                                    }),
                                )
                                .child("+"),
                        )
                        .child(
                            div()
                                .w(px(32.0))
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("L{line_no}")),
                        )
                        .when(ws.state.show_ai_attribution, |el| {
                            el.child(
                                div()
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(rgb(attr_col))
                                    .text_xs()
                                    .text_color(rgb(0xFFFFFF))
                                    .child(attr_lbl),
                            )
                        })
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.code_fg))
                                .child(match ws.state.diff_display_mode {
                                    DiffDisplayMode::SideBySide => {
                                        format!("{} | {:?} L{line_no}", ws.state.diff_file_path, ws.state.diff_side)
                                    }
                                    DiffDisplayMode::Unified => {
                                        format!("@@ {} L{line_no} @@", ws.state.diff_file_path)
                                    }
                                }),
                        ),
                )
                .when(line_anns > 0, |el| {
                    el.child(
                        div()
                            .px(px(5.0))
                            .py(px(1.0))
                            .rounded(px(4.0))
                            .bg(rgb(0xF59E0B))
                            .text_xs()
                            .text_color(rgb(0xFFFFFF))
                            .child(format!("{line_anns} note(s)")),
                    )
                }),
        );
    }

    render_annotate_diff_body_wrapper(
        ws,
        fp_focused,
        lstart_focused,
        lend_focused,
        body_focused,
        export_summary,
        gutter_rows,
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_annotate_diff_body_wrapper(
    ws: &OrcaIdeWorkspace,
    fp_focused: bool,
    lstart_focused: bool,
    lend_focused: bool,
    body_focused: bool,
    export_summary: Option<String>,
    gutter_rows: Div,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;

    let mut anns_col = div().flex().flex_col().gap(px(5.0));
    for ann in &ws.state.diff_annotations {
        let aid = ann.annotation_id.clone();
        let resolved = ann.resolved;
        anns_col = anns_col.child(
            div()
                .p(px(7.0))
                .rounded(px(6.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if resolved {
                    pal.hairline_border_rgba()
                } else {
                    rgb(0xF59E0B)
                })
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!(
                                    "{}:L{}-L{} ({:?}) {}",
                                    ann.file_path,
                                    ann.line_start,
                                    ann.line_end,
                                    ann.side,
                                    if resolved { "[RESOLVED]" } else { "[OPEN]" }
                                )),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .child(ann.body.clone()),
                        ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("orca-ann-toggle-{aid}")))
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(if resolved { rgb(pal.card_bg) } else { rgb(pal.success) })
                        .text_xs()
                        .text_color(if resolved {
                            rgb(pal.text_primary)
                        } else {
                            rgb(0xFFFFFF)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                if let Err(e) = this.state.resolve_or_reopen_annotation(
                                    &rt, &aid, !resolved, now_ms,
                                ) {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child(if resolved { "Reopen" } else { "Resolve" }),
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
                .flex_wrap()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-diff-mode-toggle")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                this.state.diff_display_mode = match this.state.diff_display_mode {
                                    DiffDisplayMode::SideBySide => DiffDisplayMode::Unified,
                                    DiffDisplayMode::Unified => DiffDisplayMode::SideBySide,
                                };
                                cx.notify();
                            }),
                        )
                        .child(match ws.state.diff_display_mode {
                            DiffDisplayMode::SideBySide => "Mode: Side-by-Side",
                            DiffDisplayMode::Unified => "Mode: Unified",
                        }),
                )
                .child(
                    div()
                        .id("orca-diff-attr-toggle")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(if ws.state.show_ai_attribution {
                            rgb(pal.accent)
                        } else {
                            rgb(pal.input_bg)
                        })
                        .text_xs()
                        .text_color(if ws.state.show_ai_attribution {
                            rgb(pal.active_button_fg)
                        } else {
                            rgb(pal.text_secondary)
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                this.state.show_ai_attribution = !this.state.show_ai_attribution;
                                cx.notify();
                            }),
                        )
                        .child("AI Attribution Gutter"),
                )
                .child(
                    div()
                        .id("orca-diff-shift-plus")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let line = this
                                    .state
                                    .diff_line_start_draft
                                    .trim()
                                    .parse::<u32>()
                                    .unwrap_or(1)
                                    .max(1);
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                let _ = this.state.apply_diff_line_shift(&rt, line, 1, now_ms);
                                cx.notify();
                            }),
                        )
                        .child("Line Shift +1"),
                )
                .child(
                    div()
                        .id("orca-diff-mark-ai")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(rgb(0x8B5CF6))
                        .text_xs()
                        .text_color(rgb(0xFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let s = this.state.diff_line_start_draft.trim().parse().unwrap_or(1).max(1);
                                let e = this.state.diff_line_end_draft.trim().parse().unwrap_or(s).max(s);
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                let _ = this.state.record_local_attribution(
                                    &rt,
                                    s,
                                    e,
                                    LineAttribution::Worker {
                                        dispatch_id: "worker-active".to_string(),
                                    },
                                    now_ms,
                                );
                                cx.notify();
                            }),
                        )
                        .child("Mark AI Range"),
                )
                .child(
                    div()
                        .id("orca-diff-mark-human")
                        .cursor_pointer()
                        .px(px(7.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(rgb(0x16A34A))
                        .text_xs()
                        .text_color(rgb(0xFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let s = this.state.diff_line_start_draft.trim().parse().unwrap_or(1).max(1);
                                let e = this.state.diff_line_end_draft.trim().parse().unwrap_or(s).max(s);
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                let _ = this.state.record_local_attribution(
                                    &rt,
                                    s,
                                    e,
                                    LineAttribution::Human,
                                    now_ms,
                                );
                                cx.notify();
                            }),
                        )
                        .child("Mark Human Edit"),
                ),
        )
        .when_some(export_summary, |el, txt| {
            el.child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(5.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(txt),
            )
        })
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-diff-file-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if fp_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::DiffFilePath, w, cx);
                            }),
                        )
                        .child(if fp_focused {
                            format!("{}|", ws.state.diff_file_path)
                        } else {
                            ws.state.diff_file_path.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-diff-lstart-input")
                        .cursor_text()
                        .w(px(52.0))
                        .px(px(6.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if lstart_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::DiffLineStart, w, cx);
                            }),
                        )
                        .child(format!("L{}", ws.state.diff_line_start_draft)),
                )
                .child(
                    div()
                        .id("orca-diff-lend-input")
                        .cursor_text()
                        .w(px(52.0))
                        .px(px(6.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if lend_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::DiffLineEnd, w, cx);
                            }),
                        )
                        .child(format!("..{}", ws.state.diff_line_end_draft)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-diff-ann-body-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if body_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::DiffAnnotationBody, w, cx);
                            }),
                        )
                        .child(if ws.state.diff_annotation_body_draft.is_empty() {
                            "Line-anchored review comment…".to_string()
                        } else if body_focused {
                            format!("{}|", ws.state.diff_annotation_body_draft)
                        } else {
                            ws.state.diff_annotation_body_draft.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-diff-add-ann-btn")
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
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let s = this.state.diff_line_start_draft.trim().parse::<u32>().unwrap_or(1).max(1);
                                let e = this.state.diff_line_end_draft.trim().parse::<u32>().unwrap_or(s).max(s);
                                let ann = DiffAnnotation {
                                    annotation_id: format!("ann-{}", uuid::Uuid::new_v4()),
                                    file_path: this.state.diff_file_path.clone(),
                                    side: this.state.diff_side,
                                    line_start: s,
                                    line_end: e,
                                    body: this.state.diff_annotation_body_draft.trim().to_string(),
                                    author: LineAttribution::Human,
                                    resolved: false,
                                    created_at_ms: now_ms,
                                    updated_at_ms: now_ms,
                                };
                                let rt = this.runtime.clone();
                                match this.state.add_diff_annotation_and_build_batch(&rt, ann) {
                                    Ok(_) => this.state.diff_annotation_body_draft.clear(),
                                    Err(err) => this.state.error_banner = Some(err),
                                }
                                cx.notify();
                            }),
                        )
                        .child("+ Annotate"),
                )
                .child(
                    div()
                        .id("orca-diff-send-batch-btn")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(0x16A34A))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let rt = this.runtime.clone();
                                let batch =
                                    this.state.build_diff_batch_prompt_for_active_file(&rt);
                                if batch.trim().is_empty() {
                                    this.state.status_banner =
                                        Some("No unresolved diff annotations to batch".to_string());
                                } else {
                                    this.state.session_prompt_draft = batch.clone();
                                    this.push_action(OrcaIdeAction::SendPromptToActiveChat(batch));
                                    this.state.status_banner =
                                        Some("Batched annotations sent to active agent".to_string());
                                }
                                cx.notify();
                            }),
                        )
                        .child("Send to agent"),
                ),
        )
        .child(gutter_rows)
        .child(anns_col)
        .when_some(ws.state.last_batch_prompt.clone(), |el, batch| {
            el.child(
                div()
                    .p(px(7.0))
                    .rounded(px(6.0))
                    .bg(rgb(pal.code_bg))
                    .text_xs()
                    .text_color(rgb(pal.code_fg))
                    .child(format!("Last batch prompt:\n{batch}")),
            )
        })
        .into_any_element()
}

fn render_commit_pr_section(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let scope_focused = ws.state.active_input == IdeInputTarget::CommitScope;
    let summary_focused = ws.state.active_input == IdeInputTarget::CommitSummary;
    let parent_focused = ws.state.active_input == IdeInputTarget::CommitParentPr;

    let mut types_row = div().flex().flex_row().flex_wrap().gap(px(4.0));
    for ctype in ["feat", "fix", "refactor", "docs", "test", "chore", "perf", "ci"] {
        let active = ws.state.commit_type == ctype;
        types_row = types_row.child(
            div()
                .id(SharedString::from(format!("orca-commit-type-{ctype}")))
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
                        this.state.commit_type = ctype.to_string();
                        cx.notify();
                    }),
                )
                .child(ctype),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(types_row)
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-commit-scope-input")
                        .cursor_text()
                        .w(px(110.0))
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if scope_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::CommitScope, w, cx);
                            }),
                        )
                        .child(if ws.state.commit_scope.is_empty() {
                            "scope".to_string()
                        } else if scope_focused {
                            format!("{}|", ws.state.commit_scope)
                        } else {
                            ws.state.commit_scope.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-commit-summary-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if summary_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::CommitSummary, w, cx);
                            }),
                        )
                        .child(if ws.state.commit_summary.is_empty() {
                            "Conventional commit summary…".to_string()
                        } else if summary_focused {
                            format!("{}|", ws.state.commit_summary)
                        } else {
                            ws.state.commit_summary.clone()
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-commit-parent-pr-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if parent_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::CommitParentPr, w, cx);
                            }),
                        )
                        .child(if ws.state.commit_parent_pr.is_empty() {
                            "Stacked parent PR (optional, e.g. #412)…".to_string()
                        } else if parent_focused {
                            format!("{}|", ws.state.commit_parent_pr)
                        } else {
                            ws.state.commit_parent_pr.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-commit-generate-btn")
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
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                if let Err(e) = this.state.generate_commit_and_pr_draft() {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("Generate Commit & PR"),
                ),
        )
        .when_some(ws.state.last_commit_pr_draft.clone(), |el, draft| {
            el.child(
                div()
                    .p(px(8.0))
                    .rounded(px(6.0))
                    .bg(rgb(pal.code_bg))
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(draft.commit_title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.code_fg))
                            .child(draft.pr_body_markdown),
                    ),
            )
        })
        .into_any_element()
}

fn render_tasks_hub_section(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let search_focused = ws.state.active_input == IdeInputTarget::TaskHubSearch;
    let key_focused = ws.state.active_input == IdeInputTarget::TaskHubExternalKey;
    let title_focused = ws.state.active_input == IdeInputTarget::TaskHubTitle;
    let url_focused = ws.state.active_input == IdeInputTarget::TaskHubArtifactUrl;
    let tpl_focused = ws.state.active_input == IdeInputTarget::TaskHubCommandTemplate;

    let mut filter_bar = div().flex().flex_row().flex_wrap().gap(px(4.0)).child(
        div()
            .id("orca-hub-filter-all")
            .cursor_pointer()
            .px(px(7.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .bg(if ws.state.tasks_hub_forge_filter.is_none() {
                rgb(pal.accent)
            } else {
                rgb(pal.input_bg)
            })
            .text_xs()
            .text_color(if ws.state.tasks_hub_forge_filter.is_none() {
                rgb(pal.active_button_fg)
            } else {
                rgb(pal.text_secondary)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev, _w, cx| {
                    cx.stop_propagation();
                    this.state.tasks_hub_forge_filter = None;
                    let rt = this.runtime.clone();
                    let _ = this.state.refresh_diff_and_tasks(&rt);
                    cx.notify();
                }),
            )
            .child("All (4)"),
    );
    for forge in TaskForge::ALL {
        let active = ws.state.tasks_hub_forge_filter == Some(forge);
        filter_bar = filter_bar.child(
            div()
                .id(SharedString::from(format!("orca-hub-filter-{}", forge.as_str())))
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
                        this.state.tasks_hub_forge_filter = Some(forge);
                        this.state.new_hub_task_forge = forge;
                        let rt = this.runtime.clone();
                        let _ = this.state.refresh_diff_and_tasks(&rt);
                        cx.notify();
                    }),
                )
                .child(forge.as_str()),
        );
    }

    let mut tasks_col = div().flex().flex_col().gap(px(5.0));
    for task in &ws.state.hub_tasks {
        let tid_launch = task.task_id.clone();
        let tid_del = task.task_id.clone();
        tasks_col = tasks_col.child(
            div()
                .p(px(7.0))
                .rounded(px(6.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!(
                                    "[{}] {} · {}",
                                    task.forge.as_str(),
                                    task.external_key,
                                    task.title
                                )),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("{} · {}", task.status, task.artifact_url)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(4.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-hub-launch-{tid_launch}")))
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
                                        if let Err(e) =
                                            this.state.build_hub_task_launch_command(&rt, &tid_launch)
                                        {
                                            this.state.error_banner = Some(e);
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("Launch"),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-hub-del-{tid_del}")))
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .bg(rgb(pal.danger))
                                .text_xs()
                                .text_color(rgb(0xFFFFFF))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let rt = this.runtime.clone();
                                        let _ = this.state.delete_hub_task(&rt, &tid_del);
                                        cx.notify();
                                    }),
                                )
                                .child("Del"),
                        ),
                ),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(filter_bar)
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-hub-search-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if search_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::TaskHubSearch, w, cx);
                            }),
                        )
                        .child(if ws.state.tasks_hub_search.is_empty() {
                            "Search issues across GitHub/GitLab/Linear/Jira…".to_string()
                        } else if search_focused {
                            format!("{}|", ws.state.tasks_hub_search)
                        } else {
                            ws.state.tasks_hub_search.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-hub-tpl-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if tpl_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::TaskHubCommandTemplate, w, cx);
                            }),
                        )
                        .child(if tpl_focused {
                            format!("{}|", ws.state.issue_command_template)
                        } else {
                            ws.state.issue_command_template.clone()
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(4.0))
                .child(
                    div()
                        .id("orca-hub-new-key")
                        .cursor_text()
                        .w(px(84.0))
                        .px(px(6.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if key_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::TaskHubExternalKey, w, cx);
                            }),
                        )
                        .child(if ws.state.new_hub_task_key.is_empty() {
                            "KEY-1".to_string()
                        } else {
                            ws.state.new_hub_task_key.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-hub-new-title")
                        .cursor_text()
                        .flex_1()
                        .px(px(6.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if title_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::TaskHubTitle, w, cx);
                            }),
                        )
                        .child(if ws.state.new_hub_task_title.is_empty() {
                            "Issue title…".to_string()
                        } else {
                            ws.state.new_hub_task_title.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-hub-new-url")
                        .cursor_text()
                        .flex_1()
                        .px(px(6.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if url_focused { rgb(pal.accent) } else { pal.hairline_border_rgba() })
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::TaskHubArtifactUrl, w, cx);
                            }),
                        )
                        .child(if ws.state.new_hub_task_url.is_empty() {
                            "https://github.com/…".to_string()
                        } else {
                            ws.state.new_hub_task_url.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-hub-upsert-btn")
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
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                if let Err(e) = this.state.upsert_hub_task(&rt, now_ms) {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("+ Upsert"),
                ),
        )
        .when_some(ws.state.last_expanded_launch_cmd.clone(), |el, cmd| {
            el.child(
                div()
                    .px(px(7.0))
                    .py(px(5.0))
                    .rounded(px(5.0))
                    .bg(rgb(pal.code_bg))
                    .text_xs()
                    .text_color(rgb(pal.code_fg))
                    .child(format!("Safe expanded launch command: {cmd}")),
            )
        })
        .child(tasks_col)
        .into_any_element()
}
