//! Apple App Store macOS unified storefront (`AppStoreHubView`): Today / Discover editorial
//! hero cards, Create / Work / Play / Develop / Categories / Installed tabs, 3-row shelves
//! aggregating published `App` creations, `mini_apps`, and cross-vertical Books/Music/Games.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::creations::{
    AppStoreListingPayload, AppStoreMiniAppEntry, CatalogSortMode, CreationKind, CreationPayload,
    CreationReviewStatus, PublicCatalogItemDto, UpsertCreationStateRequest, UserCreationDto,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::{
    macos_primary_pill_btn, macos_secondary_pill_btn,
};
use crate::views::content_hub::apple_primitives::{
    apple_grouped_card, apple_shelf_header, cover_artwork_box, creation_kind_accent,
    creation_kind_label, empty_state_card_shell, filter_pill, parse_hex_color, rating_pill,
    render_editable_field_box, review_status_badge, vertical_sub_tab_pill,
};
use crate::views::content_hub::{
    render_community_reviews_panel, ContentHubEvent, ContentHubInputTarget, ContentHubSection,
    ContentHubWorkspace,
};

pub const APPSTORE_ACCENT: u32 = 0x007AFF;

/// Canonical macOS App Store sub-navigation tabs.
pub const APPSTORE_SUB_TABS: &[(&str, &str, &str)] = &[
    ("today", "探索", "Discover"),
    ("create", "创作", "Create"),
    ("work", "工作", "Work"),
    ("play", "娱乐", "Play"),
    ("develop", "开发", "Develop"),
    ("categories", "类别", "Categories"),
    ("installed", "已安装", "Installed"),
];

pub fn normalize_appstore_sub_tab(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "today" | "discover" | "featured" => "today",
        "create" | "creativity" => "create",
        "work" | "productivity" => "work",
        "play" | "entertainment" => "play",
        "develop" | "developer" => "develop",
        "categories" | "browse" => "categories",
        "installed" | "updates" | "library" => "installed",
        _ => "today",
    }
}

fn extract_app_payload(creation: &UserCreationDto) -> Option<AppStoreListingPayload> {
    match &creation.payload {
        CreationPayload::App(ap) => Some(ap.clone()),
        _ => None,
    }
}

fn item_matches_tab(item: &PublicCatalogItemDto, tab: &str) -> bool {
    let hay = format!(
        "{} {} {}",
        item.genres.join(" "),
        item.tags.join(" "),
        item.summary
    )
    .to_ascii_lowercase();
    match tab {
        "create" => {
            matches!(item.kind, CreationKind::Book | CreationKind::Music)
                || hay.contains("creat")
                || hay.contains("design")
                || hay.contains("art")
                || hay.contains("studio")
                || hay.contains("创作")
        }
        "work" => {
            item.kind == CreationKind::App
                || hay.contains("work")
                || hay.contains("prod")
                || hay.contains("util")
                || hay.contains("office")
                || hay.contains("工具")
        }
        "play" => {
            matches!(item.kind, CreationKind::Game | CreationKind::Music)
                || hay.contains("game")
                || hay.contains("play")
                || hay.contains("arcade")
                || hay.contains("music")
        }
        "develop" => {
            item.kind == CreationKind::App
                || hay.contains("dev")
                || hay.contains("code")
                || hay.contains("bot")
                || hay.contains("api")
                || hay.contains("开发")
        }
        _ => true,
    }
}

fn render_today_editorial_card(
    item: &PublicCatalogItemDto,
    kicker: &str,
    installed: bool,
    _pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Stateful<Div> {
    let cid = item.creation_id.clone();
    let cid_cta = item.creation_id.clone();
    let item_open = item.clone();
    let item_cta = item.clone();
    let target_sec = ContentHubSection::from_creation_kind(item.kind);
    let accent = creation_kind_accent(item.kind);
    let start = parse_hex_color(&item.cover.gradient_start_hex, accent);
    let end = parse_hex_color(&item.cover.gradient_end_hex, 0x1C1C1E);

    div()
        .id(SharedString::from(format!("appstore-today-hero-{cid}")))
        .cursor_pointer()
        .flex_1()
        .min_w(px(300.0))
        .rounded(px(16.0))
        .overflow_hidden()
        .bg(linear_gradient(
            135.0,
            linear_color_stop(rgb(start), 0.0),
            linear_color_stop(rgb(end), 1.0),
        ))
        .border_1()
        .border_color(TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.16))
        .flex()
        .flex_col()
        .justify_between()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                if item_open.kind == CreationKind::App {
                    this.selected_creation_id = Some(cid.clone());
                    this.selected_catalog_item = Some(item_open.clone());
                    cx.emit(ContentHubEvent::OpenCreationDetail {
                        section: ContentHubSection::AppStore,
                        creation_id: cid.clone(),
                        action: Some("open".to_string()),
                    });
                } else {
                    cx.emit(ContentHubEvent::OpenCreationDetail {
                        section: target_sec,
                        creation_id: cid.clone(),
                        action: Some("open".to_string()),
                    });
                }
                cx.notify();
            }),
        )
        .child(
            div()
                .p(px(20.0))
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.80))
                        .child(kicker.to_string()),
                )
                .child(
                    div()
                        .text_size(px(20.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .child(item.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(TdesktopThemePalette::rgba_with_alpha(0xFFFFFF, 0.86))
                        .child(item.summary.clone()),
                ),
        )
        // Frosted bottom app strip (authentic macOS App Store Today card footer)
        .child(
            div()
                .px(px(16.0))
                .py(px(12.0))
                .bg(TdesktopThemePalette::rgba_with_alpha(0x000000, 0.34))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(cover_artwork_box(&item.cover, 40.0, 40.0, 10.0, accent))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .child(item.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(TdesktopThemePalette::rgba_with_alpha(
                                            0xFFFFFF, 0.78,
                                        ))
                                        .child(format!(
                                            "{} · {}",
                                            creation_kind_label(item.kind, lang),
                                            item.author_display_name
                                        )),
                                ),
                        ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("appstore-today-cta-{cid_cta}")))
                        .cursor_pointer()
                        .px(px(14.0))
                        .py(px(5.0))
                        .rounded(px(999.0))
                        .bg(rgb(0xFFFFFF))
                        .text_size(px(11.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(APPSTORE_ACCENT))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _, _, cx| {
                                cx.stop_propagation();
                                if !installed {
                                    cx.emit(ContentHubEvent::UpsertCreationState {
                                        creation_id: cid_cta.clone(),
                                        req: UpsertCreationStateRequest {
                                            in_library: Some(true),
                                            ..Default::default()
                                        },
                                    });
                                }
                                let action = match item_cta.kind {
                                    CreationKind::Book => "read",
                                    CreationKind::Music | CreationKind::Game => "play",
                                    CreationKind::App => "open",
                                };
                                cx.emit(ContentHubEvent::OpenCreationDetail {
                                    section: target_sec,
                                    creation_id: cid_cta.clone(),
                                    action: Some(action.to_string()),
                                });
                            }),
                        )
                        .child(if installed {
                            tr_str(lang, "打开", "Open")
                        } else {
                            tr_str(lang, "获取", "Get")
                        }),
                ),
        )
}

fn render_app_store_row_card(
    item: &PublicCatalogItemDto,
    installed: bool,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Stateful<Div> {
    let cid = item.creation_id.clone();
    let cid_btn = item.creation_id.clone();
    let item_open = item.clone();
    let item_btn = item.clone();
    let accent = creation_kind_accent(item.kind);
    let target_sec = ContentHubSection::from_creation_kind(item.kind);
    let hover_bg = pal.row_hover_rgba();

    div()
        .id(SharedString::from(format!("appstore-row-{cid}")))
        .cursor_pointer()
        .w(px(292.0))
        .p(px(10.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.separator_rgba())
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .gap(px(12.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                if item_open.kind == CreationKind::App {
                    this.selected_creation_id = Some(cid.clone());
                    this.selected_catalog_item = Some(item_open.clone());
                    cx.emit(ContentHubEvent::OpenCreationDetail {
                        section: ContentHubSection::AppStore,
                        creation_id: cid.clone(),
                        action: Some("open".to_string()),
                    });
                } else {
                    cx.emit(ContentHubEvent::OpenCreationDetail {
                        section: target_sec,
                        creation_id: cid.clone(),
                        action: Some("open".to_string()),
                    });
                }
                cx.notify();
            }),
        )
        .child(cover_artwork_box(&item.cover, 54.0, 54.0, 12.0, accent))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(13.5))
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
                        .child(format!(
                            "{} · {}",
                            creation_kind_label(item.kind, lang),
                            item.author_display_name
                        )),
                )
                .child(rating_pill(item.rating_avg, item.rating_count, pal)),
        )
        .child(
            div()
                .id(SharedString::from(format!("appstore-get-btn-{cid_btn}")))
                .cursor_pointer()
                .px(px(12.0))
                .py(px(5.0))
                .rounded(px(999.0))
                .bg(if installed {
                    TdesktopThemePalette::rgba_with_alpha(APPSTORE_ACCENT, 0.16)
                } else {
                    rgb(APPSTORE_ACCENT)
                })
                .text_size(px(11.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(if installed { APPSTORE_ACCENT } else { 0xFFFFFF }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_this, _, _, cx| {
                        cx.stop_propagation();
                        if !installed {
                            cx.emit(ContentHubEvent::UpsertCreationState {
                                creation_id: cid_btn.clone(),
                                req: UpsertCreationStateRequest {
                                    in_library: Some(true),
                                    ..Default::default()
                                },
                            });
                        }
                        let action = match item_btn.kind {
                            CreationKind::Book => "read",
                            CreationKind::Music | CreationKind::Game => "play",
                            CreationKind::App => "open",
                        };
                        cx.emit(ContentHubEvent::OpenCreationDetail {
                            section: target_sec,
                            creation_id: cid_btn.clone(),
                            action: Some(action.to_string()),
                        });
                    }),
                )
                .child(if installed {
                    tr_str(lang, "打开", "Open")
                } else {
                    tr_str(lang, "获取", "Get")
                }),
        )
}

fn render_app_detail_card(
    ws: &mut ContentHubWorkspace,
    detail: UserCreationDto,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.clone();
    let cid = detail.creation_id.clone();
    let app_payload = extract_app_payload(&detail);
    let installed = ws
        .my_creation_states
        .get(&cid)
        .is_some_and(|s| s.in_library);
    let is_own = ws.find_my_creation(&cid).is_some();
    let is_published = detail.review_status == CreationReviewStatus::Published;
    let linked_miniapp = app_payload
        .as_ref()
        .and_then(|ap| ap.linked_miniapp.clone());

    let cid_install = cid.clone();
    let detail_pub = detail.clone();
    let linked_for_btn = linked_miniapp.clone();

    div()
        .id("apple-appstore-detail-panel")
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
                        "appstore-detail-back-btn",
                        tr_str(&lang, "← 返回 App Store", "← Back to App Store"),
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
        // App Store Product Header
        .child(
            apple_grouped_card(pal)
                .p(px(20.0))
                .flex()
                .flex_col()
                .gap(px(16.0))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(18.0))
                        .child(cover_artwork_box(
                            &detail.cover,
                            88.0,
                            88.0,
                            20.0,
                            APPSTORE_ACCENT,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(220.0))
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
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
                                        .text_color(rgb(pal.text_secondary))
                                        .child(if detail.subtitle.trim().is_empty() {
                                            detail.summary.clone()
                                        } else {
                                            detail.subtitle.clone()
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(APPSTORE_ACCENT))
                                        .child(
                                            app_payload
                                                .as_ref()
                                                .filter(|a| !a.developer_name.trim().is_empty())
                                                .map(|a| a.developer_name.clone())
                                                .unwrap_or_else(|| {
                                                    detail.author_display_name.clone()
                                                }),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .id("appstore-detail-primary-btn")
                                        .cursor_pointer()
                                        .px(px(20.0))
                                        .py(px(8.0))
                                        .rounded(px(999.0))
                                        .bg(rgb(APPSTORE_ACCENT))
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |_this, _, _, cx| {
                                                if !installed {
                                                    cx.emit(ContentHubEvent::UpsertCreationState {
                                                        creation_id: cid_install.clone(),
                                                        req: UpsertCreationStateRequest {
                                                            in_library: Some(true),
                                                            ..Default::default()
                                                        },
                                                    });
                                                }
                                                if let Some(ref linked) = linked_for_btn {
                                                    if !linked.bot_username.trim().is_empty()
                                                        || linked
                                                            .web_url
                                                            .as_deref()
                                                            .is_some_and(|u| !u.trim().is_empty())
                                                    {
                                                        cx.emit(ContentHubEvent::LaunchMiniApp {
                                                            bot_username: linked
                                                                .bot_username
                                                                .clone(),
                                                            short_name: linked.short_name.clone(),
                                                            web_url: linked.web_url.clone(),
                                                        });
                                                    }
                                                }
                                            }),
                                        )
                                        .child(if installed {
                                            tr_str(&lang, "打开", "Open").to_string()
                                        } else {
                                            app_payload
                                                .as_ref()
                                                .map(|a| a.price_label.clone())
                                                .filter(|s| !s.trim().is_empty())
                                                .unwrap_or_else(|| {
                                                    tr_str(&lang, "获取", "Get").to_string()
                                                })
                                        }),
                                )
                                .when(is_own, |el| {
                                    el.child(
                                        macos_secondary_pill_btn(
                                            "appstore-detail-publish-btn",
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
                )
                // Horizontal App Store metadata bar
                .child(
                    div()
                        .pt(px(12.0))
                        .border_t_1()
                        .border_color(pal.separator_rgba())
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(20.0))
                        .child(rating_pill(detail.rating_avg, detail.rating_count, pal))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!(
                                    "{}: {}",
                                    tr_str(&lang, "年龄分级", "Age"),
                                    detail.age_rating
                                )),
                        )
                        .when_some(app_payload.as_ref(), |el, ap| {
                            el.child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(format!(
                                        "{}: {} · {} · {}",
                                        tr_str(&lang, "版本", "Version"),
                                        ap.version_string,
                                        ap.category,
                                        if ap.size_label.trim().is_empty() {
                                            "Cloud App"
                                        } else {
                                            ap.size_label.as_str()
                                        }
                                    )),
                            )
                        }),
                ),
        )
        // Description, Release Notes & Screenshots
        .when_some(app_payload, |el, ap| {
            el.child(
                apple_grouped_card(pal)
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(apple_shelf_header(
                        tr_str(&lang, "应用介绍", "About This App"),
                        None,
                        pal,
                    ))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .line_height(px(20.0))
                            .text_color(rgb(pal.text_secondary))
                            .child(if ap.long_description.trim().is_empty() {
                                detail.summary.clone()
                            } else {
                                ap.long_description.clone()
                            }),
                    )
                    .when(!ap.release_notes.trim().is_empty(), |inner| {
                        inner.child(
                            div()
                                .pt(px(8.0))
                                .border_t_1()
                                .border_color(pal.separator_rgba())
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(&lang, "新功能", "What's New")),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(ap.release_notes.clone()),
                                ),
                        )
                    })
                    .when(!ap.screenshots.is_empty(), |inner| {
                        inner.child(
                            div()
                                .pt(px(8.0))
                                .flex()
                                .flex_wrap()
                                .gap(px(10.0))
                                .children(ap.screenshots.iter().map(|ss| {
                                    let tint = ss
                                        .accent_hex
                                        .as_deref()
                                        .map(|h| parse_hex_color(h, APPSTORE_ACCENT))
                                        .unwrap_or(APPSTORE_ACCENT);
                                    div()
                                        .w(px(220.0))
                                        .rounded(px(12.0))
                                        .overflow_hidden()
                                        .border_1()
                                        .border_color(pal.separator_rgba())
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .h(px(110.0))
                                                .bg(linear_gradient(
                                                    135.0,
                                                    linear_color_stop(rgb(tint), 0.0),
                                                    linear_color_stop(rgb(0x1C1C1E), 1.0),
                                                ))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(0xFFFFFF))
                                                .child(ss.title.clone()),
                                        )
                                        .when_some(ss.caption.clone(), |c_el, cap| {
                                            c_el.child(
                                                div()
                                                    .p(px(8.0))
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(pal.text_secondary))
                                                    .child(cap),
                                            )
                                        })
                                })),
                        )
                    })
                    .when(!ap.permissions.is_empty(), |inner| {
                        inner.child(
                            div()
                                .pt(px(6.0))
                                .flex()
                                .flex_wrap()
                                .gap(px(6.0))
                                .children(ap.permissions.iter().map(|perm| {
                                    div()
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(999.0))
                                        .bg(TdesktopThemePalette::rgba_with_alpha(
                                            pal.text_primary,
                                            0.07,
                                        ))
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(perm.clone())
                                })),
                        )
                    }),
            )
        })
        // Community Ratings & Reviews
        .child(render_community_reviews_panel(
            ws,
            &cid,
            is_published,
            APPSTORE_ACCENT,
            cx,
        ))
        .into_any_element()
}

/// Renders the Apple App Store unified storefront (`AppStoreHubView`).
pub fn render_app_store_hub_view(
    ws: &mut ContentHubWorkspace,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.clone();
    let active_tab = normalize_appstore_sub_tab(&ws.active_sub_tab);
    let search_focused = ws.active_input == ContentHubInputTarget::CatalogSearch;
    let search_field = ws.catalog_search.clone();
    let sort_mode = ws.sort_mode;
    let selected_detail = ws.resolve_selected_detail_creation();

    let all_items = ws.active_catalog_items(ContentHubSection::AppStore);
    let miniapps: Vec<AppStoreMiniAppEntry> = if !ws.appstore_catalog.mini_apps.is_empty() {
        ws.appstore_catalog.mini_apps.clone()
    } else {
        ws.local_miniapps_fallback.clone()
    };

    let q_lower = ws.catalog_search.text.trim().to_lowercase();
    let filtered_miniapps: Vec<AppStoreMiniAppEntry> = miniapps
        .into_iter()
        .filter(|m| {
            q_lower.is_empty()
                || m.title.to_lowercase().contains(&q_lower)
                || m.description.to_lowercase().contains(&q_lower)
                || m.bot_username.to_lowercase().contains(&q_lower)
        })
        .collect();

    let installed_items: Vec<PublicCatalogItemDto> = all_items
        .iter()
        .filter(|i| {
            ws.my_creation_states
                .get(&i.creation_id)
                .is_some_and(|st| st.in_library)
        })
        .cloned()
        .collect();

    let tab_items: Vec<PublicCatalogItemDto> = match active_tab {
        "installed" => installed_items.clone(),
        "today" | "categories" => all_items.clone(),
        other => all_items
            .iter()
            .filter(|i| item_matches_tab(i, other))
            .cloned()
            .collect(),
    };

    let today_heroes: Vec<PublicCatalogItemDto> = if !ws.appstore_catalog.featured.is_empty() {
        ws.appstore_catalog
            .featured
            .iter()
            .take(2)
            .cloned()
            .collect()
    } else {
        tab_items.iter().take(2).cloned().collect()
    };

    let cross_vertical: Vec<PublicCatalogItemDto> = ws
        .appstore_catalog
        .cross_vertical
        .iter()
        .filter(|i| {
            q_lower.is_empty()
                || i.title.to_lowercase().contains(&q_lower)
                || i.summary.to_lowercase().contains(&q_lower)
        })
        .cloned()
        .collect();

    let sort_options: &[(CatalogSortMode, &'static str, &'static str, &'static str)] = &[
        (CatalogSortMode::Featured, "store-sort-featured", "精选", "Featured"),
        (CatalogSortMode::Popular, "store-sort-popular", "热门", "Popular"),
        (CatalogSortMode::TopRated, "store-sort-rated", "高分", "Top Rated"),
        (CatalogSortMode::Newest, "store-sort-newest", "最新", "Newest"),
    ];

    div()
        .id("apple-appstore-hub-root")
        .size_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        // Top Header + 7 App Store tabs + Search
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
                                .child(tg_icon(IconName::HubAppStore, 22.0, APPSTORE_ACCENT))
                                .child(
                                    div()
                                        .text_size(px(22.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child("App Store"),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(6.0))
                                .children(APPSTORE_SUB_TABS.iter().map(|&(slug, zh, en)| {
                                    vertical_sub_tab_pill(
                                        SharedString::from(format!("appstore-tab-{slug}")),
                                        tr_str(&lang, zh, en),
                                        active_tab == slug,
                                        APPSTORE_ACCENT,
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
                                            tr_str(&lang, "搜索应用、小程序与作品…", "Search App Store…"),
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
                                    macos_secondary_pill_btn(
                                        "appstore-refresh-btn",
                                        tr_str(&lang, "刷新", "Refresh"),
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::RefreshCatalog(
                                                ContentHubSection::AppStore,
                                            ));
                                        }),
                                    ),
                                ),
                        ),
                )
                .when(selected_detail.is_none(), |el| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
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
                                                        ContentHubSection::AppStore,
                                                    ));
                                                    cx.notify();
                                                }),
                                            )
                                    })),
                            )
                            .child(
                                macos_primary_pill_btn(
                                    "appstore-open-studio-bot-btn",
                                    tr_str(&lang, "打开 @CreatorStudioBot", "Open @CreatorStudioBot"),
                                    pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|_this, _, _, cx| {
                                        cx.emit(ContentHubEvent::OpenCreatorBot(None));
                                    }),
                                ),
                            ),
                    )
                }),
        )
        // Main Scrollable Storefront
        .child(
            div()
                .id("apple-appstore-hub-scroll")
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_y_scroll()
                .p(px(24.0))
                .flex()
                .flex_col()
                .gap(px(20.0))
                .when_some(selected_detail.clone(), |el, detail| {
                    el.child(render_app_detail_card(ws, detail, cx))
                })
                .when(selected_detail.is_none(), |el| {
                    el
                        // Today Editorial Hero Cards (authentic macOS App Store Today layout)
                        .when(active_tab == "today" && !today_heroes.is_empty(), |inner| {
                            inner
                                .child(apple_shelf_header(
                                    tr_str(&lang, "今日精选 · Today", "Today"),
                                    Some(
                                        tr_str(&lang, "编辑精选与热门上架", "Editor's Picks")
                                            .to_string(),
                                    ),
                                    pal,
                                ))
                                .child(div().flex().flex_wrap().gap(px(16.0)).children(
                                    today_heroes.iter().enumerate().map(|(idx, item)| {
                                        let kicker = if idx == 0 {
                                            tr_str(&lang, "今日推荐 · TODAY'S PICK", "TODAY'S PICK")
                                        } else {
                                            tr_str(&lang, "精品聚焦 · FEATURED", "FEATURED STORY")
                                        };
                                        let installed = ws
                                            .my_creation_states
                                            .get(&item.creation_id)
                                            .is_some_and(|s| s.in_library);
                                        render_today_editorial_card(
                                            item, kicker, installed, pal, &lang, cx,
                                        )
                                    }),
                                ))
                        })
                        // Category quick-jump tiles on Categories tab
                        .when(active_tab == "categories", |inner| {
                            let cats: &[(&str, &str, ContentHubSection, u32)] = &[
                                ("图书 · Books", "Books", ContentHubSection::Books, 0xFF9500),
                                ("音乐 · Music", "Music", ContentHubSection::Music, 0xFA2D48),
                                ("游戏 · Games", "Games", ContentHubSection::Games, 0x5856D6),
                                ("我的创作 · Cloud Studio", "My Creations", ContentHubSection::MyCreations, 0x30D158),
                            ];
                            inner.child(div().flex().flex_wrap().gap(px(12.0)).children(
                                cats.iter().map(|&(zh, en, sec, accent)| {
                                    div()
                                        .id(SharedString::from(format!("appstore-cat-{sec:?}")))
                                        .cursor_pointer()
                                        .w(px(210.0))
                                        .p(px(16.0))
                                        .rounded(px(14.0))
                                        .bg(TdesktopThemePalette::rgba_with_alpha(accent, 0.16))
                                        .border_1()
                                        .border_color(TdesktopThemePalette::rgba_with_alpha(
                                            accent, 0.35,
                                        ))
                                        .text_size(px(14.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |_this, _, _, cx| {
                                                cx.emit(ContentHubEvent::NavigateSection(sec));
                                            }),
                                        )
                                        .child(tr_str(&lang, zh, en))
                                }),
                            ))
                        })
                        // Published Mini Apps shelf
                        .when(
                            !filtered_miniapps.is_empty() && active_tab != "installed",
                            |inner| {
                                inner
                                    .child(apple_shelf_header(
                                        tr_str(&lang, "精选 Mini Apps 与机器人应用", "Featured Mini Apps"),
                                        Some(format!(
                                            "{} {}",
                                            filtered_miniapps.len(),
                                            tr_str(&lang, "个小程序", "mini apps")
                                        )),
                                        pal,
                                    ))
                                    .child(div().flex().flex_wrap().gap(px(12.0)).children(
                                        filtered_miniapps.iter().map(|app| {
                                            let bot_un = app.bot_username.clone();
                                            let short = app.short_name.clone();
                                            let web_url = (!app.web_url.trim().is_empty())
                                                .then(|| app.web_url.clone());
                                            apple_grouped_card(pal)
                                                .w(px(292.0))
                                                .p(px(12.0))
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .gap(px(10.0))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w(px(0.0))
                                                        .flex()
                                                        .flex_col()
                                                        .gap(px(2.0))
                                                        .child(
                                                            div()
                                                                .text_size(px(13.5))
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(rgb(pal.text_primary))
                                                                .truncate()
                                                                .child(app.title.clone()),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(11.0))
                                                                .text_color(rgb(pal.text_secondary))
                                                                .truncate()
                                                                .child(format!(
                                                                    "@{} · {}",
                                                                    app.bot_username,
                                                                    app.description
                                                                )),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "appstore-launch-miniapp-{}-{}",
                                                            app.bot_username, app.short_name
                                                        )))
                                                        .cursor_pointer()
                                                        .px(px(12.0))
                                                        .py(px(5.0))
                                                        .rounded(px(999.0))
                                                        .bg(rgb(APPSTORE_ACCENT))
                                                        .text_size(px(11.0))
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(rgb(0xFFFFFF))
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(move |_this, _, _, cx| {
                                                                cx.emit(
                                                                    ContentHubEvent::LaunchMiniApp {
                                                                        bot_username: bot_un.clone(),
                                                                        short_name: short.clone(),
                                                                        web_url: web_url.clone(),
                                                                    },
                                                                );
                                                            }),
                                                        )
                                                        .child(tr_str(&lang, "打开", "Open")),
                                                )
                                        }),
                                    ))
                            },
                        )
                        // Unified Apps & Verticals Shelf
                        .when(!tab_items.is_empty(), |inner| {
                            inner
                                .child(apple_shelf_header(
                                    if active_tab == "installed" {
                                        tr_str(&lang, "已获取 / 已安装", "Installed & Saved")
                                    } else {
                                        tr_str(&lang, "应用与精选公开作品", "Apps & Featured Creations")
                                    },
                                    Some(format!(
                                        "{} {}",
                                        tab_items.len(),
                                        tr_str(&lang, "项", "items")
                                    )),
                                    pal,
                                ))
                                .child(div().flex().flex_wrap().gap(px(12.0)).children(
                                    tab_items.iter().map(|item| {
                                        let installed = ws
                                            .my_creation_states
                                            .get(&item.creation_id)
                                            .is_some_and(|s| s.in_library);
                                        render_app_store_row_card(item, installed, pal, &lang, cx)
                                    }),
                                ))
                        })
                        // Cross-Vertical Spotlight (Books + Music + Games)
                        .when(
                            active_tab == "today" && !cross_vertical.is_empty(),
                            |inner| {
                                inner
                                    .child(apple_shelf_header(
                                        tr_str(
                                            &lang,
                                            "跨板块精品推荐（图书 · 音乐 · 游戏）",
                                            "Featured Across Books, Music & Games",
                                        ),
                                        Some(format!(
                                            "{} {}",
                                            cross_vertical.len(),
                                            tr_str(&lang, "款精选", "picks")
                                        )),
                                        pal,
                                    ))
                                    .child(div().flex().flex_wrap().gap(px(12.0)).children(
                                        cross_vertical.iter().map(|item| {
                                            let installed = ws
                                                .my_creation_states
                                                .get(&item.creation_id)
                                                .is_some_and(|s| s.in_library);
                                            render_app_store_row_card(
                                                item, installed, pal, &lang, cx,
                                            )
                                        }),
                                    ))
                            },
                        )
                        // Empty state when truly empty
                        .when(tab_items.is_empty() && filtered_miniapps.is_empty(), |inner| {
                            let cta = macos_primary_pill_btn(
                                "appstore-empty-cta",
                                tr_str(&lang, "用 @CreatorStudioBot 创作并上架", "Create with @CreatorStudioBot"),
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _, _, cx| {
                                    cx.emit(ContentHubEvent::OpenCreatorBot(None));
                                }),
                            );
                            inner.child(empty_state_card_shell(
                                IconName::HubAppStore,
                                if active_tab == "installed" {
                                    tr_str(&lang, "暂未获取任何应用或作品", "No Installed Items Yet")
                                } else {
                                    tr_str(&lang, "App Store 暂无上架条目", "No Store Listings Yet")
                                },
                                tr_str(
                                    &lang,
                                    "公开发布的应用、Mini Apps 以及精选图书/音乐/游戏将汇聚在这里。",
                                    "Published apps, Mini Apps, and featured creations appear here automatically.",
                                ),
                                cta,
                                pal,
                            ))
                        })
                }),
        )
        .into_any_element()
}
