//! Cloud Creations workspace (`MyCreationsView`) and collapsible sidebar-adjacent
//! Quick Creations Drawer (`render_quick_creations_drawer`).

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::creations::{CreationKind, CreationReviewStatus, UserCreationDto};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::{
    macos_primary_pill_btn, macos_secondary_pill_btn,
};
use crate::views::content_hub::apple_primitives::{
    apple_grouped_card, cover_artwork_box, creation_kind_accent, creation_kind_label,
    empty_state_card_shell, filter_pill, render_editable_field_box, review_status_badge,
};
use crate::views::content_hub::publish_review_modal::ReviewerAccessState;
use crate::views::content_hub::{
    ContentHubEvent, ContentHubInputTarget, ContentHubSection, ContentHubWorkspace,
};

/// Width of the collapsible sidebar-adjacent Quick Creations Drawer in logical pixels.
pub const QUICK_DRAWER_WIDTH: f32 = 268.0;

fn render_creation_card(
    item: &UserCreationDto,
    selected: bool,
    pal: TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<ContentHubWorkspace>,
) -> Stateful<Div> {
    let cid = item.creation_id.clone();
    let kind = item.kind;
    let status = item.review_status;
    let accent = creation_kind_accent(kind);
    let cid_open = cid.clone();
    let cid_publish = cid.clone();
    let cid_withdraw = cid.clone();
    let cid_unpublish = cid.clone();
    let cid_bot = cid.clone();
    let cid_del = cid.clone();
    let item_for_modal = item.clone();
    let card_id = SharedString::from(format!("creation-card-{cid}"));
    let border_col = if selected {
        TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.72)
    } else {
        pal.separator_rgba()
    };

    div()
        .id(card_id)
        .w_full()
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(border_col)
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(12.0))
                .child(cover_artwork_box(&item.cover, 56.0, 68.0, 10.0, accent))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_size(px(15.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(if item.title.trim().is_empty() {
                                            tr_str(lang, "未命名作品", "Untitled Creation")
                                                .to_string()
                                        } else {
                                            item.title.clone()
                                        }),
                                )
                                .child(review_status_badge(status, lang, pal)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_size(px(12.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(creation_kind_label(kind, lang))
                                .child("·")
                                .child(format!("Rev #{}", item.cloud_revision))
                                .when_some(item.published_version, |el, pv| {
                                    el.child("·").child(format!("Live v{pv}"))
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(if item.summary.trim().is_empty() {
                                    tr_str(lang, "暂无简介，点击编辑完善作品信息。", "No summary yet. Edit metadata before submitting.")
                                        .to_string()
                                } else {
                                    item.summary.clone()
                                }),
                        )
                        .when_some(
                            item.rejection_reason
                                .as_deref()
                                .filter(|r| !r.trim().is_empty()),
                            |el, reason| {
                                el.child(
                                    div()
                                        .mt(px(2.0))
                                        .px(px(8.0))
                                        .py(px(4.0))
                                        .rounded(px(6.0))
                                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.12))
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.danger))
                                        .child(format!(
                                            "{}: {}",
                                            tr_str(lang, "退回原因", "Rejection reason"),
                                            reason
                                        )),
                                )
                            },
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id(SharedString::from(format!("creation-open-{cid_open}")))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .bg(pal.row_hover_rgba())
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.selected_creation_id = Some(cid_open.clone());
                                this.selected_catalog_item =
                                    this.find_catalog_item(&cid_open).cloned();
                                this.selected_detail_creation =
                                    this.find_my_creation(&cid_open).cloned();
                                cx.emit(ContentHubEvent::OpenCreationDetail {
                                    section: ContentHubSection::from_creation_kind(kind),
                                    creation_id: cid_open.clone(),
                                    action: Some("open".to_string()),
                                });
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "预览", "Preview")),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("creation-publish-{cid_publish}")))
                        .cursor_pointer()
                        .px(px(11.0))
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .bg(rgb(pal.accent))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.open_publish_modal_for_creation(item_for_modal.clone(), cx);
                            }),
                        )
                        .child(tg_icon(IconName::HubPublish, 13.0, pal.active_button_fg))
                        .child(tr_str(lang, "推送审核上架", "Submit for Review")),
                )
                .when(status == CreationReviewStatus::PendingReview, |el| {
                    el.child(
                        div()
                            .id(SharedString::from(format!("creation-withdraw-{cid_withdraw}")))
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(8.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(0xFF9F0A, 0.16))
                            .text_size(px(12.0))
                            .text_color(rgb(0xFF9F0A))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |_this, _, _, cx| {
                                    cx.emit(ContentHubEvent::WithdrawCreationReview {
                                        creation_id: cid_withdraw.clone(),
                                    });
                                }),
                            )
                            .child(tr_str(lang, "撤回审核", "Withdraw")),
                    )
                })
                .when(status == CreationReviewStatus::Published, |el| {
                    el.child(
                        div()
                            .id(SharedString::from(format!("creation-unpublish-{cid_unpublish}")))
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(8.0))
                            .bg(pal.row_hover_rgba())
                            .text_size(px(12.0))
                            .text_color(rgb(pal.text_secondary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |_this, _, _, cx| {
                                    cx.emit(ContentHubEvent::UnpublishCreation {
                                        creation_id: cid_unpublish.clone(),
                                    });
                                }),
                            )
                            .child(tr_str(lang, "下架", "Unpublish")),
                    )
                })
                .child(
                    div()
                        .id(SharedString::from(format!("creation-bot-edit-{cid_bot}")))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .bg(pal.row_hover_rgba())
                        .text_size(px(12.0))
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _, _, cx| {
                                cx.emit(ContentHubEvent::OpenCreatorBotWithCommand(format!(
                                    "/editwork {} title=",
                                    cid_bot
                                )));
                            }),
                        )
                        .child(tr_str(lang, "Bot 编辑", "Edit via Bot")),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("creation-delete-{cid_del}")))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.12))
                        .text_size(px(12.0))
                        .text_color(rgb(pal.danger))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _, _, cx| {
                                cx.emit(ContentHubEvent::DeleteCreation {
                                    creation_id: cid_del.clone(),
                                });
                            }),
                        )
                        .child(tr_str(lang, "删除", "Delete")),
                ),
        )
}

/// Renders the full-width `MyCreations` Apple HIG workspace pane.
pub fn render_my_creations_view(
    hub: &mut ContentHubWorkspace,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = hub.palette;
    let lang = hub.language_id.clone();
    let kind_filter = hub.creations_kind_filter;
    let status_filter = hub.creations_status_filter;
    let search_focused = hub.active_input == ContentHubInputTarget::CreationsSearch;
    let search_field = hub.creations_search.clone();
    let reviewer_allowed = hub.publish_modal.reviewer_access == ReviewerAccessState::Allowed;
    let review_queue_count = hub.publish_modal.review_queue.len();
    let filtered = hub.filtered_my_creations();
    let selected_id = hub.selected_creation_id.clone();
    let rev = hub.my_creations_revision;
    let loading = hub.is_loading_creations;

    let kind_tabs: &[(Option<CreationKind>, &'static str)] = &[
        (None, "my-creations-kind-all"),
        (Some(CreationKind::Book), "my-creations-kind-book"),
        (Some(CreationKind::Music), "my-creations-kind-music"),
        (Some(CreationKind::Game), "my-creations-kind-game"),
        (Some(CreationKind::App), "my-creations-kind-app"),
    ];
    let status_tabs: &[(Option<CreationReviewStatus>, &'static str, &'static str, &'static str)] = &[
        (None, "my-creations-status-all", "全部状态", "All Status"),
        (Some(CreationReviewStatus::Draft), "my-creations-status-draft", "草稿", "Draft"),
        (Some(CreationReviewStatus::PendingReview), "my-creations-status-pending", "审核中", "Pending"),
        (Some(CreationReviewStatus::Published), "my-creations-status-published", "已上架", "Published"),
        (Some(CreationReviewStatus::Rejected), "my-creations-status-rejected", "已退回", "Rejected"),
        (Some(CreationReviewStatus::Unpublished), "my-creations-status-unpublished", "已下架", "Unpublished"),
    ];

    div()
        .id("my-creations-workspace")
        .size_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div()
                .w_full()
                .px(px(24.0))
                .py(px(16.0))
                .border_b_1()
                .border_color(pal.separator_rgba())
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .text_size(px(22.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(pal.text_primary))
                                                .child(tr_str(&lang, "我的云端创作", "My Cloud Creations")),
                                        )
                                        .child(
                                            div()
                                                .px(px(8.0))
                                                .py(px(2.0))
                                                .rounded(px(999.0))
                                                .bg(pal.row_hover_rgba())
                                                .text_size(px(11.0))
                                                .text_color(rgb(pal.text_secondary))
                                                .child(format!("Rev #{rev}")),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(tr_str(
                                            &lang,
                                            "使用 @CreatorStudioBot 创作图书、音乐、游戏并一键推送审核上架",
                                            "Create books, music & games with @CreatorStudioBot and submit to Apple Hub catalogs",
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .when(reviewer_allowed, |el| {
                                    let q_label = if review_queue_count > 0 {
                                        format!(
                                            "{} ({})",
                                            tr_str(&lang, "审核队列", "Review Queue"),
                                            review_queue_count
                                        )
                                    } else {
                                        tr_str(&lang, "审核队列", "Review Queue").to_string()
                                    };
                                    el.child(
                                        div()
                                            .id("my-creations-open-review-queue")
                                            .cursor_pointer()
                                            .px(px(12.0))
                                            .py(px(7.0))
                                            .rounded(px(9.0))
                                            .bg(TdesktopThemePalette::rgba_with_alpha(0xFF9F0A, 0.16))
                                            .border_1()
                                            .border_color(TdesktopThemePalette::rgba_with_alpha(0xFF9F0A, 0.35))
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(0xFF9F0A))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.open_reviewer_queue(cx);
                                                }),
                                            )
                                            .child(q_label),
                                    )
                                })
                                .child(
                                    macos_secondary_pill_btn(
                                        "my-creations-refresh-btn",
                                        if loading {
                                            tr_str(&lang, "同步中…", "Syncing…")
                                        } else {
                                            tr_str(&lang, "云端同步", "Sync Cloud")
                                        },
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::RefreshMyCreations);
                                        }),
                                    ),
                                )
                                .child(
                                    macos_primary_pill_btn(
                                        "my-creations-bot-book-btn",
                                        tr_str(&lang, "+ Bot 创作图书", "+ Book via Bot"),
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
                                )
                                .child(
                                    macos_secondary_pill_btn(
                                        "my-creations-bot-music-btn",
                                        tr_str(&lang, "+ 音乐", "+ Music"),
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::OpenCreatorBot(Some(
                                                CreationKind::Music,
                                            )));
                                        }),
                                    ),
                                )
                                .child(
                                    macos_secondary_pill_btn(
                                        "my-creations-bot-game-btn",
                                        tr_str(&lang, "+ 游戏", "+ Game"),
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
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(10.0))
                        .child(
                            div().w(px(260.0)).child(
                                render_editable_field_box(
                                    "my-creations-search-input",
                                    &search_field,
                                    tr_str(&lang, "搜索我的作品标题或简介…", "Search my creations…"),
                                    search_focused,
                                    false,
                                    pal,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.focus_input(ContentHubInputTarget::CreationsSearch, cx);
                                    }),
                                ),
                            ),
                        )
                        .child(
                            div().flex().items_center().gap(px(6.0)).children(
                                kind_tabs.iter().map(|(k_opt, pill_id)| {
                                    let kc = *k_opt;
                                    let lbl = match kc {
                                        None => tr_str(&lang, "全部类型", "All"),
                                        Some(k) => creation_kind_label(k, &lang),
                                    };
                                    filter_pill(*pill_id, lbl, kind_filter == kc, pal)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.creations_kind_filter = kc;
                                                cx.notify();
                                            }),
                                        )
                                }),
                            ),
                        ),
                )
                .child(
                    div().flex().flex_wrap().items_center().gap(px(6.0)).children(
                        status_tabs.iter().map(|(st_opt, pill_id, zh, en)| {
                            let stc = *st_opt;
                            filter_pill(
                                *pill_id,
                                tr_str(&lang, zh, en),
                                status_filter == stc,
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.creations_status_filter = stc;
                                    cx.notify();
                                }),
                            )
                        }),
                    ),
                ),
        )
        .child(
            div()
                .id("my-creations-scroll-body")
                .flex_1()
                .w_full()
                .overflow_y_scroll()
                .p(px(24.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .when(filtered.is_empty(), |el| {
                    let cta = macos_primary_pill_btn(
                        "my-creations-empty-cta",
                        tr_str(&lang, "打开 @CreatorStudioBot 开始创作", "Open @CreatorStudioBot"),
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _, _, cx| {
                            cx.emit(ContentHubEvent::OpenCreatorBot(None));
                        }),
                    );
                    el.child(empty_state_card_shell(
                        IconName::HubSparkles,
                        tr_str(&lang, "还没有匹配的云端作品", "No Cloud Creations Yet"),
                        tr_str(
                            &lang,
                            "在聊天中通过 @CreatorStudioBot 使用 /newbook、/newmusic、/newgame 创作并保存到云端。",
                            "Use /newbook, /newmusic, or /newgame with @CreatorStudioBot to save creations to your cloud library.",
                        ),
                        cta,
                        pal,
                    ))
                })
                .children(filtered.iter().map(|item| {
                    let is_sel = selected_id.as_deref() == Some(item.creation_id.as_str());
                    render_creation_card(item, is_sel, pal, &lang, cx)
                })),
        )
        .into_any_element()
}

/// Renders the collapsible sidebar-adjacent Quick Creations Drawer (`268px`) mounted directly
/// beside `AppleAppSidebar` so cloud creations are reachable from Chats OR any Apple vertical.
pub fn render_quick_creations_drawer(
    hub: Entity<ContentHubWorkspace>,
    cx: &App,
) -> AnyElement {
    let (pal, lang, items, active_filter, search_field, search_focused) = {
        let h = hub.read(cx);
        (
            h.palette,
            h.language_id.clone(),
            h.filtered_quick_drawer_creations(),
            h.quick_drawer_kind_filter,
            h.quick_drawer_search.clone(),
            h.active_input == ContentHubInputTarget::QuickDrawerSearch,
        )
    };
    let hub_open_full = hub.clone();
    let hub_close = hub.clone();
    let hub_search = hub.clone();
    let hub_bot = hub.clone();
    let kinds: &[(Option<CreationKind>, &'static str)] = &[
        (None, "quick-drawer-kind-all"),
        (Some(CreationKind::Book), "quick-drawer-kind-book"),
        (Some(CreationKind::Music), "quick-drawer-kind-music"),
        (Some(CreationKind::Game), "quick-drawer-kind-game"),
        (Some(CreationKind::App), "quick-drawer-kind-app"),
    ];

    div()
        .id("quick-creations-drawer")
        .w(px(QUICK_DRAWER_WIDTH))
        .h_full()
        .flex_shrink_0()
        .bg(pal.sidebar_bg_rgba())
        .border_r_1()
        .border_color(pal.separator_rgba())
        .flex()
        .flex_col()
        .child(
            div()
                .w_full()
                .h(px(52.0))
                .px(px(12.0))
                .border_b_1()
                .border_color(pal.separator_rgba())
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon(IconName::HubCreations, 15.0, pal.accent))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(&lang, "我的云端作品", "Cloud Creations")),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .id("quick-drawer-open-hub-btn")
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .bg(pal.row_hover_rgba())
                                .text_size(px(11.0))
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    hub_open_full.update(cx, |_, hcx| {
                                        hcx.emit(ContentHubEvent::NavigateSection(
                                            ContentHubSection::MyCreations,
                                        ));
                                    });
                                })
                                .child(tr_str(&lang, "全部", "All")),
                        )
                        .child(
                            div()
                                .id("quick-drawer-close-btn")
                                .cursor_pointer()
                                .w(px(24.0))
                                .h(px(24.0))
                                .rounded(px(6.0))
                                .hover(move |s| s.bg(pal.row_hover_rgba()))
                                .flex()
                                .items_center()
                                .justify_center()
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    hub_close.update(cx, |_, hcx| {
                                        hcx.emit(ContentHubEvent::CloseQuickDrawer);
                                        hcx.notify();
                                    });
                                })
                                .child(tg_icon(IconName::Close, 13.0, pal.icon_fg)),
                        ),
                ),
        )
        .child(
            div()
                .p(px(10.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .border_b_1()
                .border_color(pal.separator_rgba())
                .child(
                    render_editable_field_box(
                        "quick-drawer-search-input",
                        &search_field,
                        tr_str(&lang, "快速搜索作品…", "Filter creations…"),
                        search_focused,
                        false,
                        pal,
                    )
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        hub_search.update(cx, |h, hcx| {
                            h.focus_input(ContentHubInputTarget::QuickDrawerSearch, hcx);
                        });
                    }),
                )
                .child(
                    div().flex().flex_wrap().gap(px(4.0)).children(kinds.iter().map(
                        |(k_opt, pill_id)| {
                            let kc = *k_opt;
                            let hub_k = hub.clone();
                            let lbl = match kc {
                                None => tr_str(&lang, "全部", "All"),
                                Some(k) => creation_kind_label(k, &lang),
                            };
                            filter_pill(*pill_id, lbl, active_filter == kc, pal).on_mouse_down(
                                MouseButton::Left,
                                move |_, _, cx| {
                                    hub_k.update(cx, |h, hcx| {
                                        h.quick_drawer_kind_filter = kc;
                                        hcx.notify();
                                    });
                                },
                            )
                        },
                    )),
                ),
        )
        .child(
            div()
                .id("quick-drawer-items-scroll")
                .flex_1()
                .w_full()
                .overflow_y_scroll()
                .p(px(10.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .when(items.is_empty(), |el| {
                    el.child(
                        apple_grouped_card(pal).p(px(14.0)).child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(tr_str(
                                    &lang,
                                    "暂无云端作品，点击下方按钮通过 Bot 创作。",
                                    "No creations found. Create with @CreatorStudioBot below.",
                                )),
                        ),
                    )
                })
                .children(items.iter().map(|item| {
                    let cid = item.creation_id.clone();
                    let kind = item.kind;
                    let accent = creation_kind_accent(kind);
                    let hub_open = hub.clone();
                    let hub_pub = hub.clone();
                    let item_clone = item.clone();

                    apple_grouped_card(pal)
                        .p(px(10.0))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("quick-drawer-item-{cid}")))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    hub_open.update(cx, |h, hcx| {
                                        h.selected_creation_id = Some(cid.clone());
                                        h.selected_catalog_item =
                                            h.find_catalog_item(&cid).cloned();
                                        h.selected_detail_creation =
                                            h.find_my_creation(&cid).cloned();
                                        hcx.emit(ContentHubEvent::OpenCreationDetail {
                                            section: ContentHubSection::from_creation_kind(kind),
                                            creation_id: cid.clone(),
                                            action: Some("open".to_string()),
                                        });
                                        hcx.notify();
                                    });
                                })
                                .child(cover_artwork_box(&item.cover, 36.0, 36.0, 8.0, accent))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(pal.text_primary))
                                                .child(item.title.clone()),
                                        )
                                        .child(review_status_badge(item.review_status, &lang, pal)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "quick-drawer-publish-{}",
                                            item.creation_id
                                        )))
                                        .cursor_pointer()
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.accent))
                                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                            hub_pub.update(cx, |h, hcx| {
                                                h.open_publish_modal_for_creation(
                                                    item_clone.clone(),
                                                    hcx,
                                                );
                                            });
                                        })
                                        .child(tr_str(&lang, "推送审核", "Publish")),
                                ),
                        )
                })),
        )
        .child(
            div()
                .p(px(10.0))
                .border_t_1()
                .border_color(pal.separator_rgba())
                .child(
                    macos_primary_pill_btn(
                        "quick-drawer-bot-cta",
                        tr_str(&lang, "用 @CreatorStudioBot 创作", "Create with Bot"),
                        pal,
                    )
                    .w_full()
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        hub_bot.update(cx, |_, hcx| {
                            hcx.emit(ContentHubEvent::OpenCreatorBot(None));
                        });
                    }),
                ),
        )
        .into_any_element()
}
