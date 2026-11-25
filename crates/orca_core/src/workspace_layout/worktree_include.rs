//! Pure-Rust `.worktreeinclude` rule parser, deterministic copy planner, and
//! first-class non-Git `FolderWorkspace` domain records.
//!
//! Enforces strict safety & resource invariants:
//! - Rejects symlinks, path traversal (`..`), absolute paths, Windows drive/UNC
//!   prefixes, `.git` segments, and NUL/control characters.
//! - Enforces `WORKTREE_INCLUDE_MAX_BYTES` (2 GiB), `WORKTREE_INCLUDE_MAX_ENTRIES`
//!   (50,000), and `WORKTREE_INCLUDE_MAX_PATHS` (1,000) -> `CopyBudgetExceeded`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    OrcaIdeError, OrcaIdeErrorCode, WORKTREE_INCLUDE_MAX_BYTES, WORKTREE_INCLUDE_MAX_ENTRIES,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const WORKTREE_INCLUDE_MAX_PATHS: usize = 1_000;
pub const FOLDER_WORKSPACE_NAME_MAX_LEN: usize = 128;

// ---------------------------------------------------------------------------
// `.worktreeinclude` Rule Model & Parser
// ---------------------------------------------------------------------------

/// A parsed gitignore-style rule from `.worktreeinclude`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeIncludeRule {
    pub raw: String,
    pub pattern: String,
    pub negated: bool,
    pub dir_only: bool,
    pub anchored: bool,
    pub has_glob: bool,
    pub line_number: usize,
}

impl WorktreeIncludeRule {
    /// Evaluates whether this rule matches a normalized relative path (`a/b/c`).
    pub fn matches(&self, relative_path: &str, is_dir: bool) -> bool {
        if self.pattern.is_empty() {
            return false;
        }
        let norm = relative_path.trim_matches('/');
        if norm.is_empty() {
            return false;
        }

        // A directory-only rule matches the directory itself AND any descendant inside it.
        if self.dir_only {
            if is_dir && self.matches_raw_path(norm) {
                return true;
            }
            // Check every parent directory prefix of `norm`.
            let mut prefix_end = 0usize;
            for (idx, b) in norm.bytes().enumerate() {
                if b == b'/' {
                    prefix_end = idx;
                    if self.matches_raw_path(&norm[..prefix_end]) {
                        return true;
                    }
                }
            }
            let _ = prefix_end;
            return false;
        }

        // A non-dir-only rule also matches descendants if an ancestor directory matches literally.
        if self.matches_raw_path(norm) {
            return true;
        }
        for (idx, b) in norm.bytes().enumerate() {
            if b == b'/' && self.matches_raw_path(&norm[..idx]) {
                return true;
            }
        }
        false
    }

    fn matches_raw_path(&self, norm_path: &str) -> bool {
        if self.anchored || self.pattern.contains('/') {
            matches_worktree_glob(&self.pattern, norm_path)
        } else {
            // Unanchored filename pattern matches either the whole path or the basename.
            let basename = norm_path.rsplit('/').next().unwrap_or(norm_path);
            matches_worktree_glob(&self.pattern, basename)
                || matches_worktree_glob(&self.pattern, norm_path)
        }
    }
}

/// Parses `.worktreeinclude` text into structured gitignore-style rules, ignoring
/// blank lines and `#` comments while supporting `!` negation, trailing `/`,
/// leading `/`, and `*`/`**`/`?` globs.
pub fn parse_worktree_include(raw: &str) -> Vec<WorktreeIncludeRule> {
    let mut rules = Vec::new();
    for (idx, line) in raw.lines().enumerate() {
        let line_number = idx + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let raw_line = trimmed.to_string();
        let mut s = trimmed;
        let negated = if let Some(rest) = s.strip_prefix('!') {
            s = rest;
            true
        } else {
            if let Some(escaped) = s.strip_prefix("\\#").or_else(|| s.strip_prefix("\\!")) {
                // Keep literal '#' or '!' character.
                s = &s[1..];
                let _ = escaped;
            }
            false
        };

        let anchored = if let Some(rest) = s.strip_prefix('/') {
            s = rest;
            true
        } else {
            false
        };

        let dir_only = if let Some(rest) = s.strip_suffix('/') {
            s = rest;
            true
        } else {
            false
        };

        let pattern = s.replace('\\', "/");
        if pattern.is_empty() {
            continue;
        }
        let has_glob = pattern.contains('*') || pattern.contains('?');
        rules.push(WorktreeIncludeRule {
            raw: raw_line,
            pattern,
            negated,
            dir_only,
            anchored,
            has_glob,
            line_number,
        });
    }
    rules
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GlobToken {
    DoubleStar, // matches any sequence including '/'
    Star,       // matches any sequence excluding '/'
    Question,   // matches any single char excluding '/'
    Lit(char),
}

fn tokenize_glob(pattern: &str) -> Vec<GlobToken> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' {
            if i + 1 < chars.len() && chars[i + 1] == '*' {
                while i + 1 < chars.len() && chars[i + 1] == '*' {
                    i += 1;
                }
                if tokens.last() != Some(&GlobToken::DoubleStar) {
                    tokens.push(GlobToken::DoubleStar);
                }
            } else {
                tokens.push(GlobToken::Star);
            }
        } else if chars[i] == '?' {
            tokens.push(GlobToken::Question);
        } else {
            tokens.push(GlobToken::Lit(chars[i]));
        }
        i += 1;
    }
    tokens
}

/// Pure polynomial-time (ReDoS-immune) glob matcher supporting `*`, `**`, and `?`.
pub fn matches_worktree_glob(pattern: &str, path: &str) -> bool {
    let tokens = tokenize_glob(pattern);
    let text: Vec<char> = path.chars().collect();
    let m = tokens.len();
    let n = text.len();

    let mut prev = vec![false; n + 1];
    let mut curr = vec![false; n + 1];
    prev[0] = true;

    for tok in &tokens {
        curr.fill(false);
        match *tok {
            GlobToken::DoubleStar => {
                let mut seen_true = false;
                for j in 0..=n {
                    if prev[j] {
                        seen_true = true;
                    }
                    curr[j] = seen_true;
                }
            }
            GlobToken::Star => {
                for j in 0..=n {
                    if prev[j] || (j > 0 && curr[j - 1] && text[j - 1] != '/') {
                        curr[j] = true;
                    }
                }
            }
            GlobToken::Question => {
                for j in 1..=n {
                    if prev[j - 1] && text[j - 1] != '/' {
                        curr[j] = true;
                    }
                }
            }
            GlobToken::Lit(c) => {
                // Handle `**/` matching zero leading directories (e.g. `**/foo.txt` matches `foo.txt`).
                if c == '/' && m >= 2 && tokens.first() == Some(&GlobToken::DoubleStar) && prev[0] {
                    curr[0] = true;
                }
                for j in 1..=n {
                    if prev[j - 1] && text[j - 1] == c {
                        curr[j] = true;
                    }
                }
            }
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    // Also allow `a/**/b` to match `a/b` by checking collapsed `/**/` -> `/` variant when present.
    if prev[n] {
        return true;
    }
    if pattern.contains("/**/") {
        let collapsed = pattern.replacen("/**/", "/", 1);
        return matches_worktree_glob(&collapsed, path);
    }
    false
}

// ---------------------------------------------------------------------------
// Path Safety Validation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum WorktreePathError {
    #[error("path cannot be empty")]
    Empty,
    #[error("path contains forbidden NUL or control character")]
    ControlCharacter,
    #[error("absolute paths are forbidden: '{0}'")]
    AbsolutePath(String),
    #[error("path traversal ('..') is forbidden: '{0}'")]
    PathTraversal(String),
    #[error("accessing '.git' metadata is forbidden: '{0}'")]
    GitMetadataForbidden(String),
}

/// Validates and normalizes a worktree-relative path, rejecting empty paths,
/// NUL/control characters, absolute Unix/Windows/UNC paths, `..` traversal, and `.git`.
pub fn validate_worktree_relative_path(raw: &str) -> Result<String, WorktreePathError> {
    if raw.trim().is_empty() {
        return Err(WorktreePathError::Empty);
    }
    if raw.chars().any(char::is_control) {
        return Err(WorktreePathError::ControlCharacter);
    }
    if raw.starts_with('/') || raw.starts_with('\\') {
        return Err(WorktreePathError::AbsolutePath(raw.to_string()));
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(WorktreePathError::AbsolutePath(raw.to_string()));
    }

    let normalized = raw.replace('\\', "/");
    let mut clean_segments = Vec::new();
    for seg in normalized.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err(WorktreePathError::PathTraversal(raw.to_string()));
        }
        if seg == ".git" {
            return Err(WorktreePathError::GitMetadataForbidden(raw.to_string()));
        }
        clean_segments.push(seg);
    }
    if clean_segments.is_empty() {
        return Err(WorktreePathError::Empty);
    }
    Ok(clean_segments.join("/"))
}

// ---------------------------------------------------------------------------
// Deterministic Copy Planner
// ---------------------------------------------------------------------------

/// Input entry describing a candidate file/directory in the source repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeCopyCandidate {
    pub relative_path: String,
    pub size_bytes: u64,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub is_git_ignored: bool,
    /// When `true` (e.g., APFS `clonefile`), entry bypasses the byte copy budget
    /// while still consuming the entry-count budget.
    pub can_clone_cow: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeCopyAction {
    pub relative_path: String,
    pub size_bytes: u64,
    pub is_dir: bool,
    pub clone_cow: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeSkipReason {
    NotMatched,
    Negated,
    TrackedFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSkippedEntry {
    pub relative_path: String,
    pub reason: WorktreeSkipReason,
}

/// Pure deterministic plan returned by [`plan_worktree_include_copy`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WorktreeCopyPlan {
    pub actions: Vec<WorktreeCopyAction>,
    pub skipped: Vec<WorktreeSkippedEntry>,
    pub total_copied_bytes: u64,
    pub total_cloned_bytes: u64,
    pub total_entries: usize,
}

/// Evaluates `.worktreeinclude` rules against repository entries, producing a
/// deterministic copy plan while rejecting symlinks, path traversal, absolute
/// paths, NUL/control characters, `.git`, and budget violations (>2 GiB or >50,000 entries).
pub fn plan_worktree_include_copy(
    rules: &[WorktreeIncludeRule],
    entries: &[WorktreeCopyCandidate],
) -> Result<WorktreeCopyPlan, OrcaIdeError> {
    if rules.len() > WORKTREE_INCLUDE_MAX_PATHS {
        return Err(OrcaIdeError::new(
            OrcaIdeErrorCode::CopyBudgetExceeded,
            format!(
                ".worktreeinclude rule count {} exceeds limit {WORKTREE_INCLUDE_MAX_PATHS}",
                rules.len()
            ),
        ));
    }

    // Validate every rule pattern does not attempt traversal or escape repo root.
    for rule in rules {
        if rule.pattern.chars().any(char::is_control)
            || rule.pattern.split('/').any(|seg| seg == ".." || seg == ".git")
            || (rule.pattern.len() >= 2
                && rule.pattern.as_bytes()[0].is_ascii_alphabetic()
                && rule.pattern.as_bytes()[1] == b':')
        {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::CopyBudgetExceeded,
                format!(
                    "unsafe .worktreeinclude rule at line {}: '{}'",
                    rule.line_number, rule.raw
                ),
            ));
        }
    }

    // Sort indices deterministically by relative path for order-independent output.
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|&a, &b| entries[a].relative_path.cmp(&entries[b].relative_path));

    let mut plan = WorktreeCopyPlan::default();

    for idx in order {
        let entry = &entries[idx];
        let norm_path = validate_worktree_relative_path(&entry.relative_path).map_err(|e| {
            OrcaIdeError::new(
                OrcaIdeErrorCode::CopyBudgetExceeded,
                format!("unsafe candidate path rejected: {e}"),
            )
        })?;

        let mut matched = false;
        let mut negated = false;
        for rule in rules {
            if rule.matches(&norm_path, entry.is_dir) {
                if rule.negated {
                    matched = false;
                    negated = true;
                } else {
                    matched = true;
                    negated = false;
                }
            }
        }

        if !matched {
            plan.skipped.push(WorktreeSkippedEntry {
                relative_path: norm_path,
                reason: if negated {
                    WorktreeSkipReason::Negated
                } else {
                    WorktreeSkipReason::NotMatched
                },
            });
            continue;
        }

        if entry.is_symlink {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::CopyBudgetExceeded,
                format!("symlink '{norm_path}' rejected by .worktreeinclude security policy"),
            ));
        }

        if !entry.is_git_ignored {
            plan.skipped.push(WorktreeSkippedEntry {
                relative_path: norm_path,
                reason: WorktreeSkipReason::TrackedFile,
            });
            continue;
        }

        let next_entries = plan.total_entries.saturating_add(1);
        if next_entries > WORKTREE_INCLUDE_MAX_ENTRIES {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::CopyBudgetExceeded,
                format!(
                    ".worktreeinclude entry count {next_entries} exceeds limit {WORKTREE_INCLUDE_MAX_ENTRIES}"
                ),
            ));
        }

        if entry.can_clone_cow {
            plan.total_cloned_bytes = plan.total_cloned_bytes.saturating_add(entry.size_bytes);
        } else {
            let next_bytes = plan.total_copied_bytes.saturating_add(entry.size_bytes);
            if next_bytes > WORKTREE_INCLUDE_MAX_BYTES {
                return Err(OrcaIdeError::new(
                    OrcaIdeErrorCode::CopyBudgetExceeded,
                    format!(
                        ".worktreeinclude copy size {next_bytes} bytes exceeds limit {WORKTREE_INCLUDE_MAX_BYTES} bytes"
                    ),
                ));
            }
            plan.total_copied_bytes = next_bytes;
        }

        plan.total_entries = next_entries;
        plan.actions.push(WorktreeCopyAction {
            relative_path: norm_path,
            size_bytes: entry.size_bytes,
            is_dir: entry.is_dir,
            clone_cow: entry.can_clone_cow,
        });
    }

    Ok(plan)
}

// ---------------------------------------------------------------------------
// First-Class Non-Git Folder Workspaces
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum FolderWorkspaceError {
    #[error("folder workspace id cannot be empty")]
    EmptyId,
    #[error("folder workspace name cannot be empty or exceed {FOLDER_WORKSPACE_NAME_MAX_LEN} chars")]
    InvalidName,
    #[error("folder workspace root_path cannot be empty")]
    EmptyRootPath,
    #[error("folder workspace field '{0}' contains forbidden control character")]
    ControlCharacter(&'static str),
    #[error("folder workspace root_path contains forbidden traversal segment '..'")]
    PathTraversal,
}

/// First-class non-Git folder workspace record (used for arbitrary directories and Bot homes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderWorkspace {
    pub id: String,
    pub name: String,
    pub root_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_agent: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_status: Option<String>,
}

impl FolderWorkspace {
    pub fn validate(&self) -> Result<(), FolderWorkspaceError> {
        if self.id.trim().is_empty() {
            return Err(FolderWorkspaceError::EmptyId);
        }
        if self.id.chars().any(char::is_control) {
            return Err(FolderWorkspaceError::ControlCharacter("id"));
        }
        let trimmed_name = self.name.trim();
        if trimmed_name.is_empty() || trimmed_name.chars().count() > FOLDER_WORKSPACE_NAME_MAX_LEN {
            return Err(FolderWorkspaceError::InvalidName);
        }
        if self.name.chars().any(char::is_control) {
            return Err(FolderWorkspaceError::ControlCharacter("name"));
        }
        if self.root_path.trim().is_empty() {
            return Err(FolderWorkspaceError::EmptyRootPath);
        }
        if self.root_path.chars().any(char::is_control) {
            return Err(FolderWorkspaceError::ControlCharacter("root_path"));
        }
        let norm = self.root_path.replace('\\', "/");
        if norm.split('/').any(|seg| seg == "..") {
            return Err(FolderWorkspaceError::PathTraversal);
        }
        if let Some(agent) = &self.default_agent
            && agent.chars().any(char::is_control)
        {
            return Err(FolderWorkspaceError::ControlCharacter("default_agent"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, bytes: u64) -> WorktreeCopyCandidate {
        WorktreeCopyCandidate {
            relative_path: path.to_string(),
            size_bytes: bytes,
            is_dir: false,
            is_symlink: false,
            is_git_ignored: true,
            can_clone_cow: false,
        }
    }

    #[test]
    fn parses_globs_negation_comments_and_dir_rules() {
        let rules = parse_worktree_include(
            "# local secrets & caches\n\
             .env.*\n\
             !.env.example\n\
             certs/\n\
             data/**/cache?.bin\n",
        );
        assert_eq!(rules.len(), 4);
        assert!(rules[1].negated);
        assert!(rules[2].dir_only);
        assert!(rules[3].has_glob);

        let entries = vec![
            file(".env.local", 128),
            file(".env.example", 64),
            file("certs/dev.pem", 512),
            file("data/a/b/cache1.bin", 1024),
            file("data/cache2.bin", 256),
            WorktreeCopyCandidate {
                relative_path: ".env.tracked".to_string(),
                size_bytes: 64,
                is_dir: false,
                is_symlink: false,
                is_git_ignored: false,
                can_clone_cow: false,
            },
        ];

        let plan = plan_worktree_include_copy(&rules, &entries).expect("valid plan");
        let paths: Vec<&str> = plan.actions.iter().map(|a| a.relative_path.as_str()).collect();
        assert_eq!(
            paths,
            vec![".env.local", "certs/dev.pem", "data/a/b/cache1.bin", "data/cache2.bin"]
        );
        assert_eq!(plan.total_copied_bytes, 128 + 512 + 1024 + 256);
        assert_eq!(plan.total_entries, 4);
        assert_eq!(plan.skipped.len(), 2);
    }

    #[test]
    fn rejects_symlinks_traversal_absolute_control_chars_and_git() {
        let rules = parse_worktree_include("**/*\n");

        let mut sym = file("secrets/link", 10);
        sym.is_symlink = true;
        let err = plan_worktree_include_copy(&rules, &[sym]).expect_err("symlink rejected");
        assert_eq!(err.code, OrcaIdeErrorCode::CopyBudgetExceeded);

        for bad in ["../escape", "a/../../b", "/etc/passwd", "C:\\Windows\\win.ini", "a\0b", ".git/config"] {
            let err = plan_worktree_include_copy(&rules, &[file(bad, 10)])
                .expect_err(&format!("unsafe path {bad} must fail"));
            assert_eq!(err.code, OrcaIdeErrorCode::CopyBudgetExceeded);
        }
    }

    #[test]
    fn enforces_2gib_byte_cap_and_50000_entry_cap_and_cow_semantics() {
        let rules = parse_worktree_include("*.bin\n");

        let too_big = vec![
            file("a.bin", WORKTREE_INCLUDE_MAX_BYTES),
            file("b.bin", 1),
        ];
        let err = plan_worktree_include_copy(&rules, &too_big).expect_err("exceeds 2 GiB");
        assert_eq!(err.code, OrcaIdeErrorCode::CopyBudgetExceeded);

        // APFS COW clone bypasses byte budget while consuming entry count
        let mut cow = file("huge.bin", WORKTREE_INCLUDE_MAX_BYTES + 1024);
        cow.can_clone_cow = true;
        let cow_plan = plan_worktree_include_copy(&rules, &[cow]).expect("COW clone allowed");
        assert_eq!(cow_plan.total_copied_bytes, 0);
        assert_eq!(cow_plan.total_cloned_bytes, WORKTREE_INCLUDE_MAX_BYTES + 1024);
        assert_eq!(cow_plan.total_entries, 1);

        // 50,001 entries exceeds WORKTREE_INCLUDE_MAX_ENTRIES
        let many: Vec<WorktreeCopyCandidate> = (0..=WORKTREE_INCLUDE_MAX_ENTRIES)
            .map(|i| file(&format!("f_{i:05}.bin"), 1))
            .collect();
        let err = plan_worktree_include_copy(&rules, &many).expect_err("exceeds 50k entries");
        assert_eq!(err.code, OrcaIdeErrorCode::CopyBudgetExceeded);
    }

    #[test]
    fn validates_folder_workspace_records() {
        let mut ws = FolderWorkspace {
            id: "fw_01".to_string(),
            name: "Scratch Notes".to_string(),
            root_path: "/home/dev/notes".to_string(),
            project_group_id: None,
            default_agent: Some("claude".to_string()),
            created_at_ms: 1_000,
            updated_at_ms: 2_000,
            pinned: true,
            checkpoint_comment: None,
            workspace_status: None,
        };
        assert!(ws.validate().is_ok());

        ws.name = "Bad\nName".to_string();
        assert_eq!(ws.validate(), Err(FolderWorkspaceError::ControlCharacter("name")));

        ws.name = "Good".to_string();
        ws.root_path = "   ".to_string();
        assert_eq!(ws.validate(), Err(FolderWorkspaceError::EmptyRootPath));

        ws.root_path = "/home/dev/../etc".to_string();
        assert_eq!(ws.validate(), Err(FolderWorkspaceError::PathTraversal));
    }
}
