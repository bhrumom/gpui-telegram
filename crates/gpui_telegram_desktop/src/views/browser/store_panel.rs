//! Automation App Store Marketplace UI (`F8.2`):
//! Publish local Script Apps (with explicit HTTP 422 secret-redaction alerts),
//! browse/search server catalog, install/uninstall, and execute zero-LLM replays.

use bot_browser::{validate_script_app_safe_for_publish, AutomationScriptApp};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_client::HttpClientError;
use gpui_telegram_proto::automation_apps::{
    AutomationAppSummary, InstalledAutomationApp, PublishAutomationAppRequest,
};

use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::browser::recorder_panel::{browser_input_box, panel_action_btn, panel_section_card};
use crate::views::browser::workspace::{ActiveBrowserInput, BrowserWorkspace};
use crate::views::tdesktop_theme::tr_str;

#[derive(Debug, Default)]
pub struct StorePanelState {
    pub selected_draft_app: Option<AutomationScriptApp>,
    pub changelog_draft: String,
    pub is_publishing: bool,
    pub publish_error: Option<String>,
    pub publish_secret_rejected: bool,
    pub publish_success: Option<String>,
    pub search_query_draft: String,
    pub category_filter: Option<String>,
    pub is_loading_catalog: bool,
    pub catalog_apps: Vec<AutomationAppSummary>,
    pub installed_apps: Vec<InstalledAutomationApp>,
    pub store_error: Option<String>,
    pub last_installed_app_id: Option<String>,
}

impl StorePanelState {
    pub fn build_publish_request(
        app: &AutomationScriptApp,
        changelog: Option<String>,
    ) -> Result<PublishAutomationAppRequest, String> {
        let manifest = serde_json::to_value(app)
            .map_err(|e| format!("Failed to serialize AutomationScriptApp manifest: {e}"))?;
        let cleaned_changelog = changelog
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Ok(PublishAutomationAppRequest {
            manifest,
            is_published: true,
            changelog: cleaned_changelog,
        })
    }

    pub fn format_publish_error(err: &HttpClientError) -> (bool, String) {
        match err {
            HttpClientError::Api { status: 422, message } => (
                true,
                format!(
                    "Publish blocked (422 secret_leak_detected): {message}. Replace plaintext secrets with {{param}} (is_secret=true) or secret_vault_key."
                ),
            ),
            HttpClientError::Api { status: 403, message } => {
                (false, format!("Publish forbidden (403): {message}"))
            }
            HttpClientError::Api { status: 400, message } => {
                (false, format!("Invalid manifest (400): {message}"))
            }
            HttpClientError::Conflict(message) => (
                false,
                format!("Version conflict (409): {message}. Bump version before publishing."),
            ),
            other => (false, format!("Publish failed: {other}")),
        }
    }
}

pub fn render_store_tab(
    ws: &BrowserWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<BrowserWorkspace>,
) -> AnyElement {
    let store = &ws.store;
    let draft_app = store
        .selected_draft_app
        .as_ref()
        .or_else(|| ws.recorder.selected_script());
    let precheck_warning = draft_app.and_then(|app| validate_script_app_safe_for_publish(app).err());

    let categories: &[(&str, Option<&str>)] = &[
        ("All", None),
        ("social", Some("social")),
        ("general", Some("general")),
        ("productivity", Some("productivity")),
    ];

    div()
        .id("browser-store-tab-scroll")
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            panel_section_card(
                tr_str(lang, "发布脚本应用到商店", "Publish Script App to Store"),
                IconName::ArrowUp,
                pal,
            )
            .child(if let Some(app) = draft_app {
                div()
                    .p(px(8.0))
                    .rounded(px(6.0))
                    .bg(rgb(pal.input_bg))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(format!("{} v{} ({})", app.name, app.version, app.category)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(format!(
                                "{} steps · id={}",
                                app.steps.len(),
                                app.app_id
                            )),
                    )
                    .into_any_element()
            } else {
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(tr_str(
                        lang,
                        "请先在“脚本”标签页将录制轨迹保存为脚本应用。",
                        "Distill a Script App in the Recorder tab first to publish it.",
                    ))
                    .into_any_element()
            })
            .when_some(precheck_warning, |card, warn| {
                card.child(
                    div()
                        .p(px(7.0))
                        .rounded(px(6.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(0xE67E22, 0.14))
                        .text_xs()
                        .text_color(rgb(0xE67E22))
                        .child(format!("Pre-check warning: {warn}")),
                )
            })
            .child(browser_input_box(
                "browser-store-changelog-input",
                &store.changelog_draft,
                tr_str(lang, "更新说明 (可选 changelog)", "Changelog (optional)"),
                ActiveBrowserInput::StoreChangelog,
                &ws.active_input,
                pal,
                false,
                cx,
            ))
            .when_some(draft_app.cloned(), |card, app| {
                card.child(panel_action_btn(
                    "browser-store-publish-btn",
                    if store.is_publishing {
                        tr_str(lang, "发布中…", "Publishing…")
                    } else {
                        tr_str(lang, "上架到应用商店", "Publish to Store")
                    },
                    pal.accent,
                    pal.active_button_fg,
                    move |this, cx| {
                        this.publish_script_app(app.clone(), cx);
                    },
                    cx,
                ))
            })
            .when_some(store.publish_error.clone(), |card, err| {
                let is_secret = store.publish_secret_rejected;
                card.child(
                    div()
                        .p(px(8.0))
                        .rounded(px(7.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.14))
                        .border_1()
                        .border_color(rgb(pal.danger))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .when(is_secret, |b| {
                            b.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .child(tg_icon(IconName::Lock, 12.0, pal.danger))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(pal.danger))
                                            .child(tr_str(
                                                lang,
                                                "安全脱敏校验拦截 (422)",
                                                "Secret Redaction Rejected (422)",
                                            )),
                                    ),
                            )
                        })
                        .child(div().text_xs().text_color(rgb(pal.danger)).child(err)),
                )
            })
            .when_some(store.publish_success.clone(), |card, ok_msg| {
                card.child(
                    div()
                        .p(px(7.0))
                        .rounded(px(6.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.success, 0.14))
                        .text_xs()
                        .text_color(rgb(pal.success))
                        .child(ok_msg),
                )
            }),
        )
        .child(
            panel_section_card(
                tr_str(lang, "应用商店目录与检索", "Browse & Search Store"),
                IconName::Search,
                pal,
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(browser_input_box(
                        "browser-store-search-input",
                        &store.search_query_draft,
                        tr_str(lang, "搜索自动化脚本应用…", "Search automation apps…"),
                        ActiveBrowserInput::StoreSearch,
                        &ws.active_input,
                        pal,
                        false,
                        cx,
                    ))
                    .child(panel_action_btn(
                        "browser-store-search-btn",
                        if store.is_loading_catalog {
                            tr_str(lang, "搜索中…", "Searching…")
                        } else {
                            tr_str(lang, "搜索", "Search")
                        },
                        pal.accent,
                        pal.active_button_fg,
                        |this, cx| this.search_store_apps(cx),
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(5.0))
                    .children(categories.iter().map(|(label, cat_val)| {
                        let active = store.category_filter.as_deref() == *cat_val;
                        let cat_owned = cat_val.map(str::to_string);
                        div()
                            .px(px(7.0))
                            .py(px(3.0))
                            .rounded(px(5.0))
                            .bg(rgb(if active { pal.accent } else { pal.input_bg }))
                            .text_xs()
                            .text_color(rgb(if active {
                                pal.active_button_fg
                            } else {
                                pal.text_secondary
                            }))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.store.category_filter = cat_owned.clone();
                                    this.search_store_apps(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(*label)
                    }))
                    .child(panel_action_btn(
                        "browser-store-refresh-installed-btn",
                        tr_str(lang, "同步已安装", "Sync Installed"),
                        pal.input_bg,
                        pal.text_primary,
                        |this, cx| this.refresh_installed_store_apps(cx),
                        cx,
                    )),
            )
            .when_some(store.store_error.clone(), |card, err| {
                card.child(div().text_xs().text_color(rgb(pal.danger)).child(err))
            })
            .when(store.catalog_apps.is_empty(), |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(
                            lang,
                            "点击“搜索”浏览服务端已发布的脚本应用。",
                            "Click Search to browse published Automation Script Apps.",
                        )),
                )
            })
            .children(store.catalog_apps.iter().map(|item| {
                let is_installed = item.is_installed
                    || store
                        .installed_apps
                        .iter()
                        .any(|ins| ins.app_id == item.app_id);
                let id_install = item.app_id.clone();
                let id_uninstall = item.app_id.clone();
                div()
                    .p(px(8.0))
                    .rounded(px(7.0))
                    .bg(rgb(pal.input_bg))
                    .border_1()
                    .border_color(rgb(pal.border_fg))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
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
                                    .child(format!("{} v{}", item.name, item.latest_version)),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.5))
                                    .rounded(px(4.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                    .text_xs()
                                    .text_color(rgb(pal.accent))
                                    .child(item.category.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(format!(
                                "{} · {} steps · {} installs · {} runs",
                                item.author_username.as_deref().unwrap_or("author"),
                                item.step_count,
                                item.install_count,
                                item.run_count
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .when(!is_installed, |row| {
                                row.child(panel_action_btn(
                                    SharedString::from(format!("install-store-{}", item.app_id)),
                                    tr_str(lang, "安装", "Install"),
                                    pal.accent,
                                    pal.active_button_fg,
                                    move |this, cx| {
                                        this.install_store_app(&id_install, cx);
                                    },
                                    cx,
                                ))
                            })
                            .when(is_installed, |row| {
                                row.child(
                                    div()
                                        .px(px(7.0))
                                        .py(px(3.0))
                                        .rounded(px(5.0))
                                        .bg(TdesktopThemePalette::rgba_with_alpha(
                                            pal.success,
                                            0.16,
                                        ))
                                        .text_xs()
                                        .text_color(rgb(pal.success))
                                        .child(tr_str(lang, "已安装", "Installed")),
                                )
                                .child(panel_action_btn(
                                    SharedString::from(format!("uninstall-store-{}", item.app_id)),
                                    tr_str(lang, "卸载", "Uninstall"),
                                    pal.card_bg,
                                    pal.danger,
                                    move |this, cx| {
                                        this.uninstall_store_app(&id_uninstall, cx);
                                    },
                                    cx,
                                ))
                            }),
                    )
            })),
        )
        .child(
            panel_section_card(
                tr_str(lang, "已安装应用（离线缓存可执行）", "Installed Apps"),
                IconName::SelectCheck,
                pal,
            )
            .when(store.installed_apps.is_empty(), |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child(tr_str(
                            lang,
                            "暂无已安装的脚本应用。",
                            "No installed Automation Script Apps yet.",
                        )),
                )
            })
            .children(store.installed_apps.iter().map(|ins| {
                let ins_clone = ins.clone();
                let id_uninst = ins.app_id.clone();
                let parsed_params = serde_json::from_value::<AutomationScriptApp>(ins.manifest.clone())
                    .map(|app| app.parameters)
                    .unwrap_or_default();
                div()
                    .p(px(8.0))
                    .rounded(px(7.0))
                    .bg(rgb(pal.input_bg))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
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
                                    .child(format!("{} v{}", ins.name, ins.installed_version)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(pal.text_secondary))
                                    .child(format!("runs: {}", ins.local_run_count)),
                            ),
                    )
                    .when(!parsed_params.is_empty(), |card| {
                        card.child(
                            div()
                                .pt(px(2.0))
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .children(parsed_params.iter().map(|p| {
                                    if p.is_secret {
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .px(px(6.0))
                                            .py(px(3.0))
                                            .rounded(px(5.0))
                                            .bg(TdesktopThemePalette::rgba_with_alpha(
                                                pal.success,
                                                0.12,
                                            ))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(4.0))
                                                    .child(tg_icon(IconName::Lock, 11.0, pal.success))
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
                                        let val = ws
                                            .recorder
                                            .param_inputs
                                            .get(&p.name)
                                            .cloned()
                                            .or_else(|| p.default_value.clone())
                                            .unwrap_or_default();
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.0))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(pal.text_secondary))
                                                    .child(p.name.clone()),
                                            )
                                            .child(browser_input_box(
                                                SharedString::from(format!(
                                                    "store-installed-{}-param-{}",
                                                    ins.app_id, p.name
                                                )),
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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(panel_action_btn(
                                SharedString::from(format!("run-installed-{}", ins.app_id)),
                                tr_str(lang, "运行 (无 LLM)", "Run (No LLM)"),
                                pal.success,
                                0xFFFFFF,
                                move |this, cx| {
                                    this.run_installed_store_app(ins_clone.clone(), cx);
                                },
                                cx,
                            ))
                            .child(panel_action_btn(
                                SharedString::from(format!("remove-installed-{}", ins.app_id)),
                                tr_str(lang, "移除", "Remove"),
                                pal.card_bg,
                                pal.danger,
                                move |this, cx| {
                                    this.uninstall_store_app(&id_uninst, cx);
                                },
                                cx,
                            )),
                    )
            })),
        )
        .into_any_element()
}
