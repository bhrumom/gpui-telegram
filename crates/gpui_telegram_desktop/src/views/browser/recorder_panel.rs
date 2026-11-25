//! Session/Credential Reuse, Live AI Copilot Step Log, Trace Distillation,
//! and Zero-LLM Script App Replay Panel (`F8.1`).

use std::collections::BTreeMap;

use bot_browser::{
    AutomationScriptApp, BrowserControlMode, ChromeImportProbe, ScriptReplayReport,
    SemanticLocator,
};
use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::browser::workspace::{ActiveBrowserInput, BrowserWorkspace};
use crate::views::tdesktop_theme::tr_str;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserLogEntry {
    pub seq: usize,
    pub level: BrowserLogLevel,
    pub title: String,
    pub detail: String,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserLogLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChromeProbeUiState {
    NotProbed,
    Probing,
    Available {
        path: String,
        profile_name: String,
        size_bytes: u64,
        encrypted_v10: bool,
    },
    PermissionDeniedTcc {
        path: String,
        guidance: String,
    },
    NotFound,
}

impl From<ChromeImportProbe> for ChromeProbeUiState {
    fn from(probe: ChromeImportProbe) -> Self {
        match probe {
            ChromeImportProbe::Available {
                path,
                profile_name,
                size_bytes,
                encrypted_v10,
            } => Self::Available {
                path: path.display().to_string(),
                profile_name,
                size_bytes,
                encrypted_v10,
            },
            ChromeImportProbe::PermissionDeniedTcc { path, guidance } => {
                Self::PermissionDeniedTcc {
                    path: path.display().to_string(),
                    guidance,
                }
            }
            ChromeImportProbe::NotFound => Self::NotFound,
        }
    }
}

pub struct RecorderPanelState {
    pub profile_draft: String,
    pub available_profiles: Vec<String>,
    pub cookie_import_draft: String,
    pub cookie_status_message: Option<String>,
    pub cookie_error_message: Option<String>,
    pub last_imported_cookie_count: usize,
    pub last_persisted_cookie_count: usize,
    pub chrome_probe: ChromeProbeUiState,
    pub vault_sync_status: Option<String>,
    pub copilot_prompt_draft: String,
    pub copilot_running: bool,
    pub copilot_last_reply: Option<String>,
    pub is_dom_recording: bool,
    pub recorded_dom_count: usize,
    pub event_logs: Vec<BrowserLogEntry>,
    pub next_log_seq: usize,
    pub distill_name_draft: String,
    pub distill_version_draft: String,
    pub distill_category_draft: String,
    pub distill_description_draft: String,
    pub distill_error: Option<String>,
    pub local_scripts: Vec<AutomationScriptApp>,
    pub selected_script_id: Option<String>,
    pub param_inputs: BTreeMap<String, String>,
    pub is_replaying: bool,
    pub last_replay_report: Option<ScriptReplayReport>,
    pub replay_error: Option<String>,
}

impl Default for RecorderPanelState {
    fn default() -> Self {
        Self {
            profile_draft: "default".to_string(),
            available_profiles: vec![
                "default".to_string(),
                "x-social".to_string(),
                "work".to_string(),
            ],
            cookie_import_draft: String::new(),
            cookie_status_message: None,
            cookie_error_message: None,
            last_imported_cookie_count: 0,
            last_persisted_cookie_count: 0,
            chrome_probe: ChromeProbeUiState::NotProbed,
            vault_sync_status: None,
            copilot_prompt_draft: String::new(),
            copilot_running: false,
            copilot_last_reply: None,
            is_dom_recording: true,
            recorded_dom_count: 0,
            event_logs: Vec::new(),
            next_log_seq: 1,
            distill_name_draft: "Recorded Browser Flow".to_string(),
            distill_version_draft: "1.0.0".to_string(),
            distill_category_draft: "general".to_string(),
            distill_description_draft: "Reusable browser automation distilled from session trace"
                .to_string(),
            distill_error: None,
            local_scripts: Vec::new(),
            selected_script_id: None,
            param_inputs: BTreeMap::new(),
            is_replaying: false,
            last_replay_report: None,
            replay_error: None,
        }
    }
}

impl RecorderPanelState {
    pub fn push_log(
        &mut self,
        level: BrowserLogLevel,
        title: impl Into<String>,
        detail: impl Into<String>,
        duration_ms: Option<u64>,
    ) {
        let seq = self.next_log_seq.max(1);
        self.next_log_seq = seq.saturating_add(1);
        self.event_logs.push(BrowserLogEntry {
            seq,
            level,
            title: title.into(),
            detail: detail.into(),
            duration_ms,
        });
        if self.event_logs.len() > 250 {
            let drain = self.event_logs.len() - 250;
            self.event_logs.drain(0..drain);
        }
    }

    pub fn upsert_script(&mut self, app: AutomationScriptApp) {
        let app_id = app.app_id.clone();
        for p in &app.parameters {
            if !p.is_secret {
                if let Some(ref def) = p.default_value {
                    self.param_inputs
                        .entry(p.name.clone())
                        .or_insert_with(|| def.clone());
                } else {
                    self.param_inputs.entry(p.name.clone()).or_default();
                }
            }
        }
        if let Some(pos) = self.local_scripts.iter().position(|s| s.app_id == app_id) {
            self.local_scripts[pos] = app;
        } else {
            self.local_scripts.insert(0, app);
        }
        self.selected_script_id = Some(app_id);
        self.distill_error = None;
    }

    pub fn selected_script(&self) -> Option<&AutomationScriptApp> {
        if let Some(ref id) = self.selected_script_id {
            if let Some(found) = self.local_scripts.iter().find(|s| &s.app_id == id) {
                return Some(found);
            }
        }
        self.local_scripts.first()
    }
}

pub(crate) fn format_locator_summary(loc: &SemanticLocator) -> String {
    if let Some(ref id) = loc.test_id {
        return format!("[data-testid={id}]");
    }
    if let Some(ref aria) = loc.aria_label {
        return format!("[aria-label={aria}]");
    }
    if let Some(ref role) = loc.role {
        if let Some(ref txt) = loc.text_anchor {
            return format!("{role}({txt})");
        }
        return format!("[role={role}]");
    }
    if let Some(ref css) = loc.css_selector {
        return css.clone();
    }
    if let Some(ref name) = loc.name_attr {
        return format!("[name={name}]");
    }
    loc.text_anchor.clone().unwrap_or_else(|| "element".to_string())
}

pub(crate) fn panel_section_card(
    title: impl Into<String>,
    icon: IconName,
    pal: TdesktopThemePalette,
) -> Div {
    div()
        .p(px(12.0))
        .rounded(px(10.0))
        .bg(rgb(pal.card_bg))
        .border_1()
        .border_color(rgb(pal.border_fg))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(tg_icon(icon, 14.0, pal.accent))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(title.into()),
                ),
        )
}

pub(crate) fn browser_input_box(
    id: impl Into<ElementId>,
    value: &str,
    placeholder: &str,
    target: ActiveBrowserInput,
    active: &ActiveBrowserInput,
    pal: TdesktopThemePalette,
    multiline: bool,
    cx: &mut Context<BrowserWorkspace>,
) -> Stateful<Div> {
    let is_active = target == *active;
    let target_clone = target.clone();
    div()
        .id(id)
        .w_full()
        .px(px(9.0))
        .py(px(6.0))
        .rounded(px(7.0))
        .bg(rgb(pal.input_bg))
        .border_1()
        .border_color(rgb(if is_active { pal.accent } else { pal.border_fg }))
        .cursor_text()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                this.set_active_input(target_clone.clone(), Some(window), cx);
                cx.stop_propagation();
            }),
        )
        .when(multiline, |el| el.min_h(px(68.0)).items_start())
        .text_xs()
        .text_color(rgb(if value.is_empty() {
            pal.text_secondary
        } else {
            pal.text_primary
        }))
        .child(if value.is_empty() {
            placeholder.to_string()
        } else if is_active {
            format!("{value}|")
        } else {
            value.to_string()
        })
}

pub(crate) fn panel_action_btn(
    id: impl Into<ElementId>,
    label: impl Into<String>,
    bg_color: u32,
    fg_color: u32,
    on_click: impl Fn(&mut BrowserWorkspace, &mut Context<BrowserWorkspace>) + 'static,
    cx: &mut Context<BrowserWorkspace>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(9.0))
        .py(px(5.0))
        .rounded(px(6.0))
        .bg(rgb(bg_color))
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(fg_color))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                on_click(this, cx);
                cx.stop_propagation();
            }),
        )
        .child(label.into())
}

pub fn render_copilot_tab(
    ws: &BrowserWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<BrowserWorkspace>,
) -> AnyElement {
    let rec = &ws.recorder;
    let mode_label = match &ws.control_mode {
        BrowserControlMode::Idle => tr_str(lang, "空闲", "Idle").to_string(),
        BrowserControlMode::AiControlling { active_step, status_text } => {
            format!("{} #{} · {}", tr_str(lang, "AI 执行中", "AI Running"), active_step + 1, status_text)
        }
        BrowserControlMode::WaitingForTakeover { reason } => {
            format!("{}: {}", tr_str(lang, "等待接管", "Needs Takeover"), reason)
        }
        BrowserControlMode::UserTakeover { paused_at_step } => {
            format!("{} (#{})", tr_str(lang, "人工接管中", "User Takeover"), paused_at_step + 1)
        }
    };
    let mode_color = match &ws.control_mode {
        BrowserControlMode::Idle => pal.text_secondary,
        BrowserControlMode::AiControlling { .. } => pal.accent,
        BrowserControlMode::WaitingForTakeover { .. } => 0xE67E22,
        BrowserControlMode::UserTakeover { .. } => pal.success,
    };

    div()
        .id("browser-copilot-tab-scroll")
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            panel_section_card(
                tr_str(lang, "AI 浏览器副驾驶", "AI Browser Copilot"),
                IconName::BotRobot,
                pal,
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .child(
                        div()
                            .px(px(7.0))
                            .py(px(2.5))
                            .rounded(px(5.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(mode_color, 0.16))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(mode_color))
                            .child(mode_label),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(format!(
                                "{}: {}",
                                tr_str(lang, "记录步数", "Recorded"),
                                rec.recorded_dom_count
                            )),
                    ),
            )
            .child(browser_input_box(
                "browser-copilot-prompt-input",
                &rec.copilot_prompt_draft,
                tr_str(
                    lang,
                    "输入自动化指令（如：登录 X 并发布动态 / 检索网页信息）…",
                    "Instruct AI to automate this browser tab…",
                ),
                ActiveBrowserInput::CopilotPrompt,
                &ws.active_input,
                pal,
                true,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(6.0))
                    .child(panel_action_btn(
                        "browser-copilot-run-btn",
                        if rec.copilot_running {
                            tr_str(lang, "执行中…", "Running…")
                        } else {
                            tr_str(lang, "AI 执行", "Run AI")
                        },
                        pal.accent,
                        pal.active_button_fg,
                        |this, cx| this.trigger_copilot_prompt(cx),
                        cx,
                    ))
                    .child(panel_action_btn(
                        "browser-dom-rec-toggle-btn",
                        if rec.is_dom_recording {
                            tr_str(lang, "录制: 开", "Record DOM: ON")
                        } else {
                            tr_str(lang, "录制: 关", "Record DOM: OFF")
                        },
                        if rec.is_dom_recording {
                            pal.success
                        } else {
                            pal.input_bg
                        },
                        if rec.is_dom_recording {
                            0xFFFFFF
                        } else {
                            pal.text_primary
                        },
                        |this, cx| this.toggle_dom_recording(cx),
                        cx,
                    ))
                    .child(panel_action_btn(
                        "browser-quick-distill-btn",
                        tr_str(lang, "保存为脚本应用", "Save as Script App"),
                        pal.input_bg,
                        pal.text_primary,
                        |this, cx| {
                            this.set_inspector_tab(
                                crate::views::browser::workspace::BrowserInspectorTab::Recorder,
                                cx,
                            );
                            let _ = this.distill_from_form(cx);
                        },
                        cx,
                    )),
            )
            .when_some(rec.copilot_last_reply.clone(), |card, reply| {
                card.child(
                    div()
                        .p(px(8.0))
                        .rounded(px(6.0))
                        .bg(rgb(pal.input_bg))
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .child(reply),
                )
            }),
        )
        .child(
            panel_section_card(
                tr_str(lang, "实时操作与遥测日志", "Live Execution Log"),
                IconName::Poll,
                pal,
            )
            .when(rec.event_logs.is_empty(), |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(
                            lang,
                            "尚无浏览器自动化事件。导航网页、启动 AI 或重放脚本后将实时流式展示。",
                            "No browser automation events yet. Navigate, run AI, or replay a Script App to stream live steps.",
                        )),
                )
            })
            .children(rec.event_logs.iter().rev().take(40).map(|entry| {
                let badge_col = match entry.level {
                    BrowserLogLevel::Info => pal.accent,
                    BrowserLogLevel::Success => pal.success,
                    BrowserLogLevel::Warning => 0xE67E22,
                    BrowserLogLevel::Error => pal.danger,
                };
                div()
                    .p(px(7.0))
                    .rounded(px(6.0))
                    .bg(rgb(pal.input_bg))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(badge_col))
                                    .child(format!("#{} {}", entry.seq, entry.title)),
                            )
                            .when_some(entry.duration_ms, |row, ms| {
                                row.child(
                                    div()
                                        .px(px(5.0))
                                        .py(px(1.0))
                                        .rounded(px(4.0))
                                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                        .text_xs()
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("{ms}ms")),
                                )
                            }),
                    )
                    .when(!entry.detail.is_empty(), |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.text_secondary))
                                .child(entry.detail.clone()),
                        )
                    })
            })),
        )
        .into_any_element()
}

pub fn render_recorder_tab(
    ws: &BrowserWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<BrowserWorkspace>,
) -> AnyElement {
    let rec = &ws.recorder;
    let selected = rec.selected_script().cloned();

    div()
        .id("browser-recorder-tab-scroll")
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            panel_section_card(
                tr_str(lang, "沉淀为可复用脚本应用", "Save as Script App"),
                IconName::FolderBots,
                pal,
            )
            .child(browser_input_box(
                "browser-distill-name-input",
                &rec.distill_name_draft,
                tr_str(lang, "脚本应用名称", "Script App Name"),
                ActiveBrowserInput::DistillName,
                &ws.active_input,
                pal,
                false,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(browser_input_box(
                        "browser-distill-version-input",
                        &rec.distill_version_draft,
                        "1.0.0",
                        ActiveBrowserInput::DistillVersion,
                        &ws.active_input,
                        pal,
                        false,
                        cx,
                    ))
                    .child(browser_input_box(
                        "browser-distill-category-input",
                        &rec.distill_category_draft,
                        "social / general",
                        ActiveBrowserInput::DistillCategory,
                        &ws.active_input,
                        pal,
                        false,
                        cx,
                    )),
            )
            .child(browser_input_box(
                "browser-distill-desc-input",
                &rec.distill_description_draft,
                tr_str(lang, "应用说明", "Description"),
                ActiveBrowserInput::DistillDescription,
                &ws.active_input,
                pal,
                false,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(panel_action_btn(
                        "browser-distill-save-btn",
                        tr_str(lang, "保存为脚本应用", "Save as Script App"),
                        pal.accent,
                        pal.active_button_fg,
                        |this, cx| {
                            let _ = this.distill_from_form(cx);
                            cx.notify();
                        },
                        cx,
                    )),
            )
            .when_some(rec.distill_error.clone(), |card, err| {
                card.child(
                    div()
                        .p(px(7.0))
                        .rounded(px(6.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                        .text_xs()
                        .text_color(rgb(pal.danger))
                        .child(err),
                )
            }),
        )
        .child(
            panel_section_card(
                tr_str(lang, "本地脚本应用库与零 LLM 重放", "Local Script Apps (Zero-LLM Replay)"),
                IconName::Play,
                pal,
            )
            .when(rec.local_scripts.is_empty(), |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(
                            lang,
                            "完成操作后点击“保存为脚本应用”，即可无需 LLM 直接高速重放。",
                            "Distill a trace above to replay steps deterministically without LLM calls.",
                        )),
                )
            })
            .children(rec.local_scripts.iter().map(|script| {
                let is_sel = selected.as_ref().is_some_and(|s| s.app_id == script.app_id);
                let sid_select = script.app_id.clone();
                let script_run = script.clone();
                let script_pub = script.clone();
                div()
                    .p(px(8.0))
                    .rounded(px(8.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(rgb(if is_sel { pal.accent } else { pal.border_fg }))
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.recorder.selected_script_id = Some(sid_select.clone());
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(format!("{} v{}", script.name, script.version)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(pal.text_secondary))
                                    .child(format!(
                                        "{} · {} steps",
                                        script.category,
                                        script.steps.len()
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(panel_action_btn(
                                SharedString::from(format!("replay-script-{}", script.app_id)),
                                if rec.is_replaying {
                                    tr_str(lang, "重放中…", "Replaying…")
                                } else {
                                    tr_str(lang, "运行 (无 LLM)", "Run (No LLM)")
                                },
                                pal.success,
                                0xFFFFFF,
                                move |this, cx| {
                                    let params = this.collect_replay_params_for(&script_run);
                                    this.replay_script_app(script_run.clone(), params, cx);
                                },
                                cx,
                            ))
                            .child(panel_action_btn(
                                SharedString::from(format!("stage-publish-{}", script.app_id)),
                                tr_str(lang, "上架到商店", "Publish to Store"),
                                pal.card_bg,
                                pal.accent,
                                move |this, cx| {
                                    this.store.selected_draft_app = Some(script_pub.clone());
                                    this.set_inspector_tab(
                                        crate::views::browser::workspace::BrowserInspectorTab::Store,
                                        cx,
                                    );
                                },
                                cx,
                            )),
                    )
            }))
            .when_some(selected.clone(), |card, app| {
                card.when(!app.parameters.is_empty(), |c| {
                    c.child(
                        div()
                            .pt(px(4.0))
                            .flex()
                            .flex_col()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.text_secondary))
                                    .child(tr_str(lang, "脚本参数与凭证槽位", "Script Parameters")),
                            )
                            .children(app.parameters.iter().map(|p| {
                                if p.is_secret {
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .px(px(8.0))
                                        .py(px(5.0))
                                        .rounded(px(6.0))
                                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.12))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(5.0))
                                                .child(tg_icon(IconName::Lock, 12.0, pal.success))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(pal.text_primary))
                                                        .child(p.name.clone()),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(pal.success))
                                                .child(tr_str(lang, "Vault 密钥", "Vault Secret")),
                                        )
                                        .into_any_element()
                                } else {
                                    let val = rec
                                        .param_inputs
                                        .get(&p.name)
                                        .cloned()
                                        .or_else(|| p.default_value.clone())
                                        .unwrap_or_default();
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(3.0))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(pal.text_secondary))
                                                .child(p.name.clone()),
                                        )
                                        .child(browser_input_box(
                                            SharedString::from(format!("param-input-{}", p.name)),
                                            &val,
                                            &p.description,
                                            ActiveBrowserInput::ScriptParam(p.name.clone()),
                                            &ws.active_input,
                                            pal,
                                            false,
                                            cx,
                                        ))
                                        .into_any_element()
                                }
                            })),
                    )
                })
            }),
        )
        .when_some(rec.replay_error.clone(), |col, err| {
            col.child(
                div()
                    .p(px(8.0))
                    .rounded(px(8.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                    .text_xs()
                    .text_color(rgb(pal.danger))
                    .child(err),
            )
        })
        .when_some(rec.last_replay_report.clone(), |col, report| {
            col.child(
                panel_section_card(
                    tr_str(lang, "零 LLM 重放延迟报告", "Zero-LLM Replay Telemetry"),
                    IconName::ClockSending,
                    pal,
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_xs()
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(if report.success {
                                    pal.success
                                } else {
                                    pal.danger
                                }))
                                .child(format!(
                                    "{} · used_llm={} · {} steps",
                                    if report.success { "PASS" } else { "FAIL" },
                                    report.used_llm,
                                    report.steps_executed
                                )),
                        )
                        .child(
                            div()
                                .text_color(rgb(pal.accent))
                                .child(format!("{} ms", report.total_duration_ms)),
                        ),
                )
                .children(report.step_timings.iter().map(|st| {
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "#{} {}",
                            st.step_index.map(|i| i + 1).unwrap_or(1),
                            st.operation
                        ))
                        .child(format!("{} ms", st.duration_ms))
                }))
                .when(!report.extracted_outputs.is_empty(), |c| {
                    c.children(report.extracted_outputs.iter().map(|(k, v)| {
                        div()
                            .p(px(5.0))
                            .rounded(px(5.0))
                            .bg(rgb(pal.input_bg))
                            .text_xs()
                            .text_color(rgb(pal.text_primary))
                            .child(format!("{k}: {v}"))
                    }))
                }),
            )
        })
        .into_any_element()
}

pub fn render_session_tab(
    ws: &BrowserWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<BrowserWorkspace>,
) -> AnyElement {
    let rec = &ws.recorder;
    div()
        .id("browser-session-tab-scroll")
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            panel_section_card(
                tr_str(lang, "浏览器会话与 Profile 持久化", "Session Profile & Vault"),
                IconName::Lock,
                pal,
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(5.0))
                    .children(rec.available_profiles.iter().map(|prof| {
                        let is_cur = ws.active_profile_id == *prof;
                        let prof_name = prof.clone();
                        div()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .bg(rgb(if is_cur { pal.accent } else { pal.input_bg }))
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(if is_cur {
                                pal.active_button_fg
                            } else {
                                pal.text_primary
                            }))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.switch_profile(prof_name.clone(), cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(prof.clone())
                    })),
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(browser_input_box(
                        "browser-profile-name-input",
                        &rec.profile_draft,
                        tr_str(lang, "自定义 Profile", "Custom profile"),
                        ActiveBrowserInput::ProfileName,
                        &ws.active_input,
                        pal,
                        false,
                        cx,
                    ))
                    .child(panel_action_btn(
                        "browser-profile-switch-btn",
                        tr_str(lang, "切换", "Switch"),
                        pal.input_bg,
                        pal.text_primary,
                        |this, cx| {
                            let name = this.recorder.profile_draft.trim().to_string();
                            if !name.is_empty() {
                                this.switch_profile(name, cx);
                            }
                        },
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .child(panel_action_btn(
                        "browser-save-cookies-btn",
                        tr_str(lang, "保存会话 Cookies", "Save Cookies"),
                        pal.accent,
                        pal.active_button_fg,
                        |this, cx| this.save_active_profile_cookies(cx),
                        cx,
                    ))
                    .child(panel_action_btn(
                        "browser-restore-cookies-btn",
                        tr_str(lang, "恢复会话 Cookies", "Restore Cookies"),
                        pal.input_bg,
                        pal.text_primary,
                        |this, cx| this.restore_active_profile_cookies(cx),
                        cx,
                    ))
                    .child(panel_action_btn(
                        "browser-vault-backup-btn",
                        tr_str(lang, "同步至 Vault", "Sync to Vault"),
                        pal.input_bg,
                        pal.text_primary,
                        |this, cx| this.backup_profile_to_vault(cx),
                        cx,
                    )),
            )
            .when_some(rec.vault_sync_status.clone(), |c, note| {
                c.child(div().text_xs().text_color(rgb(pal.success)).child(note))
            }),
        )
        .child(
            panel_section_card(
                tr_str(lang, "本地 Chrome 凭证探测 (TCC 安全)", "Local Chrome Cookie Probe"),
                IconName::Search,
                pal,
            )
            .child(panel_action_btn(
                "browser-probe-chrome-btn",
                tr_str(lang, "探测本地 Chrome Cookies", "Probe Local Chrome"),
                pal.input_bg,
                pal.text_primary,
                |this, cx| this.probe_local_chrome(cx),
                cx,
            ))
            .child(match &rec.chrome_probe {
                ChromeProbeUiState::NotProbed => div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(
                        lang,
                        "可按需检测本机 Chrome Cookie 数据库以复用已有登录态（如 X 账号）。",
                        "Optionally probe local Chrome cookies to reuse existing sessions (e.g. X).",
                    ))
                    .into_any_element(),
                ChromeProbeUiState::Probing => div()
                    .text_xs()
                    .text_color(rgb(pal.accent))
                    .child(tr_str(lang, "正在探测本地浏览器凭证…", "Probing local Chrome…"))
                    .into_any_element(),
                ChromeProbeUiState::Available {
                    path,
                    profile_name,
                    size_bytes,
                    ..
                } => {
                    let path_clone = path.clone();
                    div()
                        .p(px(8.0))
                        .rounded(px(7.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.12))
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.success))
                                .child(format!(
                                    "Chrome ({profile_name}) · {size_bytes} bytes · {path}"
                                )),
                        )
                        .child(panel_action_btn(
                            "browser-import-chrome-sqlite-btn",
                            tr_str(lang, "一键导入 Chrome Cookies", "Import from Chrome"),
                            pal.success,
                            0xFFFFFF,
                            move |this, cx| {
                                this.import_cookies_from_chrome_path(path_clone.clone(), cx);
                            },
                            cx,
                        ))
                        .into_any_element()
                }
                ChromeProbeUiState::PermissionDeniedTcc { path, guidance } => div()
                    .p(px(8.0))
                    .rounded(px(7.0))
                    .bg(TdesktopThemePalette::rgba_with_alpha(0xE67E22, 0.16))
                    .border_1()
                    .border_color(rgb(0xE67E22))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .child(tg_icon(IconName::Lock, 13.0, 0xE67E22))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xE67E22))
                                    .child(tr_str(
                                        lang,
                                        "系统权限受限 (macOS TCC)",
                                        "Permission Denied (TCC)",
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_primary))
                            .child(guidance.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(path.clone()),
                    )
                    .into_any_element(),
                ChromeProbeUiState::NotFound => div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(
                        lang,
                        "未检测到本地 Chrome Cookie 文件，可在下方粘贴 JSON 或 Netscape Cookies 导入。",
                        "No local Chrome cookie DB found; paste JSON or Netscape cookies below.",
                    ))
                    .into_any_element(),
            }),
        )
        .child(
            panel_section_card(
                tr_str(lang, "JSON / Netscape Cookie 导入", "Import JSON / Netscape Cookies"),
                IconName::Copy,
                pal,
            )
            .child(browser_input_box(
                "browser-cookie-import-input",
                &rec.cookie_import_draft,
                tr_str(
                    lang,
                    "粘贴 JSON 数组或 Netscape cookies.txt 内容…",
                    "Paste JSON cookie array or Netscape cookies.txt…",
                ),
                ActiveBrowserInput::CookieImport,
                &ws.active_input,
                pal,
                true,
                cx,
            ))
            .child(panel_action_btn(
                "browser-import-cookies-btn",
                tr_str(lang, "导入 Cookies 到当前会话", "Import Cookies"),
                pal.accent,
                pal.active_button_fg,
                |this, cx| {
                    let raw = this.recorder.cookie_import_draft.clone();
                    this.import_cookies_from_text_async(raw, cx);
                },
                cx,
            ))
            .when_some(rec.cookie_status_message.clone(), |c, msg| {
                c.child(div().text_xs().text_color(rgb(pal.success)).child(msg))
            })
            .when_some(rec.cookie_error_message.clone(), |c, err| {
                c.child(div().text_xs().text_color(rgb(pal.danger)).child(err))
            }),
        )
        .into_any_element()
}
