//! Douyin-style floating HUD overlays for Code Scene (`CS-F13`).
//!
//! Renders translucent floating top tabs, right vertical engagement action rail,
//! bottom-left creator/bot metadata & recommendation explanation card, playable
//! game focus HUD, and bottom video timeline scrubber + playback speed control.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_code_scene::GameHudSnapshot;
use gpui_telegram_proto::code_scene_contracts::{
    AssetKind, CodeSceneFeedTab, RankedSceneItemDto,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::code_scene::viewport::AUDIO_BAR_COUNT;

/// User actions emitted by floating Douyin HUD controls.
#[derive(Debug, Clone, PartialEq)]
pub enum HudAction {
    SelectTab(CodeSceneFeedTab),
    ToggleSearch,
    FocusSearch,
    ClearSearch,
    ClearTagFilter,
    SelectTagFilter(String),
    OpenAiStudio,
    ToggleImmersive,
    PrevCard,
    NextCard,
    ToggleLike,
    ToggleBookmark,
    ToggleFollow,
    OpenComments,
    OpenCodeInspector,
    OpenCreatorProfile,
    OpenRemixStudio,
    OpenShareToChat,
    ToggleMute,
    ToggleMoreMenu,
    NotInterested,
    HideAuthor,
    RestartScene,
    TogglePlayPause,
    CycleSpeed,
    SeekVideoRatio(f64),
    ToggleScoreBreakdown,
    CaptureGameFocus,
    ReleaseGameFocus,
}

/// Read-only HUD render snapshot passed into overlay builders.
pub struct HudRenderProps<'a> {
    pub item: Option<&'a RankedSceneItemDto>,
    pub active_tab: CodeSceneFeedTab,
    pub active_index: usize,
    pub total_items: usize,
    pub tag_filter: Option<&'a str>,
    pub search_open: bool,
    pub search_focused: bool,
    pub search_query: &'a str,
    pub search_ime_preedit: Option<&'a str>,
    pub paused: bool,
    pub playback_speed: f64,
    pub timeline_sec: f64,
    pub duration_sec: Option<f64>,
    pub loop_count: u32,
    pub measured_fps: f32,
    pub audio_muted: bool,
    pub audio_bars: [f32; AUDIO_BAR_COUNT],
    pub game_input_captured: bool,
    pub game_hud: Option<&'a GameHudSnapshot>,
    pub show_more_menu: bool,
    pub show_score_breakdown: bool,
    pub language_id: &'a str,
    pub palette: TdesktopThemePalette,
}

/// Formats compact Douyin-style counters (`0`, `42`, `1.2k`, `3.4w`).
pub fn format_compact_count(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 10_000 {
        format!("{:.1}w", count as f64 / 10_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

/// Formats timeline seconds as `MM:SS`.
pub fn format_timecode(sec: f64) -> String {
    let clamped = if sec.is_finite() && sec > 0.0 {
        sec.floor() as u64
    } else {
        0
    };
    let mins = clamped / 60;
    let rem = clamped % 60;
    format!("{mins:02}:{rem:02}")
}

fn author_initials(name: &str) -> String {
    let trimmed = name.trim().trim_start_matches('@');
    let mut chars = trimmed.chars();
    let first = chars.next().map(|c| c.to_uppercase().to_string());
    let second = chars.next().map(|c| c.to_uppercase().to_string());
    match (first, second) {
        (Some(a), Some(b)) => format!("{a}{b}"),
        (Some(a), None) => a,
        _ => "CS".to_string(),
    }
}

/// Renders the floating top navigation bar: 6 Douyin channel tabs + search + immersive toggle + `+ AI 创作`.
pub fn render_top_floating_bar<V: 'static>(
    props: &HudRenderProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, HudAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let lang = props.language_id;
    let is_zh = lang.starts_with("zh");
    let active_tab = props.active_tab;

    let tab_els = CodeSceneFeedTab::ALL.into_iter().map(|tab| {
        let is_active = tab == active_tab;
        let label = if is_zh { tab.label_zh() } else { tab.label_en() };
        let cb = on_action.clone();
        div()
            .id(("cs-tab", tab as usize))
            .cursor_pointer()
            .px(px(10.0))
            .py(px(6.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(3.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    cb(this, HudAction::SelectTab(tab), cx);
                }),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .font_weight(if is_active {
                        FontWeight::BOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if is_active {
                        rgba(0xFFFFFFFF)
                    } else {
                        rgba(0xFFFFFFA6)
                    })
                    .child(label),
            )
            .child(
                div()
                    .w(px(if is_active { 18.0 } else { 0.0 }))
                    .h(px(2.5))
                    .rounded(px(2.0))
                    .bg(if is_active {
                        rgba(0xFE2C55FF)
                    } else {
                        rgba(0x00000000)
                    }),
            )
    });

    let cb_search_toggle = on_action.clone();
    let cb_search_focus = on_action.clone();
    let cb_search_clear = on_action.clone();
    let cb_tag_clear = on_action.clone();
    let cb_immersive = on_action.clone();
    let cb_studio = on_action.clone();

    let search_display = if props.search_query.is_empty() && props.search_ime_preedit.is_none() {
        tr_str(lang, "搜索代码场景 / 标签…", "Search scenes or #tags…").to_string()
    } else {
        let pre = props.search_ime_preedit.unwrap_or("");
        if props.search_focused {
            format!("{}{}|", props.search_query, pre)
        } else {
            format!("{}{}", props.search_query, pre)
        }
    };
    let search_empty = props.search_query.is_empty() && props.search_ime_preedit.is_none();

    div()
        .id("cs-top-floating-bar")
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .px(px(18.0))
        .pt(px(14.0))
        .pb(px(10.0))
        .bg(linear_gradient(
            180.0,
            linear_color_stop(rgba(0x07070BD8), 0.0),
            linear_color_stop(rgba(0x07070B00), 1.0),
        ))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .px(px(8.0))
                .py(px(2.0))
                .rounded(px(999.0))
                .bg(rgba(0x12131CB8))
                .border_1()
                .border_color(rgba(0xFFFFFF1C))
                .children(tab_els),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .when_some(props.tag_filter, |row, tag| {
                    row.child(
                        div()
                            .id("cs-active-tag-chip")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(999.0))
                            .bg(rgba(0xFE2C55CC))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgba(0xFFFFFFFF))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    cb_tag_clear(this, HudAction::ClearTagFilter, cx);
                                }),
                            )
                            .child(format!("#{tag} ×")),
                    )
                })
                .when(props.search_open, |row| {
                    row.child(
                        div()
                            .id("cs-search-pill")
                            .w(px(200.0))
                            .h(px(32.0))
                            .px(px(10.0))
                            .rounded(px(999.0))
                            .bg(rgba(0x141622E0))
                            .border_1()
                            .border_color(if props.search_focused {
                                rgba(0xFE2C55CC)
                            } else {
                                rgba(0xFFFFFF2A)
                            })
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.0))
                            .cursor_text()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    cb_search_focus(this, HudAction::FocusSearch, cx);
                                }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .text_size(px(12.0))
                                    .text_color(if search_empty {
                                        rgba(0xFFFFFF7A)
                                    } else {
                                        rgba(0xFFFFFFFF)
                                    })
                                    .child(search_display.clone()),
                            )
                            .child(
                                div()
                                    .id("cs-search-clear-btn")
                                    .cursor_pointer()
                                    .px(px(4.0))
                                    .text_size(px(12.0))
                                    .text_color(rgba(0xFFFFFF99))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            cb_search_clear(this, HudAction::ClearSearch, cx);
                                        }),
                                    )
                                    .child("×"),
                            ),
                    )
                })
                .when(!props.search_open, |row| {
                    row.child(
                        div()
                            .id("cs-search-toggle-btn")
                            .cursor_pointer()
                            .w(px(32.0))
                            .h(px(32.0))
                            .rounded(px(999.0))
                            .bg(rgba(0x141622C4))
                            .border_1()
                            .border_color(rgba(0xFFFFFF22))
                            .hover(|s| s.bg(rgba(0x26293CE0)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    cb_search_toggle(this, HudAction::ToggleSearch, cx);
                                }),
                            )
                            .child(tg_icon(IconName::Search, 15.0, 0xFFFFFF)),
                    )
                })
                .child(
                    div()
                        .id("cs-immersive-toggle-btn")
                        .cursor_pointer()
                        .w(px(32.0))
                        .h(px(32.0))
                        .rounded(px(999.0))
                        .bg(rgba(0x141622C4))
                        .border_1()
                        .border_color(rgba(0xFFFFFF22))
                        .hover(|s| s.bg(rgba(0x26293CE0)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_immersive(this, HudAction::ToggleImmersive, cx);
                            }),
                        )
                        .child(tg_icon(IconName::SidebarPanel, 15.0, 0xFFFFFF)),
                )
                .child(
                    div()
                        .id("cs-open-ai-studio-btn")
                        .cursor_pointer()
                        .h(px(34.0))
                        .px(px(14.0))
                        .rounded(px(999.0))
                        .bg(linear_gradient(
                            120.0,
                            linear_color_stop(rgba(0xFE2C55FF), 0.0),
                            linear_color_stop(rgba(0x7C3AEDFF), 1.0),
                        ))
                        .hover(|s| s.opacity(0.92))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_studio(this, HudAction::OpenAiStudio, cx);
                            }),
                        )
                        .child(tg_icon(IconName::BotRobot, 15.0, 0xFFFFFF))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(0xFFFFFFFF))
                                .child(tr_str(lang, "+ AI 创作", "+ AI Create")),
                        ),
                ),
        )
        .into_any_element()
}

fn render_rail_action_btn<V: 'static>(
    id: &'static str,
    icon_el: AnyElement,
    label: String,
    bg_color: Rgba,
    act: HudAction,
    on_act: &(impl Fn(&mut V, HudAction, &mut Context<V>) + Clone + 'static),
    cx: &mut Context<V>,
) -> AnyElement {
    let cb = on_act.clone();
    div()
        .id(id)
        .cursor_pointer()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                cb(this, act.clone(), cx);
            }),
        )
        .child(
            div()
                .w(px(44.0))
                .h(px(44.0))
                .rounded(px(999.0))
                .bg(bg_color)
                .border_1()
                .border_color(rgba(0xFFFFFF22))
                .hover(|s| s.bg(rgba(0xFFFFFF30)))
                .flex()
                .items_center()
                .justify_center()
                .child(icon_el),
        )
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgba(0xFFFFFFEE))
                .child(label),
        )
        .into_any_element()
}

/// Renders the signature Douyin right-side vertical engagement action rail.
pub fn render_right_action_rail<V: 'static>(
    props: &HudRenderProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, HudAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let Some(item) = props.item else {
        return div().size_0().into_any_element();
    };
    let asset = &item.asset;
    let lang = props.language_id;

    let cb_profile = on_action.clone();
    let cb_follow = on_action.clone();
    let cb_prev = on_action.clone();
    let cb_next = on_action.clone();
    let cb_not_int = on_action.clone();
    let cb_hide_author = on_action.clone();
    let cb_restart = on_action.clone();

    let initials = author_initials(&asset.author_username);
    let following = asset.viewer_following_author;

    // Animated 4-bar visual audio equalizer inside the mute button
    let eq_bars = div()
        .flex()
        .items_end()
        .gap(px(2.5))
        .h(px(16.0))
        .children((0..4).map(|i| {
            let level = if props.audio_muted {
                0.18
            } else {
                props.audio_bars[(i * 2).min(AUDIO_BAR_COUNT - 1)].clamp(0.18, 1.0)
            };
            div()
                .w(px(3.0))
                .h(px(4.0 + level * 12.0))
                .rounded(px(2.0))
                .bg(if props.audio_muted {
                    rgba(0xFFFFFF66)
                } else {
                    rgba(0x25F4EEFF)
                })
        }))
        .into_any_element();

    div()
        .id("cs-right-action-rail")
        .absolute()
        .right(px(16.0))
        .bottom(px(88.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(12.0))
        // Prev / Next quick stepper buttons
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .mb(px(4.0))
                .child(
                    div()
                        .id("cs-rail-prev-btn")
                        .cursor_pointer()
                        .w(px(34.0))
                        .h(px(34.0))
                        .rounded(px(999.0))
                        .bg(rgba(0x12131CC4))
                        .border_1()
                        .border_color(rgba(0xFFFFFF22))
                        .hover(|s| s.bg(rgba(0xFFFFFF2E)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_prev(this, HudAction::PrevCard, cx);
                            }),
                        )
                        .child(tg_icon(IconName::ArrowUp, 15.0, 0xFFFFFF)),
                )
                .child(
                    div()
                        .id("cs-rail-next-btn")
                        .cursor_pointer()
                        .w(px(34.0))
                        .h(px(34.0))
                        .rounded(px(999.0))
                        .bg(rgba(0x12131CC4))
                        .border_1()
                        .border_color(rgba(0xFFFFFF22))
                        .hover(|s| s.bg(rgba(0xFFFFFF2E)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_next(this, HudAction::NextCard, cx);
                            }),
                        )
                        .child(tg_icon(IconName::ChevronDown, 16.0, 0xFFFFFF)),
                ),
        )
        // Creator Avatar + Follow `+` Pill
        .child(
            div()
                .relative()
                .pb(px(10.0))
                .flex()
                .flex_col()
                .items_center()
                .child(
                    div()
                        .id("cs-rail-creator-avatar")
                        .cursor_pointer()
                        .w(px(48.0))
                        .h(px(48.0))
                        .rounded(px(999.0))
                        .bg(linear_gradient(
                            135.0,
                            linear_color_stop(rgba(0x25F4EEFF), 0.0),
                            linear_color_stop(rgba(0xFE2C55FF), 1.0),
                        ))
                        .border_2()
                        .border_color(rgba(0xFFFFFFFF))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_profile(this, HudAction::OpenCreatorProfile, cx);
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(0xFFFFFFFF))
                                .child(initials),
                        ),
                )
                .child(
                    div()
                        .id("cs-rail-follow-pill")
                        .cursor_pointer()
                        .absolute()
                        .bottom(px(2.0))
                        .w(px(22.0))
                        .h(px(22.0))
                        .rounded(px(999.0))
                        .bg(if following {
                            rgba(0x10B981FF)
                        } else {
                            rgba(0xFE2C55FF)
                        })
                        .border_2()
                        .border_color(rgba(0x0A0A0FFF))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_follow(this, HudAction::ToggleFollow, cx);
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(0xFFFFFFFF))
                                .child(if following { "✓" } else { "+" }),
                        ),
                ),
        )
        // Like
        .child(render_rail_action_btn(
            "cs-rail-like-btn",
            tg_icon(
                IconName::HeartReaction,
                22.0,
                if asset.viewer_liked { 0xFE2C55 } else { 0xFFFFFF },
            )
            .into_any_element(),
            format_compact_count(asset.likes_count),
            if asset.viewer_liked {
                rgba(0xFE2C5538)
            } else {
                rgba(0x12131CC4)
            },
            HudAction::ToggleLike,
            &on_action,
            cx,
        ))
        // Comments
        .child(render_rail_action_btn(
            "cs-rail-comments-btn",
            tg_icon(IconName::FolderChat, 20.0, 0xFFFFFF).into_any_element(),
            format_compact_count(asset.comments_count),
            rgba(0x12131CC4),
            HudAction::OpenComments,
            &on_action,
            cx,
        ))
        // Bookmark
        .child(render_rail_action_btn(
            "cs-rail-bookmark-btn",
            tg_icon(
                IconName::PremiumStar,
                21.0,
                if asset.viewer_bookmarked { 0xF5B700 } else { 0xFFFFFF },
            )
            .into_any_element(),
            format_compact_count(asset.bookmarks_count),
            if asset.viewer_bookmarked {
                rgba(0xF5B70036)
            } else {
                rgba(0x12131CC4)
            },
            HudAction::ToggleBookmark,
            &on_action,
            cx,
        ))
        // Remix ("做同款")
        .child(render_rail_action_btn(
            "cs-rail-remix-btn",
            tg_icon(IconName::BotRobot, 20.0, 0x25F4EE).into_any_element(),
            tr_str(lang, "做同款", "Remix").to_string(),
            rgba(0x12131CC4),
            HudAction::OpenRemixStudio,
            &on_action,
            cx,
        ))
        // Live Code Inspector (`</>`)
        .child(render_rail_action_btn(
            "cs-rail-code-btn",
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::BOLD)
                .font_family("monospace")
                .text_color(rgba(0xA78BFAFF))
                .child("</>")
                .into_any_element(),
            tr_str(lang, "看代码", "Code").to_string(),
            rgba(0x12131CC4),
            HudAction::OpenCodeInspector,
            &on_action,
            cx,
        ))
        // Share to Chat
        .child(render_rail_action_btn(
            "cs-rail-share-btn",
            tg_icon(IconName::Forward, 20.0, 0xFFFFFF).into_any_element(),
            format_compact_count(asset.shares_count),
            rgba(0x12131CC4),
            HudAction::OpenShareToChat,
            &on_action,
            cx,
        ))
        // Mute / Equalizer
        .child(render_rail_action_btn(
            "cs-rail-mute-btn",
            eq_bars,
            if props.audio_muted {
                tr_str(lang, "静音", "Muted").to_string()
            } else {
                tr_str(lang, "声波", "Pulse").to_string()
            },
            rgba(0x12131CC4),
            HudAction::ToggleMute,
            &on_action,
            cx,
        ))
        // More menu (Not Interested / Hide Author / Restart)
        .child(
            div()
                .relative()
                .child(render_rail_action_btn(
                    "cs-rail-more-btn",
                    tg_icon(IconName::MoreVertical, 18.0, 0xFFFFFF).into_any_element(),
                    tr_str(lang, "更多", "More").to_string(),
                    rgba(0x12131CC4),
                    HudAction::ToggleMoreMenu,
                    &on_action,
                    cx,
                ))
                .when(props.show_more_menu, |el| {
                    el.child(
                        div()
                            .id("cs-more-popover")
                            .absolute()
                            .right(px(54.0))
                            .bottom(px(0.0))
                            .w(px(168.0))
                            .p(px(6.0))
                            .rounded(px(12.0))
                            .bg(rgba(0x141622F2))
                            .border_1()
                            .border_color(rgba(0xFFFFFF26))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .id("cs-more-not-interested")
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(7.0))
                                    .rounded(px(8.0))
                                    .hover(|s| s.bg(rgba(0xFFFFFF1C)))
                                    .text_size(px(12.5))
                                    .text_color(rgba(0xFF94A4FF))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            cb_not_int(this, HudAction::NotInterested, cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "✕ 不感兴趣", "✕ Not Interested")),
                            )
                            .child(
                                div()
                                    .id("cs-more-hide-author")
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(7.0))
                                    .rounded(px(8.0))
                                    .hover(|s| s.bg(rgba(0xFFFFFF1C)))
                                    .text_size(px(12.5))
                                    .text_color(rgba(0xFFFFFFDD))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            cb_hide_author(this, HudAction::HideAuthor, cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "⊘ 屏蔽此作者", "⊘ Hide Author")),
                            )
                            .child(
                                div()
                                    .id("cs-more-restart")
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(7.0))
                                    .rounded(px(8.0))
                                    .hover(|s| s.bg(rgba(0xFFFFFF1C)))
                                    .text_size(px(12.5))
                                    .text_color(rgba(0x25F4EEFF))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            cb_restart(this, HudAction::RestartScene, cx);
                                        }),
                                    )
                                    .child(tr_str(lang, "↻ 重播场景 (R)", "↻ Restart (R)")),
                            ),
                    )
                }),
        )
        .into_any_element()
}

/// Renders bottom-left creator/bot metadata, recommendation reason + score breakdown, clickable tags,
/// playable game focus controls, and the bottom timeline scrubber.
pub fn render_bottom_metadata_and_scrubber<V: 'static>(
    props: &HudRenderProps<'_>,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, HudAction, &mut Context<V>) + Clone + 'static,
) -> AnyElement {
    let Some(item) = props.item else {
        return div().size_0().into_any_element();
    };
    let asset = &item.asset;
    let expl = &item.explanation;
    let lang = props.language_id;
    let is_zh = lang.starts_with("zh");

    let reason_text = if is_zh {
        if expl.primary_reason_zh.is_empty() {
            "为你推荐".to_string()
        } else {
            expl.primary_reason_zh.clone()
        }
    } else if expl.primary_reason_en.is_empty() {
        "Recommended for you".to_string()
    } else {
        expl.primary_reason_en.clone()
    };

    let (kind_badge_text, kind_bg) = match asset.kind {
        AssetKind::Video => (
            tr_str(lang, "🎬 代码视频", "🎬 Code Video"),
            rgba(0x3B82F6CC),
        ),
        AssetKind::Animation => (
            tr_str(lang, "✨ 代码动画", "✨ Code Animation"),
            rgba(0x8B5CF6CC),
        ),
        AssetKind::Game => (
            tr_str(lang, "🎮 代码游戏", "🎮 Playable Game"),
            rgba(0x10B981CC),
        ),
    };

    let cb_reason = on_action.clone();
    let cb_profile = on_action.clone();
    let cb_play = on_action.clone();
    let cb_speed = on_action.clone();
    let cb_capture_game = on_action.clone();
    let cb_release_game = on_action.clone();
    let cb_restart_game = on_action.clone();

    let total_dur = props
        .duration_sec
        .or(asset.manifest.duration_sec)
        .unwrap_or(15.0)
        .max(1.0);
    let progress_frac = (props.timeline_sec / total_dur).clamp(0.0, 1.0) as f32;

    let tag_chips = asset.tags.iter().take(6).enumerate().map(|(i, tag)| {
        let clean = tag.trim().trim_start_matches('#').to_string();
        let tag_for_cb = clean.clone();
        let cb_tag = on_action.clone();
        div()
            .id(("cs-meta-tag", i))
            .cursor_pointer()
            .px(px(8.0))
            .py(px(2.5))
            .rounded(px(999.0))
            .bg(rgba(0xFFFFFF1C))
            .hover(|s| s.bg(rgba(0xFE2C55AA)))
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgba(0x25F4EEFF))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    cb_tag(this, HudAction::SelectTagFilter(tag_for_cb.clone()), cx);
                }),
            )
            .child(format!("#{clean}"))
    });

    // Clickable segmented scrubber (20 hit zones for effortless seek without complex pointer capture)
    let scrubber_segments = (0usize..20).map(|seg| {
        let seg_frac = (seg as f64 + 0.5) / 20.0;
        let filled = ((seg as f32) / 20.0) <= progress_frac;
        let cb_seek = on_action.clone();
        div()
            .id(("cs-scrub-seg", seg))
            .flex_1()
            .h(px(14.0))
            .cursor_pointer()
            .flex()
            .items_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    cb_seek(this, HudAction::SeekVideoRatio(seg_frac), cx);
                }),
            )
            .child(
                div()
                    .w_full()
                    .h(px(3.5))
                    .rounded(px(2.0))
                    .bg(if filled {
                        rgba(0xFE2C55FF)
                    } else {
                        rgba(0xFFFFFF38)
                    }),
            )
    });

    let b = expl.breakdown;

    div()
        .id("cs-bottom-hud-container")
        .absolute()
        .left(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .pt(px(28.0))
        .pb(px(12.0))
        .px(px(18.0))
        .bg(linear_gradient(
            0.0,
            linear_color_stop(rgba(0x07070BE8), 0.0),
            linear_color_stop(rgba(0x07070B00), 1.0),
        ))
        .flex()
        .flex_col()
        .gap(px(8.0))
        // Game Focus Control Banner (when active asset is a Playable Game)
        .when(asset.kind == AssetKind::Game, |col| {
            let hud = props.game_hud;
            let score = hud.map_or(0, |h| h.score);
            let high_score = hud.map_or(0, |h| h.high_score);
            let game_over = hud.is_some_and(|h| h.game_over);
            col.child(
                div()
                    .max_w(px(520.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .flex_wrap()
                    .child(if props.game_input_captured {
                        div()
                            .id("cs-game-captured-pill")
                            .cursor_pointer()
                            .px(px(12.0))
                            .py(px(5.0))
                            .rounded(px(999.0))
                            .bg(rgba(0x10B981DD))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgba(0xFFFFFFFF))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    cb_release_game(this, HudAction::ReleaseGameFocus, cx);
                                }),
                            )
                            .child(tr_str(
                                lang,
                                "🎮 操控中 · 按 Esc 切回刷视频",
                                "🎮 Playing · Press Esc to browse",
                            ))
                    } else {
                        div()
                            .id("cs-game-play-cta")
                            .cursor_pointer()
                            .px(px(12.0))
                            .py(px(5.0))
                            .rounded(px(999.0))
                            .bg(rgba(0xFE2C55E8))
                            .hover(|s| s.bg(rgba(0xFE2C55FF)))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgba(0xFFFFFFFF))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    cb_capture_game(this, HudAction::CaptureGameFocus, cx);
                                }),
                            )
                            .child(tr_str(
                                lang,
                                "▶ 点击游玩代码小游戏",
                                "▶ Click to Play Mini-Game",
                            ))
                    })
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(4.5))
                            .rounded(px(999.0))
                            .bg(rgba(0x12131CD8))
                            .border_1()
                            .border_color(rgba(0xFFFFFF22))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgba(0x25F4EEFF))
                            .child(format!(
                                "{}: {}  ·  {}: {}",
                                tr_str(lang, "得分", "Score"),
                                score,
                                tr_str(lang, "最高", "Best"),
                                high_score
                            )),
                    )
                    .when(game_over, |row| {
                        row.child(
                            div()
                                .id("cs-game-over-restart-btn")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.5))
                                .rounded(px(999.0))
                                .bg(rgba(0xF59E0BEE))
                                .text_size(px(12.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(0x111111FF))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        cb_restart_game(this, HudAction::RestartScene, cx);
                                    }),
                                )
                                .child(tr_str(lang, "↻ 再来一局 (R)", "↻ Play Again (R)")),
                        )
                    }),
            )
        })
        // Recommendation Explanation + Modality + FPS + Bot Pill Row
        .child(
            div()
                .max_w(px(560.0))
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(6.0))
                .child(
                    div()
                        .id("cs-reco-reason-pill")
                        .cursor_pointer()
                        .px(px(9.0))
                        .py(px(3.5))
                        .rounded(px(999.0))
                        .bg(rgba(0x1E2235D8))
                        .border_1()
                        .border_color(rgba(0x25F4EE66))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgba(0x25F4EEFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_reason(this, HudAction::ToggleScoreBreakdown, cx);
                            }),
                        )
                        .child(format!(
                            "✦ {} · {}",
                            reason_text,
                            if expl.traffic_tier.is_empty() {
                                "ForYou"
                            } else {
                                expl.traffic_tier.as_str()
                            }
                        )),
                )
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(3.5))
                        .rounded(px(999.0))
                        .bg(kind_bg)
                        .text_size(px(11.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(0xFFFFFFFF))
                        .child(kind_badge_text),
                )
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(3.5))
                        .rounded(px(999.0))
                        .bg(rgba(0x12131CC4))
                        .border_1()
                        .border_color(rgba(0xFFFFFF22))
                        .font_family("monospace")
                        .text_size(px(11.0))
                        .text_color(rgba(0xFFFFFFCC))
                        .child(format!(
                            "{:.0} FPS · {} · {}/{}",
                            props.measured_fps.clamp(1.0, 120.0),
                            asset.code_format,
                            props.active_index + 1,
                            props.total_items.max(1)
                        )),
                )
                .when_some(asset.creator_bot_username.as_ref(), |row, bot_un| {
                    row.child(
                        div()
                            .px(px(8.0))
                            .py(px(3.5))
                            .rounded(px(999.0))
                            .bg(rgba(0x7C3AEDAA))
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgba(0xFFFFFFFF))
                            .child(format!("🤖 @{}", bot_un.trim_start_matches('@'))),
                    )
                }),
        )
        // Optional Expandable 10-Field Recommendation ScoreBreakdown Popover
        .when(props.show_score_breakdown, |col| {
            let channels = if expl.recall_channels.is_empty() {
                "-".to_string()
            } else {
                expl.recall_channels.join(", ")
            };
            col.child(
                div()
                    .id("cs-score-breakdown-card")
                    .max_w(px(520.0))
                    .p(px(10.0))
                    .rounded(px(10.0))
                    .bg(rgba(0x11131FE8))
                    .border_1()
                    .border_color(rgba(0x25F4EE55))
                    .font_family("monospace")
                    .text_size(px(11.0))
                    .text_color(rgba(0xE2E8F0FF))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(format!(
                        "{}: {} | score={:.3}",
                        tr_str(lang, "召回通道", "Recall"),
                        channels,
                        b.final_score
                    ))
                    .child(format!(
                        "emb={:.2} swing={:.2} tag={:.2} creator={:.2} ecpm={:.2}",
                        b.embedding_sim, b.swing_cf, b.tag_kind_affinity, b.creator_affinity, b.quality_ecpm
                    ))
                    .child(format!(
                        "fresh={:.2} ucb={:.2} mmr_pen={:.2} neg_pen={:.2}",
                        b.freshness, b.exploration_bonus, b.mmr_penalty, b.negative_penalty
                    )),
            )
        })
        // Author & Title & Description
        .child(
            div()
                .max_w(px(560.0))
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .id("cs-meta-author-btn")
                        .cursor_pointer()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(0xFFFFFFFF))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_profile(this, HudAction::OpenCreatorProfile, cx);
                            }),
                        )
                        .child(format!(
                            "@{}  ·  {}",
                            asset.author_username.trim_start_matches('@'),
                            asset.title
                        )),
                )
                .when(!asset.description.trim().is_empty(), |box_el| {
                    box_el.child(
                        div()
                            .text_size(px(13.0))
                            .text_color(rgba(0xFFFFFFD6))
                            .child(asset.description.clone()),
                    )
                })
                .when(!asset.controls_hint.trim().is_empty(), |box_el| {
                    box_el.child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgba(0xFFFFFF99))
                            .child(format!("⌨ {}", asset.controls_hint)),
                    )
                }),
        )
        // Clickable #tag chips
        .when(!asset.tags.is_empty(), |col| {
            col.child(
                div()
                    .max_w(px(560.0))
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(6.0))
                    .children(tag_chips),
            )
        })
        // Bottom Timeline & Playback Control Bar
        .child(
            div()
                .id("cs-timeline-bar")
                .w_full()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .id("cs-play-pause-btn")
                        .cursor_pointer()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(999.0))
                        .bg(rgba(0xFFFFFF1F))
                        .hover(|s| s.bg(rgba(0xFFFFFF38)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                cb_play(this, HudAction::TogglePlayPause, cx);
                            }),
                        )
                        .child(tg_icon(
                            if props.paused {
                                IconName::Play
                            } else {
                                IconName::Pause
                            },
                            14.0,
                            0xFFFFFF,
                        )),
                )
                .when(asset.kind == AssetKind::Video, |row| {
                    row.child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .children(scrubber_segments),
                    )
                    .child(
                        div()
                            .font_family("monospace")
                            .text_size(px(11.5))
                            .text_color(rgba(0xFFFFFFCC))
                            .child(format!(
                                "{} / {} (×{})",
                                format_timecode(props.timeline_sec),
                                format_timecode(total_dur),
                                props.loop_count
                            )),
                    )
                    .child(
                        div()
                            .id("cs-speed-btn")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(6.0))
                            .bg(rgba(0xFFFFFF1F))
                            .hover(|s| s.bg(rgba(0xFFFFFF38)))
                            .font_family("monospace")
                            .text_size(px(11.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgba(0xFFFFFFFF))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    cb_speed(this, HudAction::CycleSpeed, cx);
                                }),
                            )
                            .child(format!("{:.1}x", props.playback_speed)),
                    )
                })
                .when(asset.kind != AssetKind::Video, |row| {
                    row.child(
                        div()
                            .flex_1()
                            .h(px(3.0))
                            .rounded(px(2.0))
                            .bg(rgba(0xFFFFFF22)),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgba(0xFFFFFF99))
                            .child(tr_str(
                                lang,
                                "滚轮 / ↑↓ / J·K 切换 · 双击点赞 · Space 暂停",
                                "Scroll / ↑↓ / J·K swipe · Double-click like · Space pause",
                            )),
                    )
                }),
        )
        .into_any_element()
}
