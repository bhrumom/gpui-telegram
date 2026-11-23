//! Interactive Multi-Tab Remote Terminal View with ANSI/VT100 state machine,
//! 16/256/TrueColor SGR cell grid, scrollback, OSC 52 clipboard writeback,
//! mouse wheel & selection copy, and full keyboard/IME/special-key encoding.

use std::collections::VecDeque;

use gpui::prelude::FluentBuilder;
use gpui::*;

use crate::i18n::tr_str;
use crate::theme::assets::{IconName, tg_icon};
use crate::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};

const DEFAULT_COLS: u16 = 100;
const DEFAULT_ROWS: u16 = 28;
const MAX_SCROLLBACK_LINES: usize = 5000;
const MAX_OSC_BYTES: usize = 128 * 1024;
const DEFAULT_FG: u32 = 0xE2E8F0;
const DEFAULT_BG: u32 = 0x0B1120;

const ANSI_16: [u32; 16] = [
    0x1E293B, 0xEF4444, 0x22C55E, 0xEAB308, 0x3B82F6, 0xA855F7, 0x06B6D4, 0xCBD5E1,
    0x64748B, 0xF87171, 0x4ADE80, 0xFACC15, 0x60A5FA, 0xC084FC, 0x22D3EE, 0xF8FAFC,
];

fn xterm_256_color(idx: u8) -> u32 {
    if (idx as usize) < 16 {
        return ANSI_16[idx as usize];
    }
    if idx >= 232 {
        let gray = 8u32 + ((idx - 232) as u32) * 10;
        return (gray << 16) | (gray << 8) | gray;
    }
    let n = idx - 16;
    let r_idx = n / 36;
    let g_idx = (n / 6) % 6;
    let b_idx = n % 6;
    let level = |v: u8| -> u32 {
        if v == 0 {
            0
        } else {
            55 + (v as u32) * 40
        }
    };
    (level(r_idx) << 16) | (level(g_idx) << 8) | level(b_idx)
}

fn decode_base64_str(input: &str) -> Option<String> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }
    let cleaned: Vec<u8> = input
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    if cleaned.is_empty() || cleaned.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity((cleaned.len() * 3) / 4);
    let mut i = 0;
    while i < cleaned.len() {
        let a = val(cleaned[i])?;
        let b = val(*cleaned.get(i + 1)?)?;
        let c_byte = *cleaned.get(i + 2).unwrap_or(&b'=');
        let d_byte = *cleaned.get(i + 3).unwrap_or(&b'=');
        out.push((a << 2) | (b >> 4));
        if c_byte != b'=' {
            let c = val(c_byte)?;
            out.push((b << 4) | (c >> 2));
            if d_byte != b'=' {
                let d = val(d_byte)?;
                out.push((c << 6) | d);
            }
        }
        i += 4;
    }
    String::from_utf8(out).ok()
}

/// Single character cell inside the terminal grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCell {
    pub ch: char,
    pub fg: u32,
    pub bg: u32,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl Default for TerminalCell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellStyle {
    fg: u32,
    bg: u32,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl Default for CellStyle {
    fn default() -> Self {
        Self {
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }
}

impl CellStyle {
    fn to_cell(self, ch: char) -> TerminalCell {
        TerminalCell {
            ch,
            fg: self.fg,
            bg: self.bg,
            bold: self.bold,
            dim: self.dim,
            italic: self.italic,
            underline: self.underline,
            inverse: self.inverse,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParserState {
    Ground,
    Escape,
    Csi,
    Osc,
    OscEsc,
    Charset,
}

/// ANSI/VT100 terminal screen buffer and escape-sequence state machine.
#[derive(Debug, Clone)]
pub struct TerminalScreen {
    pub cols: u16,
    pub rows: u16,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    pub scroll_offset: usize,
    pub title: Option<String>,
    pub pending_osc52_clipboard: Vec<String>,
    grid: Vec<Vec<TerminalCell>>,
    alt_grid: Option<Vec<Vec<TerminalCell>>>,
    scrollback: VecDeque<Vec<TerminalCell>>,
    saved_cursor: (u16, u16),
    scroll_top: u16,
    scroll_bottom: u16,
    style: CellStyle,
    state: ParserState,
    csi_buf: String,
    osc_buf: String,
}

impl TerminalScreen {
    pub fn new(cols: u16, rows: u16) -> Self {
        let cols = cols.max(20);
        let rows = rows.max(6);
        let blank_row = vec![TerminalCell::default(); cols as usize];
        Self {
            cols,
            rows,
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            scroll_offset: 0,
            title: None,
            pending_osc52_clipboard: Vec::new(),
            grid: vec![blank_row; rows as usize],
            alt_grid: None,
            scrollback: VecDeque::new(),
            saved_cursor: (0, 0),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            style: CellStyle::default(),
            state: ParserState::Ground,
            csi_buf: String::new(),
            osc_buf: String::new(),
        }
    }

    pub fn resize(&mut self, new_cols: u16, new_rows: u16) {
        let cols = new_cols.max(20);
        let rows = new_rows.max(6);
        for row in &mut self.grid {
            row.resize(cols as usize, TerminalCell::default());
        }
        while self.grid.len() < rows as usize {
            self.grid
                .push(vec![TerminalCell::default(); cols as usize]);
        }
        if self.grid.len() > rows as usize {
            let extra = self.grid.len() - rows as usize;
            for _ in 0..extra {
                let top = self.grid.remove(0);
                self.push_scrollback(top);
            }
        }
        self.cols = cols;
        self.rows = rows;
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    pub fn scroll_lines(&mut self, delta_up: i32) {
        if delta_up > 0 {
            self.scroll_offset = (self.scroll_offset + delta_up as usize).min(self.scrollback.len());
        } else if delta_up < 0 {
            self.scroll_offset = self.scroll_offset.saturating_sub((-delta_up) as usize);
        }
    }

    /// Returns visible row slice at visual display row `vis_row` (0 = top of viewport),
    /// accounting for `scroll_offset` into scrollback history.
    pub fn visible_row(&self, vis_row: usize) -> Option<&[TerminalCell]> {
        if vis_row >= self.rows as usize {
            return None;
        }
        if self.scroll_offset == 0 {
            return self.grid.get(vis_row).map(Vec::as_slice);
        }
        let sb_len = self.scrollback.len();
        let start_in_combined = sb_len.saturating_sub(self.scroll_offset);
        let idx = start_in_combined + vis_row;
        if idx < sb_len {
            self.scrollback.get(idx).map(Vec::as_slice)
        } else {
            self.grid.get(idx - sb_len).map(Vec::as_slice)
        }
    }

    /// Returns plain text of `vis_row` with trailing spaces trimmed.
    pub fn visible_row_text(&self, vis_row: usize) -> String {
        let Some(row) = self.visible_row(vis_row) else {
            return String::new();
        };
        let raw: String = row.iter().map(|c| c.ch).collect();
        raw.trim_end().to_string()
    }

    pub fn extract_text_range(&self, start: (usize, usize), end: (usize, usize)) -> String {
        let ((r0, c0), (r1, c1)) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let mut lines = Vec::new();
        for r in r0..=r1.min((self.rows as usize).saturating_sub(1)) {
            let Some(row) = self.visible_row(r) else {
                continue;
            };
            let from_c = if r == r0 { c0.min(row.len()) } else { 0 };
            let to_c = if r == r1 {
                (c1 + 1).min(row.len())
            } else {
                row.len()
            };
            if from_c <= to_c {
                let s: String = row[from_c..to_c].iter().map(|cell| cell.ch).collect();
                lines.push(s.trim_end().to_string());
            }
        }
        lines.join("\n")
    }

    /// Feeds raw VT100/ANSI output into the terminal state machine.
    pub fn feed_str(&mut self, data: &str) {
        for ch in data.chars() {
            self.feed_char(ch);
        }
    }

    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        self.feed_str(&text);
    }

    fn push_scrollback(&mut self, row: Vec<TerminalCell>) {
        if self.alt_grid.is_some() {
            return;
        }
        self.scrollback.push_back(row);
        while self.scrollback.len() > MAX_SCROLLBACK_LINES {
            self.scrollback.pop_front();
        }
    }

    fn scroll_region_up(&mut self, lines: u16) {
        let top = self.scroll_top as usize;
        let bottom = (self.scroll_bottom as usize).min(self.grid.len().saturating_sub(1));
        if top > bottom {
            return;
        }
        let count = (lines as usize).min(bottom - top + 1);
        for _ in 0..count {
            let removed = self.grid.remove(top);
            if top == 0 {
                self.push_scrollback(removed);
            }
            self.grid
                .insert(bottom, vec![TerminalCell::default(); self.cols as usize]);
        }
    }

    fn scroll_region_down(&mut self, lines: u16) {
        let top = self.scroll_top as usize;
        let bottom = (self.scroll_bottom as usize).min(self.grid.len().saturating_sub(1));
        if top > bottom {
            return;
        }
        let count = (lines as usize).min(bottom - top + 1);
        for _ in 0..count {
            self.grid.remove(bottom);
            self.grid
                .insert(top, vec![TerminalCell::default(); self.cols as usize]);
        }
    }

    fn line_feed(&mut self) {
        if self.cursor_row >= self.scroll_bottom {
            self.scroll_region_up(1);
        } else if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        }
    }

    fn put_char(&mut self, ch: char) {
        if self.cursor_col >= self.cols {
            self.cursor_col = 0;
            self.line_feed();
        }
        let r = self.cursor_row as usize;
        let c = self.cursor_col as usize;
        if let Some(cell) = self.grid.get_mut(r).and_then(|row| row.get_mut(c)) {
            *cell = self.style.to_cell(ch);
        }
        self.cursor_col += 1;
    }

    fn feed_char(&mut self, ch: char) {
        match self.state {
            ParserState::Ground => match ch {
                '\x1b' => self.state = ParserState::Escape,
                '\r' => self.cursor_col = 0,
                '\n' => self.line_feed(),
                '\x08' => self.cursor_col = self.cursor_col.saturating_sub(1),
                '\t' => {
                    let next = ((self.cursor_col / 8) + 1) * 8;
                    self.cursor_col = next.min(self.cols.saturating_sub(1));
                }
                '\x07' => {}
                c if c.is_control() => {}
                c => self.put_char(c),
            },
            ParserState::Escape => match ch {
                '[' => {
                    self.csi_buf.clear();
                    self.state = ParserState::Csi;
                }
                ']' => {
                    self.osc_buf.clear();
                    self.state = ParserState::Osc;
                }
                '(' | ')' | '*' | '+' => self.state = ParserState::Charset,
                '7' => {
                    self.saved_cursor = (self.cursor_row, self.cursor_col);
                    self.state = ParserState::Ground;
                }
                '8' => {
                    self.cursor_row = self.saved_cursor.0.min(self.rows - 1);
                    self.cursor_col = self.saved_cursor.1.min(self.cols - 1);
                    self.state = ParserState::Ground;
                }
                'M' => {
                    if self.cursor_row <= self.scroll_top {
                        self.scroll_region_down(1);
                    } else {
                        self.cursor_row = self.cursor_row.saturating_sub(1);
                    }
                    self.state = ParserState::Ground;
                }
                'c' => {
                    *self = Self::new(self.cols, self.rows);
                }
                _ => self.state = ParserState::Ground,
            },
            ParserState::Charset => {
                self.state = ParserState::Ground;
            }
            ParserState::Csi => {
                if ('@'..='~').contains(&ch) {
                    let buf = std::mem::take(&mut self.csi_buf);
                    self.state = ParserState::Ground;
                    self.handle_csi(&buf, ch);
                } else if self.csi_buf.len() < 128 {
                    self.csi_buf.push(ch);
                }
            }
            ParserState::Osc => match ch {
                '\x07' => {
                    let osc = std::mem::take(&mut self.osc_buf);
                    self.state = ParserState::Ground;
                    self.handle_osc(&osc);
                }
                '\x1b' => self.state = ParserState::OscEsc,
                c => {
                    if self.osc_buf.len() < MAX_OSC_BYTES {
                        self.osc_buf.push(c);
                    }
                }
            },
            ParserState::OscEsc => {
                if ch == '\\' {
                    let osc = std::mem::take(&mut self.osc_buf);
                    self.state = ParserState::Ground;
                    self.handle_osc(&osc);
                } else {
                    self.state = ParserState::Ground;
                }
            }
        }
    }

    fn handle_osc(&mut self, osc: &str) {
        if let Some((cmd, rest)) = osc.split_once(';') {
            match cmd.trim() {
                "0" | "2" => {
                    if !rest.is_empty() {
                        self.title = Some(rest.to_string());
                    }
                }
                "52" => {
                    // Format: 52;<selection>;<base64>
                    if let Some((_, b64)) = rest.split_once(';') {
                        if b64 != "?" {
                            if let Some(decoded) = decode_base64_str(b64.trim()) {
                                self.pending_osc52_clipboard.push(decoded);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn handle_csi(&mut self, raw_params: &str, final_ch: char) {
        let is_private = raw_params.starts_with('?');
        let clean = raw_params.trim_start_matches('?');
        let params: Vec<u16> = if clean.is_empty() {
            Vec::new()
        } else {
            clean
                .split(';')
                .map(|p| p.split(':').next().unwrap_or("").parse::<u16>().unwrap_or(0))
                .collect()
        };
        let p0 = params.first().copied().unwrap_or(0);
        let p1 = params.get(1).copied().unwrap_or(0);

        if is_private {
            match final_ch {
                'h' => {
                    if params.contains(&25) {
                        self.cursor_visible = true;
                    }
                    if params.contains(&1049) && self.alt_grid.is_none() {
                        let blank = vec![vec![TerminalCell::default(); self.cols as usize]; self.rows as usize];
                        let prev = std::mem::replace(&mut self.grid, blank);
                        self.alt_grid = Some(prev);
                        self.cursor_row = 0;
                        self.cursor_col = 0;
                    }
                }
                'l' => {
                    if params.contains(&25) {
                        self.cursor_visible = false;
                    }
                    if params.contains(&1049) {
                        if let Some(main) = self.alt_grid.take() {
                            self.grid = main;
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        match final_ch {
            'm' => self.apply_sgr(clean),
            'A' => self.cursor_row = self.cursor_row.saturating_sub(p0.max(1)),
            'B' => self.cursor_row = (self.cursor_row + p0.max(1)).min(self.rows - 1),
            'C' => self.cursor_col = (self.cursor_col + p0.max(1)).min(self.cols - 1),
            'D' => self.cursor_col = self.cursor_col.saturating_sub(p0.max(1)),
            'E' => {
                self.cursor_row = (self.cursor_row + p0.max(1)).min(self.rows - 1);
                self.cursor_col = 0;
            }
            'F' => {
                self.cursor_row = self.cursor_row.saturating_sub(p0.max(1));
                self.cursor_col = 0;
            }
            'G' => self.cursor_col = p0.max(1).saturating_sub(1).min(self.cols - 1),
            'd' => self.cursor_row = p0.max(1).saturating_sub(1).min(self.rows - 1),
            'H' | 'f' => {
                let row = p0.max(1).saturating_sub(1).min(self.rows - 1);
                let col = p1.max(1).saturating_sub(1).min(self.cols - 1);
                self.cursor_row = row;
                self.cursor_col = col;
            }
            'J' => {
                let r = self.cursor_row as usize;
                let c = self.cursor_col as usize;
                match p0 {
                    0 => {
                        if let Some(row) = self.grid.get_mut(r) {
                            for cell in row.iter_mut().skip(c) {
                                *cell = TerminalCell::default();
                            }
                        }
                        for row in self.grid.iter_mut().skip(r + 1) {
                            row.fill(TerminalCell::default());
                        }
                    }
                    1 => {
                        for row in self.grid.iter_mut().take(r) {
                            row.fill(TerminalCell::default());
                        }
                        if let Some(row) = self.grid.get_mut(r) {
                            for cell in row.iter_mut().take(c + 1) {
                                *cell = TerminalCell::default();
                            }
                        }
                    }
                    2 | 3 => {
                        for row in &mut self.grid {
                            row.fill(TerminalCell::default());
                        }
                        if p0 == 3 {
                            self.scrollback.clear();
                            self.scroll_offset = 0;
                        }
                    }
                    _ => {}
                }
            }
            'K' => {
                let r = self.cursor_row as usize;
                let c = self.cursor_col as usize;
                if let Some(row) = self.grid.get_mut(r) {
                    match p0 {
                        0 => {
                            for cell in row.iter_mut().skip(c) {
                                *cell = TerminalCell::default();
                            }
                        }
                        1 => {
                            for cell in row.iter_mut().take(c + 1) {
                                *cell = TerminalCell::default();
                            }
                        }
                        2 => row.fill(TerminalCell::default()),
                        _ => {}
                    }
                }
            }
            'L' => {
                let count = p0.max(1) as usize;
                let cur = self.cursor_row as usize;
                let bottom = self.scroll_bottom as usize;
                if cur <= bottom {
                    for _ in 0..count.min(bottom - cur + 1) {
                        self.grid.remove(bottom);
                        self.grid
                            .insert(cur, vec![TerminalCell::default(); self.cols as usize]);
                    }
                }
            }
            'M' => {
                let count = p0.max(1) as usize;
                let cur = self.cursor_row as usize;
                let bottom = self.scroll_bottom as usize;
                if cur <= bottom {
                    for _ in 0..count.min(bottom - cur + 1) {
                        self.grid.remove(cur);
                        self.grid
                            .insert(bottom, vec![TerminalCell::default(); self.cols as usize]);
                    }
                }
            }
            'P' => {
                let count = p0.max(1) as usize;
                let r = self.cursor_row as usize;
                let c = self.cursor_col as usize;
                if let Some(row) = self.grid.get_mut(r) {
                    for _ in 0..count.min(row.len().saturating_sub(c)) {
                        row.remove(c);
                        row.push(TerminalCell::default());
                    }
                }
            }
            '@' => {
                let count = p0.max(1) as usize;
                let r = self.cursor_row as usize;
                let c = self.cursor_col as usize;
                if let Some(row) = self.grid.get_mut(r) {
                    for _ in 0..count.min(row.len().saturating_sub(c)) {
                        row.pop();
                        row.insert(c, TerminalCell::default());
                    }
                }
            }
            'S' => self.scroll_region_up(p0.max(1)),
            'T' => self.scroll_region_down(p0.max(1)),
            'r' => {
                let top = p0.max(1).saturating_sub(1).min(self.rows - 1);
                let bottom = if p1 == 0 {
                    self.rows - 1
                } else {
                    p1.saturating_sub(1).min(self.rows - 1)
                };
                if top < bottom {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                }
            }
            's' => self.saved_cursor = (self.cursor_row, self.cursor_col),
            'u' => {
                self.cursor_row = self.saved_cursor.0.min(self.rows - 1);
                self.cursor_col = self.saved_cursor.1.min(self.cols - 1);
            }
            _ => {}
        }
    }

    fn apply_sgr(&mut self, clean: &str) {
        if clean.is_empty() {
            self.style = CellStyle::default();
            return;
        }
        // Support colon sub-parameters like `38:2::255:128:0` by normalizing `:` -> `;`
        let tokens: Vec<u16> = clean
            .replace(':', ";")
            .split(';')
            .filter(|s| !s.is_empty() || clean.contains("::"))
            .map(|s| s.parse::<u16>().unwrap_or(0))
            .collect();
        if tokens.is_empty() {
            self.style = CellStyle::default();
            return;
        }
        let mut i = 0;
        while i < tokens.len() {
            match tokens[i] {
                0 => self.style = CellStyle::default(),
                1 => self.style.bold = true,
                2 => self.style.dim = true,
                3 => self.style.italic = true,
                4 => self.style.underline = true,
                7 => self.style.inverse = true,
                22 => {
                    self.style.bold = false;
                    self.style.dim = false;
                }
                23 => self.style.italic = false,
                24 => self.style.underline = false,
                27 => self.style.inverse = false,
                30..=37 => self.style.fg = ANSI_16[(tokens[i] - 30) as usize],
                38 => {
                    if tokens.get(i + 1) == Some(&5) {
                        if let Some(&n) = tokens.get(i + 2) {
                            self.style.fg = xterm_256_color(n as u8);
                            i += 2;
                        }
                    } else if tokens.get(i + 1) == Some(&2) {
                        // Handle both `38;2;R;G;B` and `38:2::R:G:B`
                        let offset = if tokens.len() > i + 5 && tokens[i + 2] == 0 && clean.contains("::") {
                            1
                        } else {
                            0
                        };
                        if let (Some(&r), Some(&g), Some(&b)) = (
                            tokens.get(i + 2 + offset),
                            tokens.get(i + 3 + offset),
                            tokens.get(i + 4 + offset),
                        ) {
                            self.style.fg = ((r as u32 & 0xFF) << 16)
                                | ((g as u32 & 0xFF) << 8)
                                | (b as u32 & 0xFF);
                            i += 4 + offset;
                        }
                    }
                }
                39 => self.style.fg = DEFAULT_FG,
                40..=47 => self.style.bg = ANSI_16[(tokens[i] - 40) as usize],
                48 => {
                    if tokens.get(i + 1) == Some(&5) {
                        if let Some(&n) = tokens.get(i + 2) {
                            self.style.bg = xterm_256_color(n as u8);
                            i += 2;
                        }
                    } else if tokens.get(i + 1) == Some(&2) {
                        let offset = if tokens.len() > i + 5 && tokens[i + 2] == 0 && clean.contains("::") {
                            1
                        } else {
                            0
                        };
                        if let (Some(&r), Some(&g), Some(&b)) = (
                            tokens.get(i + 2 + offset),
                            tokens.get(i + 3 + offset),
                            tokens.get(i + 4 + offset),
                        ) {
                            self.style.bg = ((r as u32 & 0xFF) << 16)
                                | ((g as u32 & 0xFF) << 8)
                                | (b as u32 & 0xFF);
                            i += 4 + offset;
                        }
                    }
                }
                49 => self.style.bg = DEFAULT_BG,
                90..=97 => self.style.fg = ANSI_16[(tokens[i] - 90 + 8) as usize],
                100..=107 => self.style.bg = ANSI_16[(tokens[i] - 100 + 8) as usize],
                _ => {}
            }
            i += 1;
        }
    }
}

/// Encodes GPUI keystrokes + modifiers into standard VT100/xterm byte sequences.
pub fn encode_terminal_key(
    key: &str,
    shift: bool,
    ctrl: bool,
    alt: bool,
    key_char: Option<&str>,
) -> Option<Vec<u8>> {
    let lower = key.to_ascii_lowercase();
    if ctrl && !alt {
        if lower.len() == 1 {
            let b = lower.as_bytes()[0];
            if b.is_ascii_lowercase() {
                return Some(vec![b - b'a' + 1]);
            }
        }
        match lower.as_str() {
            "]" => return Some(vec![0x1d]),
            "[" | "escape" => return Some(vec![0x1b]),
            "\\" => return Some(vec![0x1c]),
            "space" | "@" => return Some(vec![0x00]),
            _ => {}
        }
    }

    let mod_param = 1u8
        + if shift { 1 } else { 0 }
        + if alt { 2 } else { 0 }
        + if ctrl { 4 } else { 0 };

    let csi_arrow = |letter: char| -> Vec<u8> {
        if mod_param == 1 {
            format!("\x1b[{letter}").into_bytes()
        } else {
            format!("\x1b[1;{mod_param}{letter}").into_bytes()
        }
    };

    let csi_tilde = |code: u8| -> Vec<u8> {
        if mod_param == 1 {
            format!("\x1b[{code}~").into_bytes()
        } else {
            format!("\x1b[{code};{mod_param}~").into_bytes()
        }
    };

    match lower.as_str() {
        "enter" | "return" => return Some(b"\r".to_vec()),
        "backspace" => {
            return Some(if alt {
                b"\x1b\x7f".to_vec()
            } else {
                b"\x7f".to_vec()
            });
        }
        "tab" => {
            return Some(if shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            });
        }
        "escape" | "esc" => return Some(b"\x1b".to_vec()),
        "up" => return Some(csi_arrow('A')),
        "down" => return Some(csi_arrow('B')),
        "right" => return Some(csi_arrow('C')),
        "left" => return Some(csi_arrow('D')),
        "home" => return Some(csi_arrow('H')),
        "end" => return Some(csi_arrow('F')),
        "insert" => return Some(csi_tilde(2)),
        "delete" => return Some(csi_tilde(3)),
        "pageup" => return Some(csi_tilde(5)),
        "pagedown" => return Some(csi_tilde(6)),
        "f1" => return Some(b"\x1bOP".to_vec()),
        "f2" => return Some(b"\x1bOQ".to_vec()),
        "f3" => return Some(b"\x1bOR".to_vec()),
        "f4" => return Some(b"\x1bOS".to_vec()),
        "f5" => return Some(csi_tilde(15)),
        "f6" => return Some(csi_tilde(17)),
        "f7" => return Some(csi_tilde(18)),
        "f8" => return Some(csi_tilde(19)),
        "f9" => return Some(csi_tilde(20)),
        "f10" => return Some(csi_tilde(21)),
        "f11" => return Some(csi_tilde(23)),
        "f12" => return Some(csi_tilde(24)),
        "space" => {
            return Some(if alt {
                b"\x1b ".to_vec()
            } else {
                b" ".to_vec()
            });
        }
        _ => {}
    }

    if let Some(ch) = key_char.filter(|s| !s.is_empty()) {
        let mut out = Vec::new();
        if alt {
            out.push(0x1b);
        }
        out.extend_from_slice(ch.as_bytes());
        return Some(out);
    }

    if key.chars().count() == 1 {
        let mut out = Vec::new();
        if alt {
            out.push(0x1b);
        }
        out.extend_from_slice(key.as_bytes());
        return Some(out);
    }

    None
}

/// Actions emitted by `RemoteTerminalView` for `ComputerWorkbench` / `AppShell` to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteTerminalAction {
    NewSession {
        name: String,
        shell: Option<String>,
        cols: u16,
        rows: u16,
    },
    RenameSession {
        session_id: String,
        new_name: String,
    },
    KillSession {
        session_id: String,
    },
    SendInput {
        session_id: String,
        data: String,
    },
    Resize {
        session_id: String,
        cols: u16,
        rows: u16,
    },
    RefreshSession {
        session_id: String,
    },
    CopyToClipboard(String),
}

/// State for a single persistent terminal tab.
#[derive(Debug, Clone)]
pub struct TerminalTabSession {
    pub session_id: String,
    pub name: String,
    pub shell: String,
    pub alive: bool,
    pub exit_code: Option<i32>,
    pub keep_alive_on_exit: bool,
    pub cwd: String,
    pub screen: TerminalScreen,
}

impl TerminalTabSession {
    pub fn new(session_id: impl Into<String>, name: impl Into<String>, shell: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            name: name.into(),
            shell: shell.into(),
            alive: true,
            exit_code: None,
            keep_alive_on_exit: true,
            cwd: "~".to_string(),
            screen: TerminalScreen::new(DEFAULT_COLS, DEFAULT_ROWS),
        }
    }
}

/// Interactive multi-tab terminal widget ready to embed inside `ComputerWorkbench`.
pub struct RemoteTerminalView {
    pub tabs: Vec<TerminalTabSession>,
    pub active_tab_index: usize,
    pub is_focused: bool,
    pub ime_preedit: Option<String>,
    pub renaming_tab_index: Option<usize>,
    pub rename_draft: String,
    pub selection: Option<((usize, usize), (usize, usize))>,
    pub dragging_selection: bool,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    pub pending_actions: VecDeque<RemoteTerminalAction>,
    next_local_seq: u64,
}

impl RemoteTerminalView {
    pub fn new(palette: TdesktopThemePalette, lang: &str) -> Self {
        let mut init_tab = TerminalTabSession::new("term-1", "main", "zsh");
        init_tab.screen.feed_str(if lang.starts_with("zh") {
            "\x1b[36mFabushi 远程持久化终端就绪\x1b[0m — 点击此区域直接输入按键，或点击「+ 新建会话」。\r\n"
        } else {
            "\x1b[36mFabushi Persistent Remote Terminal Ready\x1b[0m — Focus and type, or click '+ New Session'.\r\n"
        });
        Self {
            tabs: vec![init_tab],
            active_tab_index: 0,
            is_focused: false,
            ime_preedit: None,
            renaming_tab_index: None,
            rename_draft: String::new(),
            selection: None,
            dragging_selection: false,
            palette,
            language_id: lang.to_string(),
            pending_actions: VecDeque::new(),
            next_local_seq: 2,
        }
    }

    pub fn active_tab(&self) -> Option<&TerminalTabSession> {
        self.tabs.get(self.active_tab_index)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut TerminalTabSession> {
        self.tabs.get_mut(self.active_tab_index)
    }

    pub fn take_actions(&mut self) -> Vec<RemoteTerminalAction> {
        self.pending_actions.drain(..).collect()
    }

    pub fn open_new_tab(&mut self, custom_name: Option<String>) -> String {
        let seq = self.next_local_seq;
        self.next_local_seq += 1;
        let name = custom_name
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("sess-{seq}"));
        let session_id = format!("term-{seq}");
        let tab = TerminalTabSession::new(session_id.clone(), name.clone(), "zsh");
        self.tabs.push(tab);
        self.active_tab_index = self.tabs.len() - 1;
        self.selection = None;
        self.pending_actions
            .push_back(RemoteTerminalAction::NewSession {
                name,
                shell: Some("zsh".to_string()),
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
            });
        session_id
    }

    pub fn begin_rename_tab(&mut self, idx: usize) {
        if let Some(tab) = self.tabs.get(idx) {
            self.renaming_tab_index = Some(idx);
            self.rename_draft = tab.name.clone();
        }
    }

    pub fn commit_rename(&mut self) -> bool {
        let Some(idx) = self.renaming_tab_index.take() else {
            return false;
        };
        let new_name = self.rename_draft.trim().to_string();
        self.rename_draft.clear();
        if new_name.is_empty() {
            return false;
        }
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.name = new_name.clone();
            self.pending_actions
                .push_back(RemoteTerminalAction::RenameSession {
                    session_id: tab.session_id.clone(),
                    new_name,
                });
            return true;
        }
        false
    }

    pub fn close_or_kill_tab(&mut self, idx: usize, force_kill: bool) {
        let Some(tab) = self.tabs.get(idx).cloned() else {
            return;
        };
        if force_kill || !tab.keep_alive_on_exit || !tab.alive {
            self.pending_actions
                .push_back(RemoteTerminalAction::KillSession {
                    session_id: tab.session_id,
                });
            if self.tabs.len() > 1 {
                self.tabs.remove(idx);
                if self.active_tab_index >= self.tabs.len() {
                    self.active_tab_index = self.tabs.len() - 1;
                }
            } else if let Some(only) = self.tabs.first_mut() {
                only.alive = false;
                only.exit_code = Some(0);
            }
        }
    }

    pub fn toggle_exit_keep_alive(&mut self, idx: usize) {
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.keep_alive_on_exit = !tab.keep_alive_on_exit;
        }
    }

    pub fn feed_session_output(&mut self, session_id: &str, bytes: &[u8]) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.session_id == session_id) {
            tab.screen.feed_bytes(bytes);
            if let Some(title) = tab.screen.title.take() {
                if !title.trim().is_empty() {
                    tab.name = title.chars().take(24).collect();
                }
            }
            for clip in tab.screen.pending_osc52_clipboard.drain(..) {
                self.pending_actions
                    .push_back(RemoteTerminalAction::CopyToClipboard(clip));
            }
        }
    }

    pub fn mark_session_exited(&mut self, session_id: &str, exit_code: i32) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.session_id == session_id) {
            tab.alive = false;
            tab.exit_code = Some(exit_code);
            tab.screen.feed_str(&format!(
                "\r\n\x1b[33m[Process exited with code {exit_code}]\x1b[0m\r\n"
            ));
            if !tab.keep_alive_on_exit {
                let idx_opt = self.tabs.iter().position(|t| t.session_id == session_id);
                if let Some(idx) = idx_opt {
                    self.close_or_kill_tab(idx, true);
                }
            }
        }
    }

    pub fn copy_selection_to_clipboard(&mut self) -> Option<String> {
        let (start, end) = self.selection?;
        let text = self.active_tab()?.screen.extract_text_range(start, end);
        if text.is_empty() {
            return None;
        }
        self.pending_actions
            .push_back(RemoteTerminalAction::CopyToClipboard(text.clone()));
        Some(text)
    }

    /// Handles IME / committed text input into either tab rename draft or active remote PTY.
    pub fn push_committed_text(&mut self, text: &str) -> bool {
        self.ime_preedit = None;
        if text.is_empty() {
            return false;
        }
        if self.renaming_tab_index.is_some() {
            self.rename_draft.push_str(text);
            return true;
        }
        if !self.is_focused {
            return false;
        }
        if let Some(tab) = self.active_tab() {
            self.pending_actions
                .push_back(RemoteTerminalAction::SendInput {
                    session_id: tab.session_id.clone(),
                    data: text.to_string(),
                });
            return true;
        }
        false
    }

    /// Handles GPUI keydown events when focused (tab rename or remote PTY encoding).
    pub fn handle_key_stroke(
        &mut self,
        key: &str,
        shift: bool,
        ctrl: bool,
        alt: bool,
        platform: bool,
        key_char: Option<&str>,
    ) -> bool {
        if self.renaming_tab_index.is_some() {
            match key {
                "enter" => {
                    self.commit_rename();
                }
                "escape" => {
                    self.renaming_tab_index = None;
                    self.rename_draft.clear();
                }
                "backspace" => {
                    self.rename_draft.pop();
                }
                _ => {
                    if let Some(ch) = key_char {
                        self.rename_draft.push_str(ch);
                    }
                }
            }
            return true;
        }
        if !self.is_focused {
            return false;
        }
        if platform && key.eq_ignore_ascii_case("c") && self.selection.is_some() {
            return self.copy_selection_to_clipboard().is_some();
        }
        if let Some(bytes) = encode_terminal_key(key, shift, ctrl, alt, key_char) {
            let data = String::from_utf8_lossy(&bytes).into_owned();
            let session_id = if let Some(tab) = self.active_tab_mut() {
                tab.screen.scroll_offset = 0;
                Some(tab.session_id.clone())
            } else {
                None
            };
            if let Some(session_id) = session_id {
                self.pending_actions
                    .push_back(RemoteTerminalAction::SendInput { session_id, data });
                return true;
            }
        }
        false
    }

    pub fn scroll_active_wheel(&mut self, delta_lines: i32) {
        if let Some(tab) = self.active_tab_mut() {
            tab.screen.scroll_lines(delta_lines);
        }
    }

    fn render_grid_rows(&self, tab: &TerminalTabSession, pal: &TdesktopThemePalette) -> AnyElement {
        let rows = tab.screen.rows as usize;
        let cols = tab.screen.cols as usize;
        let cur_r = tab.screen.cursor_row as usize;
        let cur_c = tab.screen.cursor_col as usize;
        let show_cursor = tab.screen.cursor_visible && tab.screen.scroll_offset == 0;
        let sel_norm = self.selection.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });

        let mut col_div = div()
            .w_full()
            .flex()
            .flex_col()
            .font_family("Menlo")
            .text_size(px(12.0))
            .line_height(px(16.0));

        for r in 0..rows {
            let Some(cells) = tab.screen.visible_row(r) else {
                continue;
            };
            let mut spans: Vec<(u32, u32, bool, bool, String)> = Vec::new();
            let mut last_non_space = cells.len();
            while last_non_space > 0 && cells[last_non_space - 1].ch == ' ' {
                let c_idx = last_non_space - 1;
                let is_cur = show_cursor && r == cur_r && c_idx == cur_c;
                let cell = cells[c_idx];
                if is_cur || cell.bg != DEFAULT_BG {
                    break;
                }
                last_non_space -= 1;
            }
            let render_len = if show_cursor && r == cur_r {
                last_non_space.max((cur_c + 1).min(cols))
            } else {
                last_non_space
            };

            if render_len == 0 {
                col_div = col_div.child(div().h(px(16.0)).child(" "));
                continue;
            }

            for (c, cell) in cells.iter().enumerate().take(render_len) {
                let is_cursor = show_cursor && r == cur_r && c == cur_c;
                let is_selected = sel_norm.is_some_and(|((sr, sc), (er, ec))| {
                    (r > sr || (r == sr && c >= sc)) && (r < er || (r == er && c <= ec))
                });
                let (mut fg, mut bg) = if cell.inverse {
                    (cell.bg, cell.fg)
                } else {
                    (cell.fg, cell.bg)
                };
                if is_selected {
                    bg = pal.accent;
                    fg = 0xFFFFFF;
                } else if is_cursor {
                    bg = if self.is_focused { 0x38BDF8 } else { 0x64748B };
                    fg = 0x0B1120;
                }
                if let Some(last) = spans.last_mut() {
                    if last.0 == fg
                        && last.1 == bg
                        && last.2 == cell.bold
                        && last.3 == cell.underline
                    {
                        last.4.push(cell.ch);
                        continue;
                    }
                }
                spans.push((fg, bg, cell.bold, cell.underline, cell.ch.to_string()));
            }

            let mut row_div = div().h(px(16.0)).flex().flex_row().items_center();
            for (fg, bg, bold, underline, text) in spans {
                let mut seg = div().text_color(rgb(fg)).child(text);
                if bg != DEFAULT_BG {
                    seg = seg.bg(rgb(bg));
                }
                if bold {
                    seg = seg.font_weight(FontWeight::BOLD);
                }
                if underline {
                    seg = seg.underline();
                }
                row_div = row_div.child(seg);
            }
            col_div = col_div.child(row_div);
        }

        col_div.into_any_element()
    }
}

impl Render for RemoteTerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let lang = self.language_id.clone();
        let is_zh = lang.starts_with("zh");
        let term_bg = match pal.mode {
            TdesktopThemeMode::Day => 0x0F172A,
            _ => DEFAULT_BG,
        };
        let active_idx = self.active_tab_index.min(self.tabs.len().saturating_sub(1));
        let active_tab = self.tabs.get(active_idx);

        let mut tab_bar = div()
            .px(px(10.0))
            .py(px(6.0))
            .bg(rgb(0x111827))
            .border_b_1()
            .border_color(rgb(0x1E293B))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.0));

        let mut tabs_strip = div().flex_1().flex().flex_row().items_center().gap(px(6.0));
        for (idx, tab) in self.tabs.iter().enumerate() {
            let is_active = idx == active_idx;
            let is_renaming = self.renaming_tab_index == Some(idx);
            tabs_strip = tabs_strip.child(
                div()
                    .id(("rtv-tab", idx))
                    .cursor_pointer()
                    .px(px(9.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .bg(rgb(if is_active { 0x1E293B } else { 0x0F172A }))
                    .border_1()
                    .border_color(rgb(if is_active { pal.accent } else { 0x334155 }))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.active_tab_index = idx;
                            this.is_focused = true;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(rgb(if tab.alive { 0x22C55E } else { 0xF59E0B })),
                    )
                    .child(if is_renaming {
                        div()
                            .text_xs()
                            .text_color(rgb(0x38BDF8))
                            .child(format!("{}|", self.rename_draft))
                    } else {
                        div()
                            .text_xs()
                            .font_weight(if is_active {
                                FontWeight::BOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .text_color(rgb(0xF8FAFC))
                            .child(tab.name.clone())
                    })
                    .child(
                        div()
                            .id(("rtv-tab-rename", idx))
                            .px(px(3.0))
                            .text_size(px(10.0))
                            .text_color(rgb(0x94A3B8))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.begin_rename_tab(idx);
                                    cx.notify();
                                }),
                            )
                            .child("✎"),
                    )
                    .child(
                        div()
                            .id(("rtv-tab-close", idx))
                            .px(px(3.0))
                            .text_size(px(11.0))
                            .text_color(rgb(0xF87171))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_or_kill_tab(idx, true);
                                    cx.notify();
                                }),
                            )
                            .child("×"),
                    ),
            );
        }

        let keep_alive_active = active_tab.is_some_and(|t| t.keep_alive_on_exit);
        let has_sel = self.selection.is_some();
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .id("rtv-new-session")
                    .cursor_pointer()
                    .px(px(9.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .bg(rgb(pal.accent))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xFFFFFF))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.open_new_tab(None);
                            this.is_focused = true;
                            cx.notify();
                        }),
                    )
                    .child(tr_str(&lang, "+ 新建会话", "+ New Session")),
            )
            .child(
                div()
                    .id("rtv-keep-alive-toggle")
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .bg(rgb(if keep_alive_active { 0x1E293B } else { 0x0F172A }))
                    .border_1()
                    .border_color(rgb(if keep_alive_active {
                        0x38BDF8
                    } else {
                        0x334155
                    }))
                    .text_size(px(11.0))
                    .text_color(rgb(0xE2E8F0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.toggle_exit_keep_alive(active_idx);
                            cx.notify();
                        }),
                    )
                    .child(if keep_alive_active {
                        tr_str(&lang, "退出保留", "Keep Alive")
                    } else {
                        tr_str(&lang, "退出关闭", "Auto Close")
                    }),
            )
            .when(has_sel, |row| {
                row.child(
                    div()
                        .id("rtv-copy-sel")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .bg(rgb(0x1E293B))
                        .text_size(px(11.0))
                        .text_color(rgb(0x38BDF8))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                if let Some(txt) = this.copy_selection_to_clipboard() {
                                    cx.write_to_clipboard(ClipboardItem::new_string(txt));
                                }
                                cx.notify();
                            }),
                        )
                        .child(tr_str(&lang, "复制选区", "Copy")),
                )
            });

        tab_bar = tab_bar.child(tabs_strip).child(controls);

        let grid_el = active_tab
            .map(|tab| self.render_grid_rows(tab, &pal))
            .unwrap_or_else(|| div().into_any_element());

        let footer_info = active_tab
            .map(|t| {
                let sb_note = if t.screen.scroll_offset > 0 {
                    format!(
                        " • {} -{}",
                        if is_zh { "回看" } else { "Scrollback" },
                        t.screen.scroll_offset
                    )
                } else {
                    String::new()
                };
                format!(
                    "{} ({}x{}) • {}{}",
                    t.name,
                    t.screen.cols,
                    t.screen.rows,
                    if t.alive {
                        tr_str(&lang, "运行中", "Running")
                    } else {
                        tr_str(&lang, "已退出", "Exited")
                    },
                    sb_note
                )
            })
            .unwrap_or_default();

        div()
            .id("remote-terminal-view-root")
            .size_full()
            .rounded(px(10.0))
            .bg(rgb(term_bg))
            .border_1()
            .border_color(rgb(if self.is_focused {
                pal.accent
            } else {
                0x1E293B
            }))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(tab_bar)
            .child(
                div()
                    .id("rtv-viewport")
                    .flex_1()
                    .min_h(px(0.0))
                    .p(px(10.0))
                    .overflow_hidden()
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.is_focused = true;
                            cx.notify();
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                        let dy = match ev.delta {
                            ScrollDelta::Lines(p) => p.y.round() as i32,
                            ScrollDelta::Pixels(p) => (f32::from(p.y) / 16.0).round() as i32,
                        };
                        if dy != 0 {
                            this.scroll_active_wheel(dy);
                            cx.notify();
                        }
                    }))
                    .child(grid_el)
                    .when_some(self.ime_preedit.as_ref(), |vp, pre| {
                        vp.child(
                            div()
                                .mt(px(4.0))
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(rgb(0x1E293B))
                                .text_xs()
                                .text_color(rgb(0x38BDF8))
                                .child(format!("IME: {pre}")),
                        )
                    }),
            )
            .child(
                div()
                    .px(px(10.0))
                    .py(px(5.0))
                    .bg(rgb(0x0F172A))
                    .border_t_1()
                    .border_color(rgb(0x1E293B))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(tg_icon(IconName::TerminalMcp, 12.0, 0x38BDF8))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(0x94A3B8))
                                    .child(footer_info),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(0x64748B))
                            .child(tr_str(
                                &lang,
                                "支持 ANSI/TrueColor · 滚轮回看 · Ctrl+C/D · OSC 52 剪贴板",
                                "ANSI/TrueColor · Scrollback · Ctrl+C/D · OSC 52 Clipboard",
                            )),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn test_ansi_vt100_sgr_16_256_truecolor_cursor_scrollback_and_osc52() {
        let mut screen = TerminalScreen::new(40, 6);
        screen.feed_str("\x1b[31mRed\x1b[38;5;196m256\x1b[38;2;12;34;56mTC\x1b[0m\r\n");

        let row0 = screen.visible_row(0).unwrap();
        assert_eq!(row0[0].ch, 'R');
        assert_eq!(row0[0].fg, ANSI_16[1]);
        assert_eq!(row0[3].ch, '2');
        assert_eq!(row0[3].fg, xterm_256_color(196));
        assert_eq!(row0[6].ch, 'T');
        assert_eq!(row0[6].fg, 0x0C2238);

        // Push lines beyond 6 rows to fill scrollback
        for i in 1..=10 {
            screen.feed_str(&format!("line-{i}\r\n"));
        }
        assert!(screen.scrollback_len() >= 4);
        screen.scroll_lines(2);
        assert_eq!(screen.scroll_offset, 2);
        screen.scroll_lines(-10);
        assert_eq!(screen.scroll_offset, 0);

        // OSC 52 clipboard writeback: base64("hello fabushi") = "aGVsbG8gZmFidXNoaQ=="
        screen.feed_str("\x1b]52;c;aGVsbG8gZmFidXNoaQ==\x07");
        assert_eq!(
            screen.pending_osc52_clipboard,
            vec!["hello fabushi".to_string()]
        );

        // Selection range extraction
        let mut sel_screen = TerminalScreen::new(30, 4);
        sel_screen.feed_str("hello world\r\nsecond line");
        let extracted = sel_screen.extract_text_range((0, 0), (1, 5));
        assert_eq!(extracted, "hello world\nsecond");
    }

    #[std::prelude::v1::test]
    fn test_key_encoding_and_multi_tab_session_lifecycle() {
        assert_eq!(
            encode_terminal_key("enter", false, false, false, None),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            encode_terminal_key("c", false, true, false, None),
            Some(vec![0x03])
        );
        assert_eq!(
            encode_terminal_key("up", false, false, false, None),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_terminal_key("tab", true, false, false, None),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            encode_terminal_key("a", false, false, false, Some("你")),
            Some("你".as_bytes().to_vec())
        );

        let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12);
        let mut view = RemoteTerminalView::new(pal, "zh-CN");
        assert_eq!(view.tabs.len(), 1);
        let s2 = view.open_new_tab(Some("build".to_string()));
        assert_eq!(view.tabs.len(), 2);
        assert_eq!(view.active_tab_index, 1);

        view.begin_rename_tab(1);
        view.rename_draft = "deploy-box".to_string();
        assert!(view.commit_rename());
        assert_eq!(view.tabs[1].name, "deploy-box");

        // Feed OSC 52 from session output -> emits CopyToClipboard
        view.feed_session_output(&s2, b"\x1b]52;c;dGVzdC1jbGlw\x07output");
        // Exit with keep_alive_on_exit = true keeps tab visible
        view.mark_session_exited(&s2, 42);
        assert_eq!(view.tabs.len(), 2);
        assert!(!view.tabs[1].alive);
        assert_eq!(view.tabs[1].exit_code, Some(42));

        let actions = view.take_actions();
        assert!(actions.iter().any(|a| matches!(a, RemoteTerminalAction::NewSession { .. })));
        assert!(actions.iter().any(|a| matches!(a, RemoteTerminalAction::RenameSession { new_name, .. } if new_name == "deploy-box")));
        assert!(actions.iter().any(|a| matches!(a, RemoteTerminalAction::CopyToClipboard(t) if t == "test-clip")));

        // Kill removes tab
        view.close_or_kill_tab(1, true);
        assert_eq!(view.tabs.len(), 1);
    }
}
