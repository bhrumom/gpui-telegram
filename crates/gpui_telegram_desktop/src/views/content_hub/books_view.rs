//! Apple Books macOS section (`BooksHubView`): Reading Now, Library, Book Store, My Books,
//! 2:3 cover grid, Continue Reading progress shelf, and Book Detail sheet launching `BookReaderView`.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::creations::{
    BookPayload, CatalogSortMode, CreationKind, CreationPayload, CreationReviewStatus,
    PublicCatalogItemDto, UpsertCreationStateRequest, UserCreationDto,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::{
    macos_primary_pill_btn, macos_secondary_pill_btn,
};
use crate::views::content_hub::apple_primitives::{
    apple_grouped_card, apple_progress_bar, apple_shelf_header, cover_artwork_box,
    empty_state_card_shell, filter_pill, hero_banner, rating_pill, render_editable_field_box,
    review_status_badge, vertical_sub_tab_pill,
};
use crate::views::content_hub::book_reader::BookReaderState;
use crate::views::content_hub::{
    render_community_reviews_panel, ContentHubEvent, ContentHubInputTarget, ContentHubSection,
    ContentHubWorkspace,
};

const BOOKS_ACCENT: u32 = 0xFF9500;

/// Canonical sub-tabs inside Apple Books (`BooksHubView`).
pub const BOOKS_SUB_TABS: &[(&str, &str, &str)] = &[
    ("reading_now", "正在阅读", "Reading Now"),
    ("library", "书库", "Library"),
    ("book_store", "书城", "Book Store"),
    ("my_books", "我的图书", "My Books"),
];

pub fn normalize_books_sub_tab(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "reading_now" | "reading" | "now" => "reading_now",
        "library" | "saved" => "library",
        "book_store" | "store" | "discover" => "book_store",
        "my_books" | "my_creations" | "mine" => "my_books",
        _ => "reading_now",
    }
}

fn extract_book_payload(creation: &UserCreationDto) -> BookPayload {
    match &creation.payload {
        CreationPayload::Book(bp) => bp.clone(),
        _ => BookPayload {
            author: creation.author_display_name.clone(),
            series_name: None,
            series_index: None,
            blurb: creation.summary.clone(),
            estimated_read_mins: 5,
            word_count: creation.summary.chars().count() as u32,
            chapters: Vec::new(),
        },
    }
}

fn render_book_cover_card_2_3(
    item: &PublicCatalogItemDto,
    progress_opt: Option<f32>,
    in_library: bool,
    is_selected: bool,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Stateful<Div> {
    let cid = item.creation_id.clone();
    let item_clone = item.clone();
    let genre_tag = item.genres.first().cloned().unwrap_or_default();
    div()
        .id(SharedString::from(format!("books-card-{cid}")))
        .cursor_pointer()
        .w(px(156.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(if is_selected {
            TdesktopThemePalette::rgba_with_alpha(BOOKS_ACCENT, 0.85)
        } else {
            pal.separator_rgba()
        })
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.selected_creation_id = Some(cid.clone());
                this.selected_catalog_item = Some(item_clone.clone());
                if this
                    .selected_detail_creation
                    .as_ref()
                    .is_some_and(|d| d.creation_id != cid)
                {
                    this.selected_detail_creation = this.find_my_creation(&cid).cloned();
                }
                cx.emit(ContentHubEvent::OpenCreationDetail {
                    section: ContentHubSection::Books,
                    creation_id: cid.clone(),
                    action: Some("open".to_string()),
                });
                cx.notify();
            }),
        )
        // Classic Apple Books 2:3 cover artwork (136x204) with book spine highlight
        .child(
            div()
                .relative()
                .w(px(136.0))
                .h(px(204.0))
                .child(cover_artwork_box(&item.cover, 136.0, 204.0, 8.0, BOOKS_ACCENT))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(5.0))
                        .w(px(2.0))
                        .h_full()
                        .bg(TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.22)),
                )
                .when(in_library, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(6.0))
                            .right(px(6.0))
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(999.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(0x000000, 0.58))
                            .text_size(px(10.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xFFFFFF))
                            .child(tr_str(lang, "在库", "Saved")),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(13.0))
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
                        .child(item.author_display_name.clone()),
                )
                .child(
                    div()
                        .pt(px(2.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(4.0))
                        .child(rating_pill(item.rating_avg, item.rating_count, pal))
                        .when(!genre_tag.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .truncate()
                                    .child(genre_tag),
                            )
                        }),
                ),
        )
        .when_some(progress_opt, |el, pct| {
            el.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(apple_progress_bar(pct / 100.0, BOOKS_ACCENT, pal))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(pal.text_secondary))
                            .child(format!("{:.0}%", pct.clamp(0.0, 100.0))),
                    ),
            )
        })
}

fn render_book_detail_card(
    ws: &mut ContentHubWorkspace,
    detail: UserCreationDto,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.clone();
    let payload = extract_book_payload(&detail);
    let cid = detail.creation_id.clone();
    let saved_state = ws.my_creation_states.get(&cid).cloned();
    let in_library = saved_state.as_ref().is_some_and(|s| s.in_library);
    let favorite = saved_state.as_ref().is_some_and(|s| s.favorite);
    let progress = saved_state.as_ref().map(|s| s.progress_percent).unwrap_or(0.0);
    let is_published_public = detail.review_status == CreationReviewStatus::Published;
    let is_own = ws.find_my_creation(&cid).is_some();

    let detail_for_read = detail.clone();
    let payload_for_read = payload.clone();
    let cid_for_lib = cid.clone();
    let cid_for_fav = cid.clone();
    let detail_for_pub = detail.clone();

    div()
        .id("apple-books-detail-panel")
        .flex()
        .flex_col()
        .gap(px(16.0))
        // Back bar
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    macos_secondary_pill_btn(
                        "books-detail-back-btn",
                        tr_str(&lang, "← 返回图书", "← Back to Books"),
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
        // Book Hero Card (Apple Books product header)
        .child(
            apple_grouped_card(pal)
                .p(px(20.0))
                .flex()
                .flex_wrap()
                .items_start()
                .gap(px(20.0))
                .child(cover_artwork_box(&detail.cover, 140.0, 210.0, 10.0, BOOKS_ACCENT))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(260.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .when_some(payload.series_name.clone(), |el, series| {
                            let label = match payload.series_index {
                                Some(idx) => format!("{series} · #{idx}"),
                                None => series,
                            };
                            el.child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(BOOKS_ACCENT))
                                    .child(label),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(22.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(detail.title.clone()),
                        )
                        .when(!detail.subtitle.trim().is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(14.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(detail.subtitle.clone()),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(BOOKS_ACCENT))
                                .child(if payload.author.trim().is_empty() {
                                    detail.author_display_name.clone()
                                } else {
                                    payload.author.clone()
                                }),
                        )
                        // Metadata strip
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.0))
                                .child(rating_pill(detail.rating_avg, detail.rating_count, pal))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!(
                                            "{} {} · ~{} {} · {} {}",
                                            payload.chapters.len().max(1),
                                            tr_str(&lang, "章", "ch"),
                                            payload.estimated_read_mins.max(1),
                                            tr_str(&lang, "分钟", "min"),
                                            payload.word_count,
                                            tr_str(&lang, "字", "words")
                                        )),
                                )
                                .child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(1.0))
                                        .rounded(px(5.0))
                                        .border_1()
                                        .border_color(pal.separator_rgba())
                                        .text_size(px(10.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(detail.age_rating.clone()),
                                ),
                        )
                        .when(progress > 0.0, |el| {
                            el.child(
                                div()
                                    .w(px(240.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(apple_progress_bar(progress / 100.0, BOOKS_ACCENT, pal))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(pal.text_secondary))
                                            .child(format!(
                                                "{} {:.0}%",
                                                tr_str(&lang, "阅读进度", "Reading progress"),
                                                progress
                                            )),
                                    ),
                            )
                        })
                        // Primary & secondary CTA buttons
                        .child(
                            div()
                                .pt(px(6.0))
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .id("books-detail-open-reader-btn")
                                        .cursor_pointer()
                                        .px(px(18.0))
                                        .py(px(8.0))
                                        .rounded(px(999.0))
                                        .bg(rgb(BOOKS_ACCENT))
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                let saved = this
                                                    .my_creation_states
                                                    .get(&detail_for_read.creation_id);
                                                let reader = BookReaderState::open(
                                                    &detail_for_read,
                                                    payload_for_read.clone(),
                                                    saved,
                                                    this.palette,
                                                );
                                                let req = reader.to_upsert_state_request();
                                                let id = reader.creation_id.clone();
                                                this.book_reader = Some(reader);
                                                cx.emit(ContentHubEvent::UpsertCreationState {
                                                    creation_id: id,
                                                    req,
                                                });
                                                cx.notify();
                                            }),
                                        )
                                        .child(if progress > 0.0 {
                                            tr_str(&lang, "继续阅读", "Continue Reading")
                                        } else {
                                            tr_str(&lang, "开始阅读", "Read Now")
                                        }),
                                )
                                .child(
                                    macos_secondary_pill_btn(
                                        "books-detail-toggle-library-btn",
                                        if in_library {
                                            tr_str(&lang, "✓ 已在书库", "✓ In Library")
                                        } else {
                                            tr_str(&lang, "+ 加入书库", "+ Want to Read")
                                        },
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::UpsertCreationState {
                                                creation_id: cid_for_lib.clone(),
                                                req: UpsertCreationStateRequest {
                                                    in_library: Some(!in_library),
                                                    ..Default::default()
                                                },
                                            });
                                        }),
                                    ),
                                )
                                .child(
                                    macos_secondary_pill_btn(
                                        "books-detail-toggle-fav-btn",
                                        if favorite {
                                            tr_str(&lang, "♥ 已收藏", "♥ Loved")
                                        } else {
                                            tr_str(&lang, "♡ 收藏", "♡ Love")
                                        },
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::UpsertCreationState {
                                                creation_id: cid_for_fav.clone(),
                                                req: UpsertCreationStateRequest {
                                                    favorite: Some(!favorite),
                                                    ..Default::default()
                                                },
                                            });
                                        }),
                                    ),
                                )
                                .when(is_own, |el| {
                                    el.child(
                                        macos_secondary_pill_btn(
                                            "books-detail-publish-btn",
                                            tr_str(&lang, "推送审核上架", "Publish & Review"),
                                            pal,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |_this, _, _, cx| {
                                                cx.emit(
                                                    ContentHubEvent::OpenPublishModalForCreation(
                                                        detail_for_pub.clone(),
                                                    ),
                                                );
                                            }),
                                        ),
                                    )
                                }),
                        )
                        // Blurb
                        .child(
                            div()
                                .pt(px(8.0))
                                .text_size(px(13.0))
                                .line_height(px(20.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(if !payload.blurb.trim().is_empty() {
                                    payload.blurb.clone()
                                } else {
                                    detail.summary.clone()
                                }),
                        ),
                ),
        )
        // Table of Contents preview (clickable chapters jumping straight into BookReader)
        .when(!payload.chapters.is_empty(), |el| {
            let detail_toc = detail.clone();
            let payload_toc = payload.clone();
            el.child(
                apple_grouped_card(pal)
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(apple_shelf_header(
                        tr_str(&lang, "章节目录", "Table of Contents"),
                        Some(format!(
                            "{} {}",
                            payload.chapters.len(),
                            tr_str(&lang, "章", "chapters")
                        )),
                        pal,
                    ))
                    .children(payload.chapters.iter().enumerate().map(|(idx, ch)| {
                        let d = detail_toc.clone();
                        let p = payload_toc.clone();
                        let hover_bg = pal.row_hover_rgba();
                        div()
                            .id(SharedString::from(format!("books-detail-ch-{idx}")))
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(8.0))
                            .hover(move |s| s.bg(hover_bg))
                            .flex()
                            .items_center()
                            .justify_between()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    let saved = this.my_creation_states.get(&d.creation_id);
                                    let mut reader = BookReaderState::open(
                                        &d,
                                        p.clone(),
                                        saved,
                                        this.palette,
                                    );
                                    reader.jump_to_chapter(idx);
                                    let req = reader.to_upsert_state_request();
                                    let id = reader.creation_id.clone();
                                    this.book_reader = Some(reader);
                                    cx.emit(ContentHubEvent::UpsertCreationState {
                                        creation_id: id,
                                        req,
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(pal.text_primary))
                                    .child(format!("{}. {}", ch.chapter_number, ch.title)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(format!(
                                        "{} {}",
                                        ch.pages.len().max(1),
                                        tr_str(&lang, "页", "pg")
                                    )),
                            )
                    })),
            )
        })
        // Community Ratings & Reviews
        .child(render_community_reviews_panel(
            ws,
            &cid,
            is_published_public,
            BOOKS_ACCENT,
            cx,
        ))
        .into_any_element()
}

/// Renders the Apple Books hub section (`BooksHubView`).
pub fn render_books_hub_view(
    ws: &mut ContentHubWorkspace,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.clone();
    let active_tab = normalize_books_sub_tab(&ws.active_sub_tab);
    let search_focused = ws.active_input == ContentHubInputTarget::CatalogSearch;
    let search_field = ws.catalog_search.clone();
    let sort_mode = ws.sort_mode;
    let selected_genre = ws.selected_genre_filter.clone();

    let all_books = ws.active_catalog_items(ContentHubSection::Books);
    let mut genres: Vec<String> = Vec::new();
    for item in &ws.books_catalog {
        for g in &item.genres {
            if !g.trim().is_empty() && !genres.iter().any(|x| x.eq_ignore_ascii_case(g)) {
                genres.push(g.clone());
            }
        }
    }

    let filtered_books: Vec<PublicCatalogItemDto> = all_books
        .into_iter()
        .filter(|b| {
            selected_genre.as_deref().is_none_or(|g| {
                b.genres
                    .iter()
                    .any(|bg| bg.eq_ignore_ascii_case(g))
            })
        })
        .collect();

    let my_books: Vec<UserCreationDto> = ws
        .my_creations
        .iter()
        .filter(|c| c.deleted_at_ms.is_none() && c.kind == CreationKind::Book)
        .cloned()
        .collect();

    let reading_now_items: Vec<(PublicCatalogItemDto, f32)> = filtered_books
        .iter()
        .filter_map(|item| {
            let st = ws.my_creation_states.get(&item.creation_id)?;
            if st.in_library || st.progress_percent > 0.0 {
                Some((item.clone(), st.progress_percent))
            } else {
                None
            }
        })
        .collect();

    let library_items: Vec<(PublicCatalogItemDto, f32)> = filtered_books
        .iter()
        .filter_map(|item| {
            let st = ws.my_creation_states.get(&item.creation_id)?;
            if st.in_library || st.favorite {
                Some((item.clone(), st.progress_percent))
            } else {
                None
            }
        })
        .collect();

    let selected_detail = ws.resolve_selected_detail_creation();
    let selected_id = ws.selected_creation_id.clone();

    let sort_options: &[(CatalogSortMode, &'static str, &'static str, &'static str)] = &[
        (CatalogSortMode::Featured, "books-sort-featured", "精选", "Featured"),
        (CatalogSortMode::Popular, "books-sort-popular", "热门", "Popular"),
        (CatalogSortMode::TopRated, "books-sort-rated", "高分", "Top Rated"),
        (CatalogSortMode::Newest, "books-sort-newest", "最新", "Newest"),
    ];

    div()
        .id("apple-books-hub-root")
        .size_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        // Top Apple Books Header + Segmented Sub-Navigation + Search
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
                                .child(tg_icon(IconName::HubBooks, 22.0, BOOKS_ACCENT))
                                .child(
                                    div()
                                        .text_size(px(22.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(&lang, "图书", "Books")),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(6.0))
                                .children(BOOKS_SUB_TABS.iter().map(|&(slug, zh, en)| {
                                    vertical_sub_tab_pill(
                                        SharedString::from(format!("books-tab-{slug}")),
                                        tr_str(&lang, zh, en),
                                        active_tab == slug,
                                        BOOKS_ACCENT,
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
                                            tr_str(&lang, "搜索书库与书城…", "Search Books…"),
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
                                        "books-create-bot-btn",
                                        tr_str(&lang, "+ Bot 写书", "+ Write Book"),
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::OpenCreatorBot(Some(
                                                CreationKind::Book,
                                            )));
                                        }),
                                    ),
                                ),
                        ),
                )
                .when(selected_detail.is_none() && active_tab != "my_books", |el| {
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
                                            "books-genre-all",
                                            tr_str(&lang, "全部分类", "All Genres"),
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
                                            SharedString::from(format!("books-genre-{g_val}")),
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
                                                        ContentHubSection::Books,
                                                    ));
                                                    cx.notify();
                                                }),
                                            )
                                    }))
                                    .child(
                                        macos_secondary_pill_btn(
                                            "books-refresh-btn",
                                            tr_str(&lang, "刷新", "Refresh"),
                                            pal,
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|_this, _, _, cx| {
                                                cx.emit(ContentHubEvent::RefreshCatalog(
                                                    ContentHubSection::Books,
                                                ));
                                            }),
                                        ),
                                    ),
                            ),
                    )
                }),
        )
        // Main scrollable content
        .child(
            div()
                .id("apple-books-hub-scroll")
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_y_scroll()
                .p(px(24.0))
                .flex()
                .flex_col()
                .gap(px(20.0))
                .when_some(selected_detail.clone(), |el, detail| {
                    el.child(render_book_detail_card(ws, detail, cx))
                })
                .when(selected_detail.is_none() && active_tab == "my_books", |el| {
                    el.child(apple_shelf_header(
                        tr_str(&lang, "我的云端图书作品", "My Cloud Books"),
                        Some(format!("{} {}", my_books.len(), tr_str(&lang, "本", "books"))),
                        pal,
                    ))
                    .when(my_books.is_empty(), |inner| {
                        let cta = macos_primary_pill_btn(
                            "books-my-empty-cta",
                            tr_str(&lang, "用 @CreatorStudioBot 创作第一本书", "Write Your First Book"),
                            pal,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _, _, cx| {
                                cx.emit(ContentHubEvent::OpenCreatorBot(Some(CreationKind::Book)));
                            }),
                        );
                        inner.child(empty_state_card_shell(
                            IconName::HubBooks,
                            tr_str(&lang, "还没有云端图书作品", "No Cloud Books Yet"),
                            tr_str(
                                &lang,
                                "在聊天中使用 @CreatorStudioBot 的 /newbook 命令创作图书，完成后可一键推送审核上架。",
                                "Use @CreatorStudioBot /newbook to author books saved to your cloud library.",
                            ),
                            cta,
                            pal,
                        ))
                    })
                    .when(!my_books.is_empty(), |inner| {
                        inner.child(div().flex().flex_col().gap(px(10.0)).children(
                            my_books.iter().map(|book| {
                                let b_read = book.clone();
                                let b_pub = book.clone();
                                let bp = extract_book_payload(book);
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
                                                &book.cover,
                                                56.0,
                                                84.0,
                                                6.0,
                                                BOOKS_ACCENT,
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
                                                                    .child(book.title.clone()),
                                                            )
                                                            .child(review_status_badge(
                                                                book.review_status,
                                                                &lang,
                                                                pal,
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .text_color(rgb(pal.text_secondary))
                                                            .child(book.summary.clone()),
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
                                                        "my-book-read-{}",
                                                        book.creation_id
                                                    )),
                                                    tr_str(&lang, "试读", "Read"),
                                                    pal,
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        let saved = this
                                                            .my_creation_states
                                                            .get(&b_read.creation_id);
                                                        this.book_reader =
                                                            Some(BookReaderState::open(
                                                                &b_read,
                                                                bp.clone(),
                                                                saved,
                                                                this.palette,
                                                            ));
                                                        cx.notify();
                                                    }),
                                                ),
                                            )
                                            .child(
                                                macos_primary_pill_btn(
                                                    SharedString::from(format!(
                                                        "my-book-publish-{}",
                                                        book.creation_id
                                                    )),
                                                    tr_str(&lang, "推送审核", "Publish"),
                                                    pal,
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |_this, _, _, cx| {
                                                        cx.emit(
                                                            ContentHubEvent::OpenPublishModalForCreation(
                                                                b_pub.clone(),
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
                .when(selected_detail.is_none() && active_tab != "my_books", |el| {
                    let display_list: Vec<(PublicCatalogItemDto, Option<f32>, bool)> =
                        match active_tab {
                            "library" => library_items
                                .iter()
                                .map(|(b, pct)| (b.clone(), Some(*pct), true))
                                .collect(),
                            _ => filtered_books
                                .iter()
                                .map(|b| {
                                    let st = ws.my_creation_states.get(&b.creation_id);
                                    (
                                        b.clone(),
                                        st.filter(|s| s.progress_percent > 0.0)
                                            .map(|s| s.progress_percent),
                                        st.is_some_and(|s| s.in_library),
                                    )
                                })
                                .collect(),
                        };

                    el.child(hero_banner(
                        "apple-books-hero-banner",
                        tr_str(&lang, "APPLE BOOKS · 精选阅读", "APPLE BOOKS · EDITORIAL"),
                        tr_str(&lang, "发现精选公开图书", "Read Stories Crafted by Creators"),
                        tr_str(
                            &lang,
                            "沉浸式四主题阅读器、多章节目录、云端进度与书签同步。",
                            "Light, Sepia, Gray & Night themes, chapter TOC, and cloud-synced bookmarks.",
                        ),
                        "#FF9500",
                        "#FF5E3A",
                        IconName::HubBooks,
                        pal,
                    ))
                    .when(active_tab == "reading_now" && !reading_now_items.is_empty(), |inner| {
                        inner
                            .child(apple_shelf_header(
                                tr_str(&lang, "继续阅读", "Continue Reading"),
                                Some(format!(
                                    "{} {}",
                                    reading_now_items.len(),
                                    tr_str(&lang, "本正在读", "in progress")
                                )),
                                pal,
                            ))
                            .child(div().flex().flex_wrap().gap(px(14.0)).children(
                                reading_now_items.iter().map(|(item, pct)| {
                                    let is_sel = selected_id.as_deref()
                                        == Some(item.creation_id.as_str());
                                    render_book_cover_card_2_3(
                                        item,
                                        Some(*pct),
                                        true,
                                        is_sel,
                                        pal,
                                        &lang,
                                        cx,
                                    )
                                }),
                            ))
                    })
                    .child(apple_shelf_header(
                        match active_tab {
                            "library" => tr_str(&lang, "我的书库", "Your Library"),
                            "book_store" => tr_str(&lang, "书城精选", "Top Books in Store"),
                            _ => tr_str(&lang, "精选上架书单", "Featured Published Books"),
                        },
                        Some(format!("{} {}", display_list.len(), tr_str(&lang, "本", "titles"))),
                        pal,
                    ))
                    .when(display_list.is_empty(), |inner| {
                        let cta = macos_primary_pill_btn(
                            "books-empty-bot-cta",
                            tr_str(&lang, "用 @CreatorStudioBot 创作图书", "Write with @CreatorStudioBot"),
                            pal,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _, _, cx| {
                                cx.emit(ContentHubEvent::OpenCreatorBot(Some(CreationKind::Book)));
                            }),
                        );
                        inner.child(empty_state_card_shell(
                            IconName::HubBooks,
                            if active_tab == "library" {
                                tr_str(&lang, "书库为空", "Your Library is Empty")
                            } else {
                                tr_str(&lang, "暂无公开上架图书", "No Published Books Yet")
                            },
                            tr_str(
                                &lang,
                                "在书城中将喜欢的作品加入书库，或使用 @CreatorStudioBot 创作并审核上架。",
                                "Add books from the store or publish your own with @CreatorStudioBot.",
                            ),
                            cta,
                            pal,
                        ))
                    })
                    .when(!display_list.is_empty(), |inner| {
                        inner.child(div().flex().flex_wrap().gap(px(14.0)).children(
                            display_list.iter().map(|(item, pct, in_lib)| {
                                let is_sel =
                                    selected_id.as_deref() == Some(item.creation_id.as_str());
                                render_book_cover_card_2_3(
                                    item,
                                    *pct,
                                    *in_lib,
                                    is_sel,
                                    pal,
                                    &lang,
                                    cx,
                                )
                            }),
                        ))
                    })
                }),
        )
        .into_any_element()
}
