use gpui::*;
use gpui::prelude::FluentBuilder;
use crate::i18n::tr_str;
use crate::theme::assets::{tg_icon_rgba, IconName};
use crate::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};
use crate::views::settings::settings_theme_ui::{
    settings_card, settings_container, settings_empty_state, settings_group_card,
    settings_section_block, settings_toggle_row,
};

/// Upstream counterpart: `Settings::Folders` (`settings_folders.cpp`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatFolderItem {
    pub id: i32,
    pub title: String,
    pub icon_name: String,
    pub included_peers_count: usize,
    pub excluded_peers_count: usize,
    pub unread_count: usize,
    pub is_removed: bool,
}

pub struct SettingsFoldersSections {
    pub folders: Vec<ChatFolderItem>,
    pub tags_enabled: bool,
    pub selected_folder_id: Option<i32>,
    pub is_visible: bool,
    pub context_description: String,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub pending_create_folder: bool,
    pub new_folder_title: String,
    pub is_editing_title: bool,
    pub pending_created_folder: Option<(i32, String)>,
    pub pending_deleted_folder_id: Option<i32>,
    pub pending_prefs_sync: bool,
}

impl SettingsFoldersSections {
    pub fn new() -> Self {
        Self {
            folders: vec![
                ChatFolderItem { id: 1, title: "Unread".into(), icon_name: "folder_unread".into(), included_peers_count: 15, excluded_peers_count: 2, unread_count: 15, is_removed: false },
                ChatFolderItem { id: 2, title: "Work".into(), icon_name: "folder_work".into(), included_peers_count: 8, excluded_peers_count: 0, unread_count: 3, is_removed: false },
                ChatFolderItem { id: 3, title: "Personal".into(), icon_name: "folder_personal".into(), included_peers_count: 12, excluded_peers_count: 1, unread_count: 0, is_removed: false },
            ],
            tags_enabled: false,
            selected_folder_id: Some(1),
            is_visible: false,
            context_description: "Create folders for different groups of chats and quickly switch between them.".into(),
            palette: TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x3390ec, 14, 12),
            language_id: "en".into(),
            pending_create_folder: false,
            new_folder_title: String::new(),
            is_editing_title: false,
            pending_created_folder: None,
            pending_deleted_folder_id: None,
            pending_prefs_sync: false,
        }
    }

    pub fn new_live(palette: TdesktopThemePalette, language_id: &str) -> Self {
        Self {
            folders: Vec::new(),
            tags_enabled: false,
            selected_folder_id: None,
            is_visible: true,
            context_description: tr_str(language_id, "为不同聊天分组创建文件夹，并在它们之间快速切换。", "Create folders for different groups of chats and quickly switch between them.").to_string(),
            palette,
            language_id: language_id.to_string(),
            pending_create_folder: false,
            new_folder_title: String::new(),
            is_editing_title: false,
            pending_created_folder: None,
            pending_deleted_folder_id: None,
            pending_prefs_sync: false,
        }
    }

    pub fn sync_folders(&mut self, folders: Vec<ChatFolderItem>) {
        self.folders = folders;
        if self.selected_folder_id.is_none() {
            self.selected_folder_id = self.folders.iter().find(|f| !f.is_removed).map(|f| f.id);
        }
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) { self.palette = palette; }
    pub fn set_language(&mut self, language_id: impl Into<String>) {
        let lang = language_id.into();
        self.context_description = tr_str(
            &lang,
            "为不同聊天分组创建文件夹，并在它们之间快速切换。",
            "Create folders for different groups of chats and quickly switch between them.",
        )
        .to_string();
        self.language_id = lang;
    }
    pub fn toggle_visibility(&mut self) { self.is_visible = !self.is_visible; }
    pub fn text_context(&self) -> &str { &self.context_description }

    pub fn set_removed(&mut self, folder_id: i32) {
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == folder_id) { f.is_removed = true; }
    }

    pub fn find(&self, folder_id: i32) -> Option<&ChatFolderItem> {
        self.folders.iter().find(|f| f.id == folder_id && !f.is_removed)
    }

    pub fn lng_filters_chats_count(&self, folder_id: i32) -> usize {
        self.find(folder_id).map(|f| f.included_peers_count).unwrap_or(0)
    }

    pub fn lng_filters_no_chats(&self, folder_id: i32) -> bool { self.lng_filters_chats_count(folder_id) == 0 }

    pub fn update_data(&mut self, folder_id: i32, new_title: String) {
        if let Some(folder) = self.folders.iter_mut().find(|f| f.id == folder_id) { folder.title = new_title; }
    }
}

impl Default for SettingsFoldersSections {
    fn default() -> Self { Self::new() }
}

impl Render for SettingsFoldersSections {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.language_id.clone();
        let active: Vec<ChatFolderItem> = self.folders.iter().filter(|f| !f.is_removed).cloned().collect();
        let sel = self.selected_folder_id;
        let hover_bg = ob.hover();

        let is_editing_title = self.is_editing_title;
        let draft_title = self.new_folder_title.clone();

        let create_card = settings_card(pal)
            .py(px(14.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(36.0))
                            .px(px(12.0))
                            .py(px(7.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_control())
                            .border_1()
                            .border_color(if is_editing_title { ob.text_primary() } else { ob.border_hairline() })
                            .cursor_text()
                            .flex()
                            .items_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.is_editing_title = true;
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(if draft_title.is_empty() && !is_editing_title {
                                        ob.text_muted()
                                    } else {
                                        ob.text_primary()
                                    })
                                    .child(if draft_title.is_empty() {
                                        if is_editing_title {
                                            "|".to_string()
                                        } else {
                                            tr_str(&lang, "输入文件夹名称…", "Folder name...").to_string()
                                        }
                                    } else if is_editing_title {
                                        format!("{}|", draft_title)
                                    } else {
                                        draft_title
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .h(px(36.0))
                            .px(px(14.0))
                            .rounded(px(8.0))
                            .bg(ob.surface_light())
                            .flex()
                            .items_center()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ob.text_on_light())
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                let next_id = this.folders.iter().map(|f| f.id).max().unwrap_or(0) + 1;
                                let title = if this.new_folder_title.trim().is_empty() {
                                    if this.language_id.starts_with("zh") {
                                        format!("分组 {}", next_id)
                                    } else {
                                        format!("Folder {}", next_id)
                                    }
                                } else {
                                    this.new_folder_title.trim().to_string()
                                };
                                this.folders.push(ChatFolderItem { id: next_id, title: title.clone(), icon_name: "folder".into(), included_peers_count: 0, excluded_peers_count: 0, unread_count: 0, is_removed: false });
                                this.selected_folder_id = Some(next_id);
                                this.new_folder_title.clear();
                                this.is_editing_title = false;
                                this.pending_create_folder = true;
                                this.pending_created_folder = Some((next_id, title));
                                cx.notify();
                            }))
                            .child(format!("+ {}", tr_str(&lang, "创建", "Create"))),
                    ),
            );

        let folder_list_el: AnyElement = if active.is_empty() {
            settings_card(pal)
                .child(settings_empty_state(
                    IconName::FolderAll,
                    tr_str(&lang, "尚未创建任何分组", "No custom folders configured yet."),
                    pal,
                ))
                .into_any_element()
        } else {
            let rows: Vec<AnyElement> = active
                .into_iter()
                .map(|folder| {
                    let fid = folder.id;
                    let is_sel = sel == Some(fid);
                    div()
                        .w_full()
                        .min_h(px(54.0))
                        .px(px(16.0))
                        .py(px(10.0))
                        .when(is_sel, |el| el.bg(ob.bg_control_hover()))
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .flex_row()
                        .justify_between()
                        .items_center()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| { this.selected_folder_id = Some(fid); cx.notify(); }))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(10.0))
                                .child(tg_icon_rgba(IconName::FolderChat, 15.0, ob.text_muted()))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(1.0))
                                        .child(div().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).text_color(ob.text_primary()).child(folder.title))
                                        .child(div().text_size(px(12.0)).text_color(ob.text_muted()).child(if lang.starts_with("zh") {
                                            format!("{} 个对话", folder.included_peers_count)
                                        } else {
                                            format!("{} chats", folder.included_peers_count)
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .when(folder.unread_count > 0, |el| {
                                    el.child(
                                        div()
                                            .px(px(7.0))
                                            .py(px(2.0))
                                            .rounded_full()
                                            .bg(ob.bg_control_active())
                                            .text_size(px(11.5))
                                            .text_color(ob.text_primary())
                                            .child(format!("{}", folder.unread_count)),
                                    )
                                })
                                .child(
                                    div()
                                        .p(px(5.0))
                                        .rounded(px(6.0))
                                        .hover(move |s| s.bg(hover_bg))
                                        .cursor_pointer()
                                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                            this.set_removed(fid);
                                            this.pending_deleted_folder_id = Some(fid);
                                            cx.notify();
                                        }))
                                        .child(tg_icon_rgba(IconName::DeleteTrash, 14.0, ob.danger())),
                                ),
                        )
                        .into_any_element()
                })
                .collect();
            settings_group_card(pal, rows)
        };

        settings_container(pal)
            .child(settings_section_block(tr_str(&lang, "新建分组", "New Folder"), None, pal, create_card))
            .child(settings_section_block(tr_str(&lang, "我的分组", "Your Folders"), None, pal, folder_list_el))
            .child(settings_section_block(
                tr_str(&lang, "显示选项", "Display"),
                None,
                pal,
                settings_group_card(
                    pal,
                    vec![settings_toggle_row(
                        IconName::FolderWork,
                        tr_str(&lang, "显示分组标签", "Show folder tags"),
                        Some(tr_str(&lang, "在对话列表每行显示所属分组标签。", "Display folder badges on chat list rows.")),
                        self.tags_enabled,
                        pal,
                        |this| {
                            this.tags_enabled = !this.tags_enabled;
                            this.pending_prefs_sync = true;
                        },
                        cx,
                    )],
                ),
            ))
    }
}
