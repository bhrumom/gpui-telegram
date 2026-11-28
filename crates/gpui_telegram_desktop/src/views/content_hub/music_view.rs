//! Apple Music macOS section (`MusicHubView`): Home, New, Radio, Library, My Music,
//! Top Charts tracklist, square 1:1 album grid, Album Detail with Tracklist & Synced Lyrics,
//! and instant playback wired to `MusicTransportEngine` + `MusicPlayerBar`.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_media::media::audio::MusicPlaybackState;
use gpui_telegram_proto::creations::{
    CatalogSortMode, CreationKind, CreationPayload, CreationReviewStatus, MusicPayload,
    PublicCatalogItemDto, UpsertCreationStateRequest, UserCreationDto,
};

use crate::i18n::tr_str;
use crate::theme::{IconName, TdesktopThemePalette};
use crate::views::content_hub::apple_primitives::{
    apple_grouped_card, apple_shelf_header, cover_artwork_box, empty_state_card_shell,
    filter_pill, format_duration_ms, format_duration_secs, hero_banner,
    openbot_marketplace_nav_item, openbot_marketplace_shell, openbot_page_header,
    openbot_primary_cta_btn, openbot_secondary_btn, rating_pill, render_editable_field_box,
    review_status_badge, OPENBOT_MARKETPLACE_CONTENT_MAX_WIDTH,
};
use crate::views::content_hub::music_player_bar::MUSIC_ACCENT;
use crate::views::content_hub::{
    render_community_reviews_panel, ContentHubEvent, ContentHubInputTarget, ContentHubSection,
    ContentHubWorkspace,
};

/// Canonical Apple Music sub-navigation tabs.
pub const MUSIC_SUB_TABS: &[(&str, &str, &str)] = &[
    ("home", "主页", "Home"),
    ("new", "发现", "New"),
    ("radio", "电台", "Radio"),
    ("library", "资料库", "Library"),
];

pub fn normalize_music_sub_tab(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "home" | "listen_now" | "for_you" | "my_music" | "my_creations" | "mine" => "home",
        "new" | "browse" | "discover" | "charts" => "new",
        "radio" => "radio",
        "library" | "loved" | "saved" => "library",
        _ => "home",
    }
}

fn extract_music_payload(creation: &UserCreationDto) -> MusicPayload {
    match &creation.payload {
        CreationPayload::Music(mp) => mp.clone(),
        _ => MusicPayload {
            artist: creation.author_display_name.clone(),
            album_title: creation.title.clone(),
            release_year: 2026,
            primary_genre: creation
                .genres
                .first()
                .cloned()
                .unwrap_or_else(|| "Electronic".to_string()),
            mood_tags: Vec::new(),
            bpm: 112,
            musical_key: "C Major".to_string(),
            time_signature: "4/4".to_string(),
            total_duration_secs: 60,
            tracks: Vec::new(),
        },
    }
}

fn catalog_item_to_music_creation(item: &PublicCatalogItemDto) -> UserCreationDto {
    let payload = item.payload.clone().unwrap_or_else(|| {
        CreationPayload::Music(MusicPayload {
            artist: item.author_display_name.clone(),
            album_title: item.title.clone(),
            release_year: 2026,
            primary_genre: item
                .genres
                .first()
                .cloned()
                .unwrap_or_else(|| "Electronic".to_string()),
            mood_tags: item.tags.clone(),
            bpm: 115,
            musical_key: "C Major".to_string(),
            time_signature: "4/4".to_string(),
            total_duration_secs: 45,
            tracks: Vec::new(),
        })
    });
    UserCreationDto {
        creation_id: item.creation_id.clone(),
        owner_user_id: item.owner_user_id,
        creator_bot_user_id: None,
        creator_chat_id: None,
        kind: CreationKind::Music,
        title: item.title.clone(),
        subtitle: item.subtitle.clone(),
        summary: item.summary.clone(),
        author_display_name: item.author_display_name.clone(),
        language: item.language.clone(),
        genres: item.genres.clone(),
        tags: item.tags.clone(),
        cover: item.cover.clone(),
        age_rating: item.age_rating.clone(),
        payload,
        version: item.published_version,
        cloud_revision: 0,
        review_status: CreationReviewStatus::Published,
        visibility: gpui_telegram_proto::creations::CreationVisibility::Public,
        has_live_published_version: true,
        published_version: Some(item.published_version),
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: None,
        reviewed_at_ms: None,
        published_at_ms: Some(item.published_at_ms),
        created_at_ms: item.published_at_ms,
        updated_at_ms: item.updated_at_ms,
        deleted_at_ms: None,
        rating_avg: item.rating_avg,
        rating_count: item.rating_count,
        engagement_count: item.engagement_count,
        save_or_install_count: item.save_or_install_count,
    }
}

fn render_square_album_card(
    item: &PublicCatalogItemDto,
    is_now_playing: bool,
    is_loved: bool,
    pal: TdesktopThemePalette,
    cx: &mut Context<ContentHubWorkspace>,
) -> Stateful<Div> {
    let ob = pal.openbot_palette();
    let card_hover = ob.bg_control_hover();
    let cid = item.creation_id.clone();
    let item_clone = item.clone();
    let item_play = item.clone();
    div()
        .id(SharedString::from(format!("music-album-card-{cid}")))
        .cursor_pointer()
        .w(px(168.0))
        .rounded(px(14.0))
        .bg(ob.bg_surface())
        .hover(move |s| s.bg(card_hover))
        .border_1()
        .border_color(if is_now_playing {
            ob.border_strong_hairline()
        } else {
            ob.frame_card_border()
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
                    section: ContentHubSection::Music,
                    creation_id: cid.clone(),
                    action: Some("open".to_string()),
                });
                cx.notify();
            }),
        )
        // 1:1 album square with OpenBot monochrome floating quick-play button
        .child(
            div()
                .relative()
                .w(px(148.0))
                .h(px(148.0))
                .child(cover_artwork_box(&item.cover, 148.0, 148.0, 10.0, 0x4E3840))
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "music-album-quickplay-{}",
                            item.creation_id
                        )))
                        .cursor_pointer()
                        .absolute()
                        .bottom(px(8.0))
                        .right(px(8.0))
                        .w(px(32.0))
                        .h(px(32.0))
                        .rounded(px(999.0))
                        .bg(ob.surface_light())
                        .hover(|s| s.opacity(0.84))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(ob.text_on_light())
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(ContentHubEvent::PlayMusicCreation {
                                    creation: catalog_item_to_music_creation(&item_play),
                                    track_index: 0,
                                    shuffle: false,
                                });
                            }),
                        )
                        .child(if is_now_playing { "♫" } else { "▶" }),
                )
                .when(is_loved, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(7.0))
                            .left(px(7.0))
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(999.0))
                            .bg(TdesktopThemePalette::rgba_with_alpha(0x000000, 0.62))
                            .text_size(px(10.0))
                            .text_color(rgb(0xFFFFFF))
                            .child("♥"),
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
                        .text_color(ob.text_primary())
                        .truncate()
                        .child(item.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(ob.text_muted())
                        .truncate()
                        .child(item.author_display_name.clone()),
                )
                .child(
                    div()
                        .pt(px(2.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(rating_pill(item.rating_avg, item.rating_count, pal)),
                ),
        )
}

fn render_music_album_detail(
    ws: &mut ContentHubWorkspace,
    detail: UserCreationDto,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let ob = pal.openbot_palette();
    let lang = ws.language_id.clone();
    let payload = extract_music_payload(&detail);
    let cid = detail.creation_id.clone();
    let saved_state = ws.my_creation_states.get(&cid).cloned();
    let in_library = saved_state.as_ref().is_some_and(|s| s.in_library);
    let is_loved = saved_state.as_ref().is_some_and(|s| s.favorite);
    let is_own = ws.find_my_creation(&cid).is_some();
    let is_published = detail.review_status == CreationReviewStatus::Published;

    let current_playing_track_id = ws
        .music_engine
        .current_track()
        .filter(|t| t.creation_id.as_deref() == Some(cid.as_str()))
        .map(|t| t.track_id.clone());
    let is_playing_now = ws.music_engine.state() == MusicPlaybackState::Playing;
    let active_lyric_idx = ws.music_engine.active_lyric_index();

    let detail_play = detail.clone();
    let detail_shuffle = detail.clone();
    let cid_lib = cid.clone();
    let cid_love = cid.clone();

    let artist = if payload.artist.trim().is_empty() {
        detail.author_display_name.clone()
    } else {
        payload.artist.clone()
    };

    let displayed_lyrics = ws
        .music_engine
        .current_track()
        .filter(|t| t.creation_id.as_deref() == Some(cid.as_str()))
        .map(|t| t.lyrics.clone())
        .filter(|l| !l.is_empty())
        .or_else(|| {
            payload.tracks.first().map(|tr| {
                tr.lyrics
                    .iter()
                    .map(|l| gpui_telegram_media::media::audio::SynthLyricLine {
                        timestamp_ms: l.timestamp_ms,
                        text: l.text.clone(),
                    })
                    .collect()
            })
        })
        .unwrap_or_default();

    div()
        .id("apple-music-detail-panel")
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
                        "music-detail-back-btn",
                        tr_str(&lang, "← 返回音乐", "← Back to Music"),
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
        // Album Header
        .child(
            apple_grouped_card(pal)
                .p(px(20.0))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(20.0))
                .child(cover_artwork_box(&detail.cover, 160.0, 160.0, 12.0, 0x4E3840))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(250.0))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(22.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.text_primary())
                                .child(detail.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(14.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ob.text_secondary())
                                .child(artist),
                        )
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
                                        .text_color(ob.text_muted())
                                        .child(format!(
                                            "{} · {} BPM · {} · {} {} · {}",
                                            payload.primary_genre,
                                            payload.bpm,
                                            payload.musical_key,
                                            payload.tracks.len().max(1),
                                            tr_str(&lang, "首", "tracks"),
                                            format_duration_secs(u64::from(
                                                payload.total_duration_secs.max(1)
                                            ))
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(ob.text_secondary())
                                .child(detail.summary.clone()),
                        )
                        .child(
                            div()
                                .pt(px(8.0))
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    openbot_primary_cta_btn(
                                        "music-detail-play-btn",
                                        tr_str(&lang, "▶ 播放", "▶ Play"),
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::PlayMusicCreation {
                                                creation: detail_play.clone(),
                                                track_index: 0,
                                                shuffle: false,
                                            });
                                        }),
                                    ),
                                )
                                .child(
                                    openbot_secondary_btn(
                                        "music-detail-shuffle-btn",
                                        tr_str(&lang, "⇄ 随机播放", "⇄ Shuffle"),
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::PlayMusicCreation {
                                                creation: detail_shuffle.clone(),
                                                track_index: 0,
                                                shuffle: true,
                                            });
                                        }),
                                    ),
                                )
                                .child(
                                    openbot_secondary_btn(
                                        "music-detail-library-btn",
                                        if in_library {
                                            tr_str(&lang, "✓ 已在资料库", "✓ In Library")
                                        } else {
                                            tr_str(&lang, "+ 资料库", "+ Add")
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
                                .child(
                                    openbot_secondary_btn(
                                        "music-detail-love-btn",
                                        if is_loved {
                                            tr_str(&lang, "♥ 已喜爱", "♥ Loved")
                                        } else {
                                            tr_str(&lang, "♡ 喜爱", "♡ Love")
                                        },
                                        pal,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::UpsertCreationState {
                                                creation_id: cid_love.clone(),
                                                req: UpsertCreationStateRequest {
                                                    favorite: Some(!is_loved),
                                                    ..Default::default()
                                                },
                                            });
                                        }),
                                    ),
                                ),
                        ),
                ),
        )
        // Tracklist Table
        .child(
            apple_grouped_card(pal)
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(apple_shelf_header(
                    tr_str(&lang, "曲目列表", "Tracks"),
                    Some(format!(
                        "{} {}",
                        payload.tracks.len().max(1),
                        tr_str(&lang, "首", "songs")
                    )),
                    pal,
                ))
                .children(payload.tracks.iter().enumerate().map(|(idx, tr)| {
                    let is_active =
                        current_playing_track_id.as_deref() == Some(tr.track_id.as_str());
                    let detail_row = detail.clone();
                    let hover_bg = ob.bg_control_hover();
                    div()
                        .id(SharedString::from(format!("music-track-row-{idx}")))
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(9.0))
                        .rounded(px(8.0))
                        .bg(if is_active {
                            ob.bg_control_active()
                        } else {
                            rgba(0x00000000)
                        })
                        .hover(move |s| s.bg(hover_bg))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _, _, cx| {
                                cx.emit(ContentHubEvent::PlayMusicCreation {
                                    creation: detail_row.clone(),
                                    track_index: idx,
                                    shuffle: false,
                                });
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .w(px(22.0))
                                        .flex_shrink_0()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(if is_active {
                                            ob.text_primary()
                                        } else {
                                            ob.text_muted()
                                        })
                                        .child(if is_active && is_playing_now {
                                            "♫".to_string()
                                        } else {
                                            format!("{}", tr.track_number.max((idx + 1) as u32))
                                        }),
                                )
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
                                                .font_weight(if is_active {
                                                    FontWeight::SEMIBOLD
                                                } else {
                                                    FontWeight::MEDIUM
                                                })
                                                .text_color(ob.text_primary())
                                                .child(tr.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(11.0))
                                                .text_color(ob.text_muted())
                                                .child(format!(
                                                    "{} BPM · {} · {} notes",
                                                    tr.bpm,
                                                    tr.musical_key,
                                                    tr.melody_notes.len()
                                                )),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(12.0))
                                .text_color(ob.text_muted())
                                .child(format_duration_secs(u64::from(tr.duration_secs))),
                        )
                })),
        )
        // Inline Synced Lyrics card
        .when(!displayed_lyrics.is_empty(), |el| {
            el.child(
                apple_grouped_card(pal)
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(apple_shelf_header(
                        tr_str(&lang, "同步歌词", "Synced Lyrics"),
                        Some(format!(
                            "{} {}",
                            displayed_lyrics.len(),
                            tr_str(&lang, "句", "lines")
                        )),
                        pal,
                    ))
                    .children(displayed_lyrics.iter().enumerate().map(|(idx, line)| {
                        let is_cur = current_playing_track_id.is_some()
                            && active_lyric_idx == Some(idx);
                        let ts_ms = u64::from(line.timestamp_ms);
                        let hover_bg = ob.bg_control_hover();
                        div()
                            .id(SharedString::from(format!("music-detail-lyric-{idx}")))
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(if is_cur {
                                ob.bg_control_active()
                            } else {
                                rgba(0x00000000)
                            })
                            .hover(move |s| s.bg(hover_bg))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    let _ = this.music_engine.seek_ms(ts_ms);
                                    cx.emit(ContentHubEvent::MusicTransportChanged);
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .w(px(38.0))
                                    .text_size(px(10.5))
                                    .text_color(ob.text_muted())
                                    .child(format_duration_ms(ts_ms)),
                            )
                            .child(
                                div()
                                    .text_size(px(if is_cur { 14.0 } else { 12.5 }))
                                    .font_weight(if is_cur {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(if is_cur {
                                        ob.text_primary()
                                    } else {
                                        ob.text_secondary()
                                    })
                                    .child(line.text.clone()),
                            )
                    })),
            )
        })
        // Community Ratings & Reviews
        .child(render_community_reviews_panel(
            ws,
            &cid,
            is_published,
            MUSIC_ACCENT,
            cx,
        ))
        .into_any_element()
}

/// Renders the Apple Music vertical hub (`MusicHubView`).
pub fn render_music_hub_view(
    ws: &mut ContentHubWorkspace,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.clone();
    let active_tab = normalize_music_sub_tab(&ws.active_sub_tab);
    let search_focused = ws.active_input == ContentHubInputTarget::CatalogSearch;
    let search_field = ws.catalog_search.clone();
    let sort_mode = ws.sort_mode;
    let selected_genre = ws.selected_genre_filter.clone();
    let now_playing_cid = ws
        .music_engine
        .current_track()
        .and_then(|t| t.creation_id.clone());

    let all_music = ws.active_catalog_items(ContentHubSection::Music);
    let mut genres: Vec<String> = Vec::new();
    for item in &ws.music_catalog {
        for g in &item.genres {
            if !g.trim().is_empty() && !genres.iter().any(|x| x.eq_ignore_ascii_case(g)) {
                genres.push(g.clone());
            }
        }
    }

    let filtered_music: Vec<PublicCatalogItemDto> = all_music
        .into_iter()
        .filter(|m| {
            selected_genre
                .as_deref()
                .is_none_or(|g| m.genres.iter().any(|mg| mg.eq_ignore_ascii_case(g)))
        })
        .collect();

    let library_music: Vec<PublicCatalogItemDto> = filtered_music
        .iter()
        .filter(|m| {
            ws.my_creation_states
                .get(&m.creation_id)
                .is_some_and(|st| st.in_library || st.favorite)
        })
        .cloned()
        .collect();

    let selected_detail = ws.resolve_selected_detail_creation();
    let sort_options: &[(CatalogSortMode, &'static str, &'static str, &'static str)] = &[
        (CatalogSortMode::Featured, "music-sort-featured", "精选", "Featured"),
        (CatalogSortMode::Popular, "music-sort-popular", "热门", "Popular"),
        (CatalogSortMode::TopRated, "music-sort-rated", "高分", "Top Rated"),
        (CatalogSortMode::Newest, "music-sort-newest", "最新", "Newest"),
    ];

    let radio_stations: &[(&str, &str, &str, Option<&str>)] = &[
        (
            "station-all",
            "Creator Studio 精选电台",
            "Creator Studio Live Radio",
            None,
        ),
        (
            "station-electronic",
            "合成器与电子律动",
            "Synthwave & Electronic",
            Some("electronic"),
        ),
        (
            "station-chill",
            "静谧 Lo-Fi 专注电台",
            "Chill & Focus Beats",
            Some("chill"),
        ),
        (
            "station-acoustic",
            "钢琴与原声创作",
            "Acoustic & Piano Works",
            Some("classical"),
        ),
    ];

    let ob = pal.openbot_palette();
    let root_label = tr_str(&lang, "音乐", "Music");
    let hover_crumb = ob.bg_control();
    let text_pri = ob.text_primary();
    let header = if let Some(ref detail) = selected_detail {
        let parent_btn = div()
            .id("music-header-parent-crumb")
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
            "music-refresh-btn",
            tr_str(&lang, "刷新", "Refresh"),
            pal,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_this, _, _, cx| {
                cx.emit(ContentHubEvent::RefreshCatalog(ContentHubSection::Music));
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
                tr_str(&lang, "搜索专辑、艺人、曲风…", "Search Music…"),
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
                .children(MUSIC_SUB_TABS.iter().map(|&(slug, zh, en)| {
                    let count = match slug {
                        "library" => Some(library_music.len()),
                        "home" | "new" => Some(filtered_music.len()),
                        "radio" => Some(radio_stations.len()),
                        _ => None,
                    };
                    openbot_marketplace_nav_item(
                        SharedString::from(format!("music-tab-{slug}")),
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
                        .child(tr_str(&lang, "曲风", "Genres")),
                )
                .child(
                    openbot_marketplace_nav_item(
                        "music-genre-all",
                        tr_str(&lang, "全部曲风", "All Genres"),
                        selected_genre.is_none(),
                        Some(ws.music_catalog.len()),
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
                        SharedString::from(format!("music-genre-{g_val}")),
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
                                            ContentHubSection::Music,
                                        ));
                                        cx.notify();
                                    }),
                                )
                        })),
                ),
        );

    let body_scroll = div().id("apple-music-hub-scroll").child(
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
                el.child(render_music_album_detail(ws, detail, cx))
            })
            .when(selected_detail.is_none() && active_tab == "radio", |el| {
                let radio_hover = ob.bg_control_hover();
                el.child(hero_banner(
                    "apple-music-radio-hero",
                    tr_str(&lang, "音乐电台", "MUSIC RADIO"),
                    tr_str(&lang, "创作者精选无间断电台", "Live Creator Radio Stations"),
                    tr_str(
                        &lang,
                        "点击任意电台即刻合成并随机播放在线公开曲库。",
                        "Launch an instant synthesized mix built from published community tracks.",
                    ),
                    "#212121",
                    "#1A1A1A",
                    IconName::HubMusic,
                    pal,
                ))
                .child(div().flex().flex_wrap().gap(px(14.0)).children(
                    radio_stations.iter().map(|&(sid, zh, en, genre_opt)| {
                        let gf = genre_opt.map(str::to_string);
                        div()
                            .id(SharedString::from(format!("music-radio-{sid}")))
                            .cursor_pointer()
                            .w(px(260.0))
                            .max_w_full()
                            .rounded(px(14.0))
                            .p(px(18.0))
                            .bg(ob.bg_surface())
                            .border_1()
                            .border_color(ob.border_strong_hairline())
                            .hover(move |s| s.bg(radio_hover))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |_this, _, _, cx| {
                                    cx.emit(ContentHubEvent::PlayMusicRadio {
                                        genre_filter: gf.clone(),
                                    });
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_muted())
                                    .child("LIVE · RADIO"),
                            )
                            .child(
                                div()
                                    .text_size(px(15.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ob.text_primary())
                                    .child(tr_str(&lang, zh, en)),
                            )
                            .child(
                                div()
                                    .pt(px(4.0))
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_secondary())
                                    .child(tr_str(&lang, "▶ 立即收听", "▶ Listen Now")),
                            )
                    }),
                ))
            })
            .when(selected_detail.is_none() && active_tab != "radio", |el| {
                let list = if active_tab == "library" {
                    library_music.clone()
                } else {
                    filtered_music.clone()
                };
                el.child(hero_banner(
                    "apple-music-hero-banner",
                    tr_str(&lang, "音乐 · 原创合成", "MUSIC · SYNTH STUDIO"),
                    tr_str(&lang, "聆听社区原创专辑与精选榜单", "Top Picks & Original Albums"),
                    tr_str(
                        &lang,
                        "内置纯 Rust 音频合成器、波形进度条、同步歌词与待播队列。",
                        "Pure-Rust WAV synthesis, interactive waveform scrubber, synced lyrics & Up Next queue.",
                    ),
                    "#212121",
                    "#1A1A1A",
                    IconName::HubMusic,
                    pal,
                ))
                .child(apple_shelf_header(
                    if active_tab == "library" {
                        tr_str(&lang, "我的喜爱与资料库", "Loved & Library")
                    } else if active_tab == "new" {
                        tr_str(&lang, "最新上架专辑", "New Releases")
                    } else {
                        tr_str(&lang, "为你推荐", "Top Picks for You")
                    },
                    Some(format!(
                        "{} {}",
                        list.len(),
                        tr_str(&lang, "张专辑", "albums")
                    )),
                    pal,
                ))
                .when(list.is_empty(), |inner| {
                    let cta = openbot_secondary_btn(
                        "music-empty-refresh-btn",
                        tr_str(&lang, "刷新曲库", "Refresh Music"),
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _, _, cx| {
                            cx.emit(ContentHubEvent::RefreshCatalog(ContentHubSection::Music));
                        }),
                    );
                    inner.child(empty_state_card_shell(
                        IconName::HubMusic,
                        tr_str(&lang, "暂无公开上架音乐", "No Published Music Yet"),
                        tr_str(
                            &lang,
                            "在「消息」中直接与 @CreatorStudioBot 对话即可创作并发布音乐，审核上架后会展示在这里。",
                            "Chat with @CreatorStudioBot directly in Messages to compose and publish music. Published albums appear here.",
                        ),
                        cta,
                        pal,
                    ))
                })
                .when(!list.is_empty(), |inner| {
                    inner
                        .child(div().flex().flex_wrap().gap(px(14.0)).children(
                            list.iter().map(|item| {
                                let is_np = now_playing_cid.as_deref()
                                    == Some(item.creation_id.as_str());
                                let is_loved = ws
                                    .my_creation_states
                                    .get(&item.creation_id)
                                    .is_some_and(|st| st.favorite);
                                render_square_album_card(item, is_np, is_loved, pal, cx)
                            }),
                        ))
                        // Top Charts table below the album shelf
                        .child(
                            apple_grouped_card(pal)
                                .p(px(16.0))
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(apple_shelf_header(
                                    tr_str(&lang, "热门排行榜", "Top Charts"),
                                    Some(format!("Top {}", list.len().min(10))),
                                    pal,
                                ))
                                .children(list.iter().take(10).enumerate().map(|(rank, item)| {
                                    let item_play = item.clone();
                                    let is_np = now_playing_cid.as_deref()
                                        == Some(item.creation_id.as_str());
                                    let hover_bg = ob.bg_control_hover();
                                    div()
                                        .id(SharedString::from(format!("music-chart-row-{rank}")))
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(8.0))
                                        .rounded(px(10.0))
                                        .bg(if is_np {
                                            ob.bg_control_active()
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .hover(move |s| s.bg(hover_bg))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(10.0))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |_this, _, _, cx| {
                                                cx.emit(ContentHubEvent::PlayMusicCreation {
                                                    creation: catalog_item_to_music_creation(
                                                        &item_play,
                                                    ),
                                                    track_index: 0,
                                                    shuffle: false,
                                                });
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
                                                        .w(px(22.0))
                                                        .flex_shrink_0()
                                                        .text_size(px(13.0))
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
                                                    36.0,
                                                    36.0,
                                                    6.0,
                                                    0x4E3840,
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
                                                                .child(
                                                                    item.author_display_name
                                                                        .clone(),
                                                                ),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .flex()
                                                .items_center()
                                                .gap(px(10.0))
                                                .child(rating_pill(
                                                    item.rating_avg,
                                                    item.rating_count,
                                                    pal,
                                                ))
                                                .child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(ob.text_primary())
                                                        .child(if is_np { "♫" } else { "▶" }),
                                                ),
                                        )
                                })),
                        )
                })
            }),
    );

    div()
        .id("apple-music-hub-root")
        .size_full()
        .bg(ob.bg_canvas())
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(header)
        .child(openbot_marketplace_shell(left_nav, body_scroll, pal))
        .into_any_element()
}
