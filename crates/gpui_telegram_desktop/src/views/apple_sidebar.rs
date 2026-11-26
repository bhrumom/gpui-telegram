//! Apple-style primary navigation sidebar (`AppleAppSidebar`) for GPUI Telegram Desktop.
//!
//! Mounted leftmost inside `AppShell::render` with Apple HIG vibrancy surfaces, grouped
//! Discover & Cloud Studio sections, bilingual `zh-CN`/`en` localization, collapsible rail
//! (`204px` expanded / `68px` compact icon rail), and sidebar-adjacent Quick Creations Drawer toggle.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::creations::CreationKind;

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::content_hub::ContentHubSection;

/// Expanded Apple sidebar width in logical pixels.
pub const EXPANDED_WIDTH: f32 = 204.0;
/// Compact icon-only Apple sidebar rail width in logical pixels.
pub const COLLAPSED_WIDTH: f32 = 68.0;
/// Top macOS traffic-light clearance & window drag strip height.
pub const TOP_CLEARANCE_HEIGHT: f32 = 52.0;

/// Primary top-level destinations surfaced in `AppleAppSidebar`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppleSidebarSection {
    Chats,
    Community,
    Books,
    Music,
    Games,
    AppStore,
    MyCreations,
}

impl AppleSidebarSection {
    /// Converts this sidebar destination into a [`ContentHubSection`] (`None` for `Chats`/`Community`).
    pub fn to_content_hub_section(self) -> Option<ContentHubSection> {
        match self {
            Self::Chats | Self::Community => None,
            Self::Books => Some(ContentHubSection::Books),
            Self::Music => Some(ContentHubSection::Music),
            Self::Games => Some(ContentHubSection::Games),
            Self::AppStore => Some(ContentHubSection::AppStore),
            Self::MyCreations => Some(ContentHubSection::MyCreations),
        }
    }

    /// Maps a [`ContentHubSection`] back to its corresponding [`AppleSidebarSection`].
    pub fn from_content_hub_section(section: ContentHubSection) -> Self {
        match section {
            ContentHubSection::Books => Self::Books,
            ContentHubSection::Music => Self::Music,
            ContentHubSection::Games => Self::Games,
            ContentHubSection::AppStore => Self::AppStore,
            ContentHubSection::MyCreations => Self::MyCreations,
        }
    }
}

/// Events emitted by [`AppleAppSidebar`] to [`crate::views::AppShell`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppleSidebarEvent {
    SelectSection(AppleSidebarSection),
    ToggleCreationsDrawer(bool),
    SelectCreationKindFilter(Option<CreationKind>),
    ToggleCollapsed(bool),
    OpenCreatorBot(Option<CreationKind>),
    OpenAccountMenu,
    OpenSettings,
}

/// Apple HIG primary navigation sidebar entity.
pub struct AppleAppSidebar {
    pub active_section: AppleSidebarSection,
    pub highlight_active_section: bool,
    pub settings_active: bool,
    pub is_collapsed: bool,
    pub creations_drawer_open: bool,
    pub creations_filters_expanded: bool,
    pub active_creation_kind_filter: Option<CreationKind>,
    pub unread_chats_count: u32,
    pub community_unread_count: u32,
    pub creations_count: usize,
    pub pending_review_count: usize,
    pub account_name: String,
    pub account_initials: String,
    pub is_connected: bool,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
}

impl EventEmitter<AppleSidebarEvent> for AppleAppSidebar {}

impl AppleAppSidebar {
    pub fn new(palette: TdesktopThemePalette, language_id: impl Into<String>) -> Self {
        Self {
            active_section: AppleSidebarSection::Chats,
            highlight_active_section: true,
            settings_active: false,
            is_collapsed: false,
            creations_drawer_open: false,
            creations_filters_expanded: false,
            active_creation_kind_filter: None,
            unread_chats_count: 0,
            community_unread_count: 0,
            creations_count: 0,
            pending_review_count: 0,
            account_name: String::new(),
            account_initials: String::new(),
            is_connected: false,
            palette,
            language_id: language_id.into(),
        }
    }

    pub fn effective_width(&self) -> f32 {
        if self.is_collapsed {
            COLLAPSED_WIDTH
        } else {
            EXPANDED_WIDTH
        }
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) {
        self.palette = palette;
    }

    pub fn set_language(&mut self, lang_id: impl Into<String>) {
        self.language_id = lang_id.into();
    }

    pub fn set_active_section(&mut self, section: AppleSidebarSection) {
        self.active_section = section;
        self.highlight_active_section = true;
    }

    pub fn set_highlight_active_section(&mut self, highlight: bool) {
        self.highlight_active_section = highlight;
    }

    pub fn set_settings_active(&mut self, active: bool) {
        self.settings_active = active;
    }

    pub fn set_account_info(
        &mut self,
        name: impl Into<String>,
        initials: impl Into<String>,
        is_connected: bool,
    ) {
        self.account_name = name.into();
        self.account_initials = initials.into();
        self.is_connected = is_connected;
    }

    pub fn set_collapsed(&mut self, collapsed: bool) {
        self.is_collapsed = collapsed;
    }

    pub fn set_creations_drawer_open(&mut self, open: bool) {
        self.creations_drawer_open = open;
    }

    pub fn set_creation_kind_filter(&mut self, filter: Option<CreationKind>) {
        self.active_creation_kind_filter = filter;
    }

    pub fn set_badges(
        &mut self,
        unread_chats_count: u32,
        creations_count: usize,
        pending_review_count: usize,
    ) {
        self.unread_chats_count = unread_chats_count;
        self.creations_count = creations_count;
        self.pending_review_count = pending_review_count;
    }

    pub fn set_community_unread(&mut self, count: u32) {
        self.community_unread_count = count;
    }

    pub fn reset_for_logout_or_account_switch(&mut self) {
        self.active_section = AppleSidebarSection::Chats;
        self.highlight_active_section = true;
        self.settings_active = false;
        self.creations_drawer_open = false;
        self.active_creation_kind_filter = None;
        self.unread_chats_count = 0;
        self.community_unread_count = 0;
        self.creations_count = 0;
        self.pending_review_count = 0;
        self.account_name.clear();
        self.account_initials.clear();
        self.is_connected = false;
    }

    pub fn select_section(&mut self, section: AppleSidebarSection, cx: &mut Context<Self>) {
        self.active_section = section;
        self.highlight_active_section = true;
        cx.emit(AppleSidebarEvent::SelectSection(section));
        cx.notify();
    }

    pub fn toggle_collapsed(&mut self, cx: &mut Context<Self>) {
        self.is_collapsed = !self.is_collapsed;
        cx.emit(AppleSidebarEvent::ToggleCollapsed(self.is_collapsed));
        cx.notify();
    }

    pub fn toggle_creations_drawer(&mut self, cx: &mut Context<Self>) {
        self.creations_drawer_open = !self.creations_drawer_open;
        cx.emit(AppleSidebarEvent::ToggleCreationsDrawer(
            self.creations_drawer_open,
        ));
        cx.notify();
    }
}

impl AppleAppSidebar {
    fn render_nav_row(
        &self,
        id: &'static str,
        section: AppleSidebarSection,
        icon: IconName,
        accent: u32,
        label: &'static str,
        badge: Option<String>,
        collapsed_dot_color: Option<u32>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let pal = self.palette;
        let is_active = self.highlight_active_section && self.active_section == section;
        let collapsed = self.is_collapsed;
        let active_bg = pal.row_active_rgba();
        let hover_bg = pal.row_hover_rgba();
        let icon_color = if is_active {
            accent
        } else {
            pal.icon_fg
        };
        let label_color = if is_active {
            pal.text_primary
        } else {
            pal.text_secondary
        };

        div()
            .id(id)
            .relative()
            .cursor_pointer()
            .w_full()
            .h(px(34.0))
            .px(px(if collapsed { 0.0 } else { 10.0 }))
            .rounded(px(8.0))
            .flex()
            .items_center()
            .when(collapsed, |el| el.justify_center())
            .when(!collapsed, |el| el.justify_between().gap(px(8.0)))
            .when(is_active, |el| el.bg(active_bg))
            .when(!is_active, |el| el.hover(move |s| s.bg(hover_bg)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.select_section(section, cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .min_w(px(0.0))
                    .child(tg_icon(icon, 18.0, icon_color))
                    .when(!collapsed, |el| {
                        el.child(
                            div()
                                .truncate()
                                .text_size(px(13.0))
                                .font_weight(if is_active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(rgb(label_color))
                                .child(label),
                        )
                    }),
            )
            .when(!collapsed, |el| {
                el.when_some(badge, |b_el, badge_txt| {
                    b_el.child(
                        div()
                            .flex_shrink_0()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(999.0))
                            .bg(if matches!(
                                section,
                                AppleSidebarSection::Chats | AppleSidebarSection::Community
                            ) {
                                rgb(pal.unread_badge_bg)
                            } else {
                                TdesktopThemePalette::rgba_with_alpha(accent, 0.16)
                            })
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if matches!(
                                section,
                                AppleSidebarSection::Chats | AppleSidebarSection::Community
                            ) {
                                pal.unread_badge_fg
                            } else {
                                accent
                            }))
                            .child(badge_txt),
                    )
                })
            })
            .when(collapsed, |el| {
                el.when_some(collapsed_dot_color, |d_el, col| {
                    d_el.child(
                        div()
                            .absolute()
                            .top(px(5.0))
                            .right(px(8.0))
                            .w(px(7.0))
                            .h(px(7.0))
                            .rounded(px(999.0))
                            .bg(rgb(col)),
                    )
                })
            })
    }
}

impl Render for AppleAppSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let lang = self.language_id.clone();
        let collapsed = self.is_collapsed;
        let settings_active = self.settings_active;
        let hover_bg = pal.row_hover_rgba();
        let active_bg = TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14);

        let unread_badge = (self.unread_chats_count > 0).then(|| {
            if self.unread_chats_count > 99 {
                "99+".to_string()
            } else {
                self.unread_chats_count.to_string()
            }
        });
        let community_badge = (self.community_unread_count > 0).then(|| {
            if self.community_unread_count > 999 {
                "999+".to_string()
            } else {
                self.community_unread_count.to_string()
            }
        });
        let chats_collapsed_dot = (self.unread_chats_count > 0).then_some(pal.unread_badge_bg);
        let community_collapsed_dot =
            (self.community_unread_count > 0).then_some(pal.unread_badge_bg);

        let avatar_grad = pal.peer_avatar_gradient(1);
        let display_name = if self.account_name.trim().is_empty() {
            tr_str(&lang, "我的账号", "Account").to_string()
        } else {
            self.account_name.clone()
        };
        let initials = if self.account_initials.trim().is_empty() {
            display_name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "U".to_string())
        } else {
            self.account_initials.clone()
        };

        let collapse_btn = div()
            .id("apple-sidebar-collapse-btn")
            .cursor_pointer()
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(6.0))
            .hover(move |s| s.bg(hover_bg))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.toggle_collapsed(cx)),
            )
            .child(tg_icon(
                if collapsed {
                    IconName::SidebarLeftOpen
                } else {
                    IconName::SidebarLeftClose
                },
                16.0,
                pal.icon_fg,
            ));

        div()
            .id("apple-app-sidebar")
            .w(px(self.effective_width()))
            .h_full()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(pal.sidebar_bg_rgba())
            .border_r_1()
            .border_color(pal.separator_rgba())
            .flex()
            .flex_col()
            .child(if cfg!(target_os = "macos") && collapsed {
                // Keep macOS traffic-light strip clear of interactive buttons when collapsed to 68px
                div()
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("apple-sidebar-top-clearance")
                            .w_full()
                            .h(px(38.0))
                            .flex_shrink_0()
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(34.0))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(collapse_btn),
                    )
                    .into_any_element()
            } else {
                let top_h = if cfg!(target_os = "macos") {
                    TOP_CLEARANCE_HEIGHT
                } else {
                    44.0
                };
                div()
                    .id("apple-sidebar-top-clearance")
                    .w_full()
                    .h(px(top_h))
                    .flex_shrink_0()
                    .px(px(10.0))
                    .window_control_area(WindowControlArea::Drag)
                    .flex()
                    .items_center()
                    .when(collapsed, |row| row.justify_center())
                    .when(!collapsed, |row| row.justify_end())
                    .child(collapse_btn)
                    .into_any_element()
            })
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .px(px(10.0))
                    .py(px(6.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .overflow_hidden()
                    .when(!collapsed, |el| {
                        el.child(
                            div()
                                .px(px(10.0))
                                .pb(px(2.0))
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_secondary))
                                .child(tr_str(&lang, "工作区与探索", "Workspace & Discover")),
                        )
                    })
                    .child(self.render_nav_row(
                        "apple-sidebar-nav-chats",
                        AppleSidebarSection::Chats,
                        IconName::FolderChat,
                        pal.accent,
                        tr_str(&lang, "消息", "Chats"),
                        unread_badge,
                        chats_collapsed_dot,
                        cx,
                    ))
                    .child(self.render_nav_row(
                        "apple-sidebar-nav-community",
                        AppleSidebarSection::Community,
                        IconName::CommunityGlobe,
                        pal.accent,
                        tr_str(&lang, "社区", "Community"),
                        community_badge,
                        community_collapsed_dot,
                        cx,
                    ))
                    .child(self.render_nav_row(
                        "apple-sidebar-nav-books",
                        AppleSidebarSection::Books,
                        IconName::HubBooks,
                        0xFF9500,
                        tr_str(&lang, "图书", "Books"),
                        None,
                        None,
                        cx,
                    ))
                    .child(self.render_nav_row(
                        "apple-sidebar-nav-music",
                        AppleSidebarSection::Music,
                        IconName::HubMusic,
                        0xFA2D48,
                        tr_str(&lang, "音乐", "Music"),
                        None,
                        None,
                        cx,
                    ))
                    .child(self.render_nav_row(
                        "apple-sidebar-nav-games",
                        AppleSidebarSection::Games,
                        IconName::HubGames,
                        0x5856D6,
                        tr_str(&lang, "游戏", "Games"),
                        None,
                        None,
                        cx,
                    ))
                    .child(self.render_nav_row(
                        "apple-sidebar-nav-appstore",
                        AppleSidebarSection::AppStore,
                        IconName::HubAppStore,
                        0x007AFF,
                        "App Store",
                        None,
                        None,
                        cx,
                    )),
            )
            // Persistent Apple-HIG Account & Settings footer (always reachable across every section)
            .child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .px(px(10.0))
                    .py(px(8.0))
                    .border_t_1()
                    .border_color(pal.separator_rgba())
                    .flex()
                    .when(collapsed, |el| el.flex_col().items_center().gap(px(6.0)))
                    .when(!collapsed, |el| el.items_center().justify_between().gap(px(6.0)))
                    .child(
                        div()
                            .id("apple-sidebar-account-btn")
                            .cursor_pointer()
                            .when(!collapsed, |el| el.flex_1().min_w(px(0.0)).px(px(6.0)).py(px(4.0)))
                            .when(collapsed, |el| el.w(px(32.0)).h(px(32.0)).justify_center())
                            .rounded(px(8.0))
                            .hover(move |s| s.bg(hover_bg))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _, _, cx| {
                                    cx.emit(AppleSidebarEvent::OpenAccountMenu);
                                }),
                            )
                            .child(
                                div()
                                    .relative()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(rgb(avatar_grad.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xFFFFFF))
                                    .child(initials)
                                    .child(
                                        div()
                                            .absolute()
                                            .bottom(px(-1.0))
                                            .right(px(-1.0))
                                            .w(px(7.5))
                                            .h(px(7.5))
                                            .rounded_full()
                                            .bg(if self.is_connected {
                                                rgb(pal.online_dot_fg)
                                            } else {
                                                rgb(pal.muted_badge_bg)
                                            })
                                            .border_1()
                                            .border_color(rgb(pal.dialogs_bg)),
                                    ),
                            )
                            .when(!collapsed, |el| {
                                el.child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(pal.text_primary))
                                        .child(display_name),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("apple-sidebar-settings-btn")
                            .cursor_pointer()
                            .flex_shrink_0()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(7.0))
                            .when(settings_active, |el| el.bg(active_bg))
                            .hover(move |s| if settings_active { s } else { s.bg(hover_bg) })
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _, _, cx| {
                                    cx.emit(AppleSidebarEvent::OpenSettings);
                                }),
                            )
                            .child(tg_icon(
                                IconName::SettingsGear,
                                15.0,
                                if settings_active {
                                    pal.accent
                                } else {
                                    pal.icon_fg
                                },
                            )),
                    ),
            )
    }
}
