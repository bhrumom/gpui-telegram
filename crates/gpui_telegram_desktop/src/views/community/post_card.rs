//! Fused Reddit + X `PostCard` renderer (Card/Compact/Classic, Markdown, 1–4 Media Mosaic,
//! Interactive Polls with Human/Bot breakdown, Quote/Crosspost embeds, Unified Engagement Bar,
//! and Explainability / Award / Quote popovers).

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::community_contracts::{
    CommunityPostDto, EmbeddedPostSummaryDto, EngagementSignalKind, FeedItemDto, LinkPreviewDto,
    PollDto, PostMediaAttachmentDto,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::macos_popover_surface;
use crate::views::stage::entities_render::render_rich_message_body;
use crate::views::stage::rich_meta::resolve_image_source;

use super::shared_widgets::{
    format_compact_count, format_relative_time, linkify_community_slugs, render_actor_avatar,
    render_actor_badge_row, render_flair_chip, render_input_display, render_spoiler_veil,
    render_vote_stepper_pill, resolve_community_media_url,
};
use super::state::{
    ActiveCommunityInput, CommunityPopover, CommunityRoute, CommunityWorkspaceAction, FeedDensity,
    PollBreakdownMode,
};
use super::workspace::CommunityWorkspace;

/// Returns the Compact-density preview headline for a post, masking raw `post.body`
/// whenever `veil_locked` is `true` for NSFW or Spoiler content.
pub fn compact_post_preview_text(post: &CommunityPostDto, veil_locked: bool, lang: &str) -> String {
    if veil_locked {
        if post.is_nsfw && post.is_spoiler {
            tr_str(
                lang,
                "[18+ NSFW · 剧透] 内容已隐藏 · 点击展开",
                "[18+ NSFW · SPOILER] Hidden content · Click to reveal",
            )
            .to_string()
        } else if post.is_nsfw {
            tr_str(
                lang,
                "[18+ NSFW] 敏感内容已隐藏 · 点击查看",
                "[18+ NSFW] Sensitive content hidden · Click to reveal",
            )
            .to_string()
        } else {
            tr_str(
                lang,
                "[剧透] 内容已折叠 · 点击展开",
                "[SPOILER] Content hidden · Click to reveal",
            )
            .to_string()
        }
    } else {
        post.title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| post.body.chars().take(120).collect())
    }
}

/// Renders a single feed post card respecting the active `FeedDensity` (`Card | Compact | Classic`).
pub fn render_post_card(
    ws: &CommunityWorkspace,
    item: &FeedItemDto,
    idx: usize,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    render_post_card_inner(ws, item, idx, false, cx)
}

/// Renders a post in expanded full-detail mode (always full Markdown body, media gallery,
/// interactive poll, embeds, and engagement bar regardless of feed density).
pub fn render_post_card_expanded(
    ws: &CommunityWorkspace,
    item: &FeedItemDto,
    idx: usize,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    render_post_card_inner(ws, item, idx, true, cx)
}

fn render_post_card_inner(
    ws: &CommunityWorkspace,
    item: &FeedItemDto,
    idx: usize,
    force_expanded: bool,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let post = &item.post;
    let pid = post.post_id.clone();
    let density = if force_expanded {
        FeedDensity::Card
    } else {
        ws.state.density
    };
    let veil_locked = (post.is_nsfw || post.is_spoiler)
        && !ws.state.revealed_post_veils.contains(&post.post_id);

    if density == FeedDensity::Compact {
        return render_compact_post_row(ws, item, idx, veil_locked, cx);
    }

    let show_explain = item.explanation.is_some() && ws.state.algo_prefs.show_explanations;
    let hover_bg = pal.row_hover_rgba();

    let header = render_post_header(ws, post, idx, show_explain, cx);
    let body_section = if veil_locked {
        let reveal_id = pid.clone();
        render_spoiler_veil(
            SharedString::from(format!("post-veil-{pid}-{idx}")),
            post.is_nsfw,
            &pal,
            lang,
            cx,
            move |this, cx| {
                this.state.card_child_click_consumed = true;
                this.state.revealed_post_veils.insert(reveal_id.clone());
                cx.notify();
            },
        )
        .into_any_element()
    } else {
        render_post_body_and_attachments(ws, post, idx, cx)
    };
    let footer = render_engagement_bar(ws, post, idx, density == FeedDensity::Classic, cx);

    let card_id = SharedString::from(format!("post-card-{pid}-{idx}"));
    let open_pid = pid.clone();

    let inner = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(header)
        .child(body_section)
        .child(footer);

    let ob = pal.openbot_palette();
    let card_hover = ob.bg_control_hover();
    let _ = hover_bg;
    div()
        .id(card_id)
        .w_full()
        .p(px(14.0))
        .rounded(px(14.0))
        .bg(ob.bg_surface())
        .border_1()
        .border_color(ob.frame_card_border())
        .hover(move |s| s.bg(card_hover))
        .flex()
        .gap(px(10.0))
        .when(density == FeedDensity::Classic, |row| {
            let vote_pid = pid.clone();
            row.child(
                div()
                    .flex_shrink_0()
                    .pt(px(2.0))
                    .child(render_vote_stepper_pill(
                        format!("classic-vote-{pid}-{idx}"),
                        post.score,
                        post.viewer_vote,
                        &pal,
                        cx,
                        move |this, dir, cx| {
                            this.state.card_child_click_consumed = true;
                            this.vote_post(vote_pid.clone(), dir, cx);
                        },
                    )),
            )
        })
        .child(inner)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, _| {
                this.state.card_child_click_consumed = false;
            }),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            if std::mem::take(&mut this.state.card_child_click_consumed) {
                return;
            }
            if matches!(&this.state.route, CommunityRoute::PostDetail(cur) if cur == &open_pid) {
                return;
            }
            this.open_post_detail(open_pid.clone(), cx);
        }))
        .into_any_element()
}

fn render_compact_post_row(
    ws: &CommunityWorkspace,
    item: &FeedItemDto,
    idx: usize,
    veil_locked: bool,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let post = &item.post;
    let pid = post.post_id.clone();
    let vote_pid = pid.clone();
    let open_pid = pid.clone();
    let reveal_pid = pid.clone();
    let preview = compact_post_preview_text(post, veil_locked, lang);
    let veil_accent = if post.is_nsfw { pal.danger } else { 0x8B5CF6 };

    div()
        .id(SharedString::from(format!("post-compact-{pid}-{idx}")))
        .cursor_pointer()
        .w_full()
        .px(px(12.0))
        .py(px(9.0))
        .rounded(px(10.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .hover(move |s| s.bg(pal.row_hover_rgba()))
        .flex()
        .items_center()
        .gap(px(10.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, _| {
                this.state.card_child_click_consumed = false;
            }),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            if std::mem::take(&mut this.state.card_child_click_consumed) {
                return;
            }
            if matches!(&this.state.route, CommunityRoute::PostDetail(cur) if cur == &open_pid) {
                return;
            }
            this.open_post_detail(open_pid.clone(), cx);
        }))
        .child(render_vote_stepper_pill(
            format!("compact-vote-{pid}-{idx}"),
            post.score,
            post.viewer_vote,
            &pal,
            cx,
            move |this, dir, cx| {
                this.state.card_child_click_consumed = true;
                this.vote_post(vote_pid.clone(), dir, cx);
            },
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
                        .flex_wrap()
                        .gap(px(6.0))
                        .when_some(post.community_slug.clone(), |row, slug| {
                            row.child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(pal.accent))
                                    .child(format!("c/{slug}")),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("@{}", post.author_handle)),
                        )
                        .child(render_actor_badge_row(
                            post.author_kind,
                            post.author_verified_tier,
                            post.author_bot_badge.as_deref(),
                            post.is_mod_distinguished,
                            false,
                            &post.author_avatar_color,
                            &pal,
                            lang,
                        ))
                        .when(post.is_nsfw, |row| {
                            row.child(render_flair_chip("18+ NSFW", Some("#EF4444"), &pal))
                        })
                        .when(post.is_spoiler, |row| {
                            row.child(render_flair_chip(
                                tr_str(lang, "剧透", "SPOILER"),
                                Some("#8B5CF6"),
                                &pal,
                            ))
                        })
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(rgb(pal.text_secondary))
                                .child(format_relative_time(post.created_at_ms, lang)),
                        ),
                )
                .child(if veil_locked {
                    div()
                        .id(SharedString::from(format!("post-compact-veil-{pid}-{idx}")))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(veil_accent, 0.12))
                        .border_1()
                        .border_color(TdesktopThemePalette::rgba_with_alpha(veil_accent, 0.30))
                        .hover(move |s| {
                            s.bg(TdesktopThemePalette::rgba_with_alpha(veil_accent, 0.20))
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.card_child_click_consumed = true;
                                this.state.revealed_post_veils.insert(reveal_pid.clone());
                                cx.notify();
                            }),
                        )
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(veil_accent))
                        .truncate()
                        .child(preview)
                        .into_any_element()
                } else {
                    div()
                        .truncate()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.text_primary))
                        .child(preview)
                        .into_any_element()
                }),
        )
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(11.5))
                .text_color(rgb(pal.text_secondary))
                .child(format!("💬 {}", format_compact_count(post.comments_count as i64)))
                .child(format!("♥ {}", format_compact_count(post.likes_count as i64))),
        )
        .into_any_element()
}

fn render_post_header(
    ws: &CommunityWorkspace,
    post: &CommunityPostDto,
    card_idx: usize,
    show_explain: bool,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let author_handle = post.author_handle.clone();
    let author_handle_2 = post.author_handle.clone();
    let pid = post.post_id.clone();

    div()
        .w_full()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.0))
                .flex_1()
                .min_w(px(0.0))
                .child(
                    div()
                        .id(SharedString::from(format!("post-avatar-{pid}-{card_idx}")))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.card_child_click_consumed = true;
                                this.open_actor_profile(author_handle.clone(), cx);
                            }),
                        )
                        .child(render_actor_avatar(
                            &post.author_actor_id,
                            &post.author_display_name,
                            post.author_kind,
                            post.author_avatar_media_id.as_deref(),
                            &post.author_avatar_shape,
                            &post.author_avatar_color,
                            36.0,
                            pal.card_bg,
                            &ws.media_base_url,
                            true,
                            &pal,
                        )),
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
                                .flex_wrap()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .id(SharedString::from(format!("post-author-{pid}-{card_idx}")))
                                        .cursor_pointer()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.state.card_child_click_consumed = true;
                                                this.open_actor_profile(author_handle_2.clone(), cx);
                                            }),
                                        )
                                        .child(post.author_display_name.clone()),
                                )
                                .child(render_actor_badge_row(
                                    post.author_kind,
                                    post.author_verified_tier,
                                    post.author_bot_badge.as_deref(),
                                    post.is_mod_distinguished,
                                    false,
                                    &post.author_avatar_color,
                                    &pal,
                                    lang,
                                ))
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("@{}", post.author_handle)),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format!("· {}", format_relative_time(post.created_at_ms, lang))),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .flex_wrap()
                                .gap(px(5.0))
                                .when_some(post.community_slug.clone(), |row, slug| {
                                    let nav_slug = slug.clone();
                                    row.child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "post-comm-{pid}-{slug}-{card_idx}"
                                            )))
                                            .cursor_pointer()
                                            .px(px(6.0))
                                            .py(px(1.0))
                                            .rounded_full()
                                            .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.accent))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.state.card_child_click_consumed = true;
                                                    this.navigate_to(
                                                        CommunityRoute::CommunityPage(nav_slug.clone()),
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .child(format!("c/{slug}")),
                                    )
                                })
                                .when_some(post.flair.as_deref(), |row, fl| {
                                    row.child(render_flair_chip(fl, None, &pal))
                                })
                                .when(post.is_pinned, |row| {
                                    row.child(render_flair_chip(tr_str(lang, "置顶", "Pinned"), Some("#10B981"), &pal))
                                })
                                .when(post.is_locked, |row| {
                                    row.child(render_flair_chip(tr_str(lang, "已锁定", "Locked"), Some("#F59E0B"), &pal))
                                })
                                .when(post.is_nsfw, |row| {
                                    row.child(render_flair_chip("NSFW", Some("#EF4444"), &pal))
                                })
                                .when(post.is_spoiler, |row| {
                                    row.child(render_flair_chip(tr_str(lang, "剧透", "Spoiler"), Some("#8B5CF6"), &pal))
                                })
                                .when(post.thread_total > 1, |row| {
                                    row.child(
                                        div()
                                            .px(px(6.0))
                                            .py(px(1.0))
                                            .rounded_full()
                                            .bg(pal.service_pill_rgba())
                                            .flex()
                                            .items_center()
                                            .gap(px(3.0))
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_secondary))
                                            .child(tg_icon(IconName::CommunityThread, 11.0, pal.accent))
                                            .child(format!("{}/{}", post.thread_index + 1, post.thread_total)),
                                    )
                                }),
                        ),
                ),
        )
        .when(show_explain, |row| {
            let exp_pid = pid.clone();
            row.child(
                div()
                    .id(SharedString::from(format!("post-why-{pid}-{card_idx}")))
                    .cursor_pointer()
                    .flex_shrink_0()
                    .px(px(7.0))
                    .py(px(3.0))
                    .rounded_full()
                    .bg(pal.service_pill_rgba())
                    .hover(move |s| s.bg(pal.row_hover_rgba()))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(px(10.5))
                    .text_color(rgb(pal.text_secondary))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.state.card_child_click_consumed = true;
                            this.toggle_popover(CommunityPopover::ExplainPost(exp_pid.clone()), cx);
                        }),
                    )
                    .child(tg_icon(IconName::SparkleAlgo, 11.5, pal.accent))
                    .child(tr_str(lang, "为何推荐?", "Why this?")),
            )
        })
}

fn render_post_body_and_attachments(
    ws: &CommunityWorkspace,
    post: &CommunityPostDto,
    idx: usize,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let msg_id = gpui_telegram_community::fnv1a_64(post.post_id.as_bytes())
        .wrapping_add(idx as u64);
    let linked_body = linkify_community_slugs(&post.body);
    let rich_body = if !linked_body.trim().is_empty() {
        Some(render_rich_message_body(
            msg_id,
            &linked_body,
            &[],
            &ws.state.revealed_spoilers,
            &ws.state.expanded_quotes,
            ws.state.copied_code_msg,
            &pal,
            lang,
            false,
            cx,
            |this, action, cx| this.handle_rich_text_action(action, cx),
        ))
    } else {
        None
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .when_some(post.title.as_deref().filter(|t| !t.trim().is_empty()), |col, title| {
            col.child(
                div()
                    .text_size(px(pal.scaled_px(15.0)))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(pal.text_primary))
                    .child(title.to_string()),
            )
        })
        .when_some(rich_body, |col, el| col.child(el))
        .when(!post.media.is_empty(), |col| {
            col.child(render_media_mosaic(ws, &post.post_id, idx, &post.media, cx))
        })
        .when_some(post.link_preview.as_ref(), |col, lp| {
            col.child(render_link_preview_card(&post.post_id, idx, lp, &pal, cx))
        })
        .when_some(post.poll.as_ref(), |col, poll| {
            col.child(render_poll_widget(ws, &post.post_id, idx, poll, cx))
        })
        .when_some(post.quoted_post.as_deref(), |col, emb| {
            col.child(render_embedded_post(
                &post.post_id,
                idx,
                "quote",
                tr_str(lang, "引用", "Quoted"),
                emb,
                &pal,
                lang,
                cx,
            ))
        })
        .when_some(post.crosspost_origin.as_deref(), |col, emb| {
            col.child(render_embedded_post(
                &post.post_id,
                idx,
                "crosspost",
                tr_str(lang, "跨社区转发", "Crosspost"),
                emb,
                &pal,
                lang,
                cx,
            ))
        })
        .when_some(post.reposted_post.as_deref(), |col, emb| {
            col.child(render_embedded_post(
                &post.post_id,
                idx,
                "repost",
                tr_str(lang, "转发", "Repost"),
                emb,
                &pal,
                lang,
                cx,
            ))
        })
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_media_tile(
    post_id: &str,
    card_idx: usize,
    media_idx: usize,
    media: &PostMediaAttachmentDto,
    height_px: f32,
    base_url: &str,
    pal: &TdesktopThemePalette,
    cx: &mut Context<CommunityWorkspace>,
) -> Stateful<Div> {
    let resolved = resolve_community_media_url(
        base_url,
        Some(&media.media_id),
        media.url.as_deref().or(media.thumbnail_url.as_deref()),
    )
    .unwrap_or_else(|| media.media_id.clone());
    let is_video = media.kind.eq_ignore_ascii_case("video");
    let img_src = resolve_image_source(&resolved, None);
    let open_ref = resolved.clone();

    div()
        .id(SharedString::from(format!(
            "post-media-{post_id}-{card_idx}-{media_idx}"
        )))
        .relative()
        .cursor_pointer()
        .flex_1()
        .min_w(px(0.0))
        .h(px(height_px))
        .rounded(px(9.0))
        .overflow_hidden()
        .bg(pal.service_pill_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.state.card_child_click_consumed = true;
                this.emit_action(CommunityWorkspaceAction::OpenMediaLightbox(open_ref.clone()), cx);
            }),
        )
        .when_some(img_src, |el, src| {
            el.child(img(src).size_full().object_fit(ObjectFit::Cover))
        })
        .when(img_src_missing(&resolved), |el| {
            el.flex()
                .items_center()
                .justify_center()
                .flex_col()
                .gap(px(4.0))
                .child(tg_icon(
                    if is_video { IconName::Play } else { IconName::Camera },
                    20.0,
                    pal.text_secondary,
                ))
                .child(
                    div()
                        .px(px(8.0))
                        .truncate()
                        .text_size(px(11.0))
                        .text_color(rgb(pal.text_secondary))
                        .child(media.alt_text.clone().unwrap_or_else(|| media.media_id.clone())),
                )
        })
        .when(is_video, |el| {
            el.child(
                div()
                    .absolute()
                    .bottom(px(8.0))
                    .left(px(8.0))
                    .px(px(7.0))
                    .py(px(3.0))
                    .rounded_full()
                    .bg(TdesktopThemePalette::rgba_with_alpha(0x000000, 0.70))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xFFFFFF))
                    .child(tg_icon(IconName::Play, 11.0, 0xFFFFFF))
                    .child(
                        media
                            .duration_secs
                            .map(|d| format!("{}:{:02}", d / 60, d % 60))
                            .unwrap_or_else(|| "VIDEO".to_string()),
                    ),
            )
        })
}

fn img_src_missing(url: &str) -> bool {
    resolve_image_source(url, None).is_none()
}

fn render_media_mosaic(
    ws: &CommunityWorkspace,
    post_id: &str,
    card_idx: usize,
    media: &[PostMediaAttachmentDto],
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let base = ws.media_base_url.as_str();
    let count = media.len().min(4);
    match count {
        1 => div().w_full().flex().child(render_media_tile(
            post_id, card_idx, 0, &media[0], 240.0, base, &pal, cx,
        )),
        2 => div()
            .w_full()
            .flex()
            .gap(px(6.0))
            .child(render_media_tile(
                post_id, card_idx, 0, &media[0], 190.0, base, &pal, cx,
            ))
            .child(render_media_tile(
                post_id, card_idx, 1, &media[1], 190.0, base, &pal, cx,
            )),
        3 => div()
            .w_full()
            .flex()
            .gap(px(6.0))
            .child(render_media_tile(
                post_id, card_idx, 0, &media[0], 210.0, base, &pal, cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(render_media_tile(
                        post_id, card_idx, 1, &media[1], 102.0, base, &pal, cx,
                    ))
                    .child(render_media_tile(
                        post_id, card_idx, 2, &media[2], 102.0, base, &pal, cx,
                    )),
            ),
        _ => div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .w_full()
                    .flex()
                    .gap(px(6.0))
                    .child(render_media_tile(
                        post_id, card_idx, 0, &media[0], 140.0, base, &pal, cx,
                    ))
                    .child(render_media_tile(
                        post_id, card_idx, 1, &media[1], 140.0, base, &pal, cx,
                    )),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .gap(px(6.0))
                    .child(render_media_tile(
                        post_id, card_idx, 2, &media[2], 140.0, base, &pal, cx,
                    ))
                    .child(render_media_tile(
                        post_id, card_idx, 3, &media[3], 140.0, base, &pal, cx,
                    )),
            ),
    }
}

fn render_link_preview_card(
    post_id: &str,
    card_idx: usize,
    lp: &LinkPreviewDto,
    pal: &TdesktopThemePalette,
    cx: &mut Context<CommunityWorkspace>,
) -> Stateful<Div> {
    let url = lp.url.clone();
    div()
        .id(SharedString::from(format!("post-link-{post_id}-{card_idx}")))
        .cursor_pointer()
        .w_full()
        .p(px(10.0))
        .rounded(px(10.0))
        .bg(pal.service_pill_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .hover(move |s| s.bg(pal.row_hover_rgba()))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.state.card_child_click_consumed = true;
                this.emit_action(CommunityWorkspaceAction::OpenExternalUrl(url.clone()), cx);
            }),
        )
        .child(
            div()
                .text_size(px(10.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(pal.text_secondary))
                .child(lp.domain.clone()),
        )
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_link))
                .child(lp.title.clone()),
        )
        .when_some(lp.description.as_deref().filter(|d| !d.trim().is_empty()), |el, desc| {
            el.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(pal.text_secondary))
                    .child(desc.chars().take(180).collect::<String>()),
            )
        })
}

fn render_poll_widget(
    ws: &CommunityWorkspace,
    post_id: &str,
    card_idx: usize,
    poll: &PollDto,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let mode = ws
        .state
        .poll_breakdown_by_post
        .get(post_id)
        .copied()
        .unwrap_or(PollBreakdownMode::All);
    let denom = match mode {
        PollBreakdownMode::All => poll.total_votes,
        PollBreakdownMode::Humans => poll.total_human_votes,
        PollBreakdownMode::Bots => poll.total_bot_votes,
    }
    .max(1);

    let mode_chip = |id_sfx: &'static str, target: PollBreakdownMode, label: &'static str| {
        let active = mode == target;
        let pid = post_id.to_string();
        div()
            .id(SharedString::from(format!(
                "poll-mode-{post_id}-{card_idx}-{id_sfx}"
            )))
            .cursor_pointer()
            .px(px(7.0))
            .py(px(2.0))
            .rounded_full()
            .bg(if active {
                TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.16)
            } else {
                pal.service_pill_rgba()
            })
            .text_size(px(10.0))
            .font_weight(if active { FontWeight::BOLD } else { FontWeight::MEDIUM })
            .text_color(rgb(if active { pal.accent } else { pal.text_secondary }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.state.card_child_click_consumed = true;
                    this.state.poll_breakdown_by_post.insert(pid.clone(), target);
                    cx.notify();
                }),
            )
            .child(label)
    };

    div()
        .w_full()
        .p(px(11.0))
        .rounded(px(10.0))
        .bg(pal.service_pill_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.text_primary))
                        .child(poll.question.clone()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .child(mode_chip("all", PollBreakdownMode::All, tr_str(lang, "全部", "All")))
                        .child(mode_chip("hum", PollBreakdownMode::Humans, tr_str(lang, "人类", "Humans")))
                        .child(mode_chip("bot", PollBreakdownMode::Bots, "Bots")),
                ),
        )
        .children(poll.options.iter().map(|opt| {
            let votes = match mode {
                PollBreakdownMode::All => opt.all_votes,
                PollBreakdownMode::Humans => opt.human_votes,
                PollBreakdownMode::Bots => opt.bot_votes,
            };
            let pct = ((votes as f64 / denom as f64) * 100.0).round() as u32;
            let voted = poll.viewer_voted_option_ids.contains(&opt.option_id);
            let pid = post_id.to_string();
            let oid = opt.option_id.clone();
            let closed = poll.is_closed;

            div()
                .id(SharedString::from(format!(
                    "poll-opt-{post_id}-{card_idx}-{}",
                    opt.option_id
                )))
                .relative()
                .cursor_pointer()
                .w_full()
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(8.0))
                .overflow_hidden()
                .bg(pal.card_surface_rgba())
                .border_1()
                .border_color(if voted {
                    rgb(pal.accent).into()
                } else {
                    pal.hairline_border_rgba()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.card_child_click_consumed = true;
                        if !closed {
                            this.vote_poll(pid.clone(), oid.clone(), cx);
                        }
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(relative((pct as f32 / 100.0).clamp(0.0, 1.0)))
                        .bg(TdesktopThemePalette::rgba_with_alpha(
                            pal.accent,
                            if voted { 0.22 } else { 0.11 },
                        )),
                )
                .child(
                    div()
                        .relative()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .when(voted, |r| r.child(tg_icon(IconName::SelectCheck, 12.0, pal.accent)))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_size(px(12.5))
                                        .font_weight(if voted { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                                        .text_color(rgb(pal.text_primary))
                                        .child(opt.text.clone()),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(11.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.text_secondary))
                                .child(format!("{pct}% ({votes})")),
                        ),
                )
        }))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .text_size(px(10.5))
                .text_color(rgb(pal.text_secondary))
                .child(format!(
                    "{} {} ({}H · {}B)",
                    poll.total_votes,
                    tr_str(lang, "票", "votes"),
                    poll.total_human_votes,
                    poll.total_bot_votes
                ))
                .child(if poll.is_closed {
                    tr_str(lang, "已结束", "Closed")
                } else if poll.multi_select {
                    tr_str(lang, "多选投票进行中", "Multi-select Open")
                } else {
                    tr_str(lang, "投票进行中", "Poll Open")
                }),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_embedded_post(
    parent_post_id: &str,
    card_idx: usize,
    embed_kind: &'static str,
    label: &'static str,
    emb: &EmbeddedPostSummaryDto,
    pal: &TdesktopThemePalette,
    lang: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> Stateful<Div> {
    let target_id = emb.post_id.clone();
    div()
        .id(SharedString::from(format!(
            "post-embed-{parent_post_id}-{card_idx}-{embed_kind}-{}",
            emb.post_id
        )))
        .cursor_pointer()
        .w_full()
        .p(px(10.0))
        .rounded(px(10.0))
        .bg(pal.service_pill_rgba())
        .border_1()
        .border_color(pal.hairline_border_rgba())
        .hover(move |s| s.bg(pal.row_hover_rgba()))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.state.card_child_click_consumed = true;
                this.open_post_detail(target_id.clone(), cx);
            }),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(5.0))
                .text_size(px(11.0))
                .text_color(rgb(pal.text_secondary))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.accent))
                        .child(label),
                )
                .child(format!("{} (@{})", emb.author_display_name, emb.author_handle))
                .when_some(emb.community_slug.as_deref(), |r, slug| r.child(format!("· c/{slug}")))
                .child(format!("· {}", format_relative_time(emb.created_at_ms, lang))),
        )
        .when_some(emb.title.as_deref().filter(|t| !t.trim().is_empty()), |el, t| {
            el.child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(pal.text_primary))
                    .child(t.to_string()),
            )
        })
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(pal.text_secondary))
                .child(emb.body_preview.clone()),
        )
}

fn render_action_pill(
    id: SharedString,
    icon: IconName,
    count_label: Option<String>,
    active_color: Option<u32>,
    pal: &TdesktopThemePalette,
    cx: &mut Context<CommunityWorkspace>,
    on_click: impl Fn(&mut CommunityWorkspace, &mut Context<CommunityWorkspace>) + 'static,
) -> Stateful<Div> {
    let fg = active_color.unwrap_or(pal.text_secondary);
    let hover_bg = pal.row_hover_rgba();
    div()
        .id(id)
        .cursor_pointer()
        .px(px(7.0))
        .py(px(4.0))
        .rounded_full()
        .hover(move |s| s.bg(hover_bg))
        .flex()
        .items_center()
        .gap(px(4.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.state.card_child_click_consumed = true;
                on_click(this, cx);
            }),
        )
        .child(tg_icon(icon, 14.0, fg))
        .when_some(count_label, |el, txt| {
            el.child(
                div()
                    .text_size(px(11.5))
                    .font_weight(if active_color.is_some() {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(rgb(fg))
                    .child(txt),
            )
        })
}

fn render_engagement_bar(
    ws: &CommunityWorkspace,
    post: &CommunityPostDto,
    idx: usize,
    omit_vote_pill: bool,
    cx: &mut Context<CommunityWorkspace>,
) -> Div {
    let pal = ws.palette;
    let pid = post.post_id.clone();
    let vote_pid = pid.clone();
    let reply_pid = pid.clone();
    let repost_pid = pid.clone();
    let quote_post = post.clone();
    let crosspost_origin = post.clone();
    let edit_post = post.clone();
    let is_owner = ws.is_viewer_owned_actor(&post.author_actor_id);
    let like_pid = pid.clone();
    let award_pid = pid.clone();
    let bookmark_pid = pid.clone();
    let share_pid = pid.clone();
    let share_summary = {
        let head = post
            .title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(&post.body);
        let snippet: String = head.chars().take(140).collect();
        match post.community_slug.as_deref() {
            Some(slug) => format!("[c/{slug}] @{} — {snippet}", post.author_handle),
            None => format!("@{} — {snippet}", post.author_handle),
        }
    };

    let award_summary = if post.awards.is_empty() {
        None
    } else {
        let emojis: String = post
            .awards
            .iter()
            .take(3)
            .map(|a| a.icon_emoji.as_str())
            .collect();
        Some(format!("{emojis} {}", post.awards.len()))
    };

    div()
        .w_full()
        .pt(px(4.0))
        .border_t_1()
        .border_color(pal.hairline_border_rgba())
        .flex()
        .items_center()
        .justify_between()
        .flex_wrap()
        .gap(px(4.0))
        .child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(4.0))
                .when(!omit_vote_pill, |row| {
                    row.child(render_vote_stepper_pill(
                        format!("card-vote-{pid}-{idx}"),
                        post.score,
                        post.viewer_vote,
                        &pal,
                        cx,
                        move |this, dir, cx| {
                            this.state.card_child_click_consumed = true;
                            this.vote_post(vote_pid.clone(), dir, cx);
                        },
                    ))
                })
                .child(render_action_pill(
                    SharedString::from(format!("card-reply-{pid}-{idx}")),
                    IconName::Reply,
                    Some(format_compact_count(post.comments_count as i64)),
                    None,
                    &pal,
                    cx,
                    move |this, cx| this.open_post_detail(reply_pid.clone(), cx),
                ))
                .child(render_action_pill(
                    SharedString::from(format!("card-repost-{pid}-{idx}")),
                    IconName::Repost,
                    Some(format_compact_count(post.reposts_count as i64)),
                    post.viewer_reposted.then_some(0x10B981),
                    &pal,
                    cx,
                    move |this, cx| this.repost_post(repost_pid.clone(), cx),
                ))
                .child(render_action_pill(
                    SharedString::from(format!("card-quote-{pid}-{idx}")),
                    IconName::Edit,
                    (post.quotes_count > 0).then(|| format_compact_count(post.quotes_count as i64)),
                    None,
                    &pal,
                    cx,
                    move |this, cx| {
                        this.open_composer_quote(quote_post.clone(), cx);
                    },
                ))
                .child(render_action_pill(
                    SharedString::from(format!("card-crosspost-{pid}-{idx}")),
                    IconName::CommunityThread,
                    None,
                    None,
                    &pal,
                    cx,
                    move |this, cx| {
                        this.open_composer_crosspost(crosspost_origin.clone(), cx);
                    },
                ))
                .when(is_owner, |row| {
                    row.child(render_action_pill(
                        SharedString::from(format!("card-edit-{pid}-{idx}")),
                        IconName::ComposePen,
                        None,
                        Some(pal.accent),
                        &pal,
                        cx,
                        move |this, cx| {
                            this.open_composer_edit(edit_post.clone(), cx);
                        },
                    ))
                })
                .child(render_action_pill(
                    SharedString::from(format!("card-like-{pid}-{idx}")),
                    IconName::HeartReaction,
                    Some(format_compact_count(post.likes_count as i64)),
                    post.viewer_liked.then_some(0xF43F5E),
                    &pal,
                    cx,
                    move |this, cx| this.toggle_like(like_pid.clone(), cx),
                ))
                .child(render_action_pill(
                    SharedString::from(format!("card-award-{pid}-{idx}")),
                    IconName::CommunityAward,
                    award_summary,
                    (!post.awards.is_empty()).then_some(0xF59E0B),
                    &pal,
                    cx,
                    move |this, cx| {
                        this.toggle_popover(CommunityPopover::AwardPost(award_pid.clone()), cx);
                    },
                )),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(render_action_pill(
                    SharedString::from(format!("card-bookmark-{pid}-{idx}")),
                    IconName::Bookmark,
                    (post.bookmarks_count > 0)
                        .then(|| format_compact_count(post.bookmarks_count as i64)),
                    post.viewer_bookmarked.then_some(pal.accent),
                    &pal,
                    cx,
                    move |this, cx| this.toggle_bookmark(bookmark_pid.clone(), cx),
                ))
                .child(render_action_pill(
                    SharedString::from(format!("card-share-{pid}-{idx}")),
                    IconName::Forward,
                    None,
                    None,
                    &pal,
                    cx,
                    move |this, cx| {
                        this.emit_action(
                            CommunityWorkspaceAction::SharePostToChat {
                                post_id: share_pid.clone(),
                                summary: share_summary.clone(),
                            },
                            cx,
                        );
                    },
                ))
                .child(
                    div()
                        .px(px(5.0))
                        .text_size(px(10.5))
                        .text_color(rgb(pal.text_secondary))
                        .child(format!("👁 {}", format_compact_count(post.views_count as i64))),
                ),
        )
}

/// Renders the `"Why this post?"` explainability popover with signal bars and feedback controls.
pub fn render_explain_popover(
    ws: &CommunityWorkspace,
    post_id: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let item = ws.state.find_feed_item(post_id).cloned();
    let explanation = item.as_ref().and_then(|i| i.explanation.clone());
    let pid_less = post_id.to_string();
    let pid_more = post_id.to_string();
    let comm_slug = item.as_ref().and_then(|i| i.post.community_slug.clone());
    let author_id = item.as_ref().map(|i| i.post.author_actor_id.clone());

    let primary_reason = explanation
        .as_ref()
        .map(|e| e.primary_reason.clone())
        .unwrap_or_else(|| {
            tr_str(lang, "基于你的兴趣、互动质量与多样性平衡推荐", "Ranked by relevance, quality & diversity")
                .to_string()
        });
    let signals = explanation
        .map(|e| e.signal_breakdown)
        .unwrap_or_default();

    macos_popover_surface("community-explain-popover", pal)
        .w(px(pal.scaled_px(320.0)))
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(9.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
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
                        .child(tg_icon(IconName::SparkleAlgo, 15.0, pal.accent))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(tr_str(lang, "推荐算法透明解释", "Why this post?")),
                        ),
                )
                .child(
                    div()
                        .id("explain-close-btn")
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.active_popover = None;
                                cx.notify();
                            }),
                        )
                        .child(tg_icon(IconName::Close, 13.0, pal.text_secondary)),
                ),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(pal.text_secondary))
                .child(primary_reason),
        )
        .when(!signals.is_empty(), |col| {
            col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(signals.into_iter().map(|sig| {
                        let pct = if sig.contribution_pct.is_finite() {
                            sig.contribution_pct.clamp(0.0, 100.0)
                        } else {
                            0.0
                        };
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_size(px(11.0))
                                    .child(
                                        div()
                                            .text_color(rgb(pal.text_primary))
                                            .child(sig.label),
                                    )
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.accent))
                                            .child(format!("{:.0}%", pct)),
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .h(px(5.0))
                                    .rounded_full()
                                    .bg(pal.service_pill_rgba())
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative((pct as f32 / 100.0).clamp(0.04, 1.0)))
                                            .rounded_full()
                                            .bg(rgb(pal.accent)),
                                    ),
                            )
                    })),
            )
        })
        .child(
            div()
                .pt(px(6.0))
                .border_t_1()
                .border_color(pal.separator_rgba())
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("explain-not-interested-btn")
                        .cursor_pointer()
                        .flex_1()
                        .py(px(6.0))
                        .px(px(8.0))
                        .rounded(px(8.0))
                        .bg(pal.card_surface_rgba())
                        .border_1()
                        .border_color(pal.hairline_border_rgba())
                        .hover(move |s| s.bg(pal.row_hover_rgba()))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.danger))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener({
                                let comm_slug = comm_slug.clone();
                                let author_id = author_id.clone();
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.record_signal(
                                        pid_less.clone(),
                                        EngagementSignalKind::NotInterested,
                                        author_id.clone(),
                                        comm_slug.clone(),
                                        cx,
                                    );
                                }
                            }),
                        )
                        .child(tr_str(lang, "不感兴趣", "Not interested")),
                )
                .child(
                    div()
                        .id("explain-show-more-btn")
                        .cursor_pointer()
                        .flex_1()
                        .py(px(6.0))
                        .px(px(8.0))
                        .rounded(px(8.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.14))
                        .hover(move |s| s.bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.22)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.accent))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.record_signal(
                                    pid_more.clone(),
                                    EngagementSignalKind::ShowMore,
                                    author_id.clone(),
                                    comm_slug.clone(),
                                    cx,
                                );
                            }),
                        )
                        .child(tr_str(lang, "多看类似", "Show more like this")),
                ),
        )
        .into_any_element()
}

/// Renders the quick Award picker popover for a post.
pub fn render_award_popover(
    ws: &CommunityWorkspace,
    post_id: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let awards: [(&str, &str, &str, &str); 4] = [
        ("gold", "🏆", "金星", "Gold"),
        ("insightful", "💡", "洞见", "Insightful"),
        ("helpful", "🤝", "有帮助", "Helpful"),
        ("mindblown", "🤯", "震撼", "Mindblown"),
    ];
    macos_popover_surface("community-award-popover", pal)
        .w(px(260.0))
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(pal.text_primary))
                .child(tr_str(lang, "颁发社区徽章奖励", "Give Community Award")),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.0))
                .children(awards.into_iter().map(|(kind, emoji, zh, en)| {
                    let pid = post_id.to_string();
                    let ak = kind.to_string();
                    div()
                        .id(SharedString::from(format!("award-opt-{kind}")))
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(8.0))
                        .bg(pal.card_surface_rgba())
                        .border_1()
                        .border_color(pal.hairline_border_rgba())
                        .hover(move |s| s.bg(pal.row_hover_rgba()))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .text_size(px(12.0))
                        .text_color(rgb(pal.text_primary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.give_award(pid.clone(), ak.clone(), cx);
                            }),
                        )
                        .child(emoji)
                        .child(tr_str(lang, zh, en))
                })),
        )
        .into_any_element()
}

/// Renders the inline Quote-Post popover allowing users/bots to quote any post with commentary.
pub fn render_quote_popover(
    ws: &CommunityWorkspace,
    post_id: &str,
    cx: &mut Context<CommunityWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.as_str();
    let focused = ws.state.active_input == ActiveCommunityInput::QuotePostNote;
    let preedit = if focused {
        ws.state.ime_preedit.as_deref()
    } else {
        None
    };
    let pid = post_id.to_string();

    macos_popover_surface("community-quote-popover", pal)
        .w(px(340.0))
        .p(px(12.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(pal.text_primary))
                .child(tr_str(lang, "引用转发并发表观点", "Quote Post")),
        )
        .child(
            div()
                .id("quote-post-input")
                .cursor_text()
                .w_full()
                .min_h(px(64.0))
                .p(px(8.0))
                .rounded(px(8.0))
                .bg(rgb(pal.input_bg))
                .border_1()
                .border_color(if focused {
                    rgb(pal.accent).into()
                } else {
                    pal.hairline_border_rgba()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.state.focus_input(ActiveCommunityInput::QuotePostNote);
                        cx.notify();
                    }),
                )
                .child(render_input_display(
                    &ws.state.quote_post_draft,
                    preedit,
                    tr_str(lang, "添加你的评论…", "Add your commentary…"),
                    focused,
                    &pal,
                )),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(
                    div()
                        .id("quote-cancel-btn")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .bg(pal.card_surface_rgba())
                        .text_size(px(12.0))
                        .text_color(rgb(pal.text_secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.state.active_popover = None;
                                this.state.blur_input();
                                cx.notify();
                            }),
                        )
                        .child(tr_str(lang, "取消", "Cancel")),
                )
                .child(
                    div()
                        .id("quote-submit-btn")
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .bg(rgb(pal.accent))
                        .hover(move |s| s.bg(rgb(pal.accent_hover)))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.active_button_fg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.submit_quote_post(pid.clone(), cx);
                            }),
                        )
                        .child(tr_str(lang, "发布引用", "Publish Quote")),
                ),
        )
        .into_any_element()
}
