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
    zh: &'static str,
    en: &'static str,
    sections: &'static [SettingsSectionType],
}

const SETTINGS_NAV_GROUPS: &[SettingsNavGroup] = &[
    SettingsNavGroup {
        zh: "常规",
        en: "General",
        sections: &[
            SettingsSectionType::Main,
            SettingsSectionType::Information,
            SettingsSectionType::Chat,
            SettingsSectionType::Folders,
            SettingsSectionType::Notifications,
            SettingsSectionType::Calls,
            SettingsSectionType::Language,
        ],
    },
    SettingsNavGroup {
        zh: "隐私与安全",
        en: "Privacy & Security",
        sections: &[
            SettingsSectionType::PrivacySecurity,
            SettingsSectionType::ActiveSessions,
            SettingsSectionType::LocalPasscode,
            SettingsSectionType::Passkeys,
            SettingsSectionType::BlockedPeers,
            SettingsSectionType::GlobalTtl,
            SettingsSectionType::Websites,
        ],
    },
    SettingsNavGroup {
        zh: "AI 与工作区",
        en: "AI & Workspace",
        sections: &[
            SettingsSectionType::AiProviders,
            SettingsSectionType::Computers,
            SettingsSectionType::Shortcuts,
            SettingsSectionType::Business,
        ],
    },
    SettingsNavGroup {
        zh: "存储与账户",
        en: "System & Account",
        sections: &[
            SettingsSectionType::Advanced,
            SettingsSectionType::LocalStorage,
            SettingsSectionType::PowerSaving,
            SettingsSectionType::Premium,
            SettingsSectionType::Credits,
        ],
    },
];

impl SettingsRouter {
    /// Renders an individual OpenBot-styled language card (`12px` radius, `64px` min-height).
    fn render_language_option(
        &self,
        lang_code: &'static str,
        badge_code: &'static str,
        title: &'static str,
        subtitle: &'static str,
        is_selected: bool,
        status_label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let hover_bg = ob.bg_control_hover();
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

        let card_bg = gpui::rgb(if ob.scheme.is_dark() {
            super::settings_theme_ui::SETTINGS_CARD_BG_DARK
        } else {
            super::settings_theme_ui::SETTINGS_CARD_BG_LIGHT
        });
        div()
            .cursor_pointer()
            .min_h(px(pal.scaled_px(super::settings_theme_ui::SETTINGS_ROW_MIN_HEIGHT_PX)))
            .px(px(pal.scaled_px(16.0)))
            .py(px(pal.scaled_px(12.0)))
            .rounded(px(pal.scaled_px(super::settings_theme_ui::SETTINGS_CARD_RADIUS_PX)))
            .bg(card_bg)
            .border_1()
            .border_color(if is_selected {
                ob.border_strong_hairline()
            } else {
                ob.frame_card_border()
            })
            .hover(move |s| s.bg(hover_bg))
            .flex()
            .items_center()
            .justify_between()
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
                            .px(px(pal.scaled_px(10.0)))
                            .py(px(pal.scaled_px(6.0)))
                            .rounded(px(pal.scaled_px(8.0)))
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
                                    .text_size(px(pal.scaled_px(14.0)))
                                    .font_weight(FontWeight::SEMIBOLD)
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
                            .text_size(px(pal.scaled_px(13.0)))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if is_selected {
                                ob.text_primary()
                            } else {
                                ob.text_muted()
                            })
                            .child(status_label),
                    ),
            )
    }

    /// Renders the built-in interface language selector section.
    pub(super) fn render_language_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let is_zh = self.is_chinese_locale;
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.lang_id;
        div()
            .flex()
            .flex_col()
            .gap(px(pal.scaled_px(16.0)))
            .py(px(pal.scaled_px(8.0)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(pal.scaled_px(10.0)))
                    .child(
                        div()
                            .w(px(pal.scaled_px(32.0)))
                            .h(px(pal.scaled_px(32.0)))
                            .rounded_full()
                            .bg(ob.bg_control_active())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(tg_icon_rgba(
                                IconName::Translate,
                                pal.scaled_px(16.0),
                                ob.text_primary(),
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(pal.scaled_px(18.0)))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ob.text_primary())
                            .child(tr_str(lang, "界面语言设置", "Interface Language Settings")),
                    ),
            )
            .child(
                div()
                    .text_size(px(pal.scaled_px(13.0)))
                    .text_color(ob.text_muted())
                    .child(if is_zh {
                        "选择应用的默认显示语言。修改后将即时生效并持久化。"
                    } else {
                        "Choose the default display language for the app. Changes apply and persist immediately."
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(pal.scaled_px(10.0)))
                    .child(self.render_language_option(
                        "zh-CN",
                        "ZH",
                        "简体中文 (Simplified Chinese)",
                        "zh-CN • 完整翻译支持",
                        is_zh,
                        if is_zh { "已启用" } else { "选择" },
                        cx,
                    ))
                    .child(self.render_language_option(
                        "en-US",
                        "EN",
                        "English (US)",
                        "en-US • Default English",
                        !is_zh,
                        if !is_zh { "Active" } else { "Select" },
                        cx,
                    )),
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

    /// Maps sub-sections (like `NotificationsType` / `NotificationsReactions`) to their highlighted nav item.
    fn nav_highlight_section(&self) -> SettingsSectionType {
        match self.settings_section {
            SettingsSectionType::NotificationsType
            | SettingsSectionType::NotificationsReactions => SettingsSectionType::Notifications,
            other => other,
        }
    }

    /// Renders OpenBot's `220px` left navigation rail with title, instant search pill,
    /// ranked search results / 4 grouped navigation sections, and monochrome footer.
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
                        let title = if zh { hit.title_zh } else { hit.title_en };
                        let desc = if zh { hit.desc_zh } else { hit.desc_en };
                        div()
                            .id(("settings-nav-hit", idx))
                            .cursor_pointer()
                            .w_full()
                            .px(px(10.0))
                            .py(px(7.0))
                            .rounded(px(8.0))
                            .when(is_sel, |el| el.bg(active_bg))
                            .hover(move |s| s.bg(hover_row))
                            .flex()
                            .items_start()
                            .gap(px(8.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.nav_search_query.clear();
                                    this.nav_search_selected_index = 0;
                                    this.nav_search_focused = false;
                                    this.navigate_to(target, false, cx);
                                }),
                            )
                            .child(
                                div()
                                    .pt(px(2.0))
                                    .child(tg_icon_rgba(
                                        target.icon_name(),
                                        14.0,
                                        if is_sel { text_pri } else { text_muted },
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(1.0))
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
                                    ),
                            )
                    }))
                    .into_any_element()
            }
        } else {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .children(SETTINGS_NAV_GROUPS.iter().map(|group| {
                    let group_label = if zh { group.zh } else { group.en };
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .px(px(10.0))
                                .pb(px(3.0))
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_muted)
                                .child(group_label),
                        )
                        .children(group.sections.iter().copied().map(|sec| {
                            let is_active = active_nav == sec;
                            let label = sec.display_title_localized(zh);
                            div()
                                .id(gpui::SharedString::from(format!("settings-nav-item-{sec:?}")))
                                .cursor_pointer()
                                .w_full()
                                .h(px(30.0))
                                .px(px(10.0))
                                .rounded(px(8.0))
                                .when(is_active, |el| el.bg(active_bg))
                                .hover(move |s| s.bg(if is_active { active_bg } else { hover_row }))
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.navigate_to(sec, false, cx);
                                    }),
                                )
                                .child(tg_icon_rgba(
                                    sec.icon_name(),
                                    14.0,
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
                    .pt(px(16.0))
                    .pb(px(10.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .px(px(4.0))
                            .text_size(px(18.0))
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
                            .px(px(10.0))
                            .rounded_full()
                            .bg(ob.bg_control())
                            .border_1()
                            .border_color(if search_focused {
                                ob.text_muted()
                            } else {
                                ob.border_strong_hairline()
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
                                    .gap(px(6.0))
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
                                                tr_str(lang, "搜索设置...", "Search settings...")
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
                    .px(px(8.0))
                    .py(px(8.0))
                    .border_t_1()
                    .border_color(ob.border_hairline())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(4.0))
                    .child(
                        div()
                            .cursor_pointer()
                            .flex_1()
                            .h(px(28.0))
                            .px(px(8.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_control())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .hover(move |s| s.bg(hover_ctrl))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(5.0))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_pri)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_language(cx);
                                }),
                            )
                            .child(tg_icon_rgba(IconName::Translate, 12.0, text_muted))
                            .child(tr_str(lang, "中文 / EN", "EN / 中文")),
                    )
                    .child(
                        div()
                            .cursor_pointer()
                            .h(px(28.0))
                            .px(px(8.0))
                            .rounded(px(8.0))
                            .bg(if self.settings_section == SettingsSectionType::Search {
                                active_bg
                            } else {
                                ob.bg_control()
                            })
                            .border_1()
                            .border_color(ob.border_hairline())
                            .hover(move |s| s.bg(hover_ctrl))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.navigate_to(SettingsSectionType::Search, false, cx);
                                }),
                            )
                            .child(tg_icon_rgba(IconName::Search, 12.0, text_muted)),
                    ),
            )
    }

    /// Renders OpenBot's 2-column searchable settings frame (`220px` left nav + `48px` top bar
    /// with breadcrumb trail & top-right circular `×` close button + `960px` max centered canvas).
    fn render_settings_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let is_root = self.settings_section.is_root();
        let zh = self.is_chinese_locale;
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.lang_id;
        let title = self.settings_section.display_title_localized(zh);
        let section_icon = self.settings_section.icon_name();
        let hover_bg = ob.hover();
        let text_pri = ob.text_primary();
        let text_sec = ob.text_secondary();
        let text_muted = ob.text_muted();

        let page_layout = super::settings_theme_ui::compute_settings_page_layout(self);
        let crumbs = self.settings_section.breadcrumb_path();

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
                            .border_b_1()
                            .border_color(ob.border_hairline())
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
                                    .when(!is_root || !self.settings_stack.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .cursor_pointer()
                                                .h(px(28.0))
                                                .px(px(8.0))
                                                .rounded(px(8.0))
                                                .hover(move |s| s.bg(hover_bg).text_color(text_pri))
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
                                    })
                                    .child(if crumbs.len() > 1 {
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .children(crumbs.into_iter().enumerate().map(
                                                |(idx, sec)| {
                                                    let is_current = sec == self.settings_section;
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
                                                                .cursor_pointer()
                                                                .hover(move |s| {
                                                                    s.text_color(text_pri)
                                                                })
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    cx.listener(
                                                                        move |this, _, _, cx| {
                                                                            this.navigate_to(
                                                                                sec, false, cx,
                                                                            );
                                                                        },
                                                                    ),
                                                                )
                                                                .child(
                                                                    sec.display_title_localized(zh),
                                                                ),
                                                        )
                                                },
                                            ))
                                            .into_any_element()
                                    } else {
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(tg_icon_rgba(section_icon, 15.0, text_muted))
                                            .child(
                                                div()
                                                    .text_size(px(14.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(text_pri)
                                                    .child(title),
                                            )
                                            .into_any_element()
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
                                    .max_w(px(page_layout.content_max_px))
                                    .mx_auto()
                                    .px(px(24.0))
                                    .py(px(16.0))
                                    .pb(px(32.0))
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
