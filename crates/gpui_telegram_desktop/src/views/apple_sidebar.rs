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
use crate::views::content_hub::apple_primitives::{creation_kind_accent, creation_kind_label};
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
    CodeScene,
    Books,
    Music,
    Games,
    AppStore,
    MyCreations,
}

impl AppleSidebarSection {
    /// Converts this sidebar destination into a [`ContentHubSection`] (`None` for `Chats`/`Community`/`CodeScene`).
    pub fn to_content_hub_section(self) -> Option<ContentHubSection> {
        match self {
            Self::Chats | Self::Community | Self::CodeScene => None,
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
}

/// Apple HIG primary navigation sidebar entity.
pub struct AppleAppSidebar {
    pub active_section: AppleSidebarSection,
    pub is_collapsed: bool,
    pub creations_drawer_open: bool,
    pub creations_filters_expanded: bool,
    pub active_creation_kind_filter: Option<CreationKind>,
    pub unread_chats_count: u32,
    pub community_unread_count: u32,
    pub creations_count: usize,
    pub pending_review_count: usize,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
}

impl EventEmitter<AppleSidebarEvent> for AppleAppSidebar {}

impl AppleAppSidebar {
    pub fn new(palette: TdesktopThemePalette, language_id: impl Into<String>) -> Self {
        Self {
            active_section: AppleSidebarSection::Chats,
            is_collapsed: false,
            creations_drawer_open: false,
            creations_filters_expanded: true,
            active_creation_kind_filter: None,
            unread_chats_count: 0,
            community_unread_count: 0,
            creations_count: 0,
            pending_review_count: 0,
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
        self.creations_drawer_open = false;
        self.active_creation_kind_filter = None;
        self.unread_chats_count = 0;
        self.community_unread_count = 0;
        self.creations_count = 0;
        self.pending_review_count = 0;
    }

    pub fn select_section(&mut self, section: AppleSidebarSection, cx: &mut Context<Self>) {
        self.active_section = section;
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
        let is_active = self.active_section == section;
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
                    .child(tg_icon(icon, 18.0, icon_color))
                    .when(!collapsed, |el| {
                        el.child(
                            div()
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
        let drawer_open = self.creations_drawer_open;
        let active_filter = self.active_creation_kind_filter;
        let hover_bg = pal.row_hover_rgba();

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
        let creations_badge = (self.creations_count > 0).then(|| self.creations_count.to_string());
        let pending_review_label = (self.pending_review_count > 0).then(|| {
            let count_txt = if self.pending_review_count > 99 {
                "99+".to_string()
            } else {
                self.pending_review_count.to_string()
            };
            format!("{} {}", tr_str(&lang, "待审", "Review"), count_txt)
        });
        let chats_collapsed_dot = (self.unread_chats_count > 0).then_some(pal.unread_badge_bg);
        let community_collapsed_dot =
            (self.community_unread_count > 0).then_some(pal.unread_badge_bg);
        let creations_collapsed_dot = (self.pending_review_count > 0).then_some(0xFF9F0A);

        div()
            .id("apple-app-sidebar")
            .w(px(self.effective_width()))
            .h_full()
            .flex_shrink_0()
            .bg(pal.sidebar_bg_rgba())
            .border_r_1()
            .border_color(pal.separator_rgba())
            .flex()
            .flex_col()
            .child(
                div()
                    .id("apple-sidebar-top-clearance")
                    .w_full()
                    .h(px(TOP_CLEARANCE_HEIGHT))
                    .flex_shrink_0()
                    .px(px(10.0))
                    .window_control_area(WindowControlArea::Drag)
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(
                        div()
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
                            )),
                    ),
            )
            .child(
                div()
                    .flex_1()
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
                        "apple-sidebar-nav-code-scene",
                        AppleSidebarSection::CodeScene,
                        IconName::CodeScene,
                        0xFE2C55,
                        tr_str(&lang, "代码场景", "Code Scene"),
                        None,
                        None,
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
                    ))
                    .child(div().my(px(6.0)).w_full().h(px(1.0)).bg(pal.separator_rgba()))
                    .when(!collapsed, |el| {
                        el.child(
                            div()
                                .px(px(10.0))
                                .pb(px(2.0))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_secondary))
                                        .child(tr_str(&lang, "云端创作", "Cloud Studio")),
                                )
                                .when_some(pending_review_label, |hdr, badge_txt| {
                                    hdr.child(
                                        div()
                                            .id("apple-sidebar-pending-review-badge")
                                            .cursor_pointer()
                                            .px(px(6.0))
                                            .py(px(1.0))
                                            .rounded(px(999.0))
                                            .bg(TdesktopThemePalette::rgba_with_alpha(
                                                0xFF9F0A, 0.18,
                                            ))
                                            .text_size(px(10.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(0xFF9F0A))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.select_section(
                                                        AppleSidebarSection::MyCreations,
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .child(badge_txt),
                                    )
                                }),
                        )
                    })
                    .child(self.render_nav_row(
                        "apple-sidebar-nav-creations",
                        AppleSidebarSection::MyCreations,
                        IconName::HubCreations,
                        0xAF52DE,
                        tr_str(&lang, "我的创作", "My Creations"),
                        creations_badge,
                        creations_collapsed_dot,
                        cx,
                    ))
                    .child(
                        div()
                            .id("apple-sidebar-quick-drawer-toggle")
                            .cursor_pointer()
                            .w_full()
                            .h(px(32.0))
                            .px(px(if collapsed { 0.0 } else { 10.0 }))
                            .rounded(px(8.0))
                            .flex()
                            .items_center()
                            .when(collapsed, |el| el.justify_center())
                            .when(!collapsed, |el| el.justify_between().gap(px(8.0)))
                            .when(drawer_open, |el| {
                                el.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                            })
                            .when(!drawer_open, |el| el.hover(move |s| s.bg(hover_bg)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.toggle_creations_drawer(cx)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(tg_icon(
                                        IconName::SidebarPanel,
                                        16.0,
                                        if drawer_open { pal.accent } else { pal.icon_fg },
                                    ))
                                    .when(!collapsed, |el| {
                                        el.child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(if drawer_open {
                                                    pal.accent
                                                } else {
                                                    pal.text_secondary
                                                }))
                                                .child(tr_str(&lang, "快速作品抽屉", "Quick Drawer")),
                                        )
                                    }),
                            ),
                    )
                    .when(!collapsed && self.creations_filters_expanded, |el| {
                        let kinds: &[(Option<CreationKind>, &'static str)] = &[
                            (None, "apple-sidebar-filter-all"),
                            (Some(CreationKind::Book), "apple-sidebar-filter-book"),
                            (Some(CreationKind::Music), "apple-sidebar-filter-music"),
                            (Some(CreationKind::Game), "apple-sidebar-filter-game"),
                            (Some(CreationKind::App), "apple-sidebar-filter-app"),
                        ];
                        el.child(
                            div()
                                .pt(px(4.0))
                                .px(px(6.0))
                                .flex()
                                .flex_wrap()
                                .gap(px(4.0))
                                .children(kinds.iter().map(|(kind_opt, pill_id)| {
                                    let is_sel = active_filter == *kind_opt;
                                    let k_copy = *kind_opt;
                                    let label = match k_copy {
                                        None => tr_str(&lang, "全部", "All"),
                                        Some(k) => creation_kind_label(k, &lang),
                                    };
                                    let accent =
                                        k_copy.map(creation_kind_accent).unwrap_or(pal.accent);
                                    div()
                                        .id(*pill_id)
                                        .cursor_pointer()
                                        .px(px(7.0))
                                        .py(px(2.0))
                                        .rounded(px(999.0))
                                        .bg(if is_sel {
                                            TdesktopThemePalette::rgba_with_alpha(accent, 0.22)
                                        } else {
                                            pal.row_hover_rgba()
                                        })
                                        .text_size(px(11.0))
                                        .font_weight(if is_sel {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .text_color(rgb(if is_sel {
                                            accent
                                        } else {
                                            pal.text_secondary
                                        }))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.active_creation_kind_filter = k_copy;
                                                cx.emit(
                                                    AppleSidebarEvent::SelectCreationKindFilter(
                                                        k_copy,
                                                    ),
                                                );
                                                cx.notify();
                                            }),
                                        )
                                        .child(label)
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .p(px(10.0))
                    .border_t_1()
                    .border_color(pal.separator_rgba())
                    .child(
                        div()
                            .id("apple-sidebar-creator-bot-cta")
                            .cursor_pointer()
                            .w_full()
                            .h(px(34.0))
                            .px(px(if collapsed { 0.0 } else { 10.0 }))
                            .rounded(px(9.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                            .hover(move |s| {
                                s.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.22))
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(6.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _, _, cx| {
                                    cx.emit(AppleSidebarEvent::OpenCreatorBot(None));
                                }),
                            )
                            .child(tg_icon(IconName::HubSparkles, 16.0, pal.accent))
                            .when(!collapsed, |el| {
                                el.child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.accent))
                                        .child(tr_str(&lang, "Bot 创作", "Creator Bot")),
                                )
                            }),
                    ),
            )
    }
}
