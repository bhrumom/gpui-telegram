//! Rendering implementation for `SettingsRouter` OpenBot 2-column searchable settings page,
//! left navigation rail with instant cross-tab search, breadcrumb top bar, lazy active section
//! mounting, and interface language selector.

use super::settings_router::SettingsRouter;
use crate::i18n::tr_str;
use crate::theme::{tg_icon_rgba, IconName};
use crate::views::app_shell::modal_container::{macos_modal_backdrop_rgba, macos_modal_card};
use crate::views::settings::settings_type::SettingsSectionType;
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window,
};

struct SettingsNavGroup {
    zh: Option<&'static str>,
    en: Option<&'static str>,
    sections: &'static [SettingsSectionType],
}

/// OpenBot-aligned concise 10-tab navigation layout (primary ungrouped + Account + This computer).
/// All drill-down sub-pages remain reachable from their parent tab and instant cross-tab search.
pub(crate) const SETTINGS_NAV_GROUPS: &[SettingsNavGroup] = &[
    SettingsNavGroup {
        zh: None,
        en: None,
        sections: &[
            SettingsSectionType::Main,
            SettingsSectionType::Chat,
            SettingsSectionType::Notifications,
            SettingsSectionType::Calls,
            SettingsSectionType::Shortcuts,
        ],
    },
    SettingsNavGroup {
        zh: Some("账号"),
        en: Some("Account"),
        sections: &[
            SettingsSectionType::Information,
            SettingsSectionType::PrivacySecurity,
        ],
    },
    SettingsNavGroup {
        zh: Some("这台电脑"),
        en: Some("This computer"),
        sections: &[
            SettingsSectionType::AiProviders,
            SettingsSectionType::Computers,
            SettingsSectionType::Advanced,
        ],
    },
];

/// Returns all top-level settings sections displayed in the left navigation rail.
pub fn top_level_settings_nav_sections() -> Vec<SettingsSectionType> {
    SETTINGS_NAV_GROUPS
        .iter()
        .flat_map(|g| g.sections.iter().copied())
        .collect()
}

impl SettingsRouter {
    /// Renders an individual OpenBot-styled language row inside a grouped card.
    fn render_language_option(
        &self,
        lang_code: &'static str,
        badge_code: &'static str,
        title: &'static str,
        subtitle: &'static str,
        is_selected: bool,
        status_label: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let hover_bg = ob.hover();
        let badge_bg = if is_selected {
            ob.surface_light()
        } else {
            ob.bg_control_active()
        };
        let badge_fg = if is_selected {
            ob.text_on_light()
        } else {
            ob.text_primary()
        };

        div()
            .cursor_pointer()
            .w_full()
            .min_h(px(pal.scaled_px(super::settings_theme_ui::SETTINGS_ROW_MIN_HEIGHT_PX)))
            .px(px(pal.scaled_px(16.0)))
            .py(px(pal.scaled_px(12.0)))
            .hover(move |s| s.bg(hover_bg))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(pal.scaled_px(12.0)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.select_language_code(lang_code, cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(pal.scaled_px(12.0)))
                    .child(
                        div()
                            .px(px(pal.scaled_px(9.0)))
                            .py(px(pal.scaled_px(5.0)))
                            .rounded(px(pal.scaled_px(7.0)))
                            .bg(badge_bg)
                            .border_1()
                            .border_color(ob.border_hairline())
                            .flex()
                            .items_center()
                            .gap(px(pal.scaled_px(5.0)))
                            .child(tg_icon_rgba(IconName::Translate, pal.scaled_px(13.0), badge_fg))
                            .child(
                                div()
                                    .text_size(px(pal.scaled_px(12.0)))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(badge_fg)
                                    .child(badge_code),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(pal.scaled_px(13.5)))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_primary())
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(pal.scaled_px(12.0)))
                                    .text_color(ob.text_muted())
                                    .child(subtitle),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(pal.scaled_px(6.0)))
                    .when(is_selected, |el| {
                        el.child(tg_icon_rgba(
                            IconName::SelectCheck,
                            pal.scaled_px(14.0),
                            ob.text_primary(),
                        ))
                    })
                    .child(
                        div()
                            .text_size(px(pal.scaled_px(12.5)))
                            .font_weight(if is_selected {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(if is_selected {
                                ob.text_primary()
                            } else {
                                ob.text_muted()
                            })
                            .child(status_label),
                    ),
            )
            .into_any_element()
    }

    /// Renders the built-in interface language selector section.
    pub(super) fn render_language_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let is_zh = self.is_chinese_locale;
        let pal = self.palette;
        let lang = self.lang_id;
        super::settings_theme_ui::settings_container(pal).child(
            super::settings_theme_ui::settings_section_block(
                tr_str(lang, "界面语言", "Display Language"),
                Some(tr_str(
                    lang,
                    "菜单、按钮和提示文字将立即切换到所选语言。",
                    "Menus, buttons, and messages switch immediately to this language.",
                )),
                pal,
                super::settings_theme_ui::settings_group_card(
                    pal,
                    vec![
                        self.render_language_option(
                            "zh-CN",
                            "ZH",
                            "简体中文",
                            "zh-CN · 完整中文界面",
                            is_zh,
                            if is_zh { "当前使用" } else { "切换" },
                            cx,
                        ),
                        self.render_language_option(
                            "en-US",
                            "EN",
                            "English",
                            "en-US · English interface",
                            !is_zh,
                            if !is_zh { "Active" } else { "Select" },
                            cx,
                        ),
                    ],
                ),
            ),
        )
    }

    /// Resolves and lazily materializes the currently active settings section view.
    fn render_active_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.settings_section {
            SettingsSectionType::Main => self.ensure_main(cx).into_any_element(),
            SettingsSectionType::Information => self.ensure_information(cx).into_any_element(),
            SettingsSectionType::Notifications => self.ensure_notifications(cx).into_any_element(),
            SettingsSectionType::NotificationsReactions => {
                self.ensure_notif_reactions(cx).into_any_element()
            }
            SettingsSectionType::NotificationsType => self.ensure_notif_type(cx).into_any_element(),
            SettingsSectionType::PrivacySecurity => self.ensure_privacy(cx).into_any_element(),
            SettingsSectionType::BlockedPeers => self.ensure_blocked_peers(cx).into_any_element(),
            SettingsSectionType::ActiveSessions => {
                self.ensure_active_sessions(cx).into_any_element()
            }
            SettingsSectionType::LocalPasscode => {
                self.ensure_local_passcode(cx).into_any_element()
            }
            SettingsSectionType::Passkeys => self.ensure_passkeys(cx).into_any_element(),
            SettingsSectionType::GlobalTtl => self.ensure_global_ttl(cx).into_any_element(),
            SettingsSectionType::Websites => self.ensure_websites(cx).into_any_element(),
            SettingsSectionType::Chat => self.ensure_chat(cx).into_any_element(),
            SettingsSectionType::Folders => self.ensure_folders(cx).into_any_element(),
            SettingsSectionType::Advanced => self.ensure_advanced(cx).into_any_element(),
            SettingsSectionType::LocalStorage => self.ensure_local_storage(cx).into_any_element(),
            SettingsSectionType::PowerSaving => self.ensure_power_saving(cx).into_any_element(),
            SettingsSectionType::Calls => self.ensure_calls(cx).into_any_element(),
            SettingsSectionType::Credits => self.ensure_credits(cx).into_any_element(),
            SettingsSectionType::Premium => self.ensure_premium(cx).into_any_element(),
            SettingsSectionType::Shortcuts => self.ensure_shortcuts(cx).into_any_element(),
            SettingsSectionType::Business => self.ensure_business(cx).into_any_element(),
            SettingsSectionType::Computers => self.ensure_computers(cx).into_any_element(),
            SettingsSectionType::AiProviders => self.ensure_ai_providers(cx).into_any_element(),
            SettingsSectionType::Search => self.ensure_search(cx).into_any_element(),
            SettingsSectionType::Language => self.render_language_settings(cx).into_any_element(),
        }
    }

    /// Maps any settings section (including drill-down sub-pages) to its highlighted top-level nav tab.
    fn nav_highlight_section(&self) -> SettingsSectionType {
        self.settings_section.nav_parent()
    }

    /// Renders OpenBot's `220px` left navigation rail with title, instant search pill,
    /// ranked search results / 3 concise grouped navigation sections (10 items total), and quiet footer.
    fn render_left_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let zh = self.is_chinese_locale;
        let lang = self.lang_id;
        let active_nav = self.nav_highlight_section();
        let query_trimmed = self.nav_search_query.trim().to_string();
        let has_query = !query_trimmed.is_empty();
        let search_focused = self.nav_search_focused;
        let hover_row = ob.hover();
        let hover_ctrl = ob.bg_control_hover();
        let active_bg = ob.bg_control_hover();
        let text_pri = ob.text_primary();
        let text_sec = ob.text_secondary();
        let text_muted = ob.text_muted();

        let nav_body: AnyElement = if has_query {
            let hits = self.ranked_nav_search_results();
            let sel_idx = self.nav_search_selected_index;
            if hits.is_empty() {
                div()
                    .w_full()
                    .px(px(10.0))
                    .py(px(18.0))
                    .text_size(px(12.5))
                    .text_color(text_muted)
                    .child(tr_str(lang, "无匹配设置项", "No matching settings"))
                    .into_any_element()
            } else {
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .children(hits.into_iter().enumerate().map(|(idx, hit)| {
                        let is_sel = idx == sel_idx;
                        let target = hit.section;
                        let parent_tab = target.nav_parent();
                        let place_label = parent_tab.nav_title_localized(zh);
                        let title = if zh { hit.title_zh } else { hit.title_en };
                        let desc = if zh { hit.desc_zh } else { hit.desc_en };
                        div()
                            .id(("settings-nav-hit", idx))
                            .cursor_pointer()
                            .w_full()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(8.0))
                            .when(is_sel, |el| el.bg(active_bg))
                            .hover(move |s| s.bg(hover_row))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.open_search_hit(target, cx);
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .child(tg_icon_rgba(
                                        parent_tab.icon_name(),
                                        12.0,
                                        text_muted,
                                    ))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(text_muted)
                                            .truncate()
                                            .child(place_label),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(text_pri)
                                    .truncate()
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(text_muted)
                                    .truncate()
                                    .child(desc),
                            )
                    }))
                    .into_any_element()
            }
        } else {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .children(SETTINGS_NAV_GROUPS.iter().enumerate().map(|(g_idx, group)| {
                    let group_label = if zh { group.zh } else { group.en };
                    div()
                        .w_full()
                        .when(g_idx > 0, |el| el.pt(px(14.0)))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .when_some(group_label, |el, heading| {
                            el.child(
                                div()
                                    .px(px(10.0))
                                    .pb(px(4.0))
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(text_muted)
                                    .child(heading),
                            )
                        })
                        .children(group.sections.iter().copied().map(|sec| {
                            let is_active = active_nav == sec;
                            let label = sec.nav_title_localized(zh);
                            div()
                                .id(gpui::SharedString::from(format!("settings-nav-item-{sec:?}")))
                                .cursor_pointer()
                                .w_full()
                                .h(px(32.0))
                                .px(px(10.0))
                                .rounded(px(8.0))
                                .when(is_active, |el| el.bg(active_bg))
                                .hover(move |s| s.bg(if is_active { active_bg } else { hover_row }))
                                .flex()
                                .items_center()
                                .gap(px(9.0))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.select_nav_tab(sec, cx);
                                    }),
                                )
                                .child(tg_icon_rgba(
                                    sec.icon_name(),
                                    15.0,
                                    if is_active { text_pri } else { text_muted },
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_size(px(13.0))
                                        .font_weight(if is_active {
                                            FontWeight::MEDIUM
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .text_color(if is_active { text_pri } else { text_sec })
                                        .child(label),
                                )
                        }))
                }))
                .into_any_element()
        };

        let page_layout = super::settings_theme_ui::compute_settings_page_layout(self);
        let version_label = format!("v{}", crate::app::updater::CURRENT_APP_VERSION);
        div()
            .w(px(page_layout.left_nav_px))
            .flex_shrink_0()
            .h_full()
            .bg(ob.bg_sidebar())
            .border_r_1()
            .border_color(ob.border_hairline())
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .px(px(12.0))
                    .pt(px(18.0))
                    .pb(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(
                        div()
                            .px(px(4.0))
                            .text_size(px(20.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_pri)
                            .child(tr_str(lang, "设置", "Settings")),
                    )
                    .child(
                        div()
                            .id("settings-nav-search-field")
                            .cursor_pointer()
                            .w_full()
                            .h(px(32.0))
                            .px(px(11.0))
                            .rounded_full()
                            .bg(ob.bg_control())
                            .border_1()
                            .border_color(if search_focused {
                                ob.text_muted()
                            } else {
                                ob.border_hairline()
                            })
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.nav_search_focused = true;
                                    this.sync_active_input_key(cx);
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(7.0))
                                    .child(tg_icon_rgba(IconName::Search, 13.0, text_muted))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .text_size(px(12.5))
                                            .text_color(if self.nav_search_query.is_empty() {
                                                text_muted
                                            } else {
                                                text_pri
                                            })
                                            .child(if self.nav_search_query.is_empty() {
                                                tr_str(lang, "搜索设置", "Search settings")
                                                    .to_string()
                                            } else {
                                                self.nav_search_query.clone()
                                            }),
                                    ),
                            )
                            .when(!self.nav_search_query.is_empty(), |el| {
                                el.child(
                                    div()
                                        .cursor_pointer()
                                        .w(px(18.0))
                                        .h(px(18.0))
                                        .rounded_full()
                                        .hover(move |s| s.bg(hover_ctrl))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.nav_search_query.clear();
                                                this.nav_search_selected_index = 0;
                                                cx.notify();
                                                cx.stop_propagation();
                                            }),
                                        )
                                        .child(tg_icon_rgba(IconName::Close, 11.0, text_muted)),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .id("settings-nav-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .px(px(8.0))
                    .pb(px(12.0))
                    .child(nav_body),
            )
            .child(
                div()
                    .w_full()
                    .px(px(12.0))
                    .py(px(10.0))
                    .border_t_1()
                    .border_color(ob.border_hairline())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(text_muted)
                            .child(format!("Fabushi {version_label}")),
                    )
                    .child(
                        div()
                            .id("settings-footer-lang-toggle")
                            .cursor_pointer()
                            .h(px(24.0))
                            .px(px(8.0))
                            .rounded(px(6.0))
                            .hover(move |s| s.bg(hover_ctrl).text_color(text_pri))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_sec)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_language(cx);
                                }),
                            )
                            .child(tg_icon_rgba(IconName::Translate, 11.5, text_muted))
                            .child(tr_str(lang, "EN", "中文")),
                    ),
            )
    }

    /// Renders OpenBot's 2-column searchable settings frame (`220px` left nav + clean top bar
    /// showing breadcrumbs ONLY for drill-down sub-pages & circular `×` close button + `960px` max centered canvas).
    fn render_settings_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let zh = self.is_chinese_locale;
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.lang_id;
        let current = self.settings_section;
        let is_subpage = !current.is_top_level_nav_tab();
        let title = current.nav_title_localized(zh);
        let subtitle = current.subtitle_localized(zh);
        let hover_bg = ob.hover();
        let text_pri = ob.text_primary();
        let text_sec = ob.text_secondary();
        let text_muted = ob.text_muted();

        let page_layout = super::settings_theme_ui::compute_settings_page_layout(self);
        let raw_crumbs = self.settings_section.breadcrumb_path();
        let nav_parent = current.nav_parent();
        let display_crumbs: Vec<SettingsSectionType> = if is_subpage {
            if nav_parent == SettingsSectionType::Main {
                vec![SettingsSectionType::Main, current]
            } else {
                let mut trimmed: Vec<SettingsSectionType> = raw_crumbs
                    .into_iter()
                    .filter(|sec| *sec != SettingsSectionType::Main)
                    .collect();
                if trimmed.first().copied() != Some(nav_parent) {
                    trimmed.insert(0, nav_parent);
                }
                trimmed
            }
        } else {
            Vec::new()
        };
        let show_page_heading = !is_subpage
            && !matches!(
                current,
                SettingsSectionType::AiProviders | SettingsSectionType::Computers
            );
        let is_computers = current == SettingsSectionType::Computers;

        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .overflow_hidden()
            .bg(ob.bg_canvas())
            .flex()
            .flex_row()
            .child(self.render_left_nav(cx))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .bg(ob.bg_canvas())
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(page_layout.topbar_height_px))
                            .flex_shrink_0()
                            .w_full()
                            .px(px(20.0))
                            .when(is_subpage, |el| {
                                el.border_b_1().border_color(ob.border_hairline())
                            })
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .min_w(px(0.0))
                                    .when(is_subpage, |el| {
                                        el.child(
                                            div()
                                                .id("settings-subpage-back-btn")
                                                .cursor_pointer()
                                                .h(px(28.0))
                                                .px(px(8.0))
                                                .rounded(px(8.0))
                                                .hover(move |s| {
                                                    s.bg(hover_bg).text_color(text_pri)
                                                })
                                                .flex()
                                                .items_center()
                                                .gap(px(4.0))
                                                .text_size(px(12.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(text_sec)
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        this.navigate_back(cx);
                                                    }),
                                                )
                                                .child(tg_icon_rgba(
                                                    IconName::BackArrow,
                                                    13.0,
                                                    text_muted,
                                                ))
                                                .child(tr_str(lang, "返回", "Back")),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(6.0))
                                                .children(display_crumbs.into_iter().enumerate().map(
                                                    |(idx, sec)| {
                                                        let is_current = sec == current;
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap(px(6.0))
                                                            .when(idx > 0, |sep| {
                                                                sep.child(
                                                                    div()
                                                                        .text_size(px(12.5))
                                                                        .text_color(text_muted)
                                                                        .child("›"),
                                                                )
                                                            })
                                                            .child(
                                                                div()
                                                                    .text_size(px(13.5))
                                                                    .font_weight(if is_current {
                                                                        FontWeight::SEMIBOLD
                                                                    } else {
                                                                        FontWeight::NORMAL
                                                                    })
                                                                    .text_color(if is_current {
                                                                        text_pri
                                                                    } else {
                                                                        text_muted
                                                                    })
                                                                    .when(!is_current, |clickable| {
                                                                        clickable
                                                                            .cursor_pointer()
                                                                            .hover(move |s| {
                                                                                s.text_color(text_pri)
                                                                            })
                                                                            .on_mouse_down(
                                                                                MouseButton::Left,
                                                                                cx.listener(
                                                                                    move |this, _, _, cx| {
                                                                                        if sec.is_top_level_nav_tab() {
                                                                                            this.select_nav_tab(sec, cx);
                                                                                        } else {
                                                                                            this.navigate_to(sec, false, cx);
                                                                                        }
                                                                                    },
                                                                                ),
                                                                            )
                                                                    })
                                                                    .child(
                                                                        sec.nav_title_localized(
                                                                            zh,
                                                                        ),
                                                                    ),
                                                            )
                                                    },
                                                )),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .id("settings-modal-close-btn")
                                    .cursor_pointer()
                                    .w(px(28.0))
                                    .h(px(28.0))
                                    .rounded_full()
                                    .hover(move |s| s.bg(hover_bg))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.close(cx);
                                        }),
                                    )
                                    .child(tg_icon_rgba(IconName::Close, 14.0, text_muted)),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-scroll-view")
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .child(
                                div()
                                    .w_full()
                                    .when(!is_computers, |el| {
                                        el.max_w(px(page_layout.content_max_px))
                                            .mx_auto()
                                            .px(px(28.0))
                                            .pt(px(if is_subpage { 14.0 } else { 4.0 }))
                                            .pb(px(36.0))
                                    })
                                    .when(is_computers, |el| {
                                        el.max_w(px(page_layout.content_max_px))
                                            .mx_auto()
                                            .px(px(16.0))
                                            .pb(px(24.0))
                                    })
                                    .flex()
                                    .flex_col()
                                    .gap(px(18.0))
                                    .when(show_page_heading, |el| {
                                        el.child(super::settings_theme_ui::settings_page_heading(
                                            title,
                                            Some(subtitle),
                                            pal,
                                        ))
                                    })
                                    .when(is_subpage && !subtitle.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .text_size(px(13.0))
                                                .text_color(text_muted)
                                                .child(subtitle),
                                        )
                                    })
                                    .child(self.render_active_section(cx)),
                            ),
                    )
                    .when(page_layout.save_bar_dock_visible, |col| {
                        let dock_bg = gpui::rgb(if ob.scheme.is_dark() {
                            super::settings_theme_ui::SETTINGS_CARD_BG_DARK
                        } else {
                            super::settings_theme_ui::SETTINGS_CARD_BG_LIGHT
                        });
                        let err_msg = self.save_bar.error.clone();
                        col.child(
                            div()
                                .id("settings-save-bar-dock")
                                .w_full()
                                .px(px(24.0))
                                .py(px(12.0))
                                .bg(dock_bg)
                                .border_t_1()
                                .border_color(ob.border_hairline())
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(if err_msg.is_some() {
                                            ob.danger()
                                        } else {
                                            text_sec
                                        })
                                        .child(err_msg.unwrap_or_else(|| {
                                            tr_str(lang, "有未保存的设置更改", "Unsaved settings changes")
                                                .to_string()
                                        })),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .id("settings-save-bar-discard")
                                                .cursor_pointer()
                                                .px(px(12.0))
                                                .py(px(6.0))
                                                .rounded(px(8.0))
                                                .bg(ob.bg_control())
                                                .border_1()
                                                .border_color(ob.border_strong_hairline())
                                                .text_size(px(12.5))
                                                .text_color(text_pri)
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        this.save_bar.discard();
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(tr_str(lang, "放弃", "Discard")),
                                        )
                                        .child(
                                            div()
                                                .id("settings-save-bar-commit")
                                                .cursor_pointer()
                                                .px(px(14.0))
                                                .py(px(6.0))
                                                .rounded(px(8.0))
                                                .bg(ob.surface_light())
                                                .text_size(px(12.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(ob.text_on_light())
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        let mut toast = crate::views::app_shell::modal_container::OpenBotToasterState::default();
                                                        this.save_bar.commit_save(&mut toast);
                                                        if let Some(t) = toast.toasts.pop() {
                                                            cx.emit(super::SettingsRouterEvent::ShowToast(t.message));
                                                        }
                                                        cx.notify();
                                                    }),
                                                )
                                                .child(tr_str(lang, "保存更改", "Save changes")),
                                        ),
                                ),
                        )
                    }),
            )
    }
}

impl Render for SettingsRouter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let is_standalone_computers =
            self.settings_section == SettingsSectionType::Computers && self.standalone_computers_mode;
        let panel_body: AnyElement = if is_standalone_computers {
            let comp = self.ensure_computers(cx);
            if !comp.read(cx).standalone_mode {
                comp.update(cx, |sc, _| sc.standalone_mode = true);
            }
            comp.into_any_element()
        } else {
            if let Some(ref comp) = self.settings_computers {
                if comp.read(cx).standalone_mode {
                    comp.update(cx, |sc, _| sc.standalone_mode = false);
                }
            }
            self.render_settings_panel(cx).into_any_element()
        };

        let mut modal_box = macos_modal_card("telegram-settings-modal-box", pal)
            .bg(ob.bg_canvas())
            .max_w_full()
            .max_h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(panel_body);

        let page_layout = super::settings_theme_ui::compute_settings_page_layout(self);
        if is_standalone_computers {
            let box_w = pal.scaled_px(960.0).min(1160.0);
            let box_h = pal.scaled_px(660.0).min(860.0);
            modal_box = modal_box.w(px(box_w)).h(px(box_h));
        } else {
            modal_box = modal_box
                .w_full()
                .max_w(px(page_layout.modal_max_width_px))
                .h_full()
                .max_h(px(page_layout.modal_max_height_px));
        }

        div()
            .id("telegram-settings-modal-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .p(px(20.0))
            .occlude()
            .bg(macos_modal_backdrop_rgba(pal))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close(cx);
                }),
            )
            .child(modal_box)
    }
}
