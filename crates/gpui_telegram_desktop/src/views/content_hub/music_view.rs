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
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::{
    macos_primary_pill_btn, macos_secondary_pill_btn,
};
use crate::views::content_hub::apple_primitives::{
    apple_grouped_card, apple_shelf_header, cover_artwork_box, empty_state_card_shell,
    filter_pill, format_duration_ms, format_duration_secs, hero_banner, rating_pill,
    render_editable_field_box, review_status_badge, vertical_sub_tab_pill,
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
    ("my_music", "我的音乐", "My Music"),
];

pub fn normalize_music_sub_tab(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "home" | "listen_now" | "for_you" => "home",
        "new" | "browse" | "discover" | "charts" => "new",
        "radio" => "radio",
        "library" | "loved" | "saved" => "library",
        "my_music" | "my_creations" | "mine" => "my_music",
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
    let cid = item.creation_id.clone();
    let item_clone = item.clone();
    let item_play = item.clone();
    div()
        .id(SharedString::from(format!("music-album-card-{cid}")))
        .cursor_pointer()
        .w(px(168.0))
        .rounded(px(12.0))
        .bg(pal.card_surface_rgba())
        .border_1()
        .border_color(if is_now_playing {
            TdesktopThemePalette::rgba_with_alpha(MUSIC_ACCENT, 0.85)
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
                    section: ContentHubSection::Music,
                    creation_id: cid.clone(),
                    action: Some("open".to_string()),
                });
                cx.notify();
            }),
        )
        // Classic Apple Music 1:1 album square with floating play pill
        .child(
            div()
                .relative()
                .w(px(148.0))
                .h(px(148.0))
                .child(cover_artwork_box(&item.cover, 148.0, 148.0, 10.0, MUSIC_ACCENT))
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
                        .bg(rgb(MUSIC_ACCENT))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
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
                            .bg(TdesktopThemePalette::rgba_with_alpha(0x000000, 0.55))
                            .text_size(px(10.0))
                            .text_color(rgb(MUSIC_ACCENT))
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
    let detail_pub = detail.clone();
    let cid_lib = cid.clone();
    let cid_love = cid.clone();

    let artist = if payload.artist.trim().is_empty() {
        detail.author_display_name.clone()
    } else {
        payload.artist.clone()
    };

    // Active track lyrics for inline Apple Music Lyrics card
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
                    macos_secondary_pill_btn(
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
        // Apple Music Album Header
        .child(
            apple_grouped_card(pal)
                .p(px(20.0))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(20.0))
                .child(cover_artwork_box(&detail.cover, 160.0, 160.0, 12.0, MUSIC_ACCENT))
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
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(pal.text_primary))
                                .child(detail.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(MUSIC_ACCENT))
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
                                        .text_color(rgb(pal.text_secondary))
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
                                .text_color(rgb(pal.text_secondary))
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
                                    div()
                                        .id("music-detail-play-btn")
                                        .cursor_pointer()
                                        .px(px(20.0))
                                        .py(px(8.0))
                                        .rounded(px(999.0))
                                        .bg(rgb(MUSIC_ACCENT))
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |_this, _, _, cx| {
                                                cx.emit(ContentHubEvent::PlayMusicCreation {
                                                    creation: detail_play.clone(),
                                                    track_index: 0,
                                                    shuffle: false,
                                                });
                                            }),
                                        )
                                        .child(tr_str(&lang, "▶ 播放", "▶ Play")),
                                )
                                .child(
                                    macos_secondary_pill_btn(
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
                                    macos_secondary_pill_btn(
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
                                    macos_secondary_pill_btn(
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
                                )
                                .when(is_own, |el| {
                                    el.child(
                                        macos_secondary_pill_btn(
                                            "music-detail-publish-btn",
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
        // Tracklist Table (Apple Music style)
        .child(
            apple_grouped_card(pal)
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
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
                    let hover_bg = pal.row_hover_rgba();
                    div()
                        .id(SharedString::from(format!("music-track-row-{idx}")))
                        .cursor_pointer()
                        .px(px(12.0))
                        .py(px(9.0))
                        .rounded(px(8.0))
                        .bg(if is_active {
                            TdesktopThemePalette::rgba_with_alpha(MUSIC_ACCENT, 0.14)
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
                                .flex()
                                .items_center()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .w(px(22.0))
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(if is_active {
                                            MUSIC_ACCENT
                                        } else {
                                            pal.text_secondary
                                        }))
                                        .child(if is_active && is_playing_now {
                                            "♫".to_string()
                                        } else {
                                            format!("{}", tr.track_number.max((idx + 1) as u32))
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(if is_active {
                                                    MUSIC_ACCENT
                                                } else {
                                                    pal.text_primary
                                                }))
                                                .child(tr.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(rgb(pal.text_secondary))
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
                                .text_size(px(12.0))
                                .text_color(rgb(pal.text_secondary))
                                .child(format_duration_secs(u64::from(tr.duration_secs))),
                        )
                })),
        )
        // Inline Apple Music Synced Lyrics card
        .when(!displayed_lyrics.is_empty(), |el| {
            el.child(
                apple_grouped_card(pal)
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
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
                        div()
                            .id(SharedString::from(format!("music-detail-lyric-{idx}")))
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(5.0))
                            .rounded(px(7.0))
                            .bg(if is_cur {
                                TdesktopThemePalette::rgba_with_alpha(MUSIC_ACCENT, 0.16)
                            } else {
                                rgba(0x00000000)
                            })
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
                                    .text_size(px(10.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .child(format_duration_ms(ts_ms)),
                            )
                            .child(
                                div()
                                    .text_size(px(if is_cur { 14.0 } else { 12.5 }))
                                    .font_weight(if is_cur {
                                        FontWeight::BOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(rgb(if is_cur {
                                        MUSIC_ACCENT
                                    } else {
                                        pal.text_primary
                                    }))
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

    let my_music: Vec<UserCreationDto> = ws
        .my_creations
        .iter()
        .filter(|c| c.deleted_at_ms.is_none() && c.kind == CreationKind::Music)
        .cloned()
        .collect();

    let selected_detail = ws.resolve_selected_detail_creation();
    let sort_options: &[(CatalogSortMode, &'static str, &'static str, &'static str)] = &[
        (CatalogSortMode::Featured, "music-sort-featured", "精选", "Featured"),
        (CatalogSortMode::Popular, "music-sort-popular", "热门", "Popular"),
        (CatalogSortMode::TopRated, "music-sort-rated", "高分", "Top Rated"),
        (CatalogSortMode::Newest, "music-sort-newest", "最新", "Newest"),
    ];

    let radio_stations: &[(&str, &str, &str, Option<&str>, &str, &str)] = &[
        (
            "station-all",
            "Creator Studio 精选电台",
            "Creator Studio Live Radio",
            None,
            "#FA2D48",
            "#5E17EB",
        ),
        (
            "station-electronic",
            "合成器与电子律动",
            "Synthwave & Electronic",
            Some("electronic"),
            "#007AFF",
            "#5856D6",
        ),
        (
            "station-chill",
            "静谧 Lo-Fi 专注电台",
            "Chill & Focus Beats",
            Some("chill"),
            "#FF9500",
            "#FA2D48",
        ),
        (
            "station-acoustic",
            "钢琴与原声创作",
            "Acoustic & Piano Works",
            Some("classical"),
            "#30D158",
            "#0A84FF",
        ),
    ];

    div()
        .id("apple-music-hub-root")
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
                                .child(tg_icon(IconName::HubMusic, 22.0, MUSIC_ACCENT))
                                .child(
                                    div()
                                        .text_size(px(22.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .child(tr_str(&lang, "音乐", "Music")),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(6.0))
                                .children(MUSIC_SUB_TABS.iter().map(|&(slug, zh, en)| {
                                    vertical_sub_tab_pill(
                                        SharedString::from(format!("music-tab-{slug}")),
                                        tr_str(&lang, zh, en),
                                        active_tab == slug,
                                        MUSIC_ACCENT,
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
                                            tr_str(&lang, "搜索专辑、艺人、曲风…", "Search Music…"),
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
                                        "music-create-bot-btn",
                                        tr_str(&lang, "+ Bot 作曲", "+ Compose"),
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
                                ),
                        ),
                )
                .when(
                    selected_detail.is_none() && active_tab != "my_music" && active_tab != "radio",
                    |el| {
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
                                                "music-genre-all",
                                                tr_str(&lang, "全部曲风", "All Genres"),
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
                                                SharedString::from(format!("music-genre-{g_val}")),
                                                g_val.clone(),
                                                active,
                                                pal,
                                            )
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.selected_genre_filter =
                                                        Some(g_val.clone());
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
                                            filter_pill(
                                                id,
                                                tr_str(&lang, zh, en),
                                                sort_mode == mode,
                                                pal,
                                            )
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
                        )
                    },
                ),
        )
        // Main Scrollable Body
        .child(
            div()
                .id("apple-music-hub-scroll")
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_y_scroll()
                .p(px(24.0))
                .flex()
                .flex_col()
                .gap(px(20.0))
                .when_some(selected_detail.clone(), |el, detail| {
                    el.child(render_music_album_detail(ws, detail, cx))
                })
                .when(selected_detail.is_none() && active_tab == "radio", |el| {
                    el.child(hero_banner(
                        "apple-music-radio-hero",
                        tr_str(&lang, "APPLE MUSIC 电台", "APPLE MUSIC RADIO"),
                        tr_str(&lang, "创作者精选无间断电台", "Live Creator Radio Stations"),
                        tr_str(
                            &lang,
                            "点击任意电台即刻合成并随机播放在线公开曲库。",
                            "Launch an instant synthesized mix built from published community tracks.",
                        ),
                        "#FA2D48",
                        "#7C3AED",
                        IconName::HubMusic,
                        pal,
                    ))
                    .child(div().flex().flex_wrap().gap(px(14.0)).children(
                        radio_stations.iter().map(|&(sid, zh, en, genre_opt, c1, c2)| {
                            let gf = genre_opt.map(str::to_string);
                            div()
                                .id(SharedString::from(format!("music-radio-{sid}")))
                                .cursor_pointer()
                                .w(px(260.0))
                                .rounded(px(14.0))
                                .p(px(18.0))
                                .bg(linear_gradient(
                                    135.0,
                                    linear_color_stop(
                                        rgb(crate::views::content_hub::apple_primitives::parse_hex_color(c1, MUSIC_ACCENT)),
                                        0.0,
                                    ),
                                    linear_color_stop(
                                        rgb(crate::views::content_hub::apple_primitives::parse_hex_color(c2, 0x1C1C1E)),
                                        1.0,
                                    ),
                                ))
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
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(TdesktopThemePalette::rgba_with_alpha(
                                            0xFFFFFF, 0.80,
                                        ))
                                        .child("LIVE · RADIO"),
                                )
                                .child(
                                    div()
                                        .text_size(px(16.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .child(tr_str(&lang, zh, en)),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .child(tr_str(&lang, "▶ 立即收听", "▶ Listen Now")),
                                )
                        }),
                    ))
                })
                .when(selected_detail.is_none() && active_tab == "my_music", |el| {
                    el.child(apple_shelf_header(
                        tr_str(&lang, "我的云端音乐作品", "My Cloud Music"),
                        Some(format!("{} {}", my_music.len(), tr_str(&lang, "张", "works"))),
                        pal,
                    ))
                    .when(my_music.is_empty(), |inner| {
                        let cta = macos_primary_pill_btn(
                            "music-my-empty-cta",
                            tr_str(&lang, "用 @CreatorStudioBot 创作第一首音乐", "Compose Your First Track"),
                            pal,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _, _, cx| {
                                cx.emit(ContentHubEvent::OpenCreatorBot(Some(CreationKind::Music)));
                            }),
                        );
                        inner.child(empty_state_card_shell(
                            IconName::HubMusic,
                            tr_str(&lang, "还没有云端音乐作品", "No Cloud Music Yet"),
                            tr_str(
                                &lang,
                                "使用 @CreatorStudioBot 的 /newmusic 命令创作带旋律、和弦与歌词的音乐作品。",
                                "Use @CreatorStudioBot /newmusic to compose tracks with synthesized WAV audio & lyrics.",
                            ),
                            cta,
                            pal,
                        ))
                    })
                    .when(!my_music.is_empty(), |inner| {
                        inner.child(div().flex().flex_col().gap(px(10.0)).children(
                            my_music.iter().map(|m| {
                                let m_play = m.clone();
                                let m_pub = m.clone();
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
                                                &m.cover,
                                                56.0,
                                                56.0,
                                                8.0,
                                                MUSIC_ACCENT,
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
                                                                    .child(m.title.clone()),
                                                            )
                                                            .child(review_status_badge(
                                                                m.review_status,
                                                                &lang,
                                                                pal,
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .text_color(rgb(pal.text_secondary))
                                                            .child(m.summary.clone()),
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
                                                        "my-music-play-{}",
                                                        m.creation_id
                                                    )),
                                                    tr_str(&lang, "▶ 试听", "▶ Play"),
                                                    pal,
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |_this, _, _, cx| {
                                                        cx.emit(
                                                            ContentHubEvent::PlayMusicCreation {
                                                                creation: m_play.clone(),
                                                                track_index: 0,
                                                                shuffle: false,
                                                            },
                                                        );
                                                    }),
                                                ),
                                            )
                                            .child(
                                                macos_primary_pill_btn(
                                                    SharedString::from(format!(
                                                        "my-music-publish-{}",
                                                        m.creation_id
                                                    )),
                                                    tr_str(&lang, "推送审核", "Publish"),
                                                    pal,
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |_this, _, _, cx| {
                                                        cx.emit(
                                                            ContentHubEvent::OpenPublishModalForCreation(
                                                                m_pub.clone(),
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
                .when(
                    selected_detail.is_none() && active_tab != "my_music" && active_tab != "radio",
                    |el| {
                        let list = if active_tab == "library" {
                            library_music.clone()
                        } else {
                            filtered_music.clone()
                        };
                        el.child(hero_banner(
                            "apple-music-hero-banner",
                            tr_str(&lang, "APPLE MUSIC · 无损合成", "APPLE MUSIC · SPATIAL SYNTH"),
                            tr_str(&lang, "聆听社区原创专辑与精选榜单", "Top Picks & Original Albums"),
                            tr_str(
                                &lang,
                                "内置纯 Rust 音频合成器、5-bit 波形进度条、同步歌词与待播队列。",
                                "Pure-Rust WAV synthesis, interactive waveform scrubber, synced lyrics & Up Next queue.",
                            ),
                            "#FA2D48",
                            "#A21CAF",
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
                            Some(format!("{} {}", list.len(), tr_str(&lang, "张专辑", "albums"))),
                            pal,
                        ))
                        .when(list.is_empty(), |inner| {
                            let cta = macos_primary_pill_btn(
                                "music-empty-bot-cta",
                                tr_str(&lang, "用 @CreatorStudioBot 创作音乐", "Compose with @CreatorStudioBot"),
                                pal,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _, _, cx| {
                                    cx.emit(ContentHubEvent::OpenCreatorBot(Some(
                                        CreationKind::Music,
                                    )));
                                }),
                            );
                            inner.child(empty_state_card_shell(
                                IconName::HubMusic,
                                tr_str(&lang, "暂无公开上架音乐", "No Published Music Yet"),
                                tr_str(
                                    &lang,
                                    "使用 @CreatorStudioBot 创作音乐并审核上架后，专辑与曲目会展示在这里。",
                                    "Published albums appear here ready for instant synthesized playback.",
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
                                        .gap(px(6.0))
                                        .child(apple_shelf_header(
                                            tr_str(&lang, "热门排行榜", "Top Charts"),
                                            Some(format!(
                                                "Top {}",
                                                list.len().min(10)
                                            )),
                                            pal,
                                        ))
                                        .children(list.iter().take(10).enumerate().map(
                                            |(rank, item)| {
                                                let item_play = item.clone();
                                                let is_np = now_playing_cid.as_deref()
                                                    == Some(item.creation_id.as_str());
                                                let hover_bg = pal.row_hover_rgba();
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "music-chart-row-{rank}"
                                                    )))
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
                                                        cx.listener(move |_this, _, _, cx| {
                                                            cx.emit(
                                                                ContentHubEvent::PlayMusicCreation {
                                                                    creation: catalog_item_to_music_creation(&item_play),
                                                                    track_index: 0,
                                                                    shuffle: false,
                                                                },
                                                            );
                                                        }),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap(px(10.0))
                                                            .child(
                                                                div()
                                                                    .w(px(22.0))
                                                                    .text_size(px(13.0))
                                                                    .font_weight(FontWeight::BOLD)
                                                                    .text_color(rgb(if rank < 3 {
                                                                        MUSIC_ACCENT
                                                                    } else {
                                                                        pal.text_secondary
                                                                    }))
                                                                    .child(format!("{}", rank + 1)),
                                                            )
                                                            .child(cover_artwork_box(
                                                                &item.cover,
                                                                36.0,
                                                                36.0,
                                                                6.0,
                                                                MUSIC_ACCENT,
                                                            ))
                                                            .child(
                                                                div()
                                                                    .flex()
                                                                    .flex_col()
                                                                    .child(
                                                                        div()
                                                                            .text_size(px(13.0))
                                                                            .font_weight(
                                                                                FontWeight::SEMIBOLD,
                                                                            )
                                                                            .text_color(rgb(
                                                                                if is_np {
                                                                                    MUSIC_ACCENT
                                                                                } else {
                                                                                    pal.text_primary
                                                                                },
                                                                            ))
                                                                            .child(
                                                                                item.title.clone(),
                                                                            ),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_size(px(11.0))
                                                                            .text_color(rgb(
                                                                                pal.text_secondary,
                                                                            ))
                                                                            .child(
                                                                                item.author_display_name
                                                                                    .clone(),
                                                                            ),
                                                                    ),
                                                            ),
                                                    )
                                                    .child(
                                                        div()
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
                                                                    .font_weight(FontWeight::BOLD)
                                                                    .text_color(rgb(MUSIC_ACCENT))
                                                                    .child(if is_np {
                                                                        "♫"
                                                                    } else {
                                                                        "▶"
                                                                    }),
                                                            ),
                                                    )
                                            },
                                        )),
                                )
                        })
                    },
                ),
        )
        .into_any_element()
}
