//! 1:1 OpenBot Reorderable `WorkspaceSidebar` GPUI `0.2.2` renderer (`OB-F14`).
//!
//! Mirrors `packages/ui/src/features/sidebar/{Sidebar.tsx, SidebarTopbar.tsx, SidebarSearch.tsx, SidebarNav.tsx}`:
//! - Supports expanded (`[128, 400]px`, default `280px`), compact avatar-only (`88px`), and closed (`0px`) modes
//! - `SidebarTopbar`: `ServerMenu` dropdown, History Back/Forward, Update badge, Collapse, Marketplace
//! - `SidebarSearch`: filter input + filter chips (`Unread`, `Waiting`, `Working`, `Muted`, `By section`) + `+` New menu
//! - Exact 7-group vertical `SidebarNav`:
//!   1. `SidebarPinnedGroup` (`56px` pinned avatar/channel tiles)
//!   2. `SidebarWaitingGroup` (amber wait chip + felt waiting badge)
//!   3. `SidebarUnreadGroup` (`render_sidebar_felt_badge` lilac pill)
//!   4. `SidebarSectionList` (nested folders up to depth `3` + People + Unassigned + drag-drop indicators + inline threads)
//!   5. `SidebarHiddenGroup` (asleep agents with `AvatarMood::Asleep` and `0.6` custom image opacity)
//!   6. `ArchivedChannelsGroup` (with Restore pill)
//!   7. Dismissible bottom cards (`UpdateIsland`, `Storage`, `Machine`, `MobileApp`, `Community`)

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::agents::AgentSummary;
use openbot_contracts::avatar::{AvatarHue, AvatarMood};
use openbot_contracts::channels::{ChannelIcon, ChannelMember, ChannelSummary};
use serde::{Deserialize, Serialize};

use crate::components::bloub_avatar::{
    AgentAvatarProps, ChannelAgentBrief, ChannelAvatarLayout, SidebarFeltBadge,
    SidebarPlushRingKind, SidebarRoutinePhase, SidebarWaitReason, render_agent_avatar,
    render_channel_avatar, render_sidebar_felt_badge,
};
use crate::theme::openbot_palette::OpenBotPalette;
use crate::theme::{IconName, tg_icon_rgba};

use super::shell_state::{
    SIDEBAR_PEOPLE_SECTION_ID, SIDEBAR_PINNED_TILE_PX, SIDEBAR_UNASSIGNED_SECTION_ID,
    SidebarBottomCardKind, SidebarFilter, SidebarLayoutSnapshot, SidebarPinKind, SidebarState,
};

fn pal(palette: &OpenBotPalette, token: &str, fallback: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback))
}

/// Inline child side-thread row shown beneath an agent in `SidebarAgentThreads`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarAgentThreadItem {
    pub id: String,
    pub title: String,
    pub running: bool,
    pub unread_count: u64,
}

/// Render-ready agent row item for `WorkspaceSidebar`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarAgentItem {
    pub id: String,
    pub name: String,
    pub title: String,
    pub preview: String,
    pub avatar_seed: String,
    pub avatar_hue: Option<AvatarHue>,
    pub avatar_color: Option<String>,
    pub avatar_url: Option<String>,
    pub mood: AvatarMood,
    pub unread_count: u64,
    pub waiting_reason: Option<SidebarWaitReason>,
    pub waiting_summary: Option<String>,
    pub routine_phase: Option<SidebarRoutinePhase>,
    pub working: bool,
    pub usage_limited: bool,
    pub muted: bool,
    pub hidden_since: Option<String>,
    pub is_person_dm: bool,
    pub threads: Vec<SidebarAgentThreadItem>,
}

impl SidebarAgentItem {
    pub fn from_agent_summary(summary: &AgentSummary) -> Self {
        let is_hidden = summary.hidden_at.is_some();
        Self {
            id: summary.id.clone(),
            name: summary.name.clone(),
            title: summary.title.clone(),
            preview: summary.preview.clone(),
            avatar_seed: summary.avatar_seed.clone(),
            avatar_hue: summary.avatar_hue,
            avatar_color: summary.avatar_color.clone().flatten(),
            avatar_url: summary.avatar_url.clone(),
            mood: if is_hidden {
                AvatarMood::Asleep
            } else {
                AvatarMood::Idle
            },
            unread_count: 0,
            waiting_reason: None,
            waiting_summary: None,
            routine_phase: None,
            working: false,
            usage_limited: summary.usage_limit_return.is_some(),
            muted: !summary.notifications,
            hidden_since: summary.hidden_at.clone(),
            is_person_dm: false,
            threads: Vec::new(),
        }
    }

    pub fn is_waiting(&self) -> bool {
        self.waiting_reason.is_some() || self.usage_limited
    }

    pub fn effective_badge(&self) -> Option<SidebarFeltBadge> {
        if let Some(reason) = self.waiting_reason {
            return Some(SidebarFeltBadge::Waiting { reason });
        }
        if self.usage_limited {
            return Some(SidebarFeltBadge::Limited);
        }
        if let Some(phase) = self.routine_phase {
            return Some(SidebarFeltBadge::Routine { phase });
        }
        if self.unread_count > 0 {
            return Some(SidebarFeltBadge::Unread {
                count: self.unread_count,
            });
        }
        if self.working {
            return Some(SidebarFeltBadge::Working);
        }
        None
    }

    pub fn to_channel_brief(&self) -> ChannelAgentBrief {
        ChannelAgentBrief {
            id: self.id.clone(),
            avatar_seed: Some(self.avatar_seed.clone()),
            avatar_hue: self.avatar_hue,
            avatar_color: self.avatar_color.clone(),
            avatar_url: self.avatar_url.clone(),
        }
    }
}

/// Render-ready multi-bot shared channel row item for `WorkspaceSidebar`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarChannelItem {
    pub id: String,
    pub name: String,
    pub preview: String,
    pub members: Vec<ChannelMember>,
    pub icon: Option<ChannelIcon>,
    pub image_url: Option<String>,
    pub image_failed: bool,
    pub unread_count: u64,
    pub active_tasks: u64,
    pub waiting_reason: Option<SidebarWaitReason>,
    pub muted: bool,
    pub archived: bool,
    pub section_id: Option<String>,
}

impl SidebarChannelItem {
    pub fn from_channel_summary(summary: &ChannelSummary) -> Self {
        let preview = summary
            .last_message
            .as_ref()
            .map(|m| format!("{}: {}", m.author_name, m.text))
            .unwrap_or_else(|| format!("{} agents", summary.channel.draft.members.len()));
        Self {
            id: summary.channel.id.clone(),
            name: summary.channel.draft.name.clone(),
            preview,
            members: summary.channel.draft.members.clone(),
            icon: summary.channel.draft.icon.clone().flatten(),
            image_url: summary.channel.image_url.clone().flatten(),
            image_failed: false,
            unread_count: summary.unread_count,
            active_tasks: summary.active_tasks,
            waiting_reason: None,
            muted: false,
            archived: summary.channel.archived,
            section_id: None,
        }
    }

    pub fn effective_badge(&self) -> Option<SidebarFeltBadge> {
        if let Some(reason) = self.waiting_reason {
            return Some(SidebarFeltBadge::Waiting { reason });
        }
        if self.unread_count > 0 {
            return Some(SidebarFeltBadge::Unread {
                count: self.unread_count,
            });
        }
        if self.active_tasks > 0 {
            return Some(SidebarFeltBadge::Working);
        }
        None
    }
}

/// Dismissible bottom card displayed below `SidebarNav`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarBottomCard {
    pub kind: SidebarBottomCardKind,
    pub title: String,
    pub subtitle: String,
    pub cta_label: String,
}

/// Evaluates whether an agent matches the active sidebar search text and filter chip.
pub fn agent_matches_sidebar_filter(
    agent: &SidebarAgentItem,
    query: &str,
    filter: &SidebarFilter,
    layout: &SidebarLayoutSnapshot,
) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if !q.is_empty()
        && !agent.name.to_ascii_lowercase().contains(&q)
        && !agent.title.to_ascii_lowercase().contains(&q)
        && !agent.preview.to_ascii_lowercase().contains(&q)
    {
        return false;
    }
    match filter {
        SidebarFilter::All => true,
        SidebarFilter::Unread => agent.unread_count > 0,
        SidebarFilter::Waiting => agent.is_waiting(),
        SidebarFilter::Working => agent.working,
        SidebarFilter::Muted => agent.muted,
        SidebarFilter::BySection { section_id } => {
            if section_id == SIDEBAR_PEOPLE_SECTION_ID {
                agent.is_person_dm
            } else if section_id == SIDEBAR_UNASSIGNED_SECTION_ID {
                !agent.is_person_dm && !layout.agent_assignments.contains_key(&agent.id)
            } else {
                layout.agent_assignments.get(&agent.id).map(String::as_str)
                    == Some(section_id.as_str())
            }
        }
    }
}

/// Evaluates whether a channel matches the active sidebar search text and filter chip.
pub fn channel_matches_sidebar_filter(
    channel: &SidebarChannelItem,
    query: &str,
    filter: &SidebarFilter,
) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if !q.is_empty()
        && !channel.name.to_ascii_lowercase().contains(&q)
        && !channel.preview.to_ascii_lowercase().contains(&q)
    {
        return false;
    }
    match filter {
        SidebarFilter::All => true,
        SidebarFilter::Unread => channel.unread_count > 0,
        SidebarFilter::Waiting => channel.waiting_reason.is_some(),
        SidebarFilter::Working => channel.active_tasks > 0,
        SidebarFilter::Muted => channel.muted,
        SidebarFilter::BySection { section_id } => {
            channel.section_id.as_deref() == Some(section_id.as_str())
                || (section_id == SIDEBAR_UNASSIGNED_SECTION_ID && channel.section_id.is_none())
        }
    }
}

/// Orders non-hidden agents belonging to `section_id` according to `layout.agent_order`.
pub fn ordered_agents_for_section<'a>(
    agents: &'a [SidebarAgentItem],
    section_id: &str,
    layout: &SidebarLayoutSnapshot,
) -> Vec<&'a SidebarAgentItem> {
    let mut matched: Vec<&SidebarAgentItem> = agents
        .iter()
        .filter(|a| a.hidden_since.is_none())
        .filter(|a| {
            if section_id == SIDEBAR_PEOPLE_SECTION_ID {
                a.is_person_dm
            } else if section_id == SIDEBAR_UNASSIGNED_SECTION_ID {
                !a.is_person_dm && !layout.agent_assignments.contains_key(&a.id)
            } else {
                !a.is_person_dm
                    && layout.agent_assignments.get(&a.id).map(String::as_str)
                        == Some(section_id)
            }
        })
        .collect();
    matched.sort_by_key(|a| {
        layout
            .agent_order
            .iter()
            .position(|id| id == &a.id)
            .unwrap_or(usize::MAX)
    });
    matched
}

/// Renders `SidebarTopbar` (`ServerMenu`, History Back/Forward, Update badge, Collapse, Marketplace).
pub fn render_sidebar_topbar(
    state: &SidebarState,
    server_name: &str,
    palette: &OpenBotPalette,
) -> Div {
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);
    let hover = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let overlay = pal(palette, "--openbot-bg-overlay", 0x1A1A1A);
    let border = pal(palette, "--openbot-border-grouped", 0x333335);
    let accent = pal(palette, "--openbot-accent", 0x007CF7);
    let accent_soft = pal(palette, "--openbot-accent-soft", 0x163150);

    let server_trigger = div()
        .relative()
        .flex_1()
        .min_w(px(0.0))
        .child(
            div()
                .id("openbot-sidebar-server-menu")
                .h(px(28.0))
                .px(px(8.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(5.0))
                .hover(move |s| s.bg(hover))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child(server_name.to_string()),
                )
                .child(tg_icon_rgba(IconName::ChevronDown, 12.0, text_muted)),
        )
        .when(state.server_menu_open, |el| {
            let menu_items = [
                ("add-server", "Add Server"),
                ("create-local", "Create Local Server"),
                ("install-ssh", "Install on Remote SSH"),
            ];
            let mut pop = div()
                .w(px(208.0))
                .p(px(6.0))
                .rounded(px(12.0))
                .bg(overlay)
                .border_1()
                .border_color(border)
                .flex()
                .flex_col()
                .gap(px(2.0));
            for (k, lbl) in menu_items {
                pop = pop.child(
                    div()
                        .id(SharedString::from(format!("openbot-srv-menu-{k}")))
                        .h(px(30.0))
                        .px(px(8.0))
                        .rounded(px(8.0))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .text_size(px(13.0))
                        .text_color(text_pri)
                        .hover(move |s| s.bg(hover))
                        .child(lbl),
                );
            }
            el.child(
                deferred(div().absolute().top(px(32.0)).left(px(0.0)).occlude().child(pop))
                    .with_priority(50),
            )
        });

    div()
        .h(px(42.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(server_trigger)
        .when(state.server_update_available, |el| {
            el.child(
                div()
                    .id("openbot-sidebar-update-badge")
                    .px(px(6.0))
                    .h(px(20.0))
                    .rounded(px(999.0))
                    .bg(accent_soft)
                    .flex()
                    .items_center()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(accent)
                    .child("Update"),
            )
        })
        .child(
            div()
                .id("openbot-sidebar-back")
                .w(px(24.0))
                .h(px(24.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .opacity(if state.history_back_available { 1.0 } else { 0.35 })
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(hover))
                .child(tg_icon_rgba(IconName::BackArrow, 13.0, text_muted)),
        )
        .child(
            div()
                .id("openbot-sidebar-forward")
                .w(px(24.0))
                .h(px(24.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .opacity(if state.history_forward_available { 1.0 } else { 0.35 })
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(hover))
                .child(tg_icon_rgba(IconName::Forward, 13.0, text_muted)),
        )
        .child(
            div()
                .id("openbot-sidebar-collapse")
                .w(px(24.0))
                .h(px(24.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(hover))
                .child(tg_icon_rgba(IconName::SidebarLeftClose, 14.0, text_muted)),
        )
}

/// Renders `SidebarSearch` input bar + filter chips (`Unread`, `Waiting`, `Working`, `Muted`, `By section`) + `+` New menu.
pub fn render_sidebar_search_bar(state: &SidebarState, palette: &OpenBotPalette) -> Div {
    let ctrl_bg = pal(palette, "--openbot-bg-control", 0x212121);
    let hover = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let overlay = pal(palette, "--openbot-bg-overlay", 0x1A1A1A);
    let border = pal(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);
    let accent = pal(palette, "--openbot-accent", 0x007CF7);
    let accent_soft = pal(palette, "--openbot-accent-soft", 0x163150);

    let filter_active = state.filter != SidebarFilter::All;
    let search_text = if state.search_query.is_empty() {
        "Search chats…".to_string()
    } else {
        state.search_query.clone()
    };

    let mut chips = div()
        .id("openbot-sidebar-filter-chips")
        .min_w(px(0.0))
        .overflow_x_scroll()
        .flex()
        .items_center()
        .gap(px(4.0))
        .px(px(10.0))
        .pb(px(6.0));

    let chip_specs = [
        ("unread", "Unread", matches!(state.filter, SidebarFilter::Unread)),
        ("waiting", "Waiting", matches!(state.filter, SidebarFilter::Waiting)),
        ("working", "Working", matches!(state.filter, SidebarFilter::Working)),
        ("muted", "Muted", matches!(state.filter, SidebarFilter::Muted)),
        (
            "section",
            "By section",
            matches!(state.filter, SidebarFilter::BySection { .. }),
        ),
    ];
    for (slug, lbl, active) in chip_specs {
        chips = chips.child(
            div()
                .id(SharedString::from(format!("openbot-sb-chip-{slug}")))
                .h(px(22.0))
                .px(px(8.0))
                .rounded(px(999.0))
                .bg(if active { accent_soft } else { ctrl_bg })
                .border_1()
                .border_color(if active { accent } else { border })
                .cursor_pointer()
                .flex()
                .items_center()
                .flex_shrink_0()
                .text_size(px(11.0))
                .text_color(if active { accent } else { text_muted })
                .hover(move |s| s.bg(hover))
                .child(lbl),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .id("openbot-sidebar-search-input")
                        .flex_1()
                        .min_w(px(0.0))
                        .h(px(30.0))
                        .px(px(8.0))
                        .rounded(px(9.0))
                        .bg(ctrl_bg)
                        .border_1()
                        .border_color(border)
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
                                .text_color(if state.search_query.is_empty() {
                                    text_muted
                                } else {
                                    text_pri
                                })
                                .child(search_text),
                        )
                        .when(filter_active, |el| {
                            el.child(
                                div()
                                    .px(px(5.0))
                                    .rounded(px(6.0))
                                    .bg(accent_soft)
                                    .text_size(px(10.0))
                                    .text_color(accent)
                                    .child(state.filter.label()),
                            )
                        }),
                )
                .child(
                    div()
                        .relative()
                        .child(
                            div()
                                .id("openbot-sidebar-new-btn")
                                .w(px(30.0))
                                .h(px(30.0))
                                .rounded(px(9.0))
                                .bg(ctrl_bg)
                                .border_1()
                                .border_color(border)
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(16.0))
                                .text_color(text_pri)
                                .hover(move |s| s.bg(hover))
                                .child("+"),
                        )
                        .when(state.new_menu_open, |el| {
                            let new_items = [
                                ("new-agent", "New Agent", IconName::BotRobot),
                                ("new-channel", "New Channel", IconName::GroupUsers),
                                ("new-section", "New Section", IconName::FolderWork),
                            ];
                            let mut pop = div()
                                .w(px(176.0))
                                .p(px(6.0))
                                .rounded(px(12.0))
                                .bg(overlay)
                                .border_1()
                                .border_color(border)
                                .flex()
                                .flex_col()
                                .gap(px(2.0));
                            for (k, lbl, ic) in new_items {
                                pop = pop.child(
                                    div()
                                        .id(SharedString::from(format!("openbot-sb-new-{k}")))
                                        .h(px(30.0))
                                        .px(px(8.0))
                                        .rounded(px(8.0))
                                        .cursor_pointer()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .hover(move |s| s.bg(hover))
                                        .child(tg_icon_rgba(ic, 14.0, text_muted))
                                        .child(div().text_size(px(13.0)).text_color(text_pri).child(lbl)),
                                );
                            }
                            el.child(
                                deferred(
                                    div().absolute().top(px(34.0)).right(px(0.0)).occlude().child(pop),
                                )
                                .with_priority(50),
                            )
                        }),
                ),
        )
        .when(state.filter_menu_open || filter_active, |el| el.child(chips))
}

fn ring_rgb_for_row(ring: SidebarPlushRingKind, dark: bool) -> u32 {
    let spec = ring.spec();
    if dark {
        spec.dark_fallback_rgb
    } else {
        spec.light_fallback_rgb
    }
}

fn render_agent_row(
    agent: &SidebarAgentItem,
    state: &SidebarState,
    palette: &OpenBotPalette,
    indent_px: f32,
    asleep_row: bool,
    now_sec: f64,
    now_ms: u64,
) -> Div {
    let dark = palette.scheme.is_dark();
    let active = state.selected_agent_id.as_deref() == Some(agent.id.as_str());
    let hovered = state.hovered_item_id.as_deref() == Some(agent.id.as_str());
    let ring = SidebarPlushRingKind::select_for_row(
        hovered,
        active,
        true,
        !dark,
        state.dragging_item_id.is_some(),
    );
    let ring_rgb = ring_rgb_for_row(ring, dark);
    let selected_bg = pal(palette, "--openbot-bg-control-active", 0x2A2A2A);
    let hover_bg = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);
    let warning = pal(palette, "--openbot-warning", 0xFF9412);
    let warning_soft = pal(palette, "--openbot-warning-soft", 0x3A280E);

    let mut avatar_props = AgentAvatarProps::new(&agent.avatar_seed)
        .with_size(34.0)
        .with_mood(if asleep_row { AvatarMood::Asleep } else { agent.mood })
        .with_hue_and_color(agent.avatar_hue, agent.avatar_color.clone())
        .with_url(agent.avatar_url.clone());
    avatar_props.dark_scheme = dark;
    avatar_props.cutout_ring = Some(ring);

    let subtitle = if let Some(ref since) = agent.hidden_since {
        format!("Hidden since {since}")
    } else if let Some(ref wait_txt) = agent.waiting_summary {
        wait_txt.clone()
    } else {
        agent.preview.clone()
    };

    let row_el = div()
        .id(SharedString::from(format!("openbot-sb-agent-{}", agent.id)))
        .h(px(48.0))
        .pl(px(10.0 + indent_px))
        .pr(px(10.0))
        .mx(px(6.0))
        .rounded(px(11.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(10.0))
        .when(active, |el| el.bg(selected_bg))
        .when(asleep_row, |el| el.opacity(0.6))
        .hover(move |s| s.bg(hover_bg))
        .child(
            div()
                .relative()
                .child(render_agent_avatar(&avatar_props, None, now_sec))
                .when_some(agent.effective_badge(), |el, badge| {
                    el.child(
                        div()
                            .absolute()
                            .bottom(px(-3.0))
                            .right(px(-4.0))
                            .child(render_sidebar_felt_badge(badge, ring_rgb, None, now_ms)),
                    )
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(px(13.5))
                                .font_weight(if agent.unread_count > 0 || active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(text_pri)
                                .child(agent.name.clone()),
                        )
                        .when(agent.muted, |el| {
                            el.child(tg_icon_rgba(IconName::Mute, 11.0, text_muted))
                        }),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.0))
                        .text_color(if agent.is_waiting() { warning } else { text_muted })
                        .child(subtitle),
                ),
        )
        .when(agent.is_waiting(), |el| {
            el.child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(999.0))
                    .bg(warning_soft)
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(warning)
                    .child(if agent.usage_limited { "Limit" } else { "Wait" }),
            )
        });

    let mut wrapper = div().flex().flex_col().child(row_el);
    for thread in &agent.threads {
        let t_active = state.selected_thread_id.as_deref() == Some(thread.id.as_str());
        wrapper = wrapper.child(
            div()
                .id(SharedString::from(format!("openbot-sb-thread-{}", thread.id)))
                .h(px(28.0))
                .pl(px(46.0 + indent_px))
                .pr(px(12.0))
                .mx(px(6.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_between()
                .when(t_active, |el| el.bg(selected_bg))
                .hover(move |s| s.bg(hover_bg))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(12.0))
                        .text_color(if t_active { text_pri } else { text_muted })
                        .child(format!("↳ {}", thread.title)),
                )
                .when(thread.unread_count > 0, |el| {
                    el.child(render_sidebar_felt_badge(
                        SidebarFeltBadge::Unread {
                            count: thread.unread_count,
                        },
                        ring_rgb,
                        None,
                        now_ms,
                    ))
                }),
        );
    }
    wrapper
}

fn render_channel_row(
    channel: &SidebarChannelItem,
    briefs: &[ChannelAgentBrief],
    state: &SidebarState,
    palette: &OpenBotPalette,
    indent_px: f32,
    now_sec: f64,
    now_ms: u64,
) -> Stateful<Div> {
    let dark = palette.scheme.is_dark();
    let active = state.selected_channel_id.as_deref() == Some(channel.id.as_str());
    let hovered = state.hovered_item_id.as_deref() == Some(channel.id.as_str());
    let ring = SidebarPlushRingKind::select_for_row(
        hovered,
        active,
        true,
        !dark,
        state.dragging_item_id.is_some(),
    );
    let ring_rgb = ring_rgb_for_row(ring, dark);
    let selected_bg = pal(palette, "--openbot-bg-control-active", 0x2A2A2A);
    let hover_bg = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);
    let accent = pal(palette, "--openbot-accent", 0x007CF7);
    let accent_soft = pal(palette, "--openbot-accent-soft", 0x163150);

    let avatar = render_channel_avatar(
        &channel.members,
        briefs,
        ChannelAvatarLayout::Cluster,
        channel.icon.as_ref(),
        channel.image_url.as_deref(),
        channel.image_failed,
        34.0,
        Some(ring),
        dark,
        now_sec,
    );

    div()
        .id(SharedString::from(format!("openbot-sb-chan-{}", channel.id)))
        .h(px(48.0))
        .pl(px(10.0 + indent_px))
        .pr(px(10.0))
        .mx(px(6.0))
        .rounded(px(11.0))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(10.0))
        .when(active, |el| el.bg(selected_bg))
        .when(channel.archived, |el| el.opacity(0.65))
        .hover(move |s| s.bg(hover_bg))
        .child(
            div()
                .relative()
                .child(avatar)
                .when_some(channel.effective_badge(), |el, badge| {
                    el.child(
                        div()
                            .absolute()
                            .bottom(px(-3.0))
                            .right(px(-4.0))
                            .child(render_sidebar_felt_badge(badge, ring_rgb, None, now_ms)),
                    )
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .truncate()
                        .text_size(px(13.5))
                        .font_weight(if channel.unread_count > 0 || active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(text_pri)
                        .child(channel.name.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.0))
                        .text_color(text_muted)
                        .child(channel.preview.clone()),
                ),
        )
        .when(channel.archived, |el| {
            el.child(
                div()
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(999.0))
                    .bg(accent_soft)
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(accent)
                    .child("Restore"),
            )
        })
}

fn render_pinned_group(
    state: &SidebarState,
    agents: &[SidebarAgentItem],
    channels: &[SidebarChannelItem],
    briefs: &[ChannelAgentBrief],
    palette: &OpenBotPalette,
    now_sec: f64,
    now_ms: u64,
) -> Option<Div> {
    if state.pins.is_empty() {
        return None;
    }
    let dark = palette.scheme.is_dark();
    let ring_rgb = ring_rgb_for_row(SidebarPlushRingKind::Default, dark);
    let hover = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let text_sec = pal(palette, "--openbot-text-secondary", 0xDCDCDC);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);

    let mut tiles = div()
        .px(px(10.0))
        .pb(px(6.0))
        .flex()
        .flex_wrap()
        .gap(px(8.0));
    let mut rendered_any = false;

    for pin in &state.pins {
        match pin.kind {
            SidebarPinKind::Agent | SidebarPinKind::Dm => {
                if let Some(agent) = agents.iter().find(|a| a.id == pin.id) {
                    rendered_any = true;
                    let mut p = AgentAvatarProps::new(&agent.avatar_seed)
                        .with_size(SIDEBAR_PINNED_TILE_PX)
                        .with_mood(agent.mood)
                        .with_hue_and_color(agent.avatar_hue, agent.avatar_color.clone())
                        .with_url(agent.avatar_url.clone());
                    p.dark_scheme = dark;
                    tiles = tiles.child(
                        div()
                            .id(SharedString::from(format!("openbot-pin-agent-{}", agent.id)))
                            .w(px(68.0))
                            .p(px(4.0))
                            .rounded(px(12.0))
                            .cursor_pointer()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(4.0))
                            .hover(move |s| s.bg(hover))
                            .child(
                                div()
                                    .relative()
                                    .child(render_agent_avatar(&p, None, now_sec))
                                    .when_some(agent.effective_badge(), |el, badge| {
                                        el.child(
                                            div()
                                                .absolute()
                                                .bottom(px(-2.0))
                                                .right(px(-2.0))
                                                .child(render_sidebar_felt_badge(
                                                    badge, ring_rgb, None, now_ms,
                                                )),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_center()
                                    .text_size(px(11.0))
                                    .text_color(text_sec)
                                    .child(agent.name.clone()),
                            ),
                    );
                }
            }
            SidebarPinKind::Channel => {
                if let Some(chan) = channels.iter().find(|c| c.id == pin.id) {
                    rendered_any = true;
                    let avatar = render_channel_avatar(
                        &chan.members,
                        briefs,
                        ChannelAvatarLayout::Cluster,
                        chan.icon.as_ref(),
                        chan.image_url.as_deref(),
                        chan.image_failed,
                        SIDEBAR_PINNED_TILE_PX,
                        Some(SidebarPlushRingKind::Default),
                        dark,
                        now_sec,
                    );
                    tiles = tiles.child(
                        div()
                            .id(SharedString::from(format!("openbot-pin-chan-{}", chan.id)))
                            .w(px(68.0))
                            .p(px(4.0))
                            .rounded(px(12.0))
                            .cursor_pointer()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(4.0))
                            .hover(move |s| s.bg(hover))
                            .child(
                                div()
                                    .relative()
                                    .child(avatar)
                                    .when_some(chan.effective_badge(), |el, badge| {
                                        el.child(
                                            div()
                                                .absolute()
                                                .bottom(px(-2.0))
                                                .right(px(-2.0))
                                                .child(render_sidebar_felt_badge(
                                                    badge, ring_rgb, None, now_ms,
                                                )),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_center()
                                    .text_size(px(11.0))
                                    .text_color(text_sec)
                                    .child(chan.name.clone()),
                            ),
                    );
                }
            }
        }
    }

    if !rendered_any {
        return None;
    }

    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .px(px(14.0))
                    .py(px(2.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text_muted)
                    .child("PINNED"),
            )
            .child(tiles),
    )
}

fn render_compact_sidebar(
    state: &SidebarState,
    agents: &[SidebarAgentItem],
    channels: &[SidebarChannelItem],
    briefs: &[ChannelAgentBrief],
    palette: &OpenBotPalette,
    now_sec: f64,
    now_ms: u64,
) -> Div {
    let dark = palette.scheme.is_dark();
    let bg = pal(palette, "--openbot-bg-sidebar", 0x1A1A1A);
    let border = pal(palette, "--openbot-border", 0x222222);
    let hover = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let selected_bg = pal(palette, "--openbot-bg-control-active", 0x2A2A2A);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);
    let ring_rgb = ring_rgb_for_row(SidebarPlushRingKind::Default, dark);

    let mut rail_list = div()
        .id("openbot-sidebar-compact-scroll")
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .overflow_y_scroll()
        .py(px(6.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.0));

    for agent in agents.iter().filter(|a| a.hidden_since.is_none()) {
        let active = state.selected_agent_id.as_deref() == Some(agent.id.as_str());
        let mut p = AgentAvatarProps::new(&agent.avatar_seed)
            .with_size(40.0)
            .with_mood(agent.mood)
            .with_hue_and_color(agent.avatar_hue, agent.avatar_color.clone())
            .with_url(agent.avatar_url.clone());
        p.dark_scheme = dark;
        rail_list = rail_list.child(
            div()
                .id(SharedString::from(format!("openbot-compact-agent-{}", agent.id)))
                .relative()
                .w(px(56.0))
                .h(px(56.0))
                .rounded(px(14.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .when(active, |el| el.bg(selected_bg))
                .hover(move |s| s.bg(hover))
                .child(render_agent_avatar(&p, None, now_sec))
                .when_some(agent.effective_badge(), |el, badge| {
                    el.child(
                        div()
                            .absolute()
                            .bottom(px(3.0))
                            .right(px(3.0))
                            .child(render_sidebar_felt_badge(badge, ring_rgb, None, now_ms)),
                    )
                }),
        );
    }

    for chan in channels.iter().filter(|c| !c.archived) {
        let active = state.selected_channel_id.as_deref() == Some(chan.id.as_str());
        let av = render_channel_avatar(
            &chan.members,
            briefs,
            ChannelAvatarLayout::Cluster,
            chan.icon.as_ref(),
            chan.image_url.as_deref(),
            chan.image_failed,
            40.0,
            Some(SidebarPlushRingKind::Default),
            dark,
            now_sec,
        );
        rail_list = rail_list.child(
            div()
                .id(SharedString::from(format!("openbot-compact-chan-{}", chan.id)))
                .relative()
                .w(px(56.0))
                .h(px(56.0))
                .rounded(px(14.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .when(active, |el| el.bg(selected_bg))
                .hover(move |s| s.bg(hover))
                .child(av)
                .when_some(chan.effective_badge(), |el, badge| {
                    el.child(
                        div()
                            .absolute()
                            .bottom(px(3.0))
                            .right(px(3.0))
                            .child(render_sidebar_felt_badge(badge, ring_rgb, None, now_ms)),
                    )
                }),
        );
    }

    div()
        .w(px(state.effective_width_px()))
        .h_full()
        .flex_shrink_0()
        .bg(bg)
        .border_r_1()
        .border_color(border)
        .flex()
        .flex_col()
        .items_center()
        .pt(px(10.0))
        .pb(px(10.0))
        .gap(px(8.0))
        .child(
            div()
                .id("openbot-sidebar-expand-btn")
                .w(px(36.0))
                .h(px(32.0))
                .rounded(px(9.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(hover))
                .child(tg_icon_rgba(IconName::SidebarLeftOpen, 15.0, text_muted)),
        )
        .child(rail_list)
}

fn render_sidebar_bottom_cards(
    state: &SidebarState,
    cards: &[SidebarBottomCard],
    palette: &OpenBotPalette,
) -> Option<Div> {
    let visible: Vec<&SidebarBottomCard> = cards
        .iter()
        .filter(|card| !state.is_bottom_card_dismissed(card.kind))
        .collect();
    if visible.is_empty() {
        return None;
    }

    let bg_surface = pal(palette, "--openbot-bg-surface-raised", 0x242424);
    let border = pal(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = pal(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);
    let accent = pal(palette, "--openbot-accent", 0x007CF7);
    let accent_soft = pal(palette, "--openbot-accent-soft", 0x163150);

    let mut stack = div().px(px(10.0)).py(px(8.0)).flex().flex_col().gap(px(6.0));
    for (idx, card) in visible.into_iter().enumerate() {
        stack = stack.child(
            div()
                .id(SharedString::from(format!("openbot-sb-card-{idx}")))
                .p(px(10.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border)
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
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_pri)
                                .child(card.title.clone()),
                        )
                        .child(tg_icon_rgba(IconName::Close, 12.0, text_muted)),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(text_muted)
                        .child(card.subtitle.clone()),
                )
                .child(
                    div()
                        .mt(px(2.0))
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(8.0))
                        .bg(accent_soft)
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(accent)
                        .child(card.cta_label.clone()),
                ),
        );
    }
    Some(stack)
}

/// Renders the full 7-group `WorkspaceSidebar` with explicit server title and bottom cards.
#[allow(clippy::too_many_arguments)]
pub fn render_workspace_sidebar_with_cards(
    state: &SidebarState,
    agents: &[SidebarAgentItem],
    channels: &[SidebarChannelItem],
    server_name: &str,
    bottom_cards: &[SidebarBottomCard],
    palette: &OpenBotPalette,
    now_sec: f64,
    now_ms: u64,
) -> Div {
    let width = state.effective_width_px();
    if state.closed || width <= 0.0 {
        return div().w(px(0.0)).h_full().flex_shrink_0().overflow_hidden();
    }

    let briefs: Vec<ChannelAgentBrief> = agents.iter().map(SidebarAgentItem::to_channel_brief).collect();
    if state.compact {
        return render_compact_sidebar(state, agents, channels, &briefs, palette, now_sec, now_ms);
    }

    let bg = pal(palette, "--openbot-bg-sidebar", 0x1A1A1A);
    let border = pal(palette, "--openbot-border", 0x222222);
    let hover = pal(palette, "--openbot-bg-control-hover", 0x242424);
    let text_muted = pal(palette, "--openbot-text-muted", 0x979797);
    let warning = pal(palette, "--openbot-warning", 0xFF9412);
    let accent = pal(palette, "--openbot-accent", 0x007CF7);

    let filtered_agents: Vec<SidebarAgentItem> = agents
        .iter()
        .filter(|a| agent_matches_sidebar_filter(a, &state.search_query, &state.filter, &state.layout))
        .cloned()
        .collect();
    let filtered_channels: Vec<SidebarChannelItem> = channels
        .iter()
        .filter(|c| channel_matches_sidebar_filter(c, &state.search_query, &state.filter))
        .cloned()
        .collect();

    let mut nav = div()
        .id("openbot-sidebar-nav-scroll")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .py(px(4.0))
        .flex()
        .flex_col()
        .gap(px(8.0));

    // Group 1: SidebarPinnedGroup (`56px` pinned tiles)
    if let Some(pinned_el) =
        render_pinned_group(state, &filtered_agents, &filtered_channels, &briefs, palette, now_sec, now_ms)
    {
        nav = nav.child(pinned_el);
    }

    // Group 2: SidebarWaitingGroup (blocked on Question / Approval / Takeover / Usage Limit)
    let waiting_agents: Vec<&SidebarAgentItem> = filtered_agents
        .iter()
        .filter(|a| a.hidden_since.is_none() && a.is_waiting())
        .collect();
    let waiting_channels: Vec<&SidebarChannelItem> = filtered_channels
        .iter()
        .filter(|c| !c.archived && c.waiting_reason.is_some())
        .collect();
    if !waiting_agents.is_empty() || !waiting_channels.is_empty() {
        let mut group = div().flex().flex_col().gap(px(2.0)).child(
            div()
                .px(px(14.0))
                .py(px(2.0))
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(warning)
                .child(format!("WAITING ({})", waiting_agents.len() + waiting_channels.len())),
        );
        for agent in waiting_agents {
            group = group.child(render_agent_row(agent, state, palette, 0.0, false, now_sec, now_ms));
        }
        for chan in waiting_channels {
            group = group.child(render_channel_row(chan, &briefs, state, palette, 0.0, now_sec, now_ms));
        }
        nav = nav.child(group);
    }

    // Group 3: SidebarUnreadGroup (felt lilac unread pills)
    let unread_agents: Vec<&SidebarAgentItem> = filtered_agents
        .iter()
        .filter(|a| a.hidden_since.is_none() && !a.is_waiting() && a.unread_count > 0)
        .collect();
    let unread_channels: Vec<&SidebarChannelItem> = filtered_channels
        .iter()
        .filter(|c| !c.archived && c.waiting_reason.is_none() && c.unread_count > 0)
        .collect();
    if !unread_agents.is_empty() || !unread_channels.is_empty() {
        let mut group = div().flex().flex_col().gap(px(2.0)).child(
            div()
                .px(px(14.0))
                .py(px(2.0))
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_muted)
                .child(format!("UNREAD ({})", unread_agents.len() + unread_channels.len())),
        );
        for agent in unread_agents {
            group = group.child(render_agent_row(agent, state, palette, 0.0, false, now_sec, now_ms));
        }
        for chan in unread_channels {
            group = group.child(render_channel_row(chan, &briefs, state, palette, 0.0, now_sec, now_ms));
        }
        nav = nav.child(group);
    }

    // Group 4: SidebarSectionList (nested folders up to depth 3 + People + Unassigned)
    let depths = state.resolved_folder_depths();
    let mut sections_el = div().flex().flex_col().gap(px(4.0));
    for section_id in &state.layout.order {
        let depth = depths.get(section_id).copied().unwrap_or(0);
        let indent_px = (depth as f32) * 14.0;
        let is_drop_target = state.drop_target_section_id.as_deref() == Some(section_id.as_str());
        let sec_meta = state.layout.sections.iter().find(|s| &s.id == section_id);
        let collapsed = sec_meta.is_some_and(|s| s.collapsed);
        let heading = match section_id.as_str() {
            SIDEBAR_PEOPLE_SECTION_ID => "PEOPLE".to_string(),
            SIDEBAR_UNASSIGNED_SECTION_ID => "AGENTS & CHANNELS".to_string(),
            _ => sec_meta
                .map(|s| s.name.to_uppercase())
                .unwrap_or_else(|| section_id.to_uppercase()),
        };

        let sec_agents = ordered_agents_for_section(&filtered_agents, section_id, &state.layout);
        let sec_channels: Vec<&SidebarChannelItem> = filtered_channels
            .iter()
            .filter(|c| !c.archived)
            .filter(|c| {
                if section_id == SIDEBAR_UNASSIGNED_SECTION_ID {
                    c.section_id.is_none()
                } else {
                    c.section_id.as_deref() == Some(section_id.as_str())
                }
            })
            .collect();

        if sec_agents.is_empty() && sec_channels.is_empty() && sec_meta.is_none() {
            continue;
        }

        let mut sec_block = div().flex().flex_col().gap(px(2.0));
        if is_drop_target {
            sec_block = sec_block.child(div().mx(px(12.0)).h(px(2.0)).rounded_full().bg(accent));
        }
        sec_block = sec_block.child(
            div()
                .id(SharedString::from(format!("openbot-sb-sec-{section_id}")))
                .h(px(24.0))
                .pl(px(12.0 + indent_px))
                .pr(px(10.0))
                .mx(px(4.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(5.0))
                .hover(move |s| s.bg(hover))
                .child(tg_icon_rgba(
                    if collapsed {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    },
                    10.0,
                    text_muted,
                ))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_muted)
                        .child(heading),
                ),
        );

        if !collapsed {
            for chan in sec_channels {
                sec_block = sec_block.child(render_channel_row(
                    chan, &briefs, state, palette, indent_px, now_sec, now_ms,
                ));
            }
            for agent in sec_agents {
                sec_block = sec_block.child(render_agent_row(
                    agent, state, palette, indent_px, false, now_sec, now_ms,
                ));
            }
        }
        sections_el = sections_el.child(sec_block);
    }
    nav = nav.child(sections_el);

    // Group 5: SidebarHiddenGroup (asleep agents with opacity 0.6)
    let hidden_agents: Vec<&SidebarAgentItem> = filtered_agents
        .iter()
        .filter(|a| a.hidden_since.is_some())
        .collect();
    if !hidden_agents.is_empty() {
        let mut hidden_group = div().flex().flex_col().gap(px(2.0)).child(
            div()
                .id("openbot-sb-hidden-toggle")
                .h(px(24.0))
                .px(px(14.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(5.0))
                .hover(move |s| s.bg(hover))
                .child(tg_icon_rgba(
                    if state.show_hidden_group {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    },
                    10.0,
                    text_muted,
                ))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_muted)
                        .child(format!("HIDDEN ({})", hidden_agents.len())),
                ),
        );
        if state.show_hidden_group {
            for agent in hidden_agents {
                hidden_group = hidden_group.child(render_agent_row(
                    agent, state, palette, 0.0, true, now_sec, now_ms,
                ));
            }
        }
        nav = nav.child(hidden_group);
    }

    // Group 6: Archived Channels
    let archived_channels: Vec<&SidebarChannelItem> =
        filtered_channels.iter().filter(|c| c.archived).collect();
    if !archived_channels.is_empty() {
        let mut arch_group = div().flex().flex_col().gap(px(2.0)).child(
            div()
                .id("openbot-sb-archived-toggle")
                .h(px(24.0))
                .px(px(14.0))
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(5.0))
                .hover(move |s| s.bg(hover))
                .child(tg_icon_rgba(IconName::ArchivedChats, 12.0, text_muted))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_muted)
                        .child(format!("ARCHIVED CHANNELS ({})", archived_channels.len())),
                ),
        );
        if state.show_archived_channels {
            for chan in archived_channels {
                arch_group = arch_group.child(render_channel_row(
                    chan, &briefs, state, palette, 0.0, now_sec, now_ms,
                ));
            }
        }
        nav = nav.child(arch_group);
    }

    // Group 7: Bottom dismissible cards
    div()
        .w(px(width))
        .h_full()
        .flex_shrink_0()
        .bg(bg)
        .border_r_1()
        .border_color(border)
        .flex()
        .flex_col()
        .child(render_sidebar_topbar(state, server_name, palette))
        .child(render_sidebar_search_bar(state, palette))
        .child(nav)
        .when_some(
            render_sidebar_bottom_cards(state, bottom_cards, palette),
            |el, cards_el| el.child(cards_el),
        )
}

/// Primary `WorkspaceSidebar` GPUI renderer (`OB-F14`).
pub fn render_workspace_sidebar(
    state: &SidebarState,
    agents: &[SidebarAgentItem],
    channels: &[SidebarChannelItem],
    palette: &OpenBotPalette,
    now_sec: f64,
    now_ms: u64,
) -> Div {
    render_workspace_sidebar_with_cards(
        state,
        agents,
        channels,
        "Local OpenBot",
        &[],
        palette,
        now_sec,
        now_ms,
    )
}
