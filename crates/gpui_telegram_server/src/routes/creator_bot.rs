use crate::db::miniapp_store::normalize_bot_slug;
use crate::db::social_store::MediaMetadataDto;
use crate::db::{CreationOpError, ServerDatabase};
use crate::media::cas_store::CasStore;
use crate::routes::telegram_bot_api::{enqueue_update_and_maybe_webhook, tg_user_json};
use crate::routes::{broadcast_chat_pts, err_resp, map_telegram_err};
use crate::search::PublicCatalogSearchService;
use crate::server::{AppState, AuthBearer};
use crate::ws::GatewayHub;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use chrono::Utc;
use gpui_telegram_core::core::deep_links::{AppleDeepLink, AppleDeepLinkSection};
use gpui_telegram_core::types::{ChatId, MsgId, PeerId, TopicId, UserId};
use gpui_telegram_media::{
    synthesize_music_track_wav, SynthChord, SynthInstrument, SynthLyricLine, SynthNote,
    SynthTrackInput, SynthWaveformKind,
};
use gpui_telegram_proto::cards::{GenerativeCard, GenerativeCardLayout, GenerativeComponent};
use gpui_telegram_proto::creations::{
    AppStoreListingPayload, BookChapter, BookPage, BookPayload, BookRichBlock,
    BotRecommendationItem, CardPair, ChordEvent, CoverArtwork, CreateCreationRequest, CreationKind,
    CreationPayload, CreationReviewStatus, CreationVisibility, GameAchievementDef, GameCategory,
    GameEngineKind, GamePayload, GameSpec, GridPuzzleLevel, InstrumentSpec, MusicPayload,
    MusicTrack, NoteEvent, ReviewDecision, ReviewDecisionRequest, StoryChoice, StoryNode,
    SubmitReviewRequest, SyncedLyricLine, SynthWaveform, TriviaQuestion, UpdateCreationRequest,
    UserCreationDto, CREATOR_STUDIO_BOT_USER_ID, CREATOR_STUDIO_BOT_USERNAME,
    MIN_CREATION_SUMMARY_CHARS,
};
use gpui_telegram_proto::message::{InlineKeyboardButton, MessageContent, MessageItem};
use gpui_telegram_proto::miniapp_contracts::BotCommandSpec;
use gpui_telegram_proto::sync::UpdateEvent;
use gpui_telegram_proto::ws::WsServerFrame;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) const CREATOR_STUDIO_BOT_TOKEN: &str = "777020:AAOfficialCreatorStudioBotV1";

pub(crate) fn creator_studio_commands() -> Vec<BotCommandSpec> {
    vec![
        BotCommandSpec {
            command: "start".into(),
            description: "开始使用 Creator Studio 创作助手".into(),
        },
        BotCommandSpec {
            command: "help".into(),
            description: "查看创作、提交审核与公开推荐命令".into(),
        },
        BotCommandSpec {
            command: "newbook".into(),
            description: "创作云端图书: /newbook 标题 | 简介".into(),
        },
        BotCommandSpec {
            command: "newmusic".into(),
            description: "创作云端音乐: /newmusic 标题 | 简介".into(),
        },
        BotCommandSpec {
            command: "newgame".into(),
            description: "创作云端游戏: /newgame 标题 | 简介".into(),
        },
        BotCommandSpec {
            command: "newapp".into(),
            description: "创作云端应用: /newapp 标题 | 简介".into(),
        },
        BotCommandSpec {
            command: "mycreations".into(),
            description: "查看我的云端作品: /mycreations [book|music|game|app]".into(),
        },
        BotCommandSpec {
            command: "editwork".into(),
            description: "编辑我的作品: /editwork <id> title=新标题 [summary=新简介]".into(),
        },
        BotCommandSpec {
            command: "submitwork".into(),
            description: "推送作品审核上架: /submitwork <creation_id>".into(),
        },
        BotCommandSpec {
            command: "withdraw".into(),
            description: "撤回待审核作品: /withdraw <creation_id>".into(),
        },
        BotCommandSpec {
            command: "unpublish".into(),
            description: "下架已发布作品: /unpublish <creation_id>".into(),
        },
        BotCommandSpec {
            command: "deletework".into(),
            description: "删除我的作品: /deletework <creation_id>".into(),
        },
        BotCommandSpec {
            command: "recommend".into(),
            description: "推荐已公开上架作品: /recommend [books|music|games|appstore] [关键词]".into(),
        },
    ]
}

pub(crate) fn resolve_owner_display_name(db: &ServerDatabase, owner_uid: UserId) -> String {
    if let Ok(Some(u)) = db.get_user(owner_uid) {
        let mut full = u.first_name.trim().to_string();
        if let Some(last) = u.last_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if !full.is_empty() {
                full.push(' ');
            }
            full.push_str(last);
        }
        if !full.is_empty() {
            return full;
        }
        if let Some(un) = u.username.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return un.to_string();
        }
    }
    format!("Creator {}", owner_uid.0)
}

pub(crate) fn detect_language(text: &str) -> String {
    if text
        .chars()
        .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c))
    {
        "zh-CN".to_string()
    } else {
        "en-US".to_string()
    }
}

pub(crate) fn default_cover_for_kind(kind: CreationKind) -> CoverArtwork {
    match kind {
        CreationKind::Book => CoverArtwork {
            gradient_start_hex: "#EA580C".into(),
            gradient_end_hex: "#431407".into(),
            accent_hex: "#FDBA74".into(),
            icon_emoji: "📖".into(),
            media_file_id: None,
            thumbnail_data_url: None,
        },
        CreationKind::Music => CoverArtwork {
            gradient_start_hex: "#FA2D48".into(),
            gradient_end_hex: "#4C0519".into(),
            accent_hex: "#FDA4AF".into(),
            icon_emoji: "🎵".into(),
            media_file_id: None,
            thumbnail_data_url: None,
        },
        CreationKind::Game => CoverArtwork {
            gradient_start_hex: "#6366F1".into(),
            gradient_end_hex: "#1E1B4B".into(),
            accent_hex: "#A5B4FC".into(),
            icon_emoji: "🎮".into(),
            media_file_id: None,
            thumbnail_data_url: None,
        },
        CreationKind::App => CoverArtwork {
            gradient_start_hex: "#007AFF".into(),
            gradient_end_hex: "#0F172A".into(),
            accent_hex: "#60A5FA".into(),
            icon_emoji: "✨".into(),
            media_file_id: None,
            thumbnail_data_url: None,
        },
    }
}

pub(crate) fn ensure_bot_summary(title: &str, raw_summary: Option<&str>, kind: CreationKind) -> String {
    if let Some(s) = raw_summary.map(str::trim).filter(|s| !s.is_empty()) {
        if s.chars().count() >= MIN_CREATION_SUMMARY_CHARS {
            return s.to_string();
        }
        if detect_language(&format!("{title} {s}")) == "zh-CN" {
            return format!("{s} — 由 Creator Studio 创作的《{title}》原创云端作品。");
        }
        return format!("{s} — An original cloud creation titled \"{title}\".");
    }
    let is_zh = detect_language(title) == "zh-CN";
    match (kind, is_zh) {
        (CreationKind::Book, true) => {
            format!("《{title}》是一部由 Creator Studio 辅助创作的多章节云端互动图书作品。")
        }
        (CreationKind::Book, false) => {
            format!("An original multi-chapter cloud book titled \"{title}\", crafted with Creator Studio.")
        }
        (CreationKind::Music, true) => {
            format!("《{title}》是一首包含多轨旋律、和声编排与同步歌词的原创云端音乐作品。")
        }
        (CreationKind::Music, false) => {
            format!("An original multi-track cloud music composition titled \"{title}\" with melody, chords, and lyrics.")
        }
        (CreationKind::Game, true) => {
            format!("《{title}》是一款支持即时试玩与云端排行榜的原创互动迷你游戏。")
        }
        (CreationKind::Game, false) => {
            format!("An original playable mini-game titled \"{title}\" with interactive rules and cloud leaderboard support.")
        }
        (CreationKind::App, true) => {
            format!("《{title}》是一款已上架云端生态的原创应用作品。")
        }
        (CreationKind::App, false) => {
            format!("An original cloud application listing titled \"{title}\".")
        }
    }
}

pub(crate) fn emit_user_creation_changed_to_hub(hub: &GatewayHub, dto: &UserCreationDto) {
    let event = UpdateEvent::UserCreationChanged {
        creation_id: dto.creation_id.clone(),
        owner_user_id: dto.owner_user_id,
        kind: dto.kind,
        review_status: dto.review_status,
        cloud_revision: dto.cloud_revision,
    };
    hub.broadcast_to_user(
        UserId(dto.owner_user_id as u64),
        WsServerFrame::Update(event),
    );
}

pub(crate) fn compute_book_word_count(chapters: &[BookChapter]) -> (u32, u32) {
    let mut units: usize = 0;
    for ch in chapters {
        for page in &ch.pages {
            for blk in &page.blocks {
                let text = match blk {
                    BookRichBlock::Heading { text, .. } => text.as_str(),
                    BookRichBlock::Paragraph { text } => text.as_str(),
                    BookRichBlock::BlockQuote { text, .. } => text.as_str(),
                    BookRichBlock::Callout { text, .. } => text.as_str(),
                    BookRichBlock::Illustration { caption, .. } => caption.as_str(),
                    BookRichBlock::Divider => "",
                };
                let cjk_count = text
                    .chars()
                    .filter(|c| ('\u{4E00}'..='\u{9FFF}').contains(c))
                    .count();
                let word_count = text.split_whitespace().count();
                units = units.saturating_add(cjk_count.max(word_count));
            }
        }
    }
    let words = u32::try_from(units).unwrap_or(u32::MAX);
    let mins = (words / 220).max(1);
    (words, mins)
}

pub(crate) fn build_default_book_payload(
    title: &str,
    summary: &str,
    author: &str,
    language: &str,
) -> BookPayload {
    let is_zh = language.to_ascii_lowercase().starts_with("zh") || detect_language(title) == "zh-CN";
    let chapters = if is_zh {
        vec![
            BookChapter {
                chapter_id: "ch_1".into(),
                chapter_number: 1,
                title: format!("第一章：启程——{title}"),
                subtitle: Some("序幕与世界观展开".into()),
                pages: vec![BookPage {
                    page_number: 1,
                    blocks: vec![
                        BookRichBlock::Heading {
                            level: 1,
                            text: format!("第一章：启程——{title}"),
                        },
                        BookRichBlock::Paragraph {
                            text: format!(
                                "故事从这里拉开帷幕。{summary} 在静谧而辽阔的晨光里，旅人翻开了第一页手记，记录下关于未知旅程的线索与期待。"
                            ),
                        },
                        BookRichBlock::BlockQuote {
                            text: format!("每一段值得铭记的旅程，都始于推开《{title}》第一扇门的那一刻。"),
                            attribution: Some(author.to_string()),
                        },
                        BookRichBlock::Paragraph {
                            text: "沿着星图与古老的注脚前行，细节逐渐清晰起来。人物的命运与世界的脉络交织在一起，引向更加宏大的篇章。".into(),
                        },
                    ],
                }],
            },
            BookChapter {
                chapter_id: "ch_2".into(),
                chapter_number: 2,
                title: "第二章：回响与新章".into(),
                subtitle: Some("转折与深入探索".into()),
                pages: vec![BookPage {
                    page_number: 1,
                    blocks: vec![
                        BookRichBlock::Heading {
                            level: 1,
                            text: "第二章：回响与新章".into(),
                        },
                        BookRichBlock::Paragraph {
                            text: format!(
                                "当最初的疑问找到答案，新的地平线已在前方浮现。《{title}》的核心主题在这一章逐渐沉淀，为后续章节埋下伏笔。"
                            ),
                        },
                        BookRichBlock::Callout {
                            title: "创作手记".into(),
                            text: "你可以在「我的创作」中继续增补章节、调整排版主题，并在完成后推送审核上架至图书板块。".into(),
                        },
                    ],
                }],
            },
        ]
    } else {
        vec![
            BookChapter {
                chapter_id: "ch_1".into(),
                chapter_number: 1,
                title: format!("Chapter 1: Into {title}"),
                subtitle: Some("Opening movements and setting".into()),
                pages: vec![BookPage {
                    page_number: 1,
                    blocks: vec![
                        BookRichBlock::Heading {
                            level: 1,
                            text: format!("Chapter 1: Into {title}"),
                        },
                        BookRichBlock::Paragraph {
                            text: format!(
                                "The journey begins with a quiet spark. {summary} Across the opening horizon, every clue points toward a larger story waiting to unfold."
                            ),
                        },
                        BookRichBlock::BlockQuote {
                            text: format!("Every memorable world opens with a single turning page of {title}."),
                            attribution: Some(author.to_string()),
                        },
                        BookRichBlock::Paragraph {
                            text: "Step by step, characters step out of the shadows, carrying notebooks, questions, and promises that bind the entire narrative together.".into(),
                        },
                    ],
                }],
            },
            BookChapter {
                chapter_id: "ch_2".into(),
                chapter_number: 2,
                title: "Chapter 2: Echoes on the Horizon".into(),
                subtitle: Some("Turning point and discovery".into()),
                pages: vec![BookPage {
                    page_number: 1,
                    blocks: vec![
                        BookRichBlock::Heading {
                            level: 1,
                            text: "Chapter 2: Echoes on the Horizon".into(),
                        },
                        BookRichBlock::Paragraph {
                            text: format!(
                                "As early mysteries resolve, deeper currents rise beneath the surface of {title}, inviting readers to continue into the next act."
                            ),
                        },
                        BookRichBlock::Callout {
                            title: "Author Note".into(),
                            text: "Continue editing chapters anytime in My Creations, then submit for review to publish into Apple Books.".into(),
                        },
                    ],
                }],
            },
        ]
    };
    let (word_count, estimated_read_mins) = compute_book_word_count(&chapters);
    BookPayload {
        author: author.to_string(),
        series_name: None,
        series_index: None,
        blurb: summary.to_string(),
        estimated_read_mins,
        word_count,
        chapters,
    }
}

pub(crate) fn parse_book_chapters_or_default(
    raw_chapters: Option<&Value>,
    title: &str,
    summary: &str,
    author: &str,
    language: &str,
) -> Result<Vec<BookChapter>, String> {
    let Some(val) = raw_chapters.filter(|v| !v.is_null()) else {
        return Ok(build_default_book_payload(title, summary, author, language).chapters);
    };
    let arr = val
        .as_array()
        .ok_or_else(|| "'chapters' must be an array".to_string())?;
    let mut chapters = Vec::with_capacity(arr.len());
    for (idx, item) in arr.iter().enumerate() {
        if let Ok(ch) = serde_json::from_value::<BookChapter>(item.clone()) {
            chapters.push(ch);
            continue;
        }
        let obj = item
            .as_object()
            .ok_or_else(|| format!("chapters[{idx}] must be an object"))?;
        let ch_title = obj
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        let ch_id = obj
            .get("chapter_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("ch_{}", idx + 1));
        let mut blocks = Vec::new();
        if !ch_title.is_empty() {
            blocks.push(BookRichBlock::Heading {
                level: 1,
                text: ch_title.clone(),
            });
        }
        if let Some(content) = obj.get("content").and_then(Value::as_str) {
            for para in content.split("\n\n").map(str::trim).filter(|s| !s.is_empty()) {
                blocks.push(BookRichBlock::Paragraph {
                    text: para.to_string(),
                });
            }
        }
        if let Some(paras) = obj.get("paragraphs").and_then(Value::as_array) {
            for p in paras.iter().filter_map(Value::as_str) {
                if !p.trim().is_empty() {
                    blocks.push(BookRichBlock::Paragraph {
                        text: p.trim().to_string(),
                    });
                }
            }
        }
        chapters.push(BookChapter {
            chapter_id: ch_id,
            chapter_number: u32::try_from(idx.saturating_add(1)).unwrap_or(u32::MAX),
            title: ch_title,
            subtitle: obj
                .get("subtitle")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            pages: vec![BookPage {
                page_number: 1,
                blocks,
            }],
        });
    }
    Ok(chapters)
}

pub(crate) fn map_synth_waveform(wf: SynthWaveform) -> SynthWaveformKind {
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

pub(crate) fn build_default_music_payload(
    title: &str,
    summary: &str,
    artist: &str,
    bpm: u16,
    musical_key: Option<&str>,
    primary_genre: Option<&str>,
    language: &str,
    seed_salt: u64,
) -> MusicPayload {
    let mut hasher = Sha256::new();
    hasher.update(title.as_bytes());
    hasher.update(b"|");
    hasher.update(summary.as_bytes());
    hasher.update(seed_salt.to_le_bytes());
    let digest = hasher.finalize();

    let key_table: [(&str, u8, [&str; 4]); 6] = [
        ("C major", 60, ["C", "Am", "F", "G"]),
        ("D major", 62, ["D", "Bm", "G", "A"]),
        ("Eb major", 63, ["Eb", "Cm", "Ab", "Bb"]),
        ("F major", 65, ["F", "Dm", "Bb", "C"]),
        ("G major", 67, ["G", "Em", "C", "D"]),
        ("A minor", 57, ["Am", "F", "C", "G"]),
    ];
    let (default_key, root_midi, chords) = key_table[(digest[0] as usize) % key_table.len()];
    let resolved_key = musical_key
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default_key)
        .to_string();
    let resolved_genre = primary_genre
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Electronic")
        .to_string();

    let scale_offsets: [u8; 8] = [0, 2, 4, 5, 7, 9, 11, 12];
    let motif_idx = (digest[1] as usize) % 3;
    let degree_pattern: [usize; 12] = match motif_idx {
        0 => [0, 2, 4, 7, 5, 4, 2, 4, 0, 4, 5, 7],
        1 => [4, 2, 0, 2, 4, 5, 7, 5, 4, 2, 4, 0],
        _ => [0, 4, 5, 4, 2, 4, 7, 5, 4, 2, 0, 4],
    };

    let mut melody_notes = Vec::with_capacity(degree_pattern.len());
    for (idx, &deg) in degree_pattern.iter().enumerate() {
        let vel_jitter = (digest[2 + (idx % 12)] % 18) as u8;
        let midi = (root_midi + scale_offsets[deg]).min(120);
        melody_notes.push(NoteEvent {
            midi_note: midi,
            start_beat: idx as f32 * 0.75,
            duration_beats: 0.65,
            velocity: 88 + vel_jitter,
            channel: 0,
        });
    }
    // Bass pulse notes on channel 2
    for bar in 0..4usize {
        let bass_midi = root_midi.saturating_sub(12);
        melody_notes.push(NoteEvent {
            midi_note: bass_midi + [0, 9, 5, 7][bar] % 12,
            start_beat: (bar * 2) as f32,
            duration_beats: 1.75,
            velocity: 82,
            channel: 2,
        });
    }

    let chord_progression: Vec<ChordEvent> = chords
        .iter()
        .enumerate()
        .map(|(idx, sym)| ChordEvent {
            symbol: (*sym).to_string(),
            start_beat: (idx * 2) as f32,
            duration_beats: 2.0,
            velocity: 70,
            channel: Some(1),
        })
        .collect();

    let instruments = vec![
        InstrumentSpec {
            channel: 0,
            name: "Lead Piano".into(),
            waveform: SynthWaveform::Piano,
            gain: 0.80,
            pan: -0.1,
        },
        InstrumentSpec {
            channel: 1,
            name: "Warm Pad".into(),
            waveform: SynthWaveform::Pad,
            gain: 0.45,
            pan: 0.15,
        },
        InstrumentSpec {
            channel: 2,
            name: "Deep Bass".into(),
            waveform: SynthWaveform::Bass,
            gain: 0.62,
            pan: 0.0,
        },
    ];

    let is_zh = language.to_ascii_lowercase().starts_with("zh") || detect_language(title) == "zh-CN";
    let lyrics = if is_zh {
        vec![
            SyncedLyricLine { timestamp_ms: 0, text: format!("♪ {title} — 序奏缓缓响起") },
            SyncedLyricLine { timestamp_ms: 1500, text: "星光与节拍在夜色里轻轻交汇".into() },
            SyncedLyricLine { timestamp_ms: 3000, text: "旋律沿着云端回荡向远方".into() },
            SyncedLyricLine { timestamp_ms: 4500, text: format!("在《{title}》的尾声留下一抹余韵") },
        ]
    } else {
        vec![
            SyncedLyricLine { timestamp_ms: 0, text: format!("♪ {title} — opening notes drift in") },
            SyncedLyricLine { timestamp_ms: 1500, text: "Warm chords shimmer across the midnight air".into() },
            SyncedLyricLine { timestamp_ms: 3000, text: "Every beat carries a bright horizon forward".into() },
            SyncedLyricLine { timestamp_ms: 4500, text: format!("Echoes of {title} linger in the quiet") },
        ]
    };

    let duration_secs = ((9.5_f32 * 60.0 / f32::from(bpm.clamp(30, 300))).ceil() as u32).max(4);
    let track = MusicTrack {
        track_id: "trk_1".into(),
        track_number: 1,
        title: title.to_string(),
        duration_secs,
        bpm,
        musical_key: resolved_key.clone(),
        instruments,
        melody_notes,
        chord_progression,
        lyrics,
        audio_media_file_id: None,
        waveform_5bit: Vec::new(),
    };

    MusicPayload {
        artist: artist.to_string(),
        album_title: title.to_string(),
        release_year: Utc::now().format("%Y").to_string().parse::<u16>().unwrap_or(2026),
        primary_genre: resolved_genre,
        mood_tags: vec!["Atmospheric".into(), "Original".into()],
        bpm,
        musical_key: resolved_key,
        time_signature: "4/4".into(),
        total_duration_secs: duration_secs,
        tracks: vec![track],
    }
}

fn append_riff_junk_chunk(wav_bytes: &mut Vec<u8>, tag_payload: &[u8]) {
    if wav_bytes.len() < 12 || &wav_bytes[0..4] != b"RIFF" || &wav_bytes[8..12] != b"WAVE" {
        return;
    }
    if wav_bytes.len() % 2 != 0 {
        wav_bytes.push(0);
    }
    let mut chunk_data = tag_payload.to_vec();
    if chunk_data.len() % 2 != 0 {
        chunk_data.push(0);
    }
    let chunk_size = u32::try_from(chunk_data.len()).unwrap_or(u32::MAX);
    wav_bytes.extend_from_slice(b"JUNK");
    wav_bytes.extend_from_slice(&chunk_size.to_le_bytes());
    wav_bytes.extend_from_slice(&chunk_data);
    let new_riff_size = u32::try_from(wav_bytes.len().saturating_sub(8)).unwrap_or(u32::MAX);
    wav_bytes[4..8].copy_from_slice(&new_riff_size.to_le_bytes());
}

pub(crate) async fn synthesize_and_store_music_tracks(
    db: &ServerDatabase,
    cas: Option<&CasStore>,
    owner_uid: UserId,
    payload: &mut MusicPayload,
    now_ms: i64,
) -> Vec<String> {
    let mut stored_ids = Vec::new();
    for track in &mut payload.tracks {
        if !(30..=300).contains(&track.bpm) {
            continue;
        }
        if track.melody_notes.is_empty() && track.chord_progression.is_empty() {
            continue;
        }
        let synth_input = SynthTrackInput {
            track_id: track.track_id.clone(),
            title: track.title.clone(),
            bpm: track.bpm,
            sample_rate: 44_100,
            channels: 1,
            duration_secs_hint: Some(track.duration_secs.clamp(3, 12) as f32),
            instruments: track
                .instruments
                .iter()
                .map(|inst| SynthInstrument {
                    channel: inst.channel,
                    name: inst.name.clone(),
                    waveform: map_synth_waveform(inst.waveform),
                    gain: inst.gain,
                    pan: inst.pan,
                    adsr: None,
                })
                .collect(),
            melody_notes: track
                .melody_notes
                .iter()
                .map(|n| SynthNote {
                    midi_note: n.midi_note,
                    start_beat: n.start_beat,
                    duration_beats: n.duration_beats,
                    velocity: n.velocity,
                    channel: n.channel,
                })
                .collect(),
            chord_progression: track
                .chord_progression
                .iter()
                .map(|c| SynthChord {
                    symbol: c.symbol.clone(),
                    start_beat: c.start_beat,
                    duration_beats: c.duration_beats,
                    velocity: c.velocity,
                    channel: c.channel,
                })
                .collect(),
            lyrics: track
                .lyrics
                .iter()
                .map(|l| SynthLyricLine {
                    timestamp_ms: l.timestamp_ms,
                    text: l.text.clone(),
                })
                .collect(),
        };
        let Ok(synth) = synthesize_music_track_wav(&synth_input) else {
            continue;
        };
        track.waveform_5bit = synth.waveform_5bit.clone();
        let synthesized_secs = u32::try_from(synth.duration_ms.saturating_add(999) / 1000)
            .unwrap_or(u32::MAX)
            .max(3);
        if track.duration_secs < 3 || track.duration_secs < synthesized_secs {
            track.duration_secs = synthesized_secs;
        }
        if let Some(cas_store) = cas {
            let mut wav_bytes = synth.wav_bytes;
            // Unconditionally append an even-aligned RIFF JUNK chunk tagged with owner_uid + UUID
            // BEFORE hashing/persisting in CAS so concurrent multi-user creations never collide
            // on media_id (eliminating TOCTOU uploader_id overwrites) and same-owner creations
            // never clobber chat_id on pre-existing chat media_files rows.
            let unique_tag = format!("owner:{}:{}", owner_uid.0, Uuid::new_v4().simple());
            append_riff_junk_chunk(&mut wav_bytes, unique_tag.as_bytes());
            if let Ok(media_id) = cas_store.store_blob(&wav_bytes).await {
                let meta = MediaMetadataDto {
                    media_id: media_id.clone(),
                    filename: format!("{}.wav", track.track_id),
                    mime_type: "audio/wav".to_string(),
                    size_bytes: wav_bytes.len() as u64,
                    sha256: media_id.clone(),
                    width: None,
                    height: None,
                    duration_seconds: Some(track.duration_secs),
                    waveform: Some(track.waveform_5bit.clone()),
                    thumbnail_data_url: None,
                    is_voice: false,
                    is_video_note: false,
                    created_at_ms: now_ms,
                };
                if db.save_media_file(&meta, owner_uid, None).is_ok() {
                    track.audio_media_file_id = Some(media_id.clone());
                    stored_ids.push(media_id);
                }
            }
        }
    }
    let total_secs: u32 = payload
        .tracks
        .iter()
        .fold(0u32, |acc, t| acc.saturating_add(t.duration_secs));
    if total_secs > 0 {
        payload.total_duration_secs = total_secs;
    }
    stored_ids
}

pub(crate) fn default_engine_for_category(category: GameCategory) -> GameEngineKind {
    match category {
        GameCategory::Puzzle | GameCategory::Strategy => GameEngineKind::GridPuzzle,
        GameCategory::Trivia => GameEngineKind::TriviaQuiz,
        GameCategory::TextAdventure => GameEngineKind::InteractiveFiction,
        GameCategory::Arcade | GameCategory::Rhythm => GameEngineKind::ArcadeDodge,
        GameCategory::CardMatch => GameEngineKind::CardMatch,
    }
}

pub(crate) fn default_category_for_engine(engine: GameEngineKind) -> GameCategory {
    match engine {
        GameEngineKind::GridPuzzle => GameCategory::Puzzle,
        GameEngineKind::TriviaQuiz => GameCategory::Trivia,
        GameEngineKind::InteractiveFiction => GameCategory::TextAdventure,
        GameEngineKind::ArcadeDodge | GameEngineKind::SnakeGrid => GameCategory::Arcade,
        GameEngineKind::CardMatch => GameCategory::CardMatch,
        GameEngineKind::MiniAppHtml => GameCategory::Arcade,
    }
}

pub(crate) fn build_default_game_spec(
    title: &str,
    summary: &str,
    engine_kind: GameEngineKind,
    language: &str,
) -> GameSpec {
    let is_zh = language.to_ascii_lowercase().starts_with("zh") || detect_language(title) == "zh-CN";
    match engine_kind {
        GameEngineKind::GridPuzzle => GameSpec::GridPuzzle {
            rows: 3,
            cols: 3,
            levels: vec![
                GridPuzzleLevel {
                    level_id: "lvl_1".into(),
                    title: if is_zh { format!("{title} · 初级阵列") } else { format!("{title} · Starter Grid") },
                    initial_grid: vec![vec![1, 2, 3], vec![4, 0, 5], vec![7, 8, 6]],
                    target_grid: Some(vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 0]]),
                    max_moves: Some(12),
                    hint: Some(if is_zh { "先将 5 左移，再把 6 上移归位。".into() } else { "Slide 5 left, then move 6 upward into place.".into() }),
                },
                GridPuzzleLevel {
                    level_id: "lvl_2".into(),
                    title: if is_zh { format!("{title} · 进阶星阵") } else { format!("{title} · Star Matrix") },
                    initial_grid: vec![vec![1, 2, 3], vec![0, 4, 6], vec![7, 5, 8]],
                    target_grid: Some(vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 0]]),
                    max_moves: Some(16),
                    hint: Some(if is_zh { "沿着中间行顺时针整理方块。".into() } else { "Rotate tiles around the center row.".into() }),
                },
            ],
        },
        GameEngineKind::TriviaQuiz => GameSpec::TriviaQuiz {
            time_limit_secs: Some(90),
            questions: if is_zh {
                vec![
                    TriviaQuestion {
                        question_id: "q_1".into(),
                        prompt: format!("在《{title}》的挑战中，以下哪项最有助于获得高分？"),
                        choices: vec!["连续准确作答保持连击".into(), "随机跳过所有题目".into(), "无视时间限制".into()],
                        correct_index: 0,
                        explanation: Some("连续答对可以获得额外连击加分并解锁成就。".into()),
                        points: 100,
                    },
                    TriviaQuestion {
                        question_id: "q_2".into(),
                        prompt: "探索深空轨道时，卫星绕行星稳定运行主要依赖哪种力？".into(),
                        choices: vec!["万有引力".into(), "摩擦力".into(), "浮力".into()],
                        correct_index: 0,
                        explanation: Some("万有引力提供了轨道运行所需的向心力。".into()),
                        points: 120,
                    },
                    TriviaQuestion {
                        question_id: "q_3".into(),
                        prompt: "光在真空中传播一年的距离被称为什么？".into(),
                        choices: vec!["光年".into(), "分贝".into(), "赫兹".into()],
                        correct_index: 0,
                        explanation: Some("光年是天文学中常用的长度单位。".into()),
                        points: 150,
                    },
                ]
            } else {
                vec![
                    TriviaQuestion {
                        question_id: "q_1".into(),
                        prompt: format!("In {title}, what is the surest strategy to top the leaderboard?"),
                        choices: vec!["Answer accurately in a streak".into(), "Skip every question".into(), "Ignore clues".into()],
                        correct_index: 0,
                        explanation: Some("Accurate streaks maximize your score and unlock achievements.".into()),
                        points: 100,
                    },
                    TriviaQuestion {
                        question_id: "q_2".into(),
                        prompt: "Which force keeps planets in stable orbits around their star?".into(),
                        choices: vec!["Gravity".into(), "Friction".into(), "Buoyancy".into()],
                        correct_index: 0,
                        explanation: Some("Gravitational attraction provides the centripetal force for orbits.".into()),
                        points: 120,
                    },
                    TriviaQuestion {
                        question_id: "q_3".into(),
                        prompt: "What unit measures the distance light travels in one vacuum year?".into(),
                        choices: vec!["Light-year".into(), "Decibel".into(), "Hertz".into()],
                        correct_index: 0,
                        explanation: Some("A light-year is an astronomical unit of distance.".into()),
                        points: 150,
                    },
                ]
            },
        },
        GameEngineKind::InteractiveFiction => GameSpec::InteractiveFiction {
            start_node_id: "start".into(),
            nodes: vec![
                StoryNode {
                    node_id: "start".into(),
                    title: title.to_string(),
                    body: format!("{summary} Two paths open ahead of you."),
                    choices: vec![
                        StoryChoice {
                            label: if is_zh { "沿着发光星门前进".into() } else { "Step through the luminous gate".into() },
                            target_node_id: "victory".into(),
                            score_delta: 150,
                        },
                        StoryChoice {
                            label: if is_zh { "先检查观测日志".into() } else { "Inspect the observatory log first".into() },
                            target_node_id: "clue".into(),
                            score_delta: 50,
                        },
                    ],
                    is_ending: false,
                    ending_badge: None,
                },
                StoryNode {
                    node_id: "clue".into(),
                    title: if is_zh { "星图线索".into() } else { "Star Chart Clue".into() },
                    body: if is_zh { "日志揭示了隐藏通道的坐标。".into() } else { "The log reveals the coordinates to the hidden passage.".into() },
                    choices: vec![StoryChoice {
                        label: if is_zh { "启动跃迁核心".into() } else { "Engage the warp core".into() },
                        target_node_id: "victory".into(),
                        score_delta: 200,
                    }],
                    is_ending: false,
                    ending_badge: None,
                },
                StoryNode {
                    node_id: "victory".into(),
                    title: if is_zh { "抵达新纪元".into() } else { "New Horizon Reached".into() },
                    body: if is_zh { format!("你成功完成了《{title}》的探索旅程！") } else { format!("You completed the adventure of {title}!") },
                    choices: Vec::new(),
                    is_ending: true,
                    ending_badge: Some("Explorer".into()),
                },
            ],
        },
        GameEngineKind::ArcadeDodge => GameSpec::ArcadeDodge {
            tick_ms: 33,
            arena_width: 480,
            arena_height: 320,
            player_speed: 220.0,
            obstacle_speed: 150.0,
            lives: 3,
            target_score: 500,
            theme_accent_hex: Some("#6366F1".into()),
        },
        GameEngineKind::SnakeGrid => GameSpec::SnakeGrid {
            grid_width: 16,
            grid_height: 12,
            tick_ms: 140,
            wrap_walls: false,
            target_score: 200,
        },
        GameEngineKind::CardMatch => GameSpec::CardMatch {
            pairs: vec![
                CardPair { pair_id: "p1".into(), label: "Sun".into(), symbol: "☀️".into(), accent_hex: Some("#F59E0B".into()) },
                CardPair { pair_id: "p2".into(), label: "Moon".into(), symbol: "🌙".into(), accent_hex: Some("#6366F1".into()) },
                CardPair { pair_id: "p3".into(), label: "Star".into(), symbol: "⭐".into(), accent_hex: Some("#EAB308".into()) },
                CardPair { pair_id: "p4".into(), label: "Comet".into(), symbol: "☄️".into(), accent_hex: Some("#EC4899".into()) },
            ],
            time_limit_secs: Some(90),
        },
        GameEngineKind::MiniAppHtml => GameSpec::MiniAppHtml {
            bot_username: CREATOR_STUDIO_BOT_USERNAME.to_string(),
            short_name: "arcade".into(),
            html_bundle_media_id: None,
        },
    }
}

pub(crate) fn build_default_game_payload(
    title: &str,
    summary: &str,
    developer: &str,
    category: GameCategory,
    engine_kind: GameEngineKind,
    language: &str,
) -> GamePayload {
    let is_zh = language.to_ascii_lowercase().starts_with("zh") || detect_language(title) == "zh-CN";
    let controls_hint = match engine_kind {
        GameEngineKind::GridPuzzle => if is_zh { "方向键或点击方块滑动归位" } else { "Arrow keys or click tiles to slide" },
        GameEngineKind::TriviaQuiz => if is_zh { "点击选项作答，连击获得更高分" } else { "Select the right choice to build streaks" },
        GameEngineKind::InteractiveFiction => if is_zh { "选择剧情分支解锁结局" } else { "Choose story branches to reach endings" },
        GameEngineKind::ArcadeDodge => if is_zh { "方向键或 WASD 躲避障碍并收集能量" } else { "Arrow keys / WASD to dodge obstacles" },
        GameEngineKind::SnakeGrid => if is_zh { "方向键控制转向，吃掉目标得分" } else { "Arrow keys to steer and collect targets" },
        GameEngineKind::CardMatch => if is_zh { "翻开两张相同卡牌完成配对" } else { "Flip matching card pairs before time runs out" },
        GameEngineKind::MiniAppHtml => if is_zh { "使用触控或键盘游玩" } else { "Use touch or keyboard controls" },
    };
    let rules_markdown = if is_zh {
        format!("### 《{title}》玩法规则\n\n{summary}\n\n- 操作提示：{controls_hint}\n- 完成挑战后自动记录云端最高分与成就。")
    } else {
        format!("### {title} Rules\n\n{summary}\n\n- Controls: {controls_hint}\n- Complete rounds to submit your best score to the cloud leaderboard.")
    };
    let spec = build_default_game_spec(title, summary, engine_kind, language);
    GamePayload {
        developer: developer.to_string(),
        category,
        controls_hint: controls_hint.to_string(),
        rules_markdown,
        engine_kind: spec.engine_kind(),
        supports_leaderboard: true,
        estimated_session_mins: 5,
        spec,
        achievements: vec![
            GameAchievementDef {
                achievement_id: "ach_first_play".into(),
                title: if is_zh { "初露锋芒".into() } else { "First Steps".into() },
                description: if is_zh { "完成首局挑战".into() } else { "Complete your first round".into() },
                icon_emoji: "🏅".into(),
                target_score: Some(100),
            },
            GameAchievementDef {
                achievement_id: "ach_master".into(),
                title: if is_zh { "榜单大师".into() } else { "High Scorer".into() },
                description: if is_zh { "达到目标高分".into() } else { "Reach the target high score".into() },
                icon_emoji: "🏆".into(),
                target_score: Some(300),
            },
        ],
    }
}

pub struct CreatorBotReply {
    pub card: GenerativeCard,
    pub keyboard: Vec<Vec<InlineKeyboardButton>>,
}

pub fn send_creator_bot_reply_in_topic(
    state: &Arc<AppState>,
    chat_id: ChatId,
    bot_user_id: UserId,
    reply: CreatorBotReply,
    topic_id: Option<TopicId>,
) -> gpui_telegram_core::error::Result<MsgId> {
    let mut bot_msg = MessageItem::new(
        MsgId(0),
        chat_id,
        bot_user_id,
        PeerId::Chat(chat_id),
        MessageContent::GenerativeCard(reply.card),
    );
    bot_msg.topic_id = topic_id;
    bot_msg.flags.is_out = false;
    if !reply.keyboard.is_empty() {
        bot_msg.inline_keyboard = Some(reply.keyboard.clone());
    }
    let pts_update = state.message_pipeline.process_incoming_message(bot_msg)?;
    let mut saved_id = MsgId(0);
    if let UpdateEvent::NewMessage(saved) = &pts_update.event {
        saved_id = saved.id;
        if !reply.keyboard.is_empty() {
            if let Ok(kb_json) = serde_json::to_string(&reply.keyboard) {
                let _ = state
                    .db
                    .save_message_extras(chat_id, saved.id, Some(&kb_json), None);
            }
        }
    }
    broadcast_chat_pts(state, chat_id, pts_update);
    Ok(saved_id)
}

fn new_card_id(prefix: &str) -> String {
    format!("card_{prefix}_{}", &Uuid::new_v4().simple().to_string()[..10])
}

fn creation_deep_link(creation_id: &str) -> String {
    AppleDeepLink::for_creation(AppleDeepLinkSection::Creations, creation_id).to_uri()
}

fn kind_icon(kind: CreationKind) -> &'static str {
    match kind {
        CreationKind::Book => "📖",
        CreationKind::Music => "🎵",
        CreationKind::Game => "🎮",
        CreationKind::App => "✨",
    }
}

/// Strips raw user query echoes from `PublicCatalogSearchService::recommend` reasons
/// (`Matches "<raw_query>" • <rating>`) so adversarial probe tokens are never echoed back.
pub(crate) fn sanitize_recommendation_reason(reason: &str) -> String {
    if reason.starts_with("Matches \"") {
        if let Some(sep_idx) = reason.rfind(" • ") {
            return format!("Matched Public Catalog{}", &reason[sep_idx..]);
        }
        return "Matched Public Catalog".to_string();
    }
    reason.to_string()
}

pub(crate) fn build_creator_studio_welcome_reply() -> CreatorBotReply {
    let card = GenerativeCard {
        card_id: new_card_id("welcome"),
        title: "Creator Studio · 对话式创作与发布助手".into(),
        subtitle: Some("直接在消息里对话即可创作图书、音乐、游戏与应用，并完成编辑、提交审核、上架与管理。".into()),
        layout: GenerativeCardLayout::VerticalStack,
        components: vec![
            GenerativeComponent::Badge {
                label: "Official Creator Studio".into(),
                color: "#7C3AED".into(),
            },
            GenerativeComponent::Text {
                text: "常用命令 / Commands:\n\
• /newbook <标题> | <简介> — 创作多章节图书\n\
• /newmusic <标题> | <简介> — 创作含音频合成的音乐\n\
• /newgame <标题> | <简介> — 创作可游玩互动迷你游戏\n\
• /newapp <标题> | <简介> — 创作应用并上架 App Store\n\
• /mycreations [book|music|game|app] — 查看并管理我的作品\n\
• /editwork <creation_id> title=<新标题> [summary=<新简介>] — 编辑作品\n\
• /submitwork <creation_id> — 推送审核上架\n\
• /withdraw <creation_id> · /unpublish <creation_id> · /deletework <creation_id>\n\
• /recommend [books|music|games|appstore|all] [关键词] — 推荐已公开上架作品"
                    .into(),
                bold: false,
                color: None,
            },
            GenerativeComponent::Text {
                text: "也可以直接用中英文自然语言告诉我你想创作或发布的图书、音乐、游戏或应用！".into(),
                bold: false,
                color: None,
            },
        ],
        status: Some("ready".into()),
    };
    let keyboard = vec![vec![
        InlineKeyboardButton {
            text: "📚 我的作品 (/mycreations)".into(),
            callback_data: Some("/mycreations".into()),
            url: None,
            web_app_url: None,
        },
        InlineKeyboardButton {
            text: "✨ 热门公开推荐 (/recommend)".into(),
            callback_data: Some("/recommend".into()),
            url: None,
            web_app_url: None,
        },
    ]];
    CreatorBotReply { card, keyboard }
}

fn parse_kind_filter_token(token: &str) -> Option<Option<CreationKind>> {
    let norm = token.trim().to_ascii_lowercase();
    match norm.as_str() {
        "all" | "全部" => Some(None),
        "book" | "books" | "图书" | "书" => Some(Some(CreationKind::Book)),
        "music" | "音乐" | "歌曲" => Some(Some(CreationKind::Music)),
        "game" | "games" | "游戏" => Some(Some(CreationKind::Game)),
        "app" | "apps" | "appstore" | "应用" => Some(Some(CreationKind::App)),
        _ => None,
    }
}

fn parse_title_and_summary(raw_args: &str) -> Option<(String, Option<String>)> {
    let trimmed = raw_args.trim();
    if trimmed.is_empty() {
        return None;
    }
    let split_pos = trimmed
        .find('|')
        .or_else(|| trimmed.find('｜'));
    let (raw_title, raw_summary) = if let Some(idx) = split_pos {
        let sep_len = if trimmed[idx..].starts_with('｜') { '｜'.len_utf8() } else { 1 };
        (trimmed[..idx].trim(), Some(trimmed[idx + sep_len..].trim()))
    } else {
        (trimmed, None)
    };
    if raw_title.is_empty() {
        return None;
    }
    let summary = raw_summary.filter(|s| !s.is_empty()).map(ToString::to_string);
    Some((raw_title.to_string(), summary))
}

fn match_editwork_key(s: &str) -> Option<(&'static str, usize)> {
    for (key, pat) in [("title", "title="), ("summary", "summary=")] {
        if s.as_bytes()
            .get(..pat.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(pat.as_bytes()))
        {
            return Some((key, pat.len()));
        }
    }
    None
}

fn is_editwork_field_boundary(after: &str) -> bool {
    if after.is_empty() {
        return true;
    }
    if !after.chars().next().is_some_and(char::is_whitespace) {
        return false;
    }
    let trimmed = after.trim_start();
    trimmed.is_empty() || match_editwork_key(trimmed).is_some()
}

fn is_n_contraction_open(text: &str, idx: usize) -> bool {
    let Some(rest) = text.get(idx..) else {
        return false;
    };
    let mut chars = rest.chars();
    if chars.next() != Some('\'') {
        return false;
    }
    if !matches!(chars.next(), Some('n' | 'N')) {
        return false;
    }
    if chars.next() != Some('\'') {
        return false;
    }
    let followed_ok = chars
        .next()
        .is_some_and(|c| c.is_whitespace() || c.is_alphanumeric());
    let preceded_alnum = text[..idx]
        .chars()
        .last()
        .is_some_and(char::is_alphanumeric);
    followed_ok || preceded_alnum
}

fn is_n_contraction_close(text: &str, idx: usize) -> bool {
    idx >= 2 && text.is_char_boundary(idx - 2) && is_n_contraction_open(text, idx - 2)
}

fn has_split_single_quoted_segments(inner: &str) -> bool {
    let mut open_depth = 0usize;
    let mut saw_unmatched_closer = false;
    let mut saw_field_eq_opener = false;
    let mut escaped = false;
    for (idx, ch) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch != '\'' {
            continue;
        }
        let after = &inner[idx + 1..];
        if after.chars().next().is_some_and(char::is_whitespace) {
            let trimmed_after = after.trim_start();
            if let Some((_, key_len)) = match_editwork_key(trimmed_after) {
                if trimmed_after[key_len..].starts_with('\'') {
                    return true;
                }
            }
        }
        if is_n_contraction_open(inner, idx) || is_n_contraction_close(inner, idx) {
            continue;
        }
        let prev = inner[..idx].chars().last();
        let next = after.chars().next();
        let is_opener = prev.is_none_or(|c| !c.is_alphanumeric())
            && next.is_some_and(|c| !c.is_whitespace() && !c.is_ascii_digit());
        let is_closer = prev.is_some_and(|c| !c.is_whitespace())
            && next.is_none_or(|c| !c.is_alphanumeric());
        if is_opener && !is_closer {
            open_depth += 1;
            if prev == Some('=') {
                for pat in ["title=", "summary="] {
                    if idx >= pat.len() && inner.is_char_boundary(idx - pat.len()) {
                        let key_start = idx - pat.len();
                        let before_key_ok = key_start == 0
                            || inner[..key_start]
                                .chars()
                                .last()
                                .is_some_and(char::is_whitespace);
                        if before_key_ok && inner[key_start..idx].eq_ignore_ascii_case(pat) {
                            saw_field_eq_opener = true;
                        }
                    }
                }
            }
        } else if is_closer && !is_opener {
            if open_depth > 0 {
                open_depth -= 1;
            } else {
                saw_unmatched_closer = true;
            }
        }
    }
    open_depth > 0 && (saw_unmatched_closer || saw_field_eq_opener)
}

fn parse_quoted_editwork_value(rest: &str, val_start: usize) -> Option<(String, usize)> {
    let tail = &rest[val_start..];
    let first = tail.chars().next()?;
    match first {
        '"' => {
            let inner_start = val_start + 1;
            let mut out = String::new();
            let mut escaped = false;
            for (rel, ch) in rest[inner_start..].char_indices() {
                if escaped {
                    if ch == '"' || ch == '\\' {
                        out.push(ch);
                    } else {
                        out.push('\\');
                        out.push(ch);
                    }
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == '"' {
                    let after_close = inner_start + rel + 1;
                    if is_editwork_field_boundary(&rest[after_close..]) {
                        return Some((out.trim().to_string(), after_close));
                    }
                    return None;
                }
                out.push(ch);
            }
            None
        }
        '\'' => {
            let inner_start = val_start + 1;
            let mut out = String::new();
            let mut escaped = false;
            let mut best: Option<(String, usize)> = None;
            for (rel, ch) in rest[inner_start..].char_indices() {
                if escaped {
                    if ch == '\'' || ch == '\\' {
                        out.push(ch);
                    } else {
                        out.push('\\');
                        out.push(ch);
                    }
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == '\'' {
                    let after_close = inner_start + rel + 1;
                    if is_editwork_field_boundary(&rest[after_close..])
                        && !has_split_single_quoted_segments(&rest[inner_start..inner_start + rel])
                    {
                        best = Some((out.trim().to_string(), after_close));
                    }
                }
                out.push(ch);
            }
            best
        }
        '“' | '《' => {
            let close_ch = if first == '“' { '”' } else { '》' };
            let inner_start = val_start + first.len_utf8();
            let mut depth = 1usize;
            for (rel, ch) in rest[inner_start..].char_indices() {
                if ch == first {
                    depth += 1;
                } else if ch == close_ch {
                    depth -= 1;
                    if depth == 0 {
                        let after_close = inner_start + rel + close_ch.len_utf8();
                        if is_editwork_field_boundary(&rest[after_close..]) {
                            let inner = &rest[inner_start..inner_start + rel];
                            return Some((inner.trim().to_string(), after_close));
                        }
                        return None;
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_editwork_args(raw_args: &str) -> Result<(String, Option<String>, Option<String>), String> {
    let trimmed = raw_args.trim();
    if trimmed.is_empty() {
        return Err("Usage: /editwork <creation_id> title=<new_title> [summary=<new_summary>]".into());
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let creation_id = parts.next().unwrap_or("").trim().to_string();
    let rest = parts.next().unwrap_or("").trim();
    if creation_id.is_empty() || rest.is_empty() {
        return Err("Usage: /editwork <creation_id> title=<new_title> [summary=<new_summary>]".into());
    }

    let mut pos = {
        let mut found = None;
        let mut prev: Option<char> = None;
        for (idx, ch) in rest.char_indices() {
            if (idx == 0 || prev.is_some_and(char::is_whitespace))
                && match_editwork_key(&rest[idx..]).is_some()
            {
                found = Some(idx);
                break;
            }
            prev = Some(ch);
        }
        match found {
            Some(idx) => idx,
            None => return Err("Specify title=<new_title> and/or summary=<new_summary>".into()),
        }
    };

    let mut title_val: Option<String> = None;
    let mut summary_val: Option<String> = None;
    while pos < rest.len() {
        let ws_len = rest[pos..]
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(rest.len() - pos);
        pos += ws_len;
        if pos >= rest.len() {
            break;
        }
        let Some((key, key_len)) = match_editwork_key(&rest[pos..]) else {
            break;
        };
        let val_start = pos + key_len;
        let (cleaned, next_pos) =
            if let Some((quoted_val, after_q)) = parse_quoted_editwork_value(rest, val_start) {
                (quoted_val, after_q)
            } else {
                let mut val_end = rest.len();
                let mut prev_ch: Option<char> = None;
                for (rel, ch) in rest[val_start..].char_indices() {
                    if prev_ch.is_some_and(char::is_whitespace)
                        && match_editwork_key(&rest[val_start + rel..]).is_some()
                    {
                        val_end = val_start + rel;
                        break;
                    }
                    prev_ch = Some(ch);
                }
                (rest[val_start..val_end].trim().to_string(), val_end)
            };
        if !cleaned.is_empty() {
            if key == "title" {
                title_val = Some(cleaned);
            } else if key == "summary" {
                summary_val = Some(cleaned);
            }
        }
        pos = next_pos;
    }

    if title_val.is_none() && summary_val.is_none() {
        return Err("Provide a non-empty value for title= or summary=".into());
    }
    Ok((creation_id, title_val, summary_val))
}

async fn create_bot_work_and_reply(
    state: &Arc<AppState>,
    chat_id: ChatId,
    sender_id: UserId,
    kind: CreationKind,
    title: &str,
    raw_summary: Option<&str>,
    preferred_game_engine: Option<GameEngineKind>,
) -> CreatorBotReply {
    let now_ms = Utc::now().timestamp_millis();
    let author = resolve_owner_display_name(&state.db, sender_id);
    let summary = ensure_bot_summary(title, raw_summary, kind);
    let language = detect_language(&format!("{title} {summary}"));
    let cover = default_cover_for_kind(kind);

    let (genres, tags, payload) = match kind {
        CreationKind::Book => {
            let book = build_default_book_payload(title, &summary, &author, &language);
            (
                vec!["Fiction".into(), "Originals".into()],
                vec!["creator-studio".into(), "book".into()],
                CreationPayload::Book(book),
            )
        }
        CreationKind::Music => {
            let mut music = build_default_music_payload(
                title,
                &summary,
                &author,
                110,
                None,
                Some("Electronic"),
                &language,
                (sender_id.0 ^ (now_ms as u64)).wrapping_add(17),
            );
            let _ = synthesize_and_store_music_tracks(
                &state.db,
                Some(state.cas.as_ref()),
                sender_id,
                &mut music,
                now_ms,
            )
            .await;
            (
                vec!["Electronic".into(), "Ambient".into()],
                vec!["creator-studio".into(), "music".into()],
                CreationPayload::Music(music),
            )
        }
        CreationKind::Game => {
            let engine = preferred_game_engine.unwrap_or_else(|| {
                let lower = format!("{title} {summary}").to_ascii_lowercase();
                if lower.contains("quiz") || lower.contains("trivia") || lower.contains("问答") {
                    GameEngineKind::TriviaQuiz
                } else if lower.contains("snake") || lower.contains("贪吃蛇") {
                    GameEngineKind::SnakeGrid
                } else if lower.contains("card") || lower.contains("记忆") || lower.contains("卡牌") {
                    GameEngineKind::CardMatch
                } else if lower.contains("story") || lower.contains("文字") || lower.contains("冒险") {
                    GameEngineKind::InteractiveFiction
                } else if lower.contains("dodge") || lower.contains("躲避") || lower.contains("arcade") {
                    GameEngineKind::ArcadeDodge
                } else {
                    GameEngineKind::GridPuzzle
                }
            });
            let category = default_category_for_engine(engine);
            let game = build_default_game_payload(title, &summary, &author, category, engine, &language);
            (
                vec![category.as_str().to_string(), "Arcade".into()],
                vec!["creator-studio".into(), "game".into()],
                CreationPayload::Game(game),
            )
        }
        CreationKind::App => {
            let app_payload = AppStoreListingPayload {
                developer_name: author.clone(),
                version_string: "1.0.0".into(),
                release_notes: "Initial release".into(),
                category: "Utilities".into(),
                long_description: summary.clone(),
                screenshots: Vec::new(),
                size_label: "Cloud App".into(),
                price_label: "Free".into(),
                permissions: Vec::new(),
                linked_miniapp: None,
                ui_schema_json: Some(serde_json::json!({
                    "version": 1,
                    "title": title,
                    "summary": summary,
                    "components": [
                        {
                            "type": "header",
                            "title": title,
                            "subtitle": summary
                        },
                        {
                            "type": "text",
                            "text": summary
                        }
                    ]
                })),
            };
            (
                vec!["Utilities".into(), "App".into()],
                vec!["creator-studio".into(), "app".into()],
                CreationPayload::App(app_payload),
            )
        }
    };

    let req = CreateCreationRequest {
        kind,
        title: title.to_string(),
        subtitle: format!("Created with @{}", CREATOR_STUDIO_BOT_USERNAME),
        summary: summary.clone(),
        author_display_name: author,
        language,
        genres,
        tags,
        cover,
        age_rating: "4+".into(),
        visibility: CreationVisibility::Private,
        creator_bot_user_id: Some(CREATOR_STUDIO_BOT_USER_ID),
        creator_chat_id: Some(chat_id.0),
        payload,
    };

    match state.db.create_user_creation(sender_id, &req, now_ms) {
        Ok(dto) => {
            emit_user_creation_changed_to_hub(&state.gateway_hub, &dto);
            let deep_link = creation_deep_link(&dto.creation_id);
            let kind_label = match dto.kind {
                CreationKind::Book => "图书 / Book",
                CreationKind::Music => "音乐 / Music",
                CreationKind::Game => "游戏 / Game",
                CreationKind::App => "应用 / App",
            };
            let card = GenerativeCard {
                card_id: new_card_id("created"),
                title: format!("{} {} ({})", kind_icon(dto.kind), dto.title, dto.creation_id),
                subtitle: Some(format!("已保存到云端 · {kind_label}")),
                layout: GenerativeCardLayout::VerticalStack,
                components: vec![
                    GenerativeComponent::Badge {
                        label: format!("draft · rev {}", dto.cloud_revision),
                        color: "#16A34A".into(),
                    },
                    GenerativeComponent::Text {
                        text: format!(
                            "作品 ID: {}\n简介: {}\n云端版本: v{} (rev {})\n直达链接: {}",
                            dto.creation_id, dto.summary, dto.version, dto.cloud_revision, deep_link
                        ),
                        bold: false,
                        color: None,
                    },
                    GenerativeComponent::Button {
                        id: format!("open_{}", dto.creation_id),
                        label: "预览作品".into(),
                        action_payload: deep_link.clone(),
                    },
                ],
                status: Some("created".into()),
            };
            let keyboard = vec![
                vec![
                    InlineKeyboardButton {
                        text: format!("{} 打开云端作品", kind_icon(dto.kind)),
                        callback_data: None,
                        url: Some(deep_link),
                        web_app_url: None,
                    },
                    InlineKeyboardButton {
                        text: "🚀 提交审核上架".into(),
                        callback_data: Some(format!("/submitwork {}", dto.creation_id)),
                        url: None,
                        web_app_url: None,
                    },
                ],
                vec![InlineKeyboardButton {
                    text: "📚 查看我的全部作品".into(),
                    callback_data: Some("/mycreations".into()),
                    url: None,
                    web_app_url: None,
                }],
            ];
            CreatorBotReply { card, keyboard }
        }
        Err(err) => build_error_card("Creation Failed", &format_creation_op_error(&err)),
    }
}

fn build_error_card(title: &str, message: &str) -> CreatorBotReply {
    CreatorBotReply {
        card: GenerativeCard {
            card_id: new_card_id("err"),
            title: title.to_string(),
            subtitle: None,
            layout: GenerativeCardLayout::VerticalStack,
            components: vec![
                GenerativeComponent::Badge {
                    label: "error".into(),
                    color: "#DC2626".into(),
                },
                GenerativeComponent::Text {
                    text: message.to_string(),
                    bold: false,
                    color: None,
                },
            ],
            status: Some("error".into()),
        },
        keyboard: Vec::new(),
    }
}

fn format_creation_op_error(err: &CreationOpError) -> String {
    match err {
        CreationOpError::NotFound => "Creation not found.".to_string(),
        CreationOpError::Forbidden(_) => {
            "Permission denied: you can only manage your own creations.".to_string()
        }
        CreationOpError::RevisionConflict { expected, current } => {
            format!("Revision conflict: expected rev {expected}, current rev {current}.")
        }
        CreationOpError::InvalidState { message, .. } => message.clone(),
        CreationOpError::ValidationFailed(errors) => {
            format!("Validation failed:\n- {}", errors.join("\n- "))
        }
        CreationOpError::BadRequest { message, .. } => message.clone(),
        CreationOpError::Db(e) => format!("Storage error: {e}"),
    }
}

fn handle_mycreations_reply(
    state: &Arc<AppState>,
    sender_id: UserId,
    raw_args: &str,
) -> CreatorBotReply {
    let trimmed = raw_args.trim();
    let kind_filter = if trimmed.is_empty() {
        None
    } else {
        match parse_kind_filter_token(trimmed) {
            Some(k) => k,
            None => {
                return build_error_card(
                    "Invalid Creation Kind",
                    "Use /mycreations [book|music|game|app|all]",
                );
            }
        }
    };

    match state.db.list_my_creations(sender_id, kind_filter, None, None) {
        Ok(list) => {
            if list.items.is_empty() {
                return CreatorBotReply {
                    card: GenerativeCard {
                        card_id: new_card_id("my_empty"),
                        title: "我的云端作品 (0)".into(),
                        subtitle: Some("你还没有任何云端作品。使用 /newbook、/newmusic 或 /newgame 开始创作！".into()),
                        layout: GenerativeCardLayout::VerticalStack,
                        components: vec![GenerativeComponent::Text {
                            text: "示例: /newbook 星海航路 | 一部关于深空探索的科幻小说".into(),
                            bold: false,
                            color: None,
                        }],
                        status: Some("empty".into()),
                    },
                    keyboard: vec![vec![InlineKeyboardButton {
                        text: "✨ 浏览公开推荐 (/recommend)".into(),
                        callback_data: Some("/recommend".into()),
                        url: None,
                        web_app_url: None,
                    }]],
                };
            }

            let mut lines = Vec::new();
            let mut components = vec![GenerativeComponent::Badge {
                label: format!("{} items · rev {}", list.items.len(), list.latest_revision),
                color: "#2563EB".into(),
            }];
            let mut keyboard: Vec<Vec<InlineKeyboardButton>> = Vec::new();

            for item in list.items.iter().take(12) {
                let link = creation_deep_link(&item.creation_id);
                lines.push(format!(
                    "{} {} [{} | {} | rev {}] — {} ({})",
                    kind_icon(item.kind),
                    item.title,
                    item.kind.as_str(),
                    item.review_status.as_str(),
                    item.cloud_revision,
                    item.creation_id,
                    link
                ));
                if keyboard.len() < 6 {
                    keyboard.push(vec![InlineKeyboardButton {
                        text: format!("{} {}", kind_icon(item.kind), item.title),
                        callback_data: None,
                        url: Some(link),
                        web_app_url: None,
                    }]);
                }
            }
            components.push(GenerativeComponent::Text {
                text: lines.join("\n"),
                bold: false,
                color: None,
            });
            CreatorBotReply {
                card: GenerativeCard {
                    card_id: new_card_id("my_list"),
                    title: format!("我的云端作品 ({})", list.items.len()),
                    subtitle: Some(format!("最新云端版本 rev {}", list.latest_revision)),
                    layout: GenerativeCardLayout::VerticalStack,
                    components,
                    status: Some("ok".into()),
                },
                keyboard,
            }
        }
        Err(e) => build_error_card("Failed to List Creations", &e.to_string()),
    }
}

fn handle_editwork_reply(
    state: &Arc<AppState>,
    sender_id: UserId,
    raw_args: &str,
) -> CreatorBotReply {
    let (creation_id, title_opt, summary_opt) = match parse_editwork_args(raw_args) {
        Ok(parsed) => parsed,
        Err(msg) => return build_error_card("Invalid /editwork Syntax", &msg),
    };
    let now_ms = Utc::now().timestamp_millis();
    let req = UpdateCreationRequest {
        title: title_opt,
        summary: summary_opt,
        ..UpdateCreationRequest::default()
    };
    match state.db.update_user_creation(&creation_id, sender_id, &req, now_ms) {
        Ok(dto) => {
            emit_user_creation_changed_to_hub(&state.gateway_hub, &dto);
            let link = creation_deep_link(&dto.creation_id);
            CreatorBotReply {
                card: GenerativeCard {
                    card_id: new_card_id("edited"),
                    title: format!("已更新作品：{} ({})", dto.title, dto.creation_id),
                    subtitle: Some(format!(
                        "云端修订 rev {} · 状态 {}",
                        dto.cloud_revision,
                        dto.review_status.as_str()
                    )),
                    layout: GenerativeCardLayout::VerticalStack,
                    components: vec![
                        GenerativeComponent::Badge {
                            label: format!("updated · rev {}", dto.cloud_revision),
                            color: "#2563EB".into(),
                        },
                        GenerativeComponent::Text {
                            text: format!(
                                "作品 ID: {}\n标题: {}\n简介: {}\n直达链接: {}",
                                dto.creation_id, dto.title, dto.summary, link
                            ),
                            bold: false,
                            color: None,
                        },
                        GenerativeComponent::Button {
                            id: format!("open_{}", dto.creation_id),
                            label: "打开作品".into(),
                            action_payload: link.clone(),
                        },
                    ],
                    status: Some("updated".into()),
                },
                keyboard: vec![vec![
                    InlineKeyboardButton {
                        text: "打开作品".into(),
                        callback_data: None,
                        url: Some(link),
                        web_app_url: None,
                    },
                    InlineKeyboardButton {
                        text: "🚀 提交审核上架".into(),
                        callback_data: Some(format!("/submitwork {}", dto.creation_id)),
                        url: None,
                        web_app_url: None,
                    },
                ]],
            }
        }
        Err(err) => build_error_card("Edit Rejected", &format_creation_op_error(&err)),
    }
}

fn handle_submitwork_reply(
    state: &Arc<AppState>,
    sender_id: UserId,
    raw_args: &str,
) -> CreatorBotReply {
    let creation_id = raw_args.split_whitespace().next().unwrap_or("").trim();
    if creation_id.is_empty() {
        return build_error_card("Invalid /submitwork Syntax", "Usage: /submitwork <creation_id>");
    }
    let now_ms = Utc::now().timestamp_millis();
    let req = SubmitReviewRequest {
        visibility: Some(CreationVisibility::Public),
        notes: None,
    };
    match state.db.submit_creation_for_review(creation_id, sender_id, &req, now_ms) {
        Ok(dto) => {
            emit_user_creation_changed_to_hub(&state.gateway_hub, &dto);
            let link = creation_deep_link(&dto.creation_id);
            CreatorBotReply {
                card: GenerativeCard {
                    card_id: new_card_id("submitted"),
                    title: format!("已推送审核上架：{} ({})", dto.title, dto.creation_id),
                    subtitle: Some(format!(
                        "审核状态: {} · rev {}",
                        dto.review_status.as_str(),
                        dto.cloud_revision
                    )),
                    layout: GenerativeCardLayout::VerticalStack,
                    components: vec![
                        GenerativeComponent::Badge {
                            label: "pending_review".into(),
                            color: "#D97706".into(),
                        },
                        GenerativeComponent::Text {
                            text: format!(
                                "作品 {} 已通过结构校验并进入审核队列，审核通过后将公开上架至对应板块。\n直达链接: {}",
                                dto.creation_id, link
                            ),
                            bold: false,
                            color: None,
                        },
                    ],
                    status: Some("pending_review".into()),
                },
                keyboard: vec![vec![
                    InlineKeyboardButton {
                        text: "预览作品".into(),
                        callback_data: None,
                        url: Some(link),
                        web_app_url: None,
                    },
                    InlineKeyboardButton {
                        text: "↩ 撤回审核".into(),
                        callback_data: Some(format!("/withdraw {}", dto.creation_id)),
                        url: None,
                        web_app_url: None,
                    },
                ]],
            }
        }
        Err(CreationOpError::ValidationFailed(errors)) => {
            let mut components = vec![
                GenerativeComponent::Badge {
                    label: "validation_failed".into(),
                    color: "#DC2626".into(),
                },
                GenerativeComponent::Text {
                    text: format!(
                        "作品校验未通过，请先修正以下问题后再提交审核：\n{}",
                        errors
                            .iter()
                            .map(|e| format!("• {e}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                    bold: false,
                    color: None,
                },
            ];
            components.push(GenerativeComponent::Text {
                text: format!("validation_errors: {}", errors.join("; ")),
                bold: false,
                color: None,
            });
            CreatorBotReply {
                card: GenerativeCard {
                    card_id: new_card_id("val_err"),
                    title: "审核校验未通过 (Validation Failed)".into(),
                    subtitle: Some("请修正作品内容后重新执行 /submitwork".into()),
                    layout: GenerativeCardLayout::VerticalStack,
                    components,
                    status: Some("validation_failed".into()),
                },
                keyboard: Vec::new(),
            }
        }
        Err(err) => build_error_card("Submit Rejected", &format_creation_op_error(&err)),
    }
}

fn emit_public_catalog_changed_to_hub(
    hub: &GatewayHub,
    creation_id: &str,
    kind: CreationKind,
    action: Option<&'static str>,
) {
    if let Some(act) = action {
        let event = UpdateEvent::PublicCatalogChanged {
            creation_id: creation_id.to_string(),
            kind,
            action: act.to_string(),
        };
        hub.broadcast_to_all(WsServerFrame::Update(event));
    }
}

fn handle_withdraw_reply(
    state: &Arc<AppState>,
    sender_id: UserId,
    raw_args: &str,
) -> CreatorBotReply {
    let creation_id = raw_args.split_whitespace().next().unwrap_or("").trim();
    if creation_id.is_empty() {
        return build_error_card("Invalid /withdraw Syntax", "Usage: /withdraw <creation_id>");
    }
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .withdraw_creation_review(creation_id, sender_id, now_ms)
    {
        Ok(dto) => {
            emit_user_creation_changed_to_hub(&state.gateway_hub, &dto);
            CreatorBotReply {
                card: GenerativeCard {
                    card_id: new_card_id("withdrawn"),
                    title: format!("已撤回审核：{} ({})", dto.title, dto.creation_id),
                    subtitle: Some(format!("当前状态: {} · rev {}", dto.review_status.as_str(), dto.cloud_revision)),
                    layout: GenerativeCardLayout::VerticalStack,
                    components: vec![GenerativeComponent::Text {
                        text: format!(
                            "作品 {} 已撤回至草稿状态，编辑完成后可随时使用 /submitwork {} 重新提交审核。",
                            dto.creation_id, dto.creation_id
                        ),
                        bold: false,
                        color: None,
                    }],
                    status: Some("draft".into()),
                },
                keyboard: vec![vec![InlineKeyboardButton {
                    text: "🚀 重新提交审核".into(),
                    callback_data: Some(format!("/submitwork {}", dto.creation_id)),
                    url: None,
                    web_app_url: None,
                }]],
            }
        }
        Err(err) => build_error_card("Withdraw Rejected", &format_creation_op_error(&err)),
    }
}

fn handle_unpublish_reply(
    state: &Arc<AppState>,
    sender_id: UserId,
    raw_args: &str,
) -> CreatorBotReply {
    let creation_id = raw_args.split_whitespace().next().unwrap_or("").trim();
    if creation_id.is_empty() {
        return build_error_card("Invalid /unpublish Syntax", "Usage: /unpublish <creation_id>");
    }
    let is_admin = crate::routes::hermes::caller_is_admin(state, sender_id);
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .unpublish_creation(creation_id, sender_id, is_admin, now_ms)
    {
        Ok(res) => {
            emit_user_creation_changed_to_hub(&state.gateway_hub, &res.creation);
            emit_public_catalog_changed_to_hub(
                &state.gateway_hub,
                &res.creation.creation_id,
                res.creation.kind,
                res.catalog_ws_action,
            );
            CreatorBotReply {
                card: GenerativeCard {
                    card_id: new_card_id("unpublished"),
                    title: format!("已下架作品：{} ({})", res.creation.title, res.creation.creation_id),
                    subtitle: Some(format!("当前状态: {}", res.creation.review_status.as_str())),
                    layout: GenerativeCardLayout::VerticalStack,
                    components: vec![GenerativeComponent::Text {
                        text: format!(
                            "作品 {} 已从公开商店下架。如需重新发布，请发送 /submitwork {}。",
                            res.creation.creation_id, res.creation.creation_id
                        ),
                        bold: false,
                        color: None,
                    }],
                    status: Some("unpublished".into()),
                },
                keyboard: vec![vec![InlineKeyboardButton {
                    text: "🚀 重新提交上架".into(),
                    callback_data: Some(format!("/submitwork {}", res.creation.creation_id)),
                    url: None,
                    web_app_url: None,
                }]],
            }
        }
        Err(err) => build_error_card("Unpublish Rejected", &format_creation_op_error(&err)),
    }
}

fn handle_deletework_reply(
    state: &Arc<AppState>,
    sender_id: UserId,
    raw_args: &str,
) -> CreatorBotReply {
    let creation_id = raw_args.split_whitespace().next().unwrap_or("").trim();
    if creation_id.is_empty() {
        return build_error_card("Invalid /deletework Syntax", "Usage: /deletework <creation_id>");
    }
    let now_ms = Utc::now().timestamp_millis();
    match state.db.soft_delete_creation(creation_id, sender_id, now_ms) {
        Ok(res) => {
            emit_user_creation_changed_to_hub(&state.gateway_hub, &res.creation);
            emit_public_catalog_changed_to_hub(
                &state.gateway_hub,
                &res.creation.creation_id,
                res.creation.kind,
                res.catalog_ws_action,
            );
            CreatorBotReply {
                card: GenerativeCard {
                    card_id: new_card_id("deleted"),
                    title: format!("已删除作品：{} ({})", res.creation.title, res.creation.creation_id),
                    subtitle: Some("作品已从云端作品库与公开目录移除".into()),
                    layout: GenerativeCardLayout::VerticalStack,
                    components: vec![GenerativeComponent::Text {
                        text: format!("作品 {} 已删除。", res.creation.creation_id),
                        bold: false,
                        color: None,
                    }],
                    status: Some("deleted".into()),
                },
                keyboard: vec![vec![InlineKeyboardButton {
                    text: "📚 查看剩余作品".into(),
                    callback_data: Some("/mycreations".into()),
                    url: None,
                    web_app_url: None,
                }]],
            }
        }
        Err(err) => build_error_card("Delete Rejected", &format_creation_op_error(&err)),
    }
}

fn handle_reviewqueue_reply(
    state: &Arc<AppState>,
    sender_id: UserId,
) -> CreatorBotReply {
    if !crate::routes::hermes::caller_is_admin(state, sender_id) {
        return build_error_card("Forbidden", "仅管理员可查看和审批待审核队列。");
    }
    let list = match state
        .db
        .list_creation_review_queue(Some(CreationReviewStatus::PendingReview), None)
    {
        Ok(r) => r.items,
        Err(e) => return build_error_card("Review Queue Error", &e.to_string()),
    };
    if list.is_empty() {
        return CreatorBotReply {
            card: GenerativeCard {
                card_id: new_card_id("rq_empty"),
                title: "待审核队列 (0)".into(),
                subtitle: Some("当前没有待审核的作品。".into()),
                layout: GenerativeCardLayout::VerticalStack,
                components: vec![GenerativeComponent::Text {
                    text: "暂无待审核作品。".into(),
                    bold: false,
                    color: None,
                }],
                status: Some("empty".into()),
            },
            keyboard: Vec::new(),
        };
    }
    let mut lines = Vec::new();
    let mut keyboard = Vec::new();
    for item in list.iter().take(6) {
        lines.push(format!(
            "• {} {} ({}) — by {}",
            kind_icon(item.kind),
            item.title,
            item.creation_id,
            item.author_display_name
        ));
        keyboard.push(vec![
            InlineKeyboardButton {
                text: format!("✅ 通过 {}", item.title.chars().take(12).collect::<String>()),
                callback_data: Some(format!("/approve {}", item.creation_id)),
                url: None,
                web_app_url: None,
            },
            InlineKeyboardButton {
                text: "⭐ 精选通过".into(),
                callback_data: Some(format!("/approve {} featured", item.creation_id)),
                url: None,
                web_app_url: None,
            },
        ]);
    }
    CreatorBotReply {
        card: GenerativeCard {
            card_id: new_card_id("rq_list"),
            title: format!("待审核队列 ({})", list.len()),
            subtitle: Some("点击下方按钮可直接通过上架，或发送 /reject <id> | <原因> 退回。".into()),
            layout: GenerativeCardLayout::VerticalStack,
            components: vec![GenerativeComponent::Text {
                text: lines.join("\n"),
                bold: false,
                color: None,
            }],
            status: Some("pending_review".into()),
        },
        keyboard,
    }
}

fn handle_review_decision_reply(
    state: &Arc<AppState>,
    sender_id: UserId,
    raw_args: &str,
    approve: bool,
) -> CreatorBotReply {
    if !crate::routes::hermes::caller_is_admin(state, sender_id) {
        return build_error_card("Forbidden", "仅管理员可执行上架审核操作。");
    }
    let (creation_id, extra) = if let Some(idx) = raw_args.find('|').or_else(|| raw_args.find('｜')) {
        let sep_len = if raw_args[idx..].starts_with('｜') { '｜'.len_utf8() } else { 1 };
        (raw_args[..idx].trim(), Some(raw_args[idx + sep_len..].trim()))
    } else {
        let mut parts = raw_args.splitn(2, char::is_whitespace);
        (
            parts.next().unwrap_or("").trim(),
            parts.next().map(str::trim).filter(|s| !s.is_empty()),
        )
    };
    if creation_id.is_empty() {
        return build_error_card(
            "Invalid Review Syntax",
            if approve {
                "Usage: /approve <creation_id> [featured]"
            } else {
                "Usage: /reject <creation_id> | <reason>"
            },
        );
    }
    let is_unlisted = approve
        && extra.is_some_and(|s| s.eq_ignore_ascii_case("unlisted") || s.contains("仅链接"));
    let decision = if approve {
        ReviewDecision::Approve
    } else {
        ReviewDecision::Reject
    };
    let req = ReviewDecisionRequest {
        decision,
        visibility: if approve {
            Some(if is_unlisted {
                CreationVisibility::Unlisted
            } else {
                CreationVisibility::Public
            })
        } else {
            None
        },
        reason: if approve {
            None
        } else {
            Some(extra.unwrap_or("请完善作品内容后重新提交").to_string())
        },
        notes: extra.map(ToString::to_string),
    };
    let now_ms = Utc::now().timestamp_millis();
    match state
        .db
        .review_creation_decision(creation_id, sender_id, &req, now_ms)
    {
        Ok(res) => {
            emit_user_creation_changed_to_hub(&state.gateway_hub, &res.creation);
            emit_public_catalog_changed_to_hub(
                &state.gateway_hub,
                &res.creation.creation_id,
                res.creation.kind,
                res.catalog_ws_action,
            );
            let action_txt = if approve { "已审核通过并公开上架" } else { "已退回修改" };
            CreatorBotReply {
                card: GenerativeCard {
                    card_id: new_card_id("reviewed"),
                    title: format!("{action_txt}：{} ({})", res.creation.title, res.creation.creation_id),
                    subtitle: Some(format!("状态: {}", res.creation.review_status.as_str())),
                    layout: GenerativeCardLayout::VerticalStack,
                    components: vec![GenerativeComponent::Text {
                        text: format!(
                            "作品 {} 当前状态为 {}。",
                            res.creation.creation_id,
                            res.creation.review_status.as_str()
                        ),
                        bold: false,
                        color: None,
                    }],
                    status: Some(res.creation.review_status.as_str().into()),
                },
                keyboard: Vec::new(),
            }
        }
        Err(err) => build_error_card("Review Decision Rejected", &format_creation_op_error(&err)),
    }
}

fn handle_recommend_reply(state: &Arc<AppState>, raw_args: &str) -> CreatorBotReply {
    let trimmed = raw_args.trim();
    let (kind_filter, query_opt) = if trimmed.is_empty() {
        (None, None)
    } else {
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let first = parts.next().unwrap_or("");
        let rest = parts.next().map(str::trim).filter(|s| !s.is_empty());
        if let Some(parsed_kind) = parse_kind_filter_token(first) {
            (parsed_kind, rest)
        } else {
            (None, Some(trimmed))
        }
    };

    // Layer 4 Zero-Leak Guarantee: query EXCLUSIVELY PublicCatalogSearchService (public_catalog_entries).
    let catalog = PublicCatalogSearchService::new(Arc::clone(&state.db));
    let items: Vec<BotRecommendationItem> = match catalog.recommend(query_opt, kind_filter, None, 6) {
        Ok(resp) => resp.items,
        Err(_) => Vec::new(),
    };

    // ZERO-LEAK NO-ECHO RULE: never echo raw user query strings when 0 items match!
    if items.is_empty() {
        return CreatorBotReply {
            card: GenerativeCard {
                card_id: new_card_id("rec_empty"),
                title: "公开精选推荐 (0)".into(),
                subtitle: Some("暂未找到已公开上架的匹配作品。".into()),
                layout: GenerativeCardLayout::VerticalStack,
                components: vec![GenerativeComponent::Text {
                    text: "仅展示已通过审核并公开上架的图书、音乐、游戏与应用。试试发送 /recommend 浏览全部热门推荐。".into(),
                    bold: false,
                    color: None,
                }],
                status: Some("empty".into()),
            },
            keyboard: vec![vec![InlineKeyboardButton {
                text: "✨ 查看全部公开推荐".into(),
                callback_data: Some("/recommend".into()),
                url: None,
                web_app_url: None,
            }]],
        };
    }

    let mut lines = Vec::with_capacity(items.len());
    let mut components = vec![GenerativeComponent::Badge {
        label: format!("Public Catalog · {} items", items.len()),
        color: "#059669".into(),
    }];
    let mut keyboard: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    for (idx, item) in items.iter().enumerate() {
        let rating_str = if item.rating_count > 0 {
            format!("★ {:.1} ({})", item.rating_avg, item.rating_count)
        } else {
            "New Release".to_string()
        };
        let safe_reason = sanitize_recommendation_reason(&item.reason);
        lines.push(format!(
            "{}. {} {} · by {} [{}] — {}\n   {}",
            idx + 1,
            kind_icon(item.kind),
            item.title,
            item.author_display_name,
            rating_str,
            safe_reason,
            item.deep_link
        ));
        components.push(GenerativeComponent::Button {
            id: format!("rec_{}", item.creation_id),
            label: format!("{} {}", kind_icon(item.kind), item.title),
            action_payload: item.deep_link.clone(),
        });
        keyboard.push(vec![InlineKeyboardButton {
            text: format!("{} 打开「{}」", kind_icon(item.kind), item.title),
            callback_data: None,
            url: Some(item.deep_link.clone()),
            web_app_url: None,
        }]);
    }
    components.insert(
        1,
        GenerativeComponent::Text {
            text: lines.join("\n"),
            bold: false,
            color: None,
        },
    );

    CreatorBotReply {
        card: GenerativeCard {
            card_id: new_card_id("rec_ok"),
            title: format!("公开上架作品推荐 ({})", items.len()),
            subtitle: Some("以下内容均来自公开上架目录，点击即可直达对应板块打开。".into()),
            layout: GenerativeCardLayout::VerticalStack,
            components,
            status: Some("ok".into()),
        },
        keyboard,
    }
}

fn extract_paired_cjk_quote(text: &str, open: char, close: char) -> Option<String> {
    let mut search_from = 0usize;
    while let Some(rel) = text[search_from..].find(open) {
        let start = search_from + rel;
        let inner_start = start + open.len_utf8();
        let mut depth = 1usize;
        let mut closed_at: Option<usize> = None;
        for (end_rel, ch) in text[inner_start..].char_indices() {
            if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    closed_at = Some(end_rel);
                    break;
                }
            }
        }
        if let Some(end_rel) = closed_at {
            let candidate = text[inner_start..inner_start + end_rel].trim();
            if !candidate.is_empty() {
                return Some(candidate.chars().take(100).collect());
            }
            search_from = inner_start + end_rel + close.len_utf8();
        } else {
            break;
        }
    }
    None
}

fn extract_double_quoted_title(text: &str) -> Option<(usize, String)> {
    let mut search_from = 0usize;
    while let Some(rel) = text[search_from..].find('"') {
        let start = search_from + rel;
        let inner_start = start + 1;
        let mut out = String::new();
        let mut escaped = false;
        let mut closed_at: Option<usize> = None;
        for (end_rel, ch) in text[inner_start..].char_indices() {
            if escaped {
                if ch == '"' || ch == '\\' {
                    out.push(ch);
                } else {
                    out.push('\\');
                    out.push(ch);
                }
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                closed_at = Some(end_rel);
                break;
            }
            out.push(ch);
        }
        if let Some(end_rel) = closed_at {
            let candidate = out.trim();
            if !candidate.is_empty() {
                return Some((start, candidate.chars().take(100).collect()));
            }
            search_from = inner_start + end_rel + 1;
        } else {
            break;
        }
    }
    None
}

fn extract_single_quoted_title(text: &str) -> Option<(usize, String)> {
    let mut search_from = 0usize;
    while let Some(rel) = text[search_from..].find('\'') {
        let start = search_from + rel;
        let after_open = &text[start + 1..];
        if after_open.starts_with('\'') {
            search_from = start + 2;
            continue;
        }
        let prev_ok = start == 0
            || text[..start]
                .chars()
                .last()
                .is_some_and(|c| c.is_whitespace() || matches!(c, ':' | '：' | '(' | '（' | ',' | '，'));
        let next_ok = after_open
            .chars()
            .next()
            .is_some_and(|c| !c.is_ascii_digit() && !c.is_whitespace() && c != '\'');
        if prev_ok && next_ok && !is_n_contraction_open(text, start) {
            let mut best_end: Option<usize> = None;
            let mut escaped = false;
            for (end_rel, ch) in after_open.char_indices() {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == '\'' {
                    let abs_pos = start + 1 + end_rel;
                    let prev_char = after_open[..end_rel].chars().last();
                    let after_close = &after_open[end_rel + 1..];
                    let close_boundary_ok = prev_char.is_some_and(|c| !c.is_whitespace())
                        && (after_close.is_empty()
                            || after_close
                                .chars()
                                .next()
                                .is_some_and(|c| !c.is_alphanumeric()))
                        && !is_n_contraction_close(text, abs_pos);
                    if close_boundary_ok {
                        best_end = Some(end_rel);
                    }
                }
            }
            if let Some(end_rel) = best_end {
                let raw_inner = &after_open[..end_rel];
                let mut unescaped = String::with_capacity(raw_inner.len());
                let mut esc = false;
                for ch in raw_inner.chars() {
                    if esc {
                        if ch == '\'' || ch == '\\' {
                            unescaped.push(ch);
                        } else {
                            unescaped.push('\\');
                            unescaped.push(ch);
                        }
                        esc = false;
                    } else if ch == '\\' {
                        esc = true;
                    } else {
                        unescaped.push(ch);
                    }
                }
                if esc {
                    unescaped.push('\\');
                }
                let candidate = unescaped.trim();
                if !candidate.is_empty() {
                    return Some((start, candidate.chars().take(100).collect()));
                }
            }
        }
        search_from = start + 1;
    }
    None
}

fn extract_nl_title(text: &str, fallback_title: &str) -> String {
    // 1. Prefer explicit 《...》 and “...” CJK/curly quoted titles.
    for (open, close) in [('《', '》'), ('“', '”')] {
        if let Some(title) = extract_paired_cjk_quote(text, open, close) {
            return title;
        }
    }
    // 2. Prefer earliest valid "..." (with \" and \\ unescaping) or '...' quoted title.
    match (
        extract_double_quoted_title(text),
        extract_single_quoted_title(text),
    ) {
        (Some((d_pos, d_title)), Some((s_pos, s_title))) => {
            if d_pos <= s_pos {
                return d_title;
            }
            return s_title;
        }
        (Some((_, d_title)), None) => return d_title,
        (None, Some((_, s_title))) => return s_title,
        (None, None) => {}
    }
    // 3. Check common Chinese/English naming markers.
    let markers = [
        "叫做", "名叫", "标题为", "标题是", "名为", "叫", "titled ", "called ", "named ", "：", ":",
    ];
    let lower = text.to_ascii_lowercase();
    for marker in markers {
        let found_pos = if marker.is_ascii() {
            lower.find(marker).map(|p| (p, marker.len()))
        } else {
            text.find(marker).map(|p| (p, marker.len()))
        };
        if let Some((pos, len)) = found_pos {
            let tail = text[pos + len..]
                .trim()
                .trim_matches(|c: char| matches!(c, '。' | '.' | '!' | '！' | ',' | '，' | '《' | '》' | '"' | '\''))
                .trim();
            if !tail.is_empty() {
                let first_seg = tail
                    .split(['\n', '|', '｜', '，', ',', '。'])
                    .next()
                    .unwrap_or(tail)
                    .trim();
                if !first_seg.is_empty() {
                    return first_seg.chars().take(100).collect();
                }
            }
        }
    }
    fallback_title.to_string()
}

pub async fn handle_creator_studio_message(
    state: &Arc<AppState>,
    chat_id: ChatId,
    sender_id: UserId,
    text: &str,
) -> CreatorBotReply {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return build_creator_studio_welcome_reply();
    }

    if trimmed.starts_with('/') {
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let raw_cmd = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").trim();
        let (cmd_head, target_bot) = match raw_cmd.split_once('@') {
            Some((head, target)) => (head, Some(target)),
            None => (raw_cmd, None),
        };
        if let Some(target) = target_bot {
            if !target.eq_ignore_ascii_case(CREATOR_STUDIO_BOT_USERNAME)
                && !target.eq_ignore_ascii_case("creator_studio_bot")
            {
                return build_creator_studio_welcome_reply();
            }
        }
        let cmd = cmd_head.to_ascii_lowercase();
        match cmd.as_str() {
            "/start" | "/help" => return build_creator_studio_welcome_reply(),
            "/newbook" => {
                let Some((title, summary)) = parse_title_and_summary(rest) else {
                    return build_error_card("Usage", "Usage: /newbook <title> | <summary>");
                };
                return create_bot_work_and_reply(
                    state,
                    chat_id,
                    sender_id,
                    CreationKind::Book,
                    &title,
                    summary.as_deref(),
                    None,
                )
                .await;
            }
            "/newmusic" => {
                let Some((title, summary)) = parse_title_and_summary(rest) else {
                    return build_error_card("Usage", "Usage: /newmusic <title> | <summary>");
                };
                return create_bot_work_and_reply(
                    state,
                    chat_id,
                    sender_id,
                    CreationKind::Music,
                    &title,
                    summary.as_deref(),
                    None,
                )
                .await;
            }
            "/newgame" => {
                let Some((title, summary)) = parse_title_and_summary(rest) else {
                    return build_error_card("Usage", "Usage: /newgame <title> | <summary>");
                };
                return create_bot_work_and_reply(
                    state,
                    chat_id,
                    sender_id,
                    CreationKind::Game,
                    &title,
                    summary.as_deref(),
                    None,
                )
                .await;
            }
            "/newapp" => {
                let Some((title, summary)) = parse_title_and_summary(rest) else {
                    return build_error_card("Usage", "Usage: /newapp <title> | <summary>");
                };
                return create_bot_work_and_reply(
                    state,
                    chat_id,
                    sender_id,
                    CreationKind::App,
                    &title,
                    summary.as_deref(),
                    None,
                )
                .await;
            }
            "/mycreations" => return handle_mycreations_reply(state, sender_id, rest),
            "/editwork" => return handle_editwork_reply(state, sender_id, rest),
            "/submitwork" => return handle_submitwork_reply(state, sender_id, rest),
            "/withdraw" => return handle_withdraw_reply(state, sender_id, rest),
            "/unpublish" => return handle_unpublish_reply(state, sender_id, rest),
            "/deletework" => return handle_deletework_reply(state, sender_id, rest),
            "/reviewqueue" => return handle_reviewqueue_reply(state, sender_id),
            "/approve" => return handle_review_decision_reply(state, sender_id, rest, true),
            "/reject" => return handle_review_decision_reply(state, sender_id, rest, false),
            "/recommend" => return handle_recommend_reply(state, rest),
            _ => return build_creator_studio_welcome_reply(),
        }
    }

    handle_creator_studio_natural_language(state, chat_id, sender_id, trimmed).await
}

async fn handle_creator_studio_natural_language(
    state: &Arc<AppState>,
    chat_id: ChatId,
    sender_id: UserId,
    trimmed: &str,
) -> CreatorBotReply {
    let lower = trimmed.to_ascii_lowercase();

    let wants_list = lower.contains("my creations")
        || lower.contains("list my")
        || trimmed.contains("我的创作")
        || trimmed.contains("我的作品");
    if wants_list {
        return handle_mycreations_reply(state, sender_id, "");
    }

    let wants_recommend = lower.contains("recommend")
        || lower.contains("suggest")
        || trimmed.contains("推荐");
    let wants_create = lower.contains("create")
        || lower.contains("write")
        || lower.contains("compose")
        || lower.contains("make")
        || lower.contains("build")
        || lower.contains("generate")
        || trimmed.contains("创作")
        || trimmed.contains("写一本")
        || trimmed.contains("写一首")
        || trimmed.contains("写个")
        || trimmed.contains("做一")
        || trimmed.contains("做个")
        || trimmed.contains("发布")
        || trimmed.contains("生成");

    if wants_recommend && !wants_create {
        let mut kind_prefix = "";
        if lower.contains("book") || trimmed.contains("图书") || trimmed.contains("书") {
            kind_prefix = "books";
        } else if lower.contains("music") || lower.contains("song") || trimmed.contains("音乐") || trimmed.contains("歌") {
            kind_prefix = "music";
        } else if lower.contains("game") || trimmed.contains("游戏") {
            kind_prefix = "games";
        } else if lower.contains("app") || trimmed.contains("应用") {
            kind_prefix = "appstore";
        }
        return handle_recommend_reply(state, kind_prefix);
    }

    if wants_create {
        let is_music = lower.contains("music")
            || lower.contains("song")
            || lower.contains("track")
            || lower.contains("melody")
            || lower.contains("lo-fi")
            || lower.contains("lofi")
            || trimmed.contains("音乐")
            || trimmed.contains("歌曲")
            || trimmed.contains("一首歌");
        let is_game = lower.contains("game")
            || lower.contains("puzzle")
            || lower.contains("quiz")
            || lower.contains("trivia")
            || lower.contains("arcade")
            || trimmed.contains("游戏")
            || trimmed.contains("迷宫");
        let is_book = lower.contains("book")
            || lower.contains("novel")
            || lower.contains("story")
            || trimmed.contains("图书")
            || trimmed.contains("小说")
            || trimmed.contains("一本书");
        let is_app = lower.contains("app")
            || trimmed.contains("应用")
            || trimmed.contains("工具");

        if is_music {
            let title = extract_nl_title(trimmed, "Untitled Cloud Track");
            return create_bot_work_and_reply(
                state,
                chat_id,
                sender_id,
                CreationKind::Music,
                &title,
                Some(trimmed),
                None,
            )
            .await;
        }
        if is_game {
            let title = extract_nl_title(trimmed, "Untitled Arcade Quest");
            return create_bot_work_and_reply(
                state,
                chat_id,
                sender_id,
                CreationKind::Game,
                &title,
                Some(trimmed),
                None,
            )
            .await;
        }
        if is_book {
            let title = extract_nl_title(trimmed, "Untitled Cloud Book");
            return create_bot_work_and_reply(
                state,
                chat_id,
                sender_id,
                CreationKind::Book,
                &title,
                Some(trimmed),
                None,
            )
            .await;
        }
        if is_app {
            let title = extract_nl_title(trimmed, "Untitled Cloud App");
            return create_bot_work_and_reply(
                state,
                chat_id,
                sender_id,
                CreationKind::App,
                &title,
                Some(trimmed),
                None,
            )
            .await;
        }
    }

    build_creator_studio_welcome_reply()
}

// ============================================================================
// Inline Query Endpoint: GET /api/v1/bots/inline-query
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct InlineCreationResultDto {
    pub id: String,
    #[serde(rename = "type")]
    pub result_type: String,
    pub creation_id: String,
    pub kind: CreationKind,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub summary: String,
    pub author_display_name: String,
    pub genres: Vec<String>,
    pub cover: CoverArtwork,
    pub rating_avg: f32,
    pub rating_count: u32,
    pub score: f32,
    pub reason: String,
    pub deep_link: String,
    pub input_message_content: Value,
    pub reply_markup: Value,
}

fn parse_inline_kind(params: &HashMap<String, String>) -> Result<Option<CreationKind>, Response> {
    let Some(raw) = params.get("kind").map(|s| s.trim()) else {
        return Ok(None);
    };
    if raw.is_empty() || raw.eq_ignore_ascii_case("all") {
        return Ok(None);
    }
    if raw.eq_ignore_ascii_case("apps") {
        return Ok(Some(CreationKind::App));
    }
    CreationKind::parse(raw).map(Some).ok_or_else(|| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_kind",
            "kind must be one of: all, book, books, music, game, games, app, appstore",
        )
    })
}

fn parse_inline_limit(params: &HashMap<String, String>) -> Result<usize, Response> {
    let Some(raw) = params.get("limit").map(|s| s.trim()) else {
        return Ok(10);
    };
    if raw.is_empty() {
        return Ok(10);
    }
    let parsed: i64 = raw.parse().map_err(|_| {
        err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_limit",
            "limit must be an integer",
        )
    })?;
    if parsed <= 0 {
        Ok(0)
    } else {
        Ok((parsed as usize).min(50))
    }
}

/// GET /api/v1/bots/inline-query?bot_username=&query=&kind=&genre=&limit=
pub async fn bots_inline_query(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let bot_username_raw = params
        .get("bot_username")
        .map(|s| s.trim())
        .unwrap_or("");
    let target_slug = if bot_username_raw.is_empty() {
        CREATOR_STUDIO_BOT_USERNAME.to_string()
    } else {
        normalize_bot_slug(bot_username_raw)
    };
    if target_slug.is_empty() {
        return err_resp(StatusCode::NOT_FOUND, "bot_not_found", "Bot not found");
    }
    let bot = match state.db.get_bot_by_username(&target_slug) {
        Ok(Some(b)) => b,
        Ok(None) => return err_resp(StatusCode::NOT_FOUND, "bot_not_found", "Bot not found"),
        Err(e) => return map_telegram_err(e),
    };
    if !bot.is_inline {
        return err_resp(
            StatusCode::BAD_REQUEST,
            "inline_not_enabled",
            "Bot does not support inline queries",
        );
    }

    let kind = match parse_inline_kind(&params) {
        Ok(k) => k,
        Err(resp) => return resp,
    };
    let limit = match parse_inline_limit(&params) {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let query_opt = params
        .get("query")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .or_else(|| params.get("q").map(|s| s.trim()).filter(|s| !s.is_empty()));
    let genre_opt = params
        .get("genre")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    let query_id = format!("iq_{}", &Uuid::new_v4().simple().to_string()[..16]);

    // Optional Telegram Bot API parity hook for external inline-enabled standard bots.
    if bot.is_standard_bot && bot.is_inline {
        if let Ok(Some(caller)) = state.db.get_user(auth.user_id) {
            enqueue_update_and_maybe_webhook(
                &state,
                &bot,
                json!({
                    "inline_query": {
                        "id": query_id,
                        "from": tg_user_json(&caller),
                        "query": query_opt.unwrap_or("").trim(),
                        "offset": ""
                    }
                }),
            );
        }
    }

    // Layer 4 Zero-Leak Guarantee: EXCLUSIVELY query PublicCatalogSearchService (public_catalog_entries).
    let items: Vec<BotRecommendationItem> = if limit == 0 {
        Vec::new()
    } else {
        let catalog = PublicCatalogSearchService::new(Arc::clone(&state.db));
        match catalog.recommend(query_opt, kind, genre_opt, limit) {
            Ok(resp) => resp.items,
            Err(e) => return map_telegram_err(e),
        }
    };

    let results: Vec<InlineCreationResultDto> = items
        .into_iter()
        .map(|item| {
            let msg_text = format!(
                "{} {} — {}\n{}",
                kind_icon(item.kind),
                item.title,
                item.summary,
                item.deep_link
            );
            let reply_markup = json!({
                "inline_keyboard": [[{
                    "text": format!("{} 打开作品", kind_icon(item.kind)),
                    "url": item.deep_link
                }]]
            });
            InlineCreationResultDto {
                id: item.creation_id.clone(),
                result_type: "article".to_string(),
                creation_id: item.creation_id,
                kind: item.kind,
                title: item.title,
                subtitle: item.subtitle,
                description: item.summary.clone(),
                summary: item.summary,
                author_display_name: item.author_display_name,
                genres: item.genres,
                cover: item.cover,
                rating_avg: item.rating_avg,
                rating_count: item.rating_count,
                score: item.score,
                reason: sanitize_recommendation_reason(&item.reason),
                deep_link: item.deep_link,
                input_message_content: json!({ "message_text": msg_text }),
                reply_markup,
            }
        })
        .collect();

    // ZERO-LEAK NO-ECHO RULE: omit raw query probe string from response envelope.
    (
        StatusCode::OK,
        Json(json!({
            "query_id": query_id,
            "bot_username": bot.username,
            "total": results.len(),
            "results": results,
            "items": results
        })),
    )
        .into_response()
}
