//! Cross-app Bot Import, Workspace Hand-Over, Bot Chat Tree, 10 Interactive
//! Native Chat Bot Cards, and Usage-Limit Auto-Continue contracts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use crate::bot_ecosystem::package_scanner::{
    BotTextRiskFinding, redact_blocking_secrets, scan_bot_text_risks,
};
use crate::bot_ecosystem::skills::is_valid_skill_slug;
use crate::bot_types::{BotFolderSlug, BotImportedFrom, BotName, bot_folder_name};
use crate::{BOT_NAME_MAX_LENGTH, BotError, BotErrorCode};

// ---------------------------------------------------------------------------
// 1. Cross-App Bot Import (OpenClaw, Hermes, OpenBot, CustomFolder, ExportPaste)
// ---------------------------------------------------------------------------

pub const BOT_IMPORT_MAX_BOTS_PER_SOURCE: usize = 24;
pub const BOT_IMPORT_EXPORT_MAX_BYTES: usize = 200 * 1024;

/// Copy-pasteable prompt for exporting a foreign bot cleanly without leaking secrets.
pub const BOT_IMPORT_EXPORT_PROMPT: &str = "\
Export your bot persona and working notes as plain Markdown so I can import you into Orca.
Include:
1. `# <Bot Name>` on the first line.
2. A short summary of your role and tone.
3. `## How I work` with concrete bullet points describing your habits and workflow.
4. `## Never` with hard guardrails and actions you refuse without owner confirmation.
5. Optional `## Memory` with one non-secret durable fact per `- ` bullet line.
6. Optional `## Skills` listing lowercase kebab-case skill slugs (one per bullet).

Security rule: NEVER include API keys, passwords, private keys, cookies, tokens, webhook secrets, or personal identifiers. Replace any sensitive value with `<REDACTED>` before outputting.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BotImportSource {
    #[serde(rename = "openclaw", alias = "open_claw", alias = "open-claw")]
    OpenClaw,
    Hermes,
    #[serde(rename = "openbot", alias = "open_bot", alias = "open-bot")]
    OpenBot,
    CustomFolder,
    ExportPaste,
}

impl BotImportSource {
    pub const ALL: [Self; 5] = [
        Self::OpenClaw,
        Self::Hermes,
        Self::OpenBot,
        Self::CustomFolder,
        Self::ExportPaste,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenClaw => "openclaw",
            Self::Hermes => "hermes",
            Self::OpenBot => "openbot",
            Self::CustomFolder => "custom-folder",
            Self::ExportPaste => "export-paste",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let lower = raw.trim().to_ascii_lowercase();
        match lower.as_str() {
            "openclaw" | "open-claw" | "open_claw" => Some(Self::OpenClaw),
            "hermes" => Some(Self::Hermes),
            "openbot" | "open-bot" | "open_bot" => Some(Self::OpenBot),
            "custom-folder" | "custom_folder" => Some(Self::CustomFolder),
            "export-paste" | "export_paste" | "export" => Some(Self::ExportPaste),
            _ => None,
        }
    }

    /// Read-only default discovery paths / env hints for supported foreign apps.
    pub const fn default_discovery_paths(self) -> &'static [&'static str] {
        match self {
            Self::OpenClaw => &["~/.openclaw", "~/.clawdbot", "~/.moltbot", "$OPENCLAW_STATE_DIR"],
            Self::Hermes => &["~/.hermes", "$HERMES_HOME"],
            Self::OpenBot => &["~/OpenBot/Agents"],
            Self::CustomFolder | Self::ExportPaste => &[],
        }
    }
}

impl fmt::Display for BotImportSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Sanitized candidate produced by parsing a foreign bot export or folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotImportCandidate {
    pub source: BotImportSource,
    pub original_name: String,
    pub suggested_name: BotName,
    pub suggested_folder: BotFolderSlug,
    pub soul_md: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_md: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_slugs: Vec<String>,
    pub provenance: BotImportedFrom,
    pub redacted_secrets_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk_warnings: Vec<BotTextRiskFinding>,
}

fn sanitize_bot_name(raw: &str, fallback: &str) -> Result<BotName, BotError> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let truncated: String = cleaned.chars().take(BOT_NAME_MAX_LENGTH).collect();
    let trimmed = truncated.trim();
    if let Ok(name) = BotName::new(trimmed) {
        return Ok(name);
    }
    BotName::new(fallback)
        .or_else(|_| BotName::new("Imported bot"))
        .map_err(|code| BotError::new(code, "Failed to derive valid BotName."))
}

fn split_export_sections(export_text: &str) -> Vec<(Option<String>, String)> {
    let mut blocks: Vec<(Option<String>, Vec<&str>)> = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for raw_line in export_text.lines() {
        let trimmed = raw_line.trim();
        let delimiter_name = trimmed
            .strip_prefix("--- BOT:")
            .and_then(|s| s.strip_suffix("---"))
            .or_else(|| {
                trimmed
                    .strip_prefix("=== BOT:")
                    .and_then(|s| s.strip_suffix("==="))
            });
        if let Some(name_part) = delimiter_name {
            if !current_lines.is_empty() || current_name.is_some() {
                blocks.push((current_name.take(), std::mem::take(&mut current_lines)));
            }
            let n = name_part.trim();
            current_name = if n.is_empty() { None } else { Some(n.to_string()) };
        } else {
            current_lines.push(raw_line);
        }
    }
    if !current_lines.is_empty() || current_name.is_some() {
        blocks.push((current_name, current_lines));
    }

    blocks
        .into_iter()
        .filter_map(|(name, lines)| {
            let joined = lines.join("\n").trim().to_string();
            if joined.is_empty() && name.is_none() {
                None
            } else {
                Some((name, joined))
            }
        })
        .collect()
}

/// Parses raw foreign export text (single-bot or `--- BOT: <Name> ---` multi-bot)
/// into normalized, secret-redacted [`BotImportCandidate`] entries (capped at 24).
pub fn parse_bot_import_candidates(
    source: BotImportSource,
    export_text: &str,
    imported_at_ms: i64,
) -> Result<Vec<BotImportCandidate>, BotError> {
    if export_text.len() > BOT_IMPORT_EXPORT_MAX_BYTES {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!(
                "Bot import export exceeds maximum size ({BOT_IMPORT_EXPORT_MAX_BYTES} bytes)."
            ),
        ));
    }
    let blocks = split_export_sections(export_text);
    if blocks.is_empty() {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Bot import export text is empty.",
        ));
    }
    if blocks.len() > BOT_IMPORT_MAX_BOTS_PER_SOURCE {
        return Err(BotError::new(
            BotErrorCode::Full,
            format!(
                "Bot import exceeds maximum {BOT_IMPORT_MAX_BOTS_PER_SOURCE} bots per source."
            ),
        ));
    }

    let mut out = Vec::with_capacity(blocks.len());
    for (idx, (explicit_name, body)) in blocks.into_iter().enumerate() {
        let mut soul_lines = Vec::new();
        let mut memory_lines = Vec::new();
        let mut skill_slugs = Vec::new();
        let mut inferred_name: Option<String> = explicit_name;

        enum Section {
            Soul,
            Memory,
            Skills,
        }
        let mut section = Section::Soul;
        for line in body.lines() {
            let trimmed = line.trim();
            if inferred_name.is_none()
                && let Some(h1) = trimmed.strip_prefix("# ")
                && !h1.trim().is_empty()
            {
                inferred_name = Some(h1.trim().to_string());
            }
            if trimmed.eq_ignore_ascii_case("## memory") {
                section = Section::Memory;
                continue;
            }
            if trimmed.eq_ignore_ascii_case("## skills") {
                section = Section::Skills;
                continue;
            }
            if trimmed.starts_with("## ") {
                section = Section::Soul;
            }
            match section {
                Section::Soul => soul_lines.push(line),
                Section::Memory => memory_lines.push(line),
                Section::Skills => {
                    let cand = trimmed
                        .trim_start_matches(['-', '*', ' '])
                        .trim_matches('`')
                        .trim();
                    if is_valid_skill_slug(cand) && !skill_slugs.iter().any(|s| s == cand) {
                        skill_slugs.push(cand.to_string());
                    }
                }
            }
        }

        let fallback = format!("Imported bot {}", idx + 1);
        let raw_name = inferred_name.unwrap_or_else(|| fallback.clone());
        let redacted_name = redact_blocking_secrets(&raw_name);
        let suggested_name = sanitize_bot_name(&redacted_name.text, &fallback)?;
        let suggested_folder = bot_folder_name(suggested_name.as_str());

        let soul_raw = soul_lines.join("\n").trim().to_string();
        let soul_redacted = redact_blocking_secrets(&soul_raw);
        let soul_md = if soul_redacted.text.is_empty() {
            format!("# {}\n", suggested_name.as_str())
        } else {
            soul_redacted.text
        };

        let mem_raw = memory_lines.join("\n").trim().to_string();
        let (memory_md, mem_redacted_count) = if mem_raw.is_empty() {
            (None, 0)
        } else {
            let mr = redact_blocking_secrets(&mem_raw);
            (Some(mr.text), mr.count)
        };

        let mut risk_warnings = scan_bot_text_risks(&soul_md).risk_findings;
        if let Some(ref mem) = memory_md {
            risk_warnings.extend(scan_bot_text_risks(mem).risk_findings);
        }

        let redacted_secrets_count = redacted_name.count + soul_redacted.count + mem_redacted_count;
        out.push(BotImportCandidate {
            source,
            original_name: redacted_name.text,
            suggested_name: suggested_name.clone(),
            suggested_folder,
            soul_md,
            memory_md,
            skill_slugs,
            provenance: BotImportedFrom {
                source: source.as_str().to_string(),
                original_name: Some(suggested_name.as_str().to_string()),
                imported_at: imported_at_ms,
            },
            redacted_secrets_count,
            risk_warnings,
        });
    }
    Ok(out)
}

/// Validates a parsed [`BotImportCandidate`] before installing it as a local bot.
pub fn validate_bot_import_candidate(
    candidate: &BotImportCandidate,
    reject_unredacted_secrets: bool,
) -> Result<(), BotError> {
    BotName::new(candidate.suggested_name.as_str())
        .map_err(|code| BotError::new(code, "Import candidate has invalid suggested_name."))?;
    BotFolderSlug::new(candidate.suggested_folder.as_str())
        .map_err(|code| BotError::new(code, "Import candidate has invalid suggested_folder."))?;
    if candidate.soul_md.len() > BOT_IMPORT_EXPORT_MAX_BYTES {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Import candidate soul_md exceeds size cap.",
        ));
    }
    if let Some(ref mem) = candidate.memory_md
        && mem.len() > BOT_IMPORT_EXPORT_MAX_BYTES
    {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Import candidate memory_md exceeds size cap.",
        ));
    }
    for slug in &candidate.skill_slugs {
        if !is_valid_skill_slug(slug) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Import candidate contains invalid skill slug '{slug}'."),
            ));
        }
    }
    if reject_unredacted_secrets {
        if scan_bot_text_risks(&candidate.soul_md).blocking {
            return Err(BotError::new(
                BotErrorCode::SecretBlocked,
                "Import candidate soul_md still contains unredacted secrets.",
            ));
        }
        if let Some(ref mem) = candidate.memory_md
            && scan_bot_text_risks(mem).blocking
        {
            return Err(BotError::new(
                BotErrorCode::SecretBlocked,
                "Import candidate memory_md still contains unredacted secrets.",
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 2. Workspace Hand-Over (`Worktree | Thread | PairedWorkspace`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HandOverSourceKind {
    Worktree,
    Thread,
    PairedWorkspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HandOverRollbackPlacement {
    Restored,
    MovedSince,
    Left,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum HandOverOutcome {
    Created {
        bot_id: String,
        name: BotName,
        folder: BotFolderSlug,
    },
    Attached {
        bot_id: String,
    },
    Declined,
    Deferred,
    RolledBack {
        placement: HandOverRollbackPlacement,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandOverSuggestion {
    pub suggested_bot_name: BotName,
    pub purpose: String,
    pub folder: BotFolderSlug,
    pub source_kind: HandOverSourceKind,
}

fn title_case_slug_words(raw: &str) -> String {
    const STRIP_PREFIXES: &[&str] = &[
        "feature/",
        "feature-",
        "bugfix/",
        "bugfix-",
        "hotfix/",
        "hotfix-",
        "refactor/",
        "refactor-",
        "chore/",
        "chore-",
        "feat/",
        "feat-",
        "fix/",
        "fix-",
        "docs/",
        "docs-",
        "wip/",
        "wip-",
        "orca/",
        "orca-",
        "bot/",
        "bot-",
    ];
    let clean: String = raw.chars().filter(|c| !c.is_control()).collect();
    let mut trimmed = clean.trim();
    loop {
        let lower = trimmed.to_ascii_lowercase();
        let mut stripped = false;
        for pref in STRIP_PREFIXES {
            if lower.starts_with(pref) && trimmed.len() > pref.len() {
                trimmed = trimmed[pref.len()..].trim();
                stripped = true;
                break;
            }
        }
        if !stripped {
            break;
        }
    }
    let mut words = Vec::new();
    for part in trimmed.split(|c: char| matches!(c, '-' | '_' | '/' | '.' | ':') || c.is_whitespace()) {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            let mut word = String::new();
            for uc in first.to_uppercase() {
                word.push(uc);
            }
            for ch in chars {
                word.push(ch);
            }
            words.push(word);
        }
    }
    words.join(" ")
}

fn clean_single_line(raw: &str, max_chars: usize) -> String {
    let single: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    single.chars().take(max_chars).collect()
}

/// Suggests a clean bot name, folder slug, and purpose when handing a workspace
/// over from a human session to an Orca Bot. Handles empty, Unicode, control-char,
/// and oversized inputs safely without panicking.
pub fn suggest_hand_over_bot(
    workspace_name: &str,
    repo_slug: Option<&str>,
    recent_prompt: Option<&str>,
) -> Result<HandOverSuggestion, BotError> {
    let from_ws = title_case_slug_words(workspace_name);
    let from_repo = repo_slug.map(title_case_slug_words).unwrap_or_default();
    let from_prompt = recent_prompt
        .map(|p| clean_single_line(p, 48))
        .unwrap_or_default();

    let raw_candidate = if !from_ws.is_empty() {
        from_ws
    } else if !from_repo.is_empty() {
        from_repo
    } else if !from_prompt.is_empty() {
        from_prompt
    } else {
        "Workspace Helper".to_string()
    };

    let suggested_bot_name = sanitize_bot_name(&raw_candidate, "Workspace Helper")?;
    let folder = bot_folder_name(suggested_bot_name.as_str());

    let clean_ws = clean_single_line(workspace_name, 80);
    let clean_repo = repo_slug.map(|r| clean_single_line(r, 80)).unwrap_or_default();
    let clean_prompt = recent_prompt
        .map(|p| clean_single_line(p, 160))
        .unwrap_or_default();

    let purpose = match (clean_repo.is_empty(), clean_ws.is_empty(), clean_prompt.is_empty()) {
        (_, _, false) if !clean_repo.is_empty() => {
            format!("Supervise {clean_repo} work and continue: {clean_prompt}")
        }
        (_, _, false) => format!("Continue workspace task: {clean_prompt}"),
        (false, false, true) => {
            format!("Own and complete worktree '{clean_ws}' in {clean_repo}.")
        }
        (true, false, true) => format!("Own and complete worktree '{clean_ws}'."),
        (false, true, true) => format!("Supervise ongoing tasks in {clean_repo}."),
        (true, true, true) => "Supervise handed-over workspace tasks and report blockers.".to_string(),
    };

    Ok(HandOverSuggestion {
        suggested_bot_name,
        purpose,
        folder,
        source_kind: HandOverSourceKind::Worktree,
    })
}

/// Enforces CAS lineage precondition during transactional hand-over rollback/reparent.
pub fn check_hand_over_lineage_parent(
    expected_parent_id: Option<&str>,
    actual_parent_id: Option<&str>,
) -> Result<(), BotError> {
    if expected_parent_id != actual_parent_id {
        return Err(BotError::new(
            BotErrorCode::LineageParentChanged,
            "Workspace lineage parent changed concurrently during hand-over.",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 3. Bot Chat Tree (Cycle-Rejecting Builder & Lineage CAS Reparenting)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BotChatNodeKind {
    Home,
    Slack,
    Linear,
    Github,
    Automation,
    Side,
    Worker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BotChatLiveState {
    Replying,
    NeedsYou,
    Idle,
    Running,
    AskingBot,
    Unreachable,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BotChatNodeNote {
    HandedOver,
    FromClosedChat,
    AutomationTerminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotChatNode {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    pub title: String,
    pub kind: BotChatNodeKind,
    #[serde(default = "default_idle_live_state")]
    pub live_state: BotChatLiveState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<BotChatNodeNote>,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub needs_you: bool,
    pub updated_at_ms: i64,
}

fn default_idle_live_state() -> BotChatLiveState {
    BotChatLiveState::Idle
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotChatTreeEntry {
    pub node: BotChatNode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<BotChatTreeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotChatTree {
    pub bot_id: String,
    pub roots: Vec<BotChatTreeEntry>,
    pub total_nodes: usize,
    pub needs_you_count: usize,
    pub unread_count: usize,
}

fn chat_node_sort_key(node: &BotChatNode) -> (u8, u8, u8, std::cmp::Reverse<i64>, &str) {
    let home_rank = if node.kind == BotChatNodeKind::Home { 0 } else { 1 };
    let needs_rank = if node.needs_you || node.live_state == BotChatLiveState::NeedsYou {
        0
    } else {
        1
    };
    let unread_rank = if node.unread { 0 } else { 1 };
    (
        home_rank,
        needs_rank,
        unread_rank,
        std::cmp::Reverse(node.updated_at_ms),
        node.session_id.as_str(),
    )
}

fn build_chat_subtree(
    session_id: &str,
    by_id: &HashMap<&str, &BotChatNode>,
    children_map: &BTreeMap<&str, Vec<&BotChatNode>>,
) -> Option<BotChatTreeEntry> {
    let node = (*by_id.get(session_id)?).clone();
    let mut kids = children_map
        .get(session_id)
        .cloned()
        .unwrap_or_default();
    kids.sort_by(|a, b| chat_node_sort_key(a).cmp(&chat_node_sort_key(b)));
    let children = kids
        .into_iter()
        .filter_map(|child| build_chat_subtree(&child.session_id, by_id, children_map))
        .collect();
    Some(BotChatTreeEntry { node, children })
}

/// Builds a deterministic parent-child [`BotChatTree`], rejecting duplicate/empty
/// `session_id`s and ANY parent cycle (`self-parent` or multi-hop cycles).
///
/// Nodes whose `parent_session_id` refers to an absent/closed session are promoted
/// cleanly to top-level roots so orphaned workers remain visible.
pub fn build_bot_chat_tree(
    bot_id: impl Into<String>,
    nodes: &[BotChatNode],
) -> Result<BotChatTree, BotError> {
    let bot_id = bot_id.into();
    if bot_id.trim().is_empty() {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "BotChatTree bot_id must be non-empty.",
        ));
    }

    let mut by_id: HashMap<&str, &BotChatNode> = HashMap::with_capacity(nodes.len());
    for node in nodes {
        if node.session_id.trim().is_empty() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "BotChatNode session_id cannot be empty.",
            ));
        }
        if by_id.insert(node.session_id.as_str(), node).is_some() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Duplicate BotChatNode session_id '{}'.", node.session_id),
            ));
        }
    }

    // Cycle detection over parent pointers (O(V) with 3-state color table)
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Visit {
        Visiting,
        Visited,
    }
    let mut visit_state: HashMap<&str, Visit> = HashMap::with_capacity(nodes.len());
    for node in nodes {
        let mut cursor = node.session_id.as_str();
        let mut trail: Vec<&str> = Vec::new();
        loop {
            if let Some(&st) = visit_state.get(cursor) {
                if st == Visit::Visiting {
                    return Err(BotError::new(
                        BotErrorCode::InvalidArgument,
                        format!("Cycle detected in BotChatTree involving session '{cursor}'."),
                    ));
                }
                break;
            }
            visit_state.insert(cursor, Visit::Visiting);
            trail.push(cursor);
            let Some(curr_node) = by_id.get(cursor) else {
                break;
            };
            match curr_node.parent_session_id.as_deref() {
                Some(parent_id) if by_id.contains_key(parent_id) => {
                    cursor = parent_id;
                }
                _ => break,
            }
        }
        for visited_id in trail {
            visit_state.insert(visited_id, Visit::Visited);
        }
    }

    let mut children_map: BTreeMap<&str, Vec<&BotChatNode>> = BTreeMap::new();
    let mut root_nodes: Vec<&BotChatNode> = Vec::new();
    let mut needs_you_count = 0usize;
    let mut unread_count = 0usize;

    for node in nodes {
        if node.needs_you || node.live_state == BotChatLiveState::NeedsYou {
            needs_you_count += 1;
        }
        if node.unread {
            unread_count += 1;
        }
        match node.parent_session_id.as_deref() {
            Some(parent_id) if by_id.contains_key(parent_id) => {
                children_map.entry(parent_id).or_default().push(node);
            }
            _ => root_nodes.push(node),
        }
    }

    root_nodes.sort_by(|a, b| chat_node_sort_key(a).cmp(&chat_node_sort_key(b)));
    let roots = root_nodes
        .into_iter()
        .filter_map(|root| build_chat_subtree(&root.session_id, &by_id, &children_map))
        .collect();

    Ok(BotChatTree {
        bot_id,
        roots,
        total_nodes: nodes.len(),
        needs_you_count,
        unread_count,
    })
}

/// Reparents `session_id` with optimistic compare-and-swap on `expected_parent_id`.
///
/// Fails with:
/// - `BotChatNotFound` if `session_id` is absent
/// - `LineageParentChanged` if current `parent_session_id` != `expected_parent_id`
/// - `InvalidArgument` if `new_parent_id` creates a self-loop or ancestor cycle
pub fn reparent_bot_chat_node(
    nodes: &mut [BotChatNode],
    session_id: &str,
    expected_parent_id: Option<&str>,
    new_parent_id: Option<&str>,
) -> Result<(), BotError> {
    let target_idx = nodes
        .iter()
        .position(|n| n.session_id == session_id)
        .ok_or_else(|| {
            BotError::new(
                BotErrorCode::BotChatNotFound,
                format!("Bot chat session '{session_id}' not found."),
            )
        })?;

    check_hand_over_lineage_parent(
        expected_parent_id,
        nodes[target_idx].parent_session_id.as_deref(),
    )?;

    if let Some(new_parent) = new_parent_id {
        if new_parent == session_id {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Cannot reparent a chat session to itself.",
            ));
        }
        let parent_lookup: HashMap<&str, Option<&str>> = nodes
            .iter()
            .map(|n| (n.session_id.as_str(), n.parent_session_id.as_deref()))
            .collect();
        let mut seen = HashSet::new();
        let mut cursor = Some(new_parent);
        while let Some(curr) = cursor {
            if curr == session_id {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    "Reparenting would introduce a cycle in BotChatTree.",
                ));
            }
            if !seen.insert(curr) {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    "Existing cycle detected while validating reparent target.",
                ));
            }
            cursor = parent_lookup.get(curr).copied().flatten();
        }
    }

    nodes[target_idx].parent_session_id = new_parent_id.map(str::to_string);
    Ok(())
}

// ---------------------------------------------------------------------------
// 4. 10 Interactive Native Chat Bot Cards (`BOT_CARD_KINDS_10`)
// ---------------------------------------------------------------------------

/// Rendering surface placement for a native chat bot card (`native-chat-bot-card-kind.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BotCardPlacement {
    StatusLine,
    Message,
    Directive,
}

/// All 10 interactive Native Chat Bot Card kinds matching `BOT_CARD_KINDS_10`
/// (`orca_bots_and_designs_spec.md §1.10`).
///
/// Serializes canonically in `camelCase` (`memoryUpdated`, `connectorOffer`, …)
/// while also accepting `snake_case` aliases on deserialization and `parse()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeChatBotCardKind {
    #[serde(alias = "memory_updated")]
    MemoryUpdated,
    #[serde(alias = "connector_offer")]
    ConnectorOffer,
    #[serde(alias = "import_plan")]
    ImportPlan,
    #[serde(alias = "import_done")]
    ImportDone,
    #[serde(alias = "bot_request")]
    BotRequest,
    #[serde(alias = "google_setup")]
    GoogleSetup,
    #[serde(alias = "composio_setup")]
    ComposioSetup,
    #[serde(alias = "create_offer")]
    CreateOffer,
    #[serde(alias = "tool_added")]
    ToolAdded,
    Webhook,
}

impl NativeChatBotCardKind {
    pub const ALL: [Self; 10] = [
        Self::MemoryUpdated,
        Self::ConnectorOffer,
        Self::ImportPlan,
        Self::ImportDone,
        Self::BotRequest,
        Self::GoogleSetup,
        Self::ComposioSetup,
        Self::CreateOffer,
        Self::ToolAdded,
        Self::Webhook,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MemoryUpdated => "memoryUpdated",
            Self::ConnectorOffer => "connectorOffer",
            Self::ImportPlan => "importPlan",
            Self::ImportDone => "importDone",
            Self::BotRequest => "botRequest",
            Self::GoogleSetup => "googleSetup",
            Self::ComposioSetup => "composioSetup",
            Self::CreateOffer => "createOffer",
            Self::ToolAdded => "toolAdded",
            Self::Webhook => "webhook",
        }
    }

    pub const fn as_snake_str(self) -> &'static str {
        match self {
            Self::MemoryUpdated => "memory_updated",
            Self::ConnectorOffer => "connector_offer",
            Self::ImportPlan => "import_plan",
            Self::ImportDone => "import_done",
            Self::BotRequest => "bot_request",
            Self::GoogleSetup => "google_setup",
            Self::ComposioSetup => "composio_setup",
            Self::CreateOffer => "create_offer",
            Self::ToolAdded => "tool_added",
            Self::Webhook => "webhook",
        }
    }

    pub const fn placement(self) -> BotCardPlacement {
        match self {
            Self::MemoryUpdated
            | Self::ConnectorOffer
            | Self::ImportPlan
            | Self::ImportDone
            | Self::BotRequest
            | Self::GoogleSetup
            | Self::ComposioSetup
            | Self::CreateOffer => BotCardPlacement::StatusLine,
            Self::ToolAdded => BotCardPlacement::Message,
            Self::Webhook => BotCardPlacement::Directive,
        }
    }

    /// Parses either canonical camelCase (`memoryUpdated`) or snake_case (`memory_updated`).
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == trimmed || kind.as_snake_str() == trimmed)
    }
}

impl fmt::Display for NativeChatBotCardKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Typed interactive Native Chat Bot Card envelope rendered inside a Bot Home chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeChatBotCard {
    pub card_id: String,
    pub kind: NativeChatBotCardKind,
    pub bot_id: String,
    pub session_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub created_at_ms: i64,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl NativeChatBotCard {
    pub fn validate(&self) -> Result<(), BotError> {
        if self.card_id.trim().is_empty()
            || self.bot_id.trim().is_empty()
            || self.session_id.trim().is_empty()
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "NativeChatBotCard requires non-empty card_id, bot_id, and session_id.",
            ));
        }
        if self.title.trim().is_empty() || self.title.chars().any(char::is_control) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "NativeChatBotCard title must be non-empty without control characters.",
            ));
        }
        Ok(())
    }

    pub const fn placement(&self) -> BotCardPlacement {
        self.kind.placement()
    }
}

// ---------------------------------------------------------------------------
// 5. Usage-Limit Auto-Continue (`bot-usage-limit-stop.ts`)
// ---------------------------------------------------------------------------

/// 10-minute post-reset grace window before surfacing idle hit-limit status.
pub const USAGE_LIMIT_CONTINUE_GRACE_MS: i64 = 10 * 60 * 1000;
pub const USAGE_LIMIT_DEFAULT_MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageLimitContinue {
    pub provider: String,
    pub reset_at_ms: i64,
    pub auto_continue_enabled: bool,
    pub attempts: u32,
    pub max_attempts: u32,
    pub paused_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_resumed_at_ms: Option<i64>,
    #[serde(default = "default_usage_limit_grace_ms")]
    pub grace_ms: i64,
}

fn default_usage_limit_grace_ms() -> i64 {
    USAGE_LIMIT_CONTINUE_GRACE_MS
}

impl UsageLimitContinue {
    pub fn new(
        provider: impl Into<String>,
        paused_at_ms: i64,
        reset_at_ms: i64,
        auto_continue_enabled: bool,
    ) -> Result<Self, BotError> {
        let provider = provider.into();
        if provider.trim().is_empty() || provider.chars().any(char::is_control) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "UsageLimitContinue provider must be non-empty without control characters.",
            ));
        }
        Ok(Self {
            provider: provider.trim().to_string(),
            reset_at_ms,
            auto_continue_enabled,
            attempts: 0,
            max_attempts: USAGE_LIMIT_DEFAULT_MAX_ATTEMPTS,
            paused_at_ms,
            last_resumed_at_ms: None,
            grace_ms: USAGE_LIMIT_CONTINUE_GRACE_MS,
        })
    }

    /// Returns `true` when auto-continue is enabled, attempts remain, the reset
    /// timestamp has arrived, and this reset window has not already resumed.
    pub fn should_auto_continue(&self, now_ms: i64) -> bool {
        self.auto_continue_enabled
            && self.attempts < self.max_attempts
            && now_ms >= self.reset_at_ms
            && self
                .last_resumed_at_ms
                .is_none_or(|last| last < self.reset_at_ms)
    }

    /// Returns `true` during the `[reset_at_ms, reset_at_ms + grace_ms]` post-reset window.
    pub fn is_within_grace(&self, now_ms: i64) -> bool {
        now_ms >= self.reset_at_ms && now_ms <= self.reset_at_ms.saturating_add(self.grace_ms)
    }

    /// Records a continuation attempt, failing with `BotErrorCode::Full` once `max_attempts` is reached.
    pub fn mark_resumed(&mut self, now_ms: i64) -> Result<(), BotError> {
        if self.attempts >= self.max_attempts {
            return Err(BotError::new(
                BotErrorCode::Full,
                format!(
                    "Usage-limit auto-continue reached max attempts ({}).",
                    self.max_attempts
                ),
            ));
        }
        self.attempts = self.attempts.saturating_add(1);
        self.last_resumed_at_ms = Some(now_ms);
        Ok(())
    }

    /// Formats the deterministic status notice (`"Continues at HH:MM UTC"` vs
    /// `"Hit limit · resets HH:MM UTC"`).
    pub fn format_wait_notice(&self, now_ms: i64) -> String {
        let reset_label = DateTime::<Utc>::from_timestamp_millis(self.reset_at_ms)
            .map(|dt| dt.format("%H:%M UTC").to_string())
            .unwrap_or_else(|| "unknown time".to_string());

        if self.auto_continue_enabled && self.attempts < self.max_attempts {
            if now_ms < self.reset_at_ms {
                let remaining_secs = self.reset_at_ms.saturating_sub(now_ms) / 1000;
                let mins = (remaining_secs + 59) / 60;
                if mins > 0 {
                    format!("Continues at {reset_label} (in {mins}m)")
                } else {
                    format!("Continues at {reset_label}")
                }
            } else if self.is_within_grace(now_ms) {
                format!("Continues at {reset_label}")
            } else {
                format!("Hit limit · resets {reset_label}")
            }
        } else {
            format!("Hit limit · resets {reset_label}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_import_parses_redacts_secrets_and_preserves_provenance() {
        assert!(BOT_IMPORT_EXPORT_PROMPT.contains("## How I work"));
        assert!(BOT_IMPORT_EXPORT_PROMPT.contains("## Never"));
        assert_eq!(BotImportSource::parse("openclaw"), Some(BotImportSource::OpenClaw));
        assert!(!BotImportSource::OpenClaw.default_discovery_paths().is_empty());

        let raw = concat!(
            "--- BOT: Release Helper ---\n",
            "# Release Helper\n",
            "Helps ship releases cleanly.\n",
            "## How I work\n",
            "- Check CI with token ghp_1234567890abcdefghijklmnopQRSTUV\n",
            "## Never\n",
            "- Never push tags unapproved.\n",
            "## Memory\n",
            "- 2026-11-20 Fact: Staging password=SuperSecret12345\n",
            "## Skills\n",
            "- release-readiness\n",
            "- ../invalid-slug\n",
        );

        let candidates = parse_bot_import_candidates(
            BotImportSource::Hermes,
            raw,
            1_764_000_000_000,
        )
        .expect("parse import candidate");
        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        assert_eq!(c.suggested_name.as_str(), "Release Helper");
        assert_eq!(c.suggested_folder.as_str(), "release-helper");
        assert_eq!(c.skill_slugs, vec!["release-readiness"]);
        assert_eq!(c.provenance.source, "hermes");
        assert_eq!(c.provenance.imported_at, 1_764_000_000_000);
        assert!(c.redacted_secrets_count >= 2);
        assert!(!c.soul_md.contains("ghp_1234567890"));
        assert!(!c.memory_md.as_deref().unwrap_or("").contains("SuperSecret12345"));
        assert!(validate_bot_import_candidate(c, true).is_ok());

        // Injecting unredacted secret into candidate fails validation with SecretBlocked
        let mut dirty = c.clone();
        dirty.soul_md.push_str("\nSecret: sk-ant-api03-secretvalue1234567890\n");
        assert_eq!(
            validate_bot_import_candidate(&dirty, true)
                .expect_err("reject unredacted secret")
                .code,
            BotErrorCode::SecretBlocked
        );
    }

    #[test]
    fn suggest_hand_over_bot_handles_branch_names_empty_and_unicode_safely() {
        let s = suggest_hand_over_bot(
            "fix-relay-reconnect",
            Some("bhrumom/gpui-telegram"),
            Some("Keep investigating SSE disconnect retries."),
        )
        .expect("handover suggestion");
        assert_eq!(s.suggested_bot_name.as_str(), "Relay Reconnect");
        assert_eq!(s.folder.as_str(), "relay-reconnect");
        assert!(s.purpose.contains("bhrumom/gpui-telegram"));
        assert!(s.purpose.contains("SSE disconnect retries"));

        // Empty + control chars + huge Unicode input never panics and yields valid BotName
        let empty = suggest_hand_over_bot(" \n\t\r ", None, None).expect("empty fallback");
        assert_eq!(empty.suggested_bot_name.as_str(), "Workspace Helper");
        assert_eq!(empty.folder.as_str(), "workspace-helper");

        let huge_unicode = "🚀".repeat(120);
        let uni = suggest_hand_over_bot(&huge_unicode, None, None).expect("unicode truncated");
        assert!(uni.suggested_bot_name.as_str().chars().count() <= BOT_NAME_MAX_LENGTH);

        assert_eq!(
            check_hand_over_lineage_parent(Some("parent-a"), Some("parent-b"))
                .expect_err("cas mismatch")
                .code,
            BotErrorCode::LineageParentChanged
        );
    }

    #[test]
    fn bot_chat_tree_builds_deterministically_rejects_cycles_and_enforces_cas() {
        let mut nodes = vec![
            BotChatNode {
                session_id: "home".to_string(),
                parent_session_id: None,
                title: "Home".to_string(),
                kind: BotChatNodeKind::Home,
                live_state: BotChatLiveState::Idle,
                note: None,
                unread: false,
                needs_you: false,
                updated_at_ms: 100,
            },
            BotChatNode {
                session_id: "worker-1".to_string(),
                parent_session_id: Some("home".to_string()),
                title: "Fix tests".to_string(),
                kind: BotChatNodeKind::Worker,
                live_state: BotChatLiveState::NeedsYou,
                note: Some(BotChatNodeNote::HandedOver),
                unread: true,
                needs_you: true,
                updated_at_ms: 200,
            },
            BotChatNode {
                session_id: "orphan-worker".to_string(),
                parent_session_id: Some("closed-parent".to_string()),
                title: "Legacy worker".to_string(),
                kind: BotChatNodeKind::Worker,
                live_state: BotChatLiveState::Done,
                note: Some(BotChatNodeNote::FromClosedChat),
                unread: false,
                needs_you: false,
                updated_at_ms: 150,
            },
        ];

        let tree = build_bot_chat_tree("bot-1", &nodes).expect("build tree");
        assert_eq!(tree.total_nodes, 3);
        assert_eq!(tree.needs_you_count, 1);
        assert_eq!(tree.unread_count, 1);
        assert_eq!(tree.roots.len(), 2);
        assert_eq!(tree.roots[0].node.session_id, "home");
        assert_eq!(tree.roots[0].children.len(), 1);
        assert_eq!(tree.roots[0].children[0].node.session_id, "worker-1");
        assert_eq!(tree.roots[1].node.session_id, "orphan-worker");

        // CAS reparent mismatch -> LineageParentChanged
        assert_eq!(
            reparent_bot_chat_node(&mut nodes, "worker-1", Some("wrong"), None)
                .expect_err("stale parent")
                .code,
            BotErrorCode::LineageParentChanged
        );
        // Reparent home under its child worker-1 -> cycle rejected
        assert_eq!(
            reparent_bot_chat_node(&mut nodes, "home", None, Some("worker-1"))
                .expect_err("cycle rejected")
                .code,
            BotErrorCode::InvalidArgument
        );
        // Valid reparent succeeds
        assert!(reparent_bot_chat_node(&mut nodes, "orphan-worker", Some("closed-parent"), Some("home")).is_ok());

        // Direct cycle in input rejected by build_bot_chat_tree
        let cyclic = vec![
            BotChatNode {
                session_id: "a".to_string(),
                parent_session_id: Some("b".to_string()),
                title: "A".to_string(),
                kind: BotChatNodeKind::Side,
                live_state: BotChatLiveState::Idle,
                note: None,
                unread: false,
                needs_you: false,
                updated_at_ms: 1,
            },
            BotChatNode {
                session_id: "b".to_string(),
                parent_session_id: Some("a".to_string()),
                title: "B".to_string(),
                kind: BotChatNodeKind::Side,
                live_state: BotChatLiveState::Idle,
                note: None,
                unread: false,
                needs_you: false,
                updated_at_ms: 2,
            },
        ];
        assert_eq!(
            build_bot_chat_tree("bot-1", &cyclic).expect_err("cycle").code,
            BotErrorCode::InvalidArgument
        );
    }

    #[test]
    fn native_chat_bot_cards_match_exact_10_wire_kinds_and_usage_limit_continue() {
        let expected = [
            "memoryUpdated",
            "connectorOffer",
            "importPlan",
            "importDone",
            "botRequest",
            "googleSetup",
            "composioSetup",
            "createOffer",
            "toolAdded",
            "webhook",
        ];
        assert_eq!(NativeChatBotCardKind::ALL.len(), 10);
        for (kind, wire) in NativeChatBotCardKind::ALL.into_iter().zip(expected) {
            assert_eq!(kind.as_str(), wire);
            assert_eq!(NativeChatBotCardKind::parse(wire), Some(kind));
            assert_eq!(NativeChatBotCardKind::parse(kind.as_snake_str()), Some(kind));
            let json = serde_json::to_string(&kind).expect("serialize card kind");
            assert_eq!(json, format!("\"{wire}\""));
            let back: NativeChatBotCardKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, kind);
        }
        assert_eq!(NativeChatBotCardKind::ToolAdded.placement(), BotCardPlacement::Message);
        assert_eq!(NativeChatBotCardKind::Webhook.placement(), BotCardPlacement::Directive);
        assert_eq!(NativeChatBotCardKind::MemoryUpdated.placement(), BotCardPlacement::StatusLine);

        // UsageLimitContinue lifecycle
        let reset_at = 1_764_003_600_000i64; // 2025-11-24T17:00:00Z
        let mut ulc = UsageLimitContinue::new("claude", reset_at - 300_000, reset_at, true)
            .expect("create UsageLimitContinue");
        assert!(!ulc.should_auto_continue(reset_at - 1));
        assert!(ulc.format_wait_notice(reset_at - 120_000).starts_with("Continues at "));
        assert!(ulc.should_auto_continue(reset_at));
        assert!(ulc.is_within_grace(reset_at + 60_000));
        ulc.mark_resumed(reset_at + 1_000).expect("resume #1");
        assert!(!ulc.should_auto_continue(reset_at + 2_000));

        // Exhaust max attempts -> Full
        ulc.reset_at_ms = reset_at + 600_000;
        ulc.mark_resumed(ulc.reset_at_ms).expect("resume #2");
        ulc.reset_at_ms += 600_000;
        ulc.mark_resumed(ulc.reset_at_ms).expect("resume #3");
        assert_eq!(
            ulc.mark_resumed(ulc.reset_at_ms + 1).expect_err("max attempts").code,
            BotErrorCode::Full
        );
        assert!(ulc.format_wait_notice(ulc.reset_at_ms).starts_with("Hit limit · resets "));
    }
}
