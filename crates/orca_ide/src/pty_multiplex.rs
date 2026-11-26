//! Headless VT Screen Snapshot Buffer, Off-UI `DaemonTransientFact` Extractor,
//! Hidden View Cold-Park, Credit/ACK Round-Robin Multiplexer with `ack-pending-overflow`
//! Recovery Snapshots, `PtyIncarnationId` Kill Guards, and Durable 30-Day
//! `ClosedTerminalTabTombstone` Registry (`F14`).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use bytes::{Bytes, BytesMut};
use orca_core::workspace_layout::{
    CLOSED_TAB_TOMBSTONE_MAX_PER_PARTITION, CLOSED_TAB_TOMBSTONE_MS, ClosedTabTombstone,
    TabContentType, TabLeafState, TabViewMode, WorkspaceLayoutModel, WorkspaceTab,
};
use orca_core::{CLOSED_TAB_TOMBSTONE_DAYS, OrcaIdeErrorCode};
use parking_lot::Mutex;
use regex::Regex;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{OrcaIdeRuntimeError, Result};

/// Default incremental output ring buffer budget per PTY session (256 KiB).
pub const DEFAULT_VT_INCREMENTAL_RING_BYTES: usize = 256 * 1024;

/// Incremental output returned by [`HeadlessVtBuffer::take_pending_output`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOutputChunk {
    pub next_cursor: u64,
    pub data: Bytes,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VtParserState {
    Ground,
    Escape,
    Csi,
    Osc,
    OscEsc,
}

/// Pure-Rust headless VT100/ANSI screen state machine with scrollback ring and
/// incremental output cursor checkpoints.
#[derive(Debug, Clone)]
pub struct HeadlessVtBuffer {
    cols: usize,
    rows: usize,
    max_scrollback_lines: usize,
    grid: Vec<Vec<char>>,
    cursor_row: usize,
    cursor_col: usize,
    scrollback: VecDeque<String>,
    parser_state: VtParserState,
    csi_buf: String,
    utf8_pending: Vec<u8>,
    incremental_chunks: VecDeque<(u64, Bytes)>,
    incremental_bytes: usize,
    max_incremental_bytes: usize,
    next_cursor: u64,
    earliest_cursor: u64,
}

impl HeadlessVtBuffer {
    pub fn new(cols: usize, rows: usize, max_scrollback_lines: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            cols,
            rows,
            max_scrollback_lines,
            grid: vec![vec![' '; cols]; rows],
            cursor_row: 0,
            cursor_col: 0,
            scrollback: VecDeque::new(),
            parser_state: VtParserState::Ground,
            csi_buf: String::new(),
            utf8_pending: Vec::new(),
            incremental_chunks: VecDeque::new(),
            incremental_bytes: 0,
            max_incremental_bytes: DEFAULT_VT_INCREMENTAL_RING_BYTES,
            next_cursor: 0,
            earliest_cursor: 0,
        }
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    pub fn current_cursor(&self) -> u64 {
        self.next_cursor
    }

    /// Feeds raw PTY output bytes into both the incremental output log and the VT grid.
    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let chunk = Bytes::copy_from_slice(bytes);
        let start_cursor = self.next_cursor;
        self.next_cursor = self.next_cursor.saturating_add(chunk.len() as u64);
        self.incremental_bytes = self.incremental_bytes.saturating_add(chunk.len());
        self.incremental_chunks.push_back((start_cursor, chunk));

        while self.incremental_bytes > self.max_incremental_bytes
            && self.incremental_chunks.len() > 1
        {
            if let Some((_, evicted)) = self.incremental_chunks.pop_front() {
                self.incremental_bytes = self.incremental_bytes.saturating_sub(evicted.len());
                if let Some((front_cursor, _)) = self.incremental_chunks.front() {
                    self.earliest_cursor = *front_cursor;
                }
            }
        }

        let mut combined = Vec::with_capacity(self.utf8_pending.len() + bytes.len());
        combined.extend_from_slice(&self.utf8_pending);
        combined.extend_from_slice(bytes);
        self.utf8_pending.clear();

        let mut idx = 0usize;
        while idx < combined.len() {
            match std::str::from_utf8(&combined[idx..]) {
                Ok(valid) => {
                    for ch in valid.chars() {
                        self.process_char(ch);
                    }
                    break;
                }
                Err(err) => {
                    let valid_up_to = err.valid_up_to();
                    if valid_up_to > 0
                        && let Ok(valid) = std::str::from_utf8(&combined[idx..idx + valid_up_to])
                    {
                        for ch in valid.chars() {
                            self.process_char(ch);
                        }
                    }
                    idx += valid_up_to;
                    match err.error_len() {
                        Some(bad_len) => {
                            self.process_char('\u{FFFD}');
                            idx += bad_len;
                        }
                        None => {
                            // Incomplete trailing multi-byte UTF-8 sequence: hold for next chunk.
                            self.utf8_pending.extend_from_slice(&combined[idx..]);
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Reads incremental raw bytes emitted since `cursor` and advances the checkpoint cursor.
    pub fn take_pending_output(&self, cursor: u64) -> PendingOutputChunk {
        let truncated = cursor < self.earliest_cursor;
        let effective_cursor = cursor.max(self.earliest_cursor);
        let mut out = BytesMut::new();
        for (start, chunk) in &self.incremental_chunks {
            let end = start.saturating_add(chunk.len() as u64);
            if end <= effective_cursor {
                continue;
            }
            let slice_start = effective_cursor.saturating_sub(*start) as usize;
            if slice_start < chunk.len() {
                out.extend_from_slice(&chunk[slice_start..]);
            }
        }
        PendingOutputChunk {
            next_cursor: self.next_cursor,
            data: out.freeze(),
            truncated,
        }
    }

    /// Returns the active visible screen grid serialized as trimmed lines.
    pub fn visible_screen_text(&self) -> String {
        let mut lines: Vec<String> = self
            .grid
            .iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_string())
            .collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Serializes bounded scrollback + active visible screen grid as a compact snapshot.
    pub fn snapshot_text(&self) -> String {
        let mut lines: Vec<String> = self.scrollback.iter().cloned().collect();
        for row in &self.grid {
            lines.push(row.iter().collect::<String>().trim_end().to_string());
        }
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }
}

impl HeadlessVtBuffer {
    fn process_char(&mut self, ch: char) {
        match self.parser_state {
            VtParserState::Ground => match ch {
                '\x1b' => self.parser_state = VtParserState::Escape,
                '\r' => self.cursor_col = 0,
                '\n' => self.newline(),
                '\x08' => self.cursor_col = self.cursor_col.saturating_sub(1),
                '\t' => {
                    let next_stop = ((self.cursor_col / 8) + 1) * 8;
                    self.cursor_col = next_stop.min(self.cols.saturating_sub(1));
                }
                '\x07' => {}
                c if c.is_control() => {}
                c => {
                    if self.cursor_col >= self.cols {
                        self.cursor_col = 0;
                        self.newline();
                    }
                    if self.cursor_row < self.rows && self.cursor_col < self.cols {
                        self.grid[self.cursor_row][self.cursor_col] = c;
                        self.cursor_col += 1;
                    }
                }
            },
            VtParserState::Escape => match ch {
                '[' => {
                    self.csi_buf.clear();
                    self.parser_state = VtParserState::Csi;
                }
                ']' => {
                    self.parser_state = VtParserState::Osc;
                }
                _ => {
                    self.parser_state = VtParserState::Ground;
                }
            },
            VtParserState::Csi => {
                if ('\x40'..='\x7E').contains(&ch) {
                    let params = std::mem::take(&mut self.csi_buf);
                    self.apply_csi(&params, ch);
                    self.parser_state = VtParserState::Ground;
                } else if self.csi_buf.len() < 128 {
                    self.csi_buf.push(ch);
                }
            }
            VtParserState::Osc => match ch {
                '\x07' => self.parser_state = VtParserState::Ground,
                '\x1b' => self.parser_state = VtParserState::OscEsc,
                _ => {}
            },
            VtParserState::OscEsc => {
                if ch == '\\' {
                    self.parser_state = VtParserState::Ground;
                } else {
                    self.parser_state = VtParserState::Osc;
                }
            }
        }
    }

    fn newline(&mut self) {
        if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        } else {
            let evicted = self.grid.remove(0);
            if self.max_scrollback_lines > 0 {
                let line = evicted.iter().collect::<String>().trim_end().to_string();
                self.scrollback.push_back(line);
                while self.scrollback.len() > self.max_scrollback_lines {
                    self.scrollback.pop_front();
                }
            }
            self.grid.push(vec![' '; self.cols]);
        }
    }

    fn apply_csi(&mut self, raw_params: &str, final_char: char) {
        let cleaned = raw_params.trim_start_matches('?');
        let nums: Vec<usize> = cleaned
            .split(';')
            .map(|s| s.parse::<usize>().unwrap_or(0))
            .collect();
        let first_or_one = nums.first().copied().unwrap_or(0).max(1);

        match final_char {
            'A' => self.cursor_row = self.cursor_row.saturating_sub(first_or_one),
            'B' => self.cursor_row = (self.cursor_row + first_or_one).min(self.rows - 1),
            'C' => self.cursor_col = (self.cursor_col + first_or_one).min(self.cols - 1),
            'D' => self.cursor_col = self.cursor_col.saturating_sub(first_or_one),
            'G' => self.cursor_col = first_or_one.saturating_sub(1).min(self.cols - 1),
            'H' | 'f' => {
                let row = nums.first().copied().unwrap_or(1).max(1).saturating_sub(1);
                let col = nums.get(1).copied().unwrap_or(1).max(1).saturating_sub(1);
                self.cursor_row = row.min(self.rows - 1);
                self.cursor_col = col.min(self.cols - 1);
            }
            'J' => {
                let mode = nums.first().copied().unwrap_or(0);
                match mode {
                    2 | 3 => {
                        for row in &mut self.grid {
                            row.fill(' ');
                        }
                        if mode == 3 {
                            self.scrollback.clear();
                        }
                    }
                    1 => {
                        for r in 0..self.cursor_row {
                            self.grid[r].fill(' ');
                        }
                        let end = (self.cursor_col + 1).min(self.cols);
                        self.grid[self.cursor_row][..end].fill(' ');
                    }
                    _ => {
                        let start = self.cursor_col.min(self.cols);
                        self.grid[self.cursor_row][start..].fill(' ');
                        for r in (self.cursor_row + 1)..self.rows {
                            self.grid[r].fill(' ');
                        }
                    }
                }
            }
            'K' => {
                let mode = nums.first().copied().unwrap_or(0);
                match mode {
                    2 => self.grid[self.cursor_row].fill(' '),
                    1 => {
                        let end = (self.cursor_col + 1).min(self.cols);
                        self.grid[self.cursor_row][..end].fill(' ');
                    }
                    _ => {
                        let start = self.cursor_col.min(self.cols);
                        self.grid[self.cursor_row][start..].fill(' ');
                    }
                }
            }
            _ => {}
        }
    }
}

/// Off-UI transient facts extracted from raw PTY byte streams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DaemonTransientFact {
    Bell,
    TitleChanged { title: String },
    CommandFinished { exit_code: Option<i32> },
    PrLink { url: String },
    Mode2031Subscribe,
    Mode2031Unsubscribe,
    ForegroundChanged { process: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FactParserState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    OscEsc,
}

/// Stateful streaming parser extracting [`DaemonTransientFact`] events from raw PTY chunks.
#[derive(Debug, Default, Clone)]
pub struct TransientFactExtractor {
    state: FactParserState,
    csi_buf: String,
    osc_buf: String,
    plain_line_buf: String,
}

fn pr_link_regex() -> Option<&'static Regex> {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"https?://(?:github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/pull/[0-9]+|[A-Za-z0-9.-]+/[A-Za-z0-9_./-]+/-/merge_requests/[0-9]+)",
        )
        .ok()
    })
    .as_ref()
}

impl TransientFactExtractor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses a raw PTY byte chunk and returns all extracted [`DaemonTransientFact`] events.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<DaemonTransientFact> {
        let mut facts = Vec::new();
        let text = String::from_utf8_lossy(chunk);

        for ch in text.chars() {
            match self.state {
                FactParserState::Ground => match ch {
                    '\x07' => facts.push(DaemonTransientFact::Bell),
                    '\x1b' => self.state = FactParserState::Escape,
                    '\n' | '\r' => {
                        self.flush_plain_line(&mut facts);
                    }
                    c if c.is_control() => {}
                    c => {
                        if self.plain_line_buf.len() < 4096 {
                            self.plain_line_buf.push(c);
                        }
                    }
                },
                FactParserState::Escape => match ch {
                    '[' => {
                        self.csi_buf.clear();
                        self.state = FactParserState::Csi;
                    }
                    ']' => {
                        self.osc_buf.clear();
                        self.state = FactParserState::Osc;
                    }
                    _ => self.state = FactParserState::Ground,
                },
                FactParserState::Csi => {
                    if ('\x40'..='\x7E').contains(&ch) {
                        if self.csi_buf == "?2031" && ch == 'h' {
                            facts.push(DaemonTransientFact::Mode2031Subscribe);
                        } else if self.csi_buf == "?2031" && ch == 'l' {
                            facts.push(DaemonTransientFact::Mode2031Unsubscribe);
                        }
                        self.csi_buf.clear();
                        self.state = FactParserState::Ground;
                    } else if self.csi_buf.len() < 64 {
                        self.csi_buf.push(ch);
                    }
                }
                FactParserState::Osc => match ch {
                    '\x07' => {
                        let osc = std::mem::take(&mut self.osc_buf);
                        parse_osc_fact(&osc, &mut facts);
                        self.state = FactParserState::Ground;
                    }
                    '\x1b' => self.state = FactParserState::OscEsc,
                    c => {
                        if self.osc_buf.len() < 4096 {
                            self.osc_buf.push(c);
                        }
                    }
                },
                FactParserState::OscEsc => {
                    if ch == '\\' {
                        let osc = std::mem::take(&mut self.osc_buf);
                        parse_osc_fact(&osc, &mut facts);
                        self.state = FactParserState::Ground;
                    } else {
                        self.state = FactParserState::Osc;
                    }
                }
            }
        }

        // Also scan any complete PR URLs present in the current line buffer without waiting for '\n'
        // if followed by whitespace or at end of chunk.
        if !self.plain_line_buf.is_empty() {
            self.extract_pr_links_from_tail(&mut facts);
        }

        facts
    }

    fn flush_plain_line(&mut self, facts: &mut Vec<DaemonTransientFact>) {
        if self.plain_line_buf.is_empty() {
            return;
        }
        let line = std::mem::take(&mut self.plain_line_buf);
        if let Some(re) = pr_link_regex() {
            for m in re.find_iter(&line) {
                facts.push(DaemonTransientFact::PrLink {
                    url: m.as_str().to_string(),
                });
            }
        }
    }

    fn extract_pr_links_from_tail(&mut self, facts: &mut Vec<DaemonTransientFact>) {
        if let Some(re) = pr_link_regex() {
            let mut last_end = 0usize;
            for m in re.find_iter(&self.plain_line_buf) {
                facts.push(DaemonTransientFact::PrLink {
                    url: m.as_str().to_string(),
                });
                last_end = m.end();
            }
            if last_end > 0 {
                self.plain_line_buf = self.plain_line_buf[last_end..].to_string();
            }
        }
    }
}

fn parse_osc_fact(osc: &str, facts: &mut Vec<DaemonTransientFact>) {
    let Some((code, body)) = osc.split_once(';') else {
        return;
    };
    match code.trim() {
        "0" | "2" => {
            facts.push(DaemonTransientFact::TitleChanged {
                title: body.to_string(),
            });
        }
        "133" => {
            // OSC 133;D or OSC 133;D;<exit_code>
            let mut parts = body.split(';');
            if parts.next() == Some("D") {
                let exit_code = parts
                    .next()
                    .and_then(|s| s.trim().parse::<i32>().ok());
                facts.push(DaemonTransientFact::CommandFinished { exit_code });
            }
        }
        "7501" => {
            for token in body.split([';', '&', ',']) {
                if let Some((k, v)) = token.split_once('=') {
                    let key = k.trim();
                    let val = v.trim();
                    if (key == "fg" || key == "foreground") && !val.is_empty() {
                        facts.push(DaemonTransientFact::ForegroundChanged {
                            process: val.to_string(),
                        });
                    }
                }
            }
        }
        _ => {}
    }
}

/// Unique incarnation identifier distinguishing a live PTY instance from recycled tab IDs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PtyIncarnationId(pub String);

impl PtyIncarnationId {
    pub fn generate() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Kind of multiplexed frame emitted by [`PtyMultiplexController::drain_round_robin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MuxFrameKind {
    OutputDelta,
    RecoverySnapshot,
}

/// Multiplexed output or recovery snapshot frame for a single PTY session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuxFrame {
    pub tab_id: String,
    pub incarnation_id: PtyIncarnationId,
    pub kind: MuxFrameKind,
    pub payload: Bytes,
    pub dropped_bytes: usize,
}

/// Guarded kill request enforcing incarnation, agent-owner, and attachment invariants.
#[derive(Debug, Clone)]
pub struct KillSessionRequest {
    pub tab_id: String,
    pub expected_incarnation_id: Option<PtyIncarnationId>,
    pub require_no_agent_owner: bool,
    pub require_unattached: bool,
    pub now_ms: i64,
}

/// Configuration for [`PtyMultiplexController`] flow control and headless VT sizing.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiplexConfig {
    pub initial_credit_bytes: usize,
    pub ack_overflow_budget_bytes: usize,
    pub round_robin_quantum_bytes: usize,
    pub vt_cols: usize,
    pub vt_rows: usize,
    pub vt_scrollback: usize,
    pub link_ewma_alpha: f64,
}

impl Default for MultiplexConfig {
    fn default() -> Self {
        Self {
            initial_credit_bytes: 64 * 1024,
            ack_overflow_budget_bytes: 256 * 1024,
            round_robin_quantum_bytes: 4 * 1024,
            vt_cols: 120,
            vt_rows: 36,
            vt_scrollback: 2_000,
            link_ewma_alpha: 0.25,
        }
    }
}

/// Flow-control statistics returned by [`PtyMultiplexController::ack`].
#[derive(Debug, Clone, PartialEq)]
pub struct AckStats {
    pub tab_id: String,
    pub credit_remaining: usize,
    pub unacked_bytes: usize,
    pub link_estimate_bps: f64,
}

/// Durable 30-day closed-tab tombstone persisted by `PtyMultiplexController`.
pub type ClosedTerminalTabTombstone = ClosedTabTombstone;

struct PtySessionState {
    tab_id: String,
    incarnation_id: PtyIncarnationId,
    vt: HeadlessVtBuffer,
    facts: TransientFactExtractor,
    background: bool,
    cold_park_dirty: bool,
    pending_queue: VecDeque<Bytes>,
    pending_bytes: usize,
    credit_remaining: usize,
    unacked_bytes: usize,
    needs_recovery_snapshot: bool,
    dropped_bytes: usize,
    attached_clients: BTreeSet<String>,
    agent_owner: Option<String>,
}

struct MultiplexInner {
    config: MultiplexConfig,
    layout: WorkspaceLayoutModel,
    sessions: BTreeMap<String, PtySessionState>,
    rr_cursor: usize,
    link_estimate_bps: f64,
    db: Option<Connection>,
}

/// Pure-Rust PTY multiplexer (`Send + Sync`) providing headless VT snapshots,
/// off-UI transient fact extraction, hidden view cold-parking, fair credit/ACK
/// round-robin scheduling with `ack-pending-overflow` recovery snapshots,
/// incarnation-guarded kills, and durable 30-day closed-tab tombstones.
pub struct PtyMultiplexController {
    data_dir: Option<PathBuf>,
    inner: Mutex<MultiplexInner>,
}

impl PtyMultiplexController {
    /// Opens a durable controller persisting 30-day closed-tab tombstones under
    /// `<data_dir>/pty_tombstones.db` (SQLite WAL).
    pub fn open(
        data_dir: impl AsRef<Path>,
        workspace_id: impl Into<String>,
        host_id: impl Into<String>,
        initial_pane_id: impl Into<String>,
        config: MultiplexConfig,
    ) -> Result<Self> {
        let dir = data_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let db_path = dir.join("pty_tombstones.db");
        let conn = Connection::open(&db_path)?;
        init_tombstone_schema(&conn)?;

        let ws_id = workspace_id.into();
        let h_id = host_id.into();
        let mut layout = WorkspaceLayoutModel::new(ws_id.clone(), h_id.clone(), initial_pane_id);
        layout.closed_tombstones = load_persisted_tombstones(&conn, &ws_id, &h_id)?;

        Ok(Self {
            data_dir: Some(dir),
            inner: Mutex::new(MultiplexInner {
                config,
                layout,
                sessions: BTreeMap::new(),
                rr_cursor: 0,
                link_estimate_bps: 1_000_000.0,
                db: Some(conn),
            }),
        })
    }

    /// Creates an in-memory controller without disk persistence.
    pub fn in_memory(
        workspace_id: impl Into<String>,
        host_id: impl Into<String>,
        initial_pane_id: impl Into<String>,
        config: MultiplexConfig,
    ) -> Self {
        let layout = WorkspaceLayoutModel::new(workspace_id, host_id, initial_pane_id);
        Self {
            data_dir: None,
            inner: Mutex::new(MultiplexInner {
                config,
                layout,
                sessions: BTreeMap::new(),
                rr_cursor: 0,
                link_estimate_bps: 1_000_000.0,
                db: None,
            }),
        }
    }

    pub fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }

    pub fn link_estimate_bps(&self) -> f64 {
        self.inner.lock().link_estimate_bps
    }

    /// Creates a new PTY session and registers its tab in `WorkspaceLayoutModel`.
    /// Refuses stale creation for any unexpired 30-day closed-tab tombstone with
    /// `OrcaIdeErrorCode::TabNotFound`.
    pub fn create_session(
        &self,
        tab_id: impl Into<String>,
        pane_id: &str,
        title: impl Into<String>,
        now_ms: i64,
    ) -> Result<PtyIncarnationId> {
        let tab_id = tab_id.into();
        let mut guard = self.inner.lock();
        guard.layout.prune_expired_tombstones(now_ms);
        if guard.layout.is_tab_tombstoned(&tab_id, now_ms) {
            return Err(OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::TabNotFound,
                format!(
                    "tab '{tab_id}' is tombstoned (closed within {CLOSED_TAB_TOMBSTONE_DAYS} days); refusing stale create"
                ),
            ));
        }
        if guard.sessions.contains_key(&tab_id) {
            return Err(OrcaIdeRuntimeError::conflict(format!(
                "PTY session '{tab_id}' already exists"
            )));
        }

        let incarnation = PtyIncarnationId::generate();
        let tab = WorkspaceTab {
            tab_id: tab_id.clone(),
            content_type: TabContentType::Terminal,
            title: title.into(),
            pinned: false,
            preview: false,
            view_mode: TabViewMode::Terminal,
            leaf_state: TabLeafState::Live,
            pty_incarnation_id: Some(incarnation.0.clone()),
            dirty: false,
            created_at_ms: now_ms,
            last_active_at_ms: now_ms,
        };
        guard.layout.open_tab(pane_id, tab, now_ms)?;

        let cfg = guard.config.clone();
        guard.sessions.insert(
            tab_id.clone(),
            PtySessionState {
                tab_id,
                incarnation_id: incarnation.clone(),
                vt: HeadlessVtBuffer::new(cfg.vt_cols, cfg.vt_rows, cfg.vt_scrollback),
                facts: TransientFactExtractor::new(),
                background: false,
                cold_park_dirty: false,
                pending_queue: VecDeque::new(),
                pending_bytes: 0,
                credit_remaining: cfg.initial_credit_bytes,
                unacked_bytes: 0,
                needs_recovery_snapshot: false,
                dropped_bytes: 0,
                attached_clients: BTreeSet::new(),
                agent_owner: None,
            },
        );
        Ok(incarnation)
    }

    /// Attaches a client to an active PTY session; refuses tombstoned or missing tabs with `TabNotFound`.
    pub fn attach_session(
        &self,
        tab_id: &str,
        client_id: impl Into<String>,
        now_ms: i64,
    ) -> Result<PtyIncarnationId> {
        let mut guard = self.inner.lock();
        guard.layout.prune_expired_tombstones(now_ms);
        if guard.layout.is_tab_tombstoned(tab_id, now_ms) {
            return Err(OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::TabNotFound,
                format!("tab '{tab_id}' is tombstoned; refusing stale attach"),
            ));
        }
        let session = guard.sessions.get_mut(tab_id).ok_or_else(|| {
            OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::TabNotFound,
                format!("PTY session '{tab_id}' not found"),
            )
        })?;
        session.attached_clients.insert(client_id.into());
        Ok(session.incarnation_id.clone())
    }

    pub fn detach_session(&self, tab_id: &str, client_id: &str) -> Result<()> {
        let mut guard = self.inner.lock();
        let session = guard.sessions.get_mut(tab_id).ok_or_else(|| {
            OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::TabNotFound,
                format!("PTY session '{tab_id}' not found"),
            )
        })?;
        session.attached_clients.remove(client_id);
        Ok(())
    }

    pub fn set_agent_owner(&self, tab_id: &str, owner: Option<String>) -> Result<()> {
        let mut guard = self.inner.lock();
        let session = guard.sessions.get_mut(tab_id).ok_or_else(|| {
            OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::TabNotFound,
                format!("PTY session '{tab_id}' not found"),
            )
        })?;
        session.agent_owner = owner;
        Ok(())
    }
}

impl PtyMultiplexController {
    /// Parks (`hidden = true`) or unparks (`hidden = false`) a session off active render delivery.
    /// When unparking after background output arrived, schedules a single compact `RecoverySnapshot`.
    pub fn set_session_background(&self, tab_id: &str, hidden: bool) -> Result<()> {
        let mut guard = self.inner.lock();
        let session = guard.sessions.get_mut(tab_id).ok_or_else(|| {
            OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::TabNotFound,
                format!("PTY session '{tab_id}' not found"),
            )
        })?;
        if hidden {
            session.background = true;
            if session.pending_bytes > 0 {
                session.dropped_bytes = session.dropped_bytes.saturating_add(session.pending_bytes);
                session.pending_queue.clear();
                session.pending_bytes = 0;
                session.cold_park_dirty = true;
            }
        } else {
            let was_hidden = session.background;
            session.background = false;
            if was_hidden && session.cold_park_dirty {
                session.needs_recovery_snapshot = true;
                session.cold_park_dirty = false;
            }
        }
        Ok(())
    }

    /// Feeds raw PTY output into the headless VT buffer, extracts off-UI transient facts,
    /// honors hidden-view cold-parking, and enforces `ack-pending-overflow` recovery snapshots.
    pub fn ingest_pty_output(
        &self,
        tab_id: &str,
        chunk: &[u8],
    ) -> Result<Vec<DaemonTransientFact>> {
        let mut guard = self.inner.lock();
        let overflow_budget = guard.config.ack_overflow_budget_bytes;
        let session = guard.sessions.get_mut(tab_id).ok_or_else(|| {
            OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::TabNotFound,
                format!("PTY session '{tab_id}' not found"),
            )
        })?;

        session.vt.feed(chunk);
        let facts = session.facts.feed(chunk);

        if chunk.is_empty() {
            return Ok(facts);
        }

        if session.background {
            session.cold_park_dirty = true;
            session.dropped_bytes = session.dropped_bytes.saturating_add(chunk.len());
            return Ok(facts);
        }

        let projected_backlog = session
            .unacked_bytes
            .saturating_add(session.pending_bytes)
            .saturating_add(chunk.len());

        if projected_backlog > overflow_budget {
            // Budgeted `ack-pending-overflow`: collapse stale delta backlog into a compact VT snapshot.
            session.dropped_bytes = session
                .dropped_bytes
                .saturating_add(session.pending_bytes)
                .saturating_add(chunk.len());
            session.pending_queue.clear();
            session.pending_bytes = 0;
            session.needs_recovery_snapshot = true;
        } else if session.needs_recovery_snapshot {
            // Already coalescing into a recovery snapshot; keep collapsing stale deltas until drained.
            session.dropped_bytes = session.dropped_bytes.saturating_add(chunk.len());
        } else {
            session.pending_queue.push_back(Bytes::copy_from_slice(chunk));
            session.pending_bytes = session.pending_bytes.saturating_add(chunk.len());
        }

        Ok(facts)
    }

    /// Drains pending frames fairly in round-robin order across foreground PTYs up to
    /// `max_bytes`, enforcing per-session credit windows and emitting `RecoverySnapshot`
    /// frames for cold-parked or `ack-pending-overflow` sessions.
    pub fn drain_round_robin(&self, max_bytes: usize) -> Vec<MuxFrame> {
        if max_bytes == 0 {
            return Vec::new();
        }
        let mut guard = self.inner.lock();
        let quantum = guard.config.round_robin_quantum_bytes.max(1);
        let tab_ids: Vec<String> = guard.sessions.keys().cloned().collect();
        if tab_ids.is_empty() {
            return Vec::new();
        }

        let mut frames = Vec::new();
        let mut remaining_budget = max_bytes;
        let count = tab_ids.len();
        let mut start_idx = guard.rr_cursor % count;

        while remaining_budget > 0 {
            let mut made_progress = false;
            for step in 0..count {
                if remaining_budget == 0 {
                    break;
                }
                let idx = (start_idx + step) % count;
                let tab_id = &tab_ids[idx];
                let Some(session) = guard.sessions.get_mut(tab_id) else {
                    continue;
                };
                if session.background {
                    continue;
                }

                if session.needs_recovery_snapshot {
                    let snap = Bytes::from(session.vt.snapshot_text().into_bytes());
                    let charge = snap.len().max(1);
                    if session.credit_remaining == 0 {
                        continue;
                    }
                    let dropped = session.dropped_bytes;
                    session.dropped_bytes = 0;
                    session.needs_recovery_snapshot = false;
                    session.credit_remaining = session.credit_remaining.saturating_sub(charge);
                    session.unacked_bytes = session.unacked_bytes.saturating_add(snap.len());
                    remaining_budget = remaining_budget.saturating_sub(snap.len().min(remaining_budget));
                    frames.push(MuxFrame {
                        tab_id: session.tab_id.clone(),
                        incarnation_id: session.incarnation_id.clone(),
                        kind: MuxFrameKind::RecoverySnapshot,
                        payload: snap,
                        dropped_bytes: dropped,
                    });
                    guard.rr_cursor = (idx + 1) % count;
                    made_progress = true;
                    continue;
                }

                if session.pending_bytes == 0 || session.credit_remaining == 0 {
                    continue;
                }
                let slice_cap = remaining_budget
                    .min(quantum)
                    .min(session.credit_remaining);
                if slice_cap == 0 {
                    continue;
                }

                let payload = pop_bounded_bytes(&mut session.pending_queue, slice_cap);
                let len = payload.len();
                if len == 0 {
                    continue;
                }
                session.pending_bytes = session.pending_bytes.saturating_sub(len);
                session.credit_remaining = session.credit_remaining.saturating_sub(len);
                session.unacked_bytes = session.unacked_bytes.saturating_add(len);
                remaining_budget = remaining_budget.saturating_sub(len);

                frames.push(MuxFrame {
                    tab_id: session.tab_id.clone(),
                    incarnation_id: session.incarnation_id.clone(),
                    kind: MuxFrameKind::OutputDelta,
                    payload,
                    dropped_bytes: 0,
                });
                guard.rr_cursor = (idx + 1) % count;
                made_progress = true;
            }

            if !made_progress {
                break;
            }
            start_idx = guard.rr_cursor % count;
        }

        frames
    }
}

fn pop_bounded_bytes(queue: &mut VecDeque<Bytes>, limit: usize) -> Bytes {
    let mut out = BytesMut::new();
    let mut rem = limit;
    while rem > 0 {
        let Some(mut front) = queue.pop_front() else {
            break;
        };
        if front.len() <= rem {
            rem -= front.len();
            out.extend_from_slice(&front);
        } else {
            let head = front.split_to(rem);
            out.extend_from_slice(&head);
            queue.push_front(front);
            break;
        }
    }
    out.freeze()
}

impl PtyMultiplexController {
    /// Acknowledges delivered bytes, replenishes session credit window, and updates the
    /// adaptive `link_estimate_bps` EWMA bytes/sec estimator.
    pub fn ack(&self, tab_id: &str, acked_bytes: usize, rtt_ms: u64) -> Result<AckStats> {
        let mut guard = self.inner.lock();
        let initial_credit = guard.config.initial_credit_bytes;
        let alpha = guard.config.link_ewma_alpha.clamp(0.01, 1.0);
        let prev_estimate = guard.link_estimate_bps;

        let (credit_remaining, unacked_bytes) = {
            let session = guard.sessions.get_mut(tab_id).ok_or_else(|| {
                OrcaIdeRuntimeError::ide(
                    OrcaIdeErrorCode::TabNotFound,
                    format!("PTY session '{tab_id}' not found"),
                )
            })?;
            session.unacked_bytes = session.unacked_bytes.saturating_sub(acked_bytes);
            session.credit_remaining = session
                .credit_remaining
                .saturating_add(acked_bytes)
                .min(initial_credit);
            (session.credit_remaining, session.unacked_bytes)
        };

        if acked_bytes > 0 {
            let secs = (rtt_ms.max(1) as f64) / 1_000.0;
            let sample_bps = (acked_bytes as f64) / secs;
            guard.link_estimate_bps = alpha * sample_bps + (1.0 - alpha) * prev_estimate;
        }

        Ok(AckStats {
            tab_id: tab_id.to_string(),
            credit_remaining,
            unacked_bytes,
            link_estimate_bps: guard.link_estimate_bps,
        })
    }

    pub fn take_pending_output(&self, tab_id: &str, cursor: u64) -> Result<PendingOutputChunk> {
        let guard = self.inner.lock();
        let session = guard.sessions.get(tab_id).ok_or_else(|| {
            OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::TabNotFound,
                format!("PTY session '{tab_id}' not found"),
            )
        })?;
        Ok(session.vt.take_pending_output(cursor))
    }

    pub fn snapshot_screen(&self, tab_id: &str) -> Result<String> {
        let guard = self.inner.lock();
        let session = guard.sessions.get(tab_id).ok_or_else(|| {
            OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::TabNotFound,
                format!("PTY session '{tab_id}' not found"),
            )
        })?;
        Ok(session.vt.snapshot_text())
    }

    /// Guarded session termination enforcing `expected_incarnation_id`,
    /// `require_no_agent_owner`, and `require_unattached`, then writing a durable 30-day
    /// [`ClosedTerminalTabTombstone`] into `WorkspaceLayoutModel` and SQLite WAL.
    pub fn kill_session(&self, req: &KillSessionRequest) -> Result<ClosedTerminalTabTombstone> {
        let mut guard = self.inner.lock();
        {
            let session = guard.sessions.get(&req.tab_id).ok_or_else(|| {
                OrcaIdeRuntimeError::ide(
                    OrcaIdeErrorCode::TabNotFound,
                    format!("PTY session '{}' not found", req.tab_id),
                )
            })?;
            if let Some(expected) = &req.expected_incarnation_id
                && &session.incarnation_id != expected
            {
                return Err(OrcaIdeRuntimeError::conflict(format!(
                    "incarnation mismatch for tab '{}': expected '{}', live '{}'",
                    req.tab_id, expected.0, session.incarnation_id.0
                )));
            }
            if req.require_no_agent_owner
                && let Some(owner) = &session.agent_owner
            {
                return Err(OrcaIdeRuntimeError::active_cli_owner(format!(
                    "tab '{}' is owned by active agent '{owner}'",
                    req.tab_id
                )));
            }
            if req.require_unattached && !session.attached_clients.is_empty() {
                return Err(OrcaIdeRuntimeError::conflict(format!(
                    "tab '{}' still has {} attached client(s)",
                    req.tab_id,
                    session.attached_clients.len()
                )));
            }
        }

        guard.sessions.remove(&req.tab_id);
        let tombstone = guard.layout.close_tab(&req.tab_id, req.now_ms)?;
        sync_tombstones_to_db(&guard)?;
        Ok(tombstone)
    }

    pub fn prune_expired_tombstones(&self, now_ms: i64) -> Result<usize> {
        let mut guard = self.inner.lock();
        let pruned = guard.layout.prune_expired_tombstones(now_ms);
        if pruned > 0 {
            sync_tombstones_to_db(&guard)?;
        }
        Ok(pruned)
    }

    pub fn is_tab_tombstoned(&self, tab_id: &str, now_ms: i64) -> bool {
        self.inner.lock().layout.is_tab_tombstoned(tab_id, now_ms)
    }

    pub fn list_tombstones(&self) -> Vec<ClosedTerminalTabTombstone> {
        self.inner.lock().layout.closed_tombstones.clone()
    }
}

fn init_tombstone_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;
        CREATE TABLE IF NOT EXISTS closed_terminal_tab_tombstones (
            workspace_id TEXT NOT NULL,
            host_id TEXT NOT NULL,
            tab_id TEXT NOT NULL,
            closed_at_ms INTEGER NOT NULL,
            payload_json TEXT NOT NULL,
            PRIMARY KEY (workspace_id, host_id, tab_id)
        );
        "#,
    )?;
    Ok(())
}

fn load_persisted_tombstones(
    conn: &Connection,
    workspace_id: &str,
    host_id: &str,
) -> Result<Vec<ClosedTerminalTabTombstone>> {
    let mut stmt = conn.prepare(
        "SELECT payload_json FROM closed_terminal_tab_tombstones
         WHERE workspace_id = ?1 AND host_id = ?2
         ORDER BY closed_at_ms ASC, tab_id ASC",
    )?;
    let rows = stmt.query_map(params![workspace_id, host_id], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(serde_json::from_str(&row?)?);
    }
    if out.len() > CLOSED_TAB_TOMBSTONE_MAX_PER_PARTITION {
        let excess = out.len() - CLOSED_TAB_TOMBSTONE_MAX_PER_PARTITION;
        out.drain(0..excess);
    }
    Ok(out)
}

fn sync_tombstones_to_db(inner: &MultiplexInner) -> Result<()> {
    let Some(conn) = &inner.db else {
        return Ok(());
    };
    let ws = &inner.layout.workspace_id;
    let host = &inner.layout.host_id;
    conn.execute(
        "DELETE FROM closed_terminal_tab_tombstones WHERE workspace_id = ?1 AND host_id = ?2",
        params![ws, host],
    )?;
    for tomb in &inner.layout.closed_tombstones {
        let payload = serde_json::to_string(tomb)?;
        conn.execute(
            "INSERT OR REPLACE INTO closed_terminal_tab_tombstones
             (workspace_id, host_id, tab_id, closed_at_ms, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ws, host, &tomb.tab_id, tomb.closed_at_ms, payload],
        )?;
    }
    Ok(())
}

/// Computes the exact 30-day expiration timestamp (ms) for a closed terminal tab tombstone.
pub const fn tombstone_expires_at_ms(closed_at_ms: i64) -> i64 {
    closed_at_ms.saturating_add(CLOSED_TAB_TOMBSTONE_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn headless_vt_snapshot_csi_utf8_and_incremental_cursor() {
        assert_send_sync::<PtyMultiplexController>();

        let mut vt = HeadlessVtBuffer::new(20, 3, 2);
        vt.feed(b"line1\r\nline2\r\nline3\r\nline4");
        // Scrollback cap = 2, visible = 3 -> line1 evicted? Wait: 4 lines total -> 1 in scrollback, 3 visible.
        let snap = vt.snapshot_text();
        assert!(snap.contains("line1\nline2\nline3\nline4"), "{snap}");

        // Backspace & carriage return overwrite + multi-byte UTF-8 split across chunks ("€" = E2 82 AC)
        vt.feed(b"\r\nAB\x08C ");
        vt.feed(&[0xE2, 0x82]);
        vt.feed(&[0xAC]);
        assert!(vt.visible_screen_text().contains("AC €"));

        // CSI clear screen + cursor home
        vt.feed(b"\x1b[2J\x1b[Hfresh");
        assert_eq!(vt.visible_screen_text(), "fresh");

        // Incremental cursor checkpoints
        let c1 = vt.take_pending_output(0);
        assert!(!c1.data.is_empty());
        assert!(!c1.truncated);
        vt.feed(b"_tail");
        let c2 = vt.take_pending_output(c1.next_cursor);
        assert_eq!(c2.data.as_ref(), b"_tail");
    }

    #[test]
    fn transient_fact_extractor_parses_all_seven_variants() {
        let mut ex = TransientFactExtractor::new();
        let stream = concat!(
            "\x07",
            "\x1b]0;build-pane\x07",
            "\x1b]133;D;42\x07",
            "\x1b[?2031h",
            "\x1b[?2031l",
            "\x1b]7501;state=working;fg=cargo\x1b\\",
            "Opened https://github.com/stablyai/orca/pull/918 and https://gitlab.com/team/app/-/merge_requests/77\n"
        );
        let facts = ex.feed(stream.as_bytes());
        assert!(facts.contains(&DaemonTransientFact::Bell));
        assert!(facts.contains(&DaemonTransientFact::TitleChanged {
            title: "build-pane".into()
        }));
        assert!(facts.contains(&DaemonTransientFact::CommandFinished {
            exit_code: Some(42)
        }));
        assert!(facts.contains(&DaemonTransientFact::Mode2031Subscribe));
        assert!(facts.contains(&DaemonTransientFact::Mode2031Unsubscribe));
        assert!(facts.contains(&DaemonTransientFact::ForegroundChanged {
            process: "cargo".into()
        }));
        assert!(facts.contains(&DaemonTransientFact::PrLink {
            url: "https://github.com/stablyai/orca/pull/918".into()
        }));
        assert!(facts.contains(&DaemonTransientFact::PrLink {
            url: "https://gitlab.com/team/app/-/merge_requests/77".into()
        }));
    }

    #[test]
    fn cold_park_fair_round_robin_ack_ewma_and_overflow_recovery_snapshot() {
        let cfg = MultiplexConfig {
            initial_credit_bytes: 128,
            ack_overflow_budget_bytes: 100,
            round_robin_quantum_bytes: 16,
            vt_cols: 40,
            vt_rows: 10,
            vt_scrollback: 20,
            link_ewma_alpha: 0.5,
        };
        let ctrl = PtyMultiplexController::in_memory("ws-1", "local", "pane-1", cfg);
        let _inc_a = ctrl.create_session("tab-a", "pane-1", "A", 1_000).expect("a");
        let _inc_b = ctrl.create_session("tab-b", "pane-1", "B", 1_000).expect("b");

        // Fair round-robin across A and B
        ctrl.ingest_pty_output("tab-a", b"AAAAAAAAAAAAAAAAAAAA")
            .expect("feed a");
        ctrl.ingest_pty_output("tab-b", b"BBBBBBBBBBBBBBBBBBBB")
            .expect("feed b");
        let frames = ctrl.drain_round_robin(32);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].tab_id, "tab-a");
        assert_eq!(frames[1].tab_id, "tab-b");
        assert_eq!(frames[0].kind, MuxFrameKind::OutputDelta);

        // ACK replenishes credit & updates EWMA link estimate
        let before_bps = ctrl.link_estimate_bps();
        let stats = ctrl.ack("tab-a", 16, 100).expect("ack a");
        assert_eq!(stats.unacked_bytes, 0);
        assert_ne!(stats.link_estimate_bps, before_bps);

        // Cold-park tab-b: output while hidden does NOT drain, unpark emits RecoverySnapshot
        ctrl.set_session_background("tab-b", true).expect("park b");
        ctrl.ingest_pty_output("tab-b", b"\r\nhidden-update")
            .expect("feed hidden");
        let drained_while_parked = ctrl.drain_round_robin(128);
        assert!(drained_while_parked.iter().all(|f| f.tab_id != "tab-b"));

        ctrl.set_session_background("tab-b", false).expect("unpark b");
        let unpark_frames = ctrl.drain_round_robin(256);
        assert_eq!(unpark_frames.len(), 1);
        assert_eq!(unpark_frames[0].tab_id, "tab-b");
        assert_eq!(unpark_frames[0].kind, MuxFrameKind::RecoverySnapshot);
        assert!(
            std::str::from_utf8(&unpark_frames[0].payload)
                .expect("utf8")
                .contains("hidden-update")
        );

        // Ack-pending-overflow on tab-a (> 100 bytes backlog) collapses into RecoverySnapshot
        let big = vec![b'X'; 150];
        ctrl.ingest_pty_output("tab-a", &big).expect("overflow a");
        let overflow_frames = ctrl.drain_round_robin(512);
        assert_eq!(overflow_frames.len(), 1);
        assert_eq!(overflow_frames[0].tab_id, "tab-a");
        assert_eq!(overflow_frames[0].kind, MuxFrameKind::RecoverySnapshot);
        assert!(overflow_frames[0].dropped_bytes >= 150);
    }

    #[test]
    fn incarnation_kill_guards_and_durable_30_day_tombstones() {
        let tmp = TempDir::new().expect("tempdir");
        let t0 = 1_700_000_000_000i64;
        let inc = {
            let ctrl = PtyMultiplexController::open(
                tmp.path(),
                "ws-1",
                "local",
                "pane-1",
                MultiplexConfig::default(),
            )
            .expect("open ctrl");
            let inc = ctrl
                .create_session("tab-term-1", "pane-1", "Shell", t0)
                .expect("create");
            ctrl.attach_session("tab-term-1", "client-1", t0)
                .expect("attach");
            ctrl.set_agent_owner("tab-term-1", Some("claude".into()))
                .expect("set owner");

            // Wrong incarnation -> conflict
            assert!(
                ctrl.kill_session(&KillSessionRequest {
                    tab_id: "tab-term-1".into(),
                    expected_incarnation_id: Some(PtyIncarnationId("wrong".into())),
                    require_no_agent_owner: false,
                    require_unattached: false,
                    now_ms: t0 + 10,
                })
                .is_err()
            );

            // Agent owner guard -> ActiveCliOwner
            let owner_err = ctrl
                .kill_session(&KillSessionRequest {
                    tab_id: "tab-term-1".into(),
                    expected_incarnation_id: Some(inc.clone()),
                    require_no_agent_owner: true,
                    require_unattached: false,
                    now_ms: t0 + 10,
                })
                .expect_err("agent owner guard");
            assert_eq!(owner_err.code_str(), "active_cli_owner");

            // Attached guard -> Conflict
            ctrl.set_agent_owner("tab-term-1", None).expect("clear owner");
            assert!(
                ctrl.kill_session(&KillSessionRequest {
                    tab_id: "tab-term-1".into(),
                    expected_incarnation_id: Some(inc.clone()),
                    require_no_agent_owner: true,
                    require_unattached: true,
                    now_ms: t0 + 20,
                })
                .is_err()
            );

            // Detach & kill succeeds, creating durable 30-day tombstone
            ctrl.detach_session("tab-term-1", "client-1").expect("detach");
            let tomb = ctrl
                .kill_session(&KillSessionRequest {
                    tab_id: "tab-term-1".into(),
                    expected_incarnation_id: Some(inc.clone()),
                    require_no_agent_owner: true,
                    require_unattached: true,
                    now_ms: t0 + 30,
                })
                .expect("kill");
            assert_eq!(tomb.tab_id, "tab-term-1");
            inc
        };
        let _ = inc;

        // Reopen from disk: tombstone persists and refuses stale create/attach with TabNotFound
        let reopened = PtyMultiplexController::open(
            tmp.path(),
            "ws-1",
            "local",
            "pane-1",
            MultiplexConfig::default(),
        )
        .expect("reopen ctrl");
        assert!(reopened.is_tab_tombstoned("tab-term-1", t0 + 1_000));
        let create_err = reopened
            .create_session("tab-term-1", "pane-1", "Shell", t0 + 1_000)
            .expect_err("tombstoned create refused");
        assert_eq!(create_err.ide_error_code(), Some(OrcaIdeErrorCode::TabNotFound));

        let attach_err = reopened
            .attach_session("tab-term-1", "client-2", t0 + 1_000)
            .expect_err("tombstoned attach refused");
        assert_eq!(attach_err.ide_error_code(), Some(OrcaIdeErrorCode::TabNotFound));

        // After 30 days, pruning clears tombstone and allows reuse
        let expired_now = tombstone_expires_at_ms(t0 + 30) + 1;
        assert_eq!(reopened.prune_expired_tombstones(expired_now).expect("prune"), 1);
        assert!(reopened.create_session("tab-term-1", "pane-1", "Shell", expired_now).is_ok());
    }
}
