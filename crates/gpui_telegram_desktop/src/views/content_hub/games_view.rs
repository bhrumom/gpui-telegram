//! Apple Games / Apple Arcade macOS section (`GamesHubView`): Discover, Arcade, Charts,
//! Library, My Games, Featured Hero Banner, Game Detail with Cloud Leaderboard & Achievements,
//! launching `GameRuntimeSheet`.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::creations::{
    CatalogSortMode, CreationKind, CreationReviewStatus, GameUserState, PublicCatalogItemDto,
    UpsertCreationStateRequest, UserCreationDto,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::{
    macos_primary_pill_btn, macos_secondary_pill_btn,
};
use crate::views::content_hub::apple_primitives::{
    apple_grouped_card, apple_shelf_header, cover_artwork_box, empty_state_card_shell,
    filter_pill, hero_banner, rating_pill, render_editable_field_box, review_status_badge,
    vertical_sub_tab_pill,
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
    ("my_games", "我的创作", "My Games"),
];

pub fn normalize_games_sub_tab(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "discover" | "featured" | "home" => "discover",
        "arcade" | "play" => "arcade",
        "charts" | "top" | "leaderboard" => "charts",
        "library" | "saved" => "library",
        "my_games" | "my_creations" | "mine" => "my_games",
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
    let cid = item.creation_id.clone();
    let cid_play = item.creation_id.clone();
    let item_open = item.clone();
    let item_play = item.clone();
    let genre = item.genres.first().cloned().unwrap_or_default();

    div()
        .id(SharedString::from(format!("games-card-{cid}")))
        .cursor_pointer()
        .w(px(276.0))
        .rounded(px(14.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(if is_selected {
            TdesktopThemePalette::rgba_with_alpha(GAMES_ACCENT, 0.85)
        } else {
            pal.separator_rgba()
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
        .child(cover_artwork_box(&item.cover, 70.0, 70.0, 16.0, GAMES_ACCENT))
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
                        .text_color(rgb(pal.text_primary))
                        .truncate()
                        .child(item.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(pal.text_secondary))
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
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(GAMES_ACCENT))
                                    .child(format!("🏆 {best}")),
                            )
                        }),
                ),
        )
        .child(
            div()
                .id(SharedString::from(format!("games-card-play-{cid_play}")))
                .cursor_pointer()
                .px(px(12.0))
                .py(px(5.0))
                .rounded(px(999.0))
                .bg(rgb(GAMES_ACCENT))
                .text_size(px(11.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xFFFFFF))
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
                )
                .child(tr_str(lang, "游玩", "Play")),
        )
}

fn render_game_detail_card(
    ws: &mut ContentHubWorkspace,
    detail: UserCreationDto,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
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
    let detail_pub = detail.clone();
    let cid_lib = cid.clone();
    let cid_lb = cid.clone();

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
                    macos_secondary_pill_btn(
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
                .child(cover_artwork_box(&detail.cover, 128.0, 128.0, 24.0, GAMES_ACCENT))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(260.0))
                        .flex()
                        .flex_col()
                        .gap(px(7.0))
                        .child(
                            div()
                                .text_size(px(22.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(detail.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(GAMES_ACCENT))
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
                                            .rounded(px(999.0))
                                            .bg(TdesktopThemePalette::rgba_with_alpha(
                                                GAMES_ACCENT,
                                                0.16,
                                            ))
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(GAMES_ACCENT))
                                            .child(gp.engine_kind.as_str().to_string()),
                                    )
                                })
                                .when(game_user_state.high_score > 0, |el| {
                                    el.child(
                                        div()
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(0xFF9F0A))
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
                                .text_color(rgb(pal.text_secondary))
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
                                    div()
                                        .id("games-detail-launch-play-btn")
                                        .cursor_pointer()
                                        .px(px(22.0))
                                        .py(px(8.0))
                                        .rounded(px(999.0))
                                        .bg(rgb(GAMES_ACCENT))
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |_this, _, _, cx| {
                                                cx.emit(ContentHubEvent::LaunchGameSession {
                                                    creation: detail_launch.clone(),
                                                });
                                            }),
                                        )
                                        .child(tr_str(&lang, "▶ 立即游玩", "▶ Play Now")),
                                )
                                .child(
                                    macos_secondary_pill_btn(
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
                                )
                                .when(is_own, |el| {
                                    el.child(
                                        macos_secondary_pill_btn(
                                            "games-detail-publish-btn",
                                            tr_str(&lang, "推送审核上架", "Publish & Review"),
                                            pal,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |_this, _, _, cx| {
                                                cx.emit(
                                                    ContentHubEvent::OpenPublishModalForCreation(
                                                        detail_pub.clone(),
                                                    ),
                                                );
                                            }),
                                        ),
                                    )
                                }),
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
                            .text_color(rgb(pal.text_secondary))
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
                                        .rounded(px(9.0))
                                        .bg(if unlocked {
                                            TdesktopThemePalette::rgba_with_alpha(0x30D158, 0.16)
                                        } else {
                                            TdesktopThemePalette::rgba_with_alpha(
                                                pal.text_primary,
                                                0.05,
                                            )
                                        })
                                        .border_1()
                                        .border_color(pal.separator_rgba())
                                        .text_size(px(11.5))
                                        .text_color(rgb(pal.text_primary))
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
        // Game Center Cloud Leaderboard
        .child(
            apple_grouped_card(pal)
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(apple_shelf_header(
                            tr_str(&lang, "云端排行榜 · Game Center", "Cloud Leaderboard"),
                            Some(format!(
                                "{} {}",
                                leaderboard.len(),
                                tr_str(&lang, "名玩家", "players")
                            )),
                            pal,
                        ))
                        .child(
                            macos_secondary_pill_btn(
                                "games-leaderboard-refresh-btn",
                                tr_str(&lang, "刷新榜单", "Refresh"),
                                pal,
                            )
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
                            .text_color(rgb(pal.text_secondary))
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
                            1 => 0xFFD60A,
                            2 => 0xC7C7CC,
                            3 => 0xD97706,
                            _ => pal.text_secondary,
                        };
                        div()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.text_primary, 0.04))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .child(
                                        div()
                                            .w(px(28.0))
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(medal_color))
                                            .child(format!("#{}", entry.rank)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(pal.text_primary))
                                            .child(if entry.display_name.trim().is_empty() {
                                                format!("Player {}", entry.user_id)
                                            } else {
                                                entry.display_name.clone()
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(GAMES_ACCENT))
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

    let my_games: Vec<UserCreationDto> = ws
        .my_creations
        .iter()
        .filter(|c| c.deleted_at_ms.is_none() && c.kind == CreationKind::Game)
        .cloned()
        .collect();

    let selected_detail = ws.resolve_selected_detail_creation();
    let sort_options: &[(CatalogSortMode, &'static str, &'static str, &'static str)] = &[
        (CatalogSortMode::Featured, "games-sort-featured", "精选", "Featured"),
        (CatalogSortMode::Popular, "games-sort-popular", "热门", "Popular"),
        (CatalogSortMode::TopRated, "games-sort-rated", "高分", "Top Rated"),
        (CatalogSortMode::Newest, "games-sort-newest", "最新", "Newest"),
    ];

    div()
        .id("apple-games-hub-root")
        .size_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        // Header + Sub-nav + Search
        .child(
            div()
                .px(px(24.0))
                .py(px(14.0))
                .border_b_1()
                .border_color(pal.separator_rgba())
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(tg_icon(IconName::HubGames, 22.0, GAMES_ACCENT))
                                .child(
                                    div()
                                        .text_size(px(22.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(&lang, "游戏", "Games")),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(6.0))
                                .children(GAMES_SUB_TABS.iter().map(|&(slug, zh, en)| {
                                    vertical_sub_tab_pill(
                                        SharedString::from(format!("games-tab-{slug}")),
                                        tr_str(&lang, zh, en),
                                        active_tab == slug,
                                        GAMES_ACCENT,
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
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div().w(px(220.0)).child(
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
                                                this.focus_input(
                                                    ContentHubInputTarget::CatalogSearch,
                                                    cx,
                                                );
                                            }),
                                        ),
                                    ),
                                )
                                .child(
                                    macos_primary_pill_btn(
                                        "games-create-bot-btn",
                                        tr_str(&lang, "+ Bot 做游戏", "+ Build Game"),
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::OpenCreatorBot(Some(
                                                CreationKind::Game,
                                            )));
                                        }),
                                    ),
                                ),
                        ),
                )
                .when(selected_detail.is_none() && active_tab != "my_games", |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        filter_pill(
                                            "games-genre-all",
                                            tr_str(&lang, "全部类型", "All Categories"),
                                            selected_genre.is_none(),
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
                                    .children(genres.iter().take(6).map(|g| {
                                        let g_val = g.clone();
                                        let active = selected_genre
                                            .as_deref()
                                            .is_some_and(|s| s.eq_ignore_ascii_case(&g_val));
                                        filter_pill(
                                            SharedString::from(format!("games-genre-{g_val}")),
                                            g_val.clone(),
                                            active,
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
                                    .items_center()
                                    .gap(px(6.0))
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
                    )
                }),
        )
        // Main Scrollable Body
        .child(
            div()
                .id("apple-games-hub-scroll")
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_y_scroll()
                .p(px(24.0))
                .flex()
                .flex_col()
                .gap(px(20.0))
                .when_some(selected_detail.clone(), |el, detail| {
                    el.child(render_game_detail_card(ws, detail, cx))
                })
                .when(selected_detail.is_none() && active_tab == "my_games", |el| {
                    el.child(apple_shelf_header(
                        tr_str(&lang, "我的云端游戏作品", "My Cloud Games"),
                        Some(format!("{} {}", my_games.len(), tr_str(&lang, "款", "games"))),
                        pal,
                    ))
                    .when(my_games.is_empty(), |inner| {
                        let cta = macos_primary_pill_btn(
                            "games-my-empty-cta",
                            tr_str(&lang, "用 @CreatorStudioBot 制作第一款游戏", "Create Your First Game"),
                            pal,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _, _, cx| {
                                cx.emit(ContentHubEvent::OpenCreatorBot(Some(CreationKind::Game)));
                            }),
                        );
                        inner.child(empty_state_card_shell(
                            IconName::HubGames,
                            tr_str(&lang, "还没有云端游戏作品", "No Cloud Games Yet"),
                            tr_str(
                                &lang,
                                "使用 @CreatorStudioBot /newgame 创作益智、问答、贪吃蛇、街机或互动小说游戏。",
                                "Use @CreatorStudioBot /newgame to craft playable games saved to your cloud.",
                            ),
                            cta,
                            pal,
                        ))
                    })
                    .when(!my_games.is_empty(), |inner| {
                        inner.child(div().flex().flex_col().gap(px(10.0)).children(
                            my_games.iter().map(|g| {
                                let g_play = g.clone();
                                let g_pub = g.clone();
                                apple_grouped_card(pal)
                                    .p(px(14.0))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(14.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(12.0))
                                            .child(cover_artwork_box(
                                                &g.cover,
                                                56.0,
                                                56.0,
                                                12.0,
                                                GAMES_ACCENT,
                                            ))
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(4.0))
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap(px(8.0))
                                                            .child(
                                                                div()
                                                                    .text_size(px(15.0))
                                                                    .font_weight(
                                                                        FontWeight::SEMIBOLD,
                                                                    )
                                                                    .text_color(rgb(
                                                                        pal.text_primary,
                                                                    ))
                                                                    .child(g.title.clone()),
                                                            )
                                                            .child(review_status_badge(
                                                                g.review_status,
                                                                &lang,
                                                                pal,
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .text_color(rgb(pal.text_secondary))
                                                            .child(g.summary.clone()),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(
                                                macos_secondary_pill_btn(
                                                    SharedString::from(format!(
                                                        "my-game-play-{}",
                                                        g.creation_id
                                                    )),
                                                    tr_str(&lang, "▶ 试玩", "▶ Test Play"),
                                                    pal,
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |_this, _, _, cx| {
                                                        cx.emit(
                                                            ContentHubEvent::LaunchGameSession {
                                                                creation: g_play.clone(),
                                                            },
                                                        );
                                                    }),
                                                ),
                                            )
                                            .child(
                                                macos_primary_pill_btn(
                                                    SharedString::from(format!(
                                                        "my-game-publish-{}",
                                                        g.creation_id
                                                    )),
                                                    tr_str(&lang, "推送审核", "Publish"),
                                                    pal,
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |_this, _, _, cx| {
                                                        cx.emit(
                                                            ContentHubEvent::OpenPublishModalForCreation(
                                                                g_pub.clone(),
                                                            ),
                                                        );
                                                    }),
                                                ),
                                            ),
                                    )
                            }),
                        ))
                    })
                })
                .when(selected_detail.is_none() && active_tab != "my_games", |el| {
                    let list = if active_tab == "library" {
                        library_games.clone()
                    } else {
                        filtered_games.clone()
                    };
                    let featured_hero = list.first().cloned();

                    el.child(hero_banner(
                        "apple-games-hero-banner",
                        tr_str(&lang, "APPLE ARCADE · 精选游戏", "APPLE ARCADE · FEATURED"),
                        featured_hero
                            .as_ref()
                            .map(|g| g.title.as_str())
                            .unwrap_or_else(|| {
                                tr_str(&lang, "即点即玩的原生街机与益智游戏", "Play Native Arcade & Puzzle Games")
                            }),
                        featured_hero
                            .as_ref()
                            .map(|g| g.summary.as_str())
                            .unwrap_or_else(|| {
                                tr_str(
                                    &lang,
                                    "GPUI 原生画布渲染、成就徽章与 Game Center 云端高分榜。",
                                    "Native GPUI canvas arcade engine, achievements & cloud high-score leaderboards.",
                                )
                            }),
                        "#5856D6",
                        "#007AFF",
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
                                    let hover_bg = pal.row_hover_rgba();
                                    div()
                                        .id(SharedString::from(format!("games-chart-row-{rank}")))
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(8.0))
                                        .rounded(px(8.0))
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
                                                .flex()
                                                .items_center()
                                                .gap(px(10.0))
                                                .child(
                                                    div()
                                                        .w(px(24.0))
                                                        .text_size(px(14.0))
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(rgb(if rank < 3 {
                                                            GAMES_ACCENT
                                                        } else {
                                                            pal.text_secondary
                                                        }))
                                                        .child(format!("{}", rank + 1)),
                                                )
                                                .child(cover_artwork_box(
                                                    &item.cover,
                                                    42.0,
                                                    42.0,
                                                    10.0,
                                                    GAMES_ACCENT,
                                                ))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .child(
                                                            div()
                                                                .text_size(px(13.0))
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(rgb(pal.text_primary))
                                                                .child(item.title.clone()),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(11.0))
                                                                .text_color(rgb(pal.text_secondary))
                                                                .child(format!(
                                                                    "{} · {} {}",
                                                                    item.author_display_name,
                                                                    item.engagement_count,
                                                                    tr_str(&lang, "次游玩", "plays")
                                                                )),
                                                        ),
                                                ),
                                        )
                                        .child(rating_pill(item.rating_avg, item.rating_count, pal))
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
                                let cta = macos_primary_pill_btn(
                                    "games-empty-bot-cta",
                                    tr_str(&lang, "用 @CreatorStudioBot 创作游戏", "Build with @CreatorStudioBot"),
                                    pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_this, _, _, cx| {
                                        cx.emit(ContentHubEvent::OpenCreatorBot(Some(
                                            CreationKind::Game,
                                        )));
                                    }),
                                );
                                empty_el.child(empty_state_card_shell(
                                    IconName::HubGames,
                                    tr_str(&lang, "暂无公开上架游戏", "No Published Games Yet"),
                                    tr_str(
                                        &lang,
                                        "使用 @CreatorStudioBot 创作小游戏并审核上架后，会立即出现在这里供大家游玩。",
                                        "Games approved for public listing appear here ready to play.",
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
        )
        .into_any_element()
}
