//! Scoped `Cmd+J` Quick Switcher / Jump Palette (`F22`).
//!
//! Provides both:
//! - Headless-testable [`OrcaJumpPaletteState`] backed directly by
//!   `orca_core::workspace_layout::layout_keybindings::rank_quick_switcher_items`
//! - Standalone GPUI `0.2.2` `Entity<ScopedJumpPalette>` (`Render` + `Focusable`)
//!   mountable both inside `OrcaIdeWorkspace` and as a top-level `AppShell` overlay.

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use orca_core::workspace_layout::{
    AttentionPriority, QuickSwitcherItem, QuickSwitcherItemKind, QuickSwitcherMatch,
    SplitPaneNode, WorkspaceLayoutModel, rank_quick_switcher_items,
};
use orca_ide::{AgentRunState, TaskForge, TasksHubFilter};

use crate::components::ui::{delete_prev_word, pop_last_char, sanitize_paste_text};
use crate::i18n::tr_str;
use crate::theme::{IconName, TdesktopThemePalette, tg_icon};

use super::runtime::OrcaDesktopRuntime;

pub const JUMP_PALETTE_PAGE_SIZE: usize = 20;
pub const JUMP_PALETTE_RECENT_SHORTCUT_COUNT: usize = 6;

/// Temporary filter chip cycled via `Tab` inside the Scoped Jump Palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JumpPaletteFilterChip {
    All,
    Host(String),
    Repo(String),
    Kind(QuickSwitcherItemKind),
}

impl JumpPaletteFilterChip {
    pub fn label(&self) -> String {
        match self {
            Self::All => "All".to_string(),
            Self::Host(h) => format!("host:{h}"),
            Self::Repo(r) => format!("repo:{r}"),
            Self::Kind(k) => match k {
                QuickSwitcherItemKind::Worktree => "Worktrees".to_string(),
                QuickSwitcherItemKind::FolderWorkspace => "Folders".to_string(),
                QuickSwitcherItemKind::OpenTab => "Open Tabs".to_string(),
                QuickSwitcherItemKind::RecentChat => "Chats".to_string(),
                QuickSwitcherItemKind::PullRequest => "PRs (#)".to_string(),
                QuickSwitcherItemKind::MergeRequest => "MRs (!)".to_string(),
                QuickSwitcherItemKind::Bot => "Bots".to_string(),
                QuickSwitcherItemKind::Command => "Commands".to_string(),
            },
        }
    }

    pub fn matches_item(&self, item: &QuickSwitcherItem) -> bool {
        match self {
            Self::All => true,
            Self::Host(h) => item.host_id.as_deref() == Some(h.as_str()),
            Self::Repo(r) => item.repo_id.as_deref() == Some(r.as_str()),
            Self::Kind(k) => item.kind == *k,
        }
    }
}

/// Selection action emitted when the user activates an item or inline `"Create worktree"` fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JumpPaletteSelection {
    Item(QuickSwitcherItem),
    CreateWorktree { suggested_name: String },
}

/// Pure headless-testable state for `Cmd+J` Scoped Jump Palette.
#[derive(Debug, Clone)]
pub struct OrcaJumpPaletteState {
    pub is_open: bool,
    pub inherited_host_id: Option<String>,
    pub inherited_repo_id: Option<String>,
    pub available_chips: Vec<JumpPaletteFilterChip>,
    pub active_chip_index: usize,
    pub query: String,
    pub all_candidates: Vec<QuickSwitcherItem>,
    pub frozen_recents: Vec<QuickSwitcherItem>,
    pub visible_matches: Vec<QuickSwitcherMatch>,
    pub display_limit: usize,
    pub total_matched_count: usize,
    pub selected_index: usize,
    pub last_now_ms: i64,
}

impl Default for OrcaJumpPaletteState {
    fn default() -> Self {
        Self {
            is_open: false,
            inherited_host_id: None,
            inherited_repo_id: None,
            available_chips: vec![JumpPaletteFilterChip::All],
            active_chip_index: 0,
            query: String::new(),
            all_candidates: Vec::new(),
            frozen_recents: Vec::new(),
            visible_matches: Vec::new(),
            display_limit: JUMP_PALETTE_PAGE_SIZE,
            total_matched_count: 0,
            selected_index: 0,
            last_now_ms: 0,
        }
    }
}

impl OrcaJumpPaletteState {
    /// Opens the palette inheriting optional `(host_id, repo_id)` scope, builds temporary
    /// `Tab` filter chips, freezes top-6 recents (`Cmd+1..6`, `NeedsYou > Done > Working > Idle`,
    /// omitting idle current tab unless pinned), and computes initial ranked matches.
    pub fn open_with_scope(
        &mut self,
        host_id: Option<String>,
        repo_id: Option<String>,
        candidates: Vec<QuickSwitcherItem>,
        now_ms: i64,
    ) {
        self.is_open = true;
        self.inherited_host_id = host_id.filter(|s| !s.trim().is_empty());
        self.inherited_repo_id = repo_id.filter(|s| !s.trim().is_empty());
        self.query.clear();
        self.display_limit = JUMP_PALETTE_PAGE_SIZE;
        self.selected_index = 0;
        self.last_now_ms = now_ms;
        self.all_candidates = candidates;

        let mut chips = Vec::new();
        let mut hosts = BTreeSet::new();
        let mut repos = BTreeSet::new();
        for c in &self.all_candidates {
            if let Some(ref h) = c.host_id
                && !h.trim().is_empty()
            {
                hosts.insert(h.clone());
            }
            if let Some(ref r) = c.repo_id
                && !r.trim().is_empty()
            {
                repos.insert(r.clone());
            }
        }
        // Place inherited repo/host scope chips first (when present in candidates), followed immediately by All
        if let Some(ref r) = self.inherited_repo_id {
            let chip = JumpPaletteFilterChip::Repo(r.clone());
            if self.all_candidates.iter().any(|it| chip.matches_item(it)) {
                chips.push(chip);
                repos.remove(r);
            }
        }
        if let Some(ref h) = self.inherited_host_id {
            let chip = JumpPaletteFilterChip::Host(h.clone());
            if self.all_candidates.iter().any(|it| chip.matches_item(it)) {
                chips.push(chip);
                hosts.remove(h);
            }
        }
        chips.push(JumpPaletteFilterChip::All);
        for h in hosts {
            chips.push(JumpPaletteFilterChip::Host(h));
        }
        for r in repos {
            chips.push(JumpPaletteFilterChip::Repo(r));
        }
        for kind in [
            QuickSwitcherItemKind::Worktree,
            QuickSwitcherItemKind::OpenTab,
            QuickSwitcherItemKind::Bot,
            QuickSwitcherItemKind::PullRequest,
            QuickSwitcherItemKind::MergeRequest,
            QuickSwitcherItemKind::Command,
        ] {
            if self.all_candidates.iter().any(|it| it.kind == kind) {
                chips.push(JumpPaletteFilterChip::Kind(kind));
            }
        }
        self.available_chips = chips;
        self.active_chip_index = 0;

        // Freeze top-6 recents from initial empty-query ranking (Cmd+1..6).
        let scoped = self.scoped_candidates();
        self.frozen_recents =
            rank_quick_switcher_items("", &scoped, now_ms, JUMP_PALETTE_RECENT_SHORTCUT_COUNT)
                .into_iter()
                .map(|m| m.item)
                .collect();
        self.recompute_matches(now_ms);
    }

    pub fn close(&mut self) {
        self.is_open = false;
    }

    pub fn active_chip(&self) -> JumpPaletteFilterChip {
        self.available_chips
            .get(self.active_chip_index)
            .cloned()
            .unwrap_or(JumpPaletteFilterChip::All)
    }

    fn scoped_candidates(&self) -> Vec<QuickSwitcherItem> {
        let chip = self.active_chip();
        self.all_candidates
            .iter()
            .filter(|item| chip.matches_item(item))
            .cloned()
            .collect()
    }

    fn recompute_matches(&mut self, now_ms: i64) {
        self.last_now_ms = now_ms;
        let scoped = self.scoped_candidates();
        let trimmed = self.query.trim();

        // Support PR `#123` and MR `!123` shorthand query normalization cleanly:
        let filtered_for_forge: Vec<QuickSwitcherItem> = if let Some(num) = trimmed.strip_prefix('#')
            && !num.is_empty()
            && num.chars().all(|c| c.is_ascii_digit())
        {
            let matching_prs: Vec<QuickSwitcherItem> = scoped
                .iter()
                .filter(|it| it.kind == QuickSwitcherItemKind::PullRequest)
                .cloned()
                .collect();
            if matching_prs.is_empty() {
                scoped
            } else {
                matching_prs
            }
        } else if let Some(num) = trimmed.strip_prefix('!')
            && !num.is_empty()
            && num.chars().all(|c| c.is_ascii_digit())
        {
            let matching_mrs: Vec<QuickSwitcherItem> = scoped
                .iter()
                .filter(|it| it.kind == QuickSwitcherItemKind::MergeRequest)
                .cloned()
                .collect();
            if matching_mrs.is_empty() {
                scoped
            } else {
                matching_mrs
            }
        } else {
            scoped
        };

        let all_ranked = rank_quick_switcher_items(
            trimmed,
            &filtered_for_forge,
            now_ms,
            filtered_for_forge.len().max(1),
        );
        self.total_matched_count = all_ranked.len();
        self.visible_matches = all_ranked.into_iter().take(self.display_limit).collect();
        let total_rows = self.visible_matches.len() + 1; // +1 for inline "Create worktree" fallback
        if self.selected_index >= total_rows {
            self.selected_index = 0;
        }
    }

    /// Updates the search query and re-ranks candidates via `rank_quick_switcher_items`.
    pub fn set_query(&mut self, query: impl Into<String>, now_ms: i64) {
        self.query = query.into();
        self.display_limit = JUMP_PALETTE_PAGE_SIZE;
        self.selected_index = 0;
        self.recompute_matches(now_ms);
    }

    /// Cycles the active temporary filter chip (simulating `Tab` inside the palette).
    pub fn toggle_filter_chip_tab(&mut self, now_ms: i64) {
        if !self.available_chips.is_empty() {
            self.active_chip_index = (self.active_chip_index + 1) % self.available_chips.len();
        }
        self.display_limit = JUMP_PALETTE_PAGE_SIZE;
        self.selected_index = 0;
        self.recompute_matches(now_ms);
    }

    pub fn select_chip_index(&mut self, idx: usize, now_ms: i64) {
        if idx < self.available_chips.len() {
            self.active_chip_index = idx;
            self.display_limit = JUMP_PALETTE_PAGE_SIZE;
            self.selected_index = 0;
            self.recompute_matches(now_ms);
        }
    }

    /// Selects a frozen top-6 recent item addressable via `Cmd+1..6`.
    pub fn select_recent_shortcut(&self, digit_1_to_6: u8) -> Option<&QuickSwitcherItem> {
        if !(1..=6).contains(&digit_1_to_6) {
            return None;
        }
        self.frozen_recents.get((digit_1_to_6 - 1) as usize)
    }

    pub fn visible_matches(&self) -> &[QuickSwitcherMatch] {
        &self.visible_matches
    }

    pub fn remaining_match_count(&self) -> usize {
        self.total_matched_count
            .saturating_sub(self.visible_matches.len())
    }

    /// Expands pagination by `+20` (`"See more (+20)"`).
    pub fn see_more(&mut self, now_ms: i64) {
        self.display_limit = self.display_limit.saturating_add(JUMP_PALETTE_PAGE_SIZE);
        self.recompute_matches(now_ms);
    }

    pub fn move_selection(&mut self, delta: isize) {
        let total = self.visible_matches.len() + 1; // last row is inline "Create worktree"
        if total == 0 {
            self.selected_index = 0;
            return;
        }
        let next = (self.selected_index as isize + delta).rem_euclid(total as isize);
        self.selected_index = next as usize;
    }

    pub fn confirm_selected(&self) -> Option<JumpPaletteSelection> {
        if let Some(m) = self.visible_matches.get(self.selected_index) {
            Some(JumpPaletteSelection::Item(m.item.clone()))
        } else {
            let suggested = if self.query.trim().is_empty() {
                "new-worktree".to_string()
            } else {
                self.query.trim().to_string()
            };
            Some(JumpPaletteSelection::CreateWorktree {
                suggested_name: suggested,
            })
        }
    }

    /// Collects live candidates from `OrcaDesktopRuntime` (`WorktreeManager`, `BotController`,
    /// `ChatJournalStore`, `TasksHubStore`, `AgentStatusEngine`, active `WorkspaceLayoutModel`,
    /// and built-in IDE commands).
    pub fn collect_live_candidates(
        runtime: &OrcaDesktopRuntime,
        active_layout: Option<&WorkspaceLayoutModel>,
        now_ms: i64,
    ) -> Vec<QuickSwitcherItem> {
        let ide = runtime.ide();
        let host_id = ide.config().host_id.clone();
        let mut items = Vec::new();

        // 1. Live Git worktrees
        if let Ok(worktrees) = ide.worktrees().list_worktrees() {
            for wt in worktrees {
                items.push(QuickSwitcherItem {
                    id: format!("wt:{}", wt.id),
                    kind: QuickSwitcherItemKind::Worktree,
                    title: wt.title.clone(),
                    subtitle: format!("{} · {}", wt.branch_name, wt.repo_root),
                    host_id: Some(host_id.clone()),
                    repo_id: Some(wt.repo_root.clone()),
                    recency_ms: wt.updated_at_ms,
                    pinned: wt.pinned,
                    attention: AttentionPriority::Idle,
                    is_current_tab: false,
                });
            }
        }

        // 2. Live Folder workspaces
        if let Ok(folders) = ide.worktrees().list_folder_workspaces() {
            for fw in folders {
                items.push(QuickSwitcherItem {
                    id: format!("folder:{}", fw.id),
                    kind: QuickSwitcherItemKind::FolderWorkspace,
                    title: fw.name.clone(),
                    subtitle: fw.root_path.clone(),
                    host_id: Some(host_id.clone()),
                    repo_id: None,
                    recency_ms: fw.updated_at_ms,
                    pinned: fw.pinned,
                    attention: AttentionPriority::Idle,
                    is_current_tab: false,
                });
            }
        }

        // 3. Live Orca Bots
        let bots = runtime.block_on_local(runtime.bots().controller().list_bots());
        for bot in bots {
            items.push(QuickSwitcherItem {
                id: format!("bot:{}", bot.id),
                kind: QuickSwitcherItemKind::Bot,
                title: bot.name.as_str().to_string(),
                subtitle: format!("{} · {}", bot.agent.as_str(), bot.folder),
                host_id: Some(bot.host_id.clone()),
                repo_id: None,
                recency_ms: bot.updated_at,
                pinned: false,
                attention: AttentionPriority::Idle,
                is_current_tab: false,
            });
        }

        // 4. Open tabs from active WorkspaceLayoutModel
        let statuses = ide.agent_status().list_statuses();
        if let Some(layout) = active_layout {
            fn collect_tabs(
                node: &SplitPaneNode,
                focused_pane_id: &str,
                host_id: &str,
                out: &mut Vec<QuickSwitcherItem>,
            ) {
                match node {
                    SplitPaneNode::Leaf {
                        pane_id,
                        tabs,
                        active_tab_id,
                    } => {
                        for tab in tabs {
                            let is_current = pane_id == focused_pane_id
                                && active_tab_id.as_deref() == Some(tab.tab_id.as_str());
                            out.push(QuickSwitcherItem {
                                id: format!("tab:{}", tab.tab_id),
                                kind: QuickSwitcherItemKind::OpenTab,
                                title: tab.title.clone(),
                                subtitle: format!("{} · pane {}", tab.content_type.as_str(), pane_id),
                                host_id: Some(host_id.to_string()),
                                repo_id: None,
                                recency_ms: tab.last_active_at_ms,
                                pinned: tab.pinned,
                                attention: AttentionPriority::Idle,
                                is_current_tab: is_current,
                            });
                        }
                    }
                    SplitPaneNode::Split { first, second, .. } => {
                        collect_tabs(first, focused_pane_id, host_id, out);
                        collect_tabs(second, focused_pane_id, host_id, out);
                    }
                }
            }
            collect_tabs(&layout.root, &layout.focused_pane_id, &layout.host_id, &mut items);
        }

        // 5. Recent ChatJournal sessions (enriched with AgentRunState attention priority)
        if let Ok(sessions) = ide.chat_journal().list_sessions(None) {
            for sess in sessions {
                let attention = statuses
                    .iter()
                    .find(|s| s.session_id == sess.session_id)
                    .map(|s| match s.state {
                        AgentRunState::Blocked | AgentRunState::Waiting => {
                            AttentionPriority::NeedsYou
                        }
                        AgentRunState::Done => AttentionPriority::Done,
                        AgentRunState::Working => AttentionPriority::Working,
                        AgentRunState::Idle => AttentionPriority::Idle,
                    })
                    .unwrap_or(AttentionPriority::Idle);
                items.push(QuickSwitcherItem {
                    id: format!("chat:{}", sess.session_id),
                    kind: QuickSwitcherItemKind::RecentChat,
                    title: format!("{} ({})", sess.session_id, sess.agent_id),
                    subtitle: format!("ws:{} · {:?}", sess.workspace_id, sess.view_mode),
                    host_id: Some(sess.host_id.clone()),
                    repo_id: None,
                    recency_ms: sess.updated_at_ms,
                    pinned: false,
                    attention,
                    is_current_tab: false,
                });
            }
        }

        // 6. Forge PRs (`#123`) / MRs (`!123`) from TasksHubStore
        if let Ok(hub_tasks) = ide.tasks_hub().list_tasks(&TasksHubFilter::default()) {
            for task in hub_tasks {
                let (kind, prefix) = match task.forge {
                    TaskForge::GitLab => (QuickSwitcherItemKind::MergeRequest, "!"),
                    _ => (QuickSwitcherItemKind::PullRequest, "#"),
                };
                let normalized_ref = if task.external_key.starts_with(prefix) {
                    task.external_key.clone()
                } else {
                    format!("{prefix}{}", task.external_key)
                };
                items.push(QuickSwitcherItem {
                    id: format!("task:{}", task.task_id),
                    kind,
                    title: format!("{normalized_ref} {}", task.title),
                    subtitle: format!("{} · {}", task.forge.as_str(), task.repo_or_project),
                    host_id: Some(host_id.clone()),
                    repo_id: if task.repo_or_project.is_empty() {
                        None
                    } else {
                        Some(task.repo_or_project.clone())
                    },
                    recency_ms: task.updated_at_ms,
                    pinned: false,
                    attention: AttentionPriority::Idle,
                    is_current_tab: false,
                });
            }
        }

        // 7. Built-in IDE navigation commands
        for (cmd_id, title, subtitle) in [
            ("cmd:kanban", "Toggle Agent Kanban Board", "Cmd+Shift+B · Orchestration & 5-column Kanban"),
            ("cmd:flip", "Flip Chat <-> Terminal", "Cmd+Shift+J · Toggle active session view mode"),
            ("cmd:diff", "Open Annotate AI Diff & Tasks Hub", "Review annotations, attribution & PR draft"),
            ("cmd:usage", "Open 9-Provider Usage & Hot-Swap", "Quota roster sorted tightest-first"),
            ("cmd:vault", "Open AI Vault FTS5 Search", "Cross-agent transcript search & fork"),
            ("cmd:plugins", "Open Plugin System V2 & MCP Sync", "4-tier plugins, SecuritySurface approval & skills"),
            ("cmd:design", "Toggle Browser Design Mode", "Cmd+Shift+I · DOM/CSS/PNG/SourceMap capture"),
        ] {
            items.push(QuickSwitcherItem {
                id: cmd_id.to_string(),
                kind: QuickSwitcherItemKind::Command,
                title: title.to_string(),
                subtitle: subtitle.to_string(),
                host_id: Some(host_id.clone()),
                repo_id: None,
                recency_ms: now_ms.saturating_sub(600_000),
                pinned: false,
                attention: AttentionPriority::Idle,
                is_current_tab: false,
            });
        }

        items
    }
}

/// Standalone GPUI `0.2.2` `Entity<ScopedJumpPalette>` usable both inside `OrcaIdeWorkspace`
/// and as a top-level `AppShell` overlay (`Cmd+J`).
pub struct ScopedJumpPalette {
    runtime: Arc<OrcaDesktopRuntime>,
    state: OrcaJumpPaletteState,
    palette: TdesktopThemePalette,
    language_id: String,
    focus_handle: FocusHandle,
    list_scroll: ScrollHandle,
    selected_action: Option<JumpPaletteSelection>,
}

impl ScopedJumpPalette {
    pub fn new(
        runtime: Arc<OrcaDesktopRuntime>,
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            runtime,
            state: OrcaJumpPaletteState::default(),
            palette,
            language_id: language_id.into(),
            focus_handle: cx.focus_handle(),
            list_scroll: ScrollHandle::new(),
            selected_action: None,
        }
    }

    pub fn open_with_live_candidates(
        &mut self,
        host_id: Option<String>,
        repo_id: Option<String>,
        active_layout: Option<&WorkspaceLayoutModel>,
        cx: &mut Context<Self>,
    ) {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let candidates =
            OrcaJumpPaletteState::collect_live_candidates(&self.runtime, active_layout, now_ms);
        self.state.open_with_scope(host_id, repo_id, candidates, now_ms);
        self.selected_action = None;
        cx.notify();
    }

    pub fn state(&self) -> &OrcaJumpPaletteState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut OrcaJumpPaletteState {
        &mut self.state
    }

    pub fn set_theme_and_locale(
        &mut self,
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.palette = palette;
        self.language_id = language_id.into();
        cx.notify();
    }

    pub fn take_selected_action(&mut self) -> Option<JumpPaletteSelection> {
        self.selected_action.take()
    }

    pub fn handle_shortcut_digit(&mut self, digit_1_to_6: u8, cx: &mut Context<Self>) -> bool {
        if !self.state.is_open {
            return false;
        }
        if let Some(item) = self.state.select_recent_shortcut(digit_1_to_6).cloned() {
            self.selected_action = Some(JumpPaletteSelection::Item(item));
            self.state.close();
            cx.notify();
            return true;
        }
        false
    }

    pub fn handle_tab_chip(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.state.is_open {
            return false;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.state.toggle_filter_chip_tab(now_ms);
        cx.notify();
        true
    }

    pub fn insert_committed_text(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if !self.state.is_open {
            return false;
        }
        let clean = sanitize_paste_text(text, false);
        if clean.is_empty() {
            return false;
        }
        let mut q = self.state.query.clone();
        q.push_str(&clean);
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.state.set_query(q, now_ms);
        cx.notify();
        true
    }

    pub fn handle_key_down(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if !self.state.is_open {
            return false;
        }
        let key = ev.keystroke.key.as_str();
        let m = ev.keystroke.modifiers;
        let cmd = m.platform || m.control;
        let now_ms = chrono::Utc::now().timestamp_millis();

        if cmd && !m.shift && !m.alt {
            if let Some(digit) = key.chars().next().filter(|_| key.len() == 1)
                && ('1'..='6').contains(&digit)
            {
                let idx = (digit as u8) - b'0';
                return self.handle_shortcut_digit(idx, cx);
            }
            if key.eq_ignore_ascii_case("j") {
                self.state.close();
                cx.notify();
                return true;
            }
        }

        match key {
            "escape" => {
                self.state.close();
                cx.notify();
                true
            }
            "tab" => self.handle_tab_chip(cx),
            "up" | "arrowup" => {
                self.state.move_selection(-1);
                cx.notify();
                true
            }
            "down" | "arrowdown" => {
                self.state.move_selection(1);
                cx.notify();
                true
            }
            "enter" | "return" => {
                if let Some(sel) = self.state.confirm_selected() {
                    self.selected_action = Some(sel);
                    self.state.close();
                    cx.notify();
                }
                true
            }
            "backspace" => {
                let mut q = self.state.query.clone();
                if m.alt {
                    delete_prev_word(&mut q);
                } else {
                    pop_last_char(&mut q);
                }
                self.state.set_query(q, now_ms);
                cx.notify();
                true
            }
            _ => {
                if !cmd
                    && !m.alt
                    && let Some(ref kc) = ev.keystroke.key_char
                    && !kc.chars().any(char::is_control)
                {
                    let mut q = self.state.query.clone();
                    q.push_str(kc);
                    self.state.set_query(q, now_ms);
                    cx.notify();
                    return true;
                }
                false
            }
        }
    }
}

impl Focusable for ScopedJumpPalette {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn kind_badge(kind: QuickSwitcherItemKind) -> (&'static str, IconName) {
    match kind {
        QuickSwitcherItemKind::Worktree => ("WT", IconName::FolderWork),
        QuickSwitcherItemKind::FolderWorkspace => ("DIR", IconName::FileDoc),
        QuickSwitcherItemKind::OpenTab => ("TAB", IconName::TerminalMcp),
        QuickSwitcherItemKind::RecentChat => ("CHAT", IconName::CommunityThread),
        QuickSwitcherItemKind::PullRequest => ("PR", IconName::Forward),
        QuickSwitcherItemKind::MergeRequest => ("MR", IconName::Forward),
        QuickSwitcherItemKind::Bot => ("BOT", IconName::BotRobot),
        QuickSwitcherItemKind::Command => ("CMD", IconName::SparkleAlgo),
    }
}

fn attention_badge(priority: AttentionPriority) -> Option<(&'static str, u32)> {
    match priority {
        AttentionPriority::NeedsYou => Some(("NEEDS YOU", 0xF59E0B)),
        AttentionPriority::Done => Some(("DONE", 0x16A34A)),
        AttentionPriority::Working => Some(("WORKING", 0x3B82F6)),
        AttentionPriority::Idle => None,
    }
}

impl Render for ScopedJumpPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.state.is_open {
            return div().into_any_element();
        }
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let active_chip_idx = self.state.active_chip_index;
        let selected_idx = self.state.selected_index;
        let is_empty_query = self.state.query.trim().is_empty();
        let remaining = self.state.remaining_match_count();

        let query_display = if self.state.query.is_empty() {
            tr_str(
                lang,
                "搜索工作树、标签、Bot、PR #123、MR !123 或命令… (Tab 切换过滤芯片)",
                "Jump to worktree, tab, bot, PR #123, MR !123, or command… (Tab cycles scope chips)",
            )
            .to_string()
        } else {
            format!("{}|", self.state.query)
        };

        let mut chips_row = div().flex().flex_row().flex_wrap().gap(px(6.0));
        for (idx, chip) in self.state.available_chips.iter().enumerate() {
            let active = idx == active_chip_idx;
            chips_row = chips_row.child(
                div()
                    .id(("orca-jump-chip", idx))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(10.0))
                    .text_xs()
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .bg(if active {
                        rgb(pal.accent)
                    } else {
                        rgb(pal.input_bg)
                    })
                    .text_color(if active {
                        rgb(pal.active_button_fg)
                    } else {
                        rgb(pal.text_secondary)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            let now_ms = chrono::Utc::now().timestamp_millis();
                            this.state.select_chip_index(idx, now_ms);
                            cx.notify();
                        }),
                    )
                    .child(chip.label()),
            );
        }

        let mut rows_col = div().flex().flex_col().gap(px(4.0));
        for (idx, m) in self.state.visible_matches.iter().enumerate() {
            let item = &m.item;
            let is_selected = idx == selected_idx;
            let (k_label, icon_name) = kind_badge(item.kind);
            let shortcut_num = if is_empty_query {
                self.state
                    .frozen_recents
                    .iter()
                    .position(|r| r.id == item.id)
                    .map(|p| p + 1)
            } else {
                None
            };
            let item_clone = item.clone();

            rows_col = rows_col.child(
                div()
                    .id(SharedString::from(format!("orca-jump-row-{}-{}", idx, item.id)))
                    .cursor_pointer()
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(8.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .bg(if is_selected {
                        pal.row_active_rgba()
                    } else {
                        rgb(pal.card_bg)
                    })
                    .border_1()
                    .border_color(if is_selected {
                        rgb(pal.accent)
                    } else {
                        pal.hairline_border_rgba()
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            this.selected_action = Some(JumpPaletteSelection::Item(item_clone.clone()));
                            this.state.close();
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .min_w(px(0.0))
                            .child(tg_icon(icon_name, 14.0, pal.accent))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w(px(0.0))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(pal.text_primary))
                                            .child(item.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(pal.text_secondary))
                                            .child(item.subtitle.clone()),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .when_some(attention_badge(item.attention), |el, (lbl, col)| {
                                el.child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(6.0))
                                        .bg(rgb(col))
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(0xFFFFFF))
                                        .child(lbl),
                                )
                            })
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(pal.input_bg))
                                    .text_xs()
                                    .text_color(rgb(pal.text_secondary))
                                    .child(k_label),
                            )
                            .when_some(shortcut_num, |el, n| {
                                el.child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(pal.accent))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(pal.active_button_fg))
                                        .child(format!("Cmd+{n}")),
                                )
                            }),
                    ),
            );
        }

        // Inline "Create worktree" fallback row
        let fallback_idx = self.state.visible_matches.len();
        let fallback_selected = selected_idx == fallback_idx;
        let suggested_wt = if self.state.query.trim().is_empty() {
            "new-worktree".to_string()
        } else {
            self.state.query.trim().to_string()
        };
        let suggested_wt_click = suggested_wt.clone();
        rows_col = rows_col.child(
            div()
                .id("orca-jump-create-worktree-fallback")
                .cursor_pointer()
                .px(px(10.0))
                .py(px(8.0))
                .rounded(px(8.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .bg(if fallback_selected {
                    pal.row_active_rgba()
                } else {
                    rgb(pal.input_bg)
                })
                .border_1()
                .border_color(rgb(pal.accent))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, _w, cx| {
                        cx.stop_propagation();
                        this.selected_action = Some(JumpPaletteSelection::CreateWorktree {
                            suggested_name: suggested_wt_click.clone(),
                        });
                        this.state.close();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pal.accent))
                        .child(format!(
                            "+ {} \"{}\"",
                            tr_str(lang, "新建工作树", "Create worktree"),
                            suggested_wt
                        )),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_secondary))
                        .child("Enter"),
                ),
        );

        div()
            .id("orca-scoped-jump-palette-scrim")
            .track_focus(&self.focus_handle)
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(rgba(0x00000088))
            .flex()
            .items_start()
            .justify_center()
            .pt(px(56.0))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                if this.handle_key_down(ev, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev, _w, cx| {
                    this.state.close();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("orca-scoped-jump-palette-card")
                    .w(px(640.0))
                    .max_h(px(540.0))
                    .rounded(px(12.0))
                    .bg(rgb(pal.modal_bg))
                    .border_1()
                    .border_color(pal.separator_rgba())
                    .shadow(pal.popover_shadows())
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .on_mouse_down(MouseButton::Left, |_ev, _w, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(tg_icon(IconName::Search, 15.0, pal.accent))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(pal.text_primary))
                                            .child(tr_str(
                                                lang,
                                                "作用域快速跳转 (Cmd+J)",
                                                "Scoped Jump Palette (Cmd+J)",
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(pal.text_secondary))
                                    .child("Cmd+1..6 · Tab · Esc"),
                            ),
                    )
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(8.0))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(rgb(pal.accent))
                            .text_sm()
                            .text_color(if self.state.query.is_empty() {
                                rgb(pal.text_secondary)
                            } else {
                                rgb(pal.text_primary)
                            })
                            .child(query_display),
                    )
                    .child(chips_row)
                    .child(
                        div()
                            .id("orca-jump-matches-scroll")
                            .flex_1()
                            .min_h(px(0.0))
                            .max_h(px(340.0))
                            .overflow_y_scroll()
                            .track_scroll(&self.list_scroll)
                            .child(rows_col),
                    )
                    .when(remaining > 0, |el| {
                        el.child(
                            div()
                                .id("orca-jump-see-more")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(rgb(pal.input_bg))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(pal.accent))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        let now_ms = chrono::Utc::now().timestamp_millis();
                                        this.state.see_more(now_ms);
                                        cx.notify();
                                    }),
                                )
                                .child(format!("See more (+{JUMP_PALETTE_PAGE_SIZE}) · {remaining} remaining")),
                        )
                    }),
            )
            .into_any_element()
    }
}
