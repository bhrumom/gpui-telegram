//! Field Autocomplete for Mentions (@), Hashtags (#), Bot Commands (/), and Stickers
//!
//! Upstream C++ Reference:
//! - `Telegram/SourceFiles/chat_helpers/field_autocomplete.h:L1-L222`
//! - `Telegram/SourceFiles/chat_helpers/field_autocomplete.cpp:L1-L2163`
//! - `Telegram/SourceFiles/chat_helpers/field_autocomplete.cpp:L66` (`kEphemeralHintHoverDelay`)
//! - `Telegram/SourceFiles/chat_helpers/field_autocomplete.cpp:L404-L417` (Trigger parsing)
//! - `Telegram/SourceFiles/chat_helpers/field_autocomplete.cpp:L1530-L1562` (`moveSel` navigation math)
//!
//! Subsystem: chat_helpers, Feature: TCG-N11

use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub const K_EPHEMERAL_HINT_HOVER_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutocompleteType {
    Mentions,
    Hashtags,
    BotCommands,
    Stickers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutocompleteChooseMethod {
    ByEnter,
    ByTab,
    ByClick,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionRow {
    pub user_id: u64,
    pub username: String,
    pub name: String,
    pub is_bot: bool,
}

impl MentionRow {
    pub fn new(
        user_id: u64,
        username: impl Into<String>,
        name: impl Into<String>,
        is_bot: bool,
    ) -> Self {
        Self {
            user_id,
            username: username.into(),
            name: name.into(),
            is_bot,
        }
    }

    pub fn is_everyone(&self) -> bool {
        let u = self.username.trim().trim_start_matches('@');
        u.eq_ignore_ascii_case("everyone") || u.eq_ignore_ascii_case("all")
    }

    /// Default role badge label used when no explicit role override is registered on `FieldAutocomplete`.
    pub fn default_role_hint(&self) -> &'static str {
        if self.is_everyone() {
            "所有人"
        } else if self.is_bot {
            "AI Bot"
        } else {
            "成员"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashtagRow {
    pub tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotCommandRow {
    pub command: String,
    pub description: String,
    pub bot_user_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickerRow {
    pub document_id: u64,
    pub emoji: String,
}

/// Field Autocomplete controller managing popups for `@user`, `#hashtag`, `/command` and stickers.
/// Target symbol in ledger: `FieldAutocomplete`.
#[derive(Debug, Clone)]
pub struct FieldAutocomplete {
    pub is_open: bool,
    pub autocomplete_type: AutocompleteType,
    pub filter: String,
    pub selected_index: i32,
    pub stickers_per_row: usize,

    pub mention_rows: Vec<MentionRow>,
    pub all_mention_candidates: Vec<MentionRow>,
    pub mention_role_hints: HashMap<String, String>,
    pub hashtag_rows: Vec<HashtagRow>,
    pub bot_command_rows: Vec<BotCommandRow>,
    pub sticker_rows: Vec<StickerRow>,
}

impl Default for FieldAutocomplete {
    fn default() -> Self {
        Self {
            is_open: false,
            autocomplete_type: AutocompleteType::Mentions,
            filter: String::new(),
            selected_index: -1,
            stickers_per_row: 5,
            mention_rows: Vec::new(),
            all_mention_candidates: Vec::new(),
            mention_role_hints: HashMap::new(),
            hashtag_rows: Vec::new(),
            bot_command_rows: Vec::new(),
            sticker_rows: Vec::new(),
        }
    }
}

impl FieldAutocomplete {
    pub fn new() -> Self {
        Self::default()
    }

    /// Populates the authoritative `@mention` candidate roster for the active chat/group,
    /// deduplicating by lowercase username (and non-zero `user_id`) and immediately refreshing
    /// the filtered visible list.
    pub fn set_mention_candidates(&mut self, rows: Vec<MentionRow>) {
        let pairs: Vec<(MentionRow, Option<String>)> =
            rows.into_iter().map(|r| (r, None)).collect();
        self.set_mention_candidates_with_roles(pairs);
    }

    /// Populates the `@mention` candidate roster along with optional custom role hints
    /// (e.g. `"AI Bot"`, `"所有人"`, `"群主"`, `"管理员"`, `"成员"`).
    pub fn set_mention_candidates_with_roles(&mut self, rows: Vec<(MentionRow, Option<String>)>) {
        let mut seen_usernames: HashSet<String> = HashSet::new();
        let mut seen_ids: HashSet<u64> = HashSet::new();
        let mut deduped = Vec::with_capacity(rows.len());
        self.mention_role_hints.clear();

        for (mut row, role_hint) in rows {
            let clean_un = row.username.trim().trim_start_matches('@').to_string();
            if clean_un.is_empty() && row.name.trim().is_empty() {
                continue;
            }
            let key = clean_un.to_ascii_lowercase();
            if !key.is_empty() && !seen_usernames.insert(key.clone()) {
                continue;
            }
            if row.user_id > 0 && !seen_ids.insert(row.user_id) {
                continue;
            }
            row.username = clean_un;
            if let Some(hint) = role_hint.filter(|h| !h.trim().is_empty()) {
                if !key.is_empty() {
                    self.mention_role_hints.insert(key, hint);
                }
            }
            deduped.push(row);
        }

        self.all_mention_candidates = deduped;
        self.apply_mention_filter();
    }

    /// Resolves the role badge label for `row` (`"所有人"`, `"AI Bot"`, `"成员"`, or custom hint).
    pub fn role_hint_for(&self, row: &MentionRow) -> String {
        let key = row.username.trim().trim_start_matches('@').to_ascii_lowercase();
        if let Some(custom) = self.mention_role_hints.get(&key) {
            return custom.clone();
        }
        row.default_role_hint().to_string()
    }

    fn apply_mention_filter(&mut self) {
        if self.all_mention_candidates.is_empty() {
            return;
        }
        let q = self.filter.trim().to_ascii_lowercase();
        if q.is_empty() {
            self.mention_rows = self.all_mention_candidates.clone();
            return;
        }
        let mut prefix_matches = Vec::new();
        let mut substr_matches = Vec::new();
        for row in &self.all_mention_candidates {
            let uname_lower = row.username.to_ascii_lowercase();
            let name_lower = row.name.to_ascii_lowercase();
            let role_lower = self.role_hint_for(row).to_ascii_lowercase();
            if uname_lower.starts_with(&q) || name_lower.starts_with(&q) {
                prefix_matches.push(row.clone());
            } else if uname_lower.contains(&q)
                || name_lower.contains(&q)
                || role_lower.contains(&q)
            {
                substr_matches.push(row.clone());
            }
        }
        prefix_matches.extend(substr_matches);
        self.mention_rows = prefix_matches;
    }

    pub fn row_count(&self) -> usize {
        match self.autocomplete_type {
            AutocompleteType::Mentions => self.mention_rows.len(),
            AutocompleteType::Hashtags => self.hashtag_rows.len(),
            AutocompleteType::BotCommands => self.bot_command_rows.len(),
            AutocompleteType::Stickers => self.sticker_rows.len(),
        }
    }

    /// Process input query from composer field. Directly maps `field_autocomplete.cpp:L404-L417`.
    pub fn show_filtered(&mut self, query: &str) {
        if query.is_empty() {
            self.hide();
            return;
        }

        let first = query.chars().next().unwrap_or('\0');
        let (kind, plain) = match first {
            '@' => (AutocompleteType::Mentions, &query[1..]),
            '#' => (AutocompleteType::Hashtags, &query[1..]),
            '/' => (AutocompleteType::BotCommands, &query[1..]),
            _ => {
                self.hide();
                return;
            }
        };

        self.autocomplete_type = kind;
        self.filter = plain.to_lowercase();
        if kind == AutocompleteType::Mentions {
            self.apply_mention_filter();
        }
        self.is_open = true;
        self.selected_index = if self.row_count() > 0 { 0 } else { -1 };
    }

    pub fn hide(&mut self) {
        self.is_open = false;
        self.filter.clear();
        self.selected_index = -1;
    }

    /// Keyboard selection navigation math from `field_autocomplete.cpp:L1530-L1562`.
    pub fn move_selection(&mut self, key: &str) -> bool {
        let max_sel = self.row_count() as i32;
        if max_sel == 0 {
            return false;
        }

        let mut direction = match key {
            "Up" | "ArrowUp" => -1,
            "Down" | "ArrowDown" => 1,
            _ => 0,
        };

        if self.autocomplete_type == AutocompleteType::Stickers {
            if key == "Left" || key == "ArrowLeft" {
                direction = -1;
            } else if key == "Right" || key == "ArrowRight" {
                direction = 1;
            } else {
                direction *= self.stickers_per_row as i32;
            }
        }

        if direction == 0 {
            return false;
        }

        if self.selected_index < 0 || self.selected_index >= max_sel {
            if direction < -1 {
                let spr = self.stickers_per_row as i32;
                self.selected_index = ((max_sel - 1) / spr) * spr;
            } else if direction < 0 {
                self.selected_index = max_sel - 1;
            } else {
                self.selected_index = 0;
            }
            return self.selected_index >= 0 && self.selected_index < max_sel;
        }

        let next = self.selected_index + direction;
        if next < 0 || next >= max_sel {
            // Wrap or unselect
            self.selected_index = if next < 0 { max_sel - 1 } else { 0 };
        } else {
            self.selected_index = next;
        }

        true
    }

    /// Select chosen element at current index
    pub fn choose_selected(&self, _method: AutocompleteChooseMethod) -> Option<String> {
        if self.selected_index < 0 || self.selected_index as usize >= self.row_count() {
            return None;
        }

        let idx = self.selected_index as usize;
        match self.autocomplete_type {
            AutocompleteType::Mentions => {
                let row = &self.mention_rows[idx];
                Some(if !row.username.is_empty() {
                    format!("@{}", row.username)
                } else {
                    row.name.clone()
                })
            }
            AutocompleteType::Hashtags => {
                Some(format!("#{}", self.hashtag_rows[idx].tag))
            }
            AutocompleteType::BotCommands => {
                Some(format!("/{}", self.bot_command_rows[idx].command))
            }
            AutocompleteType::Stickers => {
                Some(format!("sticker:{}", self.sticker_rows[idx].document_id))
            }
        }
    }
}
