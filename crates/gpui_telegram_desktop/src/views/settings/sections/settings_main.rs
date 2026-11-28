//! Upstream Source: Telegram/SourceFiles/settings/sections/settings_main.cpp

use crate::i18n::tr_str;
use crate::theme::assets::IconName;
use crate::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};
use crate::views::settings::settings_theme_ui::{
    settings_action_row_desc, settings_container, settings_group_card, settings_section_block,
    settings_static_row,
};
use crate::views::settings::settings_type::SettingsSectionType;
use gpui::prelude::FluentBuilder;
use gpui::*;

#[derive(Debug, Clone)]
pub struct SettingsMainProfile {
    pub name: String,
    pub phone: String,
    pub username: String,
    pub bio: String,
    pub is_verified: bool,
    pub is_premium: bool,
}

#[derive(Debug, Clone)]
pub struct SettingsMainSections {
    pub profile: SettingsMainProfile,
    pub interface_scale_pct: u32,
    pub active_section: Option<SettingsSectionType>,
    pub toast_message: Option<String>,
    pub pending_copy_text: Option<String>,
    pub pending_scale_change: Option<u32>,
    pub pending_language_code: Option<&'static str>,
    pub ai_providers_summary: Option<String>,
    pub needs_provider_selection: bool,
    pub is_visible: bool,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
}

impl SettingsMainSections {
    pub fn new() -> Self {
        Self {
            profile: SettingsMainProfile {
                name: "Pavel Durov".into(),
                phone: "+1 234 567 8900".into(),
                username: "durov".into(),
                bio: "Exploring GPUI and high-performance Rust interfaces".into(),
                is_verified: true,
                is_premium: true,
            },
            interface_scale_pct: 100,
            active_section: None,
            toast_message: None,
            pending_copy_text: None,
            pending_scale_change: None,
            pending_language_code: None,
            ai_providers_summary: None,
            needs_provider_selection: false,
            is_visible: true,
            palette: TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12),
            language_id: "en".into(),
        }
    }

    pub fn new_live() -> Self {
        let mut s = Self::new();
        s.profile = SettingsMainProfile {
            name: String::new(),
            phone: String::new(),
            username: String::new(),
            bio: String::new(),
            is_verified: false,
            is_premium: false,
        };
        s
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.palette = palette;
    }

    pub fn set_language(&mut self, lang: impl Into<String>) {
        self.language_id = lang.into();
    }

    pub fn sync_profile(&mut self, name: String, phone: String, username: String, bio: String, is_verified: bool, is_premium: bool) {
        self.profile = SettingsMainProfile { name, phone, username, bio, is_verified, is_premium };
    }

    pub fn sync_ai_providers_status(&mut self, summary: Option<String>, needs_selection: bool) {
        self.ai_providers_summary = summary;
        self.needs_provider_selection = needs_selection;
    }

    pub fn copy_phone(&mut self) -> String {
        let msg = tr_str(&self.language_id, "手机号已复制到剪贴板", "Phone copied to clipboard").to_string();
        self.toast_message = Some(msg.clone());
        if !self.profile.phone.is_empty() {
            self.pending_copy_text = Some(self.profile.phone.clone());
        }
        msg
    }

    pub fn copy_username(&mut self) -> String {
        let msg = if self.language_id.starts_with("zh") {
            format!("用户名已复制: @{}", self.profile.username)
        } else {
            format!("Username copied: @{}", self.profile.username)
        };
        self.toast_message = Some(msg.clone());
        if !self.profile.username.is_empty() {
            self.pending_copy_text = Some(format!("@{}", self.profile.username));
        }
        msg
    }

    pub fn set_interface_scale(&mut self, scale_pct: u32) {
        self.interface_scale_pct = scale_pct.clamp(100, 300);
        self.pending_scale_change = Some(self.interface_scale_pct);
    }

    pub fn select_language(&mut self, lang_code: &'static str) {
        self.language_id = lang_code.to_string();
        self.pending_language_code = Some(lang_code);
    }

    pub fn navigate_to(&mut self, section: SettingsSectionType) {
        self.active_section = Some(section);
    }

    pub fn toggle_visibility(&mut self) {
        self.is_visible = !self.is_visible;
    }
}

impl Default for SettingsMainSections {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for SettingsMainSections {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible {
            return div().into_any_element();
        }
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.language_id.clone();
        let is_zh = lang.starts_with("zh");
        let hover_ctrl = ob.bg_control_hover();

        let ai_badge = self.ai_providers_summary.clone().unwrap_or_else(|| {
            if self.needs_provider_selection {
                tr_str(&lang, "请选择服务商", "Choose provider").to_string()
            } else {
                tr_str(&lang, "管理", "Manage").to_string()
            }
        });

        let lang_control = div()
            .p(px(2.0))
            .rounded(px(8.0))
            .bg(ob.bg_control())
            .border_1()
            .border_color(ob.border_hairline())
            .flex()
            .items_center()
            .gap(px(2.0))
            .children(
                [("zh-CN", "简体中文", is_zh), ("en-US", "English", !is_zh)]
                    .into_iter()
                    .map(|(code, label, active)| {
                        div()
                            .id(SharedString::from(format!("settings-general-lang-{code}")))
                            .cursor_pointer()
                            .h(px(26.0))
                            .px(px(10.0))
                            .rounded(px(6.0))
                            .when(active, |el| el.bg(ob.surface_light()))
                            .when(!active, |el| el.hover(move |s| s.bg(hover_ctrl)))
                            .flex()
                            .items_center()
                            .text_size(px(12.0))
                            .font_weight(if active {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(if active {
                                ob.text_on_light()
                            } else {
                                ob.text_secondary()
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.select_language(code);
                                    cx.notify();
                                }),
                            )
                            .child(label)
                    }),
            );

        let scale_control = div()
            .p(px(2.0))
            .rounded(px(8.0))
            .bg(ob.bg_control())
            .border_1()
            .border_color(ob.border_hairline())
            .flex()
            .items_center()
            .gap(px(2.0))
            .children([100u32, 125, 150, 200].into_iter().map(|pct| {
                let active = self.interface_scale_pct == pct;
                div()
                    .id(SharedString::from(format!("settings-scale-btn-{pct}")))
                    .cursor_pointer()
                    .h(px(26.0))
                    .px(px(9.0))
                    .rounded(px(6.0))
                    .when(active, |el| el.bg(ob.surface_light()))
                    .when(!active, |el| el.hover(move |s| s.bg(hover_ctrl)))
                    .flex()
                    .items_center()
                    .text_size(px(12.0))
                    .font_weight(if active {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if active {
                        ob.text_on_light()
                    } else {
                        ob.text_secondary()
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_interface_scale(pct);
                            cx.notify();
                        }),
                    )
                    .child(format!("{pct}%"))
            }));

        settings_container(pal)
            .child(settings_section_block(
                tr_str(&lang, "语言与显示", "Language & Display"),
                None,
                pal,
                settings_group_card(
                    pal,
                    vec![
                        settings_static_row(
                            tr_str(&lang, "界面语言", "Language"),
                            Some(tr_str(
                                &lang,
                                "菜单、按钮和提示文字使用的显示语言。",
                                "Language used for menus, buttons, and messages.",
                            )),
                            lang_control,
                            pal,
                        ),
                        settings_static_row(
                            tr_str(&lang, "界面大小", "Interface size"),
                            Some(tr_str(
                                &lang,
                                "调整应用内文字和控件的缩放比例。",
                                "Adjust the scale of text and controls across the app.",
                            )),
                            scale_control,
                            pal,
                        ),
                    ],
                ),
            ))
            .child(settings_section_block(
                tr_str(&lang, "对话", "Conversations"),
                None,
                pal,
                settings_group_card(
                    pal,
                    vec![
                        settings_action_row_desc(
                            Some(IconName::FolderAll),
                            tr_str(&lang, "对话分组", "Chat Folders"),
                            Some(tr_str(
                                &lang,
                                "将不同场景的对话整理到自定义分组中。",
                                "Organize chats into custom tabs and filter groups.",
                            )),
                            tr_str(&lang, "管理", "Manage"),
                            pal,
                            false,
                            |this| {
                                this.navigate_to(SettingsSectionType::Folders);
                            },
                            cx,
                        ),
                        settings_action_row_desc(
                            Some(IconName::NightMoon),
                            tr_str(&lang, "外观与主题", "Appearance & Theme"),
                            Some(tr_str(
                                &lang,
                                "切换深浅色主题、聊天背景、字体大小与发送快捷键。",
                                "Customize color theme, wallpaper, font size, and send key.",
                            )),
                            tr_str(&lang, "自定义", "Customize"),
                            pal,
                            false,
                            |this| {
                                this.navigate_to(SettingsSectionType::Chat);
                            },
                            cx,
                        ),
                    ],
                ),
            ))
            .child(settings_section_block(
                tr_str(&lang, "AI 模型", "AI Models"),
                None,
                pal,
                settings_group_card(
                    pal,
                    vec![settings_action_row_desc(
                        Some(IconName::HubSparkles),
                        tr_str(&lang, "服务商", "Providers"),
                        Some(tr_str(
                            &lang,
                            "配置 AI 订阅、API 密钥与默认模型。",
                            "Configure AI subscriptions, API keys, and default models.",
                        )),
                        &ai_badge,
                        pal,
                        self.needs_provider_selection,
                        |this| {
                            this.navigate_to(SettingsSectionType::AiProviders);
                        },
                        cx,
                    )],
                ),
            ))
            .into_any_element()
    }
}
