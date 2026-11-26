//! 5-Column Agent Kanban, Durable Multi-Agent Orchestration Board, and Pop-Out Kanban
//! Window (`AgentPopoutKanban` via `cx.open_window(...)`) (`F11`, `F13`, `F22`).
//!
//! Strictly honors non-guessing worker liveness: `WorkerLivenessState::Unverifiable`
//! renders an explicit warning badge and NEVER offers autonomous retry.

use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use orca_core::orchestration_protocol::{
    GateStatus, MailboxMessageType, MailboxPriority, OrchestrationTaskStatus, WorkerLivenessState,
};
use orca_ide::{AgentRunState, AgentSessionStatus, DecisionGateKind, WorktreeRecord};

use crate::i18n::tr_str;
use crate::theme::{IconName, TdesktopThemePalette, tg_icon};

use super::ide_state::{IdeInputTarget, OrcaIdeAction, OrchestrationBoardTab};
use super::ide_workspace::OrcaIdeWorkspace;
use super::runtime::OrcaDesktopRuntime;

pub const fn agent_run_state_meta(state: AgentRunState) -> (&'static str, u32) {
    match state {
        AgentRunState::Working => ("Working", 0x3B82F6),
        AgentRunState::Blocked => ("Blocked", 0xEF4444),
        AgentRunState::Waiting => ("Waiting", 0xF59E0B),
        AgentRunState::Done => ("Done", 0x16A34A),
        AgentRunState::Idle => ("Idle", 0x64748B),
    }
}

/// Standalone Pop-out Kanban window view (`AgentPopoutKanban`) opened via `cx.open_window(...)`
/// with live state sync directly against `OrcaDesktopRuntime`.
pub struct AgentPopoutKanban {
    runtime: Arc<OrcaDesktopRuntime>,
    palette: TdesktopThemePalette,
    language_id: String,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    statuses: Vec<AgentSessionStatus>,
    worktrees: Vec<WorktreeRecord>,
    last_synced_ms: i64,
}

impl AgentPopoutKanban {
    pub fn new(
        runtime: Arc<OrcaDesktopRuntime>,
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            runtime,
            palette,
            language_id: language_id.into(),
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            statuses: Vec::new(),
            worktrees: Vec::new(),
            last_synced_ms: 0,
        };
        this.sync_from_runtime();
        this
    }

    pub fn sync_from_runtime(&mut self) {
        self.statuses = self.runtime.ide().agent_status().list_statuses();
        self.worktrees = self
            .runtime
            .ide()
            .worktrees()
            .list_worktrees()
            .unwrap_or_default();
        self.last_synced_ms = chrono::Utc::now().timestamp_millis();
    }

    pub fn kanban_columns(&self) -> [(AgentRunState, Vec<AgentSessionStatus>); 5] {
        let mut working = Vec::new();
        let mut blocked = Vec::new();
        let mut waiting = Vec::new();
        let mut done = Vec::new();
        let mut idle = Vec::new();
        for st in &self.statuses {
            match st.state {
                AgentRunState::Working => working.push(st.clone()),
                AgentRunState::Blocked => blocked.push(st.clone()),
                AgentRunState::Waiting => waiting.push(st.clone()),
                AgentRunState::Done => done.push(st.clone()),
                AgentRunState::Idle => idle.push(st.clone()),
            }
        }
        [
            (AgentRunState::Working, working),
            (AgentRunState::Blocked, blocked),
            (AgentRunState::Waiting, waiting),
            (AgentRunState::Done, done),
            (AgentRunState::Idle, idle),
        ]
    }
}

impl Focusable for AgentPopoutKanban {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AgentPopoutKanban {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let columns = self.kanban_columns();

        div()
            .id("orca-popout-kanban-root")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(rgb(pal.window_bg))
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(tg_icon(IconName::BotRobot, 16.0, pal.accent))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(tr_str(
                                        lang,
                                        "独立 Agent 看板窗口",
                                        "Orca Pop-out Agent Kanban",
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id("orca-popout-kanban-sync-btn")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .bg(rgb(pal.accent))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.active_button_fg))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _w, cx| {
                                    this.sync_from_runtime();
                                    cx.notify();
                                }),
                            )
                            .child(tr_str(lang, "刷新状态", "Sync Now")),
                    ),
            )
            .child(
                div()
                    .id("orca-popout-kanban-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(render_kanban_columns_grid(&columns, &self.worktrees, pal, now_ms)),
            )
            .into_any_element()
    }
}

/// Opens a detached GPUI `AgentPopoutKanban` window synced with `runtime`.
pub fn open_popout_kanban_window(
    runtime: Arc<OrcaDesktopRuntime>,
    palette: TdesktopThemePalette,
    language_id: String,
    cx: &mut App,
) -> Result<WindowHandle<AgentPopoutKanban>, String> {
    let bounds = Bounds::centered(None, size(px(960.0), px(580.0)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Orca Agent Kanban")),
                ..Default::default()
            }),
            focus: true,
            show: true,
            window_min_size: Some(size(px(640.0), px(400.0))),
            ..Default::default()
        },
        move |_window, cx| cx.new(|cx| AgentPopoutKanban::new(runtime, palette, language_id, cx)),
    )
    .map_err(|e| format!("failed to open pop-out kanban window: {e}"))
}

fn render_kanban_columns_grid(
    columns: &[(AgentRunState, Vec<AgentSessionStatus>); 5],
    worktrees: &[WorktreeRecord],
    pal: TdesktopThemePalette,
    now_ms: i64,
) -> AnyElement {
    let mut row = div().flex().flex_row().flex_wrap().gap(px(8.0));
    for (state, items) in columns {
        let (title, color) = agent_run_state_meta(*state);
        let mut cards = div().flex().flex_col().gap(px(6.0));
        if items.is_empty() {
            cards = cards.child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child("—"),
            );
        } else {
            for st in items {
                let cache_label = st
                    .prompt_cache_remaining_ms(now_ms)
                    .map(|ms| format!("cache {}s", ms / 1000));
                cards = cards.child(
                    div()
                        .p(px(7.0))
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
                                .text_color(rgb(pal.text_primary))
                                .child(format!("{} · {}", st.agent_id, st.session_id)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap(px(4.0))
                                .when(st.has_active_subagents(), |el| {
                                    el.child(
                                        div()
                                            .px(px(5.0))
                                            .py(px(1.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(0x6366F1))
                                            .text_xs()
                                            .text_color(rgb(0xFFFFFF))
                                            .child(format!("{} sub", st.subagents.len())),
                                    )
                                })
                                .when_some(cache_label, |el, lbl| {
                                    el.child(
                                        div()
                                            .px(px(5.0))
                                            .py(px(1.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(pal.input_bg))
                                            .text_xs()
                                            .text_color(rgb(pal.text_secondary))
                                            .child(lbl),
                                    )
                                }),
                        ),
                );
            }
        }
        row = row.child(
            div()
                .flex_1()
                .min_w(px(150.0))
                .p(px(8.0))
                .rounded(px(8.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(rgb(color))
                .flex()
                .flex_col()
                .gap(px(6.0))
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
                                .text_color(rgb(color))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("{}", items.len())),
                        ),
                )
                .child(cards),
        );
    }

    let mut wt_badges = div().flex().flex_row().flex_wrap().gap(px(5.0));
    for wt in worktrees {
        let status_lbl = wt.workspace_status.as_deref().unwrap_or("todo");
        wt_badges = wt_badges.child(
            div()
                .px(px(7.0))
                .py(px(3.0))
                .rounded(px(5.0))
                .bg(rgb(pal.card_bg))
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .text_xs()
                .text_color(rgb(pal.text_primary))
                .child(format!("WT {} [{}]", wt.branch_name, status_lbl)),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(row)
        .when(!worktrees.is_empty(), |el| {
            el.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_secondary))
                            .child(format!("Worktree Badges ({})", worktrees.len())),
                    )
                    .child(wt_badges),
            )
        })
        .into_any_element()
}

/// Renders the Orchestration & 5-Column Agent Kanban slide-over drawer inside `OrcaIdeWorkspace`.
pub fn render_orchestration_board_drawer(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let tabs = [
        (OrchestrationBoardTab::Kanban, "Kanban (5)"),
        (OrchestrationBoardTab::RunsDag, "Runs & DAG"),
        (OrchestrationBoardTab::Workers, "Workers"),
        (OrchestrationBoardTab::Mailbox, "Mailbox (9)"),
        (OrchestrationBoardTab::Gates, "Gates"),
        (OrchestrationBoardTab::Hosts, "Hosts"),
    ];

    let mut tab_bar = div().flex().flex_row().flex_wrap().gap(px(4.0));
    for (tab, lbl) in tabs {
        let active = ws.state.board_tab == tab;
        tab_bar = tab_bar.child(
            div()
                .id(SharedString::from(format!("orca-board-tab-{tab:?}")))
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
                        this.state.board_tab = tab;
                        cx.notify();
                    }),
                )
                .child(lbl),
        );
    }

    let body = match ws.state.board_tab {
        OrchestrationBoardTab::Kanban => {
            let cols = ws.state.kanban_columns();
            render_kanban_columns_grid(&cols, &ws.state.worktrees, pal, now_ms)
        }
        OrchestrationBoardTab::RunsDag => render_runs_and_dag_section(ws, cx),
        OrchestrationBoardTab::Workers => render_workers_section(ws, cx),
        OrchestrationBoardTab::Mailbox => render_mailbox_section(ws, cx),
        OrchestrationBoardTab::Gates => render_gates_section(ws, cx),
        OrchestrationBoardTab::Hosts => render_hosts_section(ws),
    };

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
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
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(
                                    lang,
                                    "多智能体编排与看板",
                                    "Agent Kanban & Orchestration",
                                )),
                        )
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded(px(4.0))
                                .bg(rgb(pal.input_bg))
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("wakes: {}", ws.state.recent_wakes.len())),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("orca-board-popout-btn")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(5.0))
                                .bg(if ws.state.popout_kanban.is_open {
                                    rgb(pal.accent)
                                } else {
                                    rgb(pal.input_bg)
                                })
                                .text_xs()
                                .text_color(if ws.state.popout_kanban.is_open {
                                    rgb(pal.active_button_fg)
                                } else {
                                    rgb(pal.text_primary)
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let opened = this.state.toggle_popout_kanban();
                                        if opened {
                                            this.open_popout_kanban_window(cx);
                                            this.push_action(OrcaIdeAction::OpenPopoutKanban);
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child(if ws.state.popout_kanban.is_open {
                                    "Pop-out: ON"
                                } else {
                                    "Pop-out Window"
                                }),
                        )
                        .child(
                            div()
                                .id("orca-board-refresh-btn")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(5.0))
                                .bg(rgb(pal.input_bg))
                                .text_xs()
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let rt = this.runtime.clone();
                                        if let Err(e) =
                                            this.state.refresh_kanban_and_orchestration(&rt)
                                        {
                                            this.state.error_banner = Some(e);
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("Refresh"),
                        ),
                ),
        )
        .child(tab_bar)
        .child(body)
        .into_any_element()
}

fn render_runs_and_dag_section(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let selected_run = ws.state.selected_run_id.clone();

    let run_focused = ws.state.active_input == IdeInputTarget::NewRunTitle;
    let task_focused = ws.state.active_input == IdeInputTarget::NewTaskTitle;
    let deps_focused = ws.state.active_input == IdeInputTarget::NewTaskDeps;

    let mut runs_list = div().flex().flex_col().gap(px(4.0));
    for run in &ws.state.runs {
        let rid = run.run_id.clone();
        let active = selected_run.as_deref() == Some(run.run_id.as_str());
        runs_list = runs_list.child(
            div()
                .id(SharedString::from(format!("orca-run-{}", run.run_id)))
                .cursor_pointer()
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(6.0))
                .bg(if active {
                    pal.row_active_rgba()
                } else {
                    rgb(pal.input_bg)
                })
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
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        this.state.selected_run_id = Some(rid.clone());
                        let rt = this.runtime.clone();
                        let _ = this.state.refresh_kanban_and_orchestration(&rt);
                        cx.notify();
                    }),
                )
                .child(format!(
                    "{} [{}] · gen {} · wake={}",
                    run.title,
                    run.status.as_str(),
                    run.consumer_generation,
                    run.wake_state
                )),
        );
    }

    let mut ready_col = div().flex().flex_col().gap(px(4.0));
    for task in &ws.state.ready_tasks {
        let run_id_w = task.run_id.clone();
        let task_id_w = task.task_id.clone();
        let run_id_s = task.run_id.clone();
        let task_id_s = task.task_id.clone();
        ready_col = ready_col.child(
            div()
                .px(px(8.0))
                .py(px(6.0))
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
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "{} ({:?}, deps={})",
                            task.title,
                            task.status,
                            task.depends_on.len()
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(4.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-start-wk-{}", task.task_id)))
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
                                        let now_ms = chrono::Utc::now().timestamp_millis();
                                        let rt = this.runtime.clone();
                                        let agent = this.state.selected_worker_agent.clone();
                                        if let Err(e) = this.state.start_orchestration_worker(
                                            &rt, &run_id_w, &task_id_w, &agent, now_ms,
                                        ) {
                                            this.state.error_banner = Some(e);
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("Start Worker"),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-done-tsk-{}", task.task_id)))
                                .cursor_pointer()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(rgb(pal.success))
                                .text_xs()
                                .text_color(rgb(0xFFFFFF))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let now_ms = chrono::Utc::now().timestamp_millis();
                                        let rt = this.runtime.clone();
                                        if let Err(e) = this.state.transition_orchestration_task(
                                            &rt,
                                            &run_id_s,
                                            &task_id_s,
                                            OrchestrationTaskStatus::Succeeded,
                                            now_ms,
                                        ) {
                                            this.state.error_banner = Some(e);
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("Mark Done"),
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
                .flex()
                .flex_row()
                .gap(px(6.0))
                .child(
                    div()
                        .id("orca-new-run-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if run_focused {
                            rgb(pal.accent)
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::NewRunTitle, w, cx);
                            }),
                        )
                        .child(if ws.state.new_run_title.is_empty() {
                            if run_focused { "|".to_string() } else { "New run title…".to_string() }
                        } else if run_focused {
                            format!("{}|", ws.state.new_run_title)
                        } else {
                            ws.state.new_run_title.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-create-run-btn")
                        .cursor_pointer()
                        .px(px(10.0))
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
                                let title = this.state.new_run_title.clone();
                                if let Err(e) =
                                    this.state.create_orchestration_run(&rt, &title, now_ms)
                                {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("+ Run"),
                ),
        )
        .child(runs_list)
        .when_some(selected_run, |el, rid| {
            let rid_btn = rid.clone();
            el.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .id("orca-new-task-input")
                                    .cursor_text()
                                    .flex_1()
                                    .px(px(7.0))
                                    .py(px(5.0))
                                    .rounded(px(5.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(if task_focused {
                                        rgb(pal.accent)
                                    } else {
                                        pal.hairline_border_rgba()
                                    })
                                    .text_xs()
                                    .text_color(rgb(pal.text_primary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev, w, cx| {
                                            cx.stop_propagation();
                                            this.set_active_input(
                                                IdeInputTarget::NewTaskTitle,
                                                w,
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(if ws.state.new_task_title.is_empty() {
                                        "New DAG task title…".to_string()
                                    } else if task_focused {
                                        format!("{}|", ws.state.new_task_title)
                                    } else {
                                        ws.state.new_task_title.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .id("orca-new-task-deps-input")
                                    .cursor_text()
                                    .w(px(120.0))
                                    .px(px(7.0))
                                    .py(px(5.0))
                                    .rounded(px(5.0))
                                    .bg(rgb(pal.input_bg))
                                    .border_1()
                                    .border_color(if deps_focused {
                                        rgb(pal.accent)
                                    } else {
                                        pal.hairline_border_rgba()
                                    })
                                    .text_xs()
                                    .text_color(rgb(pal.text_secondary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _ev, w, cx| {
                                            cx.stop_propagation();
                                            this.set_active_input(
                                                IdeInputTarget::NewTaskDeps,
                                                w,
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(if ws.state.new_task_deps.is_empty() {
                                        "deps (csv)".to_string()
                                    } else if deps_focused {
                                        format!("{}|", ws.state.new_task_deps)
                                    } else {
                                        ws.state.new_task_deps.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .id("orca-create-task-btn")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(px(5.0))
                                    .bg(rgb(pal.accent))
                                    .text_xs()
                                    .text_color(rgb(pal.active_button_fg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _ev, _w, cx| {
                                            cx.stop_propagation();
                                            let now_ms = chrono::Utc::now().timestamp_millis();
                                            let rt = this.runtime.clone();
                                            let title = this.state.new_task_title.clone();
                                            let deps: Vec<String> = this
                                                .state
                                                .new_task_deps
                                                .split(',')
                                                .map(|s| s.trim().to_string())
                                                .filter(|s| !s.is_empty())
                                                .collect();
                                            if let Err(e) = this.state.create_orchestration_task(
                                                &rt, &rid_btn, &title, deps, now_ms,
                                            ) {
                                                this.state.error_banner = Some(e);
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child("+ Task"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_secondary))
                            .child(format!("Ready DAG Tasks ({})", ws.state.ready_tasks.len())),
                    )
                    .child(ready_col),
            )
        })
        .into_any_element()
}

fn render_workers_section(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let mut col = div().flex().flex_col().gap(px(8.0));
    if ws.state.tracked_workers.is_empty() {
        col = col.child(
            div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child("No active or tracked worker dispatches yet. Start a worker from Runs & DAG."),
        );
    }

    for worker in &ws.state.tracked_workers {
        let d = &worker.dispatch;
        let did = d.dispatch_id.clone();
        let dispatch_gen = d.generation;
        let is_unverifiable = matches!(d.liveness, WorkerLivenessState::Unverifiable);
        let (liveness_lbl, liveness_col) = match d.liveness {
            WorkerLivenessState::Alive => ("ALIVE", 0x16A34A),
            WorkerLivenessState::Exited { exit_code } => (
                if exit_code == Some(0) {
                    "EXITED (0)"
                } else {
                    "EXITED (ERR)"
                },
                0x64748B,
            ),
            WorkerLivenessState::Unverifiable => ("UNVERIFIABLE · NO AUTO-RETRY", 0xF59E0B),
        };
        let did_unv = did.clone();
        let did_done = did.clone();
        let did_arch = did.clone();
        let archives = ws.state.worker_archives.get(&did);

        col = col.child(
            div()
                .p(px(8.0))
                .rounded(px(8.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if is_unverifiable {
                    rgb(0xF59E0B)
                } else {
                    pal.hairline_border_rgba()
                })
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(format!(
                                    "{} · task={} · gen={} · {:?}",
                                    d.dispatch_id, d.task_id, d.generation, d.status
                                )),
                        )
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(rgb(liveness_col))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0xFFFFFF))
                                .child(liveness_lbl),
                        ),
                )
                .when(is_unverifiable, |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xF59E0B))
                            .child("Non-guessing liveness invariant: Unverifiable NEVER proves exit and NEVER authorizes autonomous retry. Inspect host/terminal manually."),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap(px(5.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-wk-unv-{did}")))
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .bg(rgb(pal.card_bg))
                                .text_xs()
                                .text_color(rgb(0xF59E0B))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let now_ms = chrono::Utc::now().timestamp_millis();
                                        let rt = this.runtime.clone();
                                        if let Err(e) = this.state.report_worker_liveness(
                                            &rt,
                                            &did_unv,
                                            dispatch_gen,
                                            WorkerLivenessState::Unverifiable,
                                            now_ms,
                                        ) {
                                            this.state.error_banner = Some(e);
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("Mark Unverifiable"),
                        )
                        .when(!is_unverifiable, |el| {
                            el.child(
                                div()
                                    .id(SharedString::from(format!("orca-wk-done-{did}")))
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
                                            let now_ms = chrono::Utc::now().timestamp_millis();
                                            let rt = this.runtime.clone();
                                            if let Err(e) = this.state.complete_orchestration_worker(
                                                &rt,
                                                &did_done,
                                                dispatch_gen,
                                                true,
                                                Some(format!("Worker {did_done} completed cleanly")),
                                                now_ms,
                                            ) {
                                                this.state.error_banner = Some(e);
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child("Complete Worker"),
                            )
                        })
                        .child(
                            div()
                                .id(SharedString::from(format!("orca-wk-arch-{did}")))
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .bg(rgb(pal.card_bg))
                                .text_xs()
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let rt = this.runtime.clone();
                                        if let Err(e) = this.state.load_worker_archive(&rt, &did_arch) {
                                            this.state.error_banner = Some(e);
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("Read Archive"),
                        ),
                )
                .when_some(archives, |el, list| {
                    let preview = list
                        .first()
                        .map(|a| a.transcript_text.clone())
                        .unwrap_or_else(|| "No archived transcripts".to_string());
                    el.child(
                        div()
                            .px(px(6.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(pal.code_bg))
                            .text_xs()
                            .text_color(rgb(pal.code_fg))
                            .child(format!("Archive ({}): {preview}", list.len())),
                    )
                }),
        );
    }
    col.into_any_element()
}

fn render_hosts_section(ws: &OrcaIdeWorkspace) -> AnyElement {
    let pal = ws.palette;
    let mut col = div().flex().flex_col().gap(px(6.0));
    if ws.state.hosts.is_empty() {
        col = col.child(
            div()
                .text_xs()
                .text_color(rgb(pal.text_secondary))
                .child("No federated remote execution hosts registered yet (local host active)."),
        );
    }
    for host in &ws.state.hosts {
        let caps: Vec<&str> = host.capabilities.iter().map(String::as_str).collect();
        col = col.child(
            div()
                .p(px(8.0))
                .rounded(px(6.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if host.online {
                    rgb(pal.success)
                } else {
                    pal.hairline_border_rgba()
                })
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(format!(
                            "{} ({}) · {} · proto v{} · workers {}/{}",
                            host.display_name,
                            host.host_id,
                            if host.online { "ONLINE" } else { "OFFLINE" },
                            host.protocol_version,
                            host.active_workers,
                            host.max_workers
                        )),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!("capabilities: {}", caps.join(", "))),
                ),
        );
    }
    col.into_any_element()
}

fn render_mailbox_section(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let rec_focused = ws.state.active_input == IdeInputTarget::MailboxRecipient;
    let body_focused = ws.state.active_input == IdeInputTarget::MailboxBody;

    let mut types_row = div().flex().flex_row().flex_wrap().gap(px(4.0));
    for mtype in MailboxMessageType::ALL {
        let active = ws.state.mailbox_type_selected == mtype;
        types_row = types_row.child(
            div()
                .id(SharedString::from(format!("orca-mb-type-{}", mtype.as_str())))
                .cursor_pointer()
                .px(px(6.0))
                .py(px(2.0))
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
                    rgb(pal.text_secondary)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        this.state.mailbox_type_selected = mtype;
                        cx.notify();
                    }),
                )
                .child(mtype.as_str()),
        );
    }

    let mut group_pills = div().flex().flex_row().flex_wrap().gap(px(4.0));
    for preset in ["@all", "@idle", "@claude", "@worktree:main"] {
        let active = ws.state.mailbox_target_input == preset;
        group_pills = group_pills.child(
            div()
                .id(SharedString::from(format!("orca-mb-preset-{preset}")))
                .cursor_pointer()
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(4.0))
                .bg(if active {
                    rgb(pal.accent)
                } else {
                    rgb(pal.card_bg)
                })
                .text_xs()
                .text_color(if active {
                    rgb(pal.active_button_fg)
                } else {
                    rgb(pal.text_primary)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        this.state.mailbox_target_input = preset.to_string();
                        cx.notify();
                    }),
                )
                .child(preset),
        );
    }

    let mut prio_row = div().flex().flex_row().gap(px(4.0));
    for (prio, lbl) in [
        (MailboxPriority::Normal, "Normal"),
        (MailboxPriority::High, "High"),
        (MailboxPriority::Urgent, "Urgent"),
    ] {
        let active = ws.state.mailbox_priority_selected == prio;
        prio_row = prio_row.child(
            div()
                .id(SharedString::from(format!("orca-mb-prio-{lbl}")))
                .cursor_pointer()
                .px(px(6.0))
                .py(px(2.0))
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
                    rgb(pal.text_secondary)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        this.state.mailbox_priority_selected = prio;
                        cx.notify();
                    }),
                )
                .child(lbl),
        );
    }

    let mut deliveries_col = div().flex().flex_col().gap(px(5.0));
    for del in &ws.state.mailbox_deliveries {
        let did = del.delivery_id.clone();
        let delivery_gen = del.generation;
        deliveries_col = deliveries_col.child(
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
                                    "[{}] from {} ({:?})",
                                    del.message.message_type.as_str(),
                                    del.message.from_id,
                                    del.state
                                )),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .child(del.message.body.clone()),
                        ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("orca-mb-ack-{did}")))
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
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                if let Err(e) =
                                    this.state.ack_mailbox_delivery(&rt, &did, delivery_gen, now_ms)
                                {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("Ack"),
                ),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(types_row)
        .child(group_pills)
        .child(prio_row)
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .child(
                    div()
                        .id("orca-mb-target-input")
                        .cursor_text()
                        .w(px(140.0))
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if rec_focused {
                            rgb(pal.accent)
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::MailboxRecipient, w, cx);
                            }),
                        )
                        .child(if rec_focused {
                            format!("{}|", ws.state.mailbox_target_input)
                        } else {
                            ws.state.mailbox_target_input.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-mb-body-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if body_focused {
                            rgb(pal.accent)
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::MailboxBody, w, cx);
                            }),
                        )
                        .child(if ws.state.mailbox_body_draft.is_empty() {
                            "Mailbox message body…".to_string()
                        } else if body_focused {
                            format!("{}|", ws.state.mailbox_body_draft)
                        } else {
                            ws.state.mailbox_body_draft.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-mb-send-btn")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let Some(rid) = this.state.selected_run_id.clone() else {
                                    this.state.error_banner =
                                        Some("Create or select an orchestration run first".to_string());
                                    cx.notify();
                                    return;
                                };
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                let target = this.state.mailbox_target_input.clone();
                                let mtype = this.state.mailbox_type_selected;
                                let prio = this.state.mailbox_priority_selected;
                                let body = this.state.mailbox_body_draft.clone();
                                if let Err(e) = this.state.send_orchestration_mailbox(
                                    &rt, &rid, &target, mtype, prio, &body, now_ms,
                                ) {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("Send"),
                ),
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_secondary))
                .child(format!("Deliveries ({})", ws.state.mailbox_deliveries.len())),
        )
        .child(deliveries_col)
        .into_any_element()
}

fn render_gates_section(
    ws: &OrcaIdeWorkspace,
    cx: &mut Context<OrcaIdeWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let prompt_focused = ws.state.active_input == IdeInputTarget::GatePrompt;

    let mut kind_row = div().flex().flex_row().gap(px(5.0));
    for (kind, lbl) in [
        (DecisionGateKind::Review, "Review Gate"),
        (DecisionGateKind::Merge, "Merge Gate"),
        (DecisionGateKind::Decision, "Decision Gate"),
    ] {
        let active = ws.state.gate_kind_selected == kind;
        kind_row = kind_row.child(
            div()
                .id(SharedString::from(format!("orca-gate-kind-{}", kind.as_str())))
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
                    rgb(pal.text_secondary)
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        this.state.gate_kind_selected = kind;
                        cx.notify();
                    }),
                )
                .child(lbl),
        );
    }

    let mut gates_col = div().flex().flex_col().gap(px(6.0));
    for gate in &ws.state.tracked_gates {
        let gid_ok = gate.gate_id.clone();
        let gid_no = gate.gate_id.clone();
        let pending = gate.status == GateStatus::Pending;
        let status_col = match gate.status {
            GateStatus::Pending => 0xF59E0B,
            GateStatus::Approved => 0x16A34A,
            GateStatus::Rejected => 0xEF4444,
            GateStatus::TimedOut => 0x64748B,
        };
        gates_col = gates_col.child(
            div()
                .p(px(8.0))
                .rounded(px(6.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(rgb(status_col))
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
                            "{} [{}] · task={} · {:?}",
                            gate.gate_id,
                            gate.gate_kind.as_str(),
                            gate.task_id,
                            gate.status
                        )),
                )
                .when(pending, |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .id(SharedString::from(format!("orca-gate-approve-{gid_ok}")))
                                    .cursor_pointer()
                                    .px(px(7.0))
                                    .py(px(3.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(0x16A34A))
                                    .text_xs()
                                    .text_color(rgb(0xFFFFFF))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _ev, _w, cx| {
                                            cx.stop_propagation();
                                            let now_ms = chrono::Utc::now().timestamp_millis();
                                            let rt = this.runtime.clone();
                                            if let Err(e) = this.state.resolve_orchestration_gate(
                                                &rt, &gid_ok, true, now_ms,
                                            ) {
                                                this.state.error_banner = Some(e);
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child("Approve"),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("orca-gate-reject-{gid_no}")))
                                    .cursor_pointer()
                                    .px(px(7.0))
                                    .py(px(3.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(0xDC2626))
                                    .text_xs()
                                    .text_color(rgb(0xFFFFFF))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _ev, _w, cx| {
                                            cx.stop_propagation();
                                            let now_ms = chrono::Utc::now().timestamp_millis();
                                            let rt = this.runtime.clone();
                                            if let Err(e) = this.state.resolve_orchestration_gate(
                                                &rt, &gid_no, false, now_ms,
                                            ) {
                                                this.state.error_banner = Some(e);
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child("Reject"),
                            ),
                    )
                }),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(kind_row)
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(6.0))
                .child(
                    div()
                        .id("orca-gate-prompt-input")
                        .cursor_text()
                        .flex_1()
                        .px(px(7.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.input_bg))
                        .border_1()
                        .border_color(if prompt_focused {
                            rgb(pal.accent)
                        } else {
                            pal.hairline_border_rgba()
                        })
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, w, cx| {
                                cx.stop_propagation();
                                this.set_active_input(IdeInputTarget::GatePrompt, w, cx);
                            }),
                        )
                        .child(if ws.state.gate_prompt_draft.is_empty() {
                            "Gate prompt / note…".to_string()
                        } else if prompt_focused {
                            format!("{}|", ws.state.gate_prompt_draft)
                        } else {
                            ws.state.gate_prompt_draft.clone()
                        }),
                )
                .child(
                    div()
                        .id("orca-gate-create-btn")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(5.0))
                        .bg(rgb(pal.accent))
                        .text_xs()
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _ev, _w, cx| {
                                cx.stop_propagation();
                                let Some(rid) = this.state.selected_run_id.clone() else {
                                    this.state.error_banner = Some("Select a run first".to_string());
                                    cx.notify();
                                    return;
                                };
                                let Some(tid) = this
                                    .state
                                    .ready_tasks
                                    .first()
                                    .or_else(|| this.state.tracked_tasks.first())
                                    .map(|t| t.task_id.clone())
                                else {
                                    this.state.error_banner = Some("Create a task first".to_string());
                                    cx.notify();
                                    return;
                                };
                                let now_ms = chrono::Utc::now().timestamp_millis();
                                let rt = this.runtime.clone();
                                let kind = this.state.gate_kind_selected;
                                let prompt = this.state.gate_prompt_draft.clone();
                                if let Err(e) = this
                                    .state
                                    .create_orchestration_gate(&rt, &rid, &tid, kind, &prompt, now_ms)
                                {
                                    this.state.error_banner = Some(e);
                                }
                                cx.notify();
                            }),
                        )
                        .child("+ Gate"),
                ),
        )
        .child(gates_col)
        .into_any_element()
}
