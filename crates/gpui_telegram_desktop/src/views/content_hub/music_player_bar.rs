//! Persistent Apple Music Now-Playing Transport Bar (`render_music_player_bar`),
//! synced lyrics/queue drawers, and `MusicPayload` -> `synthesize_music_track_wav()` bridge.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_media::media::audio::{
    synthesize_music_track_wav, MusicPlaybackState, MusicRepeatMode, MusicTrackEntry, SynthChord,
    SynthInstrument, SynthLyricLine, SynthNote, SynthTrackInput, SynthWaveformKind,
};
use gpui_telegram_proto::creations::{
    CoverArtwork, CreationPayload, MusicPayload, MusicTrack, SynthWaveform,
    UpsertCreationStateRequest, UserCreationDto,
};

use crate::i18n::tr_str;
use crate::theme::TdesktopThemePalette;
use crate::views::content_hub::apple_primitives::{
    cover_artwork_box, format_duration_ms,
};
use crate::views::content_hub::{ContentHubEvent, ContentHubWorkspace};

pub const MUSIC_ACCENT: u32 = 0xFA2D48;

fn map_waveform_kind(wf: SynthWaveform) -> SynthWaveformKind {
    match wf {
        SynthWaveform::Sine => SynthWaveformKind::Sine,
        SynthWaveform::Triangle => SynthWaveformKind::Triangle,
        SynthWaveform::Saw => SynthWaveformKind::Saw,
        SynthWaveform::Square => SynthWaveformKind::Square,
        SynthWaveform::Piano => SynthWaveformKind::Piano,
        SynthWaveform::Pad => SynthWaveformKind::Pad,
        SynthWaveform::Bass => SynthWaveformKind::Bass,
        SynthWaveform::Percussion => SynthWaveformKind::Percussion,
    }
}

/// Converts a single `MusicTrack` DTO into a playable `MusicTrackEntry` via real pure-Rust WAV
/// synthesis (`synthesize_music_track_wav`), falling back cleanly to `clock_only` if no notes exist.
pub fn synthesize_entry_from_track(
    creation_id: &str,
    artist: &str,
    album: &str,
    fallback_bpm: u16,
    track: &MusicTrack,
) -> MusicTrackEntry {
    let lyrics: Vec<SynthLyricLine> = track
        .lyrics
        .iter()
        .map(|l| SynthLyricLine {
            timestamp_ms: l.timestamp_ms,
            text: l.text.clone(),
        })
        .collect();

    let bpm = if (30..=300).contains(&track.bpm) {
        track.bpm
    } else if (30..=300).contains(&fallback_bpm) {
        fallback_bpm
    } else {
        110
    };

    let has_synth_material = !track.melody_notes.is_empty() || !track.chord_progression.is_empty();
    let mut entry = if has_synth_material {
        let instruments: Vec<SynthInstrument> = if track.instruments.is_empty() {
            vec![SynthInstrument {
                channel: 0,
                name: "Lead".to_string(),
                waveform: SynthWaveformKind::Piano,
                gain: 0.8,
                pan: 0.0,
                adsr: None,
            }]
        } else {
            track
                .instruments
                .iter()
                .map(|i| SynthInstrument {
                    channel: i.channel,
                    name: i.name.clone(),
                    waveform: map_waveform_kind(i.waveform),
                    gain: i.gain,
                    pan: i.pan,
                    adsr: None,
                })
                .collect()
        };
        let melody_notes: Vec<SynthNote> = track
            .melody_notes
            .iter()
            .map(|n| SynthNote {
                midi_note: n.midi_note,
                start_beat: n.start_beat,
                duration_beats: n.duration_beats,
                velocity: n.velocity,
                channel: n.channel,
            })
            .collect();
        let chord_progression: Vec<SynthChord> = track
            .chord_progression
            .iter()
            .map(|c| SynthChord {
                symbol: c.symbol.clone(),
                start_beat: c.start_beat,
                duration_beats: c.duration_beats,
                velocity: c.velocity,
                channel: c.channel,
            })
            .collect();

        let input = SynthTrackInput {
            track_id: track.track_id.clone(),
            title: track.title.clone(),
            bpm,
            sample_rate: 44_100,
            channels: 1,
            duration_secs_hint: (track.duration_secs > 0).then_some(track.duration_secs.min(30) as f32),
            instruments,
            melody_notes,
            chord_progression,
            lyrics: lyrics.clone(),
        };
        match synthesize_music_track_wav(&input) {
            Ok(audio) => MusicTrackEntry::from_synthesized(
                track.track_id.clone(),
                track.title.clone(),
                artist,
                album,
                audio,
                lyrics,
            ),
            Err(_) => {
                let dur_ms = (track.duration_secs.max(15) as u64) * 1000;
                MusicTrackEntry::clock_only(
                    track.track_id.clone(),
                    track.title.clone(),
                    artist,
                    album,
                    dur_ms,
                    lyrics,
                )
            }
        }
    } else {
        let dur_ms = (track.duration_secs.max(15) as u64) * 1000;
        MusicTrackEntry::clock_only(
            track.track_id.clone(),
            track.title.clone(),
            artist,
            album,
            dur_ms,
            lyrics,
        )
    };

    entry.creation_id = Some(creation_id.to_string());
    if entry.waveform_5bit.is_empty() && !track.waveform_5bit.is_empty() {
        entry.waveform_5bit = track.waveform_5bit.clone();
    }
    entry
}

/// Synthesizes all tracks inside a `MusicPayload` creation into playable `MusicTrackEntry` items.
pub fn build_music_track_entries_from_creation(creation: &UserCreationDto) -> Vec<MusicTrackEntry> {
    let mp: MusicPayload = match &creation.payload {
        CreationPayload::Music(m) => m.clone(),
        _ => return Vec::new(),
    };
    let artist = if !mp.artist.trim().is_empty() {
        mp.artist.as_str()
    } else {
        creation.author_display_name.as_str()
    };
    let album = if !mp.album_title.trim().is_empty() {
        mp.album_title.as_str()
    } else {
        creation.title.as_str()
    };
    if mp.tracks.is_empty() {
        let mut fallback = MusicTrackEntry::clock_only(
            format!("{}_tr_1", creation.creation_id),
            creation.title.clone(),
            artist,
            album,
            (mp.total_duration_secs.max(30) as u64) * 1000,
            Vec::new(),
        );
        fallback.creation_id = Some(creation.creation_id.clone());
        return vec![fallback];
    }
    mp.tracks
        .iter()
        .map(|tr| synthesize_entry_from_track(&creation.creation_id, artist, album, mp.bpm, tr))
        .collect()
}

fn resolve_now_playing_cover(ws: &ContentHubWorkspace, creation_id: Option<&str>) -> CoverArtwork {
    if let Some(cid) = creation_id {
        if let Some(item) = ws.find_catalog_item(cid) {
            return item.cover.clone();
        }
        if let Some(own) = ws.find_my_creation(cid) {
            return own.cover.clone();
        }
        if let Some(ref d) = ws.selected_detail_creation {
            if d.creation_id == cid {
                return d.cover.clone();
            }
        }
    }
    CoverArtwork {
        gradient_start_hex: "#FA2D48".to_string(),
        gradient_end_hex: "#5E17EB".to_string(),
        accent_hex: "#FF6B81".to_string(),
        icon_emoji: "♫".to_string(),
        media_file_id: None,
        thumbnail_data_url: None,
    }
}

/// Renders the persistent Apple Music Now-Playing transport bar + collapsible Lyrics / Queue drawer.
pub fn render_music_player_bar(
    ws: &mut ContentHubWorkspace,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let pal = ws.palette;
    let lang = ws.language_id.clone();
    let snap = ws.music_engine.snapshot();
    let current_track = ws.music_engine.current_track().cloned();
    let queue = ws.music_engine.queue().to_vec();
    let active_lyric_idx = ws.music_engine.active_lyric_index();
    let now_cid = current_track
        .as_ref()
        .and_then(|t| t.creation_id.clone())
        .or_else(|| ws.music_now_playing_creation_id.clone());
    let cover = resolve_now_playing_cover(ws, now_cid.as_deref());
    let is_loved = now_cid
        .as_deref()
        .and_then(|cid| ws.my_creation_states.get(cid))
        .is_some_and(|st| st.favorite);

    let is_playing = snap.state == MusicPlaybackState::Playing;
    let dur_ms = snap.duration_ms.max(1);
    let progress_frac = (snap.position_ms as f32 / dur_ms as f32).clamp(0.0, 1.0);

    // 24 compact waveform bars sampled from current track (or deterministic fallback heights)
    let waveform_bars: Vec<f32> = if let Some(ref tr) = current_track {
        if !tr.waveform_bars.is_empty() {
            let step = (tr.waveform_bars.len() as f32 / 24.0).max(1.0);
            (0..24)
                .map(|i| {
                    let idx = ((i as f32 * step) as usize).min(tr.waveform_bars.len() - 1);
                    (tr.waveform_bars[idx] as f32 / 31.0).clamp(0.18, 1.0)
                })
                .collect()
        } else {
            (0..24)
                .map(|i| 0.30 + ((i * 7 + 3) % 10) as f32 * 0.065)
                .collect()
        }
    } else {
        vec![0.25; 24]
    };

    let repeat_label = match snap.repeat_mode {
        MusicRepeatMode::Off => tr_str(&lang, "循环:关", "Rpt Off"),
        MusicRepeatMode::All => tr_str(&lang, "列表循环", "Rpt All"),
        MusicRepeatMode::One => tr_str(&lang, "单曲循环", "Rpt 1"),
    };

    let show_lyrics = ws.music_lyrics_open;
    let show_queue = ws.music_queue_open;
    let track_lyrics = current_track
        .as_ref()
        .map(|t| t.lyrics.clone())
        .unwrap_or_default();

    div()
        .id("apple-music-player-dock")
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        // Optional synced lyrics or queue panel right above the transport bar
        .when(show_lyrics || show_queue, |el| {
            el.child(
                div()
                    .id("music-player-aux-drawer")
                    .w_full()
                    .max_h(px(210.0))
                    .bg(pal.card_surface_rgba())
                    .border_t_1()
                    .border_color(pal.separator_rgba())
                    .px(px(20.0))
                    .py(px(12.0))
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
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(pal.text_primary))
                                    .child(if show_lyrics {
                                        tr_str(&lang, "实时歌词 · Synced Lyrics", "Synced Lyrics")
                                    } else {
                                        tr_str(&lang, "待播队列 · Up Next", "Up Next Queue")
                                    }),
                            )
                            .child(
                                div()
                                    .id("music-aux-drawer-close")
                                    .cursor_pointer()
                                    .px(px(8.0))
                                    .py(px(2.0))
                                    .rounded(px(6.0))
                                    .text_size(px(11.0))
                                    .text_color(rgb(pal.text_secondary))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.music_lyrics_open = false;
                                            this.music_queue_open = false;
                                            cx.notify();
                                        }),
                                    )
                                    .child(tr_str(&lang, "收起", "Hide")),
                            ),
                    )
                    .child(
                        div()
                            .id("music-player-aux-scroll")
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap(px(5.0))
                            .when(show_lyrics, |list| {
                                if track_lyrics.is_empty() {
                                    list.child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(rgb(pal.text_secondary))
                                            .child(tr_str(
                                                &lang,
                                                "当前曲目为纯音乐或未附带时间轴歌词。",
                                                "Instrumental track or no synced lyrics attached.",
                                            )),
                                    )
                                } else {
                                    list.children(track_lyrics.iter().enumerate().map(
                                        |(idx, line)| {
                                            let is_active = active_lyric_idx == Some(idx);
                                            let ts_ms = u64::from(line.timestamp_ms);
                                            div()
                                                .id(SharedString::from(format!(
                                                    "music-lyric-line-{idx}"
                                                )))
                                                .cursor_pointer()
                                                .px(px(10.0))
                                                .py(px(4.0))
                                                .rounded(px(7.0))
                                                .bg(if is_active {
                                                    TdesktopThemePalette::rgba_with_alpha(
                                                        MUSIC_ACCENT,
                                                        0.16,
                                                    )
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
                                                        cx.emit(
                                                            ContentHubEvent::MusicTransportChanged,
                                                        );
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
                                                        .text_size(px(if is_active {
                                                            14.0
                                                        } else {
                                                            12.5
                                                        }))
                                                        .font_weight(if is_active {
                                                            FontWeight::BOLD
                                                        } else {
                                                            FontWeight::NORMAL
                                                        })
                                                        .text_color(rgb(if is_active {
                                                            MUSIC_ACCENT
                                                        } else {
                                                            pal.text_primary
                                                        }))
                                                        .child(line.text.clone()),
                                                )
                                        },
                                    ))
                                }
                            })
                            .when(!show_lyrics && show_queue, |list| {
                                if queue.is_empty() {
                                    list.child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(rgb(pal.text_secondary))
                                            .child(tr_str(
                                                &lang,
                                                "播放队列为空，在乐库中选择专辑开始播放。",
                                                "Queue is empty. Pick an album or track to play.",
                                            )),
                                    )
                                } else {
                                    list.children(queue.iter().enumerate().map(|(idx, tr)| {
                                        let is_cur = snap.current_index == Some(idx);
                                        let hover_bg = pal.row_hover_rgba();
                                        div()
                                            .id(SharedString::from(format!(
                                                "music-queue-row-{idx}"
                                            )))
                                            .cursor_pointer()
                                            .px(px(10.0))
                                            .py(px(5.0))
                                            .rounded(px(7.0))
                                            .bg(if is_cur {
                                                TdesktopThemePalette::rgba_with_alpha(
                                                    MUSIC_ACCENT,
                                                    0.14,
                                                )
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
                                                cx.listener(move |this, _, _, cx| {
                                                    let _ = this.music_engine.play_index(idx);
                                                    cx.emit(ContentHubEvent::MusicTransportChanged);
                                                    cx.notify();
                                                }),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .font_weight(if is_cur {
                                                        FontWeight::SEMIBOLD
                                                    } else {
                                                        FontWeight::NORMAL
                                                    })
                                                    .text_color(rgb(if is_cur {
                                                        MUSIC_ACCENT
                                                    } else {
                                                        pal.text_primary
                                                    }))
                                                    .child(format!(
                                                        "{}. {} — {}",
                                                        idx + 1,
                                                        tr.title,
                                                        tr.artist
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(pal.text_secondary))
                                                    .child(format_duration_ms(tr.duration_ms)),
                                            )
                                    }))
                                }
                            }),
                    ),
            )
        })
        // Main Apple Music Transport Bar
        .child(
            div()
                .id("apple-music-player-bar")
                .flex_shrink_0()
                .w_full()
                .h(px(70.0))
                .px(px(18.0))
                .overflow_hidden()
                .bg(pal.card_surface_rgba())
                .border_t_1()
                .border_color(pal.separator_rgba())
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.0))
                // Left: artwork + title/artist + love toggle
                .child(
                    div()
                        .w(px(230.0))
                        .min_w(px(120.0))
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(cover_artwork_box(&cover, 42.0, 42.0, 7.0, MUSIC_ACCENT))
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
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.text_primary))
                                        .truncate()
                                        .child(
                                            current_track
                                                .as_ref()
                                                .map(|t| t.title.clone())
                                                .unwrap_or_else(|| {
                                                    tr_str(&lang, "未在播放", "Not Playing")
                                                        .to_string()
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .truncate()
                                        .child(
                                            current_track
                                                .as_ref()
                                                .map(|t| format!("{} — {}", t.artist, t.album))
                                                .unwrap_or_else(|| {
                                                    tr_str(
                                                        &lang,
                                                        "选择一首作品开始聆听",
                                                        "Choose a track to play",
                                                    )
                                                    .to_string()
                                                }),
                                        ),
                                ),
                        )
                        .when_some(now_cid.clone(), |el, cid_love| {
                            el.child(
                                div()
                                    .id("music-bar-love-btn")
                                    .cursor_pointer()
                                    .px(px(6.0))
                                    .py(px(4.0))
                                    .rounded(px(6.0))
                                    .text_size(px(14.0))
                                    .text_color(rgb(if is_loved {
                                        MUSIC_ACCENT
                                    } else {
                                        pal.text_secondary
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_this, _, _, cx| {
                                            cx.emit(ContentHubEvent::UpsertCreationState {
                                                creation_id: cid_love.clone(),
                                                req: UpsertCreationStateRequest {
                                                    favorite: Some(!is_loved),
                                                    in_library: Some(true),
                                                    ..Default::default()
                                                },
                                            });
                                        }),
                                    )
                                    .child(if is_loved { "♥" } else { "♡" }),
                            )
                        }),
                )
                // Center: Transport buttons + Interactive 5-bit Waveform Scrubber
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .max_w(px(480.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .id("music-bar-shuffle-btn")
                                        .cursor_pointer()
                                        .px(px(7.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .bg(if snap.shuffle {
                                            TdesktopThemePalette::rgba_with_alpha(
                                                MUSIC_ACCENT,
                                                0.18,
                                            )
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(if snap.shuffle {
                                            MUSIC_ACCENT
                                        } else {
                                            pal.text_secondary
                                        }))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                let next = !this.music_engine.is_shuffle();
                                                this.music_engine.set_shuffle(next);
                                                cx.notify();
                                            }),
                                        )
                                        .child(tr_str(&lang, "随机", "Shuf")),
                                )
                                .child(
                                    div()
                                        .id("music-bar-prev-btn")
                                        .cursor_pointer()
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .text_size(px(13.0))
                                        .text_color(rgb(pal.text_primary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.music_engine.prev();
                                                cx.emit(ContentHubEvent::MusicTransportChanged);
                                                cx.notify();
                                            }),
                                        )
                                        .child("⏮"),
                                )
                                .child(
                                    div()
                                        .id("music-bar-play-pause-btn")
                                        .cursor_pointer()
                                        .w(px(32.0))
                                        .h(px(32.0))
                                        .rounded(px(999.0))
                                        .bg(rgb(MUSIC_ACCENT))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                let _ = this.music_engine.toggle_play_pause();
                                                cx.emit(ContentHubEvent::MusicTransportChanged);
                                                cx.notify();
                                            }),
                                        )
                                        .child(if is_playing { "⏸" } else { "▶" }),
                                )
                                .child(
                                    div()
                                        .id("music-bar-next-btn")
                                        .cursor_pointer()
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .text_size(px(13.0))
                                        .text_color(rgb(pal.text_primary))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.music_engine.next();
                                                cx.emit(ContentHubEvent::MusicTransportChanged);
                                                cx.notify();
                                            }),
                                        )
                                        .child("⏭"),
                                )
                                .child(
                                    div()
                                        .id("music-bar-repeat-btn")
                                        .cursor_pointer()
                                        .px(px(7.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .bg(if snap.repeat_mode != MusicRepeatMode::Off {
                                            TdesktopThemePalette::rgba_with_alpha(
                                                MUSIC_ACCENT,
                                                0.18,
                                            )
                                        } else {
                                            rgba(0x00000000)
                                        })
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(
                                            if snap.repeat_mode != MusicRepeatMode::Off {
                                                MUSIC_ACCENT
                                            } else {
                                                pal.text_secondary
                                            },
                                        ))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                let next = this.music_engine.repeat_mode().cycle();
                                                this.music_engine.set_repeat_mode(next);
                                                cx.notify();
                                            }),
                                        )
                                        .child(repeat_label),
                                ),
                        )
                        // Waveform scrubber row
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .w(px(36.0))
                                        .text_size(px(10.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format_duration_ms(snap.position_ms)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .h(px(18.0))
                                        .flex()
                                        .items_center()
                                        .gap(px(2.0))
                                        .children(waveform_bars.iter().enumerate().map(
                                            |(idx, &amp)| {
                                                let seg_frac = (idx as f32 + 0.5) / 24.0;
                                                let played = seg_frac <= progress_frac;
                                                let seek_target_ms =
                                                    ((idx as f32 / 24.0) * dur_ms as f32) as u64;
                                                let h_px = (amp * 16.0).clamp(3.0, 16.0);
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "music-wave-seek-{idx}"
                                                    )))
                                                    .cursor_pointer()
                                                    .flex_1()
                                                    .h(px(18.0))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            let _ = this
                                                                .music_engine
                                                                .seek_ms(seek_target_ms);
                                                            cx.emit(
                                                                ContentHubEvent::MusicTransportChanged,
                                                            );
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .h(px(h_px))
                                                            .rounded(px(2.0))
                                                            .bg(if played {
                                                                rgb(MUSIC_ACCENT)
                                                            } else {
                                                                TdesktopThemePalette::rgba_with_alpha(
                                                                    pal.text_secondary,
                                                                    0.34,
                                                                )
                                                            }),
                                                    )
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .w(px(36.0))
                                        .text_size(px(10.0))
                                        .text_color(rgb(pal.text_secondary))
                                        .child(format_duration_ms(snap.duration_ms)),
                                ),
                        ),
                )
                // Right: Volume + Lyrics + Queue toggles
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("music-bar-vol-down")
                                .cursor_pointer()
                                .px(px(6.0))
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.text_primary, 0.06))
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        let v = (this.music_engine.volume() - 0.15).clamp(0.0, 1.0);
                                        this.music_engine.set_volume(v);
                                        cx.notify();
                                    }),
                                )
                                .child("🔉"),
                        )
                        .child(
                            div()
                                .w(px(44.0))
                                .h(px(4.0))
                                .rounded(px(999.0))
                                .bg(pal.separator_rgba())
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative(snap.volume.clamp(0.0, 1.0)))
                                        .bg(rgb(MUSIC_ACCENT)),
                                ),
                        )
                        .child(
                            div()
                                .id("music-bar-vol-up")
                                .cursor_pointer()
                                .px(px(6.0))
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(pal.text_primary, 0.06))
                                .text_size(px(11.0))
                                .text_color(rgb(pal.text_primary))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        let v = (this.music_engine.volume() + 0.15).clamp(0.0, 1.0);
                                        this.music_engine.set_volume(v);
                                        cx.notify();
                                    }),
                                )
                                .child("🔊"),
                        )
                        .child(
                            div()
                                .id("music-bar-lyrics-btn")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(4.0))
                                .rounded(px(7.0))
                                .bg(if show_lyrics {
                                    rgb(MUSIC_ACCENT)
                                } else {
                                    TdesktopThemePalette::rgba_with_alpha(pal.text_primary, 0.07)
                                })
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if show_lyrics {
                                    0xFFFFFF
                                } else {
                                    pal.text_primary
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.music_lyrics_open = !this.music_lyrics_open;
                                        if this.music_lyrics_open {
                                            this.music_queue_open = false;
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(&lang, "词", "Lyrics")),
                        )
                        .child(
                            div()
                                .id("music-bar-queue-btn")
                                .cursor_pointer()
                                .px(px(9.0))
                                .py(px(4.0))
                                .rounded(px(7.0))
                                .bg(if show_queue {
                                    rgb(MUSIC_ACCENT)
                                } else {
                                    TdesktopThemePalette::rgba_with_alpha(pal.text_primary, 0.07)
                                })
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if show_queue {
                                    0xFFFFFF
                                } else {
                                    pal.text_primary
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.music_queue_open = !this.music_queue_open;
                                        if this.music_queue_open {
                                            this.music_lyrics_open = false;
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child(format!(
                                    "{} ({})",
                                    tr_str(&lang, "队列", "Queue"),
                                    queue.len()
                                )),
                        ),
                ),
        )
        .into_any_element()
}
