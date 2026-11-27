//! "推送审核上架" (`PublishReviewModal`) state, live `validate_creation_for_review()` engine,
//! editable metadata form, immutable audit trail display, and admin Reviewer Queue panel.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_proto::creations::{
    validate_creation_for_review, BookRichBlock, CreationPayload, CreationReviewAuditDto,
    CreationReviewStatus, CreationVisibility, GameSpec, ReviewDecision, ReviewDecisionRequest,
    SubmitReviewRequest, UpdateCreationRequest, UserCreationDto, MIN_BOOK_READABLE_CHARS,
    MIN_CREATION_SUMMARY_CHARS,
};

use crate::components::ui::TextInputController;
use crate::i18n::tr_str;
use crate::theme::{tg_icon_rgba, IconName, TdesktopThemePalette};
use crate::views::content_hub::apple_primitives::{
    apple_grouped_card, apple_hairline, cover_artwork_box, creation_kind_accent,
    creation_kind_label, filter_pill, openbot_primary_cta_btn, openbot_secondary_btn,
    render_editable_field_box, review_status_badge, EditableFieldState,
};
use crate::views::content_hub::{ContentHubEvent, ContentHubWorkspace};

/// Returns `true` for invisible `Default_Ignorable_Code_Point`, Unicode `Cf` format controls,
/// Unicode Noncharacters (`U+FDD0..=U+FDEF` and `U+xFFFE..=U+xFFFF` across all 17 planes),
/// variation selectors, Plane-14 tag/VS-supplement points, or blank-filler characters that
/// `char::is_whitespace()` and `char::is_control()` do not classify as empty.
///
/// Private Use Area (PUA) code points are intentionally preserved so custom icon/glyph fonts work.
pub fn is_invisible_format_char(c: char) -> bool {
    let cp = c as u32;
    if (0xFDD0..=0xFDEF).contains(&cp) || (cp & 0xFFFE) == 0xFFFE {
        return true;
    }
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFFC}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

/// Folds Fullwidth ASCII (`U+FF01..=U+FF5E`, e.g. `ａ`->`a`, `％`->`%`, `＆`->`&`, `／`->`/`,
/// `＼`->`\`, `：`->`:`, `＜`->`<`, `＠`->`@`) to standard ASCII for smuggling/XSS normalization.
/// NOT applied to `visible_char_count()` so Fullwidth CJK titles remain visible.
#[inline]
pub(crate) fn fold_fullwidth_ascii(c: char) -> char {
    let cp = c as u32;
    if (0xFF01..=0xFF5E).contains(&cp) {
        char::from_u32(cp - 0xFEE0).unwrap_or(c)
    } else {
        c
    }
}

/// Returns `true` if `c` is a genuinely visible character (neither whitespace, control, nor
/// invisible format/filler).
pub fn is_visible_char(c: char) -> bool {
    !c.is_whitespace() && !c.is_control() && !is_invisible_format_char(c)
}

/// Counts genuinely visible characters in `s`.
pub fn visible_char_count(s: &str) -> usize {
    s.chars().filter(|&c| is_visible_char(c)).count()
}

fn is_emoji_or_script_joiner(c: char) -> bool {
    matches!(c, '\u{200C}' | '\u{200D}' | '\u{FE0E}' | '\u{FE0F}')
}

const MAX_XSS_DECODE_PASSES: usize = 16;

/// Decodes HTML numeric/hex character references (`&#106;`, `&#x6a;`, `&#0000106;`, `&#X09;`,
/// with optional trailing `;`) and XSS-relevant named entities (`&lt;`, `&gt;`, `&colon;`,
/// `&tab;`, `&newline;`, `&sol;`, `&amp;`) in a single O(N) pass.
fn decode_html_entities_once(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&' {
            if i + 2 < bytes.len() && bytes[i + 1] == b'#' {
                let is_hex = matches!(bytes[i + 2], b'x' | b'X');
                let digits_start = if is_hex { i + 3 } else { i + 2 };
                let mut j = digits_start;
                while j < bytes.len() {
                    let b = bytes[j];
                    let ok = if is_hex {
                        b.is_ascii_hexdigit()
                    } else {
                        b.is_ascii_digit()
                    };
                    if ok {
                        j += 1;
                    } else {
                        break;
                    }
                }
                if j > digits_start {
                    let mut k = digits_start;
                    while k < j && bytes[k] == b'0' {
                        k += 1;
                    }
                    let cp: Option<u32> = if k == j {
                        Some(0)
                    } else {
                        let sig = &input[k..j];
                        if sig.len() > 8 {
                            None
                        } else {
                            u32::from_str_radix(sig, if is_hex { 16 } else { 10 }).ok()
                        }
                    };
                    let end = if j < bytes.len() && bytes[j] == b';' {
                        j + 1
                    } else {
                        j
                    };
                    match cp.and_then(|v| if v == 0 { None } else { char::from_u32(v) }) {
                        Some(ch) => out.push(ch),
                        // WHATWG HTML5 maps NUL / surrogate / out-of-range numeric refs to U+FFFD
                        // (subsequently stripped by `strip_and_lower_for_xss`, catching `java&#0;script:`).
                        None => out.push('\u{FFFD}'),
                    }
                    i = end;
                    continue;
                }
            }

            let rest = &input[i + 1..];
            let named: Option<(char, usize)> = [
                ("lt;", '<'),
                ("gt;", '>'),
                ("colon;", ':'),
                ("tab;", '\t'),
                ("newline;", '\n'),
                ("sol;", '/'),
                ("amp;", '&'),
            ]
            .into_iter()
            .find_map(|(name, ch)| {
                if rest.len() >= name.len()
                    && rest.is_char_boundary(name.len())
                    && rest[..name.len()].eq_ignore_ascii_case(name)
                {
                    Some((ch, 1 + name.len()))
                } else {
                    None
                }
            });
            if let Some((ch, consumed)) = named {
                out.push(ch);
                i += consumed;
                continue;
            }
        }
        let Some(ch) = input[i..].chars().next() else {
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn strip_and_lower_for_xss(value: &str) -> String {
    value
        .chars()
        .filter(|&c| !c.is_control() && c != '\u{FFFD}' && !is_invisible_format_char(c))
        .map(fold_fullwidth_ascii)
        .flat_map(char::to_lowercase)
        .collect()
}

fn has_raw_xss_substring(normalized_lower: &str) -> bool {
    let compact: String = normalized_lower
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    compact.contains("<script")
        || compact.contains("javascript:")
        || compact.contains("vbscript:")
        || compact.contains("data:text/html")
        || compact.contains("data:image/svg+xml")
        || compact.contains("data:application/xhtml+xml")
}

pub fn contains_obscured_xss_pattern(value: &str) -> bool {
    let mut current = strip_and_lower_for_xss(value);
    if has_raw_xss_substring(&current) {
        return true;
    }
    for _ in 0..MAX_XSS_DECODE_PASSES {
        let has_pct = current.contains('%');
        let has_amp = current.contains('&');
        if !has_pct && !has_amp {
            break;
        }
        let mut step = current.clone();
        if has_pct {
            let collapsed =
                crate::views::app_shell::shell_content_hub::collapse_repeated_percent25_chains(
                    &step,
                );
            step = crate::views::app_shell::shell_content_hub::lenient_percent_decode_once(
                &collapsed,
            );
        }
        if step.contains('&') {
            step = decode_html_entities_once(&step);
        }
        let next = strip_and_lower_for_xss(&step);
        if next == current {
            break;
        }
        if has_raw_xss_substring(&next) {
            return true;
        }
        current = next;
    }
    false
}

fn record_obscured_xss(errors: &mut Vec<String>, path: String, s: &str) {
    if contains_obscured_xss_pattern(s) {
        let msg = format!("{path} contains disallowed script or unsafe URI scheme");
        if !errors.iter().any(|e| e == &msg) {
            errors.push(msg);
        }
    }
}

fn collect_obscured_xss_json_errors(
    errors: &mut Vec<String>,
    field: &str,
    value: &serde_json::Value,
) {
    match value {
        serde_json::Value::String(s) => record_obscured_xss(errors, field.to_string(), s),
        serde_json::Value::Array(items) => {
            for (idx, item) in items.iter().enumerate() {
                collect_obscured_xss_json_errors(errors, &format!("{field}[{idx}]"), item);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let child_path = if field.is_empty() {
                    k.clone()
                } else {
                    format!("{field}.{k}")
                };
                record_obscured_xss(errors, child_path.clone(), k);
                collect_obscured_xss_json_errors(errors, &child_path, v);
            }
        }
        _ => {}
    }
}

fn push_visible_field_error(errors: &mut Vec<String>, prefix: &str, msg: String) {
    if !errors.iter().any(|e| e.starts_with(prefix)) {
        errors.push(msg);
    }
}

fn validate_payload_visible_fields(payload: &CreationPayload, errors: &mut Vec<String>) {
    match payload {
        CreationPayload::Book(book) => {
            if book.chapters.is_empty() {
                return;
            }
            let mut visible_readable = 0usize;
            for (ci, chapter) in book.chapters.iter().enumerate() {
                let title_prefix = format!("book.chapters[{ci}].title ");
                if visible_char_count(&chapter.title) == 0 {
                    push_visible_field_error(
                        errors,
                        &title_prefix,
                        format!("book.chapters[{ci}].title must contain visible characters"),
                    );
                }
                for page in &chapter.pages {
                    for block in &page.blocks {
                        match block {
                            BookRichBlock::Heading { text, .. }
                            | BookRichBlock::Paragraph { text } => {
                                visible_readable += visible_char_count(text);
                            }
                            BookRichBlock::BlockQuote { text, attribution } => {
                                visible_readable += visible_char_count(text);
                                if let Some(attr) = attribution.as_deref() {
                                    visible_readable += visible_char_count(attr);
                                }
                            }
                            BookRichBlock::Callout { title, text } => {
                                visible_readable += visible_char_count(title);
                                visible_readable += visible_char_count(text);
                            }
                            BookRichBlock::Illustration { caption, .. } => {
                                visible_readable += visible_char_count(caption);
                            }
                            BookRichBlock::Divider => {}
                        }
                    }
                }
            }
            if visible_readable < MIN_BOOK_READABLE_CHARS {
                push_visible_field_error(
                    errors,
                    "book must contain at least ",
                    format!(
                        "book must contain at least {MIN_BOOK_READABLE_CHARS} readable visible characters (found {visible_readable})"
                    ),
                );
            }
        }
        CreationPayload::Music(music) => {
            for (ti, track) in music.tracks.iter().enumerate() {
                let p = format!("music.tracks[{ti}]");
                if visible_char_count(&track.title) == 0 {
                    push_visible_field_error(
                        errors,
                        &format!("{p}.title "),
                        format!("{p}.title must contain visible characters"),
                    );
                }
                let has_visible_audio = track
                    .audio_media_file_id
                    .as_deref()
                    .is_some_and(|id| visible_char_count(id) > 0);
                if !has_visible_audio && track.melody_notes.len() < 4 {
                    push_visible_field_error(
                        errors,
                        &format!("{p} must provide "),
                        format!(
                            "{p} must provide either a visible audio_media_file_id or at least 4 melody_notes"
                        ),
                    );
                }
            }
        }
        CreationPayload::Game(game) => {
            if visible_char_count(&game.developer) == 0 {
                push_visible_field_error(
                    errors,
                    "game.developer ",
                    "game.developer must contain visible characters".to_string(),
                );
            }
            if visible_char_count(&game.rules_markdown) == 0 {
                push_visible_field_error(
                    errors,
                    "game.rules_markdown ",
                    "game.rules_markdown must contain visible characters".to_string(),
                );
            }
            match &game.spec {
                GameSpec::GridPuzzle { levels, .. } => {
                    for (li, lvl) in levels.iter().enumerate() {
                        if visible_char_count(&lvl.title) == 0 {
                            push_visible_field_error(
                                errors,
                                &format!("game.spec.levels[{li}].title "),
                                format!(
                                    "game.spec.levels[{li}].title must contain visible characters"
                                ),
                            );
                        }
                    }
                }
                GameSpec::TriviaQuiz { questions, .. } => {
                    for (qi, q) in questions.iter().enumerate() {
                        if visible_char_count(&q.prompt) == 0 {
                            push_visible_field_error(
                                errors,
                                &format!("game.spec.questions[{qi}].prompt "),
                                format!(
                                    "game.spec.questions[{qi}].prompt must contain visible characters"
                                ),
                            );
                        }
                        for (ci, choice) in q.choices.iter().enumerate() {
                            if visible_char_count(choice) == 0 {
                                push_visible_field_error(
                                    errors,
                                    &format!("game.spec.questions[{qi}].choices[{ci}] "),
                                    format!(
                                        "game.spec.questions[{qi}].choices[{ci}] must contain visible characters"
                                    ),
                                );
                            }
                        }
                    }
                }
                GameSpec::InteractiveFiction {
                    start_node_id,
                    nodes,
                } => {
                    if visible_char_count(start_node_id) == 0 {
                        push_visible_field_error(
                            errors,
                            "game.spec.interactive_fiction start_node_id",
                            "game.spec.interactive_fiction start_node_id must contain visible characters"
                                .to_string(),
                        );
                    }
                    for (ni, node) in nodes.iter().enumerate() {
                        if visible_char_count(&node.node_id) == 0 {
                            push_visible_field_error(
                                errors,
                                &format!("game.spec.nodes[{ni}].node_id "),
                                format!(
                                    "game.spec.nodes[{ni}].node_id must contain visible characters"
                                ),
                            );
                        }
                    }
                }
                GameSpec::CardMatch { pairs, .. } => {
                    for (pi, pair) in pairs.iter().enumerate() {
                        if visible_char_count(&pair.pair_id) == 0 {
                            push_visible_field_error(
                                errors,
                                &format!("game.spec.pairs[{pi}].pair_id "),
                                format!(
                                    "game.spec.pairs[{pi}].pair_id must contain visible characters"
                                ),
                            );
                        }
                        if visible_char_count(&pair.label) == 0 {
                            push_visible_field_error(
                                errors,
                                &format!("game.spec.pairs[{pi}].label "),
                                format!(
                                    "game.spec.pairs[{pi}].label must contain visible characters"
                                ),
                            );
                        }
                    }
                }
                GameSpec::MiniAppHtml {
                    bot_username,
                    short_name,
                    html_bundle_media_id,
                } => {
                    let has_miniapp =
                        visible_char_count(bot_username) > 0 && visible_char_count(short_name) > 0;
                    let has_bundle = html_bundle_media_id
                        .as_deref()
                        .is_some_and(|s| visible_char_count(s) > 0);
                    if !has_miniapp && !has_bundle {
                        push_visible_field_error(
                            errors,
                            "game.spec.mini_app_html requires ",
                            "game.spec.mini_app_html requires visible (bot_username and short_name) or html_bundle_media_id"
                                .to_string(),
                        );
                    }
                }
                GameSpec::ArcadeDodge { .. } | GameSpec::SnakeGrid { .. } => {}
            }
        }
        CreationPayload::App(app) => {
            if visible_char_count(&app.developer_name) == 0 {
                push_visible_field_error(
                    errors,
                    "app.developer_name ",
                    "app.developer_name must contain visible characters".to_string(),
                );
            }
            if visible_char_count(&app.version_string) == 0 {
                push_visible_field_error(
                    errors,
                    "app.version_string ",
                    "app.version_string must contain visible characters".to_string(),
                );
            }
            if visible_char_count(&app.long_description) == 0 {
                push_visible_field_error(
                    errors,
                    "app.long_description ",
                    "app.long_description must contain visible characters".to_string(),
                );
            }
            let mut has_visible_miniapp = false;
            if let Some(link) = app.linked_miniapp.as_ref() {
                let has_bot_pair = visible_char_count(&link.bot_username) > 0
                    && visible_char_count(&link.short_name) > 0;
                let has_web_url = link
                    .web_url
                    .as_deref()
                    .is_some_and(|u| visible_char_count(u) > 0)
                    && !errors
                        .iter()
                        .any(|e| e.starts_with("app.linked_miniapp.web_url "));
                if has_bot_pair || has_web_url {
                    has_visible_miniapp = true;
                } else {
                    push_visible_field_error(
                        errors,
                        "app.linked_miniapp must specify ",
                        "app.linked_miniapp must specify visible (bot_username and short_name) or a valid web_url"
                            .to_string(),
                    );
                }
            }
            let has_valid_ui_schema = app
                .ui_schema_json
                .as_ref()
                .is_some_and(serde_json::Value::is_object);
            if !has_visible_miniapp && !has_valid_ui_schema {
                push_visible_field_error(
                    errors,
                    "app must provide either ",
                    "app must provide either a valid linked_miniapp or a JSON object ui_schema_json"
                        .to_string(),
                );
            }
        }
    }
}

/// Strips ASCII/Unicode control characters (preserving internal newlines) and invisible format
/// characters (while preserving internal ZWJ/ZWNJ/VS15/VS16 joiners inside visible emoji/scripts),
/// trims leading/trailing whitespace and edge joiners, and returns `None` if no visible character
/// remains.
pub fn normalize_non_empty_reason(raw: &str) -> Option<String> {
    if visible_char_count(raw) == 0 {
        return None;
    }
    let cleaned: String = raw
        .chars()
        .filter(|&c| {
            (!c.is_control() || c == '\n')
                && (!is_invisible_format_char(c) || is_emoji_or_script_joiner(c))
        })
        .collect();
    let trimmed = cleaned
        .trim_start_matches(|c: char| c.is_whitespace() || is_emoji_or_script_joiner(c))
        .trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '\u{200C}' | '\u{200D}'));
    if visible_char_count(trimmed) == 0 {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Editable field target inside `PublishReviewModal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishModalField {
    Title,
    Subtitle,
    AuthorDisplayName,
    Summary,
    Genres,
    ReviewNotes,
    RejectReason,
}

impl PublishModalField {
    const SUBMIT_FIELDS: &'static [Self] = &[
        Self::Title,
        Self::Subtitle,
        Self::AuthorDisplayName,
        Self::Summary,
        Self::Genres,
        Self::ReviewNotes,
    ];

    pub fn is_multiline(self) -> bool {
        matches!(self, Self::Summary | Self::ReviewNotes | Self::RejectReason)
    }

    pub fn next_in_submit(self, reverse: bool) -> Self {
        let list = Self::SUBMIT_FIELDS;
        let idx = list.iter().position(|&f| f == self).unwrap_or(0);
        let next = if reverse {
            (idx + list.len() - 1) % list.len()
        } else {
            (idx + 1) % list.len()
        };
        list[next]
    }
}

/// Capability state probed lazily against `/api/v1/creations/review-queue` (silent on HTTP 403).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewerAccessState {
    #[default]
    Unknown,
    Allowed,
    Forbidden,
}

/// Complete modal state for Creator submission ("推送审核上架") and Admin Reviewer Queue.
#[derive(Debug, Clone, Default)]
pub struct PublishReviewModalState {
    pub is_open: bool,
    pub pending_close_ime_clear: bool,
    pub show_reviewer_queue_tab: bool,
    pub pending_open_reviewer_queue: bool,
    pub target_creation_id: Option<String>,
    pub base_creation: Option<UserCreationDto>,
    pub title: EditableFieldState,
    pub subtitle: EditableFieldState,
    pub author_display_name: EditableFieldState,
    pub summary: EditableFieldState,
    pub genres: EditableFieldState,
    pub visibility: CreationVisibility,
    pub review_notes: EditableFieldState,
    pub reject_reason: EditableFieldState,
    pub focused_field: Option<PublishModalField>,
    pub validation_errors: Vec<String>,
    pub server_error: Option<String>,
    pub status_notice: Option<String>,
    pub is_submitting: bool,
    pub audit_trail: Vec<CreationReviewAuditDto>,
    pub reviewer_access: ReviewerAccessState,
    pub review_queue: Vec<UserCreationDto>,
    pub selected_review_id: Option<String>,
    pub reviewer_error: Option<String>,
    pub is_loading_queue: bool,
    pub language_id: String,
}

impl PublishReviewModalState {
    pub fn is_showing_reviewer_queue(&self) -> bool {
        self.show_reviewer_queue_tab && self.reviewer_access == ReviewerAccessState::Allowed
    }

    pub fn open_for_creation(&mut self, creation: UserCreationDto) {
        self.clear_all_preedits();
        let vis = match creation.visibility {
            CreationVisibility::Unlisted => CreationVisibility::Unlisted,
            CreationVisibility::Public | CreationVisibility::Private => CreationVisibility::Public,
        };
        self.is_open = true;
        self.pending_close_ime_clear = false;
        self.show_reviewer_queue_tab = false;
        self.pending_open_reviewer_queue = false;
        self.target_creation_id = Some(creation.creation_id.clone());
        self.title = EditableFieldState::new(creation.title.clone());
        self.subtitle = EditableFieldState::new(creation.subtitle.clone());
        self.author_display_name = EditableFieldState::new(creation.author_display_name.clone());
        self.summary = EditableFieldState::new(creation.summary.clone());
        self.genres = EditableFieldState::new(creation.genres.join(", "));
        self.visibility = vis;
        self.review_notes =
            EditableFieldState::new(creation.review_notes.clone().unwrap_or_default());
        self.base_creation = Some(creation);
        self.focused_field = Some(PublishModalField::Title);
        self.server_error = None;
        self.status_notice = None;
        self.is_submitting = false;
        self.audit_trail.clear();
        self.recompute_validation();
    }

    pub fn reconcile_selected_review_id(&mut self) {
        let valid = self.selected_review_id.as_deref().is_some_and(|sel| {
            self.review_queue
                .iter()
                .any(|item| item.creation_id == sel)
        });
        if !valid {
            self.selected_review_id = self
                .review_queue
                .first()
                .map(|item| item.creation_id.clone());
        }
    }

    pub fn prune_review_queue_item(&mut self, creation_id: &str) -> bool {
        let before = self.review_queue.len();
        self.review_queue
            .retain(|item| item.creation_id != creation_id);
        let removed = self.review_queue.len() != before;
        if removed {
            self.reconcile_selected_review_id();
        }
        removed
    }

    pub fn open_reviewer_queue(&mut self) {
        self.clear_all_preedits();
        self.is_open = true;
        self.pending_close_ime_clear = false;
        self.reviewer_error = None;
        if self.reviewer_access == ReviewerAccessState::Allowed {
            self.show_reviewer_queue_tab = true;
            self.pending_open_reviewer_queue = false;
            self.reconcile_selected_review_id();
            self.focused_field = Some(PublishModalField::RejectReason);
        } else {
            self.show_reviewer_queue_tab = false;
            self.pending_open_reviewer_queue = true;
            self.focused_field = Some(PublishModalField::Title);
        }
    }

    pub fn close(&mut self) {
        let was_open = self.is_open;
        self.is_open = false;
        self.pending_open_reviewer_queue = false;
        self.focused_field = None;
        self.server_error = None;
        self.reviewer_error = None;
        self.is_submitting = false;
        self.clear_all_preedits();
        if was_open {
            self.pending_close_ime_clear = true;
        }
    }

    pub fn reset_for_logout_or_account_switch(&mut self) {
        let lang = std::mem::take(&mut self.language_id);
        *self = Self::default();
        self.language_id = lang;
    }

    pub fn has_any_preedit(&self) -> bool {
        self.title.ime_preedit.is_some()
            || self.subtitle.ime_preedit.is_some()
            || self.author_display_name.ime_preedit.is_some()
            || self.summary.ime_preedit.is_some()
            || self.genres.ime_preedit.is_some()
            || self.review_notes.ime_preedit.is_some()
            || self.reject_reason.ime_preedit.is_some()
    }

    pub fn clear_all_preedits(&mut self) {
        self.title.set_ime_preedit(None);
        self.subtitle.set_ime_preedit(None);
        self.author_display_name.set_ime_preedit(None);
        self.summary.set_ime_preedit(None);
        self.genres.set_ime_preedit(None);
        self.review_notes.set_ime_preedit(None);
        self.reject_reason.set_ime_preedit(None);
    }

    pub fn has_focused_field(&self) -> bool {
        self.is_open && self.focused_field.is_some()
    }

    pub fn is_active_field_multiline(&self) -> bool {
        self.focused_field.is_some_and(PublishModalField::is_multiline)
    }

    pub fn parsed_genres(&self) -> Vec<String> {
        self.genres
            .text
            .split([',', '，'])
            .map(|entry| {
                entry
                    .trim_start_matches(|c: char| {
                        c.is_whitespace() || c.is_control() || is_invisible_format_char(c)
                    })
                    .trim_end_matches(|c: char| {
                        c.is_whitespace()
                            || c.is_control()
                            || (is_invisible_format_char(c)
                                && !matches!(c, '\u{FE0E}' | '\u{FE0F}'))
                    })
            })
            .filter(|s| visible_char_count(s) > 0)
            .map(str::to_string)
            .collect()
    }

    pub fn preview_dto(&self) -> Option<UserCreationDto> {
        let mut dto = self.base_creation.clone()?;
        dto.title = self.title.text.trim().to_string();
        dto.subtitle = self.subtitle.text.trim().to_string();
        dto.author_display_name = self.author_display_name.text.trim().to_string();
        dto.summary = self.summary.text.trim().to_string();
        dto.genres = self.parsed_genres();
        dto.visibility = self.visibility;
        // Mirror server `submit_creation_for_review` (`creations_store.rs:1327`):
        // stale moderation feedback is cleared on the next submission cycle while
        // `self.base_creation.rejection_reason` stays intact for UI display.
        dto.rejection_reason = None;
        let notes = self.review_notes.text.trim();
        dto.review_notes = if visible_char_count(notes) == 0 {
            None
        } else {
            Some(notes.to_string())
        };
        Some(dto)
    }

    pub fn recompute_validation(&mut self) {
        if let Some(preview) = self.preview_dto() {
            let mut errors = match validate_creation_for_review(&preview) {
                Ok(()) => Vec::new(),
                Err(errs) => errs,
            };
            if visible_char_count(&preview.title) == 0
                && !errors.iter().any(|e| e.starts_with("title "))
            {
                errors.push(
                    tr_str(
                        &self.language_id,
                        "作品标题 (title) 必须包含至少 1 个可见字符",
                        "title must contain at least 1 visible character",
                    )
                    .to_string(),
                );
            }
            if visible_char_count(&preview.author_display_name) == 0
                && !errors
                    .iter()
                    .any(|e| e.starts_with("author_display_name "))
            {
                errors.push(
                    tr_str(
                        &self.language_id,
                        "作者署名 (author_display_name) 必须包含至少 1 个可见字符",
                        "author_display_name must contain at least 1 visible character",
                    )
                    .to_string(),
                );
            }
            if visible_char_count(&preview.summary) < MIN_CREATION_SUMMARY_CHARS
                && !errors.iter().any(|e| e.starts_with("summary "))
            {
                errors.push(
                    tr_str(
                        &self.language_id,
                        "作品简介 (summary) 至少需要 10 个可见字符",
                        "summary must contain at least 10 visible characters",
                    )
                    .to_string(),
                );
            }
            for (i, genre) in preview.genres.iter().enumerate() {
                let prefix = format!("genres[{i}] ");
                if visible_char_count(genre) == 0 && !errors.iter().any(|e| e.starts_with(&prefix))
                {
                    errors.push(format!("genres[{i}] must contain visible characters"));
                }
            }
            for (i, tag) in preview.tags.iter().enumerate() {
                let prefix = format!("tags[{i}] ");
                if visible_char_count(tag) == 0 && !errors.iter().any(|e| e.starts_with(&prefix)) {
                    errors.push(format!("tags[{i}] must contain visible characters"));
                }
            }
            validate_payload_visible_fields(&preview.payload, &mut errors);
            if let Ok(val) = serde_json::to_value(&preview) {
                collect_obscured_xss_json_errors(&mut errors, "", &val);
            } else {
                let check_xss = |errors: &mut Vec<String>, field: &str, val: &str| {
                    if contains_obscured_xss_pattern(val) {
                        let msg =
                            format!("{field} contains disallowed script or unsafe URI scheme");
                        if !errors.iter().any(|e| e == &msg) {
                            errors.push(msg);
                        }
                    }
                };
                check_xss(&mut errors, "title", &preview.title);
                check_xss(&mut errors, "subtitle", &preview.subtitle);
                check_xss(
                    &mut errors,
                    "author_display_name",
                    &preview.author_display_name,
                );
                check_xss(&mut errors, "summary", &preview.summary);
                for (i, genre) in preview.genres.iter().enumerate() {
                    check_xss(&mut errors, &format!("genres[{i}]"), genre);
                }
                for (i, tag) in preview.tags.iter().enumerate() {
                    check_xss(&mut errors, &format!("tags[{i}]"), tag);
                }
                if let Some(notes) = preview.review_notes.as_deref() {
                    check_xss(&mut errors, "review_notes", notes);
                }
            }
            self.validation_errors = errors;
        } else {
            self.validation_errors.clear();
        }
    }

    pub fn has_metadata_changes(&self) -> bool {
        let Some(base) = self.base_creation.as_ref() else {
            return false;
        };
        base.title.trim() != self.title.text.trim()
            || base.subtitle.trim() != self.subtitle.text.trim()
            || base.author_display_name.trim() != self.author_display_name.text.trim()
            || base.summary.trim() != self.summary.text.trim()
            || base.genres != self.parsed_genres()
            || base.visibility != self.visibility
    }

    pub fn build_update_request(&self) -> Option<UpdateCreationRequest> {
        let base = self.base_creation.as_ref()?;
        Some(UpdateCreationRequest {
            expected_revision: Some(base.cloud_revision),
            title: Some(self.title.text.trim().to_string()),
            subtitle: Some(self.subtitle.text.trim().to_string()),
            summary: Some(self.summary.text.trim().to_string()),
            author_display_name: Some(self.author_display_name.text.trim().to_string()),
            genres: Some(self.parsed_genres()),
            visibility: Some(self.visibility),
            ..Default::default()
        })
    }

    pub fn build_submit_request(&self) -> SubmitReviewRequest {
        let notes = self.review_notes.text.trim();
        SubmitReviewRequest {
            visibility: Some(self.visibility),
            notes: if visible_char_count(notes) == 0 {
                None
            } else {
                Some(notes.to_string())
            },
        }
    }

    pub fn build_approve_request(&self, visibility: CreationVisibility) -> ReviewDecisionRequest {
        let target_vis = match visibility {
            CreationVisibility::Unlisted => CreationVisibility::Unlisted,
            _ => CreationVisibility::Public,
        };
        ReviewDecisionRequest {
            decision: ReviewDecision::Approve,
            visibility: Some(target_vis),
            reason: None,
            notes: None,
        }
    }

    pub fn build_reject_request(&mut self, lang_id: &str) -> Result<ReviewDecisionRequest, String> {
        let Some(reason) = normalize_non_empty_reason(&self.reject_reason.text) else {
            let err = tr_str(
                lang_id,
                "拒绝上架时必须填写退回原因",
                "A non-empty rejection reason is required before rejecting.",
            )
            .to_string();
            self.reviewer_error = Some(err.clone());
            return Err(err);
        };
        self.reviewer_error = None;
        Ok(ReviewDecisionRequest {
            decision: ReviewDecision::Reject,
            visibility: None,
            reason: Some(reason),
            notes: None,
        })
    }
}

impl PublishReviewModalState {
    pub fn field(&self, target: PublishModalField) -> &EditableFieldState {
        match target {
            PublishModalField::Title => &self.title,
            PublishModalField::Subtitle => &self.subtitle,
            PublishModalField::AuthorDisplayName => &self.author_display_name,
            PublishModalField::Summary => &self.summary,
            PublishModalField::Genres => &self.genres,
            PublishModalField::ReviewNotes => &self.review_notes,
            PublishModalField::RejectReason => &self.reject_reason,
        }
    }

    pub fn field_mut(&mut self, target: PublishModalField) -> &mut EditableFieldState {
        match target {
            PublishModalField::Title => &mut self.title,
            PublishModalField::Subtitle => &mut self.subtitle,
            PublishModalField::AuthorDisplayName => &mut self.author_display_name,
            PublishModalField::Summary => &mut self.summary,
            PublishModalField::Genres => &mut self.genres,
            PublishModalField::ReviewNotes => &mut self.review_notes,
            PublishModalField::RejectReason => &mut self.reject_reason,
        }
    }

    pub fn active_text_and_preedit(
        &self,
        fallback_ime: &TextInputController,
    ) -> (String, Option<String>) {
        if let Some(target) = self.focused_field {
            self.field(target).text_and_preedit(fallback_ime)
        } else {
            (String::new(), None)
        }
    }

    pub fn insert_text(&mut self, text: &str) -> bool {
        let Some(target) = self.focused_field else {
            return false;
        };
        self.field_mut(target).insert_text(text);
        if target == PublishModalField::RejectReason {
            self.reviewer_error = None;
        } else {
            self.server_error = None;
            self.recompute_validation();
        }
        true
    }

    pub fn clear_active_field(&mut self) -> bool {
        let Some(target) = self.focused_field else {
            return false;
        };
        self.field_mut(target).clear();
        if target != PublishModalField::RejectReason {
            self.recompute_validation();
        }
        true
    }

    pub fn select_all_active_field(&mut self) -> bool {
        let Some(target) = self.focused_field else {
            return false;
        };
        self.field_mut(target).select_all_text();
        true
    }

    pub fn set_ime_preedit(&mut self, preedit: Option<&str>) {
        if preedit.is_none() {
            self.clear_all_preedits();
        } else if let Some(target) = self.focused_field {
            self.field_mut(target).set_ime_preedit(preedit);
        }
    }

    pub fn set_visibility(&mut self, vis: CreationVisibility) {
        self.visibility = vis;
        self.server_error = None;
        self.recompute_validation();
    }

    pub fn handle_non_char_key(&mut self, key: &str, is_shift: bool, is_alt: bool) -> bool {
        match key {
            "tab" => {
                self.clear_all_preedits();
                if !self.is_showing_reviewer_queue() {
                    let next = match self.focused_field {
                        Some(cur) if cur != PublishModalField::RejectReason => {
                            cur.next_in_submit(is_shift)
                        }
                        _ => PublishModalField::Title,
                    };
                    self.focused_field = Some(next);
                }
                true
            }
            "backspace" => {
                if let Some(target) = self.focused_field {
                    let changed = self.field_mut(target).backspace(is_alt);
                    if changed && target != PublishModalField::RejectReason {
                        self.recompute_validation();
                    }
                }
                true
            }
            "delete" => {
                if let Some(target) = self.focused_field {
                    let changed = self.field_mut(target).delete_forward();
                    if changed && target != PublishModalField::RejectReason {
                        self.recompute_validation();
                    }
                }
                true
            }
            "left" | "arrowleft" => {
                if let Some(target) = self.focused_field {
                    self.field_mut(target).move_left();
                }
                true
            }
            "right" | "arrowright" => {
                if let Some(target) = self.focused_field {
                    self.field_mut(target).move_right();
                }
                true
            }
            "home" => {
                if let Some(target) = self.focused_field {
                    self.field_mut(target).move_home();
                }
                true
            }
            "end" => {
                if let Some(target) = self.focused_field {
                    self.field_mut(target).move_end();
                }
                true
            }
            "enter" => {
                if let Some(target) = self.focused_field {
                    if target.is_multiline() {
                        self.field_mut(target).insert_text("\n");
                        if target != PublishModalField::RejectReason {
                            self.recompute_validation();
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }
}

fn render_field_row(
    hub: Entity<ContentHubWorkspace>,
    id: &'static str,
    label: &str,
    placeholder: &str,
    target: PublishModalField,
    state: &PublishReviewModalState,
    pal: TdesktopThemePalette,
) -> Div {
    let ob = pal.openbot_palette();
    let field = state.field(target);
    let focused = state.focused_field == Some(target);
    let multiline = target.is_multiline();
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ob.text_secondary())
                .child(label.to_string()),
        )
        .child(
            render_editable_field_box(id, field, placeholder, focused, multiline, pal).on_mouse_down(
                MouseButton::Left,
                move |_, _, cx| {
                    hub.update(cx, |h, hcx| {
                        h.publish_modal.clear_all_preedits();
                        h.publish_modal.focused_field = Some(target);
                        hcx.emit(ContentHubEvent::FocusInputTaken);
                        hcx.notify();
                    });
                },
            ),
        )
}

/// Renders the interactive body of the "推送审核上架" modal sheet (Creator Submit + Admin Reviewer Queue).
pub fn render_publish_review_modal_body(
    hub: Entity<ContentHubWorkspace>,
    cx: &App,
) -> AnyElement {
    let (pal, lang, modal) = {
        let h = hub.read(cx);
        (h.palette, h.language_id.clone(), h.publish_modal.clone())
    };
    let ob = pal.openbot_palette();
    let hover_close = ob.hover();
    let reviewer_allowed = modal.reviewer_access == ReviewerAccessState::Allowed;
    let show_queue = modal.is_showing_reviewer_queue();
    let hub_close = hub.clone();
    let hub_tab_submit = hub.clone();
    let hub_tab_queue = hub.clone();

    div()
        .id("publish-review-modal-body")
        .size_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div()
                .flex_shrink_0()
                .px(px(20.0))
                .py(px(14.0))
                .border_b_1()
                .border_color(ob.border_hairline())
                .flex()
                .flex_wrap()
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
                                .w(px(28.0))
                                .h(px(28.0))
                                .rounded_full()
                                .bg(ob.bg_control_active())
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(tg_icon_rgba(IconName::HubPublish, 15.0, ob.text_primary())),
                        )
                        .child(
                            div()
                                .text_size(px(15.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.text_primary())
                                .child(tr_str(&lang, "推送审核上架", "Publish & Review")),
                        )
                        .when(reviewer_allowed, |el| {
                            el.child(
                                filter_pill(
                                    "publish-modal-tab-submit",
                                    tr_str(&lang, "提交上架", "Submit"),
                                    !show_queue,
                                    pal,
                                )
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    hub_tab_submit.update(cx, |h, hcx| {
                                        h.publish_modal.clear_all_preedits();
                                        h.publish_modal.show_reviewer_queue_tab = false;
                                        h.publish_modal.pending_open_reviewer_queue = false;
                                        h.publish_modal.focused_field =
                                            Some(PublishModalField::Title);
                                        hcx.emit(ContentHubEvent::FocusInputTaken);
                                        hcx.notify();
                                    });
                                }),
                            )
                            .child(
                                filter_pill(
                                    "publish-modal-tab-queue",
                                    format!(
                                        "{} ({})",
                                        tr_str(&lang, "审核队列", "Review Queue"),
                                        modal.review_queue.len()
                                    ),
                                    show_queue,
                                    pal,
                                )
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    hub_tab_queue.update(cx, |h, hcx| {
                                        h.publish_modal.clear_all_preedits();
                                        h.publish_modal.open_reviewer_queue();
                                        hcx.emit(ContentHubEvent::FocusInputTaken);
                                        hcx.emit(ContentHubEvent::RefreshReviewQueue);
                                        hcx.notify();
                                    });
                                }),
                            )
                        }),
                )
                .child(
                    div()
                        .id("publish-review-modal-close-btn")
                        .cursor_pointer()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded_full()
                        .hover(move |s| s.bg(hover_close))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            hub_close.update(cx, |h, hcx| {
                                if h.publish_modal.is_open {
                                    h.publish_modal.close();
                                    hcx.emit(ContentHubEvent::ClosePublishReviewModal);
                                    hcx.notify();
                                }
                            });
                        })
                        .child(tg_icon_rgba(IconName::Close, 14.0, ob.text_muted())),
                ),
        )
        .child(if show_queue {
            render_reviewer_queue_tab(hub, &modal, pal, &lang)
        } else {
            render_creator_submit_tab(hub, &modal, pal, &lang)
        })
        .into_any_element()
}

fn render_creator_submit_tab(
    hub: Entity<ContentHubWorkspace>,
    modal: &PublishReviewModalState,
    pal: TdesktopThemePalette,
    lang: &str,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let succ_soft = gpui::Rgba { a: 0.14, ..ob.success() };
    let succ_border = gpui::Rgba { a: 0.30, ..ob.success() };
    let danger_soft = gpui::Rgba { a: 0.14, ..ob.danger() };
    let danger_border = gpui::Rgba { a: 0.32, ..ob.danger() };
    let hub_vis_pub = hub.clone();
    let hub_vis_unlisted = hub.clone();
    let hub_save = hub.clone();
    let hub_submit = hub.clone();
    let can_submit =
        modal.base_creation.is_some() && modal.validation_errors.is_empty() && !modal.is_submitting;

    div()
        .id("publish-modal-submit-scroll")
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .overflow_y_scroll()
        .p(px(20.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .when_some(modal.base_creation.as_ref(), |el, base| {
            let accent = creation_kind_accent(base.kind);
            el.child(
                apple_grouped_card(pal)
                    .p(px(12.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(cover_artwork_box(&base.cover, 44.0, 44.0, 9.0, accent))
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
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(ob.text_primary())
                                            .child(format!(
                                                "{} · {}",
                                                creation_kind_label(base.kind, lang),
                                                base.creation_id
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(ob.text_muted())
                                            .child(format!("Cloud Revision #{}", base.cloud_revision)),
                                    ),
                            ),
                    )
                    .child(review_status_badge(base.review_status, lang, pal)),
            )
        })
        .child(render_field_row(
            hub.clone(),
            "publish-modal-field-title",
            tr_str(lang, "作品标题", "Title"),
            tr_str(lang, "输入作品标题 (1-120 字)", "Enter title (1-120 chars)"),
            PublishModalField::Title,
            modal,
            pal,
        ))
        .child(render_field_row(
            hub.clone(),
            "publish-modal-field-subtitle",
            tr_str(lang, "副标题", "Subtitle"),
            tr_str(lang, "可选副标题", "Optional subtitle"),
            PublishModalField::Subtitle,
            modal,
            pal,
        ))
        .child(render_field_row(
            hub.clone(),
            "publish-modal-field-author",
            tr_str(lang, "作者 / 创作者署名", "Author / Creator Name"),
            tr_str(lang, "创作者名称", "Author display name"),
            PublishModalField::AuthorDisplayName,
            modal,
            pal,
        ))
        .child(render_field_row(
            hub.clone(),
            "publish-modal-field-summary",
            tr_str(lang, "作品简介 (至少 10 字)", "Summary (min 10 chars)"),
            tr_str(lang, "描述你的作品亮点与内容…", "Describe your creation…"),
            PublishModalField::Summary,
            modal,
            pal,
        ))
        .child(render_field_row(
            hub.clone(),
            "publish-modal-field-genres",
            tr_str(lang, "分类标签 (逗号分隔，最多 5 项)", "Genres (comma-separated, max 5)"),
            tr_str(lang, "例如：科幻, 冒险", "e.g. Sci-Fi, Adventure"),
            PublishModalField::Genres,
            modal,
            pal,
        ))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ob.text_secondary())
                        .child(tr_str(lang, "上架可见性", "Catalog Visibility")),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            filter_pill(
                                "publish-modal-vis-public",
                                tr_str(lang, "公开上架 (可被发现与 Bot 推荐)", "Public (Discover & Bot)"),
                                modal.visibility == CreationVisibility::Public,
                                pal,
                            )
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                hub_vis_pub.update(cx, |h, hcx| {
                                    h.publish_modal.set_visibility(CreationVisibility::Public);
                                    hcx.notify();
                                });
                            }),
                        )
                        .child(
                            filter_pill(
                                "publish-modal-vis-unlisted",
                                tr_str(lang, "仅链接可见", "Unlisted"),
                                modal.visibility == CreationVisibility::Unlisted,
                                pal,
                            )
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                hub_vis_unlisted.update(cx, |h, hcx| {
                                    h.publish_modal.set_visibility(CreationVisibility::Unlisted);
                                    hcx.notify();
                                });
                            }),
                        ),
                ),
        )
        .child(render_field_row(
            hub.clone(),
            "publish-modal-field-notes",
            tr_str(lang, "审核备注", "Review Notes"),
            tr_str(lang, "给审核员的补充说明 (可选)", "Optional notes for reviewer"),
            PublishModalField::ReviewNotes,
            modal,
            pal,
        ))
        .child(
            div()
                .p(px(12.0))
                .rounded(px(10.0))
                .bg(if modal.validation_errors.is_empty() {
                    succ_soft
                } else {
                    danger_soft
                })
                .border_1()
                .border_color(if modal.validation_errors.is_empty() {
                    succ_border
                } else {
                    danger_border
                })
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if modal.validation_errors.is_empty() {
                            ob.success()
                        } else {
                            ob.danger()
                        })
                        .child(if modal.validation_errors.is_empty() {
                            tr_str(lang, "✓ 实时审核校验通过，可立即提交审核", "✓ Live review validation passed")
                        } else {
                            tr_str(lang, "提交前请修复以下校验项：", "Fix validation issues before submitting:")
                        }),
                )
                .children(modal.validation_errors.iter().map(|err| {
                    div()
                        .text_size(px(11.0))
                        .text_color(ob.danger())
                        .child(format!("• {err}"))
                })),
        )
        .when_some(modal.server_error.as_ref(), |el, err| {
            el.child(
                div()
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(danger_soft)
                    .text_size(px(12.0))
                    .text_color(ob.danger())
                    .child(err.clone()),
            )
        })
        .when_some(modal.status_notice.as_ref(), |el, note| {
            el.child(
                div()
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(succ_soft)
                    .text_size(px(12.0))
                    .text_color(ob.success())
                    .child(note.clone()),
            )
        })
        .child(
            div()
                .pt(px(4.0))
                .flex()
                .items_center()
                .justify_end()
                .gap(px(10.0))
                .child(
                    openbot_secondary_btn(
                        "publish-modal-save-draft-btn",
                        tr_str(lang, "保存草稿", "Save Draft"),
                        pal,
                    )
                    .when(modal.is_submitting, |el| el.opacity(0.55))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        hub_save.update(cx, |h, hcx| {
                            if h.publish_modal.is_submitting {
                                return;
                            }
                            if let (Some(cid), Some(req)) = (
                                h.publish_modal.target_creation_id.clone(),
                                h.publish_modal.build_update_request(),
                            ) {
                                h.publish_modal.is_submitting = true;
                                h.publish_modal.server_error = None;
                                hcx.emit(ContentHubEvent::SaveCreationDraft {
                                    creation_id: cid,
                                    req,
                                });
                                hcx.notify();
                            }
                        });
                    }),
                )
                .child(
                    openbot_primary_cta_btn(
                        "publish-modal-submit-review-btn",
                        if modal.is_submitting {
                            tr_str(lang, "提交中…", "Submitting…")
                        } else {
                            tr_str(lang, "推送审核上架", "Submit for Review")
                        },
                        pal,
                    )
                    .when(!can_submit, |el| el.opacity(0.55))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        hub_submit.update(cx, |h, hcx| {
                            h.publish_modal.recompute_validation();
                            if !h.publish_modal.validation_errors.is_empty()
                                || h.publish_modal.is_submitting
                            {
                                hcx.notify();
                                return;
                            }
                            if let Some(cid) = h.publish_modal.target_creation_id.clone() {
                                let update_req = if h.publish_modal.has_metadata_changes() {
                                    h.publish_modal.build_update_request()
                                } else {
                                    None
                                };
                                let submit_req = h.publish_modal.build_submit_request();
                                h.publish_modal.is_submitting = true;
                                h.publish_modal.server_error = None;
                                hcx.emit(ContentHubEvent::SubmitCreationForReview {
                                    creation_id: cid,
                                    update_req,
                                    submit_req,
                                });
                                hcx.notify();
                            }
                        });
                    }),
                ),
        )
        .when(!modal.audit_trail.is_empty(), |el| {
            el.child(apple_hairline(pal)).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ob.text_muted())
                            .child(tr_str(lang, "审核审计记录", "Audit History")),
                    )
                    .children(modal.audit_trail.iter().take(6).map(|entry| {
                        let line = format!(
                            "{} -> {} ({})",
                            entry.from_status.as_str(),
                            entry.to_status.as_str(),
                            entry.action
                        );
                        div()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(ob.bg_control())
                            .text_size(px(11.0))
                            .text_color(ob.text_secondary())
                            .child(line)
                    })),
            )
        })
        .into_any_element()
}

fn render_reviewer_queue_tab(
    hub: Entity<ContentHubWorkspace>,
    modal: &PublishReviewModalState,
    pal: TdesktopThemePalette,
    lang: &str,
) -> AnyElement {
    let ob = pal.openbot_palette();
    let danger_soft = gpui::Rgba { a: 0.14, ..ob.danger() };
    let selected_id = modal
        .selected_review_id
        .clone()
        .or_else(|| modal.review_queue.first().map(|c| c.creation_id.clone()));
    let hub_refresh = hub.clone();

    div()
        .id("publish-modal-reviewer-scroll")
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .overflow_y_scroll()
        .p(px(20.0))
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
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ob.text_primary())
                        .child(format!(
                            "{} ({})",
                            tr_str(lang, "待审核作品", "Pending Review Submissions"),
                            modal.review_queue.len()
                        )),
                )
                .child(
                    openbot_secondary_btn(
                        "reviewer-queue-refresh-btn",
                        tr_str(lang, "刷新队列", "Refresh Queue"),
                        pal,
                    )
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        hub_refresh.update(cx, |_, hcx| {
                            hcx.emit(ContentHubEvent::RefreshReviewQueue);
                        });
                    }),
                ),
        )
        .when(modal.review_queue.is_empty(), |el| {
            el.child(
                apple_grouped_card(pal).p(px(20.0)).child(
                    div()
                        .text_size(px(13.0))
                        .text_color(ob.text_muted())
                        .child(tr_str(
                            lang,
                            "当前没有待审核的作品。",
                            "No pending submissions in the review queue.",
                        )),
                ),
            )
        })
        .children(modal.review_queue.iter().map(|item| {
            let cid = item.creation_id.clone();
            let cid_sel = cid.clone();
            let cid_pub = cid.clone();
            let cid_unl = cid.clone();
            let cid_rej = cid.clone();
            let is_sel = selected_id.as_deref() == Some(cid.as_str());
            let hub_sel = hub.clone();
            let hub_pub = hub.clone();
            let hub_unl = hub.clone();
            let hub_rej = hub.clone();
            let accent = creation_kind_accent(item.kind);
            let pending_badge = review_status_badge(CreationReviewStatus::PendingReview, lang, pal);
            let lang_owned = lang.to_string();

            apple_grouped_card(pal)
                .p(px(12.0))
                .when(is_sel, |c| c.border_color(ob.border_strong_hairline()))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .id(SharedString::from(format!("reviewer-item-{cid}")))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            hub_sel.update(cx, |h, hcx| {
                                h.publish_modal.selected_review_id = Some(cid_sel.clone());
                                hcx.notify();
                            });
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(cover_artwork_box(&item.cover, 40.0, 40.0, 8.0, accent))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(14.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(ob.text_primary())
                                                .child(item.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(11.0))
                                                .text_color(ob.text_muted())
                                                .child(format!(
                                                    "{} · {} · {}",
                                                    creation_kind_label(item.kind, lang),
                                                    item.author_display_name,
                                                    item.summary
                                                )),
                                        ),
                                ),
                        )
                        .child(pending_badge),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id(SharedString::from(format!("reviewer-approve-public-{cid}")))
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(8.0))
                                .bg(ob.success())
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xFFFFFF))
                                .when(modal.is_submitting, |el| el.opacity(0.55))
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    hub_pub.update(cx, |h, hcx| {
                                        if h.publish_modal.is_submitting {
                                            return;
                                        }
                                        let req = h
                                            .publish_modal
                                            .build_approve_request(CreationVisibility::Public);
                                        h.publish_modal.is_submitting = true;
                                        h.publish_modal.reviewer_error = None;
                                        hcx.emit(ContentHubEvent::SubmitReviewDecision {
                                            creation_id: cid_pub.clone(),
                                            req,
                                        });
                                        hcx.notify();
                                    });
                                })
                                .child(tr_str(lang, "通过并公开上架", "Approve (Public)")),
                        )
                        .child(
                            openbot_secondary_btn(
                                SharedString::from(format!("reviewer-approve-unlisted-{cid}")),
                                tr_str(lang, "通过 (仅链接可见)", "Approve (Unlisted)"),
                                pal,
                            )
                            .px(px(10.0))
                            .py(px(5.0))
                            .text_size(px(12.0))
                            .when(modal.is_submitting, |el| el.opacity(0.55))
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                hub_unl.update(cx, |h, hcx| {
                                    if h.publish_modal.is_submitting {
                                        return;
                                    }
                                    let req = h
                                        .publish_modal
                                        .build_approve_request(CreationVisibility::Unlisted);
                                    h.publish_modal.is_submitting = true;
                                    h.publish_modal.reviewer_error = None;
                                    hcx.emit(ContentHubEvent::SubmitReviewDecision {
                                        creation_id: cid_unl.clone(),
                                        req,
                                    });
                                    hcx.notify();
                                });
                            }),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("reviewer-reject-{cid}")))
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(8.0))
                                .bg(danger_soft)
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ob.danger())
                                .when(modal.is_submitting, |el| el.opacity(0.55))
                                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                    hub_rej.update(cx, |h, hcx| {
                                        if h.publish_modal.is_submitting {
                                            return;
                                        }
                                        match h.publish_modal.build_reject_request(&lang_owned) {
                                            Ok(req) => {
                                                h.publish_modal.is_submitting = true;
                                                h.publish_modal.reviewer_error = None;
                                                hcx.emit(ContentHubEvent::SubmitReviewDecision {
                                                    creation_id: cid_rej.clone(),
                                                    req,
                                                });
                                                hcx.notify();
                                            }
                                            Err(_) => {
                                                h.publish_modal.clear_all_preedits();
                                                h.publish_modal.focused_field =
                                                    Some(PublishModalField::RejectReason);
                                                hcx.emit(ContentHubEvent::FocusInputTaken);
                                                hcx.notify();
                                            }
                                        }
                                    });
                                })
                                .child(tr_str(lang, "退回修改", "Reject")),
                        ),
                )
        }))
        .child(render_field_row(
            hub,
            "reviewer-reject-reason-input",
            tr_str(lang, "退回原因 (拒绝时必填)", "Rejection Reason (required to reject)"),
            tr_str(lang, "请填写具体的修改建议或退回原因…", "Enter mandatory rejection reason…"),
            PublishModalField::RejectReason,
            modal,
            pal,
        ))
        .when_some(modal.reviewer_error.as_ref(), |el, err| {
            el.child(
                div()
                    .text_size(px(12.0))
                    .text_color(ob.danger())
                    .child(err.clone()),
            )
        })
        .into_any_element()
}
