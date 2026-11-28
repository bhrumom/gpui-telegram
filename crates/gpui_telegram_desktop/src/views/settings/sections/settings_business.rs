use crate::i18n::tr_str;
use crate::theme::assets::IconName;
use crate::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};
use crate::views::settings::settings_theme_ui::{
    settings_action_row_desc, settings_container, settings_group_card, settings_section_block,
};
use gpui::prelude::FluentBuilder;
use gpui::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusinessToolItem {
    pub title: String,
    pub subtitle: String,
    pub is_configured: bool,
}

pub struct SettingsBusinessSections {
    pub business_active: bool,
    pub tools: Vec<BusinessToolItem>,
    pub is_visible: bool,
    pub pending_sync: bool,
    pub status_message: Option<String>,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub selected_tool_index: Option<usize>,
}

impl SettingsBusinessSections {
    pub fn new() -> Self {
        Self {
            business_active: true,
            tools: vec![
                BusinessToolItem { title: "Opening Hours".into(), subtitle: "Show your business schedule".into(), is_configured: false },
                BusinessToolItem { title: "Location".into(), subtitle: "Display business address".into(), is_configured: false },
                BusinessToolItem { title: "Quick Replies".into(), subtitle: "Shortcuts for frequently used messages".into(), is_configured: false },
                BusinessToolItem { title: "Greeting Messages".into(), subtitle: "Automatic welcome for new chats".into(), is_configured: false },
                BusinessToolItem { title: "Away Messages".into(), subtitle: "Auto-reply when closed".into(), is_configured: false },
                BusinessToolItem { title: "Chatbots".into(), subtitle: "Connect bots to handle messages".into(), is_configured: false },
            ],
            is_visible: false,
            pending_sync: false,
            status_message: None,
            palette: TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x3390ec, 14, 12),
            language_id: "en".into(),
            selected_tool_index: None,
        }
    }

    pub fn new_live(business_active: bool, palette: TdesktopThemePalette, language_id: &str) -> Self {
        let mut s = Self::new();
        s.business_active = business_active;
        s.palette = palette;
        s.language_id = language_id.into();
        s.is_visible = true;
        s
    }

    pub fn sync_configured_tools(&mut self, configured_titles: &[String]) {
        for tool in &mut self.tools {
            tool.is_configured = configured_titles.iter().any(|t| t == &tool.title);
        }
    }

    pub fn configured_tool_titles(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter(|t| t.is_configured)
            .map(|t| t.title.clone())
            .collect()
    }

    pub fn toggle_tool(&mut self, idx: usize) {
        self.selected_tool_index = Some(idx);
        if let Some(t) = self.tools.get_mut(idx) {
            t.is_configured = !t.is_configured;
            let (zh_title, _) = Self::localized_tool(t, &self.language_id);
            let enabled = t.is_configured;
            self.pending_sync = true;
            self.status_message = Some(if self.language_id.starts_with("zh") {
                format!("{}: {}", zh_title, if enabled { "已启用" } else { "已关闭" })
            } else {
                format!("{}: {}", t.title, if enabled { "Configured" } else { "Disabled" })
            });
        }
    }

    fn localized_tool<'a>(item: &'a BusinessToolItem, lang: &str) -> (&'a str, &'a str) {
        if !lang.starts_with("zh") {
            return (&item.title, &item.subtitle);
        }
        match item.title.as_str() {
            "Opening Hours" => ("营业时间", "在个人主页展示每周营业时段"),
            "Location" => ("店铺位置", "在资料页显示精确地图与地址"),
            "Quick Replies" => ("快捷回复", "通过 / 命令一键发送预设模板"),
            "Greeting Messages" => ("欢迎语自动回复", "首次收到私信时自动发送问候"),
            "Away Messages" => ("离线自动回复", "非营业时段或离线时自动回复"),
            "Chatbots" => ("接入聊天机器人", "授权第三方机器人代管客户会话"),
            _ => (&item.title, &item.subtitle),
        }
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) { self.palette = palette; }
    pub fn set_language(&mut self, language_id: impl Into<String>) { self.language_id = language_id.into(); }
    pub fn toggle_visibility(&mut self) { self.is_visible = !self.is_visible; }
    pub fn lng_business_subtitle_chatbots(&self) -> &'static str { "Automate replies via connected bots" }
    pub fn lng_business_about_opening_hours(&self) -> &'static str { "Set working hours for each day of the week" }
    pub fn lng_business_about_chat_intro(&self) -> &'static str { "Customize the welcome screen before a chat starts" }
    pub fn lng_business_subtitle_chat_links(&self) -> &'static str { "Create t.me links with pre-filled messages" }
    pub fn show_business(&mut self) { self.business_active = true; }
    pub fn lng_business_about_quick_replies(&self) -> &'static str { "Send saved templates using / commands" }
}

impl Default for SettingsBusinessSections {
    fn default() -> Self { Self::new() }
}

impl Render for SettingsBusinessSections {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.language_id.clone();
        let tools = self.tools.clone();

        let rows: Vec<AnyElement> = tools
            .into_iter()
            .enumerate()
            .map(|(idx, item)| {
                let active = item.is_configured;
                let (disp_title, disp_sub) = Self::localized_tool(&item, &lang);
                let status = if active {
                    tr_str(&lang, "已启用", "Configured")
                } else {
                    tr_str(&lang, "去配置", "Set up")
                };
                settings_action_row_desc(
                    Some(IconName::BotRobot),
                    disp_title,
                    Some(disp_sub),
                    status,
                    pal,
                    false,
                    move |this| {
                        this.toggle_tool(idx);
                    },
                    cx,
                )
            })
            .collect();

        settings_container(pal)
            .when_some(self.status_message.clone(), |el, msg| {
                el.child(
                    div()
                        .px(px(4.0))
                        .text_size(px(pal.scaled_px(12.0)))
                        .text_color(ob.success())
                        .child(msg),
                )
            })
            .child(settings_section_block(
                tr_str(&lang, "商业工具", "Business Tools"),
                Some(tr_str(
                    &lang,
                    "商业账号专属自动化客服与名片工具",
                    "Tools and automation for business accounts",
                )),
                pal,
                settings_group_card(pal, rows),
            ))
            .into_any_element()
    }
}
