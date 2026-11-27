//! Pure-Rust Bot Items domain model, state machine, typed links, lineage deduplication,
//! structured Ask validation & linear-time multi-select reply parser, snooze/stale logic,
//! and `needs_you_fold` priority folding (Deliverable #4).

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{BotError, BotErrorCode};

pub use crate::bot_types::BotItemsStale;

// ---------------------------------------------------------------------------
// Canonical Limits & Constants
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BotItemLimits {
    pub title: usize,
    pub description: usize,
    pub done_means: usize,
    pub text: usize,
    pub wait: usize,
    pub links: usize,
    pub list_page: usize,
    pub list_page_max: usize,
    pub events_page: usize,
    pub comment_files: usize,
}

pub const BOT_ITEM_LIMITS: BotItemLimits = BotItemLimits {
    title: 200,
    description: 20_000,
    done_means: 2_000,
    text: 8_000,
    wait: 200,
    links: 1_000,
    list_page: 50,
    list_page_max: 100,
    events_page: 50,
    comment_files: 10,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BotItemAskLimits {
    pub max_questions: usize,
    pub prompt_max: usize,
    pub max_options: usize,
    pub label_max: usize,
    pub detail_max: usize,
    pub context_max: usize,
}

pub const BOT_ITEM_ASK_LIMITS: BotItemAskLimits = BotItemAskLimits {
    max_questions: 5,
    prompt_max: 500,
    max_options: 8,
    label_max: 150,
    detail_max: 500,
    context_max: 8_000,
};

pub const NEEDS_YOU_FOLD_CAP: usize = 5;
pub const PLANNED_STALE_MULTIPLIER: u32 = 3;
pub const MS_PER_DAY: i64 = 86_400_000;
pub const ASK_REPLY_ARROW: &str = " \u{2192} ";

/// Clamps a requested page size to `[1, BOT_ITEM_LIMITS.list_page_max]`,
/// defaulting to `BOT_ITEM_LIMITS.list_page` (`50`).
pub fn normalize_list_page_limit(requested: Option<usize>) -> usize {
    requested
        .unwrap_or(BOT_ITEM_LIMITS.list_page)
        .clamp(1, BOT_ITEM_LIMITS.list_page_max)
}

/// Forward-compatibility guard for SQLite `user_version`: if the persisted DB
/// schema is newer than this binary supports, refuse writes with `items_read_only`.
pub fn check_items_schema_version(db_version: u32, current_version: u32) -> Result<(), BotError> {
    if db_version > current_version {
        return Err(BotError::new(
            BotErrorCode::ItemsReadOnly,
            format!(
                "Bot items database schema version {db_version} is newer than supported version {current_version}; operating read-only."
            ),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// BotItemStatus & BotItemActor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotItemStatus {
    #[default]
    Planned,
    Working,
    Waiting,
    #[serde(alias = "needs-you")]
    NeedsYou,
    Done,
    Canceled,
}

impl BotItemStatus {
    pub const ALL: [Self; 6] = [
        Self::Planned,
        Self::Working,
        Self::Waiting,
        Self::NeedsYou,
        Self::Done,
        Self::Canceled,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::NeedsYou => "needs_you",
            Self::Done => "done",
            Self::Canceled => "canceled",
        }
    }

    pub fn parse_strict(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == raw)
    }

    pub fn parse_cli(raw: &str) -> Option<Self> {
        if raw == "needs-you" {
            Some(Self::NeedsYou)
        } else {
            Self::parse_strict(raw)
        }
    }

    pub const fn is_open(self) -> bool {
        matches!(
            self,
            Self::Planned | Self::Working | Self::Waiting | Self::NeedsYou
        )
    }

    pub const fn is_closed(self) -> bool {
        !self.is_open()
    }

    pub const fn auto_ends_snooze(self) -> bool {
        matches!(self, Self::NeedsYou | Self::Done | Self::Canceled)
    }
}

impl fmt::Display for BotItemStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BotItemActor {
    Owner,
    Bot {
        #[serde(alias = "botId")]
        bot_id: String,
    },
    Person {
        source: String,
        #[serde(default, alias = "userId")]
        user_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    System,
}

impl BotItemActor {
    pub const fn is_owner(&self) -> bool {
        matches!(self, Self::Owner)
    }

    pub fn bot_id(&self) -> Option<&str> {
        match self {
            Self::Bot { bot_id } => Some(bot_id.as_str()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Typed Item Links & Cycle-Safe Dispatch Lineage Deduplication
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotItemLinkKind {
    Dispatch,
    Pr,
    Url,
    File,
    Automation,
}

impl BotItemLinkKind {
    pub const ALL: [Self; 5] = [
        Self::Dispatch,
        Self::Pr,
        Self::Url,
        Self::File,
        Self::Automation,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dispatch => "dispatch",
            Self::Pr => "pr",
            Self::Url => "url",
            Self::File => "file",
            Self::Automation => "automation",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchLiveState {
    Working,
    Completed,
    Failed,
    Stopped,
    Unknown,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchReport {
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchLinkMeta {
    #[serde(default, alias = "chainId", skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    #[serde(
        default,
        alias = "parentDispatchId",
        alias = "parentDispatch",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_dispatch_id: Option<String>,
    #[serde(
        default,
        alias = "latestDispatchId",
        alias = "latest",
        skip_serializing_if = "Option::is_none"
    )]
    pub latest_dispatch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<DispatchLiveState>,
    #[serde(default, alias = "neverStarted", skip_serializing_if = "std::ops::Not::not")]
    pub never_started: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, alias = "startedAt", skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    #[serde(default, alias = "settledAt", skip_serializing_if = "Option::is_none")]
    pub settled_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<DispatchReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrLinkState {
    Open,
    Draft,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrLinkMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<PrLinkState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, alias = "checkedAt", skip_serializing_if = "Option::is_none")]
    pub checked_at_ms: Option<i64>,
    #[serde(default, alias = "worktreeId", skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubIssueCache {
    pub title: String,
    pub state: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlLinkMeta {
    #[serde(default, alias = "githubIssue", skip_serializing_if = "Option::is_none")]
    pub github_issue: Option<GitHubIssueCache>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationLinkMeta {
    #[serde(default)]
    pub active: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BotItemLinkMeta {
    #[default]
    None,
    Dispatch(DispatchLinkMeta),
    Pr(PrLinkMeta),
    Url(UrlLinkMeta),
    File,
    Automation(AutomationLinkMeta),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotItemLink {
    pub kind: BotItemLinkKind,
    #[serde(rename = "ref", alias = "reference")]
    pub reference: String,
    #[serde(default, alias = "addedEvent")]
    pub added_event: u64,
    #[serde(default)]
    pub meta: BotItemLinkMeta,
}

impl BotItemLink {
    pub fn validate(&self) -> Result<(), BotError> {
        let trimmed = self.reference.trim();
        if trimmed.is_empty() || self.reference.chars().any(char::is_control) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Item link reference must be non-empty and contain no control characters.",
            ));
        }
        let meta_ok = matches!(
            (&self.kind, &self.meta),
            (_, BotItemLinkMeta::None)
                | (BotItemLinkKind::Dispatch, BotItemLinkMeta::Dispatch(_))
                | (BotItemLinkKind::Pr, BotItemLinkMeta::Pr(_))
                | (BotItemLinkKind::Url, BotItemLinkMeta::Url(_))
                | (BotItemLinkKind::File, BotItemLinkMeta::File)
                | (BotItemLinkKind::Automation, BotItemLinkMeta::Automation(_))
        );
        if !meta_ok {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Item link metadata does not match kind '{}'.", self.kind.as_str()),
            ));
        }
        Ok(())
    }

    pub fn dispatch_meta(&self) -> Option<&DispatchLinkMeta> {
        match &self.meta {
            BotItemLinkMeta::Dispatch(m) => Some(m),
            _ => None,
        }
    }
}

/// Idempotently attaches or updates a typed link on an item, enforcing the 1,000-link cap.
pub fn add_item_link(links: &mut Vec<BotItemLink>, incoming: BotItemLink) -> Result<(), BotError> {
    incoming.validate()?;
    if let Some(existing) = links
        .iter_mut()
        .find(|l| l.kind == incoming.kind && l.reference == incoming.reference)
    {
        *existing = incoming;
        return Ok(());
    }
    if links.len() >= BOT_ITEM_LIMITS.links {
        return Err(BotError::new(
            BotErrorCode::Full,
            format!("Item link limit ({}) reached.", BOT_ITEM_LIMITS.links),
        ));
    }
    links.push(incoming);
    Ok(())
}

struct DisjointSet {
    parent: Vec<usize>,
}

impl DisjointSet {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        let mut root = x;
        while let Some(&p) = self.parent.get(root) {
            if p == root {
                break;
            }
            root = p;
        }
        while let Some(&p) = self.parent.get(x) {
            if p == root {
                break;
            }
            if let Some(slot) = self.parent.get_mut(x) {
                *slot = root;
            }
            x = p;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb
            && let Some(slot) = self.parent.get_mut(rb)
        {
            *slot = ra;
        }
    }
}

fn dispatch_timestamp_ms(link: &BotItemLink) -> Option<i64> {
    link.dispatch_meta()
        .and_then(|m| m.settled_at_ms.or(m.started_at_ms))
}

/// Keeps at most one dispatch link per follow-up dispatch chain (supporting `chain_id`,
/// `parent_dispatch_id`, and `latest_dispatch_id` with cycle-safe union-find), while
/// preserving all non-dispatch links and original slice order.
pub fn bot_item_links_one_per_chain(links: &[BotItemLink]) -> Vec<BotItemLink> {
    let dispatch_indices: Vec<usize> = links
        .iter()
        .enumerate()
        .filter_map(|(idx, l)| (l.kind == BotItemLinkKind::Dispatch).then_some(idx))
        .collect();

    if dispatch_indices.len() <= 1 {
        return links.to_vec();
    }

    let n = dispatch_indices.len();
    let mut dsu = DisjointSet::new(n);
    let mut ref_to_indices: HashMap<&str, Vec<usize>> = HashMap::with_capacity(n);
    let mut parent_ref_to_indices: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut chain_to_d: HashMap<&str, usize> = HashMap::new();
    let mut referenced_dispatch_to_d: HashMap<&str, usize> = HashMap::new();

    for (d_idx, &slice_idx) in dispatch_indices.iter().enumerate() {
        let Some(link) = links.get(slice_idx) else {
            continue;
        };
        let entry = ref_to_indices.entry(link.reference.as_str()).or_default();
        if let Some(&first) = entry.first() {
            dsu.union(first, d_idx);
        }
        entry.push(d_idx);
        if let Some(meta) = link.dispatch_meta() {
            if let Some(cid) = meta.chain_id.as_deref().filter(|s| !s.trim().is_empty()) {
                if let Some(&prev) = chain_to_d.get(cid) {
                    dsu.union(prev, d_idx);
                } else {
                    chain_to_d.insert(cid, d_idx);
                }
            }
            if let Some(lat) = meta
                .latest_dispatch_id
                .as_deref()
                .filter(|s| !s.trim().is_empty())
            {
                if let Some(&prev) = referenced_dispatch_to_d.get(lat) {
                    dsu.union(prev, d_idx);
                } else {
                    referenced_dispatch_to_d.insert(lat, d_idx);
                }
            }
            if let Some(par) = meta
                .parent_dispatch_id
                .as_deref()
                .filter(|s| !s.trim().is_empty())
            {
                parent_ref_to_indices.entry(par).or_default().push(d_idx);
                if let Some(&prev) = referenced_dispatch_to_d.get(par) {
                    dsu.union(prev, d_idx);
                } else {
                    referenced_dispatch_to_d.insert(par, d_idx);
                }
            }
        }
    }

    for (d_idx, &slice_idx) in dispatch_indices.iter().enumerate() {
        let Some(meta) = links.get(slice_idx).and_then(BotItemLink::dispatch_meta) else {
            continue;
        };
        if let Some(parent_ref) = meta
            .parent_dispatch_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            && let Some(p_indices) = ref_to_indices.get(parent_ref)
        {
            for &p_idx in p_indices {
                dsu.union(p_idx, d_idx);
            }
        }
        if let Some(lat_ref) = meta
            .latest_dispatch_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            && let Some(l_indices) = ref_to_indices.get(lat_ref)
        {
            for &l_idx in l_indices {
                dsu.union(l_idx, d_idx);
            }
        }
    }

    let mut is_superseded = vec![false; n];
    for (d_idx, &slice_idx) in dispatch_indices.iter().enumerate() {
        let Some(link) = links.get(slice_idx) else {
            continue;
        };
        let Some(meta) = link.dispatch_meta() else {
            continue;
        };
        if let Some(parent_ref) = meta
            .parent_dispatch_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            && parent_ref != link.reference.as_str()
            && let Some(p_indices) = ref_to_indices.get(parent_ref)
        {
            let my_root = dsu.find(d_idx);
            for &p_idx in p_indices {
                if p_idx != d_idx
                    && dsu.find(p_idx) == my_root
                    && let Some(flag) = is_superseded.get_mut(p_idx)
                {
                    *flag = true;
                }
            }
        }
        if let Some(lat_ref) = meta
            .latest_dispatch_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            && lat_ref != link.reference.as_str()
        {
            let my_root = dsu.find(d_idx);
            let direct_newer = ref_to_indices
                .get(lat_ref)
                .is_some_and(|l_indices| l_indices.iter().any(|&l_idx| dsu.find(l_idx) == my_root));
            let transitive_newer = !direct_newer
                && parent_ref_to_indices
                    .get(lat_ref)
                    .is_some_and(|c_indices| {
                        c_indices.iter().any(|&c_idx| {
                            dsu.find(c_idx) == my_root
                                && dispatch_indices
                                    .get(c_idx)
                                    .and_then(|&si| links.get(si))
                                    .is_some_and(|cl| cl.reference != link.reference)
                        })
                    });
            if (direct_newer || transitive_newer)
                && let Some(self_indices) = ref_to_indices.get(link.reference.as_str())
            {
                for &idx in self_indices {
                    if dsu.find(idx) == my_root
                        && let Some(flag) = is_superseded.get_mut(idx)
                    {
                        *flag = true;
                    }
                }
            }
        }
    }

    type ChainRank = (bool, u64, i64, usize);
    let mut best_by_root: HashMap<usize, (usize, ChainRank)> = HashMap::new();
    for (d_idx, &slice_idx) in dispatch_indices.iter().enumerate() {
        let Some(link) = links.get(slice_idx) else {
            continue;
        };
        let root = dsu.find(d_idx);
        let superseded = is_superseded.get(d_idx).copied().unwrap_or(false);
        let key = (
            !superseded,
            link.added_event,
            dispatch_timestamp_ms(link).unwrap_or(i64::MIN),
            slice_idx,
        );
        match best_by_root.get_mut(&root) {
            Some((best_slice, best_key)) => {
                if key > *best_key {
                    *best_slice = slice_idx;
                    *best_key = key;
                }
            }
            None => {
                best_by_root.insert(root, (slice_idx, key));
            }
        }
    }

    let winners: HashSet<usize> = best_by_root.into_values().map(|(idx, _)| idx).collect();
    links
        .iter()
        .enumerate()
        .filter(|(idx, l)| l.kind != BotItemLinkKind::Dispatch || winners.contains(idx))
        .map(|(_, l)| l.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// BotItemAsk: Validation, Linear-Time parse_label_run, Text & Quick-Reply
// ---------------------------------------------------------------------------

/// Trims, collapses any Unicode whitespace run to single ASCII `' '`, and lowercases.
pub fn fold_ask_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.trim().chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            in_ws = false;
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
        }
    }
    out
}

fn legacy_fold_ask_key(prompt: &str) -> String {
    let stripped: String = prompt.chars().filter(|&c| c != '`').collect();
    fold_ask_key(&stripped)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotItemAskOption {
    pub label: String,
    #[serde(default, alias = "description", skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotItemAskQuestion {
    pub prompt: String,
    pub options: Vec<BotItemAskOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<usize>,
    #[serde(default)]
    pub multi: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotItemAsk {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub questions: Vec<BotItemAskQuestion>,
}

impl BotItemAsk {
    pub fn validate(&self) -> Result<(), BotError> {
        if let Some(ref ctx) = self.context
            && ctx.chars().count() > BOT_ITEM_ASK_LIMITS.context_max
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "Ask context exceeds {} characters.",
                    BOT_ITEM_ASK_LIMITS.context_max
                ),
            ));
        }
        if self.questions.is_empty() || self.questions.len() > BOT_ITEM_ASK_LIMITS.max_questions {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "Ask must have between 1 and {} questions.",
                    BOT_ITEM_ASK_LIMITS.max_questions
                ),
            ));
        }

        let mut seen_prompts: HashSet<String> = HashSet::with_capacity(self.questions.len());
        for q in &self.questions {
            if q.prompt.chars().any(char::is_control) {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    "Ask question prompt cannot contain control characters.",
                ));
            }
            let p_trimmed = q.prompt.trim();
            if p_trimmed.is_empty() || q.prompt.chars().count() > BOT_ITEM_ASK_LIMITS.prompt_max {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!(
                        "Ask question prompt must be 1..={} characters.",
                        BOT_ITEM_ASK_LIMITS.prompt_max
                    ),
                ));
            }
            let p_key = fold_ask_key(p_trimmed);
            if !seen_prompts.insert(p_key) {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    "Ask question prompts must be distinct.",
                ));
            }
            if q.options.is_empty() || q.options.len() > BOT_ITEM_ASK_LIMITS.max_options {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!(
                        "Each ask question must have between 1 and {} options.",
                        BOT_ITEM_ASK_LIMITS.max_options
                    ),
                ));
            }
            if let Some(rec) = q.recommended
                && rec >= q.options.len()
            {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!(
                        "Recommended option index {rec} is out of bounds for {} options.",
                        q.options.len()
                    ),
                ));
            }
            let mut seen_labels: HashSet<String> = HashSet::with_capacity(q.options.len());
            for opt in &q.options {
                if opt.label.chars().any(char::is_control) {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        "Ask option label cannot contain control characters.",
                    ));
                }
                let l_trimmed = opt.label.trim();
                if l_trimmed.is_empty() || opt.label.chars().count() > BOT_ITEM_ASK_LIMITS.label_max {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!(
                            "Ask option label must be 1..={} characters.",
                            BOT_ITEM_ASK_LIMITS.label_max
                        ),
                    ));
                }
                let l_key = fold_ask_key(l_trimmed);
                if !seen_labels.insert(l_key) {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        "Ask option labels within a question must be distinct.",
                    ));
                }
                if let Some(ref detail) = opt.detail
                    && detail.chars().count() > BOT_ITEM_ASK_LIMITS.detail_max
                {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!(
                            "Ask option detail exceeds {} characters.",
                            BOT_ITEM_ASK_LIMITS.detail_max
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn next_start_after_comma(
    norm_s: &str,
    end: usize,
    byte_to_start_idx: &[Option<usize>],
) -> Option<usize> {
    let rest = norm_s.get(end..)?;
    let mut offset = end;
    let mut after_ws = rest;
    if let Some(stripped) = after_ws.strip_prefix(' ') {
        offset += 1;
        after_ws = stripped;
    }
    let after_comma = after_ws.strip_prefix(',')?;
    offset += 1;
    if after_comma.starts_with(' ') {
        offset += 1;
    }
    byte_to_start_idx.get(offset).copied().flatten()
}

/// Parses a comma-separated run of option labels for a `multi: true` question in O(N * k)
/// bottom-up memoized DP time (zero recursion, zero exponential backtracking, UTF-8 safe).
/// Prefers longer comma-containing labels over splitting into shorter labels.
pub fn parse_label_run(input: &str, labels: &[&str]) -> Option<Vec<String>> {
    let norm_s = fold_ask_key(input);
    if norm_s.is_empty() || labels.is_empty() {
        return None;
    }

    let mut seen: HashSet<String> = HashSet::with_capacity(labels.len());
    let mut cands: Vec<(String, &str, usize)> = Vec::with_capacity(labels.len());
    for (orig_idx, &lbl) in labels.iter().enumerate() {
        let folded = fold_ask_key(lbl);
        if !folded.is_empty() && seen.insert(folded.clone()) {
            cands.push((folded, lbl.trim(), orig_idx));
        }
    }
    if cands.is_empty() {
        return None;
    }
    cands.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.2.cmp(&b.2)));

    let mut starts: Vec<usize> = vec![0];
    let mut byte_to_start_idx: Vec<Option<usize>> = vec![None; norm_s.len() + 1];
    if let Some(slot) = byte_to_start_idx.get_mut(0) {
        *slot = Some(0);
    }
    for (byte_idx, ch) in norm_s.char_indices() {
        if ch == ',' {
            let mut next = byte_idx + 1;
            if let Some(after) = norm_s.get(next..)
                && after.starts_with(' ')
            {
                next += 1;
            }
            if next < norm_s.len()
                && norm_s.is_char_boundary(next)
                && byte_to_start_idx.get(next).copied().flatten().is_none()
            {
                let start_idx = starts.len();
                starts.push(next);
                if let Some(slot) = byte_to_start_idx.get_mut(next) {
                    *slot = Some(start_idx);
                }
            }
        }
    }

    let m = starts.len();
    let mut dp: Vec<Option<(usize, usize)>> = vec![None; m];
    for i in (0..m).rev() {
        let Some(&pos) = starts.get(i) else {
            continue;
        };
        let Some(slice) = norm_s.get(pos..) else {
            continue;
        };
        for (ci, (norm_lbl, _, _)) in cands.iter().enumerate() {
            if !slice.starts_with(norm_lbl.as_str()) {
                continue;
            }
            let end = pos + norm_lbl.len();
            if end == norm_s.len() {
                if let Some(slot) = dp.get_mut(i) {
                    *slot = Some((ci, m));
                }
                break;
            }
            if let Some(next_i) = next_start_after_comma(&norm_s, end, &byte_to_start_idx)
                && dp.get(next_i).and_then(|x| *x).is_some()
            {
                if let Some(slot) = dp.get_mut(i) {
                    *slot = Some((ci, next_i));
                }
                break;
            }
        }
    }

    dp.first().and_then(|x| *x)?;
    let mut out = Vec::new();
    let mut cur = 0;
    while cur < m {
        let (ci, next_i) = dp.get(cur).and_then(|x| *x)?;
        let (_, canonical, _) = cands.get(ci)?;
        out.push((*canonical).to_string());
        cur = next_i;
    }
    Some(out)
}

/// Renders a plain-text fallback representation of a structured `BotItemAsk`.
pub fn bot_item_ask_text(ask: &BotItemAsk) -> String {
    let mut sections: Vec<String> = Vec::new();
    if let Some(ref ctx) = ask.context {
        let trimmed = ctx.trim();
        if !trimmed.is_empty() {
            sections.push(trimmed.to_string());
        }
    }
    for q in &ask.questions {
        let mut lines: Vec<String> = Vec::with_capacity(q.options.len() + 1);
        let heading = if q.multi {
            format!("{} (select one or more)", q.prompt.trim())
        } else {
            q.prompt.trim().to_string()
        };
        lines.push(heading);
        for (idx, opt) in q.options.iter().enumerate() {
            let mut line = format!("- {}", opt.label.trim());
            if q.recommended == Some(idx) {
                line.push_str(" (recommended)");
            }
            if let Some(ref detail) = opt.detail {
                let d = detail.trim();
                if !d.is_empty() {
                    line.push_str(" \u{2014} ");
                    line.push_str(d);
                }
            }
            lines.push(line);
        }
        sections.push(lines.join("\n"));
    }
    sections.join("\n\n")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskSelection {
    pub question_index: usize,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedAskAnswer {
    pub question_index: usize,
    pub prompt: String,
    pub labels: Vec<String>,
}

/// Formats structured quick-reply selections as `<QuestionLead> → <Label>` (or comma-joined
/// labels for `multi: true`), one line per selected question.
pub fn format_ask_reply(
    ask: &BotItemAsk,
    selections: &[AskSelection],
) -> Result<String, BotError> {
    ask.validate()?;
    if selections.is_empty() {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "At least one ask selection is required.",
        ));
    }
    let mut seen_questions: HashSet<usize> = HashSet::with_capacity(selections.len());
    let mut lines: Vec<String> = Vec::with_capacity(selections.len());
    for sel in selections {
        if !seen_questions.insert(sel.question_index) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Duplicate selection for question index {}.", sel.question_index),
            ));
        }
        let Some(q) = ask.questions.get(sel.question_index) else {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Question index {} out of bounds.", sel.question_index),
            ));
        };
        if sel.labels.is_empty() || (!q.multi && sel.labels.len() != 1) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "Question '{}' requires {} selection(s).",
                    q.prompt.trim(),
                    if q.multi { "at least one" } else { "exactly one" }
                ),
            ));
        }
        let mut seen: HashSet<String> = HashSet::with_capacity(sel.labels.len());
        let mut canonical_labels: Vec<&str> = Vec::with_capacity(sel.labels.len());
        for raw_lbl in &sel.labels {
            let key = fold_ask_key(raw_lbl);
            let Some(opt) = q.options.iter().find(|o| fold_ask_key(&o.label) == key) else {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!(
                        "Unknown option '{}' for question '{}'.",
                        raw_lbl,
                        q.prompt.trim()
                    ),
                ));
            };
            if !seen.insert(key) {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!("Duplicate option '{}' in multi-select answer.", opt.label.trim()),
                ));
            }
            canonical_labels.push(opt.label.trim());
        }
        lines.push(format!(
            "{}{}{}",
            q.prompt.trim(),
            ASK_REPLY_ARROW,
            canonical_labels.join(", ")
        ));
    }
    Ok(lines.join("\n"))
}

fn parse_rhs_labels(q: &BotItemAskQuestion, rhs: &str) -> Option<Vec<String>> {
    let trimmed = rhs.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !q.multi {
        let target = fold_ask_key(trimmed);
        return q
            .options
            .iter()
            .find(|o| fold_ask_key(&o.label) == target)
            .map(|o| vec![o.label.trim().to_string()]);
    }
    let opt_refs: Vec<&str> = q.options.iter().map(|o| o.label.as_str()).collect();
    let parsed = parse_label_run(trimmed, &opt_refs)?;
    let mut seen: HashSet<String> = HashSet::with_capacity(parsed.len());
    let mut deduped: Vec<String> = Vec::with_capacity(parsed.len());
    for lbl in parsed {
        if seen.insert(fold_ask_key(&lbl)) {
            deduped.push(lbl);
        }
    }
    (!deduped.is_empty()).then_some(deduped)
}

struct AskPromptHead {
    cur: String,
    cur_chars: usize,
    leg: String,
    leg_chars: usize,
}

type AskMatchCandidate = (usize, usize, Vec<String>);

fn scan_arrow_matches(
    folded_line: &str,
    arrow: &str,
    ask: &BotItemAsk,
    heads: &[AskPromptHead],
    max_head_bytes: usize,
    best_current: &mut Option<AskMatchCandidate>,
    best_legacy: &mut Option<AskMatchCandidate>,
) {
    let arrow_len = arrow.len();
    for (idx, _) in folded_line.match_indices(arrow) {
        let (Some(lhs), Some(rhs)) = (
            folded_line.get(..idx),
            folded_line.get(idx + arrow_len..),
        ) else {
            continue;
        };
        let lhs_key = lhs.trim_end();
        if lhs_key.is_empty() {
            continue;
        }
        if lhs_key.len() > max_head_bytes {
            break;
        }
        for (q_idx, head) in heads.iter().enumerate() {
            let Some(q) = ask.questions.get(q_idx) else {
                continue;
            };
            if lhs_key == head.cur
                && best_current
                    .as_ref()
                    .is_none_or(|(best_len, _, _)| head.cur_chars > *best_len)
                && let Some(labels) = parse_rhs_labels(q, rhs)
            {
                *best_current = Some((head.cur_chars, q_idx, labels));
            } else if head.leg != head.cur
                && lhs_key == head.leg
                && best_legacy
                    .as_ref()
                    .is_none_or(|(best_len, _, _)| head.leg_chars > *best_len)
                && let Some(labels) = parse_rhs_labels(q, rhs)
            {
                *best_legacy = Some((head.leg_chars, q_idx, labels));
            }
        }
    }
}

/// Parses owner quick-reply text against `ask` in linear `O(N)` time per line.
/// Longest matching current prompt head wins; legacy backtick-stripped lead matches
/// only when strictly longer than every matching current prompt head.
pub fn parse_ask_reply(ask: &BotItemAsk, reply_text: &str) -> Vec<ParsedAskAnswer> {
    let heads: Vec<AskPromptHead> = ask
        .questions
        .iter()
        .map(|q| {
            let cur = fold_ask_key(&q.prompt);
            let cur_chars = cur.chars().count();
            let leg = legacy_fold_ask_key(&q.prompt);
            let leg_chars = leg.chars().count();
            AskPromptHead {
                cur,
                cur_chars,
                leg,
                leg_chars,
            }
        })
        .collect();
    let max_head_bytes = heads
        .iter()
        .map(|h| h.cur.len().max(h.leg.len()))
        .max()
        .unwrap_or(0);

    let mut answers: Vec<ParsedAskAnswer> = Vec::new();
    for raw_line in reply_text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let folded_line = fold_ask_key(line);
        if folded_line.is_empty() {
            continue;
        }

        let mut best_current: Option<AskMatchCandidate> = None;
        let mut best_legacy: Option<AskMatchCandidate> = None;

        if max_head_bytes > 0 {
            scan_arrow_matches(
                &folded_line,
                "\u{2192}",
                ask,
                &heads,
                max_head_bytes,
                &mut best_current,
                &mut best_legacy,
            );
            scan_arrow_matches(
                &folded_line,
                "->",
                ask,
                &heads,
                max_head_bytes,
                &mut best_current,
                &mut best_legacy,
            );
        }

        let chosen = match (best_current, best_legacy) {
            (Some(cur), Some(leg)) if leg.0 > cur.0 => Some((leg.1, leg.2)),
            (Some(cur), _) => Some((cur.1, cur.2)),
            (None, Some(leg)) => Some((leg.1, leg.2)),
            (None, None) => None,
        };

        let matched = chosen.or_else(|| {
            if ask.questions.len() == 1
                && let Some(q) = ask.questions.first()
                && let Some(labels) = parse_rhs_labels(q, &folded_line)
            {
                Some((0, labels))
            } else {
                None
            }
        });

        if let Some((q_idx, labels)) = matched
            && let Some(q) = ask.questions.get(q_idx)
        {
            let answer = ParsedAskAnswer {
                question_index: q_idx,
                prompt: q.prompt.trim().to_string(),
                labels,
            };
            if let Some(existing) = answers.iter_mut().find(|a| a.question_index == q_idx) {
                *existing = answer;
            } else {
                answers.push(answer);
            }
        }
    }
    answers
}

// ---------------------------------------------------------------------------
// BotItemEvent & BotItem Domain Model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotItemMailState {
    Owed,
    Sent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotItemEventKind {
    Created,
    StatusChanged,
    Commented,
    Linked,
    Unlinked,
    Snoozed,
    Unsnoozed,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BotItemEventData {
    Created,
    StatusChanged {
        from: BotItemStatus,
        to: BotItemStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ask: Option<BotItemAsk>,
    },
    Commented {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
        #[serde(default, alias = "questionEvent", skip_serializing_if = "Option::is_none")]
        question_event: Option<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        answers: Vec<ParsedAskAnswer>,
    },
    Linked {
        kind: BotItemLinkKind,
        #[serde(rename = "ref", alias = "reference")]
        reference: String,
    },
    Unlinked {
        kind: BotItemLinkKind,
        #[serde(rename = "ref", alias = "reference")]
        reference: String,
    },
    Snoozed {
        #[serde(alias = "untilMs")]
        until_ms: i64,
    },
    Unsnoozed {
        early: bool,
    },
    Updated {
        fields: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotItemEvent {
    pub id: u64,
    #[serde(alias = "itemUid")]
    pub item_uid: String,
    pub kind: BotItemEventKind,
    pub actor: BotItemActor,
    pub data: BotItemEventData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail: Option<BotItemMailState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(alias = "createdAtMs", alias = "createdAt")]
    pub created_at_ms: i64,
}

impl BotItemEvent {
    pub const fn is_early_unsnooze(&self) -> bool {
        matches!(self.data, BotItemEventData::Unsnoozed { early: true })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotItem {
    pub uid: String,
    pub number: u64,
    #[serde(alias = "ownerBotId")]
    pub owner_bot_id: String,
    #[serde(default, alias = "chatId", skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    #[serde(default, alias = "groupId", skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, alias = "madeBy", skip_serializing_if = "Option::is_none")]
    pub made_by: Option<BotItemActor>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, alias = "doneMeans", skip_serializing_if = "Option::is_none")]
    pub done_means: Option<String>,
    #[serde(default)]
    pub status: BotItemStatus,
    #[serde(default, alias = "questionEvent", skip_serializing_if = "Option::is_none")]
    pub question_event: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<BotItemAsk>,
    #[serde(default, alias = "waitEvent", skip_serializing_if = "Option::is_none")]
    pub wait_event: Option<u64>,
    #[serde(default, alias = "waitNote", skip_serializing_if = "Option::is_none")]
    pub wait_note: Option<String>,
    #[serde(default, alias = "closeNote", skip_serializing_if = "Option::is_none")]
    pub close_note: Option<String>,
    pub version: u64,
    #[serde(default, alias = "lastEventId")]
    pub last_event_id: u64,
    #[serde(default, alias = "othersLastEventId", skip_serializing_if = "Option::is_none")]
    pub others_last_event_id: Option<u64>,
    #[serde(default, alias = "snoozedUntilMs", alias = "snoozedUntil", skip_serializing_if = "Option::is_none")]
    pub snoozed_until_ms: Option<i64>,
    #[serde(alias = "lastActivityAtMs", alias = "lastActivityAt")]
    pub last_activity_at_ms: i64,
    #[serde(alias = "createdAtMs", alias = "createdAt")]
    pub created_at_ms: i64,
    #[serde(alias = "updatedAtMs", alias = "updatedAt")]
    pub updated_at_ms: i64,
    #[serde(default, alias = "closedAtMs", alias = "closedAt", skip_serializing_if = "Option::is_none")]
    pub closed_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<BotItemLink>,
}

impl BotItem {
    pub fn validate(&self) -> Result<(), BotError> {
        if self.uid.trim().is_empty()
            || self.uid.chars().any(char::is_control)
            || self.number == 0
            || self.owner_bot_id.trim().is_empty()
            || self.owner_bot_id.chars().any(char::is_control)
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Item requires non-empty uid and owner_bot_id without control characters, and number >= 1.",
            ));
        }
        if self.title.chars().any(char::is_control) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Item title cannot contain control characters.",
            ));
        }
        if self.title.trim().is_empty() || self.title.chars().count() > BOT_ITEM_LIMITS.title {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Item title must be 1..={} characters.", BOT_ITEM_LIMITS.title),
            ));
        }
        if let Some(ref d) = self.description
            && d.chars().count() > BOT_ITEM_LIMITS.description
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Item description exceeds {} characters.", BOT_ITEM_LIMITS.description),
            ));
        }
        if let Some(ref dm) = self.done_means
            && dm.chars().count() > BOT_ITEM_LIMITS.done_means
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Item done_means exceeds {} characters.", BOT_ITEM_LIMITS.done_means),
            ));
        }
        if let Some(ref w) = self.wait_note
            && w.chars().count() > BOT_ITEM_LIMITS.wait
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Item wait_note exceeds {} characters.", BOT_ITEM_LIMITS.wait),
            ));
        }
        if let Some(ref cn) = self.close_note
            && cn.chars().count() > BOT_ITEM_LIMITS.text
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Item close_note exceeds {} characters.", BOT_ITEM_LIMITS.text),
            ));
        }
        if self.links.len() > BOT_ITEM_LIMITS.links {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Item links exceed limit of {}.", BOT_ITEM_LIMITS.links),
            ));
        }
        for link in &self.links {
            link.validate()?;
        }
        if let Some(ref ask) = self.ask {
            ask.validate()?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Snooze, Stale Detection & State Transitions
// ---------------------------------------------------------------------------

pub const fn is_snooze_active(snoozed_until_ms: Option<i64>, now_ms: i64) -> bool {
    matches!(snoozed_until_ms, Some(until) if until > now_ms)
}

pub fn validate_snooze_until(snoozed_until_ms: i64, now_ms: i64) -> Result<(), BotError> {
    if snoozed_until_ms <= now_ms {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Snooze timestamp must be strictly in the future.",
        ));
    }
    Ok(())
}

/// Returns the stale threshold in days for `status` under `policy`, or `None`
/// if closed or `BotItemsStale::Never`. `Planned` items get `3×` the base days.
pub const fn stale_after_days(status: BotItemStatus, policy: BotItemsStale) -> Option<u32> {
    if !status.is_open() {
        return None;
    }
    let Some(base) = policy.days() else {
        return None;
    };
    match status {
        BotItemStatus::Planned => Some(base.saturating_mul(PLANNED_STALE_MULTIPLIER)),
        BotItemStatus::Working | BotItemStatus::Waiting | BotItemStatus::NeedsYou => Some(base),
        BotItemStatus::Done | BotItemStatus::Canceled => None,
    }
}

/// Determines whether `item` is stale at `now_ms` using overflow-safe saturating math.
pub fn is_item_stale(item: &BotItem, policy: BotItemsStale, now_ms: i64) -> bool {
    if !item.status.is_open() || is_snooze_active(item.snoozed_until_ms, now_ms) {
        return false;
    }
    let Some(days) = stale_after_days(item.status, policy) else {
        return false;
    };
    let expired_snooze = item
        .snoozed_until_ms
        .filter(|&until| until <= now_ms)
        .unwrap_or(i64::MIN);
    let anchor = item.last_activity_at_ms.max(expired_snooze);
    if now_ms < anchor {
        return false;
    }
    let threshold_ms = i64::from(days).saturating_mul(MS_PER_DAY);
    now_ms.saturating_sub(anchor) >= threshold_ms
}

/// Inspects `events` in reverse to determine if the most recent status transition
/// into `Canceled` was performed by the `Owner`.
pub fn was_last_canceled_by_owner(events: &[BotItemEvent]) -> bool {
    for ev in events.iter().rev() {
        if let BotItemEventData::StatusChanged {
            from,
            to: BotItemStatus::Canceled,
            ..
        } = ev.data
            && from != BotItemStatus::Canceled
        {
            return ev.actor.is_owner();
        }
    }
    false
}

/// Enforces cross-bot ownership: bots may only mutate items they own.
pub fn check_item_write_permission(item: &BotItem, actor: &BotItemActor) -> Result<(), BotError> {
    if let Some(bot_id) = actor.bot_id()
        && bot_id != item.owner_bot_id
    {
        return Err(BotError::new(
            BotErrorCode::NotOwner,
            format!(
                "Bot '{bot_id}' does not own item #{} (owned by '{}').",
                item.number, item.owner_bot_id
            ),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusTransitionRequest {
    pub actor: BotItemActor,
    pub to_status: BotItemStatus,
    pub expected_version: Option<u64>,
    pub wait_note: Option<String>,
    pub close_note: Option<String>,
    pub note: Option<String>,
    pub ask: Option<BotItemAsk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemTransitionOutcome {
    pub emitted_events: Vec<BotItemEvent>,
    pub unsnoozed_early: bool,
}

/// Applies a validated status transition to `item`, enforcing ownership, optimistic
/// concurrency, owner-cancel reopen protection, field limits, and snooze auto-end.
pub fn apply_status_transition(
    item: &mut BotItem,
    history: &[BotItemEvent],
    req: StatusTransitionRequest,
    now_ms: i64,
) -> Result<ItemTransitionOutcome, BotError> {
    check_item_write_permission(item, &req.actor)?;

    if let Some(expected) = req.expected_version
        && expected != item.version
    {
        return Err(BotError::new(
            BotErrorCode::ItemChanged,
            format!(
                "Item #{} changed concurrently (expected version {expected}, current {}).",
                item.number, item.version
            ),
        ));
    }

    if item.status == BotItemStatus::Canceled
        && !req.actor.is_owner()
        && was_last_canceled_by_owner(history)
    {
        return Err(BotError::new(
            BotErrorCode::ItemClosed,
            format!(
                "Item #{} was canceled by the owner and cannot be modified or reopened by a bot. Post a comment asking the owner before reopening.",
                item.number
            ),
        ));
    }

    let wait_note = req
        .wait_note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let close_note = req
        .close_note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let note = req
        .note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    if let Some(ref w) = wait_note
        && w.chars().count() > BOT_ITEM_LIMITS.wait
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("wait_note exceeds {} characters.", BOT_ITEM_LIMITS.wait),
        ));
    }
    if let Some(ref cn) = close_note
        && cn.chars().count() > BOT_ITEM_LIMITS.text
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("close_note exceeds {} characters.", BOT_ITEM_LIMITS.text),
        ));
    }
    if let Some(ref n) = note
        && n.chars().count() > BOT_ITEM_LIMITS.text
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("note exceeds {} characters.", BOT_ITEM_LIMITS.text),
        ));
    }
    if let Some(ref ask) = req.ask {
        ask.validate()?;
    }

    let from_status = item.status;
    let status_event_id = item.last_event_id.saturating_add(1);
    item.last_event_id = status_event_id;
    item.version = item.version.saturating_add(1);
    item.status = req.to_status;
    item.updated_at_ms = now_ms;
    item.last_activity_at_ms = now_ms;

    if req.to_status == BotItemStatus::Waiting {
        item.wait_event = Some(status_event_id);
        item.wait_note = wait_note;
    } else {
        item.wait_event = None;
        item.wait_note = None;
    }

    if req.to_status == BotItemStatus::NeedsYou {
        item.question_event = Some(status_event_id);
        item.ask = req.ask;
    } else {
        item.question_event = None;
        item.ask = None;
    }

    if req.to_status.is_closed() {
        item.closed_at_ms = Some(now_ms);
        item.close_note = close_note.or_else(|| note.clone());
    } else {
        item.closed_at_ms = None;
        item.close_note = None;
    }

    let status_event = BotItemEvent {
        id: status_event_id,
        item_uid: item.uid.clone(),
        kind: BotItemEventKind::StatusChanged,
        actor: req.actor.clone(),
        data: BotItemEventData::StatusChanged {
            from: from_status,
            to: req.to_status,
            note: note
                .or_else(|| item.wait_note.clone())
                .or_else(|| item.close_note.clone()),
            ask: item.ask.clone(),
        },
        mail: None,
        action: None,
        created_at_ms: now_ms,
    };

    let mut emitted_events = vec![status_event];
    let mut unsnoozed_early = false;

    if req.to_status.auto_ends_snooze() {
        if is_snooze_active(item.snoozed_until_ms, now_ms) {
            item.snoozed_until_ms = None;
            unsnoozed_early = true;
            let unsnooze_id = item.last_event_id.saturating_add(1);
            item.last_event_id = unsnooze_id;
            emitted_events.push(BotItemEvent {
                id: unsnooze_id,
                item_uid: item.uid.clone(),
                kind: BotItemEventKind::Unsnoozed,
                actor: req.actor,
                data: BotItemEventData::Unsnoozed { early: true },
                mail: None,
                action: None,
                created_at_ms: now_ms,
            });
        } else if item.snoozed_until_ms.is_some() {
            item.snoozed_until_ms = None;
        }
    }

    Ok(ItemTransitionOutcome {
        emitted_events,
        unsnoozed_early,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemCommentRequest {
    pub actor: BotItemActor,
    pub text: String,
    pub files: Vec<String>,
    pub question_event: Option<u64>,
}

/// Appends a comment event (and quick-reply answer transition when owner replies to `NeedsYou`).
pub fn apply_item_comment(
    item: &mut BotItem,
    req: ItemCommentRequest,
    now_ms: i64,
) -> Result<BotItemEvent, BotError> {
    check_item_write_permission(item, &req.actor)?;

    let trimmed = req.text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > BOT_ITEM_LIMITS.text {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Comment text must be 1..={} characters.", BOT_ITEM_LIMITS.text),
        ));
    }
    if req.files.len() > BOT_ITEM_LIMITS.comment_files {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!(
                "Comment cannot attach more than {} files.",
                BOT_ITEM_LIMITS.comment_files
            ),
        ));
    }

    if let Some(qid) = req.question_event
        && (item.status != BotItemStatus::NeedsYou || item.question_event != Some(qid))
    {
        return Err(BotError::new(
            BotErrorCode::QuestionClosed,
            format!(
                "Question event {qid} on item #{} is already closed.",
                item.number
            ),
        ));
    }

    let answers = if req.actor.is_owner()
        && item.status == BotItemStatus::NeedsYou
        && let Some(ref ask) = item.ask
    {
        parse_ask_reply(ask, trimmed)
    } else {
        Vec::new()
    };

    let event_id = item.last_event_id.saturating_add(1);
    item.last_event_id = event_id;
    item.updated_at_ms = now_ms;
    item.last_activity_at_ms = now_ms;

    let mail = if req.actor.is_owner() {
        item.others_last_event_id = Some(event_id);
        if item.status == BotItemStatus::NeedsYou {
            item.status = BotItemStatus::Working;
            item.question_event = None;
            item.ask = None;
            item.version = item.version.saturating_add(1);
        }
        Some(BotItemMailState::Owed)
    } else {
        None
    };

    Ok(BotItemEvent {
        id: event_id,
        item_uid: item.uid.clone(),
        kind: BotItemEventKind::Commented,
        actor: req.actor,
        data: BotItemEventData::Commented {
            text: trimmed.to_string(),
            files: req.files,
            question_event: req.question_event,
            answers,
        },
        mail,
        action: None,
        created_at_ms: now_ms,
    })
}

/// Snoozes an open item until `until_ms` (`until_ms > now_ms`).
pub fn apply_snooze(
    item: &mut BotItem,
    actor: BotItemActor,
    until_ms: i64,
    now_ms: i64,
) -> Result<BotItemEvent, BotError> {
    check_item_write_permission(item, &actor)?;
    if !item.status.is_open() {
        return Err(BotError::new(
            BotErrorCode::ItemClosed,
            format!("Closed item #{} cannot be snoozed.", item.number),
        ));
    }
    validate_snooze_until(until_ms, now_ms)?;
    let event_id = item.last_event_id.saturating_add(1);
    item.last_event_id = event_id;
    item.snoozed_until_ms = Some(until_ms);
    item.updated_at_ms = now_ms;
    Ok(BotItemEvent {
        id: event_id,
        item_uid: item.uid.clone(),
        kind: BotItemEventKind::Snoozed,
        actor,
        data: BotItemEventData::Snoozed { until_ms },
        mail: None,
        action: None,
        created_at_ms: now_ms,
    })
}

/// Clears snooze on an item, recording `early: true` only when snooze had not yet expired.
pub fn apply_unsnooze(
    item: &mut BotItem,
    actor: BotItemActor,
    now_ms: i64,
) -> Result<BotItemEvent, BotError> {
    check_item_write_permission(item, &actor)?;
    let early = is_snooze_active(item.snoozed_until_ms, now_ms);
    item.snoozed_until_ms = None;
    item.updated_at_ms = now_ms;
    let event_id = item.last_event_id.saturating_add(1);
    item.last_event_id = event_id;
    Ok(BotItemEvent {
        id: event_id,
        item_uid: item.uid.clone(),
        kind: BotItemEventKind::Unsnoozed,
        actor,
        data: BotItemEventData::Unsnoozed { early },
        mail: None,
        action: None,
        created_at_ms: now_ms,
    })
}

// ---------------------------------------------------------------------------
// needs_you_fold Priority Folding & Active Needs-You Filtering
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedsYouKind {
    Approval,
    Question,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeedsYouFoldResult<T> {
    pub visible: Vec<T>,
    pub folded: Vec<T>,
    pub hidden_count: usize,
    pub approval_count: usize,
    pub focused_preserved: bool,
}

/// Priority-folds urgent `NeedsYou` entries:
/// - Approvals are NEVER hidden (even when `approvals.len() > cap`).
/// - Non-approval slots are capped to `cap.saturating_sub(approvals.len())`.
/// - If `focused_id` lands in `folded`, it is promoted to `visible` (displacing the last
///   visible non-approval if any, else appended without ever hiding an approval).
pub fn needs_you_fold<T: Clone, FId, FApproval>(
    items: &[T],
    focused_id: Option<&str>,
    cap: usize,
    id_of: FId,
    is_approval: FApproval,
) -> NeedsYouFoldResult<T>
where
    FId: Fn(&T) -> &str,
    FApproval: Fn(&T) -> bool,
{
    let mut approvals: Vec<T> = Vec::new();
    let mut regular: Vec<T> = Vec::new();
    for item in items {
        if is_approval(item) {
            approvals.push(item.clone());
        } else {
            regular.push(item.clone());
        }
    }

    let approval_count = approvals.len();
    let regular_slots = cap.saturating_sub(approval_count);

    let mut visible: Vec<T> = approvals;
    let mut folded: Vec<T> = Vec::new();
    for (idx, item) in regular.into_iter().enumerate() {
        if idx < regular_slots {
            visible.push(item);
        } else {
            folded.push(item);
        }
    }

    let mut focused_preserved = false;
    if let Some(fid) = focused_id
        && let Some(pos) = folded.iter().position(|it| id_of(it) == fid)
    {
        let focused_item = folded.remove(pos);
        focused_preserved = true;
        if regular_slots > 0 && visible.len() > approval_count {
            if let Some(displaced) = visible.pop() {
                folded.insert(0, displaced);
            }
            visible.push(focused_item);
        } else {
            visible.push(focused_item);
        }
    }

    let hidden_count = folded.len();
    NeedsYouFoldResult {
        visible,
        folded,
        hidden_count,
        approval_count,
        focused_preserved,
    }
}

fn open_status_priority(status: BotItemStatus) -> u8 {
    match status {
        BotItemStatus::NeedsYou => 0,
        BotItemStatus::Waiting => 1,
        BotItemStatus::Working => 2,
        BotItemStatus::Planned => 3,
        BotItemStatus::Done | BotItemStatus::Canceled => 4,
    }
}

/// Filters out closed and actively snoozed items, sorting `NeedsYou` strictly ahead
/// of `Waiting`, `Working`, and `Planned` (ties preserve original input order).
pub fn filter_and_sort_needs_you_items(items: &[BotItem], now_ms: i64) -> Vec<&BotItem> {
    let mut active: Vec<&BotItem> = items
        .iter()
        .filter(|it| it.status.is_open() && !is_snooze_active(it.snoozed_until_ms, now_ms))
        .collect();
    active.sort_by_key(|it| open_status_priority(it.status));
    active
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn sample_item(number: u64, status: BotItemStatus) -> BotItem {
        BotItem {
            uid: format!("item-{number}"),
            number,
            owner_bot_id: "bot-alpha".to_string(),
            chat_id: Some("chat-1".to_string()),
            group_id: None,
            source: Some("chat".to_string()),
            made_by: Some(BotItemActor::Owner),
            title: format!("Item #{number}"),
            description: None,
            done_means: None,
            status,
            question_event: None,
            ask: None,
            wait_event: None,
            wait_note: None,
            close_note: None,
            version: 1,
            last_event_id: 1,
            others_last_event_id: None,
            snoozed_until_ms: None,
            last_activity_at_ms: 1_000_000,
            created_at_ms: 1_000_000,
            updated_at_ms: 1_000_000,
            closed_at_ms: None,
            links: Vec::new(),
        }
    }

    #[test]
    fn status_wire_contract_and_cli_alias() {
        let expected = ["planned", "working", "waiting", "needs_you", "done", "canceled"];
        assert_eq!(BotItemStatus::ALL.len(), 6);
        for (st, wire) in BotItemStatus::ALL.into_iter().zip(expected) {
            assert_eq!(st.as_str(), wire);
            assert_eq!(BotItemStatus::parse_strict(wire), Some(st));
            assert_eq!(BotItemStatus::parse_cli(wire), Some(st));
            let json = serde_json::to_string(&st).expect("serialize status");
            assert_eq!(json, format!("\"{wire}\""));
            let back: BotItemStatus = serde_json::from_str(&json).expect("deserialize status");
            assert_eq!(back, st);
        }
        assert_eq!(BotItemStatus::parse_strict("needs-you"), None);
        assert_eq!(BotItemStatus::parse_cli("needs-you"), Some(BotItemStatus::NeedsYou));
        assert_eq!(
            serde_json::from_str::<BotItemStatus>("\"needs-you\"").expect("alias"),
            BotItemStatus::NeedsYou
        );
        assert_eq!(BotItemStatus::parse_strict("archived"), None);
        assert_eq!(BotItemStatus::parse_strict("cancelled"), None);
        assert_eq!(normalize_list_page_limit(None), 50);
        assert_eq!(normalize_list_page_limit(Some(0)), 1);
        assert_eq!(normalize_list_page_limit(Some(500)), 100);
    }

    #[test]
    fn bi_01_unicode_char_vs_byte_limits() {
        let mut item = sample_item(1, BotItemStatus::Planned);
        item.title = "测".repeat(200);
        assert_eq!(item.title.len(), 600);
        assert!(item.validate().is_ok());

        item.title = "测".repeat(201);
        assert_eq!(item.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        item.title = "ok".to_string();
        item.wait_note = Some("🔥".repeat(200));
        assert_eq!(item.wait_note.as_ref().unwrap().len(), 800);
        assert!(item.validate().is_ok());
        item.wait_note = Some("🔥".repeat(201));
        assert_eq!(item.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        item.wait_note = None;
        item.description = Some("é".repeat(20_000));
        assert!(item.validate().is_ok());
        item.description = Some("é".repeat(20_001));
        assert_eq!(item.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        item.description = None;
        item.done_means = Some("中".repeat(2_000));
        assert!(item.validate().is_ok());
        item.done_means = Some("中".repeat(2_001));
        assert_eq!(item.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        item.done_means = None;
        item.close_note = Some("🚀".repeat(8_000));
        assert!(item.validate().is_ok());
        item.close_note = Some("🚀".repeat(8_001));
        assert_eq!(item.validate().unwrap_err().code, BotErrorCode::InvalidArgument);
    }

    #[test]
    fn bi_02_ask_bounds_uniqueness_and_description_alias() {
        let make_q = |idx: usize| BotItemAskQuestion {
            prompt: format!("Question {idx}?"),
            options: vec![
                BotItemAskOption {
                    label: "Yes".to_string(),
                    detail: None,
                },
                BotItemAskOption {
                    label: "No".to_string(),
                    detail: Some("Skip".to_string()),
                },
            ],
            recommended: Some(0),
            multi: false,
        };

        // 0 questions -> err; 5 -> ok; 6 -> err
        let empty_ask = BotItemAsk {
            context: None,
            questions: vec![],
        };
        assert_eq!(empty_ask.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let five_ask = BotItemAsk {
            context: Some("字".repeat(8_000)),
            questions: (0..5).map(make_q).collect(),
        };
        assert!(five_ask.validate().is_ok());

        let six_ask = BotItemAsk {
            context: None,
            questions: (0..6).map(make_q).collect(),
        };
        assert_eq!(six_ask.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let over_ctx = BotItemAsk {
            context: Some("字".repeat(8_001)),
            questions: vec![make_q(0)],
        };
        assert_eq!(over_ctx.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        // 8 options ok, 9 err
        let mut q8 = make_q(0);
        q8.options = (0..8)
            .map(|i| BotItemAskOption {
                label: format!("Opt {i}"),
                detail: Some("d".repeat(500)),
            })
            .collect();
        q8.prompt = "p".repeat(500);
        q8.recommended = Some(7);
        assert!(BotItemAsk { context: None, questions: vec![q8.clone()] }.validate().is_ok());

        let mut q9 = q8.clone();
        q9.options.push(BotItemAskOption { label: "Opt 8".to_string(), detail: None });
        assert_eq!(
            BotItemAsk { context: None, questions: vec![q9] }.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );

        // OOB recommended, prompt 501, label 151, detail 501, dup prompts & dup labels
        let mut bad_rec = make_q(0);
        bad_rec.recommended = Some(2);
        assert_eq!(
            BotItemAsk { context: None, questions: vec![bad_rec] }.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );
        let mut dup_lbl = make_q(0);
        dup_lbl.options[1].label = "  yes ".to_string();
        assert_eq!(
            BotItemAsk { context: None, questions: vec![dup_lbl] }.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );
        let mut q_dup_prompt_1 = make_q(0);
        q_dup_prompt_1.prompt = "Deploy?".to_string();
        let mut q_dup_prompt_2 = make_q(1);
        q_dup_prompt_2.prompt = "  deploy? ".to_string();
        assert_eq!(
            BotItemAsk { context: None, questions: vec![q_dup_prompt_1, q_dup_prompt_2] }
                .validate()
                .unwrap_err()
                .code,
            BotErrorCode::InvalidArgument
        );
        let mut long_lbl = make_q(0);
        long_lbl.options[0].label = "x".repeat(150);
        assert!(BotItemAsk { context: None, questions: vec![long_lbl.clone()] }.validate().is_ok());
        long_lbl.options[0].label = "x".repeat(151);
        assert_eq!(
            BotItemAsk { context: None, questions: vec![long_lbl] }.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );
        let mut long_det = make_q(0);
        long_det.options[0].detail = Some("y".repeat(501));
        assert_eq!(
            BotItemAsk { context: None, questions: vec![long_det] }.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );
        let mut long_prompt = make_q(0);
        long_prompt.prompt = "z".repeat(501);
        assert_eq!(
            BotItemAsk { context: None, questions: vec![long_prompt] }.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );

        // `description` serde alias for `detail`
        let opt: BotItemAskOption =
            serde_json::from_str(r#"{"label":"Ship","description":"Merge now"}"#).expect("alias");
        assert_eq!(opt.detail.as_deref(), Some("Merge now"));
    }

    #[test]
    fn bi_03_and_bi_04_parse_label_run_linear_and_utf8_safe() {
        let labels = ["a", "a, a", "a, a, a"];
        let adversarial = format!("{}b", "a, ".repeat(60));
        let t0 = Instant::now();
        assert_eq!(parse_label_run(&adversarial, &labels), None);
        assert!(t0.elapsed().as_millis() < 50);

        // 5,000-token stress test
        let huge_adv = format!("{}X", "a, ".repeat(5_000));
        let t1 = Instant::now();
        assert_eq!(parse_label_run(&huge_adv, &["a", "a, a"]), None);
        assert!(t1.elapsed().as_millis() < 200);

        assert_eq!(
            parse_label_run("Red, blue, Green", &["Red, blue", "Green"]),
            Some(vec!["Red, blue".to_string(), "Green".to_string()])
        );
        assert_eq!(
            parse_label_run("a, a, a", &["a", "a, a"]),
            Some(vec!["a, a".to_string(), "a".to_string()])
        );

        // BI-04 multi-byte UTF-8 boundary safety
        let utf8_labels = ["日本語, テスト", "✅ 承認", "日本"];
        assert_eq!(
            parse_label_run("日本語, テスト, ✅ 承認", &utf8_labels),
            Some(vec!["日本語, テスト".to_string(), "✅ 承認".to_string()])
        );
        assert_eq!(parse_label_run("日本語X", &utf8_labels), None);
    }
}

#[cfg(test)]
mod extended_tests {
    use super::*;

    fn sample_item(number: u64, status: BotItemStatus) -> BotItem {
        BotItem {
            uid: format!("item-{number}"),
            number,
            owner_bot_id: "bot-alpha".to_string(),
            chat_id: Some("chat-1".to_string()),
            group_id: None,
            source: Some("chat".to_string()),
            made_by: Some(BotItemActor::Owner),
            title: format!("Item #{number}"),
            description: None,
            done_means: None,
            status,
            question_event: None,
            ask: None,
            wait_event: None,
            wait_note: None,
            close_note: None,
            version: 1,
            last_event_id: 1,
            others_last_event_id: None,
            snoozed_until_ms: None,
            last_activity_at_ms: 1_000_000,
            created_at_ms: 1_000_000,
            updated_at_ms: 1_000_000,
            closed_at_ms: None,
            links: Vec::new(),
        }
    }

    #[test]
    fn bi_05_ask_reply_head_collision_and_legacy_lead_precedence() {
        let ask = BotItemAsk {
            context: Some("Rollout approval".to_string()),
            questions: vec![
                BotItemAskQuestion {
                    prompt: "Deploy".to_string(),
                    options: vec![
                        BotItemAskOption { label: "Yes".to_string(), detail: None },
                        BotItemAskOption { label: "No".to_string(), detail: None },
                    ],
                    recommended: Some(0),
                    multi: false,
                },
                BotItemAskQuestion {
                    prompt: "Deploy `prod`".to_string(),
                    options: vec![
                        BotItemAskOption { label: "Yes".to_string(), detail: Some("Ship".to_string()) },
                        BotItemAskOption { label: "Hold".to_string(), detail: None },
                    ],
                    recommended: Some(0),
                    multi: false,
                },
            ],
        };
        let fallback = bot_item_ask_text(&ask);
        assert!(fallback.contains("Rollout approval"));
        assert!(fallback.contains("- Yes (recommended)"));

        let formatted = format_ask_reply(
            &ask,
            &[AskSelection {
                question_index: 1,
                labels: vec!["Yes".to_string()],
            }],
        )
        .expect("format");
        assert_eq!(formatted, "Deploy `prod` \u{2192} Yes");

        let parsed_cur = parse_ask_reply(&ask, &formatted);
        assert_eq!(parsed_cur.len(), 1);
        assert_eq!(parsed_cur[0].question_index, 1);
        assert_eq!(parsed_cur[0].labels, vec!["Yes"]);

        let parsed_legacy = parse_ask_reply(&ask, "Deploy prod \u{2192} Yes");
        assert_eq!(parsed_legacy.len(), 1);
        assert_eq!(parsed_legacy[0].question_index, 1);

        // When an exact current prompt equals another question's legacy prompt, current wins!
        let tie_ask = BotItemAsk {
            context: None,
            questions: vec![
                BotItemAskQuestion {
                    prompt: "Deploy prod".to_string(),
                    options: vec![BotItemAskOption { label: "Yes".to_string(), detail: None }],
                    recommended: None,
                    multi: false,
                },
                BotItemAskQuestion {
                    prompt: "Deploy `prod`".to_string(),
                    options: vec![BotItemAskOption { label: "Yes".to_string(), detail: None }],
                    recommended: None,
                    multi: false,
                },
            ],
        };
        let parsed_tie = parse_ask_reply(&tie_ask, "Deploy prod \u{2192} Yes");
        assert_eq!(parsed_tie[0].question_index, 0);
    }

    #[test]
    fn bi_06_owner_cancel_guard_and_auth_and_cas_and_question_closed() {
        let mut item = sample_item(42, BotItemStatus::Working);
        let mut history: Vec<BotItemEvent> = Vec::new();
        let owner = BotItemActor::Owner;
        let own_bot = BotItemActor::Bot { bot_id: "bot-alpha".to_string() };
        let other_bot = BotItemActor::Bot { bot_id: "bot-beta".to_string() };

        // Cross-bot NotOwner
        let err = apply_status_transition(
            &mut item,
            &history,
            StatusTransitionRequest {
                actor: other_bot,
                to_status: BotItemStatus::Done,
                expected_version: None,
                wait_note: None,
                close_note: None,
                note: None,
                ask: None,
            },
            2_000,
        )
        .unwrap_err();
        assert_eq!(err.code, BotErrorCode::NotOwner);

        // Optimistic concurrency ItemChanged
        let err = apply_status_transition(
            &mut item,
            &history,
            StatusTransitionRequest {
                actor: own_bot.clone(),
                to_status: BotItemStatus::Done,
                expected_version: Some(99),
                wait_note: None,
                close_note: None,
                note: None,
                ask: None,
            },
            2_000,
        )
        .unwrap_err();
        assert_eq!(err.code, BotErrorCode::ItemChanged);

        // (a) Owner cancels -> Bot cannot reopen (ItemClosed mentioning "comment" & "owner")
        let out = apply_status_transition(
            &mut item,
            &history,
            StatusTransitionRequest {
                actor: owner.clone(),
                to_status: BotItemStatus::Canceled,
                expected_version: Some(1),
                wait_note: None,
                close_note: Some("No longer needed".to_string()),
                note: None,
                ask: None,
            },
            3_000,
        )
        .expect("owner cancel");
        history.extend(out.emitted_events);

        // (d) Bot CAN comment on owner-canceled item, and STILL cannot reopen afterward!
        let c_ev = apply_item_comment(
            &mut item,
            ItemCommentRequest {
                actor: own_bot.clone(),
                text: "Should I reopen this?".to_string(),
                files: vec![],
                question_event: None,
            },
            4_000,
        )
        .expect("bot comment allowed");
        assert_eq!(c_ev.mail, None);
        history.push(c_ev);

        let reopen_err = apply_status_transition(
            &mut item,
            &history,
            StatusTransitionRequest {
                actor: own_bot.clone(),
                to_status: BotItemStatus::Working,
                expected_version: None,
                wait_note: None,
                close_note: None,
                note: None,
                ask: None,
            },
            5_000,
        )
        .unwrap_err();
        assert_eq!(reopen_err.code, BotErrorCode::ItemClosed);
        assert!(reopen_err.message.to_lowercase().contains("comment"));
        assert!(reopen_err.message.to_lowercase().contains("owner"));

        // (b) Owner reopens -> Bot can then transition
        let out = apply_status_transition(
            &mut item,
            &history,
            StatusTransitionRequest {
                actor: owner.clone(),
                to_status: BotItemStatus::Working,
                expected_version: None,
                wait_note: None,
                close_note: None,
                note: None,
                ask: None,
            },
            6_000,
        )
        .expect("owner reopen");
        history.extend(out.emitted_events);

        // (c) Bot cancels -> Bot CAN reopen
        let out = apply_status_transition(
            &mut item,
            &history,
            StatusTransitionRequest {
                actor: own_bot.clone(),
                to_status: BotItemStatus::Canceled,
                expected_version: None,
                wait_note: None,
                close_note: None,
                note: None,
                ask: None,
            },
            7_000,
        )
        .expect("bot cancel");
        history.extend(out.emitted_events);

        let out = apply_status_transition(
            &mut item,
            &history,
            StatusTransitionRequest {
                actor: own_bot.clone(),
                to_status: BotItemStatus::NeedsYou,
                expected_version: None,
                wait_note: None,
                close_note: None,
                note: None,
                ask: None,
            },
            8_000,
        )
        .expect("bot reopens bot-canceled");
        history.extend(out.emitted_events);
        let qid = item.question_event.expect("question_event");

        // Owner replies -> closes question and transitions to Working; replay -> QuestionClosed
        let reply = apply_item_comment(
            &mut item,
            ItemCommentRequest {
                actor: owner.clone(),
                text: "Go ahead".to_string(),
                files: vec![],
                question_event: Some(qid),
            },
            9_000,
        )
        .expect("owner answer");
        assert_eq!(reply.mail, Some(BotItemMailState::Owed));
        assert_eq!(item.status, BotItemStatus::Working);

        let replay_err = apply_item_comment(
            &mut item,
            ItemCommentRequest {
                actor: owner,
                text: "Replay".to_string(),
                files: vec![],
                question_event: Some(qid),
            },
            10_000,
        )
        .unwrap_err();
        assert_eq!(replay_err.code, BotErrorCode::QuestionClosed);
    }
}

#[cfg(test)]
mod snooze_links_fold_tests {
    use super::*;

    fn sample_item(number: u64, status: BotItemStatus) -> BotItem {
        BotItem {
            uid: format!("item-{number}"),
            number,
            owner_bot_id: "bot-alpha".to_string(),
            chat_id: None,
            group_id: None,
            source: None,
            made_by: Some(BotItemActor::Owner),
            title: format!("Item #{number}"),
            description: None,
            done_means: None,
            status,
            question_event: None,
            ask: None,
            wait_event: None,
            wait_note: None,
            close_note: None,
            version: 1,
            last_event_id: 1,
            others_last_event_id: None,
            snoozed_until_ms: None,
            last_activity_at_ms: 1_000_000,
            created_at_ms: 1_000_000,
            updated_at_ms: 1_000_000,
            closed_at_ms: None,
            links: Vec::new(),
        }
    }

    #[test]
    fn bi_07_snooze_and_early_unsnooze_transitions() {
        let t0 = 10_000_000;
        let until = t0 + MS_PER_DAY;
        for target in [BotItemStatus::NeedsYou, BotItemStatus::Done, BotItemStatus::Canceled] {
            let mut item = sample_item(1, BotItemStatus::NeedsYou);
            apply_snooze(&mut item, BotItemActor::Owner, until, t0).expect("snooze");
            let out = apply_status_transition(
                &mut item,
                &[],
                StatusTransitionRequest {
                    actor: BotItemActor::Owner,
                    to_status: target,
                    expected_version: None,
                    wait_note: None,
                    close_note: None,
                    note: None,
                    ask: None,
                },
                t0 + 1_000,
            )
            .expect("transition");
            assert_eq!(item.snoozed_until_ms, None);
            assert!(out.unsnoozed_early);
            assert!(out.emitted_events.iter().any(BotItemEvent::is_early_unsnooze));
        }

        // Working keeps active snooze intact
        let mut item = sample_item(2, BotItemStatus::Waiting);
        apply_snooze(&mut item, BotItemActor::Owner, until, t0).expect("snooze");
        let out = apply_status_transition(
            &mut item,
            &[],
            StatusTransitionRequest {
                actor: BotItemActor::Owner,
                to_status: BotItemStatus::Working,
                expected_version: None,
                wait_note: None,
                close_note: None,
                note: None,
                ask: None,
            },
            t0 + 1_000,
        )
        .expect("working");
        assert_eq!(item.snoozed_until_ms, Some(until));
        assert!(!out.unsnoozed_early);

        // Transition after snooze expiry does NOT emit early: true
        let out2 = apply_status_transition(
            &mut item,
            &[],
            StatusTransitionRequest {
                actor: BotItemActor::Owner,
                to_status: BotItemStatus::Done,
                expected_version: None,
                wait_note: None,
                close_note: None,
                note: None,
                ask: None,
            },
            until,
        )
        .expect("after expiry");
        assert!(!out2.unsnoozed_early);
        assert!(!out2.emitted_events.iter().any(BotItemEvent::is_early_unsnooze));
        assert_eq!(item.snoozed_until_ms, None);
    }

    #[test]
    fn bi_08_stale_thresholds_planned_3x_and_clock_skew() {
        assert_eq!(stale_after_days(BotItemStatus::Planned, BotItemsStale::Days3), Some(9));
        assert_eq!(stale_after_days(BotItemStatus::Planned, BotItemsStale::Days5), Some(15));
        assert_eq!(stale_after_days(BotItemStatus::Planned, BotItemsStale::Days7), Some(21));
        assert_eq!(stale_after_days(BotItemStatus::Planned, BotItemsStale::Days14), Some(42));
        assert_eq!(stale_after_days(BotItemStatus::Planned, BotItemsStale::Never), None);
        assert_eq!(stale_after_days(BotItemStatus::Working, BotItemsStale::Days5), Some(5));
        assert_eq!(stale_after_days(BotItemStatus::Done, BotItemsStale::Days3), None);

        let mut planned = sample_item(1, BotItemStatus::Planned);
        planned.last_activity_at_ms = 0;
        let fifteen_days = 15 * MS_PER_DAY;
        assert!(!is_item_stale(&planned, BotItemsStale::Days5, fifteen_days - 1));
        assert!(is_item_stale(&planned, BotItemsStale::Days5, fifteen_days));
        assert!(!is_item_stale(&planned, BotItemsStale::Never, fifteen_days * 10));

        // Clock skew & extreme timestamps never panic
        let mut extreme = sample_item(2, BotItemStatus::Working);
        extreme.last_activity_at_ms = i64::MAX - 10;
        assert!(!is_item_stale(&extreme, BotItemsStale::Days3, i64::MAX));
        assert!(!is_item_stale(&extreme, BotItemsStale::Days3, 0));

        // Active snooze suppresses stale
        let mut snoozed = sample_item(3, BotItemStatus::NeedsYou);
        snoozed.last_activity_at_ms = 0;
        snoozed.snoozed_until_ms = Some(10 * MS_PER_DAY);
        assert!(!is_item_stale(&snoozed, BotItemsStale::Days3, 9 * MS_PER_DAY));
    }

    #[test]
    fn bi_09_links_one_per_chain_and_1000_cap() {
        let mk_disp = |r: &str, cid: Option<&str>, parent: Option<&str>, latest: Option<&str>, ev: u64| BotItemLink {
            kind: BotItemLinkKind::Dispatch,
            reference: r.to_string(),
            added_event: ev,
            meta: BotItemLinkMeta::Dispatch(DispatchLinkMeta {
                chain_id: cid.map(str::to_string),
                parent_dispatch_id: parent.map(str::to_string),
                latest_dispatch_id: latest.map(str::to_string),
                ..Default::default()
            }),
        };
        let mk_pr = |r: &str| BotItemLink {
            kind: BotItemLinkKind::Pr,
            reference: r.to_string(),
            added_event: 1,
            meta: BotItemLinkMeta::Pr(PrLinkMeta::default()),
        };
        let mk_url = |r: &str| BotItemLink {
            kind: BotItemLinkKind::Url,
            reference: r.to_string(),
            added_event: 1,
            meta: BotItemLinkMeta::Url(UrlLinkMeta::default()),
        };

        // latest_dispatch_id dedup
        let links = vec![
            mk_disp("d1", None, None, Some("d3"), 1),
            mk_pr("org/repo#1"),
            mk_disp("d2", None, None, Some("d3"), 2),
            mk_pr("org/repo#2"),
            mk_disp("d3", None, None, None, 3),
            mk_url("https://example.com/issue/1"),
        ];
        let deduped = bot_item_links_one_per_chain(&links);
        assert_eq!(deduped.len(), 4);
        assert_eq!(deduped[0].reference, "org/repo#1");
        assert_eq!(deduped[1].reference, "org/repo#2");
        assert_eq!(deduped[2].reference, "d3");
        assert_eq!(deduped[3].reference, "https://example.com/issue/1");

        // chain_id + parent_dispatch_id + out-of-order + cycle safety
        let chain_links = vec![
            mk_disp("run-2", Some("chain-1"), Some("run-1"), None, 2),
            mk_disp("run-1", Some("chain-1"), None, None, 1),
            mk_disp("cyc-a", None, Some("cyc-b"), None, 10),
            mk_disp("cyc-b", None, Some("cyc-a"), None, 11),
        ];
        let deduped_chain = bot_item_links_one_per_chain(&chain_links);
        let refs: Vec<&str> = deduped_chain.iter().map(|l| l.reference.as_str()).collect();
        assert_eq!(refs, vec!["run-2", "cyc-b"]);

        // 1,000 link cap on validate (InvalidArgument) and add_item_link (Full)
        let mut item = sample_item(9, BotItemStatus::Working);
        for i in 0..1_000 {
            add_item_link(&mut item.links, mk_url(&format!("https://ex.com/{i}"))).expect("add");
        }
        assert!(item.validate().is_ok());
        assert_eq!(
            add_item_link(&mut item.links, mk_url("https://ex.com/overflow")).unwrap_err().code,
            BotErrorCode::Full
        );
        item.links.push(mk_url("https://ex.com/overflow"));
        assert_eq!(item.validate().unwrap_err().code, BotErrorCode::InvalidArgument);
    }

    #[test]
    fn bi_10_needs_you_fold_and_schema_version_guard() {
        assert!(check_items_schema_version(3, 3).is_ok());
        assert!(check_items_schema_version(2, 3).is_ok());
        assert_eq!(
            check_items_schema_version(4, 3).unwrap_err().code,
            BotErrorCode::ItemsReadOnly
        );

        let now = 10_000;
        let mut ny_open = sample_item(1, BotItemStatus::NeedsYou);
        let mut ny_snoozed = sample_item(2, BotItemStatus::NeedsYou);
        ny_snoozed.snoozed_until_ms = Some(now + 1_000);
        let mut ny_expired = sample_item(3, BotItemStatus::NeedsYou);
        ny_expired.snoozed_until_ms = Some(now - 1);
        let waiting = sample_item(4, BotItemStatus::Waiting);
        let done = sample_item(5, BotItemStatus::Done);
        let canceled = sample_item(6, BotItemStatus::Canceled);
        ny_open.uid = "ny-open".to_string();
        ny_snoozed.uid = "ny-snoozed".to_string();
        ny_expired.uid = "ny-expired".to_string();

        let all = vec![waiting, ny_snoozed, done, ny_open, canceled, ny_expired];
        let filtered = filter_and_sort_needs_you_items(&all, now);
        let uids: Vec<&str> = filtered.iter().map(|i| i.uid.as_str()).collect();
        assert_eq!(uids, vec!["ny-open", "ny-expired", "item-4"]);

        // needs_you_fold: 2 approvals + 6 regular, cap 5, focused item inside folded
        let entries: Vec<(&str, bool)> = vec![
            ("r1", false),
            ("a1", true),
            ("r2", false),
            ("a2", true),
            ("r3", false),
            ("r4", false),
            ("r5", false),
            ("r6", false),
        ];
        let res = needs_you_fold(&entries, Some("r5"), NEEDS_YOU_FOLD_CAP, |e| e.0, |e| e.1);
        assert_eq!(res.approval_count, 2);
        assert!(res.focused_preserved);
        assert_eq!(res.visible.len(), 5);
        assert!(res.visible.iter().any(|e| e.0 == "a1"));
        assert!(res.visible.iter().any(|e| e.0 == "a2"));
        assert!(res.visible.iter().any(|e| e.0 == "r5"));
        assert_eq!(res.hidden_count, 3);

        // 7 approvals > cap 5: ALL 7 approvals unhidden, focused regular promoted without hiding approvals
        let many_approvals: Vec<(&str, bool)> = vec![
            ("a1", true), ("a2", true), ("a3", true), ("a4", true),
            ("a5", true), ("a6", true), ("a7", true), ("r1", false), ("r2", false),
        ];
        let res2 = needs_you_fold(&many_approvals, Some("r2"), NEEDS_YOU_FOLD_CAP, |e| e.0, |e| e.1);
        assert_eq!(res2.approval_count, 7);
        assert_eq!(res2.visible.len(), 8);
        assert!(res2.focused_preserved);
        assert_eq!(res2.hidden_count, 1);
        assert_eq!(res2.folded[0].0, "r1");
    }

    #[test]
    fn bi_11_remediation_regressions() {
        // Finding #1: Owner-cancel guard cannot be bypassed via Canceled -> Canceled
        let mut item = sample_item(77, BotItemStatus::Working);
        let owner = BotItemActor::Owner;
        let own_bot = BotItemActor::Bot {
            bot_id: "bot-alpha".to_string(),
        };
        let mut history: Vec<BotItemEvent> = Vec::new();
        let cancel_out = apply_status_transition(
            &mut item,
            &history,
            StatusTransitionRequest {
                actor: owner,
                to_status: BotItemStatus::Canceled,
                expected_version: None,
                wait_note: None,
                close_note: Some("Owner says do NOT ship".to_string()),
                note: None,
                ask: None,
            },
            2_000,
        )
        .expect("owner cancel");
        history.extend(cancel_out.emitted_events);
        assert!(was_last_canceled_by_owner(&history));

        let recancel_err = apply_status_transition(
            &mut item,
            &history,
            StatusTransitionRequest {
                actor: own_bot.clone(),
                to_status: BotItemStatus::Canceled,
                expected_version: None,
                wait_note: None,
                close_note: None,
                note: None,
                ask: None,
            },
            3_000,
        )
        .unwrap_err();
        assert_eq!(recancel_err.code, BotErrorCode::ItemClosed);
        assert!(recancel_err.message.to_lowercase().contains("comment"));
        assert!(recancel_err.message.to_lowercase().contains("owner"));
        assert_eq!(item.close_note.as_deref(), Some("Owner says do NOT ship"));

        // Synthetic non-owner Canceled -> Canceled event in history cannot erase owner attribution
        history.push(BotItemEvent {
            id: 99,
            item_uid: item.uid.clone(),
            kind: BotItemEventKind::StatusChanged,
            actor: own_bot.clone(),
            data: BotItemEventData::StatusChanged {
                from: BotItemStatus::Canceled,
                to: BotItemStatus::Canceled,
                note: None,
                ask: None,
            },
            mail: None,
            action: None,
            created_at_ms: 3_500,
        });
        assert!(was_last_canceled_by_owner(&history));
        assert_eq!(
            apply_status_transition(
                &mut item,
                &history,
                StatusTransitionRequest {
                    actor: own_bot,
                    to_status: BotItemStatus::Working,
                    expected_version: None,
                    wait_note: None,
                    close_note: None,
                    note: None,
                    ask: None,
                },
                4_000,
            )
            .unwrap_err()
            .code,
            BotErrorCode::ItemClosed
        );

        // Finding #2: Duplicate dispatch refs & missing-parent siblings
        let mk_disp = |r: &str, parent: Option<&str>, ev: u64| BotItemLink {
            kind: BotItemLinkKind::Dispatch,
            reference: r.to_string(),
            added_event: ev,
            meta: BotItemLinkMeta::Dispatch(DispatchLinkMeta {
                parent_dispatch_id: parent.map(str::to_string),
                ..Default::default()
            }),
        };
        let dup1 = bot_item_links_one_per_chain(&[
            mk_disp("d1", None, 1),
            mk_disp("d2", Some("d1"), 2),
            mk_disp("d1", None, 10),
        ]);
        assert_eq!(dup1.len(), 1);
        assert_eq!(dup1[0].reference, "d2");

        let dup2 = bot_item_links_one_per_chain(&[
            mk_disp("d1", None, 1),
            mk_disp("d1", None, 5),
            mk_disp("d2", Some("d1"), 2),
        ]);
        assert_eq!(dup2.len(), 1);
        assert_eq!(dup2[0].reference, "d2");

        let siblings = bot_item_links_one_per_chain(&[
            mk_disp("d1", Some("missing-parent"), 1),
            mk_disp("d2", Some("missing-parent"), 2),
        ]);
        assert_eq!(siblings.len(), 1);
        assert_eq!(siblings[0].reference, "d2");

        // Finding #3: format_ask_reply dup question rejection, round-trip arrow collision,
        // single-question shorthand with arrows, and duplicate-line last-wins dedup
        let collision_ask = BotItemAsk {
            context: None,
            questions: vec![
                BotItemAskQuestion {
                    prompt: "Deploy".to_string(),
                    options: vec![
                        BotItemAskOption { label: "prod \u{2192} Yes".to_string(), detail: None },
                        BotItemAskOption { label: "No".to_string(), detail: None },
                    ],
                    recommended: None,
                    multi: false,
                },
                BotItemAskQuestion {
                    prompt: "Deploy \u{2192} prod".to_string(),
                    options: vec![BotItemAskOption { label: "Rollout".to_string(), detail: None }],
                    recommended: None,
                    multi: false,
                },
            ],
        };
        assert_eq!(
            format_ask_reply(
                &collision_ask,
                &[
                    AskSelection { question_index: 0, labels: vec!["No".to_string()] },
                    AskSelection { question_index: 0, labels: vec!["prod \u{2192} Yes".to_string()] },
                ],
            )
            .unwrap_err()
            .code,
            BotErrorCode::InvalidArgument
        );
        let formatted = format_ask_reply(
            &collision_ask,
            &[AskSelection {
                question_index: 0,
                labels: vec!["prod \u{2192} Yes".to_string()],
            }],
        )
        .expect("format arrow option");
        assert_eq!(formatted, "Deploy \u{2192} prod \u{2192} Yes");
        let parsed = parse_ask_reply(&collision_ask, &formatted);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].question_index, 0);
        assert_eq!(parsed[0].labels, vec!["prod \u{2192} Yes"]);

        let deduped_reply = parse_ask_reply(
            &collision_ask,
            "Deploy \u{2192} prod \u{2192} Yes\r\nDeploy \u{2192} No\r\n",
        );
        assert_eq!(deduped_reply.len(), 1);
        assert_eq!(deduped_reply[0].question_index, 0);
        assert_eq!(deduped_reply[0].labels, vec!["No"]);

        let single_arrow_ask = BotItemAsk {
            context: None,
            questions: vec![BotItemAskQuestion {
                prompt: "Choose migration".to_string(),
                options: vec![
                    BotItemAskOption { label: "v1 -> v2".to_string(), detail: None },
                    BotItemAskOption { label: "v2 \u{2192} v3".to_string(), detail: None },
                ],
                recommended: None,
                multi: false,
            }],
        };
        let p_ascii = parse_ask_reply(&single_arrow_ask, "v1 -> v2");
        assert_eq!(p_ascii.len(), 1);
        assert_eq!(p_ascii[0].labels, vec!["v1 -> v2"]);
        let p_unicode = parse_ask_reply(&single_arrow_ask, "v2 \u{2192} v3");
        assert_eq!(p_unicode.len(), 1);
        assert_eq!(p_unicode[0].labels, vec!["v2 \u{2192} v3"]);

        // Finding #4: trimmed note storage, wait_note event fallback, raw title char validation
        let mut item2 = sample_item(78, BotItemStatus::Working);
        let padded_note = format!("{}trimmed{}", " ".repeat(100_000), " ".repeat(50));
        let out_pad = apply_status_transition(
            &mut item2,
            &[],
            StatusTransitionRequest {
                actor: BotItemActor::Owner,
                to_status: BotItemStatus::Working,
                expected_version: None,
                wait_note: None,
                close_note: None,
                note: Some(padded_note),
                ask: None,
            },
            5_000,
        )
        .expect("padded note trimmed");
        match &out_pad.emitted_events[0].data {
            BotItemEventData::StatusChanged { note, .. } => {
                assert_eq!(note.as_deref(), Some("trimmed"));
            }
            other => panic!("expected StatusChanged, got {other:?}"),
        }

        let out_wait = apply_status_transition(
            &mut item2,
            &[],
            StatusTransitionRequest {
                actor: BotItemActor::Owner,
                to_status: BotItemStatus::Waiting,
                expected_version: None,
                wait_note: Some("  waiting on CI  ".to_string()),
                close_note: None,
                note: None,
                ask: None,
            },
            6_000,
        )
        .expect("wait transition");
        assert_eq!(item2.wait_note.as_deref(), Some("waiting on CI"));
        match &out_wait.emitted_events[0].data {
            BotItemEventData::StatusChanged { note, .. } => {
                assert_eq!(note.as_deref(), Some("waiting on CI"));
            }
            other => panic!("expected StatusChanged, got {other:?}"),
        }

        let mut padded_title_item = sample_item(79, BotItemStatus::Planned);
        padded_title_item.title = format!("{}x", " ".repeat(250));
        assert_eq!(
            padded_title_item.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );
    }

    #[test]
    fn bi_12_iter2_remediation_regressions() {
        use std::time::Instant;

        // BUG-I2-01: O(N) parse_ask_reply on adversarial 8,000-char arrow payloads (< 50ms)
        let ask = BotItemAsk {
            context: None,
            questions: vec![BotItemAskQuestion {
                prompt: "Deploy".to_string(),
                options: vec![
                    BotItemAskOption {
                        label: "Yes".to_string(),
                        detail: None,
                    },
                    BotItemAskOption {
                        label: "No".to_string(),
                        detail: None,
                    },
                ],
                recommended: Some(0),
                multi: false,
            }],
        };

        let unicode_arrows = "\u{2192}".repeat(8_000);
        let t0 = Instant::now();
        assert!(parse_ask_reply(&ask, &unicode_arrows).is_empty());
        assert!(
            t0.elapsed().as_millis() < 50,
            "8,000 Unicode arrows took {:?}",
            t0.elapsed()
        );

        let ascii_arrows = "->".repeat(4_000);
        let t1 = Instant::now();
        assert!(parse_ask_reply(&ask, &ascii_arrows).is_empty());
        assert!(
            t1.elapsed().as_millis() < 50,
            "4,000 ASCII arrows took {:?}",
            t1.elapsed()
        );

        let mixed_arrows = format!("Deploy {}Yes", "-> ".repeat(5_000));
        let t2 = Instant::now();
        assert!(parse_ask_reply(&ask, &mixed_arrows).is_empty());
        assert!(
            t2.elapsed().as_millis() < 50,
            "5,000 mixed arrows took {:?}",
            t2.elapsed()
        );

        // Also via apply_item_comment on legal 8,000-char comment
        let mut ny_item = sample_item(80, BotItemStatus::NeedsYou);
        ny_item.ask = Some(ask.clone());
        let t3 = Instant::now();
        let comment_res = apply_item_comment(
            &mut ny_item,
            ItemCommentRequest {
                actor: BotItemActor::Owner,
                text: unicode_arrows,
                files: vec![],
                question_event: None,
            },
            10_000,
        );
        assert!(comment_res.is_ok());
        assert!(
            t3.elapsed().as_millis() < 50,
            "apply_item_comment 8,000 arrows took {:?}",
            t3.elapsed()
        );

        // Mixed-arrow ordering: trailing Unicode arrow far after ASCII arrow must NOT cut off ASCII arrow match
        let mixed_arrow_opt_ask = BotItemAsk {
            context: None,
            questions: vec![BotItemAskQuestion {
                prompt: "Deploy".to_string(),
                options: vec![BotItemAskOption {
                    label: "Yes \u{2192} trailing".to_string(),
                    detail: None,
                }],
                recommended: None,
                multi: false,
            }],
        };
        let mixed_parsed = parse_ask_reply(&mixed_arrow_opt_ask, "Deploy -> Yes \u{2192} trailing");
        assert_eq!(mixed_parsed.len(), 1);
        assert_eq!(mixed_parsed[0].labels, vec!["Yes \u{2192} trailing"]);

        // BUG-I2-03: Symmetric latest_dispatch_id supersession & pruned-middle latest/parent join
        let mk_disp = |r: &str, parent: Option<&str>, latest: Option<&str>, ev: u64| BotItemLink {
            kind: BotItemLinkKind::Dispatch,
            reference: r.to_string(),
            added_event: ev,
            meta: BotItemLinkMeta::Dispatch(DispatchLinkMeta {
                parent_dispatch_id: parent.map(str::to_string),
                latest_dispatch_id: latest.map(str::to_string),
                ..Default::default()
            }),
        };

        let pruned_join = bot_item_links_one_per_chain(&[
            mk_disp("d1", None, Some("d0"), 1),
            mk_disp("d2", Some("d0"), None, 2),
        ]);
        assert_eq!(pruned_join.len(), 1);
        assert_eq!(pruned_join[0].reference, "d2");
        assert_eq!(pruned_join[0].added_event, 2);

        let pruned_join_ooo = bot_item_links_one_per_chain(&[
            mk_disp("d1", None, Some("d0"), 5),
            mk_disp("d2", Some("d0"), None, 2),
        ]);
        assert_eq!(pruned_join_ooo.len(), 1);
        assert_eq!(pruned_join_ooo[0].reference, "d2");
        assert_eq!(pruned_join_ooo[0].added_event, 2);

        let latest_dup = bot_item_links_one_per_chain(&[
            mk_disp("d1", None, Some("d2"), 1),
            mk_disp("d1", None, None, 5),
            mk_disp("d2", None, None, 2),
        ]);
        assert_eq!(latest_dup.len(), 1);
        assert_eq!(latest_dup[0].reference, "d2");
        assert_eq!(latest_dup[0].added_event, 2);

        let latest_dup_rev = bot_item_links_one_per_chain(&[
            mk_disp("d1", None, None, 5),
            mk_disp("d1", None, Some("d2"), 1),
            mk_disp("d2", None, None, 2),
        ]);
        assert_eq!(latest_dup_rev.len(), 1);
        assert_eq!(latest_dup_rev[0].reference, "d2");
        assert_eq!(latest_dup_rev[0].added_event, 2);

        // BUG-I2-04 (1): BotItemAsk::validate raw char caps reject space-padded oversized fields
        let mut padded_prompt = ask.clone();
        padded_prompt.questions[0].prompt = format!("{}Q?", " ".repeat(100_000));
        assert_eq!(
            padded_prompt.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );

        let mut padded_label = ask.clone();
        padded_label.questions[0].options[0].label = format!("{}Yes", " ".repeat(100_000));
        assert_eq!(
            padded_label.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );

        let mut padded_detail = ask.clone();
        padded_detail.questions[0].options[0].detail = Some(format!("{}Detail", " ".repeat(100_000)));
        assert_eq!(
            padded_detail.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );

        // BUG-I2-04 (2): Whitespace-only notes normalize to None & never shadow fallbacks
        let mut item = sample_item(81, BotItemStatus::Working);
        let wait_out = apply_status_transition(
            &mut item,
            &[],
            StatusTransitionRequest {
                actor: BotItemActor::Owner,
                to_status: BotItemStatus::Waiting,
                expected_version: None,
                wait_note: Some("waiting on CI".to_string()),
                close_note: None,
                note: Some("   \t  ".to_string()),
                ask: None,
            },
            11_000,
        )
        .expect("waiting with ws-only note");
        assert_eq!(item.wait_note.as_deref(), Some("waiting on CI"));
        match &wait_out.emitted_events[0].data {
            BotItemEventData::StatusChanged { note, .. } => {
                assert_eq!(note.as_deref(), Some("waiting on CI"));
            }
            other => panic!("expected StatusChanged, got {other:?}"),
        }

        let wait_empty = apply_status_transition(
            &mut item,
            &[],
            StatusTransitionRequest {
                actor: BotItemActor::Owner,
                to_status: BotItemStatus::Waiting,
                expected_version: None,
                wait_note: Some("   ".to_string()),
                close_note: None,
                note: Some("   ".to_string()),
                ask: None,
            },
            11_500,
        )
        .expect("waiting with all ws-only notes");
        assert_eq!(item.wait_note, None);
        match &wait_empty.emitted_events[0].data {
            BotItemEventData::StatusChanged { note, .. } => {
                assert_eq!(*note, None);
            }
            other => panic!("expected StatusChanged, got {other:?}"),
        }

        let cancel_out = apply_status_transition(
            &mut item,
            &[],
            StatusTransitionRequest {
                actor: BotItemActor::Owner,
                to_status: BotItemStatus::Canceled,
                expected_version: None,
                wait_note: None,
                close_note: Some("   ".to_string()),
                note: Some("Real cancel reason".to_string()),
                ask: None,
            },
            12_000,
        )
        .expect("canceled with ws-only close_note");
        assert_eq!(item.close_note.as_deref(), Some("Real cancel reason"));
        match &cancel_out.emitted_events[0].data {
            BotItemEventData::StatusChanged { note, .. } => {
                assert_eq!(note.as_deref(), Some("Real cancel reason"));
            }
            other => panic!("expected StatusChanged, got {other:?}"),
        }

        // BUG-I2-04 (3): Non-NeedsYou transition does NOT leak req.ask into StatusChanged.ask
        let work_out = apply_status_transition(
            &mut item,
            &[],
            StatusTransitionRequest {
                actor: BotItemActor::Owner,
                to_status: BotItemStatus::Working,
                expected_version: None,
                wait_note: None,
                close_note: None,
                note: None,
                ask: Some(ask.clone()),
            },
            13_000,
        )
        .expect("working with ask ignored");
        assert_eq!(item.ask, None);
        match &work_out.emitted_events[0].data {
            BotItemEventData::StatusChanged { ask: ev_ask, .. } => {
                assert_eq!(*ev_ask, None);
            }
            other => panic!("expected StatusChanged, got {other:?}"),
        }

        let ny_out = apply_status_transition(
            &mut item,
            &[],
            StatusTransitionRequest {
                actor: BotItemActor::Owner,
                to_status: BotItemStatus::NeedsYou,
                expected_version: None,
                wait_note: None,
                close_note: None,
                note: None,
                ask: Some(ask.clone()),
            },
            14_000,
        )
        .expect("needs_you keeps ask");
        assert_eq!(item.ask.as_ref(), Some(&ask));
        match &ny_out.emitted_events[0].data {
            BotItemEventData::StatusChanged { ask: ev_ask, .. } => {
                assert_eq!(ev_ask.as_ref(), Some(&ask));
            }
            other => panic!("expected StatusChanged, got {other:?}"),
        }

        // BUG-I2-04 (4): BotItem::validate rejects control characters in uid and owner_bot_id
        let mut ctrl_uid = sample_item(82, BotItemStatus::Planned);
        ctrl_uid.uid = "item\u{0000}17".to_string();
        assert_eq!(
            ctrl_uid.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );

        let mut ctrl_owner = sample_item(83, BotItemStatus::Planned);
        ctrl_owner.owner_bot_id = "bot\nalpha".to_string();
        assert_eq!(
            ctrl_owner.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );
    }
}
