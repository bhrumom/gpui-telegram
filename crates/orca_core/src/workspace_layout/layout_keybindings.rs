//! Pure-Rust workspace layout tree (`SplitPaneNode`, 10-variant `TabContentType`,
//! 30-day closed-tab tombstones), 103-action keybinding table with OS/system-shortcut
//! blocklist, and `Cmd+J` Unicode-safe quick switcher ranking.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{CLOSED_TAB_TOMBSTONE_DAYS, OrcaIdeError, OrcaIdeErrorCode};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const SPLIT_RATIO_MIN_BP: u16 = 1_000;
pub const SPLIT_RATIO_MAX_BP: u16 = 9_000;
pub const SPLIT_RATIO_DEFAULT_BP: u16 = 5_000;
pub const WORKSPACE_LAYOUT_MAX_SPLIT_DEPTH: usize = 16;
pub const CLOSED_TAB_TOMBSTONE_MAX_PER_PARTITION: usize = 500;
pub const CLOSED_TAB_TOMBSTONE_MS: i64 = (CLOSED_TAB_TOMBSTONE_DAYS as i64) * 86_400_000;
pub const KEYBINDING_ACTION_COUNT: usize = 103;

// ---------------------------------------------------------------------------
// 10-Variant TabContentType (Exact 1:1 with `TAB_CONTENT_TYPES_10`)
// ---------------------------------------------------------------------------

/// Exhaustive 10-variant tab content taxonomy matching `TAB_CONTENT_TYPES_10`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TabContentType {
    Terminal,
    Editor,
    Diff,
    ConflictReview,
    CheckDetails,
    GitGraph,
    AgentSession,
    Browser,
    Simulator,
    Room,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabContentTraits {
    pub supports_dual_view: bool,
    pub supports_hibernation: bool,
    pub requires_pty_tombstone: bool,
    pub can_split_nested: bool,
    pub label: &'static str,
}

impl TabContentType {
    pub const ALL: [Self; 10] = [
        Self::Terminal,
        Self::Editor,
        Self::Diff,
        Self::ConflictReview,
        Self::CheckDetails,
        Self::GitGraph,
        Self::AgentSession,
        Self::Browser,
        Self::Simulator,
        Self::Room,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Editor => "editor",
            Self::Diff => "diff",
            Self::ConflictReview => "conflict-review",
            Self::CheckDetails => "check-details",
            Self::GitGraph => "git-graph",
            Self::AgentSession => "agent-session",
            Self::Browser => "browser",
            Self::Simulator => "simulator",
            Self::Room => "room",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == raw)
    }

    pub const fn traits(self) -> TabContentTraits {
        match self {
            Self::Terminal => TabContentTraits {
                supports_dual_view: true,
                supports_hibernation: true,
                requires_pty_tombstone: true,
                can_split_nested: true,
                label: "Terminal",
            },
            Self::Editor => TabContentTraits {
                supports_dual_view: false,
                supports_hibernation: false,
                requires_pty_tombstone: false,
                can_split_nested: false,
                label: "Editor",
            },
            Self::Diff => TabContentTraits {
                supports_dual_view: false,
                supports_hibernation: false,
                requires_pty_tombstone: false,
                can_split_nested: false,
                label: "Diff",
            },
            Self::ConflictReview => TabContentTraits {
                supports_dual_view: false,
                supports_hibernation: false,
                requires_pty_tombstone: false,
                can_split_nested: false,
                label: "Conflict Review",
            },
            Self::CheckDetails => TabContentTraits {
                supports_dual_view: false,
                supports_hibernation: false,
                requires_pty_tombstone: false,
                can_split_nested: false,
                label: "Check Details",
            },
            Self::GitGraph => TabContentTraits {
                supports_dual_view: false,
                supports_hibernation: false,
                requires_pty_tombstone: false,
                can_split_nested: false,
                label: "Git Graph",
            },
            Self::AgentSession => TabContentTraits {
                supports_dual_view: true,
                supports_hibernation: true,
                requires_pty_tombstone: true,
                can_split_nested: false,
                label: "Agent Session",
            },
            Self::Browser => TabContentTraits {
                supports_dual_view: false,
                supports_hibernation: false,
                requires_pty_tombstone: false,
                can_split_nested: false,
                label: "Browser",
            },
            Self::Simulator => TabContentTraits {
                supports_dual_view: false,
                supports_hibernation: false,
                requires_pty_tombstone: false,
                can_split_nested: false,
                label: "Simulator",
            },
            Self::Room => TabContentTraits {
                supports_dual_view: false,
                supports_hibernation: false,
                requires_pty_tombstone: false,
                can_split_nested: false,
                label: "Room",
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Workspace Layout Model & Recursive SplitPaneNode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabViewMode {
    #[default]
    Terminal,
    Chat,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TabLeafState {
    #[default]
    Live,
    Sleeping {
        hibernated_at_ms: i64,
        reason: String,
    },
    Resume {
        session_handle: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTab {
    pub tab_id: String,
    pub content_type: TabContentType,
    pub title: String,
    pub pinned: bool,
    pub preview: bool,
    pub view_mode: TabViewMode,
    pub leaf_state: TabLeafState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_incarnation_id: Option<String>,
    pub dirty: bool,
    pub created_at_ms: i64,
    pub last_active_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SplitPaneNode {
    Leaf {
        pane_id: String,
        tabs: Vec<WorkspaceTab>,
        active_tab_id: Option<String>,
    },
    Split {
        direction: SplitDirection,
        ratio_bp: u16,
        pinned_boundary: bool,
        first: Box<SplitPaneNode>,
        second: Box<SplitPaneNode>,
    },
}

/// Clamps split ratio in basis points to `1000..=9000` (10%..=90%).
pub const fn clamp_split_ratio_bp(ratio_bp: u16) -> u16 {
    if ratio_bp < SPLIT_RATIO_MIN_BP {
        SPLIT_RATIO_MIN_BP
    } else if ratio_bp > SPLIT_RATIO_MAX_BP {
        SPLIT_RATIO_MAX_BP
    } else {
        ratio_bp
    }
}

impl SplitPaneNode {
    pub fn max_depth(&self) -> usize {
        match self {
            Self::Leaf { .. } => 0,
            Self::Split { first, second, .. } => 1 + first.max_depth().max(second.max_depth()),
        }
    }

    pub fn pane_count(&self) -> usize {
        match self {
            Self::Leaf { .. } => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }

    pub fn tab_count(&self) -> usize {
        match self {
            Self::Leaf { tabs, .. } => tabs.len(),
            Self::Split { first, second, .. } => first.tab_count() + second.tab_count(),
        }
    }

    fn find_pane_mut(&mut self, pane_id: &str) -> Option<(&mut Vec<WorkspaceTab>, &mut Option<String>)> {
        match self {
            Self::Leaf {
                pane_id: id,
                tabs,
                active_tab_id,
            } if id == pane_id => Some((tabs, active_tab_id)),
            Self::Leaf { .. } => None,
            Self::Split { first, second, .. } => first
                .find_pane_mut(pane_id)
                .or_else(|| second.find_pane_mut(pane_id)),
        }
    }

    fn find_tab(&self, tab_id: &str) -> Option<&WorkspaceTab> {
        match self {
            Self::Leaf { tabs, .. } => tabs.iter().find(|t| t.tab_id == tab_id),
            Self::Split { first, second, .. } => {
                first.find_tab(tab_id).or_else(|| second.find_tab(tab_id))
            }
        }
    }

    fn find_tab_mut(&mut self, tab_id: &str) -> Option<(String, &mut WorkspaceTab)> {
        match self {
            Self::Leaf { pane_id, tabs, .. } => tabs
                .iter_mut()
                .find(|t| t.tab_id == tab_id)
                .map(|t| (pane_id.clone(), t)),
            Self::Split { first, second, .. } => first
                .find_tab_mut(tab_id)
                .or_else(|| second.find_tab_mut(tab_id)),
        }
    }

    fn remove_tab(&mut self, tab_id: &str) -> Option<(String, WorkspaceTab)> {
        match self {
            Self::Leaf {
                pane_id,
                tabs,
                active_tab_id,
            } => {
                let idx = tabs.iter().position(|t| t.tab_id == tab_id)?;
                let removed = tabs.remove(idx);
                if active_tab_id.as_deref() == Some(tab_id) {
                    *active_tab_id = tabs
                        .get(idx)
                        .or_else(|| tabs.last())
                        .map(|t| t.tab_id.clone());
                }
                Some((pane_id.clone(), removed))
            }
            Self::Split { first, second, .. } => first
                .remove_tab(tab_id)
                .or_else(|| second.remove_tab(tab_id)),
        }
    }

    fn focus_tab_in_tree(&mut self, tab_id: &str, now_ms: i64) -> Option<String> {
        match self {
            Self::Leaf {
                pane_id,
                tabs,
                active_tab_id,
            } => {
                let tab = tabs.iter_mut().find(|t| t.tab_id == tab_id)?;
                tab.last_active_at_ms = now_ms;
                *active_tab_id = Some(tab_id.to_string());
                Some(pane_id.clone())
            }
            Self::Split { first, second, .. } => first
                .focus_tab_in_tree(tab_id, now_ms)
                .or_else(|| second.focus_tab_in_tree(tab_id, now_ms)),
        }
    }

    fn split_leaf(
        &mut self,
        target_pane_id: &str,
        new_pane_id: String,
        direction: SplitDirection,
        ratio_bp: u16,
        initial_tab: Option<WorkspaceTab>,
        current_depth: usize,
    ) -> Result<bool, OrcaIdeError> {
        match self {
            Self::Leaf { pane_id, .. } if pane_id == target_pane_id => {
                if current_depth >= WORKSPACE_LAYOUT_MAX_SPLIT_DEPTH {
                    return Err(OrcaIdeError::new(
                        OrcaIdeErrorCode::TabNotFound,
                        format!("maximum split depth ({WORKSPACE_LAYOUT_MAX_SPLIT_DEPTH}) reached"),
                    ));
                }
                let placeholder = Self::Leaf {
                    pane_id: String::new(),
                    tabs: Vec::new(),
                    active_tab_id: None,
                };
                let existing = std::mem::replace(self, placeholder);
                let active = initial_tab.as_ref().map(|t| t.tab_id.clone());
                let new_tabs = initial_tab.into_iter().collect();
                *self = Self::Split {
                    direction,
                    ratio_bp: clamp_split_ratio_bp(ratio_bp),
                    pinned_boundary: false,
                    first: Box::new(existing),
                    second: Box::new(Self::Leaf {
                        pane_id: new_pane_id,
                        tabs: new_tabs,
                        active_tab_id: active,
                    }),
                };
                Ok(true)
            }
            Self::Leaf { .. } => Ok(false),
            Self::Split { first, second, .. } => {
                if first.split_leaf(
                    target_pane_id,
                    new_pane_id.clone(),
                    direction,
                    ratio_bp,
                    initial_tab.clone(),
                    current_depth + 1,
                )? {
                    return Ok(true);
                }
                second.split_leaf(
                    target_pane_id,
                    new_pane_id,
                    direction,
                    ratio_bp,
                    initial_tab,
                    current_depth + 1,
                )
            }
        }
    }

    fn rebalance_unpinned(&mut self) {
        if let Self::Split {
            ratio_bp,
            pinned_boundary,
            first,
            second,
            ..
        } = self
        {
            if !*pinned_boundary {
                *ratio_bp = SPLIT_RATIO_DEFAULT_BP;
            }
            first.rebalance_unpinned();
            second.rebalance_unpinned();
        }
    }
}

/// 30-day closed-tab tombstone preventing stale reconnect/create replays from
/// resurrecting closed terminals and enabling `Reopen Closed Tab`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedTabTombstone {
    pub tab_id: String,
    pub pane_id: String,
    pub tab: WorkspaceTab,
    pub closed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceLayoutModel {
    pub workspace_id: String,
    pub host_id: String,
    pub root: SplitPaneNode,
    pub focused_pane_id: String,
    pub closed_tombstones: Vec<ClosedTabTombstone>,
    pub revision: u64,
}

impl WorkspaceLayoutModel {
    pub fn new(
        workspace_id: impl Into<String>,
        host_id: impl Into<String>,
        initial_pane_id: impl Into<String>,
    ) -> Self {
        let pane_id = initial_pane_id.into();
        Self {
            workspace_id: workspace_id.into(),
            host_id: host_id.into(),
            root: SplitPaneNode::Leaf {
                pane_id: pane_id.clone(),
                tabs: Vec::new(),
                active_tab_id: None,
            },
            focused_pane_id: pane_id,
            closed_tombstones: Vec::new(),
            revision: 0,
        }
    }

    pub fn prune_expired_tombstones(&mut self, now_ms: i64) -> usize {
        let before = self.closed_tombstones.len();
        self.closed_tombstones
            .retain(|t| now_ms.saturating_sub(t.closed_at_ms) < CLOSED_TAB_TOMBSTONE_MS);
        before - self.closed_tombstones.len()
    }

    pub fn is_tab_tombstoned(&self, tab_id: &str, now_ms: i64) -> bool {
        self.closed_tombstones.iter().any(|t| {
            t.tab_id == tab_id && now_ms.saturating_sub(t.closed_at_ms) < CLOSED_TAB_TOMBSTONE_MS
        })
    }

    /// Opens a tab in `pane_id`. Refuses stale replay of an unexpired closed-tab
    /// tombstone with `OrcaIdeErrorCode::TabNotFound`.
    pub fn open_tab(
        &mut self,
        pane_id: &str,
        tab: WorkspaceTab,
        now_ms: i64,
    ) -> Result<(), OrcaIdeError> {
        self.prune_expired_tombstones(now_ms);
        if self.is_tab_tombstoned(&tab.tab_id, now_ms) {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::TabNotFound,
                format!(
                    "tab '{}' is tombstoned (closed within {CLOSED_TAB_TOMBSTONE_DAYS} days); refusing stale create replay",
                    tab.tab_id
                ),
            ));
        }
        let (tabs, active_tab_id) = self.root.find_pane_mut(pane_id).ok_or_else(|| {
            OrcaIdeError::new(
                OrcaIdeErrorCode::TabNotFound,
                format!("pane '{pane_id}' not found"),
            )
        })?;
        let new_id = tab.tab_id.clone();
        tabs.push(tab);
        *active_tab_id = Some(new_id);
        self.focused_pane_id = pane_id.to_string();
        self.revision += 1;
        Ok(())
    }

    pub fn close_tab(
        &mut self,
        tab_id: &str,
        now_ms: i64,
    ) -> Result<ClosedTabTombstone, OrcaIdeError> {
        self.prune_expired_tombstones(now_ms);
        let (pane_id, tab) = self.root.remove_tab(tab_id).ok_or_else(|| {
            OrcaIdeError::new(
                OrcaIdeErrorCode::TabNotFound,
                format!("tab '{tab_id}' not found"),
            )
        })?;
        let tombstone = ClosedTabTombstone {
            tab_id: tab.tab_id.clone(),
            pane_id,
            tab,
            closed_at_ms: now_ms,
        };
        self.closed_tombstones.push(tombstone.clone());
        if self.closed_tombstones.len() > CLOSED_TAB_TOMBSTONE_MAX_PER_PARTITION {
            let overflow = self.closed_tombstones.len() - CLOSED_TAB_TOMBSTONE_MAX_PER_PARTITION;
            self.closed_tombstones.drain(0..overflow);
        }
        self.revision += 1;
        Ok(tombstone)
    }

    pub fn reopen_closed_tab(
        &mut self,
        tab_id: Option<&str>,
        now_ms: i64,
    ) -> Result<WorkspaceTab, OrcaIdeError> {
        self.prune_expired_tombstones(now_ms);
        let idx = match tab_id {
            Some(id) => self
                .closed_tombstones
                .iter()
                .rposition(|t| t.tab_id == id)
                .ok_or_else(|| {
                    OrcaIdeError::new(
                        OrcaIdeErrorCode::TabNotFound,
                        format!("closed tab '{id}' not found or expired"),
                    )
                })?,
            None => self
                .closed_tombstones
                .len()
                .checked_sub(1)
                .ok_or_else(|| {
                    OrcaIdeError::new(
                        OrcaIdeErrorCode::TabNotFound,
                        "no closed tabs available to reopen",
                    )
                })?,
        };
        let tomb = self.closed_tombstones.remove(idx);
        let target_pane = if self.root.find_pane_mut(&tomb.pane_id).is_some() {
            tomb.pane_id.clone()
        } else {
            self.focused_pane_id.clone()
        };
        let (tabs, active_tab_id) = self.root.find_pane_mut(&target_pane).ok_or_else(|| {
            OrcaIdeError::new(
                OrcaIdeErrorCode::TabNotFound,
                format!("target pane '{target_pane}' not found"),
            )
        })?;
        let restored = tomb.tab;
        *active_tab_id = Some(restored.tab_id.clone());
        tabs.push(restored.clone());
        self.focused_pane_id = target_pane;
        self.revision += 1;
        Ok(restored)
    }

    pub fn focus_tab(&mut self, tab_id: &str, now_ms: i64) -> Result<(), OrcaIdeError> {
        let pane_id = self
            .root
            .focus_tab_in_tree(tab_id, now_ms)
            .ok_or_else(|| {
                OrcaIdeError::new(
                    OrcaIdeErrorCode::TabNotFound,
                    format!("tab '{tab_id}' not found"),
                )
            })?;
        self.focused_pane_id = pane_id;
        self.revision += 1;
        Ok(())
    }

    pub fn split_pane(
        &mut self,
        pane_id: &str,
        new_pane_id: impl Into<String>,
        direction: SplitDirection,
        ratio_bp: u16,
        initial_tab: Option<WorkspaceTab>,
    ) -> Result<(), OrcaIdeError> {
        let new_id = new_pane_id.into();
        let found = self.root.split_leaf(
            pane_id,
            new_id.clone(),
            direction,
            ratio_bp,
            initial_tab,
            0,
        )?;
        if !found {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::TabNotFound,
                format!("pane '{pane_id}' not found for split"),
            ));
        }
        self.focused_pane_id = new_id;
        self.revision += 1;
        Ok(())
    }

    pub fn move_tab(
        &mut self,
        tab_id: &str,
        target_pane_id: &str,
        target_index: Option<usize>,
    ) -> Result<(), OrcaIdeError> {
        if self.root.find_pane_mut(target_pane_id).is_none() {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::TabNotFound,
                format!("target pane '{target_pane_id}' not found"),
            ));
        }
        let (_, tab) = self.root.remove_tab(tab_id).ok_or_else(|| {
            OrcaIdeError::new(
                OrcaIdeErrorCode::TabNotFound,
                format!("tab '{tab_id}' not found"),
            )
        })?;
        let (tabs, active_tab_id) = self.root.find_pane_mut(target_pane_id).ok_or_else(|| {
            OrcaIdeError::new(
                OrcaIdeErrorCode::TabNotFound,
                format!("target pane '{target_pane_id}' not found"),
            )
        })?;
        let insert_at = target_index.map_or(tabs.len(), |idx| idx.min(tabs.len()));
        *active_tab_id = Some(tab.tab_id.clone());
        tabs.insert(insert_at, tab);
        self.focused_pane_id = target_pane_id.to_string();
        self.revision += 1;
        Ok(())
    }

    pub fn set_split_ratio(
        &mut self,
        branch_path: &[bool],
        ratio_bp: u16,
        pinned_boundary: Option<bool>,
    ) -> Result<(), OrcaIdeError> {
        let mut cursor = &mut self.root;
        for &go_second in branch_path {
            cursor = match cursor {
                SplitPaneNode::Split { first, second, .. } => {
                    if go_second {
                        second.as_mut()
                    } else {
                        first.as_mut()
                    }
                }
                SplitPaneNode::Leaf { .. } => {
                    return Err(OrcaIdeError::new(
                        OrcaIdeErrorCode::TabNotFound,
                        "split path traversed past leaf pane",
                    ));
                }
            };
        }
        match cursor {
            SplitPaneNode::Split {
                ratio_bp: r,
                pinned_boundary: p,
                ..
            } => {
                *r = clamp_split_ratio_bp(ratio_bp);
                if let Some(pin) = pinned_boundary {
                    *p = pin;
                }
                self.revision += 1;
                Ok(())
            }
            SplitPaneNode::Leaf { .. } => Err(OrcaIdeError::new(
                OrcaIdeErrorCode::TabNotFound,
                "split path resolved to leaf pane instead of split node",
            )),
        }
    }

    pub fn rebalance_splits(&mut self) {
        self.root.rebalance_unpinned();
        self.revision += 1;
    }

    pub fn toggle_tab_view_mode(&mut self, tab_id: &str) -> Result<TabViewMode, OrcaIdeError> {
        let (_, tab) = self.root.find_tab_mut(tab_id).ok_or_else(|| {
            OrcaIdeError::new(
                OrcaIdeErrorCode::TabNotFound,
                format!("tab '{tab_id}' not found"),
            )
        })?;
        tab.view_mode = match tab.view_mode {
            TabViewMode::Terminal => TabViewMode::Chat,
            TabViewMode::Chat => TabViewMode::Terminal,
        };
        let mode = tab.view_mode;
        self.revision += 1;
        Ok(mode)
    }

    pub fn find_tab(&self, tab_id: &str) -> Option<&WorkspaceTab> {
        self.root.find_tab(tab_id)
    }

    pub fn tab_count(&self) -> usize {
        self.root.tab_count()
    }

    pub fn pane_count(&self) -> usize {
        self.root.pane_count()
    }

    pub fn max_depth(&self) -> usize {
        self.root.max_depth()
    }
}

// ---------------------------------------------------------------------------
// 103-Action Keybinding Table & OS/System-Shortcut Blocklist
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeybindingCategory {
    Workspace,
    Tabs,
    Panes,
    Terminal,
    EditorAndDiff,
    AgentAndOrchestration,
    Bots,
    SearchAndSwitcher,
    ReviewAndDesign,
    WindowAndApp,
}

/// All 103 distinct Orca IDE & Bot keybinding actions across 10 categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeybindingAction {
    // Workspace (12)
    WorkspaceNewWorktree,
    WorkspaceNewFolder,
    WorkspaceSwitchNext,
    WorkspaceSwitchPrev,
    WorkspaceHistoryBack,
    WorkspaceHistoryForward,
    WorkspaceRename,
    WorkspaceArchive,
    WorkspaceDelete,
    WorkspaceOpenCheckpoint,
    WorkspaceToggleFloating,
    WorkspaceOpenSettings,
    // Tabs (14)
    TabNew,
    TabClose,
    TabCloseOthers,
    TabCloseRight,
    TabCloseAll,
    TabReopenClosed,
    TabNext,
    TabPrev,
    TabMruNext,
    TabMruPrev,
    TabTogglePin,
    TabDuplicate,
    TabMoveLeft,
    TabMoveRight,
    // Panes (12)
    PaneSplitRight,
    PaneSplitDown,
    PaneClose,
    PaneFocusLeft,
    PaneFocusRight,
    PaneFocusUp,
    PaneFocusDown,
    PaneSwapNext,
    PaneRebalance,
    PaneToggleZoom,
    PanePinBoundary,
    PaneMoveTabToNext,
    // Terminal (12)
    TerminalNew,
    TerminalClear,
    TerminalSplitHorizontal,
    TerminalSplitVertical,
    TerminalFind,
    TerminalCopy,
    TerminalPaste,
    TerminalSendInterrupt,
    TerminalQuickCommands,
    TerminalScrollToBottom,
    TerminalToggleWordWrap,
    TerminalReset,
    // Editor & Diff (12)
    EditorSave,
    EditorSaveAll,
    EditorFormat,
    EditorFindInFile,
    EditorReplaceInFile,
    EditorGoToLine,
    EditorToggleMinimap,
    EditorToggleBlame,
    DiffNextHunk,
    DiffPrevHunk,
    DiffToggleSideBySide,
    DiffStageHunk,
    // Agent & Orchestration (11)
    AgentFlipChatTerminal,
    AgentNewChat,
    AgentStopTurn,
    AgentApproveAction,
    AgentRejectAction,
    AgentToggleKanban,
    AgentPopoutKanban,
    OrchestrationNewRun,
    OrchestrationDispatchWorker,
    OrchestrationOpenMailbox,
    OrchestrationInspectGates,
    // Bots (10)
    BotOpenSidebar,
    BotNew,
    BotOpenNeedsYou,
    BotOpenUpNext,
    BotOpenGroupRoom,
    BotHandOverWorkspace,
    BotRunLookAround,
    BotRunTidyUp,
    BotOpenTemplates,
    BotTogglePause,
    // Search & Switcher (10)
    SwitcherQuickJump,
    SwitcherQuickOpenFile,
    SwitcherCommandPalette,
    SwitcherGlobalSearch,
    SwitcherJumpRecent1,
    SwitcherJumpRecent2,
    SwitcherJumpRecent3,
    SwitcherJumpRecent4,
    SwitcherJumpRecent5,
    SwitcherJumpRecent6,
    // Review & Design (6)
    ReviewAddDiffAnnotation,
    ReviewSendAnnotationsToAgent,
    ReviewResolveAnnotation,
    ReviewToggleAiAttribution,
    DesignToggleBrowserMode,
    DesignCaptureElement,
    // Window & App (4)
    WindowToggleSidebar,
    WindowToggleStatusBar,
    WindowZoomIn,
    WindowZoomOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct KeyModifiers {
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyChord {
    pub modifiers: KeyModifiers,
    pub key: String,
}

impl KeyChord {
    pub fn new(modifiers: KeyModifiers, key: impl Into<String>) -> Self {
        Self {
            modifiers,
            key: normalize_key_name(&key.into()),
        }
    }

    /// Parses a chord string such as `"Cmd+Shift+J"`, `"Ctrl+Alt+Del"`, `"Option+Esc"`, `"Super+L"`.
    pub fn parse(raw: &str) -> Option<Self> {
        let mut modifiers = KeyModifiers::default();
        let mut key_part: Option<String> = None;

        for token in raw.split('+').map(str::trim).filter(|t| !t.is_empty()) {
            match token.to_ascii_lowercase().as_str() {
                "cmd" | "command" | "mod" | "meta" => modifiers.cmd = true,
                "ctrl" | "control" => modifiers.ctrl = true,
                "alt" | "opt" | "option" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                "super" | "win" | "windows" => modifiers.super_key = true,
                _ => {
                    if key_part.is_some() {
                        return None;
                    }
                    key_part = Some(normalize_key_name(token));
                }
            }
        }
        let key = key_part?;
        if key.is_empty() {
            return None;
        }
        Some(Self { modifiers, key })
    }

    pub fn canonical_string(&self) -> String {
        let mut parts = Vec::with_capacity(6);
        if self.modifiers.ctrl {
            parts.push("Ctrl");
        }
        if self.modifiers.alt {
            parts.push("Alt");
        }
        if self.modifiers.shift {
            parts.push("Shift");
        }
        if self.modifiers.cmd {
            parts.push("Cmd");
        }
        if self.modifiers.super_key {
            parts.push("Super");
        }
        let norm_key = normalize_key_name(&self.key);
        parts.push(norm_key.as_str());
        parts.join("+")
    }
}

fn normalize_key_name(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "esc" | "escape" => "Escape".to_string(),
        "del" | "delete" => "Delete".to_string(),
        "ret" | "return" | "enter" => "Enter".to_string(),
        "space" | "spacebar" => "Space".to_string(),
        "tab" => "Tab".to_string(),
        "left" | "arrowleft" => "ArrowLeft".to_string(),
        "right" | "arrowright" => "ArrowRight".to_string(),
        "up" | "arrowup" => "ArrowUp".to_string(),
        "down" | "arrowdown" => "ArrowDown".to_string(),
        "backspace" => "Backspace".to_string(),
        "minus" => "-".to_string(),
        "equal" | "plus" => "=".to_string(),
        other => {
            if other.len() == 1
                || (other.starts_with('f') && other[1..].chars().all(|c| c.is_ascii_digit()))
            {
                other.to_ascii_uppercase()
            } else {
                raw.trim().to_string()
            }
        }
    }
}

/// Returns `true` if `chord` collides with a protected OS/system shortcut that
/// user keybinding overrides and unapproved ComputerUse hotkeys MUST NOT bind.
pub fn is_system_shortcut_blocked(chord: &KeyChord) -> bool {
    let m = chord.modifiers;
    let key = normalize_key_name(&chord.key);
    let k = key.as_str();

    // macOS reserved app/OS chords: Cmd+Q, Cmd+H, Cmd+M, Cmd+Option+Esc, Cmd+Tab, Cmd+Space, Ctrl+Cmd+Q
    if m.cmd
        && !m.ctrl
        && !m.alt
        && !m.shift
        && !m.super_key
        && matches!(k, "Q" | "H" | "M" | "Tab" | "Space")
    {
        return true;
    }
    if m.cmd && m.alt && !m.ctrl && !m.shift && k == "Escape" {
        return true;
    }
    if m.cmd && m.ctrl && !m.alt && !m.shift && k == "Q" {
        return true;
    }
    // Windows / Linux reserved OS chords: Ctrl+Alt+Del, Alt+F4, Alt+Tab, Super+L / Win+L
    if m.ctrl && m.alt && !m.cmd && !m.shift && !m.super_key && k == "Delete" {
        return true;
    }
    if m.alt && !m.ctrl && !m.cmd && !m.shift && !m.super_key && matches!(k, "F4" | "Tab") {
        return true;
    }
    if m.super_key && !m.ctrl && !m.alt && !m.shift && !m.cmd && k == "L" {
        return true;
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeybindingSpec {
    pub action: KeybindingAction,
    pub category: KeybindingCategory,
    pub default_chord: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeybindingEntry {
    pub action: KeybindingAction,
    pub category: KeybindingCategory,
    pub chord: KeyChord,
    pub description: String,
    pub is_overridden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeybindingTable {
    pub entries: BTreeMap<KeybindingAction, KeybindingEntry>,
}

impl KeybindingTable {
    pub fn default_table() -> Self {
        let mut entries = BTreeMap::new();
        for spec in DEFAULT_KEYBINDINGS {
            let chord = KeyChord::parse(spec.default_chord)
                .unwrap_or_else(|| KeyChord::new(KeyModifiers::default(), spec.default_chord));
            entries.insert(
                spec.action,
                KeybindingEntry {
                    action: spec.action,
                    category: spec.category,
                    chord,
                    description: spec.description.to_string(),
                    is_overridden: false,
                },
            );
        }
        Self { entries }
    }

    pub fn get(&self, action: KeybindingAction) -> Option<&KeybindingEntry> {
        self.entries.get(&action)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Applies a user keybinding override while strictly rejecting blocked OS/system shortcuts.
pub fn apply_keybinding_override(
    table: &mut KeybindingTable,
    action: KeybindingAction,
    chord: KeyChord,
) -> Result<(), OrcaIdeError> {
    if is_system_shortcut_blocked(&chord) {
        return Err(OrcaIdeError::new(
            OrcaIdeErrorCode::SystemShortcutBlocked,
            format!(
                "keybinding '{}' is reserved by the operating system",
                chord.canonical_string()
            ),
        ));
    }
    let entry = table.entries.get_mut(&action).ok_or_else(|| {
        OrcaIdeError::new(
            OrcaIdeErrorCode::TabNotFound,
            format!("keybinding action '{action:?}' not present in table"),
        )
    })?;
    entry.chord = chord;
    entry.is_overridden = true;
    Ok(())
}

const fn kb(
    action: KeybindingAction,
    category: KeybindingCategory,
    default_chord: &'static str,
    description: &'static str,
) -> KeybindingSpec {
    KeybindingSpec {
        action,
        category,
        default_chord,
        description,
    }
}

/// Complete default 103-action keybinding catalog across all 10 Orca categories.
pub const DEFAULT_KEYBINDINGS: [KeybindingSpec; KEYBINDING_ACTION_COUNT] = [
    // Workspace (12)
    kb(KeybindingAction::WorkspaceNewWorktree, KeybindingCategory::Workspace, "Cmd+N", "New Git worktree workspace"),
    kb(KeybindingAction::WorkspaceNewFolder, KeybindingCategory::Workspace, "Cmd+Shift+N", "New folder workspace"),
    kb(KeybindingAction::WorkspaceSwitchNext, KeybindingCategory::Workspace, "Cmd+Alt+ArrowDown", "Next workspace"),
    kb(KeybindingAction::WorkspaceSwitchPrev, KeybindingCategory::Workspace, "Cmd+Alt+ArrowUp", "Previous workspace"),
    kb(KeybindingAction::WorkspaceHistoryBack, KeybindingCategory::Workspace, "Cmd+[", "Workspace history back"),
    kb(KeybindingAction::WorkspaceHistoryForward, KeybindingCategory::Workspace, "Cmd+]", "Workspace history forward"),
    kb(KeybindingAction::WorkspaceRename, KeybindingCategory::Workspace, "Cmd+Shift+R", "Rename workspace"),
    kb(KeybindingAction::WorkspaceArchive, KeybindingCategory::Workspace, "Cmd+Alt+A", "Archive workspace"),
    kb(KeybindingAction::WorkspaceDelete, KeybindingCategory::Workspace, "Cmd+Alt+Backspace", "Delete workspace"),
    kb(KeybindingAction::WorkspaceOpenCheckpoint, KeybindingCategory::Workspace, "Cmd+Shift+C", "Edit workspace checkpoint"),
    kb(KeybindingAction::WorkspaceToggleFloating, KeybindingCategory::Workspace, "Cmd+Shift+`", "Toggle floating scratch workspace"),
    kb(KeybindingAction::WorkspaceOpenSettings, KeybindingCategory::Workspace, "Cmd+,", "Open Orca settings"),
    // Tabs (14)
    kb(KeybindingAction::TabNew, KeybindingCategory::Tabs, "Cmd+T", "New tab"),
    kb(KeybindingAction::TabClose, KeybindingCategory::Tabs, "Cmd+W", "Close active tab"),
    kb(KeybindingAction::TabCloseOthers, KeybindingCategory::Tabs, "Cmd+Alt+W", "Close other tabs in pane"),
    kb(KeybindingAction::TabCloseRight, KeybindingCategory::Tabs, "Cmd+Ctrl+W", "Close tabs to the right"),
    kb(KeybindingAction::TabCloseAll, KeybindingCategory::Tabs, "Cmd+Shift+W", "Close all tabs in pane"),
    kb(KeybindingAction::TabReopenClosed, KeybindingCategory::Tabs, "Cmd+Shift+T", "Reopen last closed tab"),
    kb(KeybindingAction::TabNext, KeybindingCategory::Tabs, "Cmd+Shift+]", "Next tab"),
    kb(KeybindingAction::TabPrev, KeybindingCategory::Tabs, "Cmd+Shift+[", "Previous tab"),
    kb(KeybindingAction::TabMruNext, KeybindingCategory::Tabs, "Ctrl+Tab", "Next MRU tab"),
    kb(KeybindingAction::TabMruPrev, KeybindingCategory::Tabs, "Ctrl+Shift+Tab", "Previous MRU tab"),
    kb(KeybindingAction::TabTogglePin, KeybindingCategory::Tabs, "Cmd+Alt+P", "Pin or unpin tab"),
    kb(KeybindingAction::TabDuplicate, KeybindingCategory::Tabs, "Cmd+Alt+D", "Duplicate active tab"),
    kb(KeybindingAction::TabMoveLeft, KeybindingCategory::Tabs, "Cmd+Ctrl+Shift+ArrowLeft", "Move tab left"),
    kb(KeybindingAction::TabMoveRight, KeybindingCategory::Tabs, "Cmd+Ctrl+Shift+ArrowRight", "Move tab right"),
    // Panes (12)
    kb(KeybindingAction::PaneSplitRight, KeybindingCategory::Panes, "Cmd+D", "Split pane right"),
    kb(KeybindingAction::PaneSplitDown, KeybindingCategory::Panes, "Cmd+Shift+D", "Split pane down"),
    kb(KeybindingAction::PaneClose, KeybindingCategory::Panes, "Cmd+Alt+Shift+W", "Close active pane"),
    kb(KeybindingAction::PaneFocusLeft, KeybindingCategory::Panes, "Cmd+Alt+ArrowLeft", "Focus pane left"),
    kb(KeybindingAction::PaneFocusRight, KeybindingCategory::Panes, "Cmd+Alt+ArrowRight", "Focus pane right"),
    kb(KeybindingAction::PaneFocusUp, KeybindingCategory::Panes, "Cmd+Ctrl+ArrowUp", "Focus pane up"),
    kb(KeybindingAction::PaneFocusDown, KeybindingCategory::Panes, "Cmd+Ctrl+ArrowDown", "Focus pane down"),
    kb(KeybindingAction::PaneSwapNext, KeybindingCategory::Panes, "Cmd+Alt+S", "Swap with next pane"),
    kb(KeybindingAction::PaneRebalance, KeybindingCategory::Panes, "Cmd+Alt+=", "Rebalance unpinned split ratios"),
    kb(KeybindingAction::PaneToggleZoom, KeybindingCategory::Panes, "Cmd+Shift+Enter", "Zoom or unzoom active pane"),
    kb(KeybindingAction::PanePinBoundary, KeybindingCategory::Panes, "Cmd+Alt+B", "Pin or unpin split boundary"),
    kb(KeybindingAction::PaneMoveTabToNext, KeybindingCategory::Panes, "Cmd+Alt+Shift+ArrowRight", "Move active tab to next pane"),
    // Terminal (12)
    kb(KeybindingAction::TerminalNew, KeybindingCategory::Terminal, "Ctrl+Shift+`", "New terminal tab"),
    kb(KeybindingAction::TerminalClear, KeybindingCategory::Terminal, "Cmd+K", "Clear terminal scrollback"),
    kb(KeybindingAction::TerminalSplitHorizontal, KeybindingCategory::Terminal, "Ctrl+Shift+H", "Split terminal horizontally"),
    kb(KeybindingAction::TerminalSplitVertical, KeybindingCategory::Terminal, "Ctrl+Shift+V", "Split terminal vertically"),
    kb(KeybindingAction::TerminalFind, KeybindingCategory::Terminal, "Cmd+F", "Find in terminal"),
    kb(KeybindingAction::TerminalCopy, KeybindingCategory::Terminal, "Cmd+C", "Copy terminal selection"),
    kb(KeybindingAction::TerminalPaste, KeybindingCategory::Terminal, "Cmd+V", "Paste into terminal"),
    kb(KeybindingAction::TerminalSendInterrupt, KeybindingCategory::Terminal, "Ctrl+C", "Send SIGINT to terminal"),
    kb(KeybindingAction::TerminalQuickCommands, KeybindingCategory::Terminal, "Cmd+Shift+K", "Open terminal quick commands"),
    kb(KeybindingAction::TerminalScrollToBottom, KeybindingCategory::Terminal, "Cmd+ArrowDown", "Scroll terminal to bottom"),
    kb(KeybindingAction::TerminalToggleWordWrap, KeybindingCategory::Terminal, "Alt+Z", "Toggle terminal word wrap"),
    kb(KeybindingAction::TerminalReset, KeybindingCategory::Terminal, "Cmd+Alt+R", "Reset terminal state"),
    // Editor & Diff (12)
    kb(KeybindingAction::EditorSave, KeybindingCategory::EditorAndDiff, "Cmd+S", "Save active file"),
    kb(KeybindingAction::EditorSaveAll, KeybindingCategory::EditorAndDiff, "Cmd+Alt+Shift+S", "Save all dirty files"),
    kb(KeybindingAction::EditorFormat, KeybindingCategory::EditorAndDiff, "Shift+Alt+F", "Format document"),
    kb(KeybindingAction::EditorFindInFile, KeybindingCategory::EditorAndDiff, "Cmd+Shift+F", "Find in file"),
    kb(KeybindingAction::EditorReplaceInFile, KeybindingCategory::EditorAndDiff, "Cmd+Alt+F", "Replace in file"),
    kb(KeybindingAction::EditorGoToLine, KeybindingCategory::EditorAndDiff, "Ctrl+G", "Go to line"),
    kb(KeybindingAction::EditorToggleMinimap, KeybindingCategory::EditorAndDiff, "Cmd+Alt+M", "Toggle editor minimap"),
    kb(KeybindingAction::EditorToggleBlame, KeybindingCategory::EditorAndDiff, "Cmd+Alt+G", "Toggle inline git blame"),
    kb(KeybindingAction::DiffNextHunk, KeybindingCategory::EditorAndDiff, "Alt+F5", "Next diff hunk"),
    kb(KeybindingAction::DiffPrevHunk, KeybindingCategory::EditorAndDiff, "Shift+Alt+F5", "Previous diff hunk"),
    kb(KeybindingAction::DiffToggleSideBySide, KeybindingCategory::EditorAndDiff, "Cmd+Alt+V", "Toggle side-by-side diff"),
    kb(KeybindingAction::DiffStageHunk, KeybindingCategory::EditorAndDiff, "Cmd+Shift+S", "Stage active diff hunk"),
    // Agent & Orchestration (11)
    kb(KeybindingAction::AgentFlipChatTerminal, KeybindingCategory::AgentAndOrchestration, "Cmd+Shift+J", "Flip Chat <-> Terminal view"),
    kb(KeybindingAction::AgentNewChat, KeybindingCategory::AgentAndOrchestration, "Cmd+Shift+L", "Start new agent chat"),
    kb(KeybindingAction::AgentStopTurn, KeybindingCategory::AgentAndOrchestration, "Cmd+.", "Stop active agent turn"),
    kb(KeybindingAction::AgentApproveAction, KeybindingCategory::AgentAndOrchestration, "Cmd+Enter", "Approve pending agent action"),
    kb(KeybindingAction::AgentRejectAction, KeybindingCategory::AgentAndOrchestration, "Cmd+Backspace", "Reject pending agent action"),
    kb(KeybindingAction::AgentToggleKanban, KeybindingCategory::AgentAndOrchestration, "Cmd+Shift+B", "Toggle Agent Kanban board"),
    kb(KeybindingAction::AgentPopoutKanban, KeybindingCategory::AgentAndOrchestration, "Cmd+Alt+Shift+B", "Pop out Agent Kanban window"),
    kb(KeybindingAction::OrchestrationNewRun, KeybindingCategory::AgentAndOrchestration, "Cmd+Alt+O", "Start orchestration run"),
    kb(KeybindingAction::OrchestrationDispatchWorker, KeybindingCategory::AgentAndOrchestration, "Cmd+Shift+O", "Dispatch orchestration worker"),
    kb(KeybindingAction::OrchestrationOpenMailbox, KeybindingCategory::AgentAndOrchestration, "Cmd+Alt+I", "Open orchestration mailbox"),
    kb(KeybindingAction::OrchestrationInspectGates, KeybindingCategory::AgentAndOrchestration, "Cmd+Alt+E", "Inspect review & merge gates"),
    // Bots (10)
    kb(KeybindingAction::BotOpenSidebar, KeybindingCategory::Bots, "Cmd+Shift+E", "Focus Bots sidebar"),
    kb(KeybindingAction::BotNew, KeybindingCategory::Bots, "Cmd+Alt+N", "Create new Orca Bot"),
    kb(KeybindingAction::BotOpenNeedsYou, KeybindingCategory::Bots, "Cmd+Shift+U", "Open Needs You inbox"),
    kb(KeybindingAction::BotOpenUpNext, KeybindingCategory::Bots, "Cmd+Alt+U", "Open Up Next suggestions"),
    kb(KeybindingAction::BotOpenGroupRoom, KeybindingCategory::Bots, "Cmd+Shift+G", "Open Bot Group room"),
    kb(KeybindingAction::BotHandOverWorkspace, KeybindingCategory::Bots, "Cmd+Alt+H", "Hand over workspace to Bot"),
    kb(KeybindingAction::BotRunLookAround, KeybindingCategory::Bots, "Cmd+Alt+L", "Trigger Bot Look Around"),
    kb(KeybindingAction::BotRunTidyUp, KeybindingCategory::Bots, "Cmd+Alt+T", "Trigger Bot Memory Tidy-Up"),
    kb(KeybindingAction::BotOpenTemplates, KeybindingCategory::Bots, "Cmd+Alt+Shift+T", "Open 24-Bot Template Gallery"),
    kb(KeybindingAction::BotTogglePause, KeybindingCategory::Bots, "Cmd+Alt+Shift+P", "Pause or resume active Bot"),
    // Search & Switcher (10)
    kb(KeybindingAction::SwitcherQuickJump, KeybindingCategory::SearchAndSwitcher, "Cmd+J", "Open Cmd+J Quick Switcher"),
    kb(KeybindingAction::SwitcherQuickOpenFile, KeybindingCategory::SearchAndSwitcher, "Cmd+P", "Quick open file"),
    kb(KeybindingAction::SwitcherCommandPalette, KeybindingCategory::SearchAndSwitcher, "Cmd+Shift+P", "Open command palette"),
    kb(KeybindingAction::SwitcherGlobalSearch, KeybindingCategory::SearchAndSwitcher, "Cmd+Alt+Shift+F", "Search across workspaces"),
    kb(KeybindingAction::SwitcherJumpRecent1, KeybindingCategory::SearchAndSwitcher, "Cmd+1", "Jump to recent item 1"),
    kb(KeybindingAction::SwitcherJumpRecent2, KeybindingCategory::SearchAndSwitcher, "Cmd+2", "Jump to recent item 2"),
    kb(KeybindingAction::SwitcherJumpRecent3, KeybindingCategory::SearchAndSwitcher, "Cmd+3", "Jump to recent item 3"),
    kb(KeybindingAction::SwitcherJumpRecent4, KeybindingCategory::SearchAndSwitcher, "Cmd+4", "Jump to recent item 4"),
    kb(KeybindingAction::SwitcherJumpRecent5, KeybindingCategory::SearchAndSwitcher, "Cmd+5", "Jump to recent item 5"),
    kb(KeybindingAction::SwitcherJumpRecent6, KeybindingCategory::SearchAndSwitcher, "Cmd+6", "Jump to recent item 6"),
    // Review & Design (6)
    kb(KeybindingAction::ReviewAddDiffAnnotation, KeybindingCategory::ReviewAndDesign, "C", "Annotate active diff line"),
    kb(KeybindingAction::ReviewSendAnnotationsToAgent, KeybindingCategory::ReviewAndDesign, "Cmd+Shift+Enter", "Send diff annotations batch to agent"),
    kb(KeybindingAction::ReviewResolveAnnotation, KeybindingCategory::ReviewAndDesign, "Alt+Enter", "Resolve active diff annotation"),
    kb(KeybindingAction::ReviewToggleAiAttribution, KeybindingCategory::ReviewAndDesign, "Cmd+Alt+Shift+A", "Toggle line-level AI attribution"),
    kb(KeybindingAction::DesignToggleBrowserMode, KeybindingCategory::ReviewAndDesign, "Cmd+Shift+I", "Toggle Browser Design Mode"),
    kb(KeybindingAction::DesignCaptureElement, KeybindingCategory::ReviewAndDesign, "Cmd+Alt+C", "Capture selected DOM element to chat"),
    // Window & App (4)
    kb(KeybindingAction::WindowToggleSidebar, KeybindingCategory::WindowAndApp, "Cmd+B", "Toggle primary sidebar"),
    kb(KeybindingAction::WindowToggleStatusBar, KeybindingCategory::WindowAndApp, "Cmd+Shift+/", "Toggle status bar"),
    kb(KeybindingAction::WindowZoomIn, KeybindingCategory::WindowAndApp, "Cmd+=", "Zoom UI in"),
    kb(KeybindingAction::WindowZoomOut, KeybindingCategory::WindowAndApp, "Cmd+-", "Zoom UI out"),
];

// ---------------------------------------------------------------------------
// `Cmd+J` Quick Switcher (Unicode-Safe Fuzzy / Prefix / Boundary / Attention Scorer)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickSwitcherItemKind {
    Worktree,
    FolderWorkspace,
    OpenTab,
    RecentChat,
    PullRequest,
    MergeRequest,
    Bot,
    Command,
}

/// Attention priority used by `Cmd+J` empty-query ranking (`NeedsYou > Done > Working > Idle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionPriority {
    #[default]
    Idle = 0,
    Working = 1,
    Done = 2,
    NeedsYou = 3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickSwitcherItem {
    pub id: String,
    pub kind: QuickSwitcherItemKind,
    pub title: String,
    pub subtitle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
    pub recency_ms: i64,
    pub pinned: bool,
    #[serde(default)]
    pub attention: AttentionPriority,
    #[serde(default)]
    pub is_current_tab: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickSwitcherMatch {
    pub item: QuickSwitcherItem,
    pub score: i64,
}

/// Scores a single `Cmd+J` candidate against `query` using Unicode-safe character
/// matching (exact > prefix > word-boundary > substring > fuzzy subsequence),
/// title-over-subtitle preference, attention (`needs_you > done > idle`), pinned
/// boost, and recency tie-breaker.
pub fn score_quick_switcher_match(
    query: &str,
    item: &QuickSwitcherItem,
    now_ms: i64,
) -> Option<i64> {
    let q_trimmed = query.trim();
    let recency_bonus = compute_recency_bonus(item.recency_ms, now_ms);
    let pin_bonus = if item.pinned { 120 } else { 0 };
    let attention_bonus = match item.attention {
        AttentionPriority::NeedsYou => 4_000,
        AttentionPriority::Done => 2_000,
        AttentionPriority::Working => 500,
        AttentionPriority::Idle => 0,
    };

    if q_trimmed.is_empty() {
        // Omit idle current tab from empty-query recent jump list unless pinned.
        if item.is_current_tab && item.attention == AttentionPriority::Idle && !item.pinned {
            return None;
        }
        return Some(attention_bonus + pin_bonus + recency_bonus);
    }

    let q_chars: Vec<char> = q_trimmed.chars().flat_map(char::to_lowercase).collect();
    if q_chars.is_empty() {
        return Some(attention_bonus + pin_bonus + recency_bonus);
    }

    let title_score = score_text_field(&q_chars, &item.title).map(|s| s * 3);
    let sub_score = score_text_field(&q_chars, &item.subtitle);

    let best_match = match (title_score, sub_score) {
        (Some(ts), Some(ss)) => ts.max(ss) + (ss / 6),
        (Some(ts), None) => ts,
        (None, Some(ss)) => ss,
        (None, None) => {
            // Try matching across combined "title subtitle" for multi-word queries.
            let combined = format!("{} {}", item.title, item.subtitle);
            score_text_field(&q_chars, &combined)?
        }
    };

    let open_tab_bonus = if matches!(
        item.kind,
        QuickSwitcherItemKind::OpenTab | QuickSwitcherItemKind::RecentChat
    ) {
        80
    } else {
        0
    };

    Some(best_match + (attention_bonus / 4) + pin_bonus + open_tab_bonus + recency_bonus)
}

/// Deterministically ranks `Cmd+J` items returning the top `limit` matches.
pub fn rank_quick_switcher_items(
    query: &str,
    items: &[QuickSwitcherItem],
    now_ms: i64,
    limit: usize,
) -> Vec<QuickSwitcherMatch> {
    if limit == 0 {
        return Vec::new();
    }
    let mut matches: Vec<QuickSwitcherMatch> = items
        .iter()
        .filter_map(|item| {
            score_quick_switcher_match(query, item, now_ms).map(|score| QuickSwitcherMatch {
                item: item.clone(),
                score,
            })
        })
        .collect();

    matches.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.item.recency_ms.cmp(&a.item.recency_ms))
            .then_with(|| a.item.id.cmp(&b.item.id))
    });
    matches.truncate(limit);
    matches
}

fn compute_recency_bonus(recency_ms: i64, now_ms: i64) -> i64 {
    let age_ms = now_ms.saturating_sub(recency_ms).max(0);
    let age_minutes = age_ms / 60_000;
    (1_000 - age_minutes.min(1_000)) / 4
}

fn is_word_boundary(orig: &[char], idx: usize) -> bool {
    if idx == 0 {
        return true;
    }
    let prev = orig[idx - 1];
    let curr = orig[idx];
    matches!(prev, ' ' | '/' | '\\' | '-' | '_' | '.' | ':' | '#')
        || (prev.is_lowercase() && curr.is_uppercase())
}

fn score_text_field(q_lower: &[char], text: &str) -> Option<i64> {
    let orig_chars: Vec<char> = text.chars().collect();
    // Map each char in `orig_chars` to its first lowercased char for aligned boundary detection.
    let lower_chars: Vec<char> = orig_chars
        .iter()
        .map(|&c| c.to_lowercase().next().unwrap_or(c))
        .collect();

    let q_len = q_lower.len();
    let t_len = lower_chars.len();
    if q_len == 0 || q_len > t_len {
        return None;
    }

    if lower_chars == q_lower {
        return Some(10_000);
    }
    if lower_chars.starts_with(q_lower) {
        return Some(8_000 - ((t_len - q_len).min(200) as i64));
    }

    // Check contiguous substring match (preferring word-boundary start).
    for start in 0..=(t_len - q_len) {
        if lower_chars[start..start + q_len] == *q_lower {
            let boundary_bonus = if is_word_boundary(&orig_chars, start) {
                2_000
            } else {
                0
            };
            return Some(5_000 + boundary_bonus - (start.min(200) as i64));
        }
    }

    // Ordered Unicode subsequence match preferring word-boundary & tight runs.
    let mut qi = 0usize;
    let mut prev_match: Option<usize> = None;
    let mut score: i64 = 1_500;

    for (ti, &ch) in lower_chars.iter().enumerate() {
        if qi < q_len && ch == q_lower[qi] {
            if is_word_boundary(&orig_chars, ti) {
                score += 220;
            }
            if let Some(pm) = prev_match {
                let gap = ti - pm - 1;
                if gap == 0 {
                    score += 180;
                } else {
                    score -= (gap as i64).min(30) * 12;
                }
            } else {
                score -= (ti as i64).min(60) * 4;
            }
            prev_match = Some(ti);
            qi += 1;
        }
    }

    if qi == q_len { Some(score.max(1)) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn make_tab(id: &str, kind: TabContentType) -> WorkspaceTab {
        WorkspaceTab {
            tab_id: id.to_string(),
            content_type: kind,
            title: format!("Tab {id}"),
            pinned: false,
            preview: false,
            view_mode: TabViewMode::Terminal,
            leaf_state: TabLeafState::Live,
            pty_incarnation_id: Some(format!("inc_{id}")),
            dirty: false,
            created_at_ms: 1_000,
            last_active_at_ms: 1_000,
        }
    }

    #[test]
    fn tab_content_types_exact_10_wire_contract() {
        let expected = [
            "terminal",
            "editor",
            "diff",
            "conflict-review",
            "check-details",
            "git-graph",
            "agent-session",
            "browser",
            "simulator",
            "room",
        ];
        assert_eq!(TabContentType::ALL.len(), 10);
        for (variant, wire) in TabContentType::ALL.into_iter().zip(expected) {
            assert_eq!(variant.as_str(), wire);
            assert_eq!(TabContentType::parse(wire), Some(variant));
            let json = serde_json::to_string(&variant).expect("serialize");
            assert_eq!(json, format!("\"{wire}\""));
            let back: TabContentType = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, variant);
        }
        assert!(TabContentType::Terminal.traits().supports_dual_view);
        assert!(TabContentType::AgentSession.traits().supports_hibernation);
        assert!(!TabContentType::Browser.traits().requires_pty_tombstone);
    }

    #[test]
    fn layout_splits_ratios_tombstones_reopen_and_stale_replay_rejection() {
        let mut model = WorkspaceLayoutModel::new("ws_1", "local", "pane_1");
        model
            .open_tab("pane_1", make_tab("t1", TabContentType::Terminal), 1_000)
            .expect("open t1");
        model
            .split_pane(
                "pane_1",
                "pane_2",
                SplitDirection::Horizontal,
                9_999, // must clamp to 9000
                Some(make_tab("t2", TabContentType::Diff)),
            )
            .expect("split pane");
        assert_eq!(model.pane_count(), 2);
        assert_eq!(model.tab_count(), 2);
        if let SplitPaneNode::Split { ratio_bp, .. } = &model.root {
            assert_eq!(*ratio_bp, SPLIT_RATIO_MAX_BP);
        } else {
            panic!("expected Split root");
        }

        // Pin boundary -> rebalance preserves pinned ratio; unpin -> resets to 5000
        model.set_split_ratio(&[], 200, Some(true)).expect("set ratio");
        if let SplitPaneNode::Split { ratio_bp, .. } = &model.root {
            assert_eq!(*ratio_bp, SPLIT_RATIO_MIN_BP);
        }
        model.rebalance_splits();
        if let SplitPaneNode::Split { ratio_bp, .. } = &model.root {
            assert_eq!(*ratio_bp, SPLIT_RATIO_MIN_BP);
        }
        model.set_split_ratio(&[], 2_500, Some(false)).expect("unpin");
        model.rebalance_splits();
        if let SplitPaneNode::Split { ratio_bp, .. } = &model.root {
            assert_eq!(*ratio_bp, SPLIT_RATIO_DEFAULT_BP);
        }

        // Move & dual-view flip
        model.move_tab("t2", "pane_1", Some(0)).expect("move t2");
        assert_eq!(model.toggle_tab_view_mode("t1").expect("flip"), TabViewMode::Chat);

        // Close t1 -> tombstone rejects stale recreate for 30 days
        model.close_tab("t1", 5_000).expect("close t1");
        let stale_err = model
            .open_tab("pane_1", make_tab("t1", TabContentType::Terminal), 10_000)
            .expect_err("stale create rejected");
        assert_eq!(stale_err.code, OrcaIdeErrorCode::TabNotFound);

        // Reopen restores t1
        let reopened = model.reopen_closed_tab(Some("t1"), 20_000).expect("reopen t1");
        assert_eq!(reopened.tab_id, "t1");

        // Tombstone expires after 30 days
        model.close_tab("t1", 100_000).expect("close t1 again");
        let after_30d = 100_000 + CLOSED_TAB_TOMBSTONE_MS + 1;
        let expired_err = model
            .reopen_closed_tab(Some("t1"), after_30d)
            .expect_err("expired tombstone");
        assert_eq!(expired_err.code, OrcaIdeErrorCode::TabNotFound);
    }

    #[test]
    fn keybindings_103_distinct_actions_and_system_shortcut_blocklist() {
        assert_eq!(DEFAULT_KEYBINDINGS.len(), KEYBINDING_ACTION_COUNT);
        let distinct: BTreeSet<KeybindingAction> =
            DEFAULT_KEYBINDINGS.iter().map(|spec| spec.action).collect();
        assert_eq!(distinct.len(), 103);

        let mut table = KeybindingTable::default_table();
        assert_eq!(table.len(), 103);

        // None of the 103 default bindings collide with blocked OS shortcuts
        for entry in table.entries.values() {
            assert!(
                !is_system_shortcut_blocked(&entry.chord),
                "default binding {:?} ({}) must not be OS-blocked",
                entry.action,
                entry.chord.canonical_string()
            );
        }

        // Blocked OS shortcuts are rejected with SystemShortcutBlocked
        for blocked in [
            "Cmd+Q",
            "Cmd+H",
            "Cmd+M",
            "Cmd+Option+Esc",
            "Ctrl+Alt+Del",
            "Alt+F4",
            "Win+L",
            "Super+L",
            "Ctrl+Cmd+Q",
        ] {
            let chord = KeyChord::parse(blocked).expect("parse blocked chord");
            assert!(is_system_shortcut_blocked(&chord), "{blocked} must be blocked");
            let err = apply_keybinding_override(&mut table, KeybindingAction::TabNew, chord)
                .expect_err("override rejected");
            assert_eq!(err.code, OrcaIdeErrorCode::SystemShortcutBlocked);
        }

        // Valid custom override succeeds
        let custom = KeyChord::parse("Cmd+Shift+Y").expect("parse custom");
        apply_keybinding_override(&mut table, KeybindingAction::TabNew, custom.clone())
            .expect("custom override succeeds");
        assert_eq!(table.get(KeybindingAction::TabNew).expect("entry").chord, custom);
        assert!(table.get(KeybindingAction::TabNew).expect("entry").is_overridden);
    }

    #[test]
    fn quick_switcher_ranks_attention_fuzzy_unicode_and_recency_deterministically() {
        let now = 10_000_000;
        let items = vec![
            QuickSwitcherItem {
                id: "1".to_string(),
                kind: QuickSwitcherItemKind::OpenTab,
                title: "idle current".to_string(),
                subtitle: "repo/main".to_string(),
                host_id: Some("local".to_string()),
                repo_id: Some("r1".to_string()),
                recency_ms: now - 1_000,
                pinned: false,
                attention: AttentionPriority::Idle,
                is_current_tab: true,
            },
            QuickSwitcherItem {
                id: "2".to_string(),
                kind: QuickSwitcherItemKind::RecentChat,
                title: "Bot Scout — PR #42".to_string(),
                subtitle: "gpui-telegram/fix-relay".to_string(),
                host_id: Some("local".to_string()),
                repo_id: Some("r1".to_string()),
                recency_ms: now - 60_000,
                pinned: false,
                attention: AttentionPriority::NeedsYou,
                is_current_tab: false,
            },
            QuickSwitcherItem {
                id: "3".to_string(),
                kind: QuickSwitcherItemKind::Worktree,
                title: "orca-yaml-parser".to_string(),
                subtitle: "feature/中文测试".to_string(),
                host_id: Some("local".to_string()),
                repo_id: Some("r1".to_string()),
                recency_ms: now - 30_000,
                pinned: true,
                attention: AttentionPriority::Done,
                is_current_tab: false,
            },
        ];

        // Empty query: NeedsYou outranks Done, and idle current tab is omitted
        let empty_ranked = rank_quick_switcher_items("", &items, now, 6);
        assert_eq!(empty_ranked.len(), 2);
        assert_eq!(empty_ranked[0].item.id, "2");
        assert_eq!(empty_ranked[1].item.id, "3");

        // Word-boundary / fuzzy match on "oyp" matches "orca-yaml-parser"
        let fuzzy = rank_quick_switcher_items("oyp", &items, now, 5);
        assert_eq!(fuzzy.len(), 1);
        assert_eq!(fuzzy[0].item.id, "3");

        // Unicode query safely matches CJK subtitle
        let cjk = rank_quick_switcher_items("中文", &items, now, 5);
        assert_eq!(cjk.len(), 1);
        assert_eq!(cjk[0].item.id, "3");
    }
}
