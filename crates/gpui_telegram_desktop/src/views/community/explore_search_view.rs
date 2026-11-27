//! Feature F21-Explore — Global & Scoped Search (All 7 Tabs + Operator Normalization
//! for Online Tantivy & Local SQLite Parity) + Live Discovery (Trending `#hashtags`,
//! Rising Communities, Featured Humans & Bots).

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_community::parse_search_operators;
use gpui_telegram_proto::community_contracts::{
    ActorKind, CommunityActorDto, CommunitySearchRequestDto, CommunitySearchResultDto,
    CommunitySearchSort, CommunitySearchTab, FeedItemDto, SubCommunityDto, TrendingItemKind,
    TrendingTopicDto,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};

use super::post_card::render_post_card;
use super::shared_widgets::{
    format_compact_count, render_actor_avatar, render_actor_badge_row, render_empty_state,
    render_input_display, render_loading_skeleton,
};
use super::state::{ActiveCommunityInput, CommunityRoute, CommunityWorkspaceAction};
use super::workspace::{run_on_community_tokio, CommunityWorkspace};

const EXPLORE_CATEGORIES: [(Option<&str>, &str, &str); 9] = [
    (None, "全部", "All"),
    (Some("tech"), "技术", "Tech"),
    (Some("ai"), "AI 与 Bots", "AI & Bots"),
    (Some("rust"), "Rust", "Rust"),
    (Some("art"), "设计", "Design"),
    (Some("news"), "资讯", "News"),
    (Some("science"), "科学", "Science"),
    (Some("gaming"), "游戏", "Gaming"),
    (Some("general"), "综合", "General"),
];

const OPERATOR_PILLS: [(&str, &str, &str, bool); 7] = [
    ("c:", "c:社区", "c:", false),
    ("author:", "author:作者", "author:", false),
    ("flair:", "flair:标签", "flair:", false),
    ("kind:bot", "kind:bot", "kind:bot", true),
    ("kind:human", "kind:human", "kind:human", true),
    ("has:media", "has:media", "has:media", true),
    ("has:poll", "has:poll", "has:poll", true),
];

/// Splits a raw search query into whitespace-separated tokens while keeping double-quoted
/// substrings (e.g. `flair:"deep dive"`) intact.
fn tokenize_preserving_quotes(raw: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in raw.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Normalizes user search operator aliases into the canonical syntax understood identically
/// by BOTH server Tantivy (`search.rs`) and local SQLite (`local_ops.rs`):
/// - `is:bot` / `is:bots` -> `kind:bot`
/// - `is:human` / `is:humans` -> `kind:human`
/// - `is:hybrid` / `is:hybrid_agent` -> `kind:hybrid_agent`
/// - `from:@handle` / `from:handle` -> `author:handle`
/// - `author:@handle` -> `author:handle`
/// - `c/<slug>` -> `c:<slug>`
pub fn normalize_search_query(raw: &str) -> String {
    tokenize_preserving_quotes(raw)
        .into_iter()
        .map(|tok| {
            let lower = tok.to_ascii_lowercase();
            if matches!(lower.as_str(), "is:bot" | "is:bots") {
                return "kind:bot".to_string();
            }
            if matches!(lower.as_str(), "is:human" | "is:humans") {
                return "kind:human".to_string();
            }
            if matches!(lower.as_str(), "is:hybrid" | "is:hybrid_agent") {
                return "kind:hybrid_agent".to_string();
            }
            if lower.starts_with("from:") {
                let handle = tok[5..].trim_start_matches('@');
                if !handle.is_empty() {
                    return format!("author:{handle}");
                }
                return "author:".to_string();
            }
            if lower.starts_with("author:@") {
                let handle = tok[8..].trim_start_matches('@');
                return format!("author:{handle}");
            }
            if lower.starts_with("c/") && tok.len() > 2 && !lower.contains("://") {
                let slug = tok[2..].trim();
                if !slug.is_empty()
                    && slug
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    return format!("c:{}", slug.to_ascii_lowercase());
                }
            }
            tok
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Builds a canonical [`CommunitySearchRequestDto`] with normalized operators and
/// People/Bots/Media/Polls tab constraints aligned across Online Tantivy and Local SQLite.
pub fn build_search_request(
    raw_query: &str,
    tab: CommunitySearchTab,
    sort: CommunitySearchSort,
) -> CommunitySearchRequestDto {
    let normalized = normalize_search_query(raw_query);
    let parsed = parse_search_operators(&normalized);
    let actor_kind = match tab {
        CommunitySearchTab::People => Some(ActorKind::Human),
        CommunitySearchTab::Bots => Some(ActorKind::Bot),
        _ => parsed.actor_kind,
    };
    let has_media = if tab == CommunitySearchTab::Media {
        Some(true)
    } else {
        parsed.has_media
    };
    let has_poll = if tab == CommunitySearchTab::Polls {
        Some(true)
    } else {
        parsed.has_poll
    };
    CommunitySearchRequestDto {
        query: normalized,
        tab,
        sort,
        community_slug: parsed.community,
        author_handle: parsed.author,
        flair: parsed.flair,
        actor_kind,
        has_media,
        has_poll,
        limit: 25,
        offset: 0,
    }
}

/// Applies structured operator filters (`c:<slug>`, `author:<handle>`, `kind:...`) to a
/// [`CommunitySearchResultDto`] so `Communities`, `People`, and `Bots` tabs match strictly
/// across both Online Tantivy and Local SQLite fallback.
pub fn filter_search_result_by_operators(
    req: &CommunitySearchRequestDto,
    mut res: CommunitySearchResultDto,
) -> CommunitySearchResultDto {
    if let Some(ref slug) = req.community_slug {
        let target = slug.trim().trim_start_matches("c/").trim_start_matches("C/");
        if !target.is_empty() {
            res.communities
                .retain(|c| c.slug.eq_ignore_ascii_case(target));
        }
    }
    if let Some(ref author) = req.author_handle {
        let target = author.trim().trim_start_matches('@');
        if !target.is_empty() {
            res.actors
                .retain(|a| a.handle.eq_ignore_ascii_case(target));
        }
    }
    if let Some(kind) = req.actor_kind {
        res.actors.retain(|a| match kind {
            ActorKind::Human => a.actor_kind == ActorKind::Human,
            ActorKind::Bot => a.actor_kind.is_bot_like(),
            ActorKind::HybridAgent => a.actor_kind == ActorKind::HybridAgent,
        });
    }
    res.total_hits = res.posts.len() + res.actors.len() + res.communities.len();
    res
}

impl CommunityWorkspace {
    pub fn set_explore_sort(&mut self, sort: CommunitySearchSort, cx: &mut Context<Self>) {
        if self.state.search_sort != sort {
            self.state.search_sort = sort;
            let q = self.state.explore_search_draft.trim().to_string();
            self.execute_search(q, self.state.search_tab, cx);
        }
    }

    pub fn set_explore_category(&mut self, category: Option<String>, cx: &mut Context<Self>) {
        self.state.search_category_filter = category;
        cx.notify();
    }

    pub fn toggle_explore_discovery(&mut self, cx: &mut Context<Self>) {
        self.state.explore_show_discovery = !self.state.explore_show_discovery;
        cx.notify();
    }

    pub fn clear_explore_search(&mut self, cx: &mut Context<Self>) {
        self.state.explore_search_draft.clear();
        self.state.explore_show_discovery = true;
        self.state.search_error = None;
        self.execute_search(String::new(), self.state.search_tab, cx);
    }

    pub fn insert_explore_operator(
        &mut self,
        operator: &str,
        execute_immediately: bool,
        cx: &mut Context<Self>,
    ) {
        let trimmed = self.state.explore_search_draft.trim_end();
        if !trimmed.is_empty() && !trimmed.contains(operator) {
            self.state.explore_search_draft = format!("{trimmed} {operator}");
        } else if trimmed.is_empty() {
            self.state.explore_search_draft = operator.to_string();
        }
        self.state.focus_input(ActiveCommunityInput::ExploreSearch);
        if execute_immediately {
            let q = self.state.explore_search_draft.trim().to_string();
            self.execute_search(q, self.state.search_tab, cx);
        } else {
            cx.notify();
        }
    }

    pub fn refresh_trending_discovery(&mut self, cx: &mut Context<Self>) {
        self.state.communities_gen = self.state.communities_gen.wrapping_add(1);
        self.state.communities_loading = true;
        let my_comm_gen = self.state.communities_gen;
        let my_actor_gen = self.state.actor_switch_gen;
        let facade = self.facade.clone();
        let handle = facade.runtime_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_community_tokio(&handle, async move {
                let trending = facade.get_trending(Some(12)).await.ok();
                let comms = facade.list_communities(None, Some(40)).await.ok();
                (trending, comms)
            })
            .await;
            match res {
                Ok((t, c)) => {
                    let _ = weak.update(app, |this, cx| {
                        if let Some(tr) = t {
                            this.state.trending = tr.topics;
                        }
                        if this.state.communities_gen == my_comm_gen
                            && this.state.actor_switch_gen == my_actor_gen
                        {
                            this.state.communities_loading = false;
                            if let Some(mut cm) = c {
                                for comm in &mut cm.communities {
                                    this.state.overlay_in_flight_community_mutations(comm);
                                }
                                this.state.communities = cm.communities;
                            }
                        }
                        cx.notify();
                    });
                }
                Err(_) => {
                    let _ = weak.update(app, |this, cx| {
                        if this.state.communities_gen == my_comm_gen
                            && this.state.actor_switch_gen == my_actor_gen
                        {
                            this.state.communities_loading = false;
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}

/// Renders the full F21 Explore & Global/Scoped Search view.
pub fn render_explore_search_view(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let focused = ws.state.active_input == ActiveCommunityInput::ExploreSearch;
    let has_query = !ws.state.explore_search_draft.trim().is_empty();
    let show_discovery = !has_query || ws.state.explore_show_discovery;
    let active_tab = ws.state.search_tab;
    let active_sort = ws.state.search_sort;
    let active_cat = ws.state.search_category_filter.clone();

    let tabs: [(CommunitySearchTab, &'static str, &'static str); 7] = [
        (CommunitySearchTab::Top, "综合", "Top"),
        (CommunitySearchTab::Latest, "最新", "Latest"),
        (CommunitySearchTab::People, "人类", "People"),
        (CommunitySearchTab::Bots, "Bots", "Bots"),
        (CommunitySearchTab::Communities, "社区", "Communities"),
        (CommunitySearchTab::Media, "媒体", "Media"),
        (CommunitySearchTab::Polls, "投票", "Polls"),
    ];
    let sorts: [(CommunitySearchSort, &'static str, &'static str); 3] = [
        (CommunitySearchSort::Relevance, "相关度", "Relevance"),
        (CommunitySearchSort::Top, "最热", "Top"),
        (CommunitySearchSort::Latest, "最新", "Latest"),
    ];

    div()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .flex_col()
        .bg(rgb(pal.stage_bg))
        .child(ws.render_secondary_header(
            tr_str(lang, "探索、热榜与全站搜索", "Explore, Trending & Search").to_string(),
            Some(
                tr_str(
                    lang,
                    "支持高级语法 c:、author:、flair:、kind:bot/human、has:media/poll",
                    "Supports c:, author:, flair:, kind:bot/human, has:media, has:poll",
                )
                .to_string(),
            ),
            cx,
        ))
        .child(
            div()
                .id("community-explore-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p(px(14.0))
                .child(
                    div()
                        .w_full()
                        .max_w(px(760.0))
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        // 1. IME-wired Search Bar + Clear + Search button
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(7.0))
                                .child(
                                    div()
                                        .id("explore-search-box")
                                        .cursor_text()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .px(px(12.0))
                                        .py(px(8.0))
                                        .rounded_full()
                                        .bg(rgb(pal.input_bg))
                                        .border_1()
                                        .border_color(if focused {
                                            rgb(pal.accent).into()
                                        } else {
                                            pal.hairline_border_rgba()
                                        })
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.state
                                                    .focus_input(ActiveCommunityInput::ExploreSearch);
                                                cx.notify();
                                            }),
                                        )
                                        .child(tg_icon(IconName::Search, 15.0, pal.text_secondary))
                                        .child(div().flex_1().min_w(px(0.0)).child(
                                            render_input_display(
                                                &ws.state.explore_search_draft,
                                                if focused {
                                                    ws.state.ime_preedit.as_deref()
                                                } else {
                                                    None
                                                },
                                                tr_str(
                                                    lang,
                                                    "搜索帖子、#话题、from:@bot、c:rust、has:poll…",
                                                    "Search posts, #tag, from:@bot, c:rust, has:poll…",
                                                ),
                                                focused,
                                                &pal,
                                            ),
                                        ))
                                        .when(has_query, |row| {
                                            row.child(
                                                div()
                                                    .id("explore-clear-btn")
                                                    .cursor_pointer()
                                                    .p(px(3.0))
                                                    .rounded_full()
                                                    .bg(pal.service_pill_rgba())
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.clear_explore_search(cx);
                                                        }),
                                                    )
                                                    .child(tg_icon(
                                                        IconName::Close,
                                                        11.0,
                                                        pal.text_secondary,
                                                    )),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .id("explore-submit-btn")
                                        .cursor_pointer()
                                        .px(px(14.0))
                                        .py(px(8.0))
                                        .rounded_full()
                                        .bg(rgb(pal.accent))
                                        .hover(move |s| s.bg(rgb(pal.accent_hover)))
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.active_button_fg))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                cx.stop_propagation();
                                                let q = this
                                                    .state
                                                    .explore_search_draft
                                                    .trim()
                                                    .to_string();
                                                this.state.explore_show_discovery = q.is_empty();
                                                this.execute_search(q, this.state.search_tab, cx);
                                            }),
                                        )
                                        .child(tr_str(lang, "搜索", "Search")),
                                ),
                        )
                        // 2. Quick-insert structured operator pills
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(tr_str(lang, "快捷筛选:", "Operators:")),
                                )
                                .children(OPERATOR_PILLS.into_iter().map(
                                    |(op, zh, en, exec_now)| {
                                        div()
                                            .id(SharedString::from(format!("explore-op-{op}")))
                                            .cursor_pointer()
                                            .px(px(8.0))
                                            .py(px(3.0))
                                            .rounded(px(6.0))
                                            .bg(pal.service_pill_rgba())
                                            .border_1()
                                            .border_color(pal.hairline_border_rgba())
                                            .hover(move |s| s.bg(pal.row_hover_rgba()))
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(pal.text_primary))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.insert_explore_operator(op, exec_now, cx);
                                                }),
                                            )
                                            .child(tr_str(lang, zh, en))
                                    },
                                )),
                        )
                        // 3. All 7 Scoped Search Tabs + Sort Selector + Discovery Toggle
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap(px(5.0))
                                        .children(tabs.into_iter().map(|(tab, zh, en)| {
                                            let active = active_tab == tab;
                                            div()
                                                .id(SharedString::from(format!("explore-tab-{en}")))
                                                .cursor_pointer()
                                                .px(px(10.0))
                                                .py(px(4.5))
                                                .rounded_full()
                                                .bg(if active {
                                                    rgb(pal.accent).into()
                                                } else {
                                                    pal.service_pill_rgba()
                                                })
                                                .text_size(px(11.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(if active {
                                                    pal.active_button_fg
                                                } else {
                                                    pal.text_secondary
                                                }))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        let q = this
                                                            .state
                                                            .explore_search_draft
                                                            .trim()
                                                            .to_string();
                                                        this.execute_search(q, tab, cx);
                                                    }),
                                                )
                                                .child(tr_str(lang, zh, en))
                                        })),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .children(sorts.into_iter().map(|(s_val, zh, en)| {
                                            let active = active_sort == s_val;
                                            div()
                                                .id(SharedString::from(format!("explore-sort-{en}")))
                                                .cursor_pointer()
                                                .px(px(8.0))
                                                .py(px(3.5))
                                                .rounded(px(7.0))
                                                .bg(if active {
                                                    TdesktopThemePalette::rgba_with_alpha(
                                                        pal.accent, 0.16,
                                                    )
                                                } else {
                                                    pal.service_pill_rgba()
                                                })
                                                .text_size(px(10.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(if active {
                                                    pal.accent
                                                } else {
                                                    pal.text_secondary
                                                }))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.set_explore_sort(s_val, cx);
                                                    }),
                                                )
                                                .child(tr_str(lang, zh, en))
                                        }))
                                        .when(has_query, |row| {
                                            row.child(
                                                div()
                                                    .id("explore-toggle-discovery-btn")
                                                    .cursor_pointer()
                                                    .px(px(8.0))
                                                    .py(px(3.5))
                                                    .rounded(px(7.0))
                                                    .bg(if show_discovery {
                                                        TdesktopThemePalette::rgba_with_alpha(
                                                            0xF97316, 0.16,
                                                        )
                                                    } else {
                                                        pal.service_pill_rgba()
                                                    })
                                                    .text_size(px(10.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(if show_discovery {
                                                        0xF97316
                                                    } else {
                                                        pal.text_secondary
                                                    }))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _, cx| {
                                                            cx.stop_propagation();
                                                            this.toggle_explore_discovery(cx);
                                                        }),
                                                    )
                                                    .child(tr_str(lang, "发现热榜", "Discovery")),
                                            )
                                        }),
                                ),
                        )
                        // 4. Category filter chips
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(5.0))
                                .children(EXPLORE_CATEGORIES.into_iter().map(|(cat, zh, en)| {
                                    let active = active_cat.as_deref() == cat;
                                    let cat_owned = cat.map(ToString::to_string);
                                    div()
                                        .id(SharedString::from(format!(
                                            "explore-cat-{}",
                                            cat.unwrap_or("all")
                                        )))
                                        .cursor_pointer()
                                        .px(px(9.0))
                                        .py(px(3.0))
                                        .rounded_full()
                                        .bg(if active {
                                            TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
                                        } else {
                                            pal.service_pill_rgba()
                                        })
                                        .text_size(px(11.0))
                                        .font_weight(if active {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .text_color(rgb(if active {
                                            pal.accent
                                        } else {
                                            pal.text_secondary
                                        }))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.set_explore_category(cat_owned.clone(), cx);
                                            }),
                                        )
                                        .child(tr_str(lang, zh, en))
                                })),
                        )
                        // 5. Search Results (when query present or actor/community/media/poll tab)
                        .child(render_search_results_section(ws, cx))
                        // 6. Live Discovery Overview (Trending + Rising Communities + Featured Citizens)
                        .when(show_discovery, |col| {
                            col.child(render_discovery_overview(ws, cx))
                        }),
                ),
        )
        .into_any_element()
}

fn render_search_results_section(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let has_query = !ws.state.explore_search_draft.trim().is_empty();
    let tab = ws.state.search_tab;
    let list_when_empty = matches!(
        tab,
        CommunitySearchTab::People
            | CommunitySearchTab::Bots
            | CommunitySearchTab::Communities
            | CommunitySearchTab::Media
            | CommunitySearchTab::Polls
    );
    if !has_query && !list_when_empty {
        return div();
    }
    if ws.state.search_loading {
        return render_loading_skeleton(3, &pal);
    }
    if let Some(err) = ws.state.search_error.clone() {
        return div()
            .p(px(12.0))
            .rounded(px(10.0))
            .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.12))
            .text_size(px(12.0))
            .text_color(rgb(pal.danger))
            .child(err);
    }
    let Some(res) = ws.state.search_result.clone() else {
        return div();
    };
    let parsed_query = parse_search_operators(&normalize_search_query(&res.query));
    let cat_filter = ws.state.search_category_filter.as_deref();
    let comm_op_filter = parsed_query.community.as_deref();
    let author_op_filter = parsed_query.author.as_deref();
    let filtered_actors: Vec<CommunityActorDto> = res
        .actors
        .into_iter()
        .filter(|a| {
            author_op_filter
                .map(|target| a.handle.eq_ignore_ascii_case(target.trim_start_matches('@')))
                .unwrap_or(true)
        })
        .collect();
    let filtered_comms: Vec<SubCommunityDto> = res
        .communities
        .into_iter()
        .filter(|c| {
            let cat_ok = cat_filter
                .map(|cat| c.category.eq_ignore_ascii_case(cat))
                .unwrap_or(true);
            let comm_ok = comm_op_filter
                .map(|target| {
                    let clean = target.trim().trim_start_matches("c/").trim_start_matches("C/");
                    clean.is_empty() || c.slug.eq_ignore_ascii_case(clean)
                })
                .unwrap_or(true);
            cat_ok && comm_ok
        })
        .collect();

    let total_visible = res.posts.len() + filtered_actors.len() + filtered_comms.len();
    if total_visible == 0 {
        return div().child(render_empty_state(
            IconName::Search,
            tr_str(lang, "未找到匹配结果", "No matching results"),
            tr_str(
                lang,
                "尝试更换关键词、切换搜索标签页，或点击上方快捷筛选语法。",
                "Try adjusting keywords, switching tabs, or using structured operators.",
            ),
            &pal,
        ));
    }

    let active_actor_id = ws.state.active_actor_id.clone();
    let base_url = ws.media_base_url.clone();

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(11.5))
                .text_color(rgb(pal.text_secondary))
                .child(format!(
                    "{} ({total_visible})",
                    tr_str(lang, "搜索结果", "Search Results")
                ))
                .when(!res.query.is_empty(), |row| {
                    row.child(format!("query: {}", res.query))
                }),
        )
        .children(res.posts.into_iter().enumerate().map(|(i, p)| {
            let it = FeedItemDto {
                post: p,
                explanation: None,
                rank_score: 0.0,
                is_exploration_slot: false,
            };
            render_post_card(ws, &it, i, cx)
        }))
        .children(filtered_actors.into_iter().map(|a| {
            render_explore_actor_card(a, "search", &active_actor_id, &base_url, &pal, lang, cx)
        }))
        .children(
            filtered_comms
                .into_iter()
                .map(|c| render_explore_community_card(c, "search", &pal, lang, cx)),
        )
}

fn render_discovery_overview(
    ws: &CommunityWorkspace,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let cat_filter = ws.state.search_category_filter.as_deref();
    let trending: Vec<TrendingTopicDto> = ws
        .state
        .trending
        .iter()
        .filter(|t| {
            cat_filter
                .map(|cat| t.category.eq_ignore_ascii_case(cat))
                .unwrap_or(true)
        })
        .take(8)
        .cloned()
        .collect();
    let rising_comms: Vec<SubCommunityDto> = ws
        .state
        .communities
        .iter()
        .filter(|c| {
            cat_filter
                .map(|cat| c.category.eq_ignore_ascii_case(cat))
                .unwrap_or(true)
        })
        .take(6)
        .cloned()
        .collect();
    let featured_actors: Vec<CommunityActorDto> = ws
        .state
        .recommended_actors
        .iter()
        .take(6)
        .cloned()
        .collect();
    let active_actor_id = ws.state.active_actor_id.clone();
    let base_url = ws.media_base_url.clone();

    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        // Trending Topics & Hashtags
        .child(
            div()
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(pal.hairline_border_rgba())
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(tg_icon(IconName::FlameTrending, 15.0, 0xF97316))
                                .child(
                                    div()
                                        .text_size(px(13.5))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(
                                            lang,
                                            "实时热议话题与 #标签",
                                            "Trending Topics & #Hashtags",
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .id("explore-refresh-trending-btn")
                                .cursor_pointer()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded_full()
                                .bg(pal.service_pill_rgba())
                                .text_size(px(10.5))
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.refresh_trending_discovery(cx);
                                    }),
                                )
                                .child(tr_str(lang, "刷新", "Refresh")),
                        ),
                )
                .when(trending.is_empty(), |col| {
                    col.child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(pal.text_secondary))
                            .child(tr_str(
                                lang,
                                "当前分类暂无热议话题",
                                "No trending topics in this category yet",
                            )),
                    )
                })
                .children(trending.into_iter().enumerate().map(|(i, t)| {
                    render_trending_row(i, t, &pal, lang, cx)
                })),
        )
        // Rising Communities
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(tg_icon(IconName::CommunityGlobe, 14.5, pal.accent))
                                .child(
                                    div()
                                        .text_size(px(13.5))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(lang, "活跃社区 (c/)", "Rising Communities")),
                                ),
                        )
                        .child(
                            div()
                                .id("explore-create-comm-btn")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(3.5))
                                .rounded_full()
                                .bg(pal.service_pill_rgba())
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.open_create_community_modal(cx);
                                    }),
                                )
                                .child(tr_str(lang, "+ 创建社区", "+ Create c/")),
                        ),
                )
                .children(
                    rising_comms
                        .into_iter()
                        .map(|c| render_explore_community_card(c, "disc", &pal, lang, cx)),
                ),
        )
        // Featured Humans & Bots
        .when(!featured_actors.is_empty(), |col| {
            col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(tg_icon(IconName::BotRobot, 15.0, 0x8B5CF6))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(tr_str(
                                        lang,
                                        "精选人类与 Bot 公民",
                                        "Featured Humans & Bots",
                                    )),
                            ),
                    )
                    .children(featured_actors.into_iter().map(|a| {
                        render_explore_actor_card(
                            a,
                            "disc",
                            &active_actor_id,
                            &base_url,
                            &pal,
                            lang,
                            cx,
                        )
                    })),
            )
        })
}

fn render_trending_row(
    idx: usize,
    topic: TrendingTopicDto,
    pal: &TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> Stateful<Div> {
    let kind = topic.kind;
    let slug_or_tag = topic.slug_or_tag.clone();
    let max_bin = topic.sparkline.iter().copied().max().unwrap_or(1).max(1);
    let hover_bg = pal.row_hover_rgba();

    div()
        .id(SharedString::from(format!("explore-trend-{idx}-{}", topic.id)))
        .cursor_pointer()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(9.0))
        .bg(pal.service_pill_rgba())
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if kind == TrendingItemKind::Community {
                    let clean = slug_or_tag.trim_start_matches("c/").to_string();
                    this.navigate_to(CommunityRoute::CommunityPage(clean), cx);
                } else {
                    this.state.explore_search_draft = slug_or_tag.clone();
                    this.state.explore_show_discovery = false;
                    this.execute_search(slug_or_tag.clone(), CommunitySearchTab::Top, cx);
                }
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
                        .gap(px(6.0))
                        .text_size(px(10.5))
                        .text_color(rgb(pal.text_secondary))
                        .child(format!("#{} · {}", idx + 1, topic.category))
                        .child(format!(
                            "· {}H / {}B",
                            topic.human_posts_count, topic.bot_posts_count
                        )),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(topic.label),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(rgb(pal.text_secondary))
                        .child(format!(
                            "{} {} · v={:.1}",
                            format_compact_count(topic.posts_count_24h as i64),
                            tr_str(lang, "条帖子 (24h)", "posts (24h)"),
                            topic.velocity_score
                        )),
                ),
        )
        .when(!topic.sparkline.is_empty(), |row| {
            row.child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(2.0))
                    .h(px(22.0))
                    .children(topic.sparkline.into_iter().take(6).map(|v| {
                        let h_px = ((v as f32 / max_bin as f32) * 18.0).clamp(3.0, 20.0);
                        div()
                            .w(px(4.0))
                            .h(px(h_px))
                            .rounded(px(2.0))
                            .bg(rgb(0xF97316))
                    })),
            )
        })
}

fn render_explore_actor_card(
    actor: CommunityActorDto,
    prefix: &str,
    active_actor_id: &str,
    base_url: &str,
    pal: &TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> Stateful<Div> {
    let handle = actor.handle.clone();
    let aid = actor.actor_id.clone();
    let is_self = actor.actor_id == active_actor_id;
    let following = actor.is_following;
    let linked_uid = actor.linked_user_id.filter(|uid| *uid > 0 && !is_self);

    div()
        .id(SharedString::from(format!("explore-{prefix}-actor-{}", actor.actor_id)))
        .cursor_pointer()
        .p(px(11.0))
        .rounded(px(11.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.open_actor_profile(handle.clone(), cx);
            }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(render_actor_avatar(
                    &actor.actor_id,
                    &actor.display_name,
                    actor.actor_kind,
                    actor.avatar_media_id.as_deref(),
                    &actor.avatar_shape,
                    &actor.avatar_color,
                    38.0,
                    pal.card_bg,
                    base_url,
                    true,
                    pal,
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
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(actor.display_name),
                                )
                                .child(render_actor_badge_row(
                                    actor.actor_kind,
                                    actor.verified_tier,
                                    actor.bot_badge.as_deref(),
                                    false,
                                    false,
                                    &actor.avatar_color,
                                    pal,
                                    lang,
                                )),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!(
                                    "@{} · {} {} · {} karma",
                                    actor.handle,
                                    format_compact_count(actor.followers_count as i64),
                                    tr_str(lang, "关注者", "followers"),
                                    format_compact_count(actor.post_karma + actor.comment_karma),
                                )),
                        )
                        .when(!actor.bio.trim().is_empty(), |col| {
                            col.child(
                                div()
                                    .truncate()
                                    .text_size(px(11.5))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(actor.bio),
                            )
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .flex_shrink_0()
                .when_some(linked_uid, |row, uid| {
                    row.child(
                        div()
                            .id(SharedString::from(format!(
                                "explore-{prefix}-dm-{}",
                                actor.actor_id
                            )))
                            .cursor_pointer()
                            .px(px(9.0))
                            .py(px(4.5))
                            .rounded_full()
                            .bg(pal.service_pill_rgba())
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(pal.text_primary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.emit_action(
                                        CommunityWorkspaceAction::OpenDmWithActor {
                                            linked_user_id: uid,
                                        },
                                        cx,
                                    );
                                }),
                            )
                            .child(tr_str(lang, "私信", "Message")),
                    )
                })
                .when(!is_self, |row| {
                    row.child(
                        div()
                            .id(SharedString::from(format!(
                                "explore-{prefix}-follow-{}",
                                actor.actor_id
                            )))
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(4.5))
                            .rounded_full()
                            .bg(if following {
                                pal.service_pill_rgba()
                            } else {
                                rgb(pal.accent).into()
                            })
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if following {
                                pal.text_secondary
                            } else {
                                pal.active_button_fg
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_follow_actor(aid.clone(), !following, cx);
                                }),
                            )
                            .child(if following {
                                tr_str(lang, "已关注", "Following")
                            } else {
                                tr_str(lang, "关注", "Follow")
                            }),
                    )
                }),
        )
}

fn render_explore_community_card(
    comm: SubCommunityDto,
    prefix: &str,
    pal: &TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> Stateful<Div> {
    let nav_slug = comm.slug.clone();
    let join_slug = comm.slug.clone();
    let joined = comm.is_joined;
    let emoji = if comm.icon_emoji.trim().is_empty() {
        "🌐".to_string()
    } else {
        comm.icon_emoji.clone()
    };

    div()
        .id(SharedString::from(format!("explore-{prefix}-comm-{}", comm.slug)))
        .cursor_pointer()
        .p(px(11.0))
        .rounded(px(11.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.navigate_to(CommunityRoute::CommunityPage(nav_slug.clone()), cx);
            }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(9.0))
                .child(
                    div()
                        .w(px(34.0))
                        .h(px(34.0))
                        .rounded(px(9.0))
                        .bg(pal.service_pill_rgba())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(16.0))
                        .child(emoji),
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
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.accent))
                                .child(format!("c/{} · {}", comm.slug, comm.name)),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(comm.description),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!(
                                    "{} {} ({}H · {}B) · {} {}",
                                    format_compact_count(comm.member_count as i64),
                                    tr_str(lang, "成员", "members"),
                                    comm.human_member_count,
                                    comm.bot_member_count,
                                    format_compact_count(comm.posts_count as i64),
                                    tr_str(lang, "帖子", "posts"),
                                )),
                        ),
                ),
        )
        .child(
            div()
                .id(SharedString::from(format!("explore-{prefix}-join-{}", comm.slug)))
                .cursor_pointer()
                .flex_shrink_0()
                .px(px(11.0))
                .py(px(5.0))
                .rounded_full()
                .bg(if joined {
                    pal.service_pill_rgba()
                } else {
                    rgb(pal.accent).into()
                })
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(if joined {
                    pal.text_secondary
                } else {
                    pal.active_button_fg
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_join_community(join_slug.clone(), !joined, cx);
                    }),
                )
                .child(if joined {
                    tr_str(lang, "已加入", "Joined")
                } else {
                    tr_str(lang, "加入", "Join")
                }),
        )
}
