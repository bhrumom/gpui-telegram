//! Embedded Apple Books reader (`BookReaderState`, `BookReaderTheme`, `BookReaderFontFamily`,
//! and `render_book_reader`) with TOC, bookmarks, 4 Apple reading themes, typography controls,
//! and cloud reading progress persistence.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::creations::{
    BookBookmark, BookPayload, BookRichBlock, BookUserState, UpsertCreationStateRequest,
    UserCreationDto, UserCreationStateDto,
};

use crate::i18n::tr_str;
use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::app_shell::modal_container::macos_secondary_pill_btn;
use crate::views::content_hub::apple_primitives::parse_hex_color;
use crate::views::content_hub::{ContentHubEvent, ContentHubWorkspace};

/// Authentic Apple Books 4 reading color schemes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BookReaderTheme {
    #[default]
    Light,
    Sepia,
    Gray,
    Night,
}

impl BookReaderTheme {
    pub const ALL: [Self; 4] = [Self::Light, Self::Sepia, Self::Gray, Self::Night];

    pub fn from_palette(pal: TdesktopThemePalette) -> Self {
        if pal.mode.is_dark() {
            Self::Night
        } else {
            Self::Light
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Light => Self::Sepia,
            Self::Sepia => Self::Gray,
            Self::Gray => Self::Night,
            Self::Night => Self::Light,
        }
    }

    pub fn label(self, lang_id: &str) -> &'static str {
        match self {
            Self::Light => tr_str(lang_id, "明亮", "Light"),
            Self::Sepia => tr_str(lang_id, "羊皮纸", "Sepia"),
            Self::Gray => tr_str(lang_id, "静谧灰", "Gray"),
            Self::Night => tr_str(lang_id, "夜间", "Night"),
        }
    }

    /// `(paper_bg, chrome_bg, text_fg, secondary_fg, accent_fg, border_rgb)`
    pub fn colors(self) -> (u32, u32, u32, u32, u32, u32) {
        match self {
            Self::Light => (0xFFFFFF, 0xF7F7F8, 0x1C1C1E, 0x636366, 0xFF9500, 0xE5E5EA),
            Self::Sepia => (0xF8F1E3, 0xEFE4CE, 0x433422, 0x7A664D, 0xC97A10, 0xDFD2B7),
            Self::Gray => (0x4A4A4D, 0x3F3F42, 0xF2F2F7, 0xC7C7CC, 0xFF9F0A, 0x5E5E62),
            Self::Night => (0x141416, 0x1C1C1E, 0xECECF0, 0x98989F, 0xFF9F0A, 0x2C2C2E),
        }
    }
}

/// Reading font styles inspired by Apple Books typography controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BookReaderFontFamily {
    #[default]
    Serif,
    Sans,
    Mono,
}

impl BookReaderFontFamily {
    pub const ALL: [Self; 3] = [Self::Serif, Self::Sans, Self::Mono];

    pub fn label(self, lang_id: &str) -> &'static str {
        match self {
            Self::Serif => tr_str(lang_id, "宋体", "Serif"),
            Self::Sans => tr_str(lang_id, "黑体", "Sans"),
            Self::Mono => tr_str(lang_id, "等宽", "Mono"),
        }
    }

    pub fn font_name(self) -> &'static str {
        match self {
            Self::Serif => "Georgia",
            Self::Sans => ".AppleSystemUIFont",
            Self::Mono => "Menlo",
        }
    }
}

pub const MIN_READER_FONT_SIZE: f32 = 13.0;
pub const MAX_READER_FONT_SIZE: f32 = 26.0;
pub const DEFAULT_READER_FONT_SIZE: f32 = 17.0;

/// Active Apple Books reader session state.
#[derive(Debug, Clone)]
pub struct BookReaderState {
    pub creation_id: String,
    pub title: String,
    pub author: String,
    pub summary: String,
    pub payload: BookPayload,
    pub current_chapter_index: usize,
    pub current_page_index: usize,
    pub theme: BookReaderTheme,
    pub font_family: BookReaderFontFamily,
    pub font_size_px: f32,
    pub toc_open: bool,
    pub show_bookmarks_tab: bool,
    pub appearance_open: bool,
    pub bookmarks: Vec<BookBookmark>,
    pub in_library: bool,
    pub favorite: bool,
    pub finished: bool,
}

impl BookReaderState {
    /// Opens a reader session for `creation`, hydrating chapter/page/bookmarks from `saved_state`.
    pub fn open(
        creation: &UserCreationDto,
        payload: BookPayload,
        saved_state: Option<&UserCreationStateDto>,
        palette: TdesktopThemePalette,
    ) -> Self {
        let mut book_state = saved_state
            .and_then(|s| serde_json::from_value::<BookUserState>(s.state_json.clone()).ok())
            .unwrap_or_default();

        let chapter_count = payload.chapters.len();
        let chapter_idx = if chapter_count == 0 {
            0
        } else {
            (book_state.current_chapter_index as usize).min(chapter_count - 1)
        };
        let page_count = payload
            .chapters
            .get(chapter_idx)
            .map(|c| c.pages.len())
            .unwrap_or(0);
        let page_idx = if page_count == 0 {
            0
        } else {
            (book_state.current_page_index as usize).min(page_count - 1)
        };

        let in_library = saved_state.is_some_and(|s| s.in_library) || book_state.in_library;
        let favorite = saved_state.is_some_and(|s| s.favorite);
        let finished = book_state.finished;

        let author = if !payload.author.trim().is_empty() {
            payload.author.clone()
        } else {
            creation.author_display_name.clone()
        };

        Self {
            creation_id: creation.creation_id.clone(),
            title: creation.title.clone(),
            author,
            summary: creation.summary.clone(),
            payload,
            current_chapter_index: chapter_idx,
            current_page_index: page_idx,
            theme: BookReaderTheme::from_palette(palette),
            font_family: BookReaderFontFamily::Serif,
            font_size_px: DEFAULT_READER_FONT_SIZE,
            toc_open: false,
            show_bookmarks_tab: false,
            appearance_open: false,
            bookmarks: std::mem::take(&mut book_state.bookmarks),
            in_library,
            favorite,
            finished,
        }
    }

    /// Computes total flattened page slots across all chapters (minimum 1).
    pub fn total_page_slots(&self) -> usize {
        let count: usize = self
            .payload
            .chapters
            .iter()
            .map(|c| c.pages.len().max(1))
            .sum();
        count.max(1)
    }

    /// Computes 0-based global page slot index across all chapters.
    pub fn current_global_page_index(&self) -> usize {
        if self.payload.chapters.is_empty() {
            return 0;
        }
        let mut offset = 0usize;
        for (idx, ch) in self.payload.chapters.iter().enumerate() {
            if idx == self.current_chapter_index {
                return offset + self.current_page_index.min(ch.pages.len().saturating_sub(1));
            }
            offset += ch.pages.len().max(1);
        }
        offset
    }

    /// Reading completion percentage in `0.0..=100.0`.
    pub fn progress_percent(&self) -> f32 {
        if self.finished {
            return 100.0;
        }
        let total = self.total_page_slots();
        if total <= 1 {
            return 100.0;
        }
        let pos = self.current_global_page_index() + 1;
        ((pos as f32 / total as f32) * 100.0).clamp(0.0, 100.0)
    }

    pub fn set_theme(&mut self, theme: BookReaderTheme) {
        self.theme = theme;
    }

    pub fn set_font_family(&mut self, family: BookReaderFontFamily) {
        self.font_family = family;
    }

    pub fn increase_font_size(&mut self) {
        self.font_size_px = (self.font_size_px + 2.0).clamp(MIN_READER_FONT_SIZE, MAX_READER_FONT_SIZE);
    }

    pub fn decrease_font_size(&mut self) {
        self.font_size_px = (self.font_size_px - 2.0).clamp(MIN_READER_FONT_SIZE, MAX_READER_FONT_SIZE);
    }

    pub fn jump_to_chapter(&mut self, chapter_index: usize) -> bool {
        if chapter_index >= self.payload.chapters.len() {
            return false;
        }
        self.current_chapter_index = chapter_index;
        self.current_page_index = 0;
        self.sync_finished_flag();
        true
    }

    pub fn jump_to_location(&mut self, chapter_index: usize, page_index: usize) -> bool {
        let Some(ch) = self.payload.chapters.get(chapter_index) else {
            return false;
        };
        self.current_chapter_index = chapter_index;
        self.current_page_index = page_index.min(ch.pages.len().saturating_sub(1));
        self.sync_finished_flag();
        true
    }

    pub fn next_page(&mut self) -> bool {
        let Some(ch) = self.payload.chapters.get(self.current_chapter_index) else {
            return false;
        };
        if self.current_page_index + 1 < ch.pages.len() {
            self.current_page_index += 1;
            self.sync_finished_flag();
            return true;
        }
        if self.current_chapter_index + 1 < self.payload.chapters.len() {
            self.current_chapter_index += 1;
            self.current_page_index = 0;
            self.sync_finished_flag();
            return true;
        }
        self.finished = true;
        false
    }

    pub fn prev_page(&mut self) -> bool {
        if self.current_page_index > 0 {
            self.current_page_index -= 1;
            return true;
        }
        if self.current_chapter_index > 0 {
            self.current_chapter_index -= 1;
            let last_page = self
                .payload
                .chapters
                .get(self.current_chapter_index)
                .map(|c| c.pages.len().saturating_sub(1))
                .unwrap_or(0);
            self.current_page_index = last_page;
            return true;
        }
        false
    }

    pub fn is_current_page_bookmarked(&self) -> bool {
        let ch = self.current_chapter_index as u32;
        let pg = self.current_page_index as u32;
        self.bookmarks
            .iter()
            .any(|b| b.chapter_index == ch && b.page_index == pg)
    }

    /// Toggles bookmark on the current chapter/page and returns `true` if added, `false` if removed.
    pub fn toggle_current_bookmark(&mut self, note: Option<String>) -> bool {
        let ch = self.current_chapter_index as u32;
        let pg = self.current_page_index as u32;
        if let Some(pos) = self
            .bookmarks
            .iter()
            .position(|b| b.chapter_index == ch && b.page_index == pg)
        {
            self.bookmarks.remove(pos);
            false
        } else {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let chapter_title = self
                .payload
                .chapters
                .get(self.current_chapter_index)
                .map(|c| c.title.clone())
                .unwrap_or_else(|| format!("Chapter {}", ch + 1));
            self.bookmarks.push(BookBookmark {
                bookmark_id: format!("bm_{}_{}_{}", ch, pg, now_ms),
                chapter_index: ch,
                page_index: pg,
                note: note.or_else(|| Some(format!("{} · P{}", chapter_title, pg + 1))),
                created_at_ms: now_ms,
            });
            true
        }
    }

    pub fn remove_bookmark(&mut self, bookmark_id: &str) -> bool {
        let before = self.bookmarks.len();
        self.bookmarks.retain(|b| b.bookmark_id != bookmark_id);
        self.bookmarks.len() != before
    }

    fn sync_finished_flag(&mut self) {
        if self.current_global_page_index() + 1 >= self.total_page_slots() {
            self.finished = true;
        }
    }

    /// Builds the cloud persistence payload (`PUT /api/v1/catalog/{id}/state`).
    pub fn to_upsert_state_request(&self) -> UpsertCreationStateRequest {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let progress = self.progress_percent();
        let state = BookUserState {
            current_chapter_index: self.current_chapter_index as u32,
            current_page_index: self.current_page_index as u32,
            progress_percent: progress,
            bookmarks: self.bookmarks.clone(),
            highlights: Vec::new(),
            in_library: true,
            finished: self.finished,
            last_read_at_ms: now_ms,
        };
        UpsertCreationStateRequest {
            in_library: Some(true),
            favorite: Some(self.favorite),
            progress_percent: Some(progress),
            state_json: serde_json::to_value(state).ok(),
        }
    }
}

fn emit_reader_state_sync(ws: &ContentHubWorkspace, cx: &mut Context<ContentHubWorkspace>) {
    if let Some(ref reader) = ws.book_reader {
        let cid = reader.creation_id.clone();
        let req = reader.to_upsert_state_request();
        cx.emit(ContentHubEvent::UpsertCreationState {
            creation_id: cid,
            req,
        });
    }
}

fn render_rich_block(
    block: &BookRichBlock,
    font_size: f32,
    text_fg: u32,
    secondary_fg: u32,
    accent_fg: u32,
    border_rgb: u32,
) -> AnyElement {
    match block {
        BookRichBlock::Heading { level, text } => {
            let scale = match level {
                1 => 1.55,
                2 => 1.30,
                _ => 1.12,
            };
            div()
                .pt(px(12.0))
                .pb(px(4.0))
                .text_size(px(font_size * scale))
                .font_weight(FontWeight::BOLD)
                .line_height(px(font_size * scale * 1.35))
                .text_color(rgb(text_fg))
                .child(text.clone())
                .into_any_element()
        }
        BookRichBlock::Paragraph { text } => div()
            .text_size(px(font_size))
            .line_height(px(font_size * 1.72))
            .text_color(rgb(text_fg))
            .child(text.clone())
            .into_any_element(),
        BookRichBlock::BlockQuote { text, attribution } => div()
            .my(px(8.0))
            .pl(px(16.0))
            .py(px(6.0))
            .border_l_2()
            .border_color(rgb(accent_fg))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(px(font_size * 0.96))
                    .line_height(px(font_size * 1.6))
                    .text_color(rgb(secondary_fg))
                    .child(format!("“{text}”")),
            )
            .when_some(attribution.clone(), |el, attr| {
                el.child(
                    div()
                        .text_size(px((font_size * 0.82).max(11.0)))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(secondary_fg))
                        .child(format!("— {attr}")),
                )
            })
            .into_any_element(),
        BookRichBlock::Callout { title, text } => div()
            .my(px(8.0))
            .p(px(14.0))
            .rounded(px(10.0))
            .bg(TdesktopThemePalette::rgba_with_alpha(accent_fg, 0.10))
            .border_1()
            .border_color(TdesktopThemePalette::rgba_with_alpha(accent_fg, 0.28))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(px((font_size * 0.92).max(12.0)))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(accent_fg))
                    .child(title.clone()),
            )
            .child(
                div()
                    .text_size(px(font_size * 0.92))
                    .line_height(px(font_size * 1.55))
                    .text_color(rgb(text_fg))
                    .child(text.clone()),
            )
            .into_any_element(),
        BookRichBlock::Illustration {
            caption,
            palette_hex,
            ..
        } => {
            let tint = palette_hex
                .as_deref()
                .map(|h| parse_hex_color(h, accent_fg))
                .unwrap_or(accent_fg);
            div()
                .my(px(10.0))
                .w_full()
                .rounded(px(12.0))
                .overflow_hidden()
                .border_1()
                .border_color(rgb(border_rgb))
                .flex()
                .flex_col()
                .child(
                    div()
                        .w_full()
                        .h(px(128.0))
                        .bg(linear_gradient(
                            135.0,
                            linear_color_stop(rgb(tint), 0.0),
                            linear_color_stop(rgb(0x1C1C1E), 1.0),
                        ))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(tg_icon(IconName::HubBooks, 36.0, 0xFFFFFF)),
                )
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(8.0))
                        .text_size(px(12.0))
                        .text_color(rgb(secondary_fg))
                        .child(caption.clone()),
                )
                .into_any_element()
        }
        BookRichBlock::Divider => div()
            .my(px(14.0))
            .w_full()
            .flex()
            .justify_center()
            .child(div().w(px(120.0)).h(px(1.0)).bg(rgb(border_rgb)))
            .into_any_element(),
    }
}

/// Renders the full-bleed embedded Apple Books reader surface inside `ContentHubWorkspace`.
pub fn render_book_reader(
    ws: &mut ContentHubWorkspace,
    cx: &mut Context<ContentHubWorkspace>,
) -> AnyElement {
    let Some(reader) = ws.book_reader.clone() else {
        return div().into_any_element();
    };
    let pal = ws.palette;
    let lang = ws.language_id.clone();
    let (paper_bg, chrome_bg, text_fg, secondary_fg, accent_fg, border_rgb) = reader.theme.colors();

    let chapter_opt = reader.payload.chapters.get(reader.current_chapter_index);
    let chapter_title = chapter_opt
        .map(|c| c.title.clone())
        .unwrap_or_else(|| reader.title.clone());
    let chapter_subtitle = chapter_opt.and_then(|c| c.subtitle.clone());
    let page_blocks: Vec<BookRichBlock> = chapter_opt
        .and_then(|c| c.pages.get(reader.current_page_index))
        .map(|p| p.blocks.clone())
        .unwrap_or_default();

    let progress_pct = reader.progress_percent();
    let is_bookmarked = reader.is_current_page_bookmarked();
    let total_chapters = reader.payload.chapters.len().max(1);
    let current_ch_pages = chapter_opt.map(|c| c.pages.len().max(1)).unwrap_or(1);
    let can_prev = reader.current_chapter_index > 0 || reader.current_page_index > 0;
    let can_next = reader.current_page_index + 1 < current_ch_pages
        || reader.current_chapter_index + 1 < reader.payload.chapters.len();

    div()
        .id("apple-book-reader-root")
        .size_full()
        .bg(rgb(paper_bg))
        .flex()
        .flex_col()
        .overflow_hidden()
        // Top Apple Books reader toolbar
        .child(
            div()
                .w_full()
                .h(px(50.0))
                .px(px(18.0))
                .bg(rgb(chrome_bg))
                .border_b_1()
                .border_color(rgb(border_rgb))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("book-reader-back-btn")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(8.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(text_fg, 0.08))
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(text_fg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        emit_reader_state_sync(this, cx);
                                        this.book_reader = None;
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(&lang, "← 返回书库", "← Library")),
                        )
                        .child(
                            div()
                                .id("book-reader-toc-toggle-btn")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(8.0))
                                .bg(if reader.toc_open {
                                    TdesktopThemePalette::rgba_with_alpha(accent_fg, 0.20)
                                } else {
                                    TdesktopThemePalette::rgba_with_alpha(text_fg, 0.08)
                                })
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(if reader.toc_open { accent_fg } else { text_fg }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(ref mut r) = this.book_reader {
                                            r.toc_open = !r.toc_open;
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child(tr_str(&lang, "目录", "Contents")),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(text_fg))
                                .truncate()
                                .child(reader.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(secondary_fg))
                                .truncate()
                                .child(format!("{} · {}", reader.author, chapter_title)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("book-reader-appearance-toggle-btn")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(8.0))
                                .bg(if reader.appearance_open {
                                    TdesktopThemePalette::rgba_with_alpha(accent_fg, 0.22)
                                } else {
                                    TdesktopThemePalette::rgba_with_alpha(text_fg, 0.08)
                                })
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(if reader.appearance_open {
                                    accent_fg
                                } else {
                                    text_fg
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(ref mut r) = this.book_reader {
                                            r.appearance_open = !r.appearance_open;
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child("Aa"),
                        )
                        .child(
                            div()
                                .id("book-reader-bookmark-toggle-btn")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(8.0))
                                .bg(if is_bookmarked {
                                    TdesktopThemePalette::rgba_with_alpha(accent_fg, 0.22)
                                } else {
                                    TdesktopThemePalette::rgba_with_alpha(text_fg, 0.08)
                                })
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(if is_bookmarked { accent_fg } else { text_fg }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(ref mut r) = this.book_reader {
                                            r.toggle_current_bookmark(None);
                                        }
                                        emit_reader_state_sync(this, cx);
                                        cx.notify();
                                    }),
                                )
                                .child(if is_bookmarked {
                                    tr_str(&lang, "★ 已加书签", "★ Bookmarked")
                                } else {
                                    tr_str(&lang, "☆ 书签", "☆ Bookmark")
                                }),
                        ),
                ),
        )
        // Collapsible Apple Books Aa Appearance bar (Themes + Font Family + Size)
        .when(reader.appearance_open, |el| {
            el.child(
                div()
                    .id("book-reader-appearance-bar")
                    .w_full()
                    .px(px(20.0))
                    .py(px(10.0))
                    .bg(rgb(chrome_bg))
                    .border_b_1()
                    .border_color(rgb(border_rgb))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    // 4 Apple Books reading themes
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .children(BookReaderTheme::ALL.iter().map(|&t| {
                                let (t_bg, _, t_fg, _, _, _) = t.colors();
                                let active = reader.theme == t;
                                div()
                                    .id(SharedString::from(format!("book-theme-{t:?}")))
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(999.0))
                                    .bg(rgb(t_bg))
                                    .border_1()
                                    .border_color(if active {
                                        rgb(accent_fg)
                                    } else {
                                        rgb(border_rgb)
                                    })
                                    .text_size(px(11.0))
                                    .font_weight(if active {
                                        FontWeight::BOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    })
                                    .text_color(rgb(t_fg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            if let Some(ref mut r) = this.book_reader {
                                                r.set_theme(t);
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child(t.label(&lang))
                            })),
                    )
                    // 3 Font families
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .children(BookReaderFontFamily::ALL.iter().map(|&fam| {
                                let active = reader.font_family == fam;
                                div()
                                    .id(SharedString::from(format!("book-font-{fam:?}")))
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(8.0))
                                    .bg(if active {
                                        rgb(accent_fg)
                                    } else {
                                        TdesktopThemePalette::rgba_with_alpha(text_fg, 0.08)
                                    })
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(if active { 0xFFFFFF } else { text_fg }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            if let Some(ref mut r) = this.book_reader {
                                                r.set_font_family(fam);
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child(fam.label(&lang))
                            })),
                    )
                    // Font scale A- / size / A+
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id("book-reader-font-dec")
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(8.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(text_fg, 0.08))
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(text_fg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            if let Some(ref mut r) = this.book_reader {
                                                r.decrease_font_size();
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child("A-"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(secondary_fg))
                                    .child(format!("{}px", reader.font_size_px.round() as i32)),
                            )
                            .child(
                                div()
                                    .id("book-reader-font-inc")
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(8.0))
                                    .bg(TdesktopThemePalette::rgba_with_alpha(text_fg, 0.08))
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(text_fg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            if let Some(ref mut r) = this.book_reader {
                                                r.increase_font_size();
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child("A+"),
                            ),
                    ),
            )
        })
        // Body: optional TOC/Bookmarks drawer + centered reading canvas
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .flex()
                .overflow_hidden()
                .when(reader.toc_open, |el| {
                    el.child(
                        div()
                            .id("book-reader-toc-drawer")
                            .w(px(250.0))
                            .h_full()
                            .flex_shrink_0()
                            .bg(rgb(chrome_bg))
                            .border_r_1()
                            .border_color(rgb(border_rgb))
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(
                                div()
                                    .p(px(10.0))
                                    .border_b_1()
                                    .border_color(rgb(border_rgb))
                                    .flex()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .id("book-toc-tab-chapters")
                                            .cursor_pointer()
                                            .flex_1()
                                            .py(px(5.0))
                                            .rounded(px(7.0))
                                            .bg(if !reader.show_bookmarks_tab {
                                                rgb(accent_fg)
                                            } else {
                                                TdesktopThemePalette::rgba_with_alpha(text_fg, 0.06)
                                            })
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if !reader.show_bookmarks_tab {
                                                0xFFFFFF
                                            } else {
                                                text_fg
                                            }))
                                            .flex()
                                            .justify_center()
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    if let Some(ref mut r) = this.book_reader {
                                                        r.show_bookmarks_tab = false;
                                                    }
                                                    cx.notify();
                                                }),
                                            )
                                            .child(tr_str(&lang, "章节目录", "Chapters")),
                                    )
                                    .child(
                                        div()
                                            .id("book-toc-tab-bookmarks")
                                            .cursor_pointer()
                                            .flex_1()
                                            .py(px(5.0))
                                            .rounded(px(7.0))
                                            .bg(if reader.show_bookmarks_tab {
                                                rgb(accent_fg)
                                            } else {
                                                TdesktopThemePalette::rgba_with_alpha(text_fg, 0.06)
                                            })
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(if reader.show_bookmarks_tab {
                                                0xFFFFFF
                                            } else {
                                                text_fg
                                            }))
                                            .flex()
                                            .justify_center()
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    if let Some(ref mut r) = this.book_reader {
                                                        r.show_bookmarks_tab = true;
                                                    }
                                                    cx.notify();
                                                }),
                                            )
                                            .child(format!(
                                                "{} ({})",
                                                tr_str(&lang, "书签", "Bookmarks"),
                                                reader.bookmarks.len()
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .id("book-reader-toc-scroll")
                                    .flex_1()
                                    .overflow_y_scroll()
                                    .p(px(10.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .when(!reader.show_bookmarks_tab, |list| {
                                        list.children(reader.payload.chapters.iter().enumerate().map(
                                            |(idx, ch)| {
                                                let active = idx == reader.current_chapter_index;
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "book-toc-ch-{idx}"
                                                    )))
                                                    .cursor_pointer()
                                                    .px(px(10.0))
                                                    .py(px(7.0))
                                                    .rounded(px(8.0))
                                                    .bg(if active {
                                                        TdesktopThemePalette::rgba_with_alpha(
                                                            accent_fg, 0.18,
                                                        )
                                                    } else {
                                                        rgba(0x00000000)
                                                    })
                                                    .text_size(px(12.0))
                                                    .font_weight(if active {
                                                        FontWeight::SEMIBOLD
                                                    } else {
                                                        FontWeight::NORMAL
                                                    })
                                                    .text_color(rgb(if active {
                                                        accent_fg
                                                    } else {
                                                        text_fg
                                                    }))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            if let Some(ref mut r) =
                                                                this.book_reader
                                                            {
                                                                r.jump_to_chapter(idx);
                                                            }
                                                            emit_reader_state_sync(this, cx);
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child(format!(
                                                        "{}. {}",
                                                        ch.chapter_number, ch.title
                                                    ))
                                            },
                                        ))
                                    })
                                    .when(reader.show_bookmarks_tab, |list| {
                                        if reader.bookmarks.is_empty() {
                                            list.child(
                                                div()
                                                    .p(px(12.0))
                                                    .text_size(px(12.0))
                                                    .text_color(rgb(secondary_fg))
                                                    .child(tr_str(
                                                        &lang,
                                                        "暂无书签，点击右上角添加书签。",
                                                        "No bookmarks yet. Click ☆ Bookmark above.",
                                                    )),
                                            )
                                        } else {
                                            list.children(reader.bookmarks.iter().map(|bm| {
                                                let ch_i = bm.chapter_index as usize;
                                                let pg_i = bm.page_index as usize;
                                                let bm_id = bm.bookmark_id.clone();
                                                let label = bm.note.clone().unwrap_or_else(|| {
                                                    format!("Ch {} · P{}", ch_i + 1, pg_i + 1)
                                                });
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "book-bm-row-{bm_id}"
                                                    )))
                                                    .cursor_pointer()
                                                    .px(px(10.0))
                                                    .py(px(7.0))
                                                    .rounded(px(8.0))
                                                    .bg(TdesktopThemePalette::rgba_with_alpha(
                                                        accent_fg, 0.10,
                                                    ))
                                                    .flex()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap(px(6.0))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(move |this, _, _, cx| {
                                                            if let Some(ref mut r) =
                                                                this.book_reader
                                                            {
                                                                r.jump_to_location(ch_i, pg_i);
                                                            }
                                                            emit_reader_state_sync(this, cx);
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w(px(0.0))
                                                            .text_size(px(12.0))
                                                            .text_color(rgb(text_fg))
                                                            .truncate()
                                                            .child(label),
                                                    )
                                            }))
                                        }
                                    }),
                            ),
                    )
                })
                // Centered editorial reading canvas
                .child(
                    div()
                        .id("book-reader-page-scroll")
                        .flex_1()
                        .min_w(px(0.0))
                        .h_full()
                        .overflow_y_scroll()
                        .px(px(36.0))
                        .py(px(32.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(
                            div()
                                .w_full()
                                .max_w(px(700.0))
                                .font_family(reader.font_family.font_name())
                                .flex()
                                .flex_col()
                                .gap(px(14.0))
                                .child(
                                    div()
                                        .pb(px(10.0))
                                        .border_b_1()
                                        .border_color(rgb(border_rgb))
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(accent_fg))
                                                .child(format!(
                                                    "{} {}/{}",
                                                    tr_str(&lang, "章节", "CHAPTER"),
                                                    reader.current_chapter_index + 1,
                                                    total_chapters
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(reader.font_size_px * 1.45))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(text_fg))
                                                .child(chapter_title),
                                        )
                                        .when_some(chapter_subtitle, |el, sub| {
                                            el.child(
                                                div()
                                                    .text_size(px(reader.font_size_px * 0.92))
                                                    .text_color(rgb(secondary_fg))
                                                    .child(sub),
                                            )
                                        }),
                                )
                                .when(!page_blocks.is_empty(), |el| {
                                    el.children(page_blocks.iter().map(|b| {
                                        render_rich_block(
                                            b,
                                            reader.font_size_px,
                                            text_fg,
                                            secondary_fg,
                                            accent_fg,
                                            border_rgb,
                                        )
                                    }))
                                })
                                .when(page_blocks.is_empty(), |el| {
                                    let fallback = if !reader.payload.blurb.trim().is_empty() {
                                        reader.payload.blurb.clone()
                                    } else {
                                        reader.summary.clone()
                                    };
                                    el.child(
                                        div()
                                            .text_size(px(reader.font_size_px))
                                            .line_height(px(reader.font_size_px * 1.7))
                                            .text_color(rgb(text_fg))
                                            .child(fallback),
                                    )
                                }),
                        ),
                ),
        )
        // Bottom reading progress & page turn footer
        .child(
            div()
                .w_full()
                .h(px(52.0))
                .px(px(20.0))
                .bg(rgb(chrome_bg))
                .border_t_1()
                .border_color(rgb(border_rgb))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(14.0))
                .child(
                    macos_secondary_pill_btn(
                        "book-reader-prev-btn",
                        tr_str(&lang, "← 上一页", "← Previous"),
                        pal,
                    )
                    .when(can_prev, |btn| {
                        btn.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                if let Some(ref mut r) = this.book_reader {
                                    if r.prev_page() {
                                        emit_reader_state_sync(this, cx);
                                    }
                                }
                                cx.notify();
                            }),
                        )
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .max_w(px(420.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(5.0))
                        .child(
                            div()
                                .w_full()
                                .h(px(4.0))
                                .rounded(px(999.0))
                                .bg(rgb(border_rgb))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(relative((progress_pct / 100.0).clamp(0.0, 1.0)))
                                        .bg(rgb(accent_fg)),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(secondary_fg))
                                .child(format!(
                                    "{} {}/{} · {} {}/{} · {:.0}%",
                                    tr_str(&lang, "第", "Ch"),
                                    reader.current_chapter_index + 1,
                                    total_chapters,
                                    tr_str(&lang, "页", "Page"),
                                    reader.current_page_index + 1,
                                    current_ch_pages,
                                    progress_pct
                                )),
                        ),
                )
                .child(
                    macos_secondary_pill_btn(
                        "book-reader-next-btn",
                        if can_next {
                            tr_str(&lang, "下一页 →", "Next →")
                        } else {
                            tr_str(&lang, "标记读完 ✓", "Finish ✓")
                        },
                        pal,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if let Some(ref mut r) = this.book_reader {
                                r.next_page();
                            }
                            emit_reader_state_sync(this, cx);
                            cx.notify();
                        }),
                    ),
                ),
        )
        .into_any_element()
}
