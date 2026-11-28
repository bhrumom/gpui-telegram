//! Apple Games / Apple Arcade macOS section (`GamesHubView`): Discover, Arcade, Charts,
//! Library, My Games, Featured Hero Banner, Game Detail with Cloud Leaderboard & Achievements,
//! launching `GameRuntimeSheet`.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::creations::{
    CatalogSortMode, CreationReviewStatus, GameUserState, PublicCatalogItemDto,
    UpsertCreationStateRequest, UserCreationDto,
};

use crate::i18n::tr_str;
use crate::theme::{IconName, TdesktopThemePalette};
use crate::views::content_hub::apple_primitives::{
    apple_grouped_card, apple_shelf_header, cover_artwork_box, empty_state_card_shell,
    filter_pill, hero_banner, openbot_marketplace_nav_item, openbot_marketplace_shell,
    openbot_page_header, openbot_primary_cta_btn, openbot_secondary_btn, rating_pill,
    render_editable_field_box, review_status_badge, OPENBOT_MARKETPLACE_CONTENT_MAX_WIDTH,
};
use crate::views::content_hub::game_runtime_sheet::{extract_game_payload, GAMES_ACCENT};
use crate::views::content_hub::{
    render_community_reviews_panel, ContentHubEvent, ContentHubInputTarget, ContentHubSection,
    ContentHubWorkspace,
};

/// Canonical Apple Games / Arcade sub-navigation tabs.
pub const GAMES_SUB_TABS: &[(&str, &str, &str)] = &[
    ("discover", "探索", "Discover"),
    ("arcade", "街机", "Arcade"),
    ("charts", "排行榜", "Charts"),
    ("library", "我的游戏", "Library"),
];

pub fn normalize_games_sub_tab(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "discover" | "featured" | "home" | "my_games" | "my_creations" | "mine" => "discover",
        "arcade" | "play" => "arcade",
        "charts" | "top" | "leaderboard" => "charts",
        "library" | "saved" => "library",
        _ => "discover",
    }
}

fn render_game_card(
    item: &PublicCatalogItemDto,
    best_score_opt: Option<i64>,
    is_selected: bool,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let card_hover = ob.bg_control_hover();
    let cid = item.creation_id.clone();
    let cid_play = item.creation_id.clone();
    let item_open = item.clone();
    let item_play = item.clone();
    let genre = item.genres.first().cloned().unwrap_or_default();

    div()
        .id(SharedString::from(format!("games-card-{cid}")))
        .cursor_pointer()
        .w(px(276.0))
        .max_w_full()
        .rounded(px(14.0))
        .bg(ob.bg_surface())
        .hover(move |s| s.bg(card_hover))
        .border_1()
        .border_color(if is_selected {
            ob.border_strong_hairline()
        } else {
            ob.frame_card_border()
        })
        .p(px(12.0))
        .flex()
        .items_center()
        .gap(px(12.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.selected_creation_id = Some(cid.clone());
                this.selected_catalog_item = Some(item_open.clone());
                if this
                    .selected_detail_creation
                    .as_ref()
                    .is_some_and(|d| d.creation_id != cid)
                {
                    this.selected_detail_creation = this.find_my_creation(&cid).cloned();
                }
                cx.emit(ContentHubEvent::OpenCreationDetail {
                    section: ContentHubSection::Games,
                    creation_id: cid.clone(),
                    action: Some("open".to_string()),
                });
                cx.notify();
            }),
        )
        .child(cover_artwork_box(&item.cover, 70.0, 70.0, 14.0, 0x3D3B52))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .truncate()
                        .child(item.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(ob.text_muted())
                        .truncate()
                        .child(if genre.is_empty() {
                            item.author_display_name.clone()
                        } else {
                            format!("{} · {}", item.author_display_name, genre)
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(rating_pill(item.rating_avg, item.rating_count, pal))
                        .when_some(best_score_opt.filter(|&s| s > 0), |el, best| {
                            el.child(
                                div()
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_secondary())
                                    .child(format!("🏆 {best}")),
                            )
                        }),
                ),
        )
        .child(
            openbot_primary_cta_btn(
                SharedString::from(format!("games-card-play-{cid_play}")),
                tr_str(lang, "游玩", "Play"),
                pal,
            )
            .flex_shrink_0()
            .px(px(11.0))
            .py(px(5.0))
            .text_size(px(11.5))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.selected_creation_id = Some(cid_play.clone());
                    this.selected_catalog_item = Some(item_play.clone());
                    cx.emit(ContentHubEvent::OpenCreationDetail {
                        section: ContentHubSection::Games,
                        creation_id: cid_play.clone(),
                        action: Some("play".to_string()),
                    });
                    cx.notify();
                }),
            ),
        )
}

fn render_game_detail_card(
    ws: &mut ContentHubWorkspace,
    detail: UserCreationDto,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let ob = pal.openbot_palette();
    let lang = ws.language_id.clone();
    let cid = detail.creation_id.clone();
    let game_payload = extract_game_payload(&detail);
    let saved_state = ws.my_creation_states.get(&cid).cloned();
    let in_library = saved_state.as_ref().is_some_and(|s| s.in_library);
    let game_user_state = saved_state
        .as_ref()
        .and_then(|s| serde_json::from_value::<GameUserState>(s.state_json.clone()).ok())
        .unwrap_or_default();
    let is_own = ws.find_my_creation(&cid).is_some();
    let is_published = detail.review_status == CreationReviewStatus::Published;
    let leaderboard = ws.active_leaderboard.clone();

    let detail_launch = detail.clone();
    let cid_lib = cid.clone();
    let cid_lb = cid.clone();
    let succ_soft = gpui::Rgba { a: 0.14, ..ob.success() };

    div()
        .id("apple-games-detail-panel")
        .flex()
        .flex_col()
        .gap(px(16.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    openbot_secondary_btn(
                        "games-detail-back-btn",
                        tr_str(&lang, "← 返回游戏", "← Back to Games"),
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.selected_creation_id = None;
                            this.selected_catalog_item = None;
                            this.selected_detail_creation = None;
                            cx.notify();
                        }),
                    ),
                )
                .when(is_own, |el| {
                    el.child(review_status_badge(detail.review_status, &lang, pal))
                }),
        )
        // Game Hero Detail Card
        .child(
            apple_grouped_card(pal)
                .p(px(20.0))
                .flex()
                .flex_wrap()
                .items_start()
                .gap(px(20.0))
                .child(cover_artwork_box(&detail.cover, 128.0, 128.0, 20.0, 0x3D3B52))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(240.0))
                        .max_w_full()
                        .flex()
                        .flex_col()
                        .gap(px(7.0))
                        .child(
                            div()
                                .text_size(px(22.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.text_primary())
                                .child(detail.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_secondary())
                                .child(
                                    game_payload
                                        .as_ref()
                                        .filter(|g| !g.developer.trim().is_empty())
                                        .map(|g| g.developer.clone())
                                        .unwrap_or_else(|| detail.author_display_name.clone()),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.0))
                                .child(rating_pill(detail.rating_avg, detail.rating_count, pal))
                                .when_some(game_payload.as_ref(), |el, gp| {
                                    el.child(
                                        div()
                                            .px(px(8.0))
                                            .py(px(2.0))
                                            .rounded(px(6.0))
                                            .bg(ob.bg_control_active())
                                            .border_1()
                                            .border_color(ob.border_hairline())
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(ob.text_secondary())
                                            .child(gp.engine_kind.as_str().to_string()),
                                    )
                                })
                                .when(game_user_state.high_score > 0, |el| {
                                    el.child(
                                        div()
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(ob.warning())
                                            .child(format!(
                                                "{}: {}",
                                                tr_str(&lang, "个人最高分", "Personal Best"),
                                                game_user_state.high_score
                                            )),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(ob.text_secondary())
                                .child(detail.summary.clone()),
                        )
                        .child(
                            div()
                                .pt(px(6.0))
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    openbot_primary_cta_btn(
                                        "games-detail-launch-play-btn",
                                        tr_str(&lang, "▶ 立即游玩", "▶ Play Now"),
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::LaunchGameSession {
                                                creation: detail_launch.clone(),
                                            });
                                        }),
                                    ),
                                )
                                .child(
                                    openbot_secondary_btn(
                                        "games-detail-library-btn",
                                        if in_library {
                                            tr_str(&lang, "✓ 已在游戏库", "✓ In Library")
                                        } else {
                                            tr_str(&lang, "+ 加入游戏库", "+ Add to Library")
                                        },
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::UpsertCreationState {
                                                creation_id: cid_lib.clone(),
                                                req: UpsertCreationStateRequest {
                                                    in_library: Some(!in_library),
                                                    ..Default::default()
                                                },
                                            });
                                        }),
                                    ),
                                ),
                        ),
                ),
        )
        // Rules & Controls + Achievements
        .when_some(game_payload.clone(), |el, gp| {
            el.child(
                apple_grouped_card(pal)
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(apple_shelf_header(
                        tr_str(&lang, "玩法说明与操作", "Rules & Controls"),
                        (!gp.controls_hint.trim().is_empty()).then(|| gp.controls_hint.clone()),
                        pal,
                    ))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .line_height(px(20.0))
                            .text_color(ob.text_secondary())
                            .child(gp.rules_markdown.clone()),
                    )
                    .when(!gp.achievements.is_empty(), |inner| {
                        inner.child(
                            div()
                                .pt(px(6.0))
                                .flex()
                                .flex_wrap()
                                .gap(px(8.0))
                                .children(gp.achievements.iter().map(|ach| {
                                    let unlocked = game_user_state
                                        .unlocked_achievements
                                        .contains(&ach.achievement_id);
                                    div()
                                        .px(px(10.0))
                                        .py(px(6.0))
                                        .rounded(px(8.0))
                                        .bg(if unlocked { succ_soft } else { ob.bg_canvas() })
                                        .border_1()
                                        .border_color(ob.border_hairline())
                                        .text_size(px(11.5))
                                        .text_color(ob.text_primary())
                                        .child(format!(
                                            "{} {} — {}",
                                            if ach.icon_emoji.trim().is_empty() {
                                                "🏆"
                                            } else {
                                                ach.icon_emoji.as_str()
                                            },
                                            ach.title,
                                            ach.description
                                        ))
                                })),
                        )
                    }),
            )
        })
        // Cloud Leaderboard
        .child(
            apple_grouped_card(pal)
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(apple_shelf_header(
                                    tr_str(&lang, "云端排行榜", "Cloud Leaderboard"),
                                    Some(format!(
                                        "{} {}",
                                        leaderboard.len(),
                                        tr_str(&lang, "名玩家", "players")
                                    )),
                                    pal,
                                )),
                        )
                        .child(
                            openbot_secondary_btn(
                                "games-leaderboard-refresh-btn",
                                tr_str(&lang, "刷新榜单", "Refresh"),
                                pal,
                            )
                            .flex_shrink_0()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |_this, _, _, cx| {
                                    cx.emit(ContentHubEvent::LoadGameLeaderboard {
                                        creation_id: cid_lb.clone(),
                                    });
                                }),
                            ),
                        ),
                )
                .when(leaderboard.is_empty(), |el| {
                    el.child(
                        div()
                            .text_size(px(12.0))
                            .text_color(ob.text_muted())
                            .child(tr_str(
                                &lang,
                                "暂无上榜记录，点击「立即游玩」成为第一位冠军！",
                                "No scores yet — play now to claim #1 on the leaderboard!",
                            )),
                    )
                })
                .when(!leaderboard.is_empty(), |el| {
                    el.children(leaderboard.iter().take(10).map(|entry| {
                        let medal_color = match entry.rank {
                            1 => ob.warning(),
                            2 => ob.text_primary(),
                            3 => ob.text_secondary(),
                            _ => ob.text_muted(),
                        };
                        div()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_canvas())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .w(px(28.0))
                                            .flex_shrink_0()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(medal_color)
                                            .child(format!("#{}", entry.rank)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(ob.text_primary())
                                            .child(if entry.display_name.trim().is_empty() {
                                                format!("Player {}", entry.user_id)
                                            } else {
                                                entry.display_name.clone()
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ob.text_primary())
                                    .child(format!("{}", entry.score)),
                            )
                    }))
                }),
        )
        // Community Ratings & Reviews
        .child(render_community_reviews_panel(
            ws,
            &cid,
            is_published,
            GAMES_ACCENT,
            cx,
        ))
        .into_any_element()
}

/// Renders the Apple Games / Arcade hub section (`GamesHubView`).
pub fn render_games_hub_view(
    ws: &mut ContentHubWorkspace,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.clone();
    let active_tab = normalize_games_sub_tab(&ws.active_sub_tab);
    let search_focused = ws.active_input == ContentHubInputTarget::CatalogSearch;
    let search_field = ws.catalog_search.clone();
    let sort_mode = ws.sort_mode;
    let selected_genre = ws.selected_genre_filter.clone();
    let selected_id = ws.selected_creation_id.clone();

    let all_games = ws.active_catalog_items(ContentHubSection::Games);
    let mut genres: Vec<String> = Vec::new();
    for item in &ws.games_catalog {
        for g in &item.genres {
            if !g.trim().is_empty() && !genres.iter().any(|x| x.eq_ignore_ascii_case(g)) {
                genres.push(g.clone());
            }
        }
    }

    let filtered_games: Vec<PublicCatalogItemDto> = all_games
        .into_iter()
        .filter(|g| {
            selected_genre
                .as_deref()
                .is_none_or(|f| g.genres.iter().any(|gg| gg.eq_ignore_ascii_case(f)))
        })
        .collect();

    let library_games: Vec<PublicCatalogItemDto> = filtered_games
        .iter()
        .filter(|g| {
            ws.my_creation_states
                .get(&g.creation_id)
                .is_some_and(|st| st.in_library || st.progress_percent > 0.0)
        })
        .cloned()
        .collect();

    let selected_detail = ws.resolve_selected_detail_creation();
    let sort_options: &[(CatalogSortMode, &'static str, &'static str, &'static str)] = &[
        (CatalogSortMode::Featured, "games-sort-featured", "精选", "Featured"),
        (CatalogSortMode::Popular, "games-sort-popular", "热门", "Popular"),
        (CatalogSortMode::TopRated, "games-sort-rated", "高分", "Top Rated"),
        (CatalogSortMode::Newest, "games-sort-newest", "最新", "Newest"),
    ];

    let ob = pal.openbot_palette();
    let root_label = tr_str(&lang, "游戏", "Games");
    let hover_crumb = ob.bg_control();
    let text_pri = ob.text_primary();
    let header = if let Some(ref detail) = selected_detail {
        let parent_btn = div()
            .id("games-header-parent-crumb")
            .cursor_pointer()
            .px(px(6.0))
            .py(px(3.0))
            .rounded(px(6.0))
            .text_size(px(13.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(ob.text_muted())
            .hover(move |s| s.bg(hover_crumb).text_color(text_pri))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.selected_creation_id = None;
                    this.selected_catalog_item = None;
                    this.selected_detail_creation = None;
                    cx.notify();
                }),
            )
            .child(root_label);
        openbot_page_header(detail.title.clone(), Some(parent_btn), None, pal)
    } else {
        let refresh_btn = openbot_secondary_btn(
            "games-refresh-btn",
            tr_str(&lang, "刷新", "Refresh"),
            pal,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_this, _, _, cx| {
                cx.emit(ContentHubEvent::RefreshCatalog(ContentHubSection::Games));
            }),
        )
        .into_any_element();
        openbot_page_header(root_label, None, Some(refresh_btn), pal)
    };

    let left_nav = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(
            render_editable_field_box(
                "content-hub-catalog-search-input",
                &search_field,
                tr_str(&lang, "搜索游戏与街机…", "Search Games…"),
                search_focused,
                false,
                pal,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.focus_input(ContentHubInputTarget::CatalogSearch, cx);
                }),
            ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .px(px(10.0))
                        .pb(px(4.0))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.text_muted())
                        .child(tr_str(&lang, "浏览", "Browse")),
                )
                .children(GAMES_SUB_TABS.iter().map(|&(slug, zh, en)| {
                    let count = match slug {
                        "library" => Some(library_games.len()),
                        "discover" | "arcade" => Some(filtered_games.len()),
                        _ => None,
                    };
                    openbot_marketplace_nav_item(
                        SharedString::from(format!("games-tab-{slug}")),
                        tr_str(&lang, zh, en),
                        active_tab == slug,
                        count,
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.active_sub_tab = slug.to_string();
                            this.selected_creation_id = None;
                            this.selected_catalog_item = None;
                            this.selected_detail_creation = None;
                            cx.notify();
                        }),
                    )
                })),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .px(px(10.0))
                        .pb(px(4.0))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.text_muted())
                        .child(tr_str(&lang, "类型", "Categories")),
                )
                .child(
                    openbot_marketplace_nav_item(
                        "games-genre-all",
                        tr_str(&lang, "全部类型", "All Categories"),
                        selected_genre.is_none(),
                        Some(ws.games_catalog.len()),
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.selected_genre_filter = None;
                            cx.notify();
                        }),
                    ),
                )
                .children(genres.iter().take(8).map(|g| {
                    let g_val = g.clone();
                    let active = selected_genre
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(&g_val));
                    openbot_marketplace_nav_item(
                        SharedString::from(format!("games-genre-{g_val}")),
                        g_val.clone(),
                        active,
                        None,
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.selected_genre_filter = Some(g_val.clone());
                            cx.notify();
                        }),
                    )
                })),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .px(px(10.0))
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.text_muted())
                        .child(tr_str(&lang, "排序", "Sort")),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.0))
                        .px(px(4.0))
                        .children(sort_options.iter().map(|&(mode, id, zh, en)| {
                            filter_pill(id, tr_str(&lang, zh, en), sort_mode == mode, pal)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.sort_mode = mode;
                                        cx.emit(ContentHubEvent::RefreshCatalog(
                                            ContentHubSection::Games,
                                        ));
                                        cx.notify();
                                    }),
                                )
                        })),
                ),
        );

    let body_scroll = div().id("apple-games-hub-scroll").child(
        div()
            .w_full()
            .max_w(px(OPENBOT_MARKETPLACE_CONTENT_MAX_WIDTH))
            .mx_auto()
            .px(px(24.0))
            .py(px(20.0))
            .flex()
            .flex_col()
            .gap(px(20.0))
            .when_some(selected_detail.clone(), |el, detail| {
                el.child(render_game_detail_card(ws, detail, cx))
            })
            .when(selected_detail.is_none(), |el| {
                let list = if active_tab == "library" {
                    library_games.clone()
                } else {
                    filtered_games.clone()
                };
                let featured_hero = list.first().cloned();

                el.child(hero_banner(
                    "apple-games-hero-banner",
                    tr_str(&lang, "游戏 · 精选推荐", "GAMES · FEATURED"),
                    featured_hero
                        .as_ref()
                        .map(|g| g.title.as_str())
                        .unwrap_or_else(|| {
                            tr_str(
                                &lang,
                                "即点即玩的原生街机与益智游戏",
                                "Play Native Arcade & Puzzle Games",
                            )
                        }),
                    featured_hero
                        .as_ref()
                        .map(|g| g.summary.as_str())
                        .unwrap_or_else(|| {
                            tr_str(
                                &lang,
                                "GPUI 原生画布渲染、成就徽章与云端高分榜。",
                                "Native GPUI canvas arcade engine, achievements & cloud high-score leaderboards.",
                            )
                        }),
                    "#212121",
                    "#1A1A1A",
                    IconName::HubGames,
                    pal,
                ))
                .when(active_tab == "charts" && !list.is_empty(), |inner| {
                    inner.child(
                        apple_grouped_card(pal)
                            .p(px(16.0))
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(apple_shelf_header(
                                tr_str(&lang, "游戏总榜 · Top Charts", "Top Games Charts"),
                                Some(format!("Top {}", list.len().min(15))),
                                pal,
                            ))
                            .children(list.iter().take(15).enumerate().map(|(rank, item)| {
                                let cid = item.creation_id.clone();
                                let item_c = item.clone();
                                let hover_bg = ob.bg_control_hover();
                                div()
                                    .id(SharedString::from(format!("games-chart-row-{rank}")))
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(8.0))
                                    .rounded(px(10.0))
                                    .hover(move |s| s.bg(hover_bg))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(12.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.selected_creation_id = Some(cid.clone());
                                            this.selected_catalog_item = Some(item_c.clone());
                                            cx.emit(ContentHubEvent::OpenCreationDetail {
                                                section: ContentHubSection::Games,
                                                creation_id: cid.clone(),
                                                action: Some("open".to_string()),
                                            });
                                            cx.notify();
                                        }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .flex()
                                            .items_center()
                                            .gap(px(10.0))
                                            .child(
                                                div()
                                                    .w(px(24.0))
                                                    .flex_shrink_0()
                                                    .text_size(px(14.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(if rank < 3 {
                                                        ob.text_primary()
                                                    } else {
                                                        ob.text_muted()
                                                    })
                                                    .child(format!("{}", rank + 1)),
                                            )
                                            .child(cover_artwork_box(
                                                &item.cover,
                                                42.0,
                                                42.0,
                                                10.0,
                                                0x3D3B52,
                                            ))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.0))
                                                    .flex()
                                                    .flex_col()
                                                    .child(
                                                        div()
                                                            .truncate()
                                                            .text_size(px(13.0))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(ob.text_primary())
                                                            .child(item.title.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .truncate()
                                                            .text_size(px(11.0))
                                                            .text_color(ob.text_muted())
                                                            .child(format!(
                                                                "{} · {} {}",
                                                                item.author_display_name,
                                                                item.engagement_count,
                                                                tr_str(&lang, "次游玩", "plays")
                                                            )),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div().flex_shrink_0().child(rating_pill(
                                            item.rating_avg,
                                            item.rating_count,
                                            pal,
                                        )),
                                    )
                            })),
                    )
                })
                .when(active_tab != "charts", |inner| {
                    inner
                        .child(apple_shelf_header(
                            match active_tab {
                                "arcade" => tr_str(&lang, "原生街机与小游戏", "Playable Arcade"),
                                "library" => tr_str(&lang, "我的游戏库", "My Game Library"),
                                _ => tr_str(&lang, "精选公开游戏", "Featured Published Games"),
                            },
                            Some(format!("{} {}", list.len(), tr_str(&lang, "款", "games"))),
                            pal,
                        ))
                        .when(list.is_empty(), |empty_el| {
                            let cta = openbot_secondary_btn(
                                "games-empty-refresh-btn",
                                tr_str(&lang, "刷新游戏", "Refresh Games"),
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _, _, cx| {
                                    cx.emit(ContentHubEvent::RefreshCatalog(
                                        ContentHubSection::Games,
                                    ));
                                }),
                            );
                            empty_el.child(empty_state_card_shell(
                                IconName::HubGames,
                                tr_str(&lang, "暂无公开上架游戏", "No Published Games Yet"),
                                tr_str(
                                    &lang,
                                    "在「消息」中直接与 @CreatorStudioBot 对话即可创作并发布游戏，审核上架后会显示在这里。",
                                    "Chat with @CreatorStudioBot directly in Messages to build and publish games. Published games appear here.",
                                ),
                                cta,
                                pal,
                            ))
                        })
                        .when(!list.is_empty(), |grid_el| {
                            grid_el.child(div().flex().flex_wrap().gap(px(14.0)).children(
                                list.iter().map(|item| {
                                    let best = ws
                                        .my_creation_states
                                        .get(&item.creation_id)
                                        .and_then(|s| {
                                            serde_json::from_value::<GameUserState>(
                                                s.state_json.clone(),
                                            )
                                            .ok()
                                        })
                                        .map(|gs| gs.high_score);
                                    let is_sel = selected_id.as_deref()
                                        == Some(item.creation_id.as_str());
                                    render_game_card(item, best, is_sel, pal, &lang, cx)
                                }),
                            ))
                        })
                })
            }),
    );

    div()
        .id("apple-games-hub-root")
        .size_full()
        .bg(ob.bg_canvas())
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(header)
        .child(openbot_marketplace_shell(left_nav, body_scroll, pal))
        .into_any_element()
}
