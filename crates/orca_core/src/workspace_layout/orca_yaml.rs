//! Pure-Rust strict YAML 1.2 subset parser and `orca.yaml` configuration validator.
//!
//! Enforces strict safety budgets with zero external YAML dependencies:
//! - Maximum input size: `ORCA_YAML_MAX_BYTES` (256 KiB, UTF-8 or UTF-16).
//! - Rejects non-mapping roots and duplicate mapping keys at ANY nesting depth.
//! - Maximum single string length: 64 KiB (`ORCA_YAML_MAX_STRING_BYTES`).
//! - Maximum alias dereferences: 100 (`ORCA_YAML_MAX_ALIAS_COUNT`) plus expanded-node
//!   caps preventing billion-laughs amplification.
//! - Maximum nesting depth: 32 (`ORCA_YAML_MAX_DEPTH`).
//! - List caps (`sharedDirectories <= 100`, `defaultTabs <= 256`, `environmentRecipes <= 256`).
//! - Deterministic SHA-256 `setup_trust_hash` over setup/teardown/scripts/services/recipes.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{ORCA_YAML_MAX_BYTES, OrcaIdeError, OrcaIdeErrorCode};

// ---------------------------------------------------------------------------
// Canonical Limits
// ---------------------------------------------------------------------------

pub const ORCA_YAML_MAX_STRING_BYTES: usize = 65_536;
pub const ORCA_YAML_MAX_ALIAS_COUNT: usize = 100;
pub const ORCA_YAML_MAX_EXPANDED_NODES: usize = 10_000;
pub const ORCA_YAML_MAX_DEPTH: usize = 32;
pub const ORCA_YAML_MAX_SHARED_DIRS: usize = 100;
pub const ORCA_YAML_MAX_DEFAULT_TABS: usize = 256;
pub const ORCA_YAML_MAX_ENV_RECIPES: usize = 256;

const SETUP_TRUST_HASH_DOMAIN: &[u8] = b"orca-yaml-setup-trust-v1\0";

// ---------------------------------------------------------------------------
// Error & Value Types
// ---------------------------------------------------------------------------

/// Structured error returned by the strict YAML 1.2 parser and `orca.yaml` validator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum OrcaYamlError {
    #[error("yaml_size_exceeded: {message}")]
    SizeExceeded {
        message: String,
        bytes: usize,
        max_bytes: usize,
    },
    #[error("yaml_duplicate_key: duplicate key '{key}' at line {line}")]
    DuplicateKey { key: String, line: usize },
    #[error("yaml_alias_limit: {message}")]
    AliasLimit {
        message: String,
        count: usize,
        max_aliases: usize,
    },
    #[error("yaml_non_mapping_root: {message}")]
    NonMappingRoot { message: String },
    #[error("yaml_size_exceeded: string of {bytes} bytes exceeds {max_bytes} bytes at line {line}")]
    StringTooLong {
        bytes: usize,
        max_bytes: usize,
        line: usize,
    },
    #[error("yaml_depth_exceeded: nesting depth exceeded {max_depth} at line {line}")]
    DepthExceeded { max_depth: usize, line: usize },
    #[error("yaml_syntax_error at line {line}: {message}")]
    Syntax { line: usize, message: String },
    #[error("yaml_schema_error at '{field}': {message}")]
    Schema { field: String, message: String },
}

impl OrcaYamlError {
    /// Maps this error to its canonical `OrcaIdeErrorCode` when defined by the wire contract.
    pub fn ide_error_code(&self) -> Option<OrcaIdeErrorCode> {
        match self {
            Self::SizeExceeded { .. } | Self::StringTooLong { .. } => {
                Some(OrcaIdeErrorCode::YamlSizeExceeded)
            }
            Self::DuplicateKey { .. } => Some(OrcaIdeErrorCode::YamlDuplicateKey),
            Self::AliasLimit { .. } => Some(OrcaIdeErrorCode::YamlAliasLimit),
            Self::NonMappingRoot { .. }
            | Self::DepthExceeded { .. }
            | Self::Syntax { .. }
            | Self::Schema { .. } => None,
        }
    }

    /// Converts to `OrcaIdeError` if an `OrcaIdeErrorCode` applies.
    pub fn to_ide_error(&self) -> Option<OrcaIdeError> {
        self.ide_error_code()
            .map(|code| OrcaIdeError::new(code, self.to_string()))
    }
}

/// Strictly parsed YAML 1.2 value AST with duplicate-free ordered mappings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum YamlValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Seq(Vec<YamlValue>),
    Map(Vec<(String, YamlValue)>),
}

impl YamlValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_seq(&self) -> Option<&[YamlValue]> {
        match self {
            Self::Seq(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&[(String, YamlValue)]> {
        match self {
            Self::Map(entries) => Some(entries.as_slice()),
            _ => None,
        }
    }

    pub fn map_get(&self, key: &str) -> Option<&YamlValue> {
        self.as_map()?
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    pub fn scalar_to_string(&self) -> Option<String> {
        match self {
            Self::Str(s) => Some(s.clone()),
            Self::Int(i) => Some(i.to_string()),
            Self::Float(f) => Some(f.to_string()),
            Self::Bool(b) => Some(b.to_string()),
            Self::Null => Some(String::new()),
            Self::Seq(_) | Self::Map(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Strongly-Typed `orca.yaml` Schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SetupAgentStartupPolicy {
    #[default]
    StartImmediately,
    WaitForSetup,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaYamlService {
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default = "default_true")]
    pub auto_start: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaYamlDefaultTab {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaYamlEnvironmentRecipe {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destroy: Option<String>,
}

/// Validated `orca.yaml` workspace configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OrcaYamlConfig {
    pub setup: Vec<String>,
    pub teardown: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub scripts: BTreeMap<String, String>,
    pub services: Vec<OrcaYamlService>,
    pub ports: Vec<u16>,
    pub worktree_include: Vec<String>,
    pub auto_archive_days: Option<u32>,
    pub setup_agent_startup_policy: SetupAgentStartupPolicy,
    pub issue_command: Option<String>,
    pub shared_directories: Vec<String>,
    pub default_tabs: Vec<OrcaYamlDefaultTab>,
    pub environment_recipes: Vec<OrcaYamlEnvironmentRecipe>,
    pub warnings: Vec<String>,
}

/// Returns `true` if the script begins with a POSIX `#!` shebang (used to refuse
/// mis-execution under Windows `cmd.exe` when Git Bash is unavailable).
pub fn is_posix_shebang_script(script: &str) -> bool {
    script.trim_start_matches('\u{feff}').starts_with("#!")
}

fn hash_len_str(hasher: &mut Sha256, s: &str) {
    hasher.update((s.len() as u64).to_be_bytes());
    hasher.update(s.as_bytes());
}

fn hash_opt_str(hasher: &mut Sha256, opt: Option<&str>) {
    match opt {
        Some(s) => {
            hasher.update([1u8]);
            hash_len_str(hasher, s);
        }
        None => hasher.update([0u8]),
    }
}

/// Computes a deterministic SHA-256 hex trust hash over all executable commands in
/// `orca.yaml` (`setup`, `teardown`, `scripts`, `services`, `environment_recipes`,
/// and `issue_command`). Any modification invalidates prior user approval.
pub fn setup_trust_hash(config: &OrcaYamlConfig) -> String {
    let mut h = Sha256::new();
    h.update(SETUP_TRUST_HASH_DOMAIN);

    h.update((config.setup.len() as u64).to_be_bytes());
    for cmd in &config.setup {
        hash_len_str(&mut h, cmd);
    }

    h.update((config.teardown.len() as u64).to_be_bytes());
    for cmd in &config.teardown {
        hash_len_str(&mut h, cmd);
    }

    h.update((config.scripts.len() as u64).to_be_bytes());
    for (k, v) in &config.scripts {
        hash_len_str(&mut h, k);
        hash_len_str(&mut h, v);
    }

    h.update((config.services.len() as u64).to_be_bytes());
    for svc in &config.services {
        hash_len_str(&mut h, &svc.name);
        hash_len_str(&mut h, &svc.command);
        hash_opt_str(&mut h, svc.cwd.as_deref());
        h.update((svc.env.len() as u64).to_be_bytes());
        for (ek, ev) in &svc.env {
            hash_len_str(&mut h, ek);
            hash_len_str(&mut h, ev);
        }
        match svc.port {
            Some(p) => {
                h.update([1u8]);
                h.update(p.to_be_bytes());
            }
            None => h.update([0u8]),
        }
        h.update([u8::from(svc.auto_start)]);
    }

    h.update((config.environment_recipes.len() as u64).to_be_bytes());
    for rec in &config.environment_recipes {
        hash_len_str(&mut h, &rec.name);
        hash_opt_str(&mut h, rec.create.as_deref());
        hash_opt_str(&mut h, rec.suspend.as_deref());
        hash_opt_str(&mut h, rec.resume.as_deref());
        hash_opt_str(&mut h, rec.destroy.as_deref());
    }

    hash_opt_str(&mut h, config.issue_command.as_deref());
    hex::encode(h.finalize())
}

// ---------------------------------------------------------------------------
// Byte Decoding & Strict YAML 1.2 Parser Core
// ---------------------------------------------------------------------------

/// Decodes raw UTF-8 or UTF-16 LE/BE bytes enforcing `ORCA_YAML_MAX_BYTES`,
/// then parses and validates `orca.yaml`.
pub fn parse_orca_yaml_bytes(raw: &[u8]) -> Result<OrcaYamlConfig, OrcaYamlError> {
    if raw.len() > ORCA_YAML_MAX_BYTES {
        return Err(OrcaYamlError::SizeExceeded {
            message: format!(
                "orca.yaml raw size {} bytes exceeds maximum {} bytes",
                raw.len(),
                ORCA_YAML_MAX_BYTES
            ),
            bytes: raw.len(),
            max_bytes: ORCA_YAML_MAX_BYTES,
        });
    }
    let decoded = if raw.starts_with(&[0xFF, 0xFE]) {
        decode_utf16_bytes(&raw[2..], false)?
    } else if raw.starts_with(&[0xFE, 0xFF]) {
        decode_utf16_bytes(&raw[2..], true)?
    } else if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        std::str::from_utf8(&raw[3..])
            .map(|s| s.to_string())
            .map_err(|e| OrcaYamlError::Syntax {
                line: 1,
                message: format!("invalid UTF-8: {e}"),
            })?
    } else {
        std::str::from_utf8(raw)
            .map(|s| s.to_string())
            .map_err(|e| OrcaYamlError::Syntax {
                line: 1,
                message: format!("invalid UTF-8: {e}"),
            })?
    };
    parse_orca_yaml(&decoded)
}

fn decode_utf16_bytes(bytes: &[u8], big_endian: bool) -> Result<String, OrcaYamlError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(OrcaYamlError::Syntax {
            line: 1,
            message: "odd byte length in UTF-16 YAML input".to_string(),
        });
    }
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| {
            if big_endian {
                u16::from_be_bytes(c)
            } else {
                u16::from_le_bytes(c)
            }
        })
        .collect();
    String::from_utf16(&units).map_err(|e| OrcaYamlError::Syntax {
        line: 1,
        message: format!("invalid UTF-16 sequence: {e}"),
    })
}

/// Strictly parses a YAML 1.2 subset document into a `YamlValue` AST.
pub fn parse_yaml_strict(input: &str) -> Result<YamlValue, OrcaYamlError> {
    if input.len() > ORCA_YAML_MAX_BYTES {
        return Err(OrcaYamlError::SizeExceeded {
            message: format!(
                "YAML document size {} bytes exceeds maximum {} bytes",
                input.len(),
                ORCA_YAML_MAX_BYTES
            ),
            bytes: input.len(),
            max_bytes: ORCA_YAML_MAX_BYTES,
        });
    }
    if input.as_bytes().contains(&0) {
        return Err(OrcaYamlError::Syntax {
            line: 1,
            message: "YAML document contains forbidden NUL byte".to_string(),
        });
    }

    let cleaned = input.strip_prefix('\u{feff}').unwrap_or(input);
    let raw_lines: Vec<String> = cleaned
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();

    let mut parser = YamlParser::new(raw_lines);
    parser.skip_blank_and_comments();
    if parser.pos < parser.lines.len() && is_doc_start(&parser.lines[parser.pos]) {
        parser.pos += 1;
        parser.skip_blank_and_comments();
    }
    if parser.pos >= parser.lines.len() || is_doc_end(&parser.lines[parser.pos]) {
        return Ok(YamlValue::Null);
    }

    let root = parser.parse_block_node(0, 0)?;
    parser.skip_blank_and_comments();
    if parser.pos < parser.lines.len() && !is_doc_end(&parser.lines[parser.pos]) {
        return Err(OrcaYamlError::Syntax {
            line: parser.pos + 1,
            message: format!(
                "unexpected trailing content at root level: '{}'",
                parser.lines[parser.pos].trim()
            ),
        });
    }
    Ok(root)
}

fn is_doc_start(line: &str) -> bool {
    let t = line.trim();
    t == "---" || t.starts_with("--- ")
}

fn is_doc_end(line: &str) -> bool {
    let t = line.trim();
    t == "..." || t.starts_with("... ")
}

struct YamlParser {
    lines: Vec<String>,
    pos: usize,
    anchors: BTreeMap<String, YamlValue>,
    alias_count: usize,
    expanded_nodes: usize,
}

impl YamlParser {
    fn new(lines: Vec<String>) -> Self {
        Self {
            lines,
            pos: 0,
            anchors: BTreeMap::new(),
            alias_count: 0,
            expanded_nodes: 0,
        }
    }

    fn check_depth(&self, depth: usize, line_no: usize) -> Result<(), OrcaYamlError> {
        if depth > ORCA_YAML_MAX_DEPTH {
            return Err(OrcaYamlError::DepthExceeded {
                max_depth: ORCA_YAML_MAX_DEPTH,
                line: line_no,
            });
        }
        Ok(())
    }

    fn count_node(&mut self, count: usize) -> Result<(), OrcaYamlError> {
        self.expanded_nodes = self.expanded_nodes.saturating_add(count);
        if self.expanded_nodes > ORCA_YAML_MAX_EXPANDED_NODES {
            return Err(OrcaYamlError::AliasLimit {
                message: format!(
                    "YAML expanded node count exceeded safety limit ({ORCA_YAML_MAX_EXPANDED_NODES})"
                ),
                count: self.expanded_nodes,
                max_aliases: ORCA_YAML_MAX_ALIAS_COUNT,
            });
        }
        Ok(())
    }

    fn skip_blank_and_comments(&mut self) {
        while self.pos < self.lines.len() {
            let trimmed = self.lines[self.pos].trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn current_indent_and_content(&self) -> Result<(usize, &str, usize), OrcaYamlError> {
        let line = &self.lines[self.pos];
        let line_no = self.pos + 1;
        let mut indent = 0usize;
        for b in line.bytes() {
            if b == b' ' {
                indent += 1;
            } else if b == b'\t' {
                return Err(OrcaYamlError::Syntax {
                    line: line_no,
                    message: "tabs are forbidden for YAML block indentation".to_string(),
                });
            } else {
                break;
            }
        }
        Ok((indent, &line[indent..], line_no))
    }

    fn parse_block_node(&mut self, min_indent: usize, depth: usize) -> Result<YamlValue, OrcaYamlError> {
        self.skip_blank_and_comments();
        if self.pos >= self.lines.len() || is_doc_end(&self.lines[self.pos]) {
            return Ok(YamlValue::Null);
        }
        let (indent, content, line_no) = self.current_indent_and_content()?;
        self.check_depth(depth, line_no)?;
        if indent < min_indent {
            return Ok(YamlValue::Null);
        }
        let content_owned = content.to_string();
        if is_seq_item(&content_owned) {
            self.parse_block_sequence(indent, depth)
        } else if content_owned.starts_with('[') || content_owned.starts_with('{') {
            self.parse_multiline_flow(depth)
        } else if split_mapping_entry(&content_owned).is_some() {
            self.parse_block_mapping(indent, depth)
        } else {
            self.pos += 1;
            self.parse_inline_value(&content_owned, indent, line_no, depth)
        }
    }

    fn parse_block_sequence(&mut self, seq_indent: usize, depth: usize) -> Result<YamlValue, OrcaYamlError> {
        let mut items = Vec::new();
        loop {
            self.skip_blank_and_comments();
            if self.pos >= self.lines.len() || is_doc_end(&self.lines[self.pos]) {
                break;
            }
            let (indent, content, line_no) = self.current_indent_and_content()?;
            if indent < seq_indent {
                break;
            }
            if indent > seq_indent {
                return Err(OrcaYamlError::Syntax {
                    line: line_no,
                    message: "unexpected indentation inside block sequence".to_string(),
                });
            }
            let content_owned = content.to_string();
            if !is_seq_item(&content_owned) {
                break;
            }
            self.pos += 1;
            self.count_node(1)?;
            let after_dash = &content_owned[1..];
            let leading_ws = after_dash.bytes().take_while(|&b| b == b' ').count();
            let rest = after_dash[leading_ws..].trim_end();
            let rest_clean = strip_inline_comment(rest).trim();

            if rest_clean.is_empty() {
                let val = self.parse_block_node(seq_indent + 1, depth + 1)?;
                items.push(val);
                continue;
            }

            let virtual_indent = seq_indent + 1 + leading_ws;
            if is_seq_item(rest) {
                // Nested inline sequence: `- - item`
                let synthetic = format!("{}{}", " ".repeat(virtual_indent), rest);
                self.lines.insert(self.pos, synthetic);
                let val = self.parse_block_sequence(virtual_indent, depth + 1)?;
                items.push(val);
            } else if split_mapping_entry(rest).is_some() {
                let val = self.parse_inline_sequence_mapping(rest, virtual_indent, line_no, depth + 1)?;
                items.push(val);
            } else {
                let val = self.parse_inline_value(rest, seq_indent, line_no, depth + 1)?;
                items.push(val);
            }
        }
        Ok(YamlValue::Seq(items))
    }

    fn parse_inline_sequence_mapping(
        &mut self,
        first_entry: &str,
        map_indent: usize,
        first_line_no: usize,
        depth: usize,
    ) -> Result<YamlValue, OrcaYamlError> {
        self.check_depth(depth, first_line_no)?;
        let mut entries: Vec<(String, YamlValue)> = Vec::new();
        let mut seen_keys: BTreeSet<String> = BTreeSet::new();
        let mut merged_entries: Vec<(String, YamlValue)> = Vec::new();

        self.insert_mapping_line(
            first_entry,
            map_indent,
            first_line_no,
            depth,
            &mut entries,
            &mut seen_keys,
            &mut merged_entries,
        )?;
        self.parse_block_mapping_body(
            map_indent,
            depth,
            &mut entries,
            &mut seen_keys,
            &mut merged_entries,
        )?;
        Ok(YamlValue::Map(finalize_merged_map(entries, merged_entries)))
    }
}

impl YamlParser {
    fn parse_block_mapping(&mut self, map_indent: usize, depth: usize) -> Result<YamlValue, OrcaYamlError> {
        let mut entries: Vec<(String, YamlValue)> = Vec::new();
        let mut seen_keys: BTreeSet<String> = BTreeSet::new();
        let mut merged_entries: Vec<(String, YamlValue)> = Vec::new();
        self.parse_block_mapping_body(
            map_indent,
            depth,
            &mut entries,
            &mut seen_keys,
            &mut merged_entries,
        )?;
        Ok(YamlValue::Map(finalize_merged_map(entries, merged_entries)))
    }

    fn parse_block_mapping_body(
        &mut self,
        map_indent: usize,
        depth: usize,
        entries: &mut Vec<(String, YamlValue)>,
        seen_keys: &mut BTreeSet<String>,
        merged_entries: &mut Vec<(String, YamlValue)>,
    ) -> Result<(), OrcaYamlError> {
        loop {
            self.skip_blank_and_comments();
            if self.pos >= self.lines.len() || is_doc_end(&self.lines[self.pos]) {
                break;
            }
            let (indent, content, line_no) = self.current_indent_and_content()?;
            if indent < map_indent {
                break;
            }
            if indent > map_indent {
                return Err(OrcaYamlError::Syntax {
                    line: line_no,
                    message: "unexpected indentation inside block mapping".to_string(),
                });
            }
            let content_owned = content.to_string();
            if is_seq_item(&content_owned) {
                break;
            }
            self.pos += 1;
            self.insert_mapping_line(
                &content_owned,
                map_indent,
                line_no,
                depth,
                entries,
                seen_keys,
                merged_entries,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_mapping_line(
        &mut self,
        line_content: &str,
        map_indent: usize,
        line_no: usize,
        depth: usize,
        entries: &mut Vec<(String, YamlValue)>,
        seen_keys: &mut BTreeSet<String>,
        merged_entries: &mut Vec<(String, YamlValue)>,
    ) -> Result<(), OrcaYamlError> {
        let (raw_key, raw_val) = split_mapping_entry(line_content).ok_or_else(|| OrcaYamlError::Syntax {
            line: line_no,
            message: format!("expected 'key: value' mapping entry, found '{line_content}'"),
        })?;
        let key = parse_mapping_key(raw_key, line_no)?;
        if !seen_keys.insert(key.clone()) {
            return Err(OrcaYamlError::DuplicateKey { key, line: line_no });
        }
        self.count_node(1)?;

        let val_trimmed = raw_val.trim();
        let val_no_comment = strip_inline_comment(val_trimmed).trim();
        let value = if val_no_comment.is_empty() {
            // Check indented block child (allow block sequence at same indent as mapping key per YAML 1.2!)
            self.parse_mapping_child_value(map_indent, depth + 1)?
        } else if let Some((anchor_name, remainder)) = split_anchor_prefix(val_trimmed) {
            let rem_clean = strip_inline_comment(remainder).trim();
            let v = if rem_clean.is_empty() {
                self.parse_mapping_child_value(map_indent, depth + 1)?
            } else {
                self.parse_inline_value(remainder.trim(), map_indent, line_no, depth + 1)?
            };
            self.anchors.insert(anchor_name.to_string(), v.clone());
            v
        } else {
            self.parse_inline_value(val_trimmed, map_indent, line_no, depth + 1)?
        };

        if key == "<<" {
            collect_merge_entries(&value, line_no, merged_entries)?;
        } else {
            entries.push((key, value));
        }
        Ok(())
    }

    fn parse_mapping_child_value(
        &mut self,
        parent_indent: usize,
        depth: usize,
    ) -> Result<YamlValue, OrcaYamlError> {
        self.skip_blank_and_comments();
        if self.pos >= self.lines.len() || is_doc_end(&self.lines[self.pos]) {
            return Ok(YamlValue::Null);
        }
        let (next_indent, next_content, line_no) = self.current_indent_and_content()?;
        self.check_depth(depth, line_no)?;
        if next_indent > parent_indent {
            self.parse_block_node(next_indent, depth)
        } else if next_indent == parent_indent && is_seq_item(next_content) {
            // YAML 1.2 allows block sequence items at the exact same indent as their mapping key.
            self.parse_block_sequence(next_indent, depth)
        } else {
            Ok(YamlValue::Null)
        }
    }

    fn parse_inline_value(
        &mut self,
        raw: &str,
        parent_indent: usize,
        line_no: usize,
        depth: usize,
    ) -> Result<YamlValue, OrcaYamlError> {
        self.check_depth(depth, line_no)?;
        let trimmed = raw.trim();
        if let Some((anchor_name, remainder)) = split_anchor_prefix(trimmed) {
            let rem_trim = remainder.trim();
            let val = if strip_inline_comment(rem_trim).trim().is_empty() {
                self.parse_mapping_child_value(parent_indent, depth + 1)?
            } else {
                self.parse_inline_value(rem_trim, parent_indent, line_no, depth)?
            };
            self.anchors.insert(anchor_name.to_string(), val.clone());
            return Ok(val);
        }

        let clean = strip_inline_comment(trimmed).trim();
        if clean.is_empty() {
            return Ok(YamlValue::Null);
        }

        if let Some(alias_name) = clean.strip_prefix('*') {
            return self.resolve_alias(alias_name.trim(), line_no);
        }

        if clean.starts_with('|') || clean.starts_with('>') {
            return self.parse_block_scalar(clean, parent_indent, line_no);
        }

        if trimmed.starts_with('[') || trimmed.starts_with('{') {
            return self.parse_flow_from_inline(trimmed, line_no, depth);
        }

        if trimmed.starts_with('"') {
            let (s, rest) = parse_double_quoted(trimmed, line_no)?;
            ensure_trailing_comment_only(rest, line_no)?;
            check_string_len(&s, line_no)?;
            return Ok(YamlValue::Str(s));
        }

        if trimmed.starts_with('\'') {
            let (s, rest) = parse_single_quoted(trimmed, line_no)?;
            ensure_trailing_comment_only(rest, line_no)?;
            check_string_len(&s, line_no)?;
            return Ok(YamlValue::Str(s));
        }

        parse_plain_scalar(clean, line_no)
    }

    fn resolve_alias(&mut self, name: &str, line_no: usize) -> Result<YamlValue, OrcaYamlError> {
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err(OrcaYamlError::Syntax {
                line: line_no,
                message: format!("invalid alias name '*{name}'"),
            });
        }
        self.alias_count += 1;
        if self.alias_count > ORCA_YAML_MAX_ALIAS_COUNT {
            return Err(OrcaYamlError::AliasLimit {
                message: format!(
                    "YAML alias dereference count {} exceeds limit {}",
                    self.alias_count, ORCA_YAML_MAX_ALIAS_COUNT
                ),
                count: self.alias_count,
                max_aliases: ORCA_YAML_MAX_ALIAS_COUNT,
            });
        }
        let target = self
            .anchors
            .get(name)
            .cloned()
            .ok_or_else(|| OrcaYamlError::Syntax {
                line: line_no,
                message: format!("undefined YAML alias '*{name}'"),
            })?;
        let nodes = count_yaml_nodes(&target);
        self.count_node(nodes)?;
        Ok(target)
    }
}

impl YamlParser {
    fn parse_block_scalar(
        &mut self,
        header: &str,
        parent_indent: usize,
        line_no: usize,
    ) -> Result<YamlValue, OrcaYamlError> {
        let mut chars = header.chars();
        let style = chars.next().ok_or_else(|| OrcaYamlError::Syntax {
            line: line_no,
            message: "empty block scalar header".to_string(),
        })?;
        let folded = style == '>';
        let mut chomp = BlockChomp::Clip;
        let mut explicit_indent: Option<usize> = None;

        for ch in chars {
            match ch {
                '-' => chomp = BlockChomp::Strip,
                '+' => chomp = BlockChomp::Keep,
                '1'..='9' => {
                    explicit_indent = Some((ch as u8 - b'0') as usize);
                }
                ' ' | '\t' => {}
                _ => {
                    return Err(OrcaYamlError::Syntax {
                        line: line_no,
                        message: format!("invalid block scalar indicator '{header}'"),
                    });
                }
            }
        }

        let mut raw_block_lines: Vec<String> = Vec::new();
        let mut detected_indent: Option<usize> =
            explicit_indent.map(|offset| parent_indent + offset);

        while self.pos < self.lines.len() {
            let line = &self.lines[self.pos];
            let trimmed = line.trim();
            if trimmed.is_empty() {
                raw_block_lines.push(line.clone());
                self.pos += 1;
                continue;
            }
            let leading_spaces = line.bytes().take_while(|&b| b == b' ').count();
            let content_indent = match detected_indent {
                Some(ind) => ind,
                None => {
                    if leading_spaces <= parent_indent {
                        break;
                    }
                    detected_indent = Some(leading_spaces);
                    leading_spaces
                }
            };
            if leading_spaces < content_indent {
                break;
            }
            raw_block_lines.push(line.clone());
            self.pos += 1;
        }

        let content_indent = detected_indent.unwrap_or(parent_indent + 1);
        let mut trailing_empty = 0usize;
        while let Some(last) = raw_block_lines.last() {
            if last.trim().is_empty() {
                raw_block_lines.pop();
                trailing_empty += 1;
            } else {
                break;
            }
        }

        let stripped: Vec<String> = raw_block_lines
            .iter()
            .map(|l| {
                if l.len() >= content_indent {
                    l[content_indent..].to_string()
                } else {
                    String::new()
                }
            })
            .collect();

        let mut out = if !folded {
            stripped.join("\n")
        } else {
            fold_block_scalar_lines(&stripped)
        };

        if !stripped.is_empty() {
            match chomp {
                BlockChomp::Strip => {}
                BlockChomp::Clip => out.push('\n'),
                BlockChomp::Keep => {
                    out.push('\n');
                    for _ in 0..trailing_empty {
                        out.push('\n');
                    }
                }
            }
        } else if matches!(chomp, BlockChomp::Keep) {
            for _ in 0..trailing_empty {
                out.push('\n');
            }
        }

        check_string_len(&out, line_no)?;
        Ok(YamlValue::Str(out))
    }

    fn parse_multiline_flow(&mut self, depth: usize) -> Result<YamlValue, OrcaYamlError> {
        let line_no = self.pos + 1;
        let first = self.lines[self.pos].trim().to_string();
        self.pos += 1;
        self.parse_flow_from_inline(&first, line_no, depth)
    }

    fn parse_flow_from_inline(
        &mut self,
        first_chunk: &str,
        start_line: usize,
        depth: usize,
    ) -> Result<YamlValue, OrcaYamlError> {
        let mut buf = strip_inline_comment(first_chunk).to_string();
        while !flow_is_balanced(&buf) {
            if self.pos >= self.lines.len() {
                return Err(OrcaYamlError::Syntax {
                    line: start_line,
                    message: "unterminated flow sequence or mapping".to_string(),
                });
            }
            buf.push('\n');
            buf.push_str(strip_inline_comment(&self.lines[self.pos]));
            self.pos += 1;
        }
        let mut flow = FlowParser {
            src: &buf,
            idx: 0,
            line_no: start_line,
            anchors: &mut self.anchors,
            alias_count: &mut self.alias_count,
            expanded_nodes: &mut self.expanded_nodes,
        };
        let val = flow.parse_value(depth)?;
        flow.skip_ws();
        if flow.idx < flow.src.len() {
            return Err(OrcaYamlError::Syntax {
                line: start_line,
                message: format!("unexpected trailing flow tokens '{}'", &flow.src[flow.idx..]),
            });
        }
        Ok(val)
    }
}

#[derive(Clone, Copy)]
enum BlockChomp {
    Clip,
    Strip,
    Keep,
}

fn fold_block_scalar_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == 0 {
            out.push_str(line);
            continue;
        }
        let prev = &lines[idx - 1];
        let prev_more_indented = prev.starts_with(' ') || prev.starts_with('\t');
        let cur_more_indented = line.starts_with(' ') || line.starts_with('\t');
        if line.is_empty() {
            out.push('\n');
        } else if prev.is_empty() {
            // Blank line already emitted '\n' separating paragraphs.
            out.push_str(line);
        } else if prev_more_indented || cur_more_indented {
            out.push('\n');
            out.push_str(line);
        } else {
            out.push(' ');
            out.push_str(line);
        }
    }
    out
}

struct FlowParser<'a> {
    src: &'a str,
    idx: usize,
    line_no: usize,
    anchors: &'a mut BTreeMap<String, YamlValue>,
    alias_count: &'a mut usize,
    expanded_nodes: &'a mut usize,
}

impl<'a> FlowParser<'a> {
    fn skip_ws(&mut self) {
        while let Some(b) = self.src.as_bytes().get(self.idx) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.idx += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.as_bytes().get(self.idx).copied()
    }

    fn count_node(&mut self, n: usize) -> Result<(), OrcaYamlError> {
        *self.expanded_nodes = self.expanded_nodes.saturating_add(n);
        if *self.expanded_nodes > ORCA_YAML_MAX_EXPANDED_NODES {
            return Err(OrcaYamlError::AliasLimit {
                message: format!(
                    "YAML expanded node count exceeded safety limit ({ORCA_YAML_MAX_EXPANDED_NODES})"
                ),
                count: *self.expanded_nodes,
                max_aliases: ORCA_YAML_MAX_ALIAS_COUNT,
            });
        }
        Ok(())
    }

    fn parse_value(&mut self, depth: usize) -> Result<YamlValue, OrcaYamlError> {
        if depth > ORCA_YAML_MAX_DEPTH {
            return Err(OrcaYamlError::DepthExceeded {
                max_depth: ORCA_YAML_MAX_DEPTH,
                line: self.line_no,
            });
        }
        self.skip_ws();
        self.count_node(1)?;
        match self.peek() {
            Some(b'[') => self.parse_seq(depth + 1),
            Some(b'{') => self.parse_map(depth + 1),
            Some(b'"') => {
                let (s, rest) = parse_double_quoted(&self.src[self.idx..], self.line_no)?;
                self.idx = self.src.len() - rest.len();
                check_string_len(&s, self.line_no)?;
                Ok(YamlValue::Str(s))
            }
            Some(b'\'') => {
                let (s, rest) = parse_single_quoted(&self.src[self.idx..], self.line_no)?;
                self.idx = self.src.len() - rest.len();
                check_string_len(&s, self.line_no)?;
                Ok(YamlValue::Str(s))
            }
            Some(b'&') => {
                self.idx += 1;
                let name = self.read_flow_token();
                if name.is_empty() {
                    return Err(OrcaYamlError::Syntax {
                        line: self.line_no,
                        message: "empty anchor name in flow collection".to_string(),
                    });
                }
                let val = self.parse_value(depth)?;
                self.anchors.insert(name, val.clone());
                Ok(val)
            }
            Some(b'*') => {
                self.idx += 1;
                let name = self.read_flow_token();
                *self.alias_count += 1;
                if *self.alias_count > ORCA_YAML_MAX_ALIAS_COUNT {
                    return Err(OrcaYamlError::AliasLimit {
                        message: format!(
                            "YAML alias dereference count {} exceeds limit {}",
                            *self.alias_count, ORCA_YAML_MAX_ALIAS_COUNT
                        ),
                        count: *self.alias_count,
                        max_aliases: ORCA_YAML_MAX_ALIAS_COUNT,
                    });
                }
                let target = self
                    .anchors
                    .get(&name)
                    .cloned()
                    .ok_or_else(|| OrcaYamlError::Syntax {
                        line: self.line_no,
                        message: format!("undefined YAML alias '*{name}' in flow collection"),
                    })?;
                let nodes = count_yaml_nodes(&target);
                self.count_node(nodes)?;
                Ok(target)
            }
            Some(_) => {
                let start = self.idx;
                while let Some(b) = self.peek() {
                    if matches!(b, b',' | b']' | b'}') {
                        break;
                    }
                    self.idx += 1;
                }
                let raw = self.src[start..self.idx].trim();
                parse_plain_scalar(raw, self.line_no)
            }
            None => Err(OrcaYamlError::Syntax {
                line: self.line_no,
                message: "unexpected end of flow value".to_string(),
            }),
        }
    }

    fn read_flow_token(&mut self) -> String {
        let start = self.idx;
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b',' | b']' | b'}' | b':' | b'[' | b'{') {
                break;
            }
            self.idx += 1;
        }
        self.src[start..self.idx].to_string()
    }

    fn parse_seq(&mut self, depth: usize) -> Result<YamlValue, OrcaYamlError> {
        self.idx += 1; // '['
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b']') {
                self.idx += 1;
                return Ok(YamlValue::Seq(items));
            }
            items.push(self.parse_value(depth)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.idx += 1;
                }
                Some(b']') => {
                    self.idx += 1;
                    return Ok(YamlValue::Seq(items));
                }
                _ => {
                    return Err(OrcaYamlError::Syntax {
                        line: self.line_no,
                        message: "expected ',' or ']' in flow sequence".to_string(),
                    });
                }
            }
        }
    }

    fn parse_map(&mut self, depth: usize) -> Result<YamlValue, OrcaYamlError> {
        self.idx += 1; // '{'
        let mut entries = Vec::new();
        let mut seen = BTreeSet::new();
        let mut merged = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.idx += 1;
                return Ok(YamlValue::Map(finalize_merged_map(entries, merged)));
            }
            let key = self.parse_flow_key()?;
            if !seen.insert(key.clone()) {
                return Err(OrcaYamlError::DuplicateKey {
                    key,
                    line: self.line_no,
                });
            }
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(OrcaYamlError::Syntax {
                    line: self.line_no,
                    message: "expected ':' after key in flow mapping".to_string(),
                });
            }
            self.idx += 1;
            let val = self.parse_value(depth)?;
            if key == "<<" {
                collect_merge_entries(&val, self.line_no, &mut merged)?;
            } else {
                entries.push((key, val));
            }
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.idx += 1,
                Some(b'}') => {
                    self.idx += 1;
                    return Ok(YamlValue::Map(finalize_merged_map(entries, merged)));
                }
                _ => {
                    return Err(OrcaYamlError::Syntax {
                        line: self.line_no,
                        message: "expected ',' or '}' in flow mapping".to_string(),
                    });
                }
            }
        }
    }

    fn parse_flow_key(&mut self) -> Result<String, OrcaYamlError> {
        self.skip_ws();
        match self.peek() {
            Some(b'"') => {
                let (s, rest) = parse_double_quoted(&self.src[self.idx..], self.line_no)?;
                self.idx = self.src.len() - rest.len();
                check_string_len(&s, self.line_no)?;
                Ok(s)
            }
            Some(b'\'') => {
                let (s, rest) = parse_single_quoted(&self.src[self.idx..], self.line_no)?;
                self.idx = self.src.len() - rest.len();
                check_string_len(&s, self.line_no)?;
                Ok(s)
            }
            _ => {
                let start = self.idx;
                while let Some(b) = self.peek() {
                    if matches!(b, b':' | b',' | b'}') {
                        break;
                    }
                    self.idx += 1;
                }
                let k = self.src[start..self.idx].trim().to_string();
                if k.is_empty() {
                    return Err(OrcaYamlError::Syntax {
                        line: self.line_no,
                        message: "empty key in flow mapping".to_string(),
                    });
                }
                check_string_len(&k, self.line_no)?;
                Ok(k)
            }
        }
    }
}

fn is_seq_item(content: &str) -> bool {
    content == "-" || content.starts_with("- ") || content.starts_with("-\t")
}

fn split_mapping_entry(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut flow_depth: usize = 0;

    for (i, &b) in bytes.iter().enumerate() {
        if in_double {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_double = false;
            }
            continue;
        }
        if in_single {
            if b == b'\'' {
                in_single = false;
            }
            continue;
        }
        match b {
            b'"' => in_double = true,
            b'\'' => in_single = true,
            b'[' | b'{' => flow_depth += 1,
            b']' | b'}' => flow_depth = flow_depth.saturating_sub(1),
            b'#' if i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t' => break,
            b':' if flow_depth == 0 => {
                let next = bytes.get(i + 1).copied();
                if matches!(next, None | Some(b' ') | Some(b'\t')) {
                    return Some((&line[..i], &line[i + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_mapping_key(raw: &str, line_no: usize) -> Result<String, OrcaYamlError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(OrcaYamlError::Syntax {
            line: line_no,
            message: "empty mapping key".to_string(),
        });
    }
    let key = if t.starts_with('"') {
        let (s, rest) = parse_double_quoted(t, line_no)?;
        if !rest.trim().is_empty() {
            return Err(OrcaYamlError::Syntax {
                line: line_no,
                message: format!("unexpected trailing characters after quoted key '{t}'"),
            });
        }
        s
    } else if t.starts_with('\'') {
        let (s, rest) = parse_single_quoted(t, line_no)?;
        if !rest.trim().is_empty() {
            return Err(OrcaYamlError::Syntax {
                line: line_no,
                message: format!("unexpected trailing characters after quoted key '{t}'"),
            });
        }
        s
    } else {
        t.to_string()
    };
    check_string_len(&key, line_no)?;
    Ok(key)
}

fn split_anchor_prefix(raw: &str) -> Option<(&str, &str)> {
    let rest = raw.strip_prefix('&')?;
    let end = rest
        .find(|c: char| c.is_whitespace())
        .unwrap_or(rest.len());
    let name = &rest[..end];
    if name.is_empty() {
        return None;
    }
    Some((name, &rest[end..]))
}

fn strip_inline_comment(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_double {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_double = false;
            }
            continue;
        }
        if in_single {
            if b == b'\'' {
                in_single = false;
            }
            continue;
        }
        match b {
            b'"' => in_double = true,
            b'\'' => in_single = true,
            b'#' if i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t' => {
                return &s[..i];
            }
            _ => {}
        }
    }
    s
}

fn flow_is_balanced(s: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut depth: i32 = 0;
    for b in s.bytes() {
        if in_double {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_double = false;
            }
            continue;
        }
        if in_single {
            if b == b'\'' {
                in_single = false;
            }
            continue;
        }
        match b {
            b'"' => in_double = true,
            b'\'' => in_single = true,
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth -= 1,
            _ => {}
        }
    }
    !in_single && !in_double && depth <= 0
}

fn parse_single_quoted(input: &str, line_no: usize) -> Result<(String, &str), OrcaYamlError> {
    let rest = input.strip_prefix('\'').ok_or_else(|| OrcaYamlError::Syntax {
        line: line_no,
        message: "expected opening single quote".to_string(),
    })?;
    let mut out = String::new();
    let mut chars = rest.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch == '\'' {
            if let Some(&(_, '\'')) = chars.peek() {
                chars.next();
                out.push('\'');
            } else {
                return Ok((out, &rest[idx + 1..]));
            }
        } else {
            out.push(ch);
        }
    }
    Err(OrcaYamlError::Syntax {
        line: line_no,
        message: "unterminated single-quoted YAML string".to_string(),
    })
}

fn parse_double_quoted(input: &str, line_no: usize) -> Result<(String, &str), OrcaYamlError> {
    let rest = input.strip_prefix('"').ok_or_else(|| OrcaYamlError::Syntax {
        line: line_no,
        message: "expected opening double quote".to_string(),
    })?;
    let mut out = String::new();
    let mut chars = rest.char_indices();
    while let Some((idx, ch)) = chars.next() {
        if ch == '"' {
            return Ok((out, &rest[idx + 1..]));
        }
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let (_, esc) = chars.next().ok_or_else(|| OrcaYamlError::Syntax {
            line: line_no,
            message: "unterminated escape sequence in double-quoted string".to_string(),
        })?;
        match esc {
            '0' => out.push('\0'),
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            't' | '\t' => out.push('\t'),
            'n' => out.push('\n'),
            'v' => out.push('\x0b'),
            'f' => out.push('\x0c'),
            'r' => out.push('\r'),
            'e' => out.push('\x1b'),
            ' ' => out.push(' '),
            '"' => out.push('"'),
            '/' => out.push('/'),
            '\\' => out.push('\\'),
            'x' => out.push(read_hex_escape(&mut chars, 2, line_no)?),
            'u' => out.push(read_hex_escape(&mut chars, 4, line_no)?),
            'U' => out.push(read_hex_escape(&mut chars, 8, line_no)?),
            other => {
                return Err(OrcaYamlError::Syntax {
                    line: line_no,
                    message: format!("unsupported escape sequence '\\{other}'"),
                });
            }
        }
    }
    Err(OrcaYamlError::Syntax {
        line: line_no,
        message: "unterminated double-quoted YAML string".to_string(),
    })
}

fn read_hex_escape(
    chars: &mut std::str::CharIndices<'_>,
    digits: usize,
    line_no: usize,
) -> Result<char, OrcaYamlError> {
    let mut hex_buf = String::with_capacity(digits);
    for _ in 0..digits {
        let (_, c) = chars.next().ok_or_else(|| OrcaYamlError::Syntax {
            line: line_no,
            message: format!("truncated \\x/\\u/\\U escape (expected {digits} hex digits)"),
        })?;
        hex_buf.push(c);
    }
    let cp = u32::from_str_radix(&hex_buf, 16).map_err(|_| OrcaYamlError::Syntax {
        line: line_no,
        message: format!("invalid hex escape '{hex_buf}'"),
    })?;
    char::from_u32(cp).ok_or_else(|| OrcaYamlError::Syntax {
        line: line_no,
        message: format!("invalid Unicode scalar U+{cp:04X}"),
    })
}

fn ensure_trailing_comment_only(rest: &str, line_no: usize) -> Result<(), OrcaYamlError> {
    let t = rest.trim();
    if t.is_empty() || t.starts_with('#') {
        Ok(())
    } else {
        Err(OrcaYamlError::Syntax {
            line: line_no,
            message: format!("unexpected trailing text after quoted scalar: '{t}'"),
        })
    }
}

fn check_string_len(s: &str, line_no: usize) -> Result<(), OrcaYamlError> {
    if s.len() > ORCA_YAML_MAX_STRING_BYTES {
        return Err(OrcaYamlError::StringTooLong {
            bytes: s.len(),
            max_bytes: ORCA_YAML_MAX_STRING_BYTES,
            line: line_no,
        });
    }
    Ok(())
}

fn parse_plain_scalar(raw: &str, line_no: usize) -> Result<YamlValue, OrcaYamlError> {
    check_string_len(raw, line_no)?;
    match raw {
        "" | "null" | "Null" | "NULL" | "~" => Ok(YamlValue::Null),
        "true" | "True" | "TRUE" => Ok(YamlValue::Bool(true)),
        "false" | "False" | "FALSE" => Ok(YamlValue::Bool(false)),
        _ => {
            if let Some(hex_digits) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X"))
                && let Ok(v) = i64::from_str_radix(hex_digits, 16)
            {
                return Ok(YamlValue::Int(v));
            }
            if let Some(oct_digits) = raw.strip_prefix("0o").or_else(|| raw.strip_prefix("0O"))
                && let Ok(v) = i64::from_str_radix(oct_digits, 8)
            {
                return Ok(YamlValue::Int(v));
            }
            if let Ok(v) = raw.parse::<i64>() {
                return Ok(YamlValue::Int(v));
            }
            if (raw.contains('.') || raw.contains('e') || raw.contains('E'))
                && let Ok(v) = raw.parse::<f64>()
                && v.is_finite()
            {
                return Ok(YamlValue::Float(v));
            }
            Ok(YamlValue::Str(raw.to_string()))
        }
    }
}

fn collect_merge_entries(
    val: &YamlValue,
    line_no: usize,
    out: &mut Vec<(String, YamlValue)>,
) -> Result<(), OrcaYamlError> {
    match val {
        YamlValue::Map(entries) => {
            for (k, v) in entries {
                if !out.iter().any(|(ek, _)| ek == k) {
                    out.push((k.clone(), v.clone()));
                }
            }
            Ok(())
        }
        YamlValue::Seq(items) => {
            for item in items {
                match item {
                    YamlValue::Map(entries) => {
                        for (k, v) in entries {
                            if !out.iter().any(|(ek, _)| ek == k) {
                                out.push((k.clone(), v.clone()));
                            }
                        }
                    }
                    _ => {
                        return Err(OrcaYamlError::Syntax {
                            line: line_no,
                            message: "merge key '<<' sequence must contain only mappings".to_string(),
                        });
                    }
                }
            }
            Ok(())
        }
        _ => Err(OrcaYamlError::Syntax {
            line: line_no,
            message: "merge key '<<' value must be a mapping or sequence of mappings".to_string(),
        }),
    }
}

fn finalize_merged_map(
    mut explicit: Vec<(String, YamlValue)>,
    merged: Vec<(String, YamlValue)>,
) -> Vec<(String, YamlValue)> {
    for (k, v) in merged {
        if !explicit.iter().any(|(ek, _)| ek == &k) {
            explicit.push((k, v));
        }
    }
    explicit
}

fn count_yaml_nodes(val: &YamlValue) -> usize {
    match val {
        YamlValue::Seq(items) => 1 + items.iter().map(count_yaml_nodes).sum::<usize>(),
        YamlValue::Map(entries) => {
            1 + entries.iter().map(|(_, v)| count_yaml_nodes(v)).sum::<usize>()
        }
        _ => 1,
    }
}

// ---------------------------------------------------------------------------
// Typed `orca.yaml` Extractor
// ---------------------------------------------------------------------------

/// Parses and validates an `orca.yaml` UTF-8 document into a strongly-typed
/// [`OrcaYamlConfig`], rejecting non-mapping roots, duplicate keys anywhere,
/// oversize files/strings, and excessive alias expansions.
pub fn parse_orca_yaml(input: &str) -> Result<OrcaYamlConfig, OrcaYamlError> {
    let root = parse_yaml_strict(input)?;
    let map = match root {
        YamlValue::Map(entries) => entries,
        other => {
            return Err(OrcaYamlError::NonMappingRoot {
                message: format!(
                    "orca.yaml root must be a YAML mapping, found {}",
                    yaml_kind_name(&other)
                ),
            });
        }
    };
    extract_orca_yaml_config(&map)
}

fn yaml_kind_name(v: &YamlValue) -> &'static str {
    match v {
        YamlValue::Null => "null",
        YamlValue::Bool(_) => "boolean",
        YamlValue::Int(_) => "integer",
        YamlValue::Float(_) => "float",
        YamlValue::Str(_) => "string",
        YamlValue::Seq(_) => "sequence",
        YamlValue::Map(_) => "mapping",
    }
}

fn extract_orca_yaml_config(map: &[(String, YamlValue)]) -> Result<OrcaYamlConfig, OrcaYamlError> {
    let mut cfg = OrcaYamlConfig::default();

    for (key, val) in map {
        match key.as_str() {
            "setup" => {
                cfg.setup.extend(extract_string_or_list("setup", val)?);
            }
            "teardown" | "archive" => {
                cfg.teardown.extend(extract_string_or_list(key, val)?);
            }
            "env" => {
                cfg.env = extract_string_map("env", val)?;
            }
            "scripts" => {
                let scripts_map = val.as_map().ok_or_else(|| OrcaYamlError::Schema {
                    field: "scripts".to_string(),
                    message: "expected mapping of script name -> command".to_string(),
                })?;
                for (sk, sv) in scripts_map {
                    let cmds = extract_string_or_list(&format!("scripts.{sk}"), sv)?;
                    let joined = cmds.join("\n");
                    if sk == "setup" {
                        cfg.setup.extend(cmds);
                    } else if sk == "teardown" || sk == "archive" {
                        cfg.teardown.extend(cmds);
                    }
                    cfg.scripts.insert(sk.clone(), joined);
                }
            }
            "services" => {
                cfg.services = extract_services(val)?;
            }
            "ports" => {
                cfg.ports = extract_ports(val)?;
            }
            "worktree_include" | "worktreeInclude" => {
                cfg.worktree_include = extract_string_list(key, val)?;
            }
            "auto_archive_days" | "autoArchiveDays" => {
                cfg.auto_archive_days = extract_opt_u32(key, val)?;
            }
            "setup_agent_startup_policy" | "setupAgentStartupPolicy" => {
                let raw = val.as_str().ok_or_else(|| OrcaYamlError::Schema {
                    field: key.clone(),
                    message: "expected 'start-immediately' or 'wait-for-setup'".to_string(),
                })?;
                cfg.setup_agent_startup_policy = match raw {
                    "start-immediately" | "start_immediately" => {
                        SetupAgentStartupPolicy::StartImmediately
                    }
                    "wait-for-setup" | "wait_for_setup" => SetupAgentStartupPolicy::WaitForSetup,
                    other => {
                        return Err(OrcaYamlError::Schema {
                            field: key.clone(),
                            message: format!("unknown setupAgentStartupPolicy '{other}'"),
                        });
                    }
                };
            }
            "issue_command" | "issueCommand" => {
                cfg.issue_command = extract_opt_string(key, val)?;
            }
            "shared_directories" | "sharedDirectories" => {
                apply_shared_directories(key, val, &mut cfg)?;
            }
            "worktree" => {
                let wt_map = val.as_map().ok_or_else(|| OrcaYamlError::Schema {
                    field: "worktree".to_string(),
                    message: "expected mapping".to_string(),
                })?;
                for (wk, wv) in wt_map {
                    match wk.as_str() {
                        "sharedDirectories" | "shared_directories" => {
                            apply_shared_directories("worktree.sharedDirectories", wv, &mut cfg)?;
                        }
                        "include" | "worktreeInclude" | "worktree_include" => {
                            cfg.worktree_include
                                .extend(extract_string_list("worktree.include", wv)?);
                        }
                        _ => {}
                    }
                }
            }
            "default_tabs" | "defaultTabs" => {
                let list = val.as_seq().ok_or_else(|| OrcaYamlError::Schema {
                    field: key.clone(),
                    message: "expected sequence".to_string(),
                })?;
                if list.len() > ORCA_YAML_MAX_DEFAULT_TABS {
                    cfg.warnings.push(format!(
                        "{key} length {} exceeds max {ORCA_YAML_MAX_DEFAULT_TABS}; list dropped",
                        list.len()
                    ));
                } else {
                    cfg.default_tabs = extract_default_tabs(list)?;
                }
            }
            "environment_recipes" | "environmentRecipes" => {
                let list = val.as_seq().ok_or_else(|| OrcaYamlError::Schema {
                    field: key.clone(),
                    message: "expected sequence".to_string(),
                })?;
                if list.len() > ORCA_YAML_MAX_ENV_RECIPES {
                    cfg.warnings.push(format!(
                        "{key} length {} exceeds max {ORCA_YAML_MAX_ENV_RECIPES}; list dropped",
                        list.len()
                    ));
                } else {
                    cfg.environment_recipes = extract_env_recipes(list)?;
                }
            }
            _ => {}
        }
    }

    for cmd in cfg.setup.iter().chain(cfg.teardown.iter()) {
        if is_posix_shebang_script(cmd) {
            cfg.warnings.push(
                "POSIX '#!' shebang script requires a POSIX shell (refused by Windows cmd.exe)"
                    .to_string(),
            );
            break;
        }
    }

    Ok(cfg)
}

fn apply_shared_directories(
    field: &str,
    val: &YamlValue,
    cfg: &mut OrcaYamlConfig,
) -> Result<(), OrcaYamlError> {
    let mut dirs = extract_string_list(field, val)?;
    if dirs.len() > ORCA_YAML_MAX_SHARED_DIRS {
        cfg.warnings.push(format!(
            "{field} length {} exceeds max {ORCA_YAML_MAX_SHARED_DIRS}; entries after {ORCA_YAML_MAX_SHARED_DIRS} ignored",
            dirs.len()
        ));
        dirs.truncate(ORCA_YAML_MAX_SHARED_DIRS);
    }
    cfg.shared_directories = dirs;
    Ok(())
}

fn extract_string_or_list(field: &str, val: &YamlValue) -> Result<Vec<String>, OrcaYamlError> {
    match val {
        YamlValue::Null => Ok(Vec::new()),
        YamlValue::Str(s) => Ok(vec![s.clone()]),
        YamlValue::Seq(_) => extract_string_list(field, val),
        _ => Err(OrcaYamlError::Schema {
            field: field.to_string(),
            message: "expected string or list of strings".to_string(),
        }),
    }
}

fn extract_string_list(field: &str, val: &YamlValue) -> Result<Vec<String>, OrcaYamlError> {
    if matches!(val, YamlValue::Null) {
        return Ok(Vec::new());
    }
    let seq = val.as_seq().ok_or_else(|| OrcaYamlError::Schema {
        field: field.to_string(),
        message: "expected sequence of strings".to_string(),
    })?;
    let mut out = Vec::with_capacity(seq.len());
    for (idx, item) in seq.iter().enumerate() {
        let s = item.scalar_to_string().ok_or_else(|| OrcaYamlError::Schema {
            field: format!("{field}[{idx}]"),
            message: "expected scalar string".to_string(),
        })?;
        out.push(s);
    }
    Ok(out)
}

fn extract_string_map(
    field: &str,
    val: &YamlValue,
) -> Result<BTreeMap<String, String>, OrcaYamlError> {
    if matches!(val, YamlValue::Null) {
        return Ok(BTreeMap::new());
    }
    let map = val.as_map().ok_or_else(|| OrcaYamlError::Schema {
        field: field.to_string(),
        message: "expected mapping of string -> scalar".to_string(),
    })?;
    let mut out = BTreeMap::new();
    for (k, v) in map {
        let s = v.scalar_to_string().ok_or_else(|| OrcaYamlError::Schema {
            field: format!("{field}.{k}"),
            message: "expected scalar value".to_string(),
        })?;
        out.insert(k.clone(), s);
    }
    Ok(out)
}

fn extract_opt_string(field: &str, val: &YamlValue) -> Result<Option<String>, OrcaYamlError> {
    match val {
        YamlValue::Null => Ok(None),
        YamlValue::Str(s) => Ok(Some(s.clone())),
        _ => Err(OrcaYamlError::Schema {
            field: field.to_string(),
            message: "expected string or null".to_string(),
        }),
    }
}

fn extract_opt_u32(field: &str, val: &YamlValue) -> Result<Option<u32>, OrcaYamlError> {
    match val {
        YamlValue::Null => Ok(None),
        YamlValue::Int(v) => u32::try_from(*v)
            .map(Some)
            .map_err(|_| OrcaYamlError::Schema {
                field: field.to_string(),
                message: format!("integer {v} out of u32 range"),
            }),
        _ => Err(OrcaYamlError::Schema {
            field: field.to_string(),
            message: "expected non-negative integer".to_string(),
        }),
    }
}

fn extract_ports(val: &YamlValue) -> Result<Vec<u16>, OrcaYamlError> {
    if matches!(val, YamlValue::Null) {
        return Ok(Vec::new());
    }
    let seq = val.as_seq().ok_or_else(|| OrcaYamlError::Schema {
        field: "ports".to_string(),
        message: "expected sequence of port numbers".to_string(),
    })?;
    let mut out = Vec::with_capacity(seq.len());
    for (idx, item) in seq.iter().enumerate() {
        let p = match item {
            YamlValue::Int(v) if *v > 0 && *v <= i64::from(u16::MAX) => *v as u16,
            _ => {
                return Err(OrcaYamlError::Schema {
                    field: format!("ports[{idx}]"),
                    message: "port must be an integer in 1..=65535".to_string(),
                });
            }
        };
        out.push(p);
    }
    Ok(out)
}

fn extract_services(val: &YamlValue) -> Result<Vec<OrcaYamlService>, OrcaYamlError> {
    if matches!(val, YamlValue::Null) {
        return Ok(Vec::new());
    }
    let seq = val.as_seq().ok_or_else(|| OrcaYamlError::Schema {
        field: "services".to_string(),
        message: "expected sequence of service definitions".to_string(),
    })?;
    let mut out = Vec::with_capacity(seq.len());
    for (idx, item) in seq.iter().enumerate() {
        let m = item.as_map().ok_or_else(|| OrcaYamlError::Schema {
            field: format!("services[{idx}]"),
            message: "service entry must be a mapping".to_string(),
        })?;
        let name = m
            .iter()
            .find(|(k, _)| k == "name")
            .and_then(|(_, v)| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| OrcaYamlError::Schema {
                field: format!("services[{idx}].name"),
                message: "service requires non-empty string 'name'".to_string(),
            })?
            .to_string();
        let command = m
            .iter()
            .find(|(k, _)| k == "command")
            .and_then(|(_, v)| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| OrcaYamlError::Schema {
                field: format!("services[{idx}].command"),
                message: "service requires non-empty string 'command'".to_string(),
            })?
            .to_string();
        let cwd = match m.iter().find(|(k, _)| k == "cwd") {
            Some((_, v)) => extract_opt_string(&format!("services[{idx}].cwd"), v)?,
            None => None,
        };
        let env = match m.iter().find(|(k, _)| k == "env") {
            Some((_, v)) => extract_string_map(&format!("services[{idx}].env"), v)?,
            None => BTreeMap::new(),
        };
        let port = match m.iter().find(|(k, _)| k == "port") {
            Some((_, YamlValue::Null)) | None => None,
            Some((_, YamlValue::Int(p))) if *p > 0 && *p <= i64::from(u16::MAX) => Some(*p as u16),
            Some(_) => {
                return Err(OrcaYamlError::Schema {
                    field: format!("services[{idx}].port"),
                    message: "service port must be in 1..=65535".to_string(),
                });
            }
        };
        let auto_start = match m.iter().find(|(k, _)| k == "auto_start" || k == "autoStart") {
            Some((_, YamlValue::Bool(b))) => *b,
            None => true,
            Some(_) => {
                return Err(OrcaYamlError::Schema {
                    field: format!("services[{idx}].auto_start"),
                    message: "expected boolean".to_string(),
                });
            }
        };
        out.push(OrcaYamlService {
            name,
            command,
            cwd,
            env,
            port,
            auto_start,
        });
    }
    Ok(out)
}

fn extract_default_tabs(seq: &[YamlValue]) -> Result<Vec<OrcaYamlDefaultTab>, OrcaYamlError> {
    let mut out = Vec::with_capacity(seq.len());
    for (idx, item) in seq.iter().enumerate() {
        let m = item.as_map().ok_or_else(|| OrcaYamlError::Schema {
            field: format!("defaultTabs[{idx}]"),
            message: "expected mapping".to_string(),
        })?;
        let kind = m
            .iter()
            .find(|(k, _)| k == "kind" || k == "type")
            .and_then(|(_, v)| v.as_str())
            .ok_or_else(|| OrcaYamlError::Schema {
                field: format!("defaultTabs[{idx}].kind"),
                message: "expected string 'kind' or 'type'".to_string(),
            })?
            .to_string();
        let title = m
            .iter()
            .find(|(k, _)| k == "title")
            .and_then(|(_, v)| v.as_str())
            .map(ToOwned::to_owned);
        let command = m
            .iter()
            .find(|(k, _)| k == "command")
            .and_then(|(_, v)| v.as_str())
            .map(ToOwned::to_owned);
        out.push(OrcaYamlDefaultTab {
            kind,
            title,
            command,
        });
    }
    Ok(out)
}

fn extract_env_recipes(seq: &[YamlValue]) -> Result<Vec<OrcaYamlEnvironmentRecipe>, OrcaYamlError> {
    let mut out = Vec::with_capacity(seq.len());
    for (idx, item) in seq.iter().enumerate() {
        let m = item.as_map().ok_or_else(|| OrcaYamlError::Schema {
            field: format!("environmentRecipes[{idx}]"),
            message: "expected mapping".to_string(),
        })?;
        let name = m
            .iter()
            .find(|(k, _)| k == "name")
            .and_then(|(_, v)| v.as_str())
            .ok_or_else(|| OrcaYamlError::Schema {
                field: format!("environmentRecipes[{idx}].name"),
                message: "expected string 'name'".to_string(),
            })?
            .to_string();
        let get_str = |k: &str| {
            m.iter()
                .find(|(ek, _)| ek == k)
                .and_then(|(_, v)| v.as_str())
                .map(ToOwned::to_owned)
        };
        out.push(OrcaYamlEnvironmentRecipe {
            name,
            create: get_str("create").or_else(|| get_str("command")),
            suspend: get_str("suspend"),
            resume: get_str("resume"),
            destroy: get_str("destroy"),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_orca_yaml_with_anchors_block_scalars_and_flow() {
        let yaml = r#"
# Orca workspace configuration
setup:
  - cargo fetch --offline
  - &build_cmd "cargo check --workspace"
teardown:
  - rm -rf tmp-artifacts
env:
  RUST_LOG: info
  QUOTED: 'it''s ready'
  ESCAPED: "line1\nline2"
scripts:
  lint: |
    cargo clippy --all-targets
    echo done
  notes: >-
    folded paragraph
    continues here
services:
  - name: api
    command: *build_cmd
    cwd: crates/api
    port: 8080
    auto_start: true
    env: { MODE: dev, PORT: 8080 }
ports: [3000, 8080]
worktree_include: [".env.local", "certs/"]
auto_archive_days: 14
setupAgentStartupPolicy: wait-for-setup
issueCommand: "orca agent run {{artifact_url}}"
worktree:
  sharedDirectories:
    - node_modules
    - target
defaultTabs:
  - kind: terminal
    title: Shell
"#;
        let cfg = parse_orca_yaml(yaml).expect("valid orca.yaml");
        assert_eq!(cfg.setup, vec!["cargo fetch --offline", "cargo check --workspace"]);
        assert_eq!(cfg.teardown, vec!["rm -rf tmp-artifacts"]);
        assert_eq!(cfg.env.get("RUST_LOG").map(String::as_str), Some("info"));
        assert_eq!(cfg.env.get("QUOTED").map(String::as_str), Some("it's ready"));
        assert_eq!(cfg.env.get("ESCAPED").map(String::as_str), Some("line1\nline2"));
        assert_eq!(
            cfg.scripts.get("lint").map(String::as_str),
            Some("cargo clippy --all-targets\necho done\n")
        );
        assert_eq!(
            cfg.scripts.get("notes").map(String::as_str),
            Some("folded paragraph continues here")
        );
        assert_eq!(cfg.services.len(), 1);
        assert_eq!(cfg.services[0].name, "api");
        assert_eq!(cfg.services[0].command, "cargo check --workspace");
        assert_eq!(cfg.services[0].port, Some(8080));
        assert_eq!(cfg.services[0].env.get("MODE").map(String::as_str), Some("dev"));
        assert_eq!(cfg.ports, vec![3000, 8080]);
        assert_eq!(cfg.worktree_include, vec![".env.local", "certs/"]);
        assert_eq!(cfg.auto_archive_days, Some(14));
        assert_eq!(cfg.setup_agent_startup_policy, SetupAgentStartupPolicy::WaitForSetup);
        assert_eq!(cfg.shared_directories, vec!["node_modules", "target"]);
        assert_eq!(cfg.default_tabs.len(), 1);

        let h1 = setup_trust_hash(&cfg);
        assert_eq!(h1.len(), 64);
        assert_eq!(h1, setup_trust_hash(&cfg));

        let mut modified = cfg.clone();
        modified.setup.push("echo changed".to_string());
        assert_ne!(h1, setup_trust_hash(&modified));

        let mut svc_mod = cfg.clone();
        svc_mod.services[0].command = "cargo run".to_string();
        assert_ne!(h1, setup_trust_hash(&svc_mod));
    }

    #[test]
    fn rejects_duplicate_keys_at_any_depth_and_in_flow_maps() {
        let top = "setup:\n  - a\nsetup:\n  - b\n";
        let err = parse_orca_yaml(top).expect_err("duplicate top-level key");
        assert_eq!(err.ide_error_code(), Some(OrcaIdeErrorCode::YamlDuplicateKey));

        let nested = "env:\n  FOO: 1\n  BAR: 2\n  FOO: 3\n";
        let err = parse_orca_yaml(nested).expect_err("duplicate nested key");
        assert_eq!(err.ide_error_code(), Some(OrcaIdeErrorCode::YamlDuplicateKey));

        let in_seq = "services:\n  - name: a\n    command: run\n    name: b\n";
        let err = parse_orca_yaml(in_seq).expect_err("duplicate key inside sequence mapping");
        assert_eq!(err.ide_error_code(), Some(OrcaIdeErrorCode::YamlDuplicateKey));

        let flow = "env: { A: 1, B: 2, \"A\": 3 }\n";
        let err = parse_orca_yaml(flow).expect_err("duplicate key inside flow map");
        assert_eq!(err.ide_error_code(), Some(OrcaIdeErrorCode::YamlDuplicateKey));
    }

    #[test]
    fn rejects_non_mapping_root_and_size_and_string_limits() {
        let seq_root = "- item1\n- item2\n";
        assert!(matches!(
            parse_orca_yaml(seq_root),
            Err(OrcaYamlError::NonMappingRoot { .. })
        ));

        let scalar_root = "just_a_string\n";
        assert!(matches!(
            parse_orca_yaml(scalar_root),
            Err(OrcaYamlError::NonMappingRoot { .. })
        ));

        let huge = format!("env:\n  BIG: \"{}\"\n", "a".repeat(ORCA_YAML_MAX_BYTES));
        let err = parse_orca_yaml(&huge).expect_err("exceeds 256KiB");
        assert_eq!(err.ide_error_code(), Some(OrcaIdeErrorCode::YamlSizeExceeded));

        let mut huge_utf16 = vec![0xFFu8, 0xFE];
        huge_utf16.extend([b'a', 0u8].repeat(ORCA_YAML_MAX_BYTES));
        let err = parse_orca_yaml_bytes(&huge_utf16).expect_err("UTF-16 > 256KiB");
        assert_eq!(err.ide_error_code(), Some(OrcaIdeErrorCode::YamlSizeExceeded));

        let long_str = format!("issueCommand: \"{}\"\n", "x".repeat(ORCA_YAML_MAX_STRING_BYTES + 1));
        let err = parse_orca_yaml(&long_str).expect_err("string > 64KiB");
        assert_eq!(err.ide_error_code(), Some(OrcaIdeErrorCode::YamlSizeExceeded));
    }

    #[test]
    fn rejects_alias_limit_billion_laughs_and_excessive_depth() {
        let mut yaml = String::from("a: &a val\nlist:\n");
        for _ in 0..=ORCA_YAML_MAX_ALIAS_COUNT {
            yaml.push_str("  - *a\n");
        }
        let err = parse_orca_yaml(&yaml).expect_err("more than 100 aliases");
        assert_eq!(err.ide_error_code(), Some(OrcaIdeErrorCode::YamlAliasLimit));

        // Exponential alias amplification bomb (only 18 aliases, > 2^18 nodes!)
        let mut bomb = String::from("a: &a [x, x]\n");
        let names = ["b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p"];
        let mut prev = "a";
        for n in names {
            bomb.push_str(&format!("{n}: &{n} [*{prev}, *{prev}]\n"));
            prev = n;
        }
        let err = parse_orca_yaml(&bomb).expect_err("billion laughs must be blocked");
        assert_eq!(err.ide_error_code(), Some(OrcaIdeErrorCode::YamlAliasLimit));

        let deep = format!(
            "env: {}{}",
            "[".repeat(ORCA_YAML_MAX_DEPTH + 5),
            "]".repeat(ORCA_YAML_MAX_DEPTH + 5)
        );
        assert!(matches!(
            parse_orca_yaml(&deep),
            Err(OrcaYamlError::DepthExceeded { .. })
        ));
    }

    #[test]
    fn enforces_list_caps_and_posix_shebang_warning() {
        let mut yaml = String::from("scripts:\n  setup: |\n    #!/usr/bin/env bash\n    echo hi\nsharedDirectories:\n");
        for i in 0..105 {
            yaml.push_str(&format!("  - dir_{i}\n"));
        }
        yaml.push_str("defaultTabs:\n");
        for i in 0..260 {
            yaml.push_str(&format!("  - kind: terminal\n    title: t{i}\n"));
        }
        let cfg = parse_orca_yaml(&yaml).expect("caps produce warnings");
        assert_eq!(cfg.shared_directories.len(), ORCA_YAML_MAX_SHARED_DIRS);
        assert!(cfg.default_tabs.is_empty());
        assert!(cfg.warnings.len() >= 3);
        assert!(is_posix_shebang_script(&cfg.setup[0]));
    }
}
