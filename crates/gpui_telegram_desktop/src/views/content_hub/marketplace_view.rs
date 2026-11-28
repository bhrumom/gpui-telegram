//! Unified OpenBot 1:1 Marketplace View (`市场`):
//! - Left `260px` sidebar with OpenBot's 3 primary tabs (`应用`, `助手`, `技能`),
//!   separated `小程序应用`, `图书`, `游戏` entries below `技能`, and live `你的应用` list.
//! - Right `880px`-max centered canvas supporting full connector/plugin browse (266 apps),
//!   category drill-down (`查看全部 ›`), connector/plugin auth setup (`open`/`key`/`link`/`local`/native),
//!   `+ 添加自定义项` STDIO/HTTP MCP server form, 29 production agents with 3D Plush avatars,
//!   40 production skills with `SKILL.md`, and embedded Mini Apps / Books / Games hubs.

use std::collections::BTreeMap;

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::{AvatarHue, AvatarMood};
use openbot_runtime::platform::marketplace::{
    OpenBotAgentCatalogItem, OpenBotConnectorCatalogItem, OpenBotPluginCatalogItem,
    OpenBotSkillCatalogItem, SKILL_CATEGORIES,
};
use openbot_storage::{McpServerConfig, McpTransport};

use crate::components::bloub_avatar::plush_felt::render_plush_seed_avatar;
use crate::components::bloub_avatar::MarketplaceNoMatchPlushState;
use crate::i18n::tr_str;
use crate::theme::{IconName, TdesktopThemePalette, tg_icon_rgba};
use crate::views::content_hub::apple_primitives::{
    OPENBOT_MARKETPLACE_CONTENT_MAX_WIDTH, apple_grouped_card, openbot_primary_cta_btn,
    openbot_secondary_btn, parse_hex_color, render_editable_field_box,
};
use crate::views::content_hub::marketplace_state::{
    MarketplaceAppEntry, MarketplaceNavTab, MarketplaceRoute, MarketplaceSkillSortMode,
    MarketplaceSourceChoice, category_localized_label,
};
use crate::views::content_hub::{
    ContentHubEvent, ContentHubInputTarget, ContentHubWorkspace, app_store_view, books_view,
    games_view,
};

const APP_GROUP_LIMIT: usize = 6;
const AGENT_GROUP_LIMIT: usize = 6;
const SKILL_GROUP_LIMIT: usize = 6;
const MARKETPLACE_SIDEBAR_WIDTH: f32 = 256.0;

fn connector_brand_icon(connector_id: &str) -> Option<IconName> {
    match connector_id {
        "slack" => Some(IconName::BrandSlack),
        "discord" => Some(IconName::BrandDiscord),
        "telegram" => Some(IconName::BrandTelegram),
        "whatsapp" => Some(IconName::BrandWhatsApp),
        "github" => Some(IconName::BrandGitHub),
        "onepassword" => Some(IconName::BrandOnePassword),
        "bitwarden" => Some(IconName::BrandBitwarden),
        "protonpass" => Some(IconName::BrandProtonPass),
        "x" => Some(IconName::BrandX),
        "stripe" => Some(IconName::BrandStripe),
        _ => None,
    }
}

fn monogram_for_name(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.len() >= 2 {
        let a = words[0].chars().next().unwrap_or('A');
        let b = words[1].chars().next().unwrap_or('P');
        format!("{a}{b}").to_uppercase()
    } else {
        name.chars().take(2).collect::<String>().to_uppercase()
    }
}

fn render_app_logo_tile(app: &MarketplaceAppEntry, size_px: f32) -> Div {
    let radius = if size_px <= 22.0 { 5.0 } else { 10.0 };
    let icon_px = (size_px * 0.56).round();
    let font_px = (size_px * 0.36).max(9.0);
    let bg_u32 = parse_hex_color(app.brand_hex(), 0x3F3F46);

    let tile = div()
        .w(px(size_px))
        .h(px(size_px))
        .flex_shrink_0()
        .rounded(px(radius))
        .bg(rgb(bg_u32))
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden();

    match app {
        MarketplaceAppEntry::Connector(c) => {
            if let Some(ic) = connector_brand_icon(&c.connector_id) {
                tile.child(tg_icon_rgba(ic, icon_px, rgba(0xFFFFFFFF)))
            } else {
                tile.text_size(px(font_px))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xFFFFFF))
                    .child(monogram_for_name(&c.name))
            }
        }
        MarketplaceAppEntry::Plugin(p) => {
            if let Some(ic) = connector_brand_icon(&p.slug) {
                tile.child(tg_icon_rgba(ic, icon_px, rgba(0xFFFFFFFF)))
            } else {
                tile.text_size(px(font_px))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0xFFFFFF))
                    .child(monogram_for_name(&p.name))
            }
        }
        MarketplaceAppEntry::Custom(_) => {
            tile.child(tg_icon_rgba(IconName::MarketplacePlug, icon_px, rgba(0xFFFFFFFF)))
        }
    }
}

fn render_nav_row(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    active: bool,
    badge: Option<String>,
    pal: TdesktopThemePalette,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let hover_bg = ob.bg_control_hover();
    let active_bg = ob.bg_control_active();
    let text_pri = ob.text_primary();
    let text_muted = ob.text_muted();
    let label_str: SharedString = label.into();

    div()
        .id(id)
        .cursor_pointer()
        .w_full()
        .h(px(34.0))
        .px(px(10.0))
        .rounded(px(8.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .when(active, |el| el.bg(active_bg))
        .when(!active, |el| el.hover(move |s| s.bg(hover_bg)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(9.0))
                .child(tg_icon_rgba(
                    icon,
                    16.0,
                    if active { text_pri } else { text_muted },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(13.5))
                        .font_weight(if active {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if active { text_pri } else { text_muted })
                        .child(label_str),
                ),
        )
        .when_some(badge, |el, count| {
            el.child(
                div()
                    .text_size(px(11.5))
                    .text_color(ob.text_dim())
                    .child(count),
            )
        })
}

fn render_source_switch(
    id_prefix: &'static str,
    current: MarketplaceSourceChoice,
    is_agents: bool,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Div {
    let ob = pal.openbot_palette();
    let active_bg = ob.bg_control_active();
    let hover_bg = ob.bg_control_hover();
    let text_pri = ob.text_primary();
    let text_muted = ob.text_muted();

    let items = [
        (MarketplaceSourceChoice::All, "all", tr_str(lang, "全部", "All"), false),
        (
            MarketplaceSourceChoice::Verified,
            "verified",
            tr_str(lang, "已验证", "Verified"),
            true,
        ),
        (
            MarketplaceSourceChoice::Public,
            "public",
            tr_str(lang, "公开", "Public"),
            false,
        ),
    ];

    div()
        .p(px(3.0))
        .rounded(px(10.0))
        .bg(ob.bg_surface())
        .border_1()
        .border_color(ob.border_hairline())
        .flex()
        .items_center()
        .gap(px(2.0))
        .children(items.into_iter().map(|(choice, slug, label, shield)| {
            let active = current == choice;
            div()
                .id(SharedString::from(format!("{id_prefix}-{slug}")))
                .cursor_pointer()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(7.0))
                .when(active, |el| el.bg(active_bg))
                .when(!active, |el| el.hover(move |s| s.bg(hover_bg)))
                .flex()
                .items_center()
                .gap(px(4.0))
                .text_size(px(12.0))
                .font_weight(if active {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if active { text_pri } else { text_muted })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if is_agents {
                            this.marketplace.agent_source = choice;
                        } else {
                            this.marketplace.skill_source = choice;
                        }
                        cx.notify();
                    }),
                )
                .when(shield, |el| {
                    el.child(tg_icon_rgba(
                        IconName::ShieldMod,
                        12.0,
                        if active { text_pri } else { text_muted },
                    ))
                })
                .child(label)
        }))
}

fn render_group_header(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    count: Option<usize>,
    show_see_all: bool,
    pal: TdesktopThemePalette,
    lang: &str,
    on_see_all: impl Fn(&mut ContentHubWorkspace, &mut Context<ContentHubWorkspace>) + 'static,
    cx: &mut Context<ContentHubWorkspace>,
) -> Div {
    let ob = pal.openbot_palette();
    let label_str: SharedString = label.into();
    let hover_bg = ob.bg_control_hover();
    let text_pri = ob.text_primary();
    let see_all_id: ElementId = id.into();

    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(7.0))
                .child(
                    div()
                        .text_size(px(14.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(label_str),
                )
                .when_some(count, |el, n| {
                    el.child(
                        div()
                            .text_size(px(13.0))
                            .text_color(ob.text_muted())
                            .child(format!("{n}")),
                    )
                }),
        )
        .when(show_see_all, |el| {
            el.child(
                div()
                    .id(see_all_id)
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(7.0))
                    .hover(move |s| s.bg(hover_bg).text_color(text_pri))
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .text_size(px(12.5))
                    .text_color(ob.text_muted())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            on_see_all(this, cx);
                            cx.notify();
                        }),
                    )
                    .child(tr_str(lang, "查看全部", "See all"))
                    .child(tg_icon_rgba(IconName::ChevronRight, 13.0, ob.text_muted())),
            )
        })
}

fn render_no_match_plush_state(
    message: &str,
    other_tabs: Vec<(MarketplaceNavTab, usize)>,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Div {
    let ob = pal.openbot_palette();
    let plush = MarketplaceNoMatchPlushState::new(false);
    div()
        .w_full()
        .py(px(36.0))
        .px(px(20.0))
        .rounded(px(14.0))
        .border_1()
        .border_color(ob.border_hairline())
        .bg(ob.bg_surface())
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.0))
        .child(plush.render_element(124.0))
        .child(
            div()
                .text_size(px(13.5))
                .text_color(ob.text_muted())
                .child(message.to_string()),
        )
        .when(!other_tabs.is_empty(), |el| {
            el.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .children(other_tabs.into_iter().map(|(tab, count)| {
                        let lbl = if lang.starts_with("zh") {
                            format!("显示 {count} 个{}", tab.localized_label(lang))
                        } else {
                            format!("Show {count} {}", tab.localized_label(lang))
                        };
                        openbot_secondary_btn(
                            SharedString::from(format!("mkt-empty-jump-{}", tab.slug())),
                            lbl,
                            pal,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.marketplace.select_tab(tab);
                                cx.notify();
                            }),
                        )
                    })),
            )
        })
}

fn render_app_row(
    app: &MarketplaceAppEntry,
    connected: bool,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let hover_bg = ob.bg_control_hover();
    let ctrl_bg = ob.bg_control();
    let ctrl_hover = ob.bg_control_active();
    let app_id = app.id();
    let app_id_open = app_id.clone();
    let app_id_btn = app_id.clone();
    let tagline = app.localized_tagline(lang);
    let is_open_plugin = matches!(app, MarketplaceAppEntry::Plugin(p) if p.auth_kind == "open");
    let open_plugin_slug = match app {
        MarketplaceAppEntry::Plugin(p) => Some(p.slug.clone()),
        _ => None,
    };

    let action_el: AnyElement = if connected {
        div()
            .flex_shrink_0()
            .px(px(10.0))
            .py(px(5.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .text_size(px(12.5))
            .text_color(ob.text_muted())
            .child(tg_icon_rgba(IconName::SelectCheck, 13.0, ob.success()))
            .child(tr_str(lang, "已连接", "Connected"))
            .into_any_element()
    } else {
        div()
            .id(SharedString::from(format!("mkt-app-connect-{app_id_btn}")))
            .flex_shrink_0()
            .cursor_pointer()
            .px(px(11.0))
            .py(px(5.0))
            .rounded(px(8.0))
            .bg(ctrl_bg)
            .border_1()
            .border_color(ob.border_hairline())
            .hover(move |s| s.bg(ctrl_hover))
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(ob.text_primary())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if is_open_plugin {
                        if let Some(ref slug) = open_plugin_slug {
                            cx.emit(ContentHubEvent::MarketplaceConnectPlugin {
                                slug: slug.clone(),
                                auth_values: BTreeMap::new(),
                                override_url: None,
                            });
                            return;
                        }
                    }
                    this.marketplace.open_setup_for_app(&app_id_btn);
                    this.marketplace.push_route(MarketplaceRoute::AppDetail {
                        app_id: app_id_btn.clone(),
                    });
                    cx.notify();
                }),
            )
            .child(format!("+ {}", tr_str(lang, "连接", "Connect")))
            .into_any_element()
    };

    div()
        .id(SharedString::from(format!("mkt-app-row-{app_id}")))
        .cursor_pointer()
        .w_full()
        .px(px(8.0))
        .py(px(8.0))
        .rounded(px(10.0))
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .gap(px(12.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.marketplace.open_setup_for_app(&app_id_open);
                this.marketplace.push_route(MarketplaceRoute::AppDetail {
                    app_id: app_id_open.clone(),
                });
                cx.notify();
            }),
        )
        .child(render_app_logo_tile(app, 40.0))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.text_primary())
                        .truncate()
                        .child(app.name().to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(ob.text_muted())
                        .truncate()
                        .child(tagline),
                ),
        )
        .child(action_el)
}

fn render_two_col_grid(items: Vec<AnyElement>) -> Div {
    let mut grid = div().w_full().flex().flex_col().gap(px(4.0));
    let mut iter = items.into_iter();
    while let Some(left) = iter.next() {
        let right = iter.next();
        grid = grid.child(
            div()
                .w_full()
                .flex()
                .gap(px(20.0))
                .child(div().flex_1().min_w(px(0.0)).child(left))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .when_some(right, |slot, el| slot.child(el)),
                ),
        );
    }
    grid
}

fn render_apps_browse(
    ws: &mut ContentHubWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let groups = ws.marketplace.grouped_apps(lang);
    let search_focused = ws.active_input == ContentHubInputTarget::MarketplaceSearch;
    let search_field = ws.marketplace.search.clone();
    let agents_count = ws.marketplace.filtered_agents().len();
    let skills_count = ws.marketplace.filtered_skills().len();

    let mut content = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(22.0))
        // Header: Title + Subtitle + Toolbar (Search + "+ 添加自定义项")
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(24.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.text_primary())
                                .child(tr_str(lang, "应用", "Apps")),
                        )
                        .child(
                            div()
                                .text_size(px(13.5))
                                .text_color(ob.text_muted())
                                .child(tr_str(
                                    lang,
                                    "连接你的助手使用的工具和服务。",
                                    "Connect the tools and services your agents use.",
                                )),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(
                            div().flex_1().min_w(px(0.0)).child(
                                render_editable_field_box(
                                    "mkt-apps-search-input",
                                    &search_field,
                                    tr_str(lang, "搜索", "Search"),
                                    search_focused,
                                    false,
                                    pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.focus_input(
                                            ContentHubInputTarget::MarketplaceSearch,
                                            cx,
                                        );
                                    }),
                                ),
                            ),
                        )
                        .child(
                            openbot_primary_cta_btn(
                                "mkt-add-custom-server-btn",
                                format!("+ {}", tr_str(lang, "添加自定义项", "Add custom")),
                                pal,
                            )
                            .h(px(34.0))
                            .px(px(14.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.marketplace.custom_form.reset_new();
                                    this.marketplace.push_route(
                                        MarketplaceRoute::CustomServerForm {
                                            edit_server_id: None,
                                        },
                                    );
                                    cx.notify();
                                }),
                            ),
                        ),
                ),
        );

    if groups.is_empty() {
        let mut others = Vec::new();
        if agents_count > 0 {
            others.push((MarketplaceNavTab::Agents, agents_count));
        }
        if skills_count > 0 {
            others.push((MarketplaceNavTab::Skills, skills_count));
        }
        content = content.child(render_no_match_plush_state(
            tr_str(lang, "没有匹配的应用。", "No matching apps."),
            others,
            pal,
            lang,
            cx,
        ));
    } else {
        for (cat_slug, items) in groups {
            let total = items.len();
            let cat_owned = cat_slug.to_string();
            let rows: Vec<AnyElement> = items
                .iter()
                .take(APP_GROUP_LIMIT)
                .map(|app| {
                    let connected = ws.marketplace.is_app_connected(&app.id());
                    render_app_row(app, connected, pal, lang, cx).into_any_element()
                })
                .collect();

            content = content.child(
                div()
                    .w_full()
                    .pt(px(16.0))
                    .border_t_1()
                    .border_color(ob.border_hairline())
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(render_group_header(
                        SharedString::from(format!("mkt-apps-see-all-{cat_slug}")),
                        category_localized_label(cat_slug, lang),
                        Some(total),
                        total > APP_GROUP_LIMIT,
                        pal,
                        lang,
                        move |this, _cx| {
                            this.marketplace.push_route(MarketplaceRoute::AppGroup {
                                category: cat_owned.clone(),
                            });
                        },
                        cx,
                    ))
                    .child(render_two_col_grid(rows)),
            );
        }
    }

    content.into_any_element()
}

fn render_app_group_page(
    ws: &mut ContentHubWorkspace,
    category: &str,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let search_focused = ws.active_input == ContentHubInputTarget::MarketplaceSearch;
    let search_field = ws.marketplace.search.clone();
    let cat_label = category_localized_label(category, lang);

    let items: Vec<MarketplaceAppEntry> = ws
        .marketplace
        .filtered_apps(lang)
        .into_iter()
        .filter(|a| a.category().eq_ignore_ascii_case(category))
        .collect();

    let rows: Vec<AnyElement> = items
        .iter()
        .map(|app| {
            let connected = ws.marketplace.is_app_connected(&app.id());
            render_app_row(app, connected, pal, lang, cx).into_any_element()
        })
        .collect();

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(18.0))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(22.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(cat_label),
                )
                .child(
                    div()
                        .text_size(px(14.0))
                        .text_color(ob.text_muted())
                        .child(format!("{}", items.len())),
                ),
        )
        .child(
            render_editable_field_box(
                "mkt-appgroup-search-input",
                &search_field,
                tr_str(lang, "在当前分类中搜索…", "Search in category…"),
                search_focused,
                false,
                pal,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.focus_input(ContentHubInputTarget::MarketplaceSearch, cx);
                }),
            ),
        )
        .child(if rows.is_empty() {
            render_no_match_plush_state(
                tr_str(lang, "没有匹配的应用。", "No matching apps."),
                Vec::new(),
                pal,
                lang,
                cx,
            )
            .into_any_element()
        } else {
            render_two_col_grid(rows).into_any_element()
        })
        .into_any_element()
}

fn render_app_detail_page(
    ws: &mut ContentHubWorkspace,
    app_id: &str,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let Some(app) = ws.marketplace.find_app(app_id) else {
        return div()
            .p(px(24.0))
            .text_size(px(13.5))
            .text_color(ob.text_muted())
            .child(tr_str(lang, "市场目录中未找到该应用。", "This app was not found."))
            .into_any_element();
    };

    if ws
        .marketplace
        .setup_draft
        .as_ref()
        .is_none_or(|d| d.app_id != app_id)
    {
        ws.marketplace.open_setup_for_app(app_id);
    }

    let connected = ws.marketplace.is_app_connected(app_id);
    let connected_rec = ws.marketplace.connected_record(app_id).cloned();
    let linked_server_id = ws.marketplace.linked_mcp_server_id(app_id);
    let tagline = app.localized_tagline(lang);
    let description = app.localized_description(lang);
    let app_name = app.name().to_string();

    let status_pill = div()
        .px(px(8.0))
        .py(px(2.5))
        .rounded(px(999.0))
        .bg(if connected {
            rgba(0x31CF7624)
        } else {
            ob.bg_control()
        })
        .border_1()
        .border_color(if connected {
            rgba(0x31CF7655)
        } else {
            ob.border_hairline()
        })
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if connected {
            ob.success()
        } else {
            ob.text_muted()
        })
        .child(if connected {
            tr_str(lang, "已连接", "Connected")
        } else {
            tr_str(lang, "未连接", "Not connected")
        });

    let share_url = match &app {
        MarketplaceAppEntry::Connector(c) => format!("https://openbot.app/apps/{}", c.slug),
        MarketplaceAppEntry::Plugin(p) => format!("https://openbot.app/apps/{}", p.slug),
        MarketplaceAppEntry::Custom(s) => format!("openbot://mcp/{}", s.id),
    };

    // Hero header
    let mut header_actions = div().flex().items_center().gap(px(8.0)).child(
        openbot_secondary_btn(
            "mkt-app-copy-link-btn",
            tr_str(lang, "复制链接", "Copy link"),
            pal,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |_this, _, _, cx| {
                cx.emit(ContentHubEvent::MarketplaceCopyLink {
                    url: share_url.clone(),
                });
            }),
        ),
    );

    if let MarketplaceAppEntry::Custom(ref srv) = app {
        let srv_clone = srv.clone();
        header_actions = header_actions.child(
            openbot_primary_cta_btn("mkt-custom-edit-btn", tr_str(lang, "编辑", "Edit"), pal)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.marketplace.custom_form.load_from_server(&srv_clone);
                        this.marketplace
                            .push_route(MarketplaceRoute::CustomServerForm {
                                edit_server_id: Some(srv_clone.id.clone()),
                            });
                        cx.notify();
                    }),
                ),
        );
    }

    let mut page = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(18.0))
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(14.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap(px(14.0))
                        .child(render_app_logo_tile(&app, 48.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .text_size(px(20.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(ob.text_primary())
                                                .child(app_name.clone()),
                                        )
                                        .child(status_pill),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(ob.text_muted())
                                        .child(tagline),
                                ),
                        ),
                )
                .child(header_actions),
        );

    // Setup / Connection Card tailored to Connector, Plugin, or Custom Server
    match &app {
        MarketplaceAppEntry::Connector(conn) => {
            page = page.child(render_native_connector_setup_card(
                ws,
                conn,
                connected,
                connected_rec.as_ref().and_then(|r| r.account_label.as_deref()),
                pal,
                lang,
                cx,
            ));
        }
        MarketplaceAppEntry::Plugin(plugin) => {
            // Try prompts card (1:1 OpenBot `TryCard`)
            if !plugin.prompts.is_empty() {
                let is_zh = lang.starts_with("zh");
                let pname = plugin.name.clone();
                let mut try_card = apple_grouped_card(pal)
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ob.text_primary())
                            .child(tr_str(lang, "试一试", "Try asking")),
                    );
                for (idx, pr) in plugin.prompts.iter().enumerate() {
                    let prompt_text = if is_zh {
                        pr.zh_text.as_deref().unwrap_or(&pr.text).to_string()
                    } else {
                        pr.text.clone()
                    };
                    let send_text = prompt_text.clone();
                    let send_app = pname.clone();
                    let hover_bg = ob.bg_control_hover();
                    try_card = try_card.child(
                        div()
                            .id(SharedString::from(format!("mkt-plugin-prompt-{idx}")))
                            .cursor_pointer()
                            .px(px(12.0))
                            .py(px(9.0))
                            .rounded(px(9.0))
                            .bg(ob.bg_canvas())
                            .border_1()
                            .border_color(ob.border_hairline())
                            .hover(move |s| s.bg(hover_bg))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |_this, _, _, cx| {
                                    cx.emit(ContentHubEvent::MarketplaceRunPrompt {
                                        app_name: send_app.clone(),
                                        prompt: send_text.clone(),
                                    });
                                }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(px(12.5))
                                    .text_color(ob.text_secondary())
                                    .child(prompt_text),
                            )
                            .child(tg_icon_rgba(IconName::ChevronRight, 13.0, ob.text_muted())),
                    );
                }
                page = page.child(try_card);
            }

            page = page.child(render_plugin_setup_card(
                ws,
                plugin,
                connected,
                connected_rec.as_ref().and_then(|r| r.account_label.as_deref()),
                pal,
                lang,
                cx,
            ));
        }
        MarketplaceAppEntry::Custom(srv) => {
            page = page.child(render_custom_server_info_card(srv, pal, lang));
        }
    }

    // Agent access control section (when connected & backed by an MCP server)
    if let Some(ref srv_id) = linked_server_id {
        if !ws.marketplace.bots.is_empty() {
            let allowed_opt = ws.marketplace.server_access.get(srv_id).cloned().flatten();
            let mut access_card = apple_grouped_card(pal)
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(if lang.starts_with("zh") {
                            format!("可使用 {app_name} 的助手")
                        } else {
                            format!("Agents that can use {app_name}")
                        }),
                );

            for (idx, bot) in ws.marketplace.bots.iter().enumerate() {
                let is_allowed = match &allowed_opt {
                    None => true,
                    Some(ids) => ids.iter().any(|id| id == &bot.openbot_agent_id),
                };
                let sid = srv_id.clone();
                let aid = bot.openbot_agent_id.clone();
                let hover_bg = ob.bg_control_hover();
                access_card = access_card.child(
                    div()
                        .id(SharedString::from(format!("mkt-access-bot-{idx}")))
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
                            cx.listener(move |_this, _, _, cx| {
                                cx.emit(ContentHubEvent::MarketplaceToggleServerAgentAccess {
                                    server_id: sid.clone(),
                                    agent_id: aid.clone(),
                                    allowed: !is_allowed,
                                });
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(ob.text_primary())
                                .child(bot.name.clone()),
                        )
                        .child(
                            div()
                                .px(px(9.0))
                                .py(px(3.0))
                                .rounded(px(999.0))
                                .bg(if is_allowed {
                                    rgba(0x31CF7624)
                                } else {
                                    ob.bg_control()
                                })
                                .text_size(px(11.5))
                                .text_color(if is_allowed {
                                    ob.success()
                                } else {
                                    ob.text_muted()
                                })
                                .child(if is_allowed {
                                    tr_str(lang, "可使用", "Allowed")
                                } else {
                                    tr_str(lang, "已禁用", "Disabled")
                                }),
                        ),
                );
            }
            page = page.child(access_card);
        }
    }

    // Information Card
    let (developer, version, links) = match &app {
        MarketplaceAppEntry::Connector(c) => {
            let mut l = Vec::new();
            if let Some(ref u) = c.website_url {
                l.push((tr_str(lang, "网站", "Website"), u.clone()));
            }
            if let Some(ref u) = c.privacy_policy_url {
                l.push((tr_str(lang, "隐私政策", "Privacy Policy"), u.clone()));
            }
            if let Some(ref u) = c.terms_url {
                l.push((tr_str(lang, "条款", "Terms"), u.clone()));
            }
            (c.creator_name.clone(), None, l)
        }
        MarketplaceAppEntry::Plugin(p) => {
            let mut l = Vec::new();
            if let Some(ref u) = p.website_url {
                l.push((tr_str(lang, "网站", "Website"), u.clone()));
            }
            if let Some(ref u) = p.privacy_policy_url {
                l.push((tr_str(lang, "隐私政策", "Privacy Policy"), u.clone()));
            }
            if let Some(ref u) = p.terms_url {
                l.push((tr_str(lang, "条款", "Terms"), u.clone()));
            }
            (p.creator_name.clone(), Some(p.version.clone()), l)
        }
        MarketplaceAppEntry::Custom(_) => (
            tr_str(lang, "自定义", "Custom").to_string(),
            None,
            Vec::new(),
        ),
    };

    let info_card = apple_grouped_card(pal)
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_primary())
                .child(tr_str(lang, "信息", "Information")),
        )
        .child(
            div()
                .text_size(px(13.0))
                .line_height(px(20.0))
                .text_color(ob.text_secondary())
                .child(description),
        )
        .child(
            div()
                .pt(px(10.0))
                .border_t_1()
                .border_color(ob.border_hairline())
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(info_fact_row(
                    tr_str(lang, "开发者", "Developer"),
                    developer,
                    pal,
                ))
                .child(info_fact_row(
                    tr_str(lang, "分类", "Category"),
                    category_localized_label(app.category(), lang).to_string(),
                    pal,
                ))
                .when_some(version, |el, v| {
                    el.child(info_fact_row(tr_str(lang, "版本", "Version"), v, pal))
                })
                .children(links.into_iter().enumerate().map(|(i, (lbl, url))| {
                    let url_open = url.clone();
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(ob.text_muted())
                                .child(lbl),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("mkt-info-link-{i}")))
                                .cursor_pointer()
                                .text_size(px(12.5))
                                .text_color(ob.accent_text())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |_this, _, _, cx| {
                                        cx.emit(ContentHubEvent::MarketplaceOpenExternalUrl {
                                            url: url_open.clone(),
                                        });
                                    }),
                                )
                                .child(url),
                        )
                })),
        );
    page = page.child(info_card);

    // Danger Zone (Disconnect / Remove)
    if connected || matches!(app, MarketplaceAppEntry::Custom(_)) {
        let is_custom = matches!(app, MarketplaceAppEntry::Custom(_));
        let custom_srv_id = match &app {
            MarketplaceAppEntry::Custom(s) => Some(s.id.clone()),
            _ => None,
        };
        let app_id_disc = app_id.to_string();
        page = page.child(
            apple_grouped_card(pal)
                .p(px(16.0))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.text_primary())
                                .child(if is_custom {
                                    tr_str(lang, "移除服务器", "Remove server").to_string()
                                } else if lang.starts_with("zh") {
                                    format!("断开 {app_name}")
                                } else {
                                    format!("Disconnect {app_name}")
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(ob.text_muted())
                                .child(if is_custom {
                                    tr_str(
                                        lang,
                                        "你的助手将无法再使用该服务器，其设置会被删除。",
                                        "Your agents will no longer be able to use this server.",
                                    )
                                } else {
                                    tr_str(
                                        lang,
                                        "从这台电脑移除该连接及其凭证，之后可随时重新连接。",
                                        "Remove this connection and its saved credentials from this computer.",
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .id("mkt-app-disconnect-btn")
                        .cursor_pointer()
                        .flex_shrink_0()
                        .px(px(12.0))
                        .py(px(7.0))
                        .rounded(px(8.0))
                        .bg(rgba(0xE5393522))
                        .border_1()
                        .border_color(rgba(0xE5393555))
                        .hover(|s| s.bg(rgba(0xE5393538)))
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.danger())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                if let Some(ref sid) = custom_srv_id {
                                    cx.emit(ContentHubEvent::MarketplaceRemoveCustomMcp {
                                        server_id: sid.clone(),
                                    });
                                    this.marketplace.pop_route();
                                } else {
                                    cx.emit(ContentHubEvent::MarketplaceDisconnectApp {
                                        app_id: app_id_disc.clone(),
                                    });
                                }
                                cx.notify();
                            }),
                        )
                        .child(if is_custom {
                            tr_str(lang, "移除", "Remove")
                        } else {
                            tr_str(lang, "断开连接", "Disconnect")
                        }),
                ),
        );
    }

    page.into_any_element()
}

fn render_native_connector_setup_card(
    ws: &mut ContentHubWorkspace,
    conn: &'static OpenBotConnectorCatalogItem,
    connected: bool,
    current_label: Option<&str>,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Div {
    let ob = pal.openbot_palette();
    let draft = ws.marketplace.setup_draft.clone().unwrap_or_default();
    let pri_focused = ws.active_input == ContentHubInputTarget::MarketplaceSetupPrimary;
    let sec_focused = ws.active_input == ContentHubInputTarget::MarketplaceSetupSecondary;
    let conn_id = conn.connector_id.clone();

    let (pri_label, pri_placeholder, sec_label, sec_placeholder) = match conn.connector_id.as_str() {
        "slack" => (
            tr_str(lang, "Slack 工作区名称", "Slack Workspace Name"),
            tr_str(lang, "例如：Acme Engineering", "e.g. Acme Engineering"),
            tr_str(lang, "Bot Token / Signing Secret (可选)", "Bot Token (xoxb-..., optional)"),
            "xoxb-...",
        ),
        "discord" => (
            tr_str(lang, "Discord 服务器名称", "Discord Server Name"),
            tr_str(lang, "例如：OpenBot Community", "e.g. OpenBot Community"),
            tr_str(lang, "Discord Bot Token (可选)", "Discord Bot Token (optional)"),
            "MT...",
        ),
        "telegram" => (
            tr_str(lang, "Telegram 机器人或群组名称", "Telegram Bot / Group Label"),
            tr_str(lang, "例如：@my_openbot", "e.g. @my_openbot"),
            tr_str(lang, "BotFather Token (可选)", "BotFather Token (optional)"),
            "123456:ABC...",
        ),
        "whatsapp" => (
            tr_str(lang, "WhatsApp 账号或手机号", "WhatsApp Account / Phone"),
            tr_str(lang, "例如：+86 138 0000 0000", "e.g. +1 415 555 0100"),
            tr_str(lang, "配对码或允许列表备注 (可选)", "Pairing Note / Allowlist (optional)"),
            tr_str(lang, "仅限自己和允许的联系人", "Self-chat & allowlist"),
        ),
        "github" => (
            tr_str(lang, "GitHub 账号或组织", "GitHub Account / Org"),
            tr_str(lang, "例如：octocat", "e.g. octocat"),
            tr_str(lang, "Personal Access Token (ghp_...)", "Personal Access Token (ghp_...)"),
            "ghp_...",
        ),
        "onepassword" | "bitwarden" | "protonpass" => (
            tr_str(lang, "共享保管库 / 文件夹名称", "Shared Vault / Folder Name"),
            tr_str(lang, "例如：OpenBot Shared", "e.g. OpenBot Shared"),
            tr_str(lang, "服务账号令牌 / API Key (可选)", "Service Account Token (optional)"),
            "ops_...",
        ),
        _ => (
            tr_str(lang, "账号标识", "Account / Handle"),
            tr_str(lang, "例如：@openbot_", "e.g. @openbot_"),
            tr_str(lang, "扩展授权令牌 (可选)", "Extension Token (optional)"),
            "",
        ),
    };

    apple_grouped_card(pal)
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(tr_str(lang, "连接配置", "Connection Setup")),
                )
                .when_some(current_label, |el, lbl| {
                    el.child(
                        div()
                            .text_size(px(12.0))
                            .text_color(ob.success())
                            .child(format!("✓ {lbl}")),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.text_secondary())
                        .child(pri_label),
                )
                .child(
                    render_editable_field_box(
                        "mkt-native-primary-input",
                        &draft.primary,
                        pri_placeholder,
                        pri_focused,
                        false,
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.focus_input(ContentHubInputTarget::MarketplaceSetupPrimary, cx);
                        }),
                    ),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.text_secondary())
                        .child(sec_label),
                )
                .child(
                    render_editable_field_box(
                        "mkt-native-secondary-input",
                        &draft.secondary,
                        sec_placeholder,
                        sec_focused,
                        false,
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.focus_input(ContentHubInputTarget::MarketplaceSetupSecondary, cx);
                        }),
                    ),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(ob.text_muted())
                        .child(tr_str(
                            lang,
                            "连接凭据仅保存在本机安全存储中，不会上传到第三方。",
                            "Credentials are stored locally on this computer.",
                        )),
                )
                .child(
                    openbot_primary_cta_btn(
                        "mkt-native-connect-submit",
                        if connected {
                            tr_str(lang, "更新连接", "Update connection").to_string()
                        } else if lang.starts_with("zh") {
                            format!("连接 {}", conn.name)
                        } else {
                            format!("Connect {}", conn.name)
                        },
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            let (acct, cred) = this
                                .marketplace
                                .setup_draft
                                .as_ref()
                                .map(|d| (d.primary.text.trim().to_string(), d.secondary.text.trim().to_string()))
                                .unwrap_or_default();
                            let mut meta = BTreeMap::new();
                            if !acct.is_empty() {
                                meta.insert("workspace".to_string(), acct.clone());
                            }
                            cx.emit(ContentHubEvent::MarketplaceConnectNative {
                                connector_id: conn_id.clone(),
                                account_label: (!acct.is_empty()).then_some(acct),
                                credential: (!cred.is_empty()).then_some(cred),
                                metadata: meta,
                            });
                            cx.notify();
                        }),
                    ),
                ),
        )
}

fn render_plugin_setup_card(
    ws: &mut ContentHubWorkspace,
    plugin: &'static OpenBotPluginCatalogItem,
    connected: bool,
    current_label: Option<&str>,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Div {
    let ob = pal.openbot_palette();
    let draft = ws.marketplace.setup_draft.clone().unwrap_or_default();
    let pri_focused = ws.active_input == ContentHubInputTarget::MarketplaceSetupPrimary;
    let sec_focused = ws.active_input == ContentHubInputTarget::MarketplaceSetupSecondary;
    let slug = plugin.slug.clone();

    let endpoint_display = plugin
        .url
        .clone()
        .or_else(|| {
            plugin.command.as_ref().map(|cmd| {
                if plugin.args.is_empty() {
                    cmd.clone()
                } else {
                    format!("{cmd} {}", plugin.args.join(" "))
                }
            })
        })
        .unwrap_or_else(|| plugin.server_name.clone());

    let mut card = apple_grouped_card(pal)
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.text_primary())
                                .child(tr_str(lang, "连接", "Connection")),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(ob.text_muted())
                                .child(format!("MCP ({}) · {}", plugin.transport.to_uppercase(), endpoint_display)),
                        ),
                )
                .when_some(current_label, |el, lbl| {
                    el.child(
                        div()
                            .text_size(px(11.5))
                            .text_color(ob.success())
                            .child(format!("✓ {lbl}")),
                    )
                }),
        );

    match plugin.auth_kind.as_str() {
        "open" => {
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(ob.text_secondary())
                            .child(tr_str(lang, "无需登录或密钥，一键连接即可使用。", "No sign-in or API key required.")),
                    )
                    .child(
                        openbot_primary_cta_btn(
                            "mkt-plugin-open-connect",
                            if connected {
                                tr_str(lang, "重新连接", "Reconnect")
                            } else {
                                tr_str(lang, "连接", "Connect")
                            },
                            pal,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _, _, cx| {
                                cx.emit(ContentHubEvent::MarketplaceConnectPlugin {
                                    slug: slug.clone(),
                                    auth_values: BTreeMap::new(),
                                    override_url: None,
                                });
                            }),
                        ),
                    ),
            );
        }
        "key" => {
            if let Some(first_field) = plugin.auth_fields.first() {
                let hint = first_field.hint.clone().unwrap_or_default();
                let placeholder = first_field
                    .placeholder
                    .clone()
                    .unwrap_or_else(|| tr_str(lang, "输入 API 密钥…", "Enter API key…").to_string());
                card = card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_secondary())
                                .child(first_field.label.clone()),
                        )
                        .child(
                            render_editable_field_box(
                                "mkt-plugin-key-primary",
                                &draft.primary,
                                &placeholder,
                                pri_focused,
                                false,
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.focus_input(
                                        ContentHubInputTarget::MarketplaceSetupPrimary,
                                        cx,
                                    );
                                }),
                            ),
                        )
                        .when(!hint.is_empty(), |el| {
                            el.child(div().text_size(px(11.5)).text_color(ob.text_muted()).child(hint))
                        }),
                );
            }
            if let Some(second_field) = plugin.auth_fields.get(1) {
                let placeholder = second_field.placeholder.clone().unwrap_or_default();
                card = card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_secondary())
                                .child(second_field.label.clone()),
                        )
                        .child(
                            render_editable_field_box(
                                "mkt-plugin-key-secondary",
                                &draft.secondary,
                                &placeholder,
                                sec_focused,
                                false,
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.focus_input(
                                        ContentHubInputTarget::MarketplaceSetupSecondary,
                                        cx,
                                    );
                                }),
                            ),
                        ),
                );
            }
            let docs_url = plugin.auth_docs_url.clone();
            let docs_lbl = plugin
                .auth_docs_label
                .clone()
                .unwrap_or_else(|| tr_str(lang, "获取密钥", "Get a key").to_string());
            let first_id = plugin
                .auth_fields
                .first()
                .map(|f| f.id.clone())
                .unwrap_or_else(|| "token".to_string());
            let second_id = plugin.auth_fields.get(1).map(|f| f.id.clone());

            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .child(
                        div()
                            .when_some(docs_url, |el, u| {
                                el.child(
                                    div()
                                        .id("mkt-plugin-key-docs-link")
                                        .cursor_pointer()
                                        .text_size(px(12.0))
                                        .text_color(ob.accent_text())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |_this, _, _, cx| {
                                                cx.emit(ContentHubEvent::MarketplaceOpenExternalUrl {
                                                    url: u.clone(),
                                                });
                                            }),
                                        )
                                        .child(format!("{docs_lbl} ↗")),
                                )
                            }),
                    )
                    .child(
                        openbot_primary_cta_btn(
                            "mkt-plugin-key-connect-btn",
                            if connected {
                                tr_str(lang, "更换密钥并重连", "Save key & reconnect")
                            } else {
                                tr_str(lang, "保存密钥并连接", "Connect")
                            },
                            pal,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                let mut vals = BTreeMap::new();
                                if let Some(ref d) = this.marketplace.setup_draft {
                                    vals.insert(first_id.clone(), d.primary.text.trim().to_string());
                                    if let Some(ref sid) = second_id {
                                        vals.insert(sid.clone(), d.secondary.text.trim().to_string());
                                    }
                                }
                                cx.emit(ContentHubEvent::MarketplaceConnectPlugin {
                                    slug: slug.clone(),
                                    auth_values: vals,
                                    override_url: None,
                                });
                            }),
                        ),
                    ),
            );
        }
        "local" => {
            if !plugin.auth_steps.is_empty() {
                let mut steps_col = div().flex().flex_col().gap(px(5.0));
                for (i, step) in plugin.auth_steps.iter().enumerate() {
                    steps_col = steps_col.child(
                        div()
                            .text_size(px(12.5))
                            .text_color(ob.text_secondary())
                            .child(format!("{}. {step}", i + 1)),
                    );
                }
                card = card.child(steps_col);
            }
            if let Some(ref note) = plugin.auth_note {
                card = card.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(ob.text_muted())
                        .child(note.clone()),
                );
            }
            card = card
                .child(
                    render_editable_field_box(
                        "mkt-plugin-local-url",
                        &draft.primary,
                        "http://127.0.0.1:3845/mcp",
                        pri_focused,
                        false,
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.focus_input(ContentHubInputTarget::MarketplaceSetupPrimary, cx);
                        }),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .child(
                            openbot_primary_cta_btn(
                                "mkt-plugin-local-connect-btn",
                                if connected {
                                    tr_str(lang, "再次连接", "Connect again")
                                } else {
                                    tr_str(lang, "连接本地服务", "Connect local server")
                                },
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    let url_override = this
                                        .marketplace
                                        .setup_draft
                                        .as_ref()
                                        .map(|d| d.primary.text.trim().to_string())
                                        .filter(|s| !s.is_empty());
                                    cx.emit(ContentHubEvent::MarketplaceConnectPlugin {
                                        slug: slug.clone(),
                                        auth_values: BTreeMap::new(),
                                        override_url: url_override,
                                    });
                                }),
                            ),
                        ),
                );
        }
        _ => {
            // OAuth / Browser sign-in (`link`)
            let sign_in_url = plugin.url.clone().or_else(|| plugin.website_url.clone());
            card = card
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(ob.text_secondary())
                        .child(if lang.starts_with("zh") {
                            format!(
                                "在浏览器中使用你的 {} 账号登录，或直接粘贴访问令牌以立即连接 MCP 服务器。",
                                plugin.name
                            )
                        } else {
                            format!(
                                "Sign in with your {} account in the browser, or paste an optional access token to connect the MCP server.",
                                plugin.name
                            )
                        }),
                )
                .child(
                    render_editable_field_box(
                        "mkt-plugin-oauth-token",
                        &draft.primary,
                        tr_str(lang, "可选：直接粘贴 OAuth / Bearer Token…", "Optional: paste OAuth / Bearer token…"),
                        pri_focused,
                        false,
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.focus_input(ContentHubInputTarget::MarketplaceSetupPrimary, cx);
                        }),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(8.0))
                        .when_some(sign_in_url, |el, u| {
                            el.child(
                                openbot_secondary_btn(
                                    "mkt-plugin-open-browser-auth",
                                    tr_str(lang, "打开浏览器授权 ↗", "Open sign-in page ↗"),
                                    pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |_this, _, _, cx| {
                                        cx.emit(ContentHubEvent::MarketplaceOpenExternalUrl {
                                            url: u.clone(),
                                        });
                                    }),
                                ),
                            )
                        })
                        .child(
                            openbot_primary_cta_btn(
                                "mkt-plugin-link-connect-btn",
                                if connected {
                                    tr_str(lang, "重新连接", "Reconnect")
                                } else {
                                    tr_str(lang, "连接", "Connect")
                                },
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    let mut vals = BTreeMap::new();
                                    if let Some(ref d) = this.marketplace.setup_draft {
                                        let tok = d.primary.text.trim().to_string();
                                        if !tok.is_empty() {
                                            vals.insert("oauth_token".to_string(), tok);
                                        }
                                    }
                                    cx.emit(ContentHubEvent::MarketplaceConnectPlugin {
                                        slug: slug.clone(),
                                        auth_values: vals,
                                        override_url: None,
                                    });
                                }),
                            ),
                        ),
                );
        }
    }

    card
}

fn render_custom_server_info_card(
    srv: &McpServerConfig,
    pal: TdesktopThemePalette,
    lang: &str,
) -> Div {
    let ob = pal.openbot_palette();
    let is_stdio = srv.transport == McpTransport::Stdio;
    let addr = if is_stdio {
        if srv.args.is_empty() {
            srv.command.clone()
        } else {
            format!("{} {}", srv.command, srv.args.join(" "))
        }
    } else {
        srv.url.clone()
    };

    apple_grouped_card(pal)
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(13.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_primary())
                .child(tr_str(lang, "服务器", "Server")),
        )
        .child(info_fact_row(
            tr_str(lang, "传输方式", "Transport"),
            if is_stdio {
                "STDIO".to_string()
            } else {
                "Streamable HTTP".to_string()
            },
            pal,
        ))
        .child(info_fact_row(
            if is_stdio {
                tr_str(lang, "命令", "Command")
            } else {
                tr_str(lang, "地址", "Address")
            },
            addr,
            pal,
        ))
        .when(is_stdio && !srv.working_directory.trim().is_empty(), |el| {
            el.child(info_fact_row(
                tr_str(lang, "工作目录", "Working directory"),
                srv.working_directory.clone(),
                pal,
            ))
        })
        .when(!srv.env.is_empty(), |el| {
            el.child(info_fact_row(
                tr_str(lang, "环境变量", "Environment variables"),
                format!("{} {}", srv.env.len(), tr_str(lang, "项已配置", "configured")),
                pal,
            ))
        })
        .when(!srv.headers.is_empty(), |el| {
            el.child(info_fact_row(
                tr_str(lang, "请求头", "Headers"),
                format!("{} {}", srv.headers.len(), tr_str(lang, "项已配置", "configured")),
                pal,
            ))
        })
}

fn render_custom_server_form_page(
    ws: &mut ContentHubWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let form = ws.marketplace.custom_form.clone();
    let is_edit = form.edit_server_id.is_some();
    let is_stdio = form.transport == McpTransport::Stdio;
    let active_bg = ob.bg_control_active();
    let hover_bg = ob.bg_control_hover();
    let text_pri = ob.text_primary();
    let text_muted = ob.text_muted();

    let transport_switch = div()
        .p(px(3.0))
        .rounded(px(10.0))
        .bg(ob.bg_canvas())
        .border_1()
        .border_color(ob.border_hairline())
        .flex()
        .items_center()
        .gap(px(2.0))
        .child(
            div()
                .id("mkt-custom-transport-stdio")
                .cursor_pointer()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(7.0))
                .when(is_stdio, |el| el.bg(active_bg))
                .when(!is_stdio, |el| el.hover(move |s| s.bg(hover_bg)))
                .text_size(px(12.0))
                .font_weight(if is_stdio { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                .text_color(if is_stdio { text_pri } else { text_muted })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.marketplace.custom_form.transport = McpTransport::Stdio;
                        cx.notify();
                    }),
                )
                .child("STDIO"),
        )
        .child(
            div()
                .id("mkt-custom-transport-http")
                .cursor_pointer()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(7.0))
                .when(!is_stdio, |el| el.bg(active_bg))
                .when(is_stdio, |el| el.hover(move |s| s.bg(hover_bg)))
                .text_size(px(12.0))
                .font_weight(if !is_stdio { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                .text_color(if !is_stdio { text_pri } else { text_muted })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.marketplace.custom_form.transport = McpTransport::Http;
                        cx.notify();
                    }),
                )
                .child("Streamable HTTP"),
        );

    let mut page = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(22.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(if is_edit {
                            tr_str(lang, "编辑自定义 MCP 服务器", "Edit custom MCP server")
                        } else {
                            tr_str(lang, "添加自定义 MCP 服务器", "Connect a custom MCP server")
                        }),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(ob.text_muted())
                        .child(tr_str(
                            lang,
                            "支持 STDIO 本地命令行或 Streamable HTTP 远程端点，保存后所有授权助手即可调用。",
                            "Connect any STDIO command or Streamable HTTP MCP endpoint for your agents.",
                        )),
                ),
        )
        // Server Details card
        .child(
            apple_grouped_card(pal)
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.text_primary())
                                .child(tr_str(lang, "服务器详情", "Server details")),
                        )
                        .child(transport_switch),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_secondary())
                                .child(tr_str(lang, "名称 *", "Name *")),
                        )
                        .child(
                            render_editable_field_box(
                                "mkt-custom-name-input",
                                &form.name,
                                tr_str(lang, "例如：my-internal-mcp", "e.g. my-internal-mcp"),
                                ws.active_input == ContentHubInputTarget::MarketplaceCustomName,
                                false,
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.focus_input(
                                        ContentHubInputTarget::MarketplaceCustomName,
                                        cx,
                                    );
                                }),
                            ),
                        ),
                ),
        );

    if is_stdio {
        page = page.child(
            apple_grouped_card(pal)
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(tr_str(lang, "启动配置 (STDIO)", "Launch configuration (STDIO)")),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_secondary())
                                .child(tr_str(lang, "命令 *", "Command *")),
                        )
                        .child(
                            render_editable_field_box(
                                "mkt-custom-cmd-input",
                                &form.command,
                                "npx",
                                ws.active_input == ContentHubInputTarget::MarketplaceCustomCommand,
                                false,
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.focus_input(
                                        ContentHubInputTarget::MarketplaceCustomCommand,
                                        cx,
                                    );
                                }),
                            ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_secondary())
                                .child(tr_str(lang, "参数 (空格分隔)", "Arguments (space-separated)")),
                        )
                        .child(
                            render_editable_field_box(
                                "mkt-custom-args-input",
                                &form.args,
                                "-y @modelcontextprotocol/server-filesystem ~/code",
                                ws.active_input == ContentHubInputTarget::MarketplaceCustomArgs,
                                false,
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.focus_input(
                                        ContentHubInputTarget::MarketplaceCustomArgs,
                                        cx,
                                    );
                                }),
                            ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_secondary())
                                .child(tr_str(
                                    lang,
                                    "环境变量 (每行一条 KEY=VALUE)",
                                    "Environment variables (KEY=VALUE per line)",
                                )),
                        )
                        .child(
                            render_editable_field_box(
                                "mkt-custom-env-input",
                                &form.env_lines,
                                "API_KEY=sk-...",
                                ws.active_input == ContentHubInputTarget::MarketplaceCustomEnv,
                                true,
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.focus_input(
                                        ContentHubInputTarget::MarketplaceCustomEnv,
                                        cx,
                                    );
                                }),
                            ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_secondary())
                                .child(tr_str(lang, "工作目录 (可选)", "Working directory (optional)")),
                        )
                        .child(
                            render_editable_field_box(
                                "mkt-custom-workdir-input",
                                &form.working_dir,
                                "~/code",
                                ws.active_input == ContentHubInputTarget::MarketplaceCustomWorkDir,
                                false,
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.focus_input(
                                        ContentHubInputTarget::MarketplaceCustomWorkDir,
                                        cx,
                                    );
                                }),
                            ),
                        ),
                ),
        );
    } else {
        page = page.child(
            apple_grouped_card(pal)
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(tr_str(lang, "端点配置 (Streamable HTTP)", "Endpoint configuration (Streamable HTTP)")),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_secondary())
                                .child(tr_str(lang, "服务器地址 *", "Server URL *")),
                        )
                        .child(
                            render_editable_field_box(
                                "mkt-custom-url-input",
                                &form.url,
                                "https://mcp.example.com/mcp",
                                ws.active_input == ContentHubInputTarget::MarketplaceCustomUrl,
                                false,
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.focus_input(
                                        ContentHubInputTarget::MarketplaceCustomUrl,
                                        cx,
                                    );
                                }),
                            ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_secondary())
                                .child(tr_str(
                                    lang,
                                    "请求头 (每行一条 Key: Value)",
                                    "Headers (Key: Value per line)",
                                )),
                        )
                        .child(
                            render_editable_field_box(
                                "mkt-custom-headers-input",
                                &form.header_lines,
                                "Authorization: Bearer ...",
                                ws.active_input == ContentHubInputTarget::MarketplaceCustomHeaders,
                                true,
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.focus_input(
                                        ContentHubInputTarget::MarketplaceCustomHeaders,
                                        cx,
                                    );
                                }),
                            ),
                        ),
                ),
        );
    }

    if let Some(ref err) = form.error {
        page = page.child(
            div()
                .px(px(12.0))
                .py(px(9.0))
                .rounded(px(9.0))
                .bg(rgba(0xE539351E))
                .border_1()
                .border_color(rgba(0xE5393555))
                .text_size(px(12.5))
                .text_color(ob.danger())
                .child(err.clone()),
        );
    }

    if let Some((ok, ref msg)) = form.test_result {
        page = page.child(
            div()
                .px(px(12.0))
                .py(px(9.0))
                .rounded(px(9.0))
                .bg(if ok { rgba(0x31CF761E) } else { rgba(0xF59E0B1E) })
                .border_1()
                .border_color(if ok { rgba(0x31CF7655) } else { rgba(0xF59E0B55) })
                .text_size(px(12.5))
                .text_color(if ok { ob.success() } else { ob.warning() })
                .child(msg.clone()),
        );
    }

    page = page.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(10.0))
            .child(
                openbot_secondary_btn(
                    "mkt-custom-test-btn",
                    tr_str(lang, "测试连接", "Test connection"),
                    pal,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        let f = &this.marketplace.custom_form;
                        cx.emit(ContentHubEvent::MarketplaceTestCustomMcp {
                            name: f.name.text.trim().to_string(),
                            transport: f.transport,
                            command: f.command.text.trim().to_string(),
                            args: f.parsed_args(),
                            env: f.parsed_env(),
                            working_dir: f.working_dir.text.trim().to_string(),
                            url: f.url.text.trim().to_string(),
                            headers: f.parsed_headers(),
                        });
                    }),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        openbot_secondary_btn(
                            "mkt-custom-cancel-btn",
                            tr_str(lang, "取消", "Cancel"),
                            pal,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.marketplace.pop_route();
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        openbot_primary_cta_btn(
                            "mkt-custom-save-btn",
                            tr_str(lang, "连接并保存", "Connect & Save"),
                            pal,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                let f = &this.marketplace.custom_form;
                                cx.emit(ContentHubEvent::MarketplaceSaveCustomMcp {
                                    edit_server_id: f.edit_server_id.clone(),
                                    name: f.name.text.trim().to_string(),
                                    transport: f.transport,
                                    command: f.command.text.trim().to_string(),
                                    args: f.parsed_args(),
                                    env: f.parsed_env(),
                                    working_dir: f.working_dir.text.trim().to_string(),
                                    url: f.url.text.trim().to_string(),
                                    headers: f.parsed_headers(),
                                });
                            }),
                        ),
                    ),
            ),
    );

    page.into_any_element()
}

fn render_agent_featured_card(
    agent: &'static OpenBotAgentCatalogItem,
    added: bool,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let hover_bg = ob.bg_control_hover();
    let ctrl_bg = ob.bg_control();
    let ctrl_hover = ob.bg_control_active();
    let slug_open = agent.slug.clone();
    let slug_act = agent.slug.clone();
    let hue = AvatarHue::from_degrees(agent.avatar_hue);

    let footer_str = if agent.routines.is_empty() {
        if lang.starts_with("zh") {
            format!("{} 个技能", agent.skills.len())
        } else {
            format!("{} skills", agent.skills.len())
        }
    } else if lang.starts_with("zh") {
        format!(
            "{} 个技能 · {} 个自动化任务",
            agent.skills.len(),
            agent.routines.len()
        )
    } else {
        format!(
            "{} skills · {} routines",
            agent.skills.len(),
            agent.routines.len()
        )
    };

    let btn_el: AnyElement = if added {
        div()
            .id(SharedString::from(format!("mkt-agent-feat-open-{slug_act}")))
            .cursor_pointer()
            .px(px(10.0))
            .py(px(5.0))
            .rounded(px(8.0))
            .bg(ctrl_bg)
            .border_1()
            .border_color(ob.border_hairline())
            .hover(move |s| s.bg(ctrl_hover))
            .text_size(px(12.0))
            .text_color(ob.text_primary())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, _, _, cx| {
                    cx.stop_propagation();
                    cx.emit(ContentHubEvent::MarketplaceOpenAgentChat {
                        slug: slug_act.clone(),
                    });
                }),
            )
            .child(tr_str(lang, "打开对话", "Open chat"))
            .into_any_element()
    } else {
        div()
            .id(SharedString::from(format!("mkt-agent-feat-add-{slug_act}")))
            .cursor_pointer()
            .px(px(11.0))
            .py(px(5.0))
            .rounded(px(8.0))
            .bg(ctrl_bg)
            .border_1()
            .border_color(ob.border_hairline())
            .hover(move |s| s.bg(ctrl_hover))
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(ob.text_primary())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, _, _, cx| {
                    cx.stop_propagation();
                    cx.emit(ContentHubEvent::MarketplaceAddAgent {
                        slug: slug_act.clone(),
                    });
                }),
            )
            .child(format!("+ {}", tr_str(lang, "添加", "Add")))
            .into_any_element()
    };

    let excerpt: String = agent.description.lines().next().unwrap_or(&agent.title).to_string();

    div()
        .id(SharedString::from(format!("mkt-agent-feat-card-{}", agent.slug)))
        .cursor_pointer()
        .flex_1()
        .min_w(px(210.0))
        .p(px(16.0))
        .rounded(px(14.0))
        .bg(ob.bg_surface())
        .border_1()
        .border_color(ob.border_hairline())
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .flex_col()
        .justify_between()
        .gap(px(10.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.marketplace.push_route(MarketplaceRoute::AgentDetail {
                    slug: slug_open.clone(),
                });
                cx.notify();
            }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(render_plush_seed_avatar(
                            &agent.slug,
                            hue,
                            None,
                            AvatarMood::Idle,
                            0.0,
                            true,
                            46.0,
                        ))
                        .child(btn_el),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(14.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.text_primary())
                                .truncate()
                                .child(agent.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(ob.text_muted())
                                .child(if lang.starts_with("zh") {
                                    format!("作者：{}", agent.creator_name)
                                } else {
                                    format!("by {}", agent.creator_name)
                                }),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(18.0))
                        .max_h(px(36.0))
                        .overflow_hidden()
                        .text_color(ob.text_secondary())
                        .child(excerpt),
                ),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(ob.text_muted())
                .child(footer_str),
        )
}

fn render_agent_row(
    agent: &'static OpenBotAgentCatalogItem,
    added: bool,
    yours: bool,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let hover_bg = ob.bg_control_hover();
    let ctrl_bg = ob.bg_control();
    let ctrl_hover = ob.bg_control_active();
    let slug_open = agent.slug.clone();
    let slug_act = agent.slug.clone();
    let hue = AvatarHue::from_degrees(agent.avatar_hue);

    let action_el: AnyElement = if added && yours {
        openbot_secondary_btn(
            SharedString::from(format!("mkt-agent-openchat-{slug_act}")),
            tr_str(lang, "打开对话", "Open chat"),
            pal,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |_this, _, _, cx| {
                cx.stop_propagation();
                cx.emit(ContentHubEvent::MarketplaceOpenAgentChat {
                    slug: slug_act.clone(),
                });
            }),
        )
        .into_any_element()
    } else if added {
        div()
            .flex_shrink_0()
            .px(px(10.0))
            .py(px(5.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .text_size(px(12.5))
            .text_color(ob.text_muted())
            .child(tg_icon_rgba(IconName::SelectCheck, 13.0, ob.success()))
            .child(tr_str(lang, "已添加", "Added"))
            .into_any_element()
    } else {
        div()
            .id(SharedString::from(format!("mkt-agent-add-{slug_act}")))
            .flex_shrink_0()
            .cursor_pointer()
            .px(px(11.0))
            .py(px(5.0))
            .rounded(px(8.0))
            .bg(ctrl_bg)
            .border_1()
            .border_color(ob.border_hairline())
            .hover(move |s| s.bg(ctrl_hover))
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(ob.text_primary())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, _, _, cx| {
                    cx.stop_propagation();
                    cx.emit(ContentHubEvent::MarketplaceAddAgent {
                        slug: slug_act.clone(),
                    });
                }),
            )
            .child(format!("+ {}", tr_str(lang, "添加", "Add")))
            .into_any_element()
    };

    div()
        .id(SharedString::from(format!("mkt-agent-row-{}", agent.slug)))
        .cursor_pointer()
        .w_full()
        .px(px(8.0))
        .py(px(8.0))
        .rounded(px(10.0))
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .gap(px(12.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.marketplace.push_route(MarketplaceRoute::AgentDetail {
                    slug: slug_open.clone(),
                });
                cx.notify();
            }),
        )
        .child(render_plush_seed_avatar(
            &agent.slug,
            hue,
            None,
            AvatarMood::Idle,
            0.0,
            true,
            40.0,
        ))
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
                        .items_baseline()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_primary())
                                .truncate()
                                .child(agent.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(ob.text_muted())
                                .whitespace_nowrap()
                                .child(if lang.starts_with("zh") {
                                    format!("作者：{}", agent.creator_name)
                                } else {
                                    format!("by {}", agent.creator_name)
                                }),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(ob.text_muted())
                        .truncate()
                        .child(agent.title.clone()),
                ),
        )
        .child(action_el)
}

fn render_agents_browse(
    ws: &mut ContentHubWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let found = ws.marketplace.filtered_agents();
    let search_focused = ws.active_input == ContentHubInputTarget::MarketplaceSearch;
    let search_field = ws.marketplace.search.clone();
    let searching = !search_field.text.trim().is_empty();

    let mine: Vec<&'static OpenBotAgentCatalogItem> = found
        .iter()
        .copied()
        .filter(|a| ws.marketplace.is_agent_added(a))
        .collect();
    let available: Vec<&'static OpenBotAgentCatalogItem> = found
        .iter()
        .copied()
        .filter(|a| !ws.marketplace.is_agent_added(a))
        .collect();

    // Featured top 3 (matching OpenBot screenshot: Research Desk, Change Reviewer, Scope Writer)
    let mut featured: Vec<&'static OpenBotAgentCatalogItem> = if searching {
        Vec::new()
    } else {
        let mut picks: Vec<&'static OpenBotAgentCatalogItem> = available
            .iter()
            .copied()
            .filter(|a| a.featured)
            .collect();
        picks.sort_by_key(|a| match a.slug.as_str() {
            "research-desk" => 0,
            "code-review-partner" => 1,
            "scope-writer" => 2,
            _ => 3,
        });
        picks.truncate(3);
        picks
    };
    if !searching && featured.is_empty() {
        featured = available.iter().copied().take(3).collect();
    }

    let mut content = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(22.0))
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_size(px(24.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(ob.text_primary())
                                        .child(tr_str(lang, "助手", "Agents")),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.5))
                                        .text_color(ob.text_muted())
                                        .child(tr_str(
                                            lang,
                                            "带有自身指令和技能的现成助手。",
                                            "Ready-made agents with their own instructions and skills.",
                                        )),
                                ),
                        )
                        .child(render_source_switch(
                            "mkt-agent-src",
                            ws.marketplace.agent_source,
                            true,
                            pal,
                            lang,
                            cx,
                        )),
                )
                .child(
                    render_editable_field_box(
                        "mkt-agents-search-input",
                        &search_field,
                        tr_str(lang, "搜索", "Search"),
                        search_focused,
                        false,
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.focus_input(ContentHubInputTarget::MarketplaceSearch, cx);
                        }),
                    ),
                ),
        );

    if found.is_empty() {
        let mut others = Vec::new();
        let apps_n = ws.marketplace.filtered_apps(lang).len();
        let skills_n = ws.marketplace.filtered_skills().len();
        if apps_n > 0 {
            others.push((MarketplaceNavTab::Apps, apps_n));
        }
        if skills_n > 0 {
            others.push((MarketplaceNavTab::Skills, skills_n));
        }
        content = content.child(render_no_match_plush_state(
            tr_str(lang, "没有匹配的助手。", "No matching agents."),
            others,
            pal,
            lang,
            cx,
        ));
        return content.into_any_element();
    }

    if !mine.is_empty() {
        let rows: Vec<AnyElement> = mine
            .iter()
            .map(|a| render_agent_row(a, true, true, pal, lang, cx).into_any_element())
            .collect();
        content = content.child(
            div()
                .w_full()
                .pt(px(16.0))
                .border_t_1()
                .border_color(ob.border_hairline())
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(render_group_header(
                    "mkt-agents-yours-hdr",
                    tr_str(lang, "我的助手", "Your agents"),
                    Some(mine.len()),
                    false,
                    pal,
                    lang,
                    |_, _| {},
                    cx,
                ))
                .child(render_two_col_grid(rows)),
        );
    }

    if !featured.is_empty() {
        content = content.child(
            div()
                .w_full()
                .pt(px(16.0))
                .border_t_1()
                .border_color(ob.border_hairline())
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .text_size(px(14.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(tr_str(lang, "精选", "Featured")),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .gap(px(12.0))
                        .children(featured.into_iter().map(|ag| {
                            let added = ws.marketplace.is_agent_added(ag);
                            render_agent_featured_card(ag, added, pal, lang, cx)
                        })),
                ),
        );
    }

    for &cat_slug in &SKILL_CATEGORIES {
        let bucket: Vec<&'static OpenBotAgentCatalogItem> = available
            .iter()
            .copied()
            .filter(|a| a.category.eq_ignore_ascii_case(cat_slug))
            .collect();
        if bucket.is_empty() {
            continue;
        }
        let total = bucket.len();
        let cat_owned = cat_slug.to_string();
        let rows: Vec<AnyElement> = bucket
            .iter()
            .take(AGENT_GROUP_LIMIT)
            .map(|a| render_agent_row(a, false, false, pal, lang, cx).into_any_element())
            .collect();
        content = content.child(
            div()
                .w_full()
                .pt(px(16.0))
                .border_t_1()
                .border_color(ob.border_hairline())
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(render_group_header(
                    SharedString::from(format!("mkt-agents-see-all-{cat_slug}")),
                    category_localized_label(cat_slug, lang),
                    Some(total),
                    total > AGENT_GROUP_LIMIT,
                    pal,
                    lang,
                    move |this, _cx| {
                        this.marketplace.push_route(MarketplaceRoute::AgentGroup {
                            category: cat_owned.clone(),
                        });
                    },
                    cx,
                ))
                .child(render_two_col_grid(rows)),
        );
    }

    content.into_any_element()
}

fn render_agent_group_page(
    ws: &mut ContentHubWorkspace,
    category: &str,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let items: Vec<&'static OpenBotAgentCatalogItem> = ws
        .marketplace
        .filtered_agents()
        .into_iter()
        .filter(|a| a.category.eq_ignore_ascii_case(category))
        .collect();
    let rows: Vec<AnyElement> = items
        .iter()
        .map(|a| {
            let added = ws.marketplace.is_agent_added(a);
            render_agent_row(a, added, added, pal, lang, cx).into_any_element()
        })
        .collect();

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .child(
            div()
                .text_size(px(22.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_primary())
                .child(category_localized_label(category, lang)),
        )
        .child(render_two_col_grid(rows))
        .into_any_element()
}

fn render_agent_detail_page(
    ws: &mut ContentHubWorkspace,
    slug: &str,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let Some(agent) = ws
        .marketplace
        .catalog()
        .agents
        .iter()
        .find(|a| a.slug == slug)
    else {
        return div()
            .p(px(24.0))
            .text_color(ob.text_muted())
            .child(tr_str(lang, "市场中没有该助手。", "Agent not found."))
            .into_any_element();
    };

    let added = ws.marketplace.is_agent_added(agent);
    let slug_act = agent.slug.clone();
    let hue = AvatarHue::from_degrees(agent.avatar_hue);

    let primary_action = if added {
        openbot_primary_cta_btn(
            "mkt-agent-detail-open-chat",
            tr_str(lang, "打开对话", "Open chat"),
            pal,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |_this, _, _, cx| {
                cx.emit(ContentHubEvent::MarketplaceOpenAgentChat {
                    slug: slug_act.clone(),
                });
            }),
        )
    } else {
        openbot_primary_cta_btn(
            "mkt-agent-detail-add-btn",
            format!("+ {}", tr_str(lang, "添加助手", "Add agent")),
            pal,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |_this, _, _, cx| {
                cx.emit(ContentHubEvent::MarketplaceAddAgent {
                    slug: slug_act.clone(),
                });
            }),
        )
    };

    let mut page = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(18.0))
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(14.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .items_center()
                        .gap(px(14.0))
                        .child(render_plush_seed_avatar(
                            &agent.slug,
                            hue,
                            None,
                            AvatarMood::Idle,
                            0.0,
                            true,
                            52.0,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .text_size(px(20.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(ob.text_primary())
                                                .child(agent.name.clone()),
                                        )
                                        .when(agent.verified, |el| {
                                            el.child(tg_icon_rgba(
                                                IconName::ShieldMod,
                                                15.0,
                                                ob.success(),
                                            ))
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(ob.text_muted())
                                        .child(format!(
                                            "{} · {}",
                                            agent.title,
                                            if lang.starts_with("zh") {
                                                format!("作者：{}", agent.creator_name)
                                            } else {
                                                format!("by {}", agent.creator_name)
                                            }
                                        )),
                                ),
                        ),
                )
                .child(primary_action),
        );

    // Included Skills
    if !agent.skills.is_empty() {
        let mut sk_card = apple_grouped_card(pal)
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ob.text_primary())
                    .child(tr_str(lang, "包含技能", "Included Skills")),
            );
        for (i, sk_slug) in agent.skills.iter().enumerate() {
            let meta = ws
                .marketplace
                .catalog()
                .skills
                .iter()
                .find(|s| s.slug == *sk_slug);
            let title = meta.map(|m| m.name.clone()).unwrap_or_else(|| sk_slug.clone());
            let desc = meta
                .map(|m| m.description.clone())
                .unwrap_or_else(|| format!("SKILL.md · {sk_slug}"));
            let open_slug = sk_slug.clone();
            let hover_bg = ob.bg_control_hover();
            sk_card = sk_card.child(
                div()
                    .id(SharedString::from(format!("mkt-agent-inc-skill-{i}")))
                    .cursor_pointer()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .hover(move |s| s.bg(hover_bg))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.marketplace.active_tab = MarketplaceNavTab::Skills;
                            this.marketplace.push_route(MarketplaceRoute::SkillDetail {
                                slug: open_slug.clone(),
                            });
                            cx.notify();
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
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_primary())
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(ob.text_muted())
                                    .truncate()
                                    .child(desc),
                            ),
                    )
                    .child(tg_icon_rgba(IconName::ChevronRight, 13.0, ob.text_muted())),
            );
        }
        page = page.child(sk_card);
    }

    // Included Routines
    if !agent.routines.is_empty() {
        let mut rt_card = apple_grouped_card(pal)
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ob.text_primary())
                    .child(tr_str(lang, "包含自动化任务", "Included Routines")),
            );
        for rt in &agent.routines {
            rt_card = rt_card.child(
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .bg(ob.bg_canvas())
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_primary())
                            .child(rt.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(ob.text_muted())
                            .child(rt.schedule_summary.clone()),
                    ),
            );
        }
        page = page.child(rt_card);
    }

    // Instructions / Description card
    page = page.child(
        apple_grouped_card(pal)
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ob.text_primary())
                    .child(tr_str(lang, "指令与信息", "Instructions & Information")),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .line_height(px(19.0))
                    .text_color(ob.text_secondary())
                    .child(agent.description.clone()),
            )
            .child(
                div()
                    .pt(px(10.0))
                    .border_t_1()
                    .border_color(ob.border_hairline())
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(info_fact_row(
                        tr_str(lang, "作者", "Creator"),
                        agent.creator_name.clone(),
                        pal,
                    ))
                    .child(info_fact_row(
                        tr_str(lang, "分类", "Category"),
                        category_localized_label(&agent.category, lang).to_string(),
                        pal,
                    ))
                    .child(info_fact_row(
                        tr_str(lang, "版本", "Version"),
                        format!("v{}", agent.version),
                        pal,
                    )),
            ),
    );

    page.into_any_element()
}

fn render_skill_row(
    ws: &ContentHubWorkspace,
    skill: &'static OpenBotSkillCatalogItem,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let hover_bg = ob.bg_control_hover();
    let ctrl_bg = ob.bg_control();
    let ctrl_hover = ob.bg_control_active();
    let installed_agents = ws.marketplace.agents_with_skill(&skill.slug);
    let installed_count = installed_agents.len();
    let slug_open = skill.slug.clone();
    let slug_btn = skill.slug.clone();
    let first_bot_id = ws
        .marketplace
        .bots
        .first()
        .map(|b| b.openbot_agent_id.clone());

    let btn_label = if installed_count > 0 {
        if lang.starts_with("zh") {
            format!("已安装 ({installed_count})")
        } else {
            format!("Installed ({installed_count})")
        }
    } else {
        format!("+ {}", tr_str(lang, "添加", "Add"))
    };

    div()
        .id(SharedString::from(format!("mkt-skill-row-{}", skill.slug)))
        .cursor_pointer()
        .w_full()
        .px(px(8.0))
        .py(px(8.0))
        .rounded(px(10.0))
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .gap(px(12.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.marketplace.push_route(MarketplaceRoute::SkillDetail {
                    slug: slug_open.clone(),
                });
                cx.notify();
            }),
        )
        .child(
            div()
                .w(px(38.0))
                .h(px(38.0))
                .flex_shrink_0()
                .rounded(px(9.0))
                .bg(ob.bg_surface())
                .border_1()
                .border_color(ob.border_hairline())
                .flex()
                .items_center()
                .justify_center()
                .child(tg_icon_rgba(IconName::FileDoc, 17.0, ob.text_secondary())),
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
                                .text_size(px(14.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_primary())
                                .truncate()
                                .child(skill.name.clone()),
                        )
                        .when(skill.verified, |el| {
                            el.child(tg_icon_rgba(IconName::ShieldMod, 12.5, ob.success()))
                        }),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(ob.text_muted())
                        .truncate()
                        .child(skill.description.clone()),
                ),
        )
        .child(
            div()
                .id(SharedString::from(format!("mkt-skill-add-{}", skill.slug)))
                .flex_shrink_0()
                .cursor_pointer()
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(8.0))
                .bg(if installed_count > 0 {
                    rgba(0x31CF761F)
                } else {
                    ctrl_bg
                })
                .border_1()
                .border_color(if installed_count > 0 {
                    rgba(0x31CF7644)
                } else {
                    ob.border_hairline()
                })
                .hover(move |s| s.bg(ctrl_hover))
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if installed_count > 0 {
                    ob.success()
                } else {
                    ob.text_primary()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if this.marketplace.bots.len() == 1 {
                            if let Some(ref aid) = first_bot_id {
                                let already = installed_count > 0;
                                cx.emit(ContentHubEvent::MarketplaceToggleSkillForAgent {
                                    slug: slug_btn.clone(),
                                    agent_id: aid.clone(),
                                    install: !already,
                                });
                                return;
                            }
                        }
                        this.marketplace.push_route(MarketplaceRoute::SkillDetail {
                            slug: slug_btn.clone(),
                        });
                        cx.notify();
                    }),
                )
                .child(btn_label),
        )
}

fn render_skills_browse(
    ws: &mut ContentHubWorkspace,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let skills = ws.marketplace.filtered_skills();
    let search_focused = ws.active_input == ContentHubInputTarget::MarketplaceSearch;
    let search_field = ws.marketplace.search.clone();
    let sort_mode = ws.marketplace.skill_sort;
    let active_bg = ob.bg_control_active();
    let hover_bg = ob.bg_control_hover();
    let text_pri = ob.text_primary();
    let text_muted = ob.text_muted();

    let mine: Vec<&'static OpenBotSkillCatalogItem> = skills
        .iter()
        .copied()
        .filter(|s| !ws.marketplace.agents_with_skill(&s.slug).is_empty())
        .collect();

    let sort_pills = [
        (MarketplaceSkillSortMode::Installs, "installs", tr_str(lang, "按安装量", "Installs")),
        (MarketplaceSkillSortMode::Updated, "updated", tr_str(lang, "按更新", "Updated")),
        (MarketplaceSkillSortMode::Name, "name", tr_str(lang, "按名称", "Name")),
    ];

    let mut content = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(22.0))
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_size(px(24.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(ob.text_primary())
                                        .child(tr_str(lang, "技能", "Skills")),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.5))
                                        .text_color(ob.text_muted())
                                        .child(tr_str(
                                            lang,
                                            "教助手完成某项任务或使用某个工具的指令。",
                                            "Instructions that teach an agent how to do a job or use a tool.",
                                        )),
                                ),
                        )
                        .child(render_source_switch(
                            "mkt-skill-src",
                            ws.marketplace.skill_source,
                            false,
                            pal,
                            lang,
                            cx,
                        )),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(
                            div().flex_1().min_w(px(0.0)).child(
                                render_editable_field_box(
                                    "mkt-skills-search-input",
                                    &search_field,
                                    tr_str(lang, "搜索技能…", "Search skills…"),
                                    search_focused,
                                    false,
                                    pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.focus_input(
                                            ContentHubInputTarget::MarketplaceSearch,
                                            cx,
                                        );
                                    }),
                                ),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .children(sort_pills.into_iter().map(|(mode, slug, lbl)| {
                                    let active = sort_mode == mode;
                                    div()
                                        .id(SharedString::from(format!("mkt-skill-sort-{slug}")))
                                        .cursor_pointer()
                                        .px(px(9.0))
                                        .py(px(6.0))
                                        .rounded(px(8.0))
                                        .when(active, |el| el.bg(active_bg))
                                        .when(!active, |el| el.hover(move |s| s.bg(hover_bg)))
                                        .text_size(px(12.0))
                                        .text_color(if active { text_pri } else { text_muted })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.marketplace.skill_sort = mode;
                                                cx.notify();
                                            }),
                                        )
                                        .child(lbl)
                                })),
                        ),
                ),
        );

    if skills.is_empty() {
        let mut others = Vec::new();
        let apps_n = ws.marketplace.filtered_apps(lang).len();
        let agents_n = ws.marketplace.filtered_agents().len();
        if apps_n > 0 {
            others.push((MarketplaceNavTab::Apps, apps_n));
        }
        if agents_n > 0 {
            others.push((MarketplaceNavTab::Agents, agents_n));
        }
        content = content.child(render_no_match_plush_state(
            tr_str(lang, "没有匹配的技能。", "No matching skills."),
            others,
            pal,
            lang,
            cx,
        ));
        return content.into_any_element();
    }

    if !mine.is_empty() {
        let rows: Vec<AnyElement> = mine
            .iter()
            .take(SKILL_GROUP_LIMIT)
            .map(|s| render_skill_row(ws, s, pal, lang, cx).into_any_element())
            .collect();
        content = content.child(
            div()
                .w_full()
                .pt(px(16.0))
                .border_t_1()
                .border_color(ob.border_hairline())
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(render_group_header(
                    "mkt-skills-yours-see-all",
                    tr_str(lang, "我的技能", "Your skills"),
                    Some(mine.len()),
                    mine.len() > SKILL_GROUP_LIMIT,
                    pal,
                    lang,
                    |this, _cx| {
                        this.marketplace.push_route(MarketplaceRoute::SkillGroup {
                            group: "yours".to_string(),
                        });
                    },
                    cx,
                ))
                .child(render_two_col_grid(rows)),
        );
    }

    for &cat_slug in &SKILL_CATEGORIES {
        let bucket: Vec<&'static OpenBotSkillCatalogItem> = skills
            .iter()
            .copied()
            .filter(|s| s.category.eq_ignore_ascii_case(cat_slug))
            .collect();
        if bucket.is_empty() {
            continue;
        }
        let total = bucket.len();
        let cat_owned = cat_slug.to_string();
        let rows: Vec<AnyElement> = bucket
            .iter()
            .take(SKILL_GROUP_LIMIT)
            .map(|s| render_skill_row(ws, s, pal, lang, cx).into_any_element())
            .collect();
        content = content.child(
            div()
                .w_full()
                .pt(px(16.0))
                .border_t_1()
                .border_color(ob.border_hairline())
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(render_group_header(
                    SharedString::from(format!("mkt-skills-see-all-{cat_slug}")),
                    category_localized_label(cat_slug, lang),
                    Some(total),
                    total > SKILL_GROUP_LIMIT,
                    pal,
                    lang,
                    move |this, _cx| {
                        this.marketplace.push_route(MarketplaceRoute::SkillGroup {
                            group: cat_owned.clone(),
                        });
                    },
                    cx,
                ))
                .child(render_two_col_grid(rows)),
        );
    }

    content.into_any_element()
}

fn render_skill_group_page(
    ws: &mut ContentHubWorkspace,
    group: &str,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let title = if group == "yours" {
        tr_str(lang, "我的技能", "Your skills")
    } else {
        category_localized_label(group, lang)
    };
    let items: Vec<&'static OpenBotSkillCatalogItem> = ws
        .marketplace
        .filtered_skills()
        .into_iter()
        .filter(|s| {
            if group == "yours" {
                !ws.marketplace.agents_with_skill(&s.slug).is_empty()
            } else {
                s.category.eq_ignore_ascii_case(group)
            }
        })
        .collect();
    let rows: Vec<AnyElement> = items
        .iter()
        .map(|s| render_skill_row(ws, s, pal, lang, cx).into_any_element())
        .collect();

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .child(
            div()
                .text_size(px(22.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_primary())
                .child(title),
        )
        .child(render_two_col_grid(rows))
        .into_any_element()
}

fn render_skill_detail_page(
    ws: &mut ContentHubWorkspace,
    slug: &str,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let Some(skill) = ws
        .marketplace
        .catalog()
        .skills
        .iter()
        .find(|s| s.slug == slug)
    else {
        return div()
            .p(px(24.0))
            .text_color(ob.text_muted())
            .child(tr_str(lang, "市场中没有该技能。", "Skill not found."))
            .into_any_element();
    };

    let mut assign_card = apple_grouped_card(pal)
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .text_size(px(13.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_primary())
                .child(tr_str(lang, "安装到助手", "Install on Agents")),
        );

    if ws.marketplace.bots.is_empty() {
        assign_card = assign_card.child(
            div()
                .text_size(px(12.5))
                .text_color(ob.text_muted())
                .child(tr_str(
                    lang,
                    "还没有助手。请先在「助手」标签添加一个助手，或在「消息」中创建助手。",
                    "No agents yet. Add one from the Agents tab first.",
                )),
        );
    } else {
        for (idx, bot) in ws.marketplace.bots.iter().enumerate() {
            let installed = bot
                .installed_skill_slugs
                .iter()
                .any(|s| s.eq_ignore_ascii_case(&skill.slug));
            let sk_slug = skill.slug.clone();
            let aid = bot.openbot_agent_id.clone();
            let hover_bg = ob.bg_control_hover();
            assign_card = assign_card.child(
                div()
                    .id(SharedString::from(format!("mkt-skill-assign-bot-{idx}")))
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
                        cx.listener(move |_this, _, _, cx| {
                            cx.emit(ContentHubEvent::MarketplaceToggleSkillForAgent {
                                slug: sk_slug.clone(),
                                agent_id: aid.clone(),
                                install: !installed,
                            });
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(ob.text_primary())
                            .child(bot.name.clone()),
                    )
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(8.0))
                            .bg(if installed {
                                rgba(0x31CF7624)
                            } else {
                                ob.bg_control()
                            })
                            .text_size(px(12.0))
                            .text_color(if installed {
                                ob.success()
                            } else {
                                ob.text_primary()
                            })
                            .child(if installed {
                                tr_str(lang, "✓ 已安装（点击卸载）", "✓ Installed (remove)")
                            } else {
                                tr_str(lang, "+ 安装", "+ Install")
                            }),
                    ),
            );
        }
    }

    let skill_preview = if skill.skill_md.trim().is_empty() {
        skill.description.clone()
    } else {
        skill.skill_md.chars().take(2400).collect()
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(18.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .w(px(46.0))
                        .h(px(46.0))
                        .rounded(px(11.0))
                        .bg(ob.bg_surface())
                        .border_1()
                        .border_color(ob.border_hairline())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(tg_icon_rgba(IconName::FileDoc, 20.0, ob.text_primary())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .text_size(px(20.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(ob.text_primary())
                                        .child(skill.name.clone()),
                                )
                                .when(skill.verified, |el| {
                                    el.child(tg_icon_rgba(IconName::ShieldMod, 15.0, ob.success()))
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(ob.text_muted())
                                .child(format!(
                                    "{} · {}",
                                    skill.creator_name,
                                    category_localized_label(&skill.category, lang)
                                )),
                        ),
                ),
        )
        .child(assign_card)
        .child(
            apple_grouped_card(pal)
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child("SKILL.md"),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(18.5))
                        .text_color(ob.text_secondary())
                        .child(skill_preview),
                ),
        )
        .into_any_element()
}

fn info_fact_row(label: &str, value: String, pal: TdesktopThemePalette) -> Div {
    let ob = pal.openbot_palette();
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(ob.text_muted())
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(ob.text_primary())
                .child(value),
        )
}

fn route_crumb_label(
    ws: &ContentHubWorkspace,
    route: &MarketplaceRoute,
    lang: &str,
) -> String {
    match route {
        MarketplaceRoute::Browse => ws.marketplace.active_tab.localized_label(lang).to_string(),
        MarketplaceRoute::AppGroup { category } | MarketplaceRoute::AgentGroup { category } => {
            category_localized_label(category, lang).to_string()
        }
        MarketplaceRoute::AppDetail { app_id } => ws
            .marketplace
            .find_app(app_id)
            .map(|a| a.name().to_string())
            .unwrap_or_else(|| app_id.clone()),
        MarketplaceRoute::CustomServerForm { edit_server_id } => {
            if edit_server_id.is_some() {
                tr_str(lang, "编辑服务器", "Edit server").to_string()
            } else {
                tr_str(lang, "添加自定义项", "Add custom").to_string()
            }
        }
        MarketplaceRoute::AgentDetail { slug } => ws
            .marketplace
            .catalog()
            .agents
            .iter()
            .find(|a| a.slug == *slug)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| slug.clone()),
        MarketplaceRoute::SkillGroup { group } => {
            if group == "yours" {
                tr_str(lang, "我的技能", "Your skills").to_string()
            } else {
                category_localized_label(group, lang).to_string()
            }
        }
        MarketplaceRoute::SkillDetail { slug } => ws
            .marketplace
            .catalog()
            .skills
            .iter()
            .find(|s| s.slug == *slug)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| slug.clone()),
    }
}

/// Renders the unified OpenBot 1:1 Marketplace workspace (`市场`).
pub fn render_marketplace_hub_view(
    ws: &mut ContentHubWorkspace,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let ob = pal.openbot_palette();
    let lang = ws.language_id.clone();
    let active_tab = ws.marketplace.active_tab;
    let current_route = ws.marketplace.current_route().clone();
    let searching = !ws.marketplace.search.text.trim().is_empty();
    let your_apps = ws.marketplace.your_connected_apps();

    let count_badge = |tab: MarketplaceNavTab| -> Option<String> {
        if !searching {
            return None;
        }
        match tab {
            MarketplaceNavTab::Apps => Some(ws.marketplace.filtered_apps(&lang).len().to_string()),
            MarketplaceNavTab::Agents => Some(ws.marketplace.filtered_agents().len().to_string()),
            MarketplaceNavTab::Skills => Some(ws.marketplace.filtered_skills().len().to_string()),
            _ => None,
        }
    };

    // Left 256px Marketplace Navigation Sidebar (1:1 OpenBot + Mini Apps / Books / Games below Skills)
    let mut left_sidebar = div()
        .id("openbot-marketplace-left-nav")
        .w(px(MARKETPLACE_SIDEBAR_WIDTH))
        .flex_shrink_0()
        .h_full()
        .bg(ob.bg_sidebar())
        .border_r_1()
        .border_color(ob.border_hairline())
        .px(px(12.0))
        .py(px(16.0))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .px(px(10.0))
                .pb(px(10.0))
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ob.text_primary())
                .child(tr_str(&lang, "市场", "Marketplace")),
        );

    // 1. Primary OpenBot 3 entries: 应用 (Connectors/Apps), 助手 (Agents), 技能 (Skills)
    for tab in MarketplaceNavTab::PRIMARY_TABS {
        let icon = match tab {
            MarketplaceNavTab::Apps => IconName::MarketplaceBlocks,
            MarketplaceNavTab::Agents => IconName::BotRobot,
            MarketplaceNavTab::Skills => IconName::FileDoc,
            _ => IconName::MarketplaceBlocks,
        };
        let is_viewing_owned_app = matches!(
            current_route,
            MarketplaceRoute::AppDetail { ref app_id } if your_apps.iter().any(|a| a.id() == *app_id)
        );
        let active = active_tab == tab && !(tab == MarketplaceNavTab::Apps && is_viewing_owned_app);
        left_sidebar = left_sidebar.child(
            render_nav_row(
                SharedString::from(format!("mkt-nav-{}", tab.slug())),
                icon,
                tab.localized_label(&lang),
                active,
                count_badge(tab),
                pal,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.marketplace.select_tab(tab);
                    this.selected_creation_id = None;
                    this.selected_catalog_item = None;
                    this.selected_detail_creation = None;
                    cx.notify();
                }),
            ),
        );
    }

    // 2. Separated section below `技能`: 小程序应用 (Mini Apps), 图书 (Books), 游戏 (Games)
    left_sidebar = left_sidebar
        .child(
            div()
                .my(px(10.0))
                .mx(px(6.0))
                .h(px(1.0))
                .bg(ob.border_hairline()),
        )
        .child(
            div()
                .px(px(10.0))
                .pb(px(4.0))
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ob.text_muted())
                .child(tr_str(&lang, "小程序与内容", "Mini Apps & Media")),
        );

    for tab in MarketplaceNavTab::MEDIA_STORE_TABS {
        let icon = match tab {
            MarketplaceNavTab::MiniApps => IconName::HubAppStore,
            MarketplaceNavTab::Books => IconName::HubBooks,
            MarketplaceNavTab::Games => IconName::HubGames,
            _ => IconName::HubAppStore,
        };
        let active = active_tab == tab;
        left_sidebar = left_sidebar.child(
            render_nav_row(
                SharedString::from(format!("mkt-nav-{}", tab.slug())),
                icon,
                tab.localized_label(&lang),
                active,
                None,
                pal,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.select_marketplace_tab_with_refresh(tab, cx);
                }),
            ),
        );
    }

    // 3. Dynamic `你的应用` (`Your apps`) connected apps section (1:1 OpenBot `MarketplaceNav.tsx:L62-93`)
    if !your_apps.is_empty() {
        left_sidebar = left_sidebar
            .child(
                div()
                    .my(px(10.0))
                    .mx(px(6.0))
                    .h(px(1.0))
                    .bg(ob.border_hairline()),
            )
            .child(
                div()
                    .px(px(10.0))
                    .pb(px(4.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ob.text_muted())
                            .child(tr_str(&lang, "你的应用", "Your apps")),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(ob.text_dim())
                            .child(format!("{}", your_apps.len())),
                    ),
            );
        for app in &your_apps {
            let aid = app.id();
            let active = matches!(
                current_route,
                MarketplaceRoute::AppDetail { ref app_id } if *app_id == aid
            );
            let hover_bg = ob.bg_control_hover();
            let active_bg = ob.bg_control_active();
            let aid_click = aid.clone();
            left_sidebar = left_sidebar.child(
                div()
                    .id(SharedString::from(format!("mkt-your-app-{aid}")))
                    .cursor_pointer()
                    .w_full()
                    .h(px(32.0))
                    .px(px(10.0))
                    .rounded(px(8.0))
                    .when(active, |el| el.bg(active_bg))
                    .when(!active, |el| el.hover(move |s| s.bg(hover_bg)))
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.marketplace.active_tab = MarketplaceNavTab::Apps;
                            this.marketplace.open_setup_for_app(&aid_click);
                            this.marketplace.stack = vec![
                                MarketplaceRoute::Browse,
                                MarketplaceRoute::AppDetail {
                                    app_id: aid_click.clone(),
                                },
                            ];
                            cx.notify();
                        }),
                    )
                    .child(render_app_logo_tile(app, 20.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(13.0))
                            .text_color(if active {
                                ob.text_primary()
                            } else {
                                ob.text_secondary()
                            })
                            .child(app.name().to_string()),
                    ),
            );
        }
    }

    // Optional Breadcrumb Topbar (shown when stack depth > 1 on primary OpenBot tabs)
    let show_crumbs = matches!(
        active_tab,
        MarketplaceNavTab::Apps | MarketplaceNavTab::Agents | MarketplaceNavTab::Skills
    ) && ws.marketplace.stack.len() > 1;

    let crumb_bar: Option<Div> = if show_crumbs {
        let stack_snap = ws.marketplace.stack.clone();
        let last_idx = stack_snap.len().saturating_sub(1);
        let hover_bg = ob.bg_control_hover();
        let text_pri = ob.text_primary();
        let mut bar = div()
            .h(px(44.0))
            .flex_shrink_0()
            .w_full()
            .px(px(24.0))
            .border_b_1()
            .border_color(ob.border_hairline())
            .bg(ob.bg_canvas())
            .flex()
            .items_center()
            .gap(px(6.0));
        for (idx, route) in stack_snap.iter().enumerate() {
            let lbl = route_crumb_label(ws, route, &lang);
            if idx > 0 {
                bar = bar.child(
                    div()
                        .text_size(px(13.0))
                        .text_color(ob.text_muted())
                        .child("›"),
                );
            }
            if idx < last_idx {
                bar = bar.child(
                    div()
                        .id(SharedString::from(format!("mkt-crumb-{idx}")))
                        .cursor_pointer()
                        .px(px(6.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .hover(move |s| s.bg(hover_bg).text_color(text_pri))
                        .text_size(px(13.5))
                        .text_color(ob.text_muted())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.marketplace.back_to(idx);
                                cx.notify();
                            }),
                        )
                        .child(lbl),
                );
            } else {
                bar = bar.child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(lbl),
                );
            }
        }
        Some(bar)
    } else {
        None
    };

    // Right Scrollable Body
    let notice = ws.marketplace.notice_banner.clone();
    let error = ws.marketplace.error_banner.clone();

    let inner_body: AnyElement = match active_tab {
        MarketplaceNavTab::Apps => match current_route {
            MarketplaceRoute::Browse => render_apps_browse(ws, pal, &lang, cx),
            MarketplaceRoute::AppGroup { ref category } => {
                render_app_group_page(ws, category, pal, &lang, cx)
            }
            MarketplaceRoute::AppDetail { ref app_id } => {
                render_app_detail_page(ws, app_id, pal, &lang, cx)
            }
            MarketplaceRoute::CustomServerForm { .. } => {
                render_custom_server_form_page(ws, pal, &lang, cx)
            }
            _ => render_apps_browse(ws, pal, &lang, cx),
        },
        MarketplaceNavTab::Agents => match current_route {
            MarketplaceRoute::Browse => render_agents_browse(ws, pal, &lang, cx),
            MarketplaceRoute::AgentGroup { ref category } => {
                render_agent_group_page(ws, category, pal, &lang, cx)
            }
            MarketplaceRoute::AgentDetail { ref slug } => {
                render_agent_detail_page(ws, slug, pal, &lang, cx)
            }
            _ => render_agents_browse(ws, pal, &lang, cx),
        },
        MarketplaceNavTab::Skills => match current_route {
            MarketplaceRoute::Browse => render_skills_browse(ws, pal, &lang, cx),
            MarketplaceRoute::SkillGroup { ref group } => {
                render_skill_group_page(ws, group, pal, &lang, cx)
            }
            MarketplaceRoute::SkillDetail { ref slug } => {
                render_skill_detail_page(ws, slug, pal, &lang, cx)
            }
            _ => render_skills_browse(ws, pal, &lang, cx),
        },
        MarketplaceNavTab::MiniApps => app_store_view::render_app_store_embedded_body(ws, cx),
        MarketplaceNavTab::Books => books_view::render_books_embedded_body(ws, cx),
        MarketplaceNavTab::Games => games_view::render_games_embedded_body(ws, cx),
    };

    let right_pane = div()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .bg(ob.bg_canvas())
        .flex()
        .flex_col()
        .overflow_hidden()
        .when_some(crumb_bar, |el, bar| el.child(bar))
        .child(
            div()
                .id("openbot-marketplace-body-scroll")
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_y_scroll()
                .child(
                    div()
                        .w_full()
                        .max_w(px(OPENBOT_MARKETPLACE_CONTENT_MAX_WIDTH))
                        .mx_auto()
                        .px(px(28.0))
                        .py(px(28.0))
                        .flex()
                        .flex_col()
                        .gap(px(16.0))
                        .when_some(error, |el, err| {
                            el.child(
                                div()
                                    .w_full()
                                    .px(px(14.0))
                                    .py(px(10.0))
                                    .rounded(px(10.0))
                                    .bg(rgba(0xE539351E))
                                    .border_1()
                                    .border_color(rgba(0xE5393555))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(ob.danger())
                                            .child(err),
                                    )
                                    .child(
                                        div()
                                            .id("mkt-dismiss-err")
                                            .cursor_pointer()
                                            .text_size(px(12.0))
                                            .text_color(ob.text_muted())
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.marketplace.error_banner = None;
                                                    cx.notify();
                                                }),
                                            )
                                            .child("✕"),
                                    ),
                            )
                        })
                        .when_some(notice, |el, msg| {
                            el.child(
                                div()
                                    .w_full()
                                    .px(px(14.0))
                                    .py(px(10.0))
                                    .rounded(px(10.0))
                                    .bg(rgba(0x31CF761E))
                                    .border_1()
                                    .border_color(rgba(0x31CF7644))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(ob.success())
                                            .child(msg),
                                    )
                                    .child(
                                        div()
                                            .id("mkt-dismiss-notice")
                                            .cursor_pointer()
                                            .text_size(px(12.0))
                                            .text_color(ob.text_muted())
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.marketplace.notice_banner = None;
                                                    cx.notify();
                                                }),
                                            )
                                            .child("✕"),
                                    ),
                            )
                        })
                        .child(inner_body),
                ),
        );

    div()
        .id("openbot-marketplace-root")
        .size_full()
        .bg(ob.bg_canvas())
        .flex()
        .flex_row()
        .overflow_hidden()
        .child(left_sidebar)
        .child(right_pane)
        .into_any_element()
}
