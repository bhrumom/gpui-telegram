//! `SkillService` (`SKILL.md`, `<= 32` assigned skills/agent, shared `openbot-installed-skills-v1.json`)
//! and `McpGateway` (`<= 100` `stdio`/`http` MCP servers, OAuth 2.0 PKCE `S256`,
//! `openbot_computer_use` descriptor, connectors, and JSON-RPC 2.0 tool execution) (`OB-F10` Part A).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openbot_contracts::agents::AgentSummary;
use openbot_contracts::limits::{INPUT_LIMITS, utf16_len};
use openbot_storage::{
    COMPUTER_USE_MCP_SERVER_ID, COMPUTER_USE_MCP_SERVER_NAME, GITHUB_CONNECTOR_MCP_SERVER_ID,
    GITHUB_CONNECTOR_MCP_SERVER_NAME, GITHUB_CONNECTOR_MCP_SERVER_URL, McpKeyValue,
    McpServerConfig, McpServerStore, McpTransport, OpenBotDatabase, StorageError,
    is_reserved_mcp_server_name, now_rfc3339_millis, redact_secrets, write_file_atomically,
    write_json_file_atomically,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

use crate::error::{OpenBotRuntimeError, RuntimeResult};

pub const INSTALLED_SKILLS_MANIFEST_FILE: &str = "openbot-installed-skills-v1.json";
pub const SKILL_OWNERSHIP_MARKER: &str = ".openbot-managed.json";
pub const SKILL_DESCRIPTION_MAX_LENGTH: usize = 1_024;
pub const SKILL_NAME_MAX_LENGTH: usize = 80;
pub const MAX_SKILL_FILE_BYTES: usize = 256 * 1024;

pub const MCP_SERVERS_JSON_FILE: &str = "openbot-mcp-servers-v1.json";
pub const MCP_ACCESS_JSON_FILE: &str = "openbot-mcp-access-v1.json";
pub const MCP_OAUTH_JSON_FILE: &str = "openbot-mcp-oauth-v1.json";
pub const OPENBOT_COMPUTER_USE_NAME: &str = "openbot_computer_use";
pub const COMPUTER_USE_GUIDE_URI: &str = "computer-use://guide";

fn map_storage_err(err: StorageError) -> OpenBotRuntimeError {
    match err {
        StorageError::Validation(msg) => OpenBotRuntimeError::Validation(msg),
        StorageError::NotFound(msg) => OpenBotRuntimeError::NotFound(msg),
        StorageError::Conflict(msg) => OpenBotRuntimeError::Conflict(msg),
        StorageError::LimitExceeded(msg) => OpenBotRuntimeError::LimitExceeded(msg),
        StorageError::Forbidden(msg) => OpenBotRuntimeError::Forbidden(msg),
        other => OpenBotRuntimeError::Storage(other),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

// ============================================================================
// Part 1: SkillService (`SKILL.md` + `openbot-installed-skills-v1.json`)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillFrontmatterProblem {
    FrontmatterMissing,
    FrontmatterInvalid,
    DescriptionLength,
    NameMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillFrontmatter {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    pub body: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<SkillFrontmatterProblem>,
}

/// Shared manifest entry matching `SCOPE.md § 3.3` exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkillEntry {
    pub skill_id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub state: String,
    pub origin: String,
    pub path: String,
    pub sha256: String,
    pub updated_at: String,
}

/// Shared manifest format persisted at `<data_dir>/openbot-installed-skills-v1.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkillsManifest {
    pub version: u32,
    pub by_agent: BTreeMap<String, Vec<InstalledSkillEntry>>,
}

impl Default for InstalledSkillsManifest {
    fn default() -> Self {
        Self {
            version: 1,
            by_agent: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSkillInput {
    pub agent_id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSkillOutcome {
    pub skill: InstalledSkillEntry,
    pub state: String,
    pub assigned_skill_count: usize,
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalLibrarySkillMeta {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub version: u64,
    pub files: Vec<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagedSkillMarker {
    managed_by: String,
    slug: String,
    version: u32,
    sha256: String,
}

pub fn is_valid_skill_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

fn unquote_yaml_scalar(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        if let Ok(parsed) = serde_json::from_str::<String>(trimmed) {
            return parsed;
        }
        return trimmed[1..trimmed.len() - 1].to_string();
    }
    if trimmed.len() >= 2 && trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        return trimmed[1..trimmed.len() - 1].replace("''", "'");
    }
    trimmed.to_string()
}

fn parse_yaml_inline_list(raw: &str) -> Option<Vec<String>> {
    let trimmed = raw.trim();
    if !(trimmed.starts_with('[') && trimmed.ends_with(']')) {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner.is_empty() {
        return Some(Vec::new());
    }
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in inner.chars() {
        if let Some(q) = quote {
            cur.push(ch);
            if escaped {
                escaped = false;
            } else if q == '"' && ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
            cur.push(ch);
        } else if ch == ',' {
            let val = unquote_yaml_scalar(&cur);
            if !val.is_empty() {
                items.push(val);
            }
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    let tail = unquote_yaml_scalar(&cur);
    if !tail.is_empty() {
        items.push(tail);
    }
    Some(items)
}

/// Parses YAML frontmatter (`name`, `title`, `description`, `triggers`, `tools`) and markdown body
/// from a `SKILL.md` file, mirroring upstream `src/backend/skill-frontmatter.ts`.
pub fn parse_skill_frontmatter(markdown: &str, expected_slug: Option<&str>) -> SkillFrontmatter {
    let sha256 = sha256_hex(markdown.as_bytes());
    let stripped = markdown.strip_prefix('\u{FEFF}').unwrap_or(markdown);
    let normalized = stripped.replace("\r\n", "\n");

    let Some(rest) = normalized.strip_prefix("---\n") else {
        return SkillFrontmatter {
            name: String::new(),
            title: None,
            description: String::new(),
            triggers: Vec::new(),
            tools: Vec::new(),
            body: normalized,
            sha256,
            problem: Some(SkillFrontmatterProblem::FrontmatterMissing),
        };
    };

    let mut fm_lines: Vec<&str> = Vec::new();
    let mut body_start_byte: Option<usize> = None;
    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n');
        if trimmed == "---" {
            body_start_byte = Some(offset + line.len());
            break;
        }
        fm_lines.push(trimmed);
        offset += line.len();
    }

    let Some(body_idx) = body_start_byte else {
        return SkillFrontmatter {
            name: String::new(),
            title: None,
            description: String::new(),
            triggers: Vec::new(),
            tools: Vec::new(),
            body: normalized,
            sha256,
            problem: Some(SkillFrontmatterProblem::FrontmatterMissing),
        };
    };
    let body = rest[body_idx..].trim_start_matches('\n').to_string();

    let mut name = String::new();
    let mut title: Option<String> = None;
    let mut description = String::new();
    let mut triggers: Vec<String> = Vec::new();
    let mut tools: Vec<String> = Vec::new();
    let mut active_list: Option<&str> = None;
    let mut active_block_desc = false;
    let mut saw_valid_key = false;

    for raw_line in fm_lines {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if active_block_desc && (raw_line.starts_with(' ') || raw_line.starts_with('\t')) {
            if !description.is_empty() {
                description.push(' ');
            }
            description.push_str(trimmed);
            continue;
        }
        if let Some(list_key) = active_list {
            if let Some(item_rest) = trimmed.strip_prefix("- ") {
                let val = unquote_yaml_scalar(item_rest);
                if !val.is_empty() {
                    if list_key == "triggers" {
                        triggers.push(val);
                    } else if list_key == "tools" {
                        tools.push(val);
                    }
                }
                continue;
            }
        }
        active_list = None;
        active_block_desc = false;

        let Some((k, v)) = raw_line.split_once(':') else {
            return SkillFrontmatter {
                name,
                title,
                description,
                triggers,
                tools,
                body,
                sha256,
                problem: Some(SkillFrontmatterProblem::FrontmatterInvalid),
            };
        };
        let key = k.trim();
        if key.is_empty() {
            return SkillFrontmatter {
                name,
                title,
                description,
                triggers,
                tools,
                body,
                sha256,
                problem: Some(SkillFrontmatterProblem::FrontmatterInvalid),
            };
        }
        saw_valid_key = true;
        let val = v.trim();
        match key {
            "name" => name = unquote_yaml_scalar(val),
            "title" => {
                let parsed = unquote_yaml_scalar(val);
                if !parsed.is_empty() {
                    title = Some(parsed);
                }
            }
            "description" => {
                if val == "|" || val == ">" || val == "|-" || val == ">-" {
                    active_block_desc = true;
                } else {
                    description = unquote_yaml_scalar(val);
                }
            }
            "triggers" => {
                if val.is_empty() {
                    active_list = Some("triggers");
                } else if let Some(list) = parse_yaml_inline_list(val) {
                    triggers = list;
                } else {
                    triggers = vec![unquote_yaml_scalar(val)];
                }
            }
            "tools" => {
                if val.is_empty() {
                    active_list = Some("tools");
                } else if let Some(list) = parse_yaml_inline_list(val) {
                    tools = list;
                } else {
                    tools = vec![unquote_yaml_scalar(val)];
                }
            }
            _ => {}
        }
    }

    let problem = if !saw_valid_key {
        Some(SkillFrontmatterProblem::FrontmatterInvalid)
    } else if description.trim().is_empty() || utf16_len(&description) > SKILL_DESCRIPTION_MAX_LENGTH
    {
        Some(SkillFrontmatterProblem::DescriptionLength)
    } else if expected_slug.is_some_and(|slug| name != slug) {
        Some(SkillFrontmatterProblem::NameMismatch)
    } else {
        None
    };

    SkillFrontmatter {
        name,
        title,
        description: description.trim().to_string(),
        triggers,
        tools,
        body,
        sha256,
        problem,
    }
}

pub fn render_skill_markdown(
    slug: &str,
    name: &str,
    description: &str,
    triggers: &[String],
    tools: &[String],
    instructions: &str,
) -> String {
    let trimmed_inst = instructions.trim_start_matches('\u{FEFF}');
    if trimmed_inst.starts_with("---\n") || trimmed_inst.starts_with("---\r\n") {
        let parsed = parse_skill_frontmatter(trimmed_inst, Some(slug));
        if parsed.problem.is_none() {
            return if trimmed_inst.ends_with('\n') {
                trimmed_inst.to_string()
            } else {
                format!("{trimmed_inst}\n")
            };
        }
    }
    let quoted_title = serde_json::to_string(name).unwrap_or_else(|_| format!("\"{name}\""));
    let quoted_desc =
        serde_json::to_string(description).unwrap_or_else(|_| format!("\"{description}\""));
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("name: {slug}\n"));
    if name != slug {
        out.push_str(&format!("title: {quoted_title}\n"));
    }
    out.push_str(&format!("description: {quoted_desc}\n"));
    if !triggers.is_empty() {
        out.push_str("triggers:\n");
        for t in triggers {
            let qt = serde_json::to_string(t).unwrap_or_else(|_| format!("\"{t}\""));
            out.push_str(&format!("  - {qt}\n"));
        }
    }
    if !tools.is_empty() {
        out.push_str("tools:\n");
        for t in tools {
            let qt = serde_json::to_string(t).unwrap_or_else(|_| format!("\"{t}\""));
            out.push_str(&format!("  - {qt}\n"));
        }
    }
    out.push_str("---\n\n");
    out.push_str(instructions.trim_start_matches('\n'));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

pub struct SkillService {
    database: OpenBotDatabase,
    data_dir: PathBuf,
    manifest_lock: Mutex<InstalledSkillsManifest>,
}

impl SkillService {
    pub fn open(database: OpenBotDatabase, data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir)?;
        let manifest_path = data_dir.join(INSTALLED_SKILLS_MANIFEST_FILE);
        let manifest = read_manifest_from_disk(&manifest_path)?;
        Ok(Arc::new(Self {
            database,
            data_dir,
            manifest_lock: Mutex::new(manifest),
        }))
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.data_dir.join(INSTALLED_SKILLS_MANIFEST_FILE)
    }

    pub fn local_library_dir(&self) -> PathBuf {
        self.data_dir.join("local-skills")
    }

    fn resolve_workspace_root(&self, agent_id: &str) -> RuntimeResult<PathBuf> {
        if let Some(agent) = self
            .database
            .agent_roster()
            .get_agent(agent_id)
            .map_err(map_storage_err)?
        {
            if !agent.workspace_path.trim().is_empty() {
                return Ok(PathBuf::from(agent.workspace_path));
            }
        }
        let safe_segment: String = agent_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Ok(self.data_dir.join("Agents").join(safe_segment))
    }

    pub fn read_skill_frontmatter(
        &self,
        markdown: &str,
        expected_slug: Option<&str>,
    ) -> SkillFrontmatter {
        parse_skill_frontmatter(markdown, expected_slug)
    }

    pub fn assigned_skill_count(&self, agent_id: &str) -> RuntimeResult<usize> {
        let mut guard = self.manifest_lock.lock();
        *guard = read_manifest_from_disk(&self.manifest_path())?;
        Ok(count_assigned_entries(
            guard.by_agent.get(agent_id).map(Vec::as_slice).unwrap_or(&[]),
        ))
    }

    pub fn list_skills(&self, agent_id: &str) -> RuntimeResult<Vec<InstalledSkillEntry>> {
        let mut guard = self.manifest_lock.lock();
        *guard = read_manifest_from_disk(&self.manifest_path())?;
        let mut list = guard.by_agent.get(agent_id).cloned().unwrap_or_default();
        for entry in &mut list {
            let p = Path::new(&entry.path);
            if let Ok(bytes) = fs::read(p) {
                let current_hash = sha256_hex(&bytes);
                if current_hash != entry.sha256 {
                    entry.state = "modified".to_string();
                } else if !entry.enabled {
                    entry.state = "disabled".to_string();
                } else {
                    entry.state = "installed".to_string();
                }
            }
        }
        Ok(list)
    }

    pub fn save_skill(&self, input: SaveSkillInput) -> RuntimeResult<SaveSkillOutcome> {
        let agent_id = input.agent_id.trim();
        if agent_id.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "agentId is required to save a skill.".to_string(),
            ));
        }
        let slug = input.slug.trim().to_ascii_lowercase();
        if !is_valid_skill_slug(&slug) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Invalid skill slug '{slug}': use 1-64 lowercase ASCII letters, digits, or hyphens."
            )));
        }
        let name = input.name.trim();
        if name.is_empty() || utf16_len(name) > SKILL_NAME_MAX_LENGTH {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Skill name must be between 1 and {SKILL_NAME_MAX_LENGTH} characters."
            )));
        }
        let desc_trimmed = input.description.trim();
        if utf16_len(desc_trimmed) > SKILL_DESCRIPTION_MAX_LENGTH {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Skill description exceeds maximum length of {SKILL_DESCRIPTION_MAX_LENGTH} characters."
            )));
        }
        let effective_desc = if desc_trimmed.is_empty() {
            name
        } else {
            desc_trimmed
        };
        if input.instructions.len() > MAX_SKILL_FILE_BYTES {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "SKILL.md content exceeds maximum size of {MAX_SKILL_FILE_BYTES} bytes."
            )));
        }

        let origin = input
            .origin
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("local")
            .to_string();
        let enabled = input.enabled.unwrap_or(true);
        let skill_id = format!("{origin}:{slug}");

        let workspace = self.resolve_workspace_root(agent_id)?;
        let skill_dir = workspace.join(".agents").join("skills").join(&slug);
        let skill_file = skill_dir.join("SKILL.md");
        let marker_file = skill_dir.join(SKILL_OWNERSHIP_MARKER);

        let markdown = render_skill_markdown(
            &slug,
            name,
            effective_desc,
            &input.triggers,
            &input.tools,
            &input.instructions,
        );
        let sha256 = sha256_hex(markdown.as_bytes());
        let updated_at = now_rfc3339_millis();

        let mut guard = self.manifest_lock.lock();
        *guard = read_manifest_from_disk(&self.manifest_path())?;
        let entries = guard.by_agent.entry(agent_id.to_string()).or_default();
        let existing_idx = entries
            .iter()
            .position(|e| e.slug == slug || e.skill_id == skill_id);

        if existing_idx.is_none() && is_counted_origin(&origin) {
            let current_assigned = count_assigned_entries(entries);
            if current_assigned >= INPUT_LIMITS.agent_skills {
                return Err(OpenBotRuntimeError::LimitExceeded(format!(
                    "Agent '{agent_id}' already has the maximum of {} assigned skills (skill limit reached).",
                    INPUT_LIMITS.agent_skills
                )));
            }
        }

        reject_if_symlink(&skill_dir)?;
        reject_if_symlink(&skill_file)?;
        reject_if_symlink(&marker_file)?;

        write_file_atomically(&skill_file, markdown.as_bytes(), true).map_err(map_storage_err)?;
        let marker = ManagedSkillMarker {
            managed_by: "openbot".to_string(),
            slug: slug.clone(),
            version: 1,
            sha256: sha256.clone(),
        };
        write_json_file_atomically(&marker_file, &marker, true).map_err(map_storage_err)?;

        let state = if enabled {
            "installed".to_string()
        } else {
            "disabled".to_string()
        };
        let entry = InstalledSkillEntry {
            skill_id,
            slug,
            name: name.to_string(),
            description: effective_desc.to_string(),
            enabled,
            state: state.clone(),
            origin,
            path: skill_file.to_string_lossy().to_string(),
            sha256: sha256.clone(),
            updated_at,
        };

        if let Some(idx) = existing_idx {
            entries[idx] = entry.clone();
        } else {
            entries.push(entry.clone());
        }
        let assigned_skill_count = count_assigned_entries(entries);
        write_json_file_atomically(self.manifest_path(), &*guard, true)
            .map_err(map_storage_err)?;

        Ok(SaveSkillOutcome {
            path: entry.path.clone(),
            sha256,
            state,
            assigned_skill_count,
            skill: entry,
        })
    }

    pub fn set_skill_enabled(
        &self,
        agent_id: &str,
        skill_id: &str,
        enabled: bool,
    ) -> RuntimeResult<InstalledSkillEntry> {
        let mut guard = self.manifest_lock.lock();
        *guard = read_manifest_from_disk(&self.manifest_path())?;
        let entries = guard.by_agent.get_mut(agent_id).ok_or_else(|| {
            OpenBotRuntimeError::NotFound(format!("No installed skills found for agent '{agent_id}'."))
        })?;
        let entry = entries
            .iter_mut()
            .find(|e| e.skill_id == skill_id || e.slug == skill_id)
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!(
                    "Installed skill '{skill_id}' not found for agent '{agent_id}'."
                ))
            })?;

        if !enabled && is_modified_on_disk(entry) {
            return Err(OpenBotRuntimeError::Conflict(format!(
                "Refusing to disable skill '{}': files were modified on disk.",
                entry.skill_id
            )));
        }

        entry.enabled = enabled;
        entry.state = if enabled {
            "installed".to_string()
        } else {
            "disabled".to_string()
        };
        entry.updated_at = now_rfc3339_millis();
        let updated = entry.clone();
        write_json_file_atomically(self.manifest_path(), &*guard, true)
            .map_err(map_storage_err)?;
        Ok(updated)
    }

    pub fn uninstall_skill(&self, agent_id: &str, skill_id: &str) -> RuntimeResult<bool> {
        let mut guard = self.manifest_lock.lock();
        *guard = read_manifest_from_disk(&self.manifest_path())?;
        let Some(entries) = guard.by_agent.get_mut(agent_id) else {
            return Ok(false);
        };
        let Some(idx) = entries
            .iter()
            .position(|e| e.skill_id == skill_id || e.slug == skill_id)
        else {
            return Ok(false);
        };

        let target = &entries[idx];
        if target.origin == "workspace" {
            return Err(OpenBotRuntimeError::Forbidden(format!(
                "Cannot uninstall workspace-folder skill '{}'.",
                target.skill_id
            )));
        }
        if is_modified_on_disk(target) {
            return Err(OpenBotRuntimeError::Conflict(format!(
                "Refusing to uninstall skill '{}': files were modified on disk.",
                target.skill_id
            )));
        }

        let removed = entries.remove(idx);
        let skill_path = PathBuf::from(&removed.path);
        if let Some(dir) = skill_path.parent() {
            if dir.join(SKILL_OWNERSHIP_MARKER).exists() {
                let _ = fs::remove_dir_all(dir);
            }
        }
        write_json_file_atomically(self.manifest_path(), &*guard, true)
            .map_err(map_storage_err)?;
        Ok(true)
    }

    pub fn execute_skill_tool(
        &self,
        agent_id: &str,
        tool_name: &str,
        args: &JsonValue,
    ) -> RuntimeResult<JsonValue> {
        let short = tool_name.strip_prefix("openbot.").unwrap_or(tool_name);
        match short {
            "create_skill" => {
                let source_path = args
                    .get("sourcePath")
                    .and_then(JsonValue::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation("create_skill requires sourcePath.".to_string())
                    })?;
                let (meta, instructions) = self.create_library_skill_from_dir(Path::new(source_path))?;
                let _ = self.save_skill(SaveSkillInput {
                    agent_id: agent_id.to_string(),
                    slug: meta.slug.clone(),
                    name: meta.name.clone(),
                    description: meta.description.clone(),
                    instructions: instructions.clone(),
                    triggers: Vec::new(),
                    tools: Vec::new(),
                    origin: Some("local".to_string()),
                    enabled: Some(true),
                })?;
                Ok(json!({
                    "id": meta.id,
                    "slug": meta.slug,
                    "name": meta.name,
                    "description": meta.description,
                    "version": meta.version,
                    "files": meta.files,
                    "instructions": instructions,
                    "updatedAt": meta.updated_at,
                }))
            }
            "revise_skill" => {
                let skill_id = args
                    .get("skillId")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation("revise_skill requires skillId.".to_string())
                    })?;
                let expected_revision = args
                    .get("expectedRevision")
                    .and_then(JsonValue::as_u64)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation(
                            "revise_skill requires expectedRevision.".to_string(),
                        )
                    })?;
                let source_path = args
                    .get("sourcePath")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation("revise_skill requires sourcePath.".to_string())
                    })?;
                let (meta, instructions) =
                    self.revise_library_skill(skill_id, expected_revision, Path::new(source_path))?;
                let _ = self.save_skill(SaveSkillInput {
                    agent_id: agent_id.to_string(),
                    slug: meta.slug.clone(),
                    name: meta.name.clone(),
                    description: meta.description.clone(),
                    instructions: instructions.clone(),
                    triggers: Vec::new(),
                    tools: Vec::new(),
                    origin: Some("local".to_string()),
                    enabled: Some(true),
                })?;
                Ok(json!({
                    "id": meta.id,
                    "slug": meta.slug,
                    "name": meta.name,
                    "description": meta.description,
                    "version": meta.version,
                    "files": meta.files,
                    "instructions": instructions,
                    "updatedAt": meta.updated_at,
                }))
            }
            "list_local_skills" => {
                let skills = self.list_library_skills()?;
                Ok(json!({ "skills": skills }))
            }
            "read_local_skill" => {
                let skill_id = args
                    .get("skillId")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation("read_local_skill requires skillId.".to_string())
                    })?;
                let (meta, instructions) = self.read_library_skill(skill_id)?;
                Ok(json!({
                    "id": meta.id,
                    "slug": meta.slug,
                    "name": meta.name,
                    "description": meta.description,
                    "version": meta.version,
                    "files": meta.files,
                    "instructions": instructions,
                    "updatedAt": meta.updated_at,
                }))
            }
            "install_local_skill" => {
                let target_agent = args
                    .get("agentId")
                    .and_then(JsonValue::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(agent_id);
                let skill_id = args
                    .get("skillId")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation(
                            "install_local_skill requires skillId.".to_string(),
                        )
                    })?;
                let (meta, instructions) = self.read_library_skill(skill_id)?;
                let outcome = self.save_skill(SaveSkillInput {
                    agent_id: target_agent.to_string(),
                    slug: meta.slug,
                    name: meta.name,
                    description: meta.description,
                    instructions,
                    triggers: Vec::new(),
                    tools: Vec::new(),
                    origin: Some("local".to_string()),
                    enabled: Some(true),
                })?;
                Ok(serde_json::to_value(outcome)?)
            }
            "set_skill_enabled" => {
                let target_agent = args
                    .get("agentId")
                    .and_then(JsonValue::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(agent_id);
                let skill_id = args
                    .get("skillId")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation(
                            "set_skill_enabled requires skillId.".to_string(),
                        )
                    })?;
                let enabled = args
                    .get("enabled")
                    .and_then(JsonValue::as_bool)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation(
                            "set_skill_enabled requires boolean enabled.".to_string(),
                        )
                    })?;
                let updated = self.set_skill_enabled(target_agent, skill_id, enabled)?;
                Ok(serde_json::to_value(updated)?)
            }
            "uninstall_skill" => {
                let target_agent = args
                    .get("agentId")
                    .and_then(JsonValue::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(agent_id);
                let skill_id = args
                    .get("skillId")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation(
                            "uninstall_skill requires skillId.".to_string(),
                        )
                    })?;
                let removed = self.uninstall_skill(target_agent, skill_id)?;
                if !removed {
                    return Err(OpenBotRuntimeError::NotFound(format!(
                        "Installed skill '{skill_id}' not found for agent '{target_agent}'."
                    )));
                }
                Ok(json!({
                    "agentId": target_agent,
                    "skillId": skill_id,
                    "removed": true,
                }))
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unknown local skill tool '{tool_name}'."
            ))),
        }
    }

    fn create_library_skill_from_dir(
        &self,
        source_dir: &Path,
    ) -> RuntimeResult<(LocalLibrarySkillMeta, String)> {
        let skill_md = source_dir.join("SKILL.md");
        let content = fs::read_to_string(&skill_md).map_err(|e| {
            OpenBotRuntimeError::Validation(format!(
                "Unable to read SKILL.md from '{}': {e}",
                source_dir.display()
            ))
        })?;
        let folder_slug = source_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let fm = parse_skill_frontmatter(&content, None);
        let slug = if is_valid_skill_slug(&fm.name) {
            fm.name.clone()
        } else if is_valid_skill_slug(&folder_slug) {
            folder_slug
        } else {
            return Err(OpenBotRuntimeError::Validation(
                "SKILL.md must declare a valid lowercase slug in frontmatter `name`.".to_string(),
            ));
        };
        if fm.description.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "SKILL.md frontmatter requires a non-empty `description`.".to_string(),
            ));
        }
        let id = format!("local-skill-{}", Uuid::new_v4());
        let name = fm.title.unwrap_or_else(|| slug.clone());
        let meta = LocalLibrarySkillMeta {
            id: id.clone(),
            slug,
            name,
            description: fm.description,
            version: 1,
            files: vec!["SKILL.md".to_string()],
            updated_at: now_rfc3339_millis(),
        };
        let lib_dir = self.local_library_dir().join(&id);
        write_file_atomically(lib_dir.join("SKILL.md"), content.as_bytes(), true)
            .map_err(map_storage_err)?;
        write_json_file_atomically(lib_dir.join("meta.json"), &meta, true)
            .map_err(map_storage_err)?;
        Ok((meta, content))
    }

    fn revise_library_skill(
        &self,
        skill_id: &str,
        expected_revision: u64,
        source_dir: &Path,
    ) -> RuntimeResult<(LocalLibrarySkillMeta, String)> {
        let (mut meta, _) = self.read_library_skill(skill_id)?;
        if meta.version != expected_revision {
            return Err(OpenBotRuntimeError::Conflict(format!(
                "Revision conflict for '{skill_id}': expected {expected_revision}, current is {}.",
                meta.version
            )));
        }
        let content = fs::read_to_string(source_dir.join("SKILL.md")).map_err(|e| {
            OpenBotRuntimeError::Validation(format!(
                "Unable to read SKILL.md from '{}': {e}",
                source_dir.display()
            ))
        })?;
        let fm = parse_skill_frontmatter(&content, Some(&meta.slug));
        if let Some(prob) = fm.problem {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Invalid SKILL.md frontmatter for '{}': {prob:?}",
                meta.slug
            )));
        }
        meta.version += 1;
        meta.description = fm.description;
        if let Some(title) = fm.title {
            meta.name = title;
        }
        meta.updated_at = now_rfc3339_millis();
        let lib_dir = self.local_library_dir().join(skill_id);
        write_file_atomically(lib_dir.join("SKILL.md"), content.as_bytes(), true)
            .map_err(map_storage_err)?;
        write_json_file_atomically(lib_dir.join("meta.json"), &meta, true)
            .map_err(map_storage_err)?;
        Ok((meta, content))
    }

    fn list_library_skills(&self) -> RuntimeResult<Vec<LocalLibrarySkillMeta>> {
        let root = self.local_library_dir();
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let meta_path = entry.path().join("meta.json");
            if let Ok(raw) = fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<LocalLibrarySkillMeta>(&raw) {
                    out.push(meta);
                }
            }
        }
        out.sort_by(|a, b| a.slug.cmp(&b.slug).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    fn read_library_skill(&self, skill_id: &str) -> RuntimeResult<(LocalLibrarySkillMeta, String)> {
        let lib_dir = self.local_library_dir().join(skill_id);
        let meta_raw = fs::read_to_string(lib_dir.join("meta.json")).map_err(|_| {
            OpenBotRuntimeError::NotFound(format!("Local library skill '{skill_id}' not found."))
        })?;
        let meta: LocalLibrarySkillMeta = serde_json::from_str(&meta_raw)?;
        let content = fs::read_to_string(lib_dir.join("SKILL.md"))?;
        Ok((meta, content))
    }
}

fn is_counted_origin(origin: &str) -> bool {
    origin != "builtin" && origin != "home"
}

fn count_assigned_entries(entries: &[InstalledSkillEntry]) -> usize {
    entries
        .iter()
        .filter(|e| is_counted_origin(&e.origin))
        .count()
}

fn read_manifest_from_disk(path: &Path) -> RuntimeResult<InstalledSkillsManifest> {
    match fs::read_to_string(path) {
        Ok(raw) if !raw.trim().is_empty() => {
            let parsed: InstalledSkillsManifest = serde_json::from_str(&raw).map_err(|e| {
                OpenBotRuntimeError::Validation(format!(
                    "Corrupt installed-skills manifest '{}': {e}",
                    path.display()
                ))
            })?;
            Ok(parsed)
        }
        Ok(_) => Ok(InstalledSkillsManifest::default()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(InstalledSkillsManifest::default())
        }
        Err(err) => Err(OpenBotRuntimeError::Io(err)),
    }
}

fn reject_if_symlink(path: &Path) -> RuntimeResult<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(OpenBotRuntimeError::Forbidden(format!(
            "Skill path cannot be a symlink: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(OpenBotRuntimeError::Io(err)),
    }
}

fn is_modified_on_disk(entry: &InstalledSkillEntry) -> bool {
    let skill_path = Path::new(&entry.path);
    let Ok(bytes) = fs::read(skill_path) else {
        return false;
    };
    let current = sha256_hex(&bytes);
    if current != entry.sha256 {
        return true;
    }
    if let Some(dir) = skill_path.parent() {
        let marker_path = dir.join(SKILL_OWNERSHIP_MARKER);
        if let Ok(raw) = fs::read_to_string(marker_path) {
            if let Ok(marker) = serde_json::from_str::<ManagedSkillMarker>(&raw) {
                return marker.managed_by != "openbot"
                    || marker.slug != entry.slug
                    || marker.version != 1
                    || marker.sha256 != current;
            }
        }
    }
    false
}

// ============================================================================
// Part 2: McpGateway (`<= 100` stdio/http MCP servers, PKCE S256, Computer Use)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterMcpServerInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub transport: McpTransport,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<McpKeyValue>,
    #[serde(default)]
    pub env_passthrough: Vec<String>,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: Vec<McpKeyValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PkceS256Challenge {
    pub code_verifier: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerUseDescriptor {
    pub server: McpServerConfig,
    pub qualified_name: String,
    pub guide_uri: String,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnectorDescriptor {
    pub id: String,
    pub name: String,
    pub url: String,
    pub transport: McpTransport,
    pub requires_oauth: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallOutcome {
    pub server_id: String,
    pub server_name: String,
    pub tool_name: String,
    pub is_error: bool,
    pub content: JsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpProbeOutcome {
    pub server_id: String,
    pub name: String,
    pub status: String,
    pub tool_count: usize,
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthState {
    pub server_id: String,
    pub signed_in: bool,
    pub pending_authorization: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpServersJsonMirror {
    version: u32,
    servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpAccessStoreFile {
    version: u32,
    revision: u64,
    servers: BTreeMap<String, Option<Vec<String>>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpOAuthRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_verifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpOAuthStoreFile {
    version: u32,
    records: BTreeMap<String, McpOAuthRecord>,
}

/// Generates an RFC 7636 OAuth 2.0 PKCE `S256` verifier, challenge, and CSRF state
/// using OS cryptographic randomness (`getrandom`), SHA-256, and URL-safe base64 (no padding).
pub fn generate_pkce_s256() -> RuntimeResult<PkceS256Challenge> {
    let mut verifier_bytes = [0u8; 32];
    getrandom::getrandom(&mut verifier_bytes).map_err(|err| {
        OpenBotRuntimeError::Execution(format!("CSPRNG failed generating PKCE verifier: {err}"))
    })?;
    let mut state_bytes = [0u8; 24];
    getrandom::getrandom(&mut state_bytes).map_err(|err| {
        OpenBotRuntimeError::Execution(format!("CSPRNG failed generating OAuth state: {err}"))
    })?;

    let code_verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let digest = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(digest);
    let state = URL_SAFE_NO_PAD.encode(state_bytes);

    Ok(PkceS256Challenge {
        code_verifier,
        code_challenge,
        code_challenge_method: "S256".to_string(),
        state,
    })
}

pub fn verify_pkce_s256(code_verifier: &str, expected_challenge: &str) -> bool {
    if !(43..=128).contains(&code_verifier.len()) {
        return false;
    }
    let computed = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
    let a = computed.as_bytes();
    let b = expected_challenge.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn frame_jsonrpc_request(id: i64, method: &str, params: JsonValue) -> JsonValue {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

/// Parses a JSON-RPC 2.0 response payload from JSONL, LSP `Content-Length` framed stdio output,
/// or Streamable HTTP SSE (`data: {...}`) responses.
pub fn parse_jsonrpc_response(raw: &str) -> RuntimeResult<JsonValue> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(OpenBotRuntimeError::Execution(
            "Empty JSON-RPC 2.0 response from MCP server.".to_string(),
        ));
    }

    // 1. Direct JSON object
    if let Ok(val) = serde_json::from_str::<JsonValue>(trimmed) {
        return unwrap_jsonrpc_envelope(val);
    }

    // 2. Content-Length framed payload or newline-delimited JSON / SSE lines
    let mut last_valid: Option<JsonValue> = None;
    for line in trimmed.lines() {
        let l = line.trim();
        let candidate = l.strip_prefix("data:").map(str::trim).unwrap_or(l);
        if candidate.starts_with('{') {
            if let Ok(val) = serde_json::from_str::<JsonValue>(candidate) {
                if val.get("jsonrpc").and_then(JsonValue::as_str) == Some("2.0")
                    && (val.get("result").is_some() || val.get("error").is_some())
                {
                    last_valid = Some(val);
                }
            }
        }
    }
    if let Some(found) = last_valid {
        return unwrap_jsonrpc_envelope(found);
    }

    Err(OpenBotRuntimeError::Execution(
        "Malformed JSON-RPC 2.0 response from MCP server.".to_string(),
    ))
}

fn unwrap_jsonrpc_envelope(envelope: JsonValue) -> RuntimeResult<JsonValue> {
    if let Some(err_obj) = envelope.get("error") {
        let code = err_obj.get("code").and_then(JsonValue::as_i64).unwrap_or(-32000);
        let message = err_obj
            .get("message")
            .and_then(JsonValue::as_str)
            .unwrap_or("MCP JSON-RPC error");
        return Err(OpenBotRuntimeError::Execution(format!(
            "MCP JSON-RPC error ({code}): {message}"
        )));
    }
    if let Some(result) = envelope.get("result") {
        return Ok(result.clone());
    }
    Ok(envelope)
}

pub struct McpGateway {
    store: McpServerStore,
    data_dir: PathBuf,
    http_client: reqwest::Client,
    access_lock: Mutex<McpAccessStoreFile>,
    oauth_lock: Mutex<McpOAuthStoreFile>,
}

impl McpGateway {
    pub fn open(database: OpenBotDatabase, data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir)?;
        let store = database.mcp_server_store();
        let access = read_json_or_default::<McpAccessStoreFile>(&data_dir.join(MCP_ACCESS_JSON_FILE))?;
        let oauth = read_json_or_default::<McpOAuthStoreFile>(&data_dir.join(MCP_OAUTH_JSON_FILE))?;
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| OpenBotRuntimeError::Http(format!("Failed to build MCP HTTP client: {e}")))?;

        let gateway = Arc::new(Self {
            store,
            data_dir,
            http_client,
            access_lock: Mutex::new(access),
            oauth_lock: Mutex::new(oauth),
        });
        gateway.sync_servers_json()?;
        Ok(gateway)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn servers_json_path(&self) -> PathBuf {
        self.data_dir.join(MCP_SERVERS_JSON_FILE)
    }

    fn sync_servers_json(&self) -> RuntimeResult<()> {
        let servers = self.store.list().map_err(map_storage_err)?;
        let mirror = McpServersJsonMirror {
            version: 1,
            servers,
        };
        write_json_file_atomically(self.servers_json_path(), &mirror, true)
            .map_err(map_storage_err)?;
        Ok(())
    }

    pub fn register_server(&self, input: RegisterMcpServerInput) -> RuntimeResult<McpServerConfig> {
        let trimmed_name = input.name.trim();
        if trimmed_name.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Enter a name for this MCP server.".to_string(),
            ));
        }
        if is_reserved_mcp_server_name(trimmed_name)
            || trimmed_name.eq_ignore_ascii_case(COMPUTER_USE_MCP_SERVER_NAME)
            || trimmed_name.eq_ignore_ascii_case(OPENBOT_COMPUTER_USE_NAME)
        {
            return Err(OpenBotRuntimeError::Validation(format!(
                "OpenBot already uses the reserved built-in MCP server name '{trimmed_name}'."
            )));
        }

        let cfg = McpServerConfig {
            id: input.id.unwrap_or_default(),
            name: trimmed_name.to_string(),
            transport: input.transport,
            enabled: input.enabled.unwrap_or(true),
            command: input.command.unwrap_or_default(),
            args: input.args,
            env: input.env,
            env_passthrough: input.env_passthrough,
            working_directory: input.working_directory.unwrap_or_default(),
            url: input.url.unwrap_or_default(),
            headers: input.headers,
        };

        let saved = self.store.save(&cfg, None).map_err(map_storage_err)?;
        self.sync_servers_json()?;
        Ok(saved)
    }

    pub fn list_servers(&self) -> RuntimeResult<Vec<McpServerConfig>> {
        self.store.list().map_err(map_storage_err)
    }

    pub fn get_server(&self, id_or_name: &str) -> RuntimeResult<Option<McpServerConfig>> {
        if let Some(found) = self.store.get(id_or_name).map_err(map_storage_err)? {
            return Ok(Some(found));
        }
        let servers = self.store.list().map_err(map_storage_err)?;
        Ok(servers
            .into_iter()
            .find(|s| s.name.eq_ignore_ascii_case(id_or_name)))
    }

    pub fn remove_server(&self, mcp_server_id: &str) -> RuntimeResult<bool> {
        let removed = self.store.remove(mcp_server_id).map_err(map_storage_err)?;
        if removed {
            self.sync_servers_json()?;
            {
                let mut access = self.access_lock.lock();
                if access.servers.remove(mcp_server_id).is_some() {
                    access.version = 1;
                    access.revision += 1;
                    write_json_file_atomically(
                        self.data_dir.join(MCP_ACCESS_JSON_FILE),
                        &*access,
                        true,
                    )
                    .map_err(map_storage_err)?;
                }
            }
            {
                let mut oauth = self.oauth_lock.lock();
                if oauth.records.remove(mcp_server_id).is_some() {
                    oauth.version = 1;
                    write_json_file_atomically(
                        self.data_dir.join(MCP_OAUTH_JSON_FILE),
                        &*oauth,
                        true,
                    )
                    .map_err(map_storage_err)?;
                }
            }
        }
        Ok(removed)
    }

    pub fn set_server_enabled(
        &self,
        mcp_server_id: &str,
        enabled: bool,
    ) -> RuntimeResult<McpServerConfig> {
        let updated = self
            .store
            .set_enabled(mcp_server_id, enabled, None)
            .map_err(map_storage_err)?;
        self.sync_servers_json()?;
        Ok(updated)
    }

    pub fn set_server_access(
        &self,
        mcp_server_id: &str,
        allowed_agents: Option<Vec<String>>,
    ) -> RuntimeResult<()> {
        let mut access = self.access_lock.lock();
        access.version = 1;
        access.revision += 1;
        access
            .servers
            .insert(mcp_server_id.to_string(), allowed_agents);
        write_json_file_atomically(self.data_dir.join(MCP_ACCESS_JSON_FILE), &*access, true)
            .map_err(map_storage_err)?;
        Ok(())
    }

    pub fn allows_agent(&self, mcp_server_id: &str, agent_id: &str) -> bool {
        let access = self.access_lock.lock();
        match access.servers.get(mcp_server_id) {
            None | Some(None) => true,
            Some(Some(allowed)) => allowed.iter().any(|id| id == agent_id),
        }
    }

    pub fn generate_pkce_s256(&self) -> RuntimeResult<PkceS256Challenge> {
        generate_pkce_s256()
    }

    pub fn build_oauth_authorize_url(
        &self,
        server_id: &str,
        authorization_endpoint: &str,
        client_id: &str,
        redirect_uri: &str,
        scope: Option<&str>,
    ) -> RuntimeResult<(String, PkceS256Challenge)> {
        let mut url = Url::parse(authorization_endpoint).map_err(|e| {
            OpenBotRuntimeError::Validation(format!("Invalid OAuth authorization_endpoint: {e}"))
        })?;
        if url.scheme() != "https" && url.scheme() != "http" {
            return Err(OpenBotRuntimeError::Validation(
                "OAuth authorization_endpoint must use http or https.".to_string(),
            ));
        }
        if client_id.trim().is_empty() || redirect_uri.trim().is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "OAuth client_id and redirect_uri are required.".to_string(),
            ));
        }

        let pkce = generate_pkce_s256()?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("response_type", "code");
            pairs.append_pair("client_id", client_id.trim());
            pairs.append_pair("redirect_uri", redirect_uri.trim());
            pairs.append_pair("code_challenge", &pkce.code_challenge);
            pairs.append_pair("code_challenge_method", &pkce.code_challenge_method);
            pairs.append_pair("state", &pkce.state);
            if let Some(sc) = scope.map(str::trim).filter(|s| !s.is_empty()) {
                pairs.append_pair("scope", sc);
            }
        }

        let mut oauth = self.oauth_lock.lock();
        oauth.version = 1;
        let rec = oauth.records.entry(server_id.to_string()).or_default();
        rec.pending_verifier = Some(pkce.code_verifier.clone());
        rec.pending_state = Some(pkce.state.clone());
        rec.updated_at = Some(now_rfc3339_millis());
        write_json_file_atomically(self.data_dir.join(MCP_OAUTH_JSON_FILE), &*oauth, true)
            .map_err(map_storage_err)?;

        Ok((url.to_string(), pkce))
    }

    pub fn store_oauth_token(
        &self,
        server_id: &str,
        access_token: &str,
        refresh_token: Option<&str>,
        expires_at_ms: Option<u64>,
    ) -> RuntimeResult<()> {
        if access_token.trim().is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "OAuth access_token must not be empty.".to_string(),
            ));
        }
        let mut oauth = self.oauth_lock.lock();
        oauth.version = 1;
        let rec = oauth.records.entry(server_id.to_string()).or_default();
        rec.access_token = Some(access_token.to_string());
        rec.refresh_token = refresh_token.map(str::to_string);
        rec.expires_at_ms = expires_at_ms;
        rec.pending_verifier = None;
        rec.pending_state = None;
        rec.updated_at = Some(now_rfc3339_millis());
        write_json_file_atomically(self.data_dir.join(MCP_OAUTH_JSON_FILE), &*oauth, true)
            .map_err(map_storage_err)?;
        Ok(())
    }

    pub fn oauth_access_token(&self, server_id: &str) -> Option<String> {
        let oauth = self.oauth_lock.lock();
        oauth
            .records
            .get(server_id)
            .and_then(|r| r.access_token.clone())
    }

    pub fn oauth_state(&self, server_id: &str) -> Option<McpOAuthState> {
        let oauth = self.oauth_lock.lock();
        oauth.records.get(server_id).map(|rec| McpOAuthState {
            server_id: server_id.to_string(),
            signed_in: rec.access_token.is_some(),
            pending_authorization: rec.pending_state.is_some() && rec.pending_verifier.is_some(),
            expires_at_ms: rec.expires_at_ms,
            updated_at: rec.updated_at.clone(),
        })
    }

    /// Validates an OAuth 2.0 callback `state` parameter against the persisted PKCE session
    /// and returns the matching `code_verifier` for token exchange.
    pub fn verify_oauth_callback(&self, server_id: &str, state: &str) -> RuntimeResult<String> {
        let oauth = self.oauth_lock.lock();
        let rec = oauth.records.get(server_id).ok_or_else(|| {
            OpenBotRuntimeError::Unauthorized(format!(
                "No pending OAuth session for MCP server '{server_id}'."
            ))
        })?;
        let Some(expected_state) = rec.pending_state.as_deref() else {
            return Err(OpenBotRuntimeError::Unauthorized(format!(
                "OAuth session for MCP server '{server_id}' has no pending state."
            )));
        };
        if expected_state != state {
            return Err(OpenBotRuntimeError::Unauthorized(
                "OAuth callback state mismatch.".to_string(),
            ));
        }
        rec.pending_verifier.clone().ok_or_else(|| {
            OpenBotRuntimeError::Unauthorized(format!(
                "OAuth session for MCP server '{server_id}' has no PKCE verifier."
            ))
        })
    }

    /// Returns the built-in `openbot_computer_use` descriptor gated strictly by `enabled`.
    pub fn computer_use_descriptor_if_enabled(enabled: bool) -> Option<ComputerUseDescriptor> {
        if !enabled {
            return None;
        }
        let server = McpServerConfig {
            id: COMPUTER_USE_MCP_SERVER_ID.to_string(),
            name: COMPUTER_USE_MCP_SERVER_NAME.to_string(),
            transport: McpTransport::Stdio,
            enabled: true,
            command: OPENBOT_COMPUTER_USE_NAME.to_string(),
            args: Vec::new(),
            env: Vec::new(),
            env_passthrough: Vec::new(),
            working_directory: String::new(),
            url: String::new(),
            headers: Vec::new(),
        };
        Some(ComputerUseDescriptor {
            server,
            qualified_name: OPENBOT_COMPUTER_USE_NAME.to_string(),
            guide_uri: COMPUTER_USE_GUIDE_URI.to_string(),
            tools: vec![
                "get_desktop_state".to_string(),
                "get_window_state".to_string(),
                "click".to_string(),
                "move_cursor".to_string(),
                "scroll".to_string(),
                "type_text".to_string(),
                "press_key".to_string(),
                "bring_to_front".to_string(),
                "set_window_frame".to_string(),
                "invoke_menu".to_string(),
                "set_value".to_string(),
                "start_recording".to_string(),
                "stop_recording".to_string(),
            ],
        })
    }

    pub fn computer_use_descriptor(&self, agent: &AgentSummary) -> Option<ComputerUseDescriptor> {
        Self::computer_use_descriptor_if_enabled(agent.computer_use_enabled())
    }

    pub fn list_connectors(&self) -> Vec<McpConnectorDescriptor> {
        vec![
            McpConnectorDescriptor {
                id: GITHUB_CONNECTOR_MCP_SERVER_ID.to_string(),
                name: GITHUB_CONNECTOR_MCP_SERVER_NAME.to_string(),
                url: GITHUB_CONNECTOR_MCP_SERVER_URL.to_string(),
                transport: McpTransport::Http,
                requires_oauth: true,
            },
            McpConnectorDescriptor {
                id: "openbot-connector-canva".to_string(),
                name: "canva".to_string(),
                url: "https://mcp.canva.com/mcp".to_string(),
                transport: McpTransport::Http,
                requires_oauth: true,
            },
            McpConnectorDescriptor {
                id: "openbot-connector-linear".to_string(),
                name: "linear".to_string(),
                url: "https://mcp.linear.app/mcp".to_string(),
                transport: McpTransport::Http,
                requires_oauth: true,
            },
            McpConnectorDescriptor {
                id: "openbot-connector-notion".to_string(),
                name: "notion".to_string(),
                url: "https://mcp.notion.com/mcp".to_string(),
                transport: McpTransport::Http,
                requires_oauth: true,
            },
            McpConnectorDescriptor {
                id: "openbot-connector-figma".to_string(),
                name: "figma".to_string(),
                url: "https://mcp.figma.com/mcp".to_string(),
                transport: McpTransport::Http,
                requires_oauth: true,
            },
            McpConnectorDescriptor {
                id: "openbot-connector-sentry".to_string(),
                name: "sentry".to_string(),
                url: "https://mcp.sentry.dev/mcp".to_string(),
                transport: McpTransport::Http,
                requires_oauth: true,
            },
            McpConnectorDescriptor {
                id: "openbot-connector-stripe".to_string(),
                name: "stripe".to_string(),
                url: "https://mcp.stripe.com".to_string(),
                transport: McpTransport::Http,
                requires_oauth: true,
            },
        ]
    }

    pub fn servers_for_agent(&self, agent: &AgentSummary) -> RuntimeResult<Vec<McpServerConfig>> {
        let mut out: Vec<McpServerConfig> = self
            .list_servers()?
            .into_iter()
            .filter(|srv| srv.enabled && self.allows_agent(&srv.id, &agent.id))
            .collect();
        if let Some(cua) = self.computer_use_descriptor(agent) {
            out.push(cua.server);
        }
        Ok(out)
    }

    pub fn redact_secrets(&self, text: &str) -> String {
        let mut masked = redact_secrets(text);
        if let Ok(servers) = self.store.list() {
            for srv in servers {
                for pair in srv.headers.iter().chain(srv.env.iter()) {
                    if pair.value.len() >= 4
                        && !matches!(
                            pair.value.to_ascii_lowercase().as_str(),
                            "true" | "false" | "null" | "none"
                        )
                    {
                        masked = masked.replace(&pair.value, "•••");
                    }
                }
            }
        }
        let oauth = self.oauth_lock.lock();
        for rec in oauth.records.values() {
            for secret in [
                rec.access_token.as_deref(),
                rec.refresh_token.as_deref(),
                rec.pending_verifier.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                if secret.len() >= 4 {
                    masked = masked.replace(secret, "•••");
                }
            }
        }
        masked
    }

    pub fn execute_mcp_server_tool(
        &self,
        agent_id: &str,
        tool_name: &str,
        args: &JsonValue,
    ) -> RuntimeResult<JsonValue> {
        let short = tool_name.strip_prefix("openbot.").unwrap_or(tool_name);
        match short {
            "list_mcp_servers" => {
                let servers: Vec<JsonValue> = self
                    .list_servers()?
                    .into_iter()
                    .map(|srv| {
                        let signed_in = self.oauth_access_token(&srv.id).is_some();
                        json!({
                            "id": srv.id,
                            "name": srv.name,
                            "transport": srv.transport.as_str(),
                            "enabled": srv.enabled,
                            "forYou": self.allows_agent(&srv.id, agent_id),
                            "command": if srv.transport == McpTransport::Stdio { JsonValue::String(srv.command) } else { JsonValue::Null },
                            "argumentCount": srv.args.len(),
                            "url": if srv.transport == McpTransport::Http { JsonValue::String(strip_url_query(&srv.url)) } else { JsonValue::Null },
                            "headerNames": srv.headers.iter().map(|h| h.key.clone()).collect::<Vec<_>>(),
                            "environmentNames": srv.env.iter().map(|e| e.key.clone()).chain(srv.env_passthrough.into_iter()).collect::<Vec<_>>(),
                            "signedIn": signed_in,
                        })
                    })
                    .collect();
                Ok(json!({ "servers": servers }))
            }
            "save_mcp_server" => {
                let name = args
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation("save_mcp_server requires name.".to_string())
                    })?;
                let existing = self.get_server(name)?;
                let transport = match args.get("transport").and_then(JsonValue::as_str) {
                    Some(raw) => McpTransport::parse(raw).ok_or_else(|| {
                        OpenBotRuntimeError::Validation(format!("Invalid MCP transport '{raw}'."))
                    })?,
                    None => {
                        if args.get("command").and_then(JsonValue::as_str).is_some() {
                            McpTransport::Stdio
                        } else if args.get("url").and_then(JsonValue::as_str).is_some() {
                            McpTransport::Http
                        } else if let Some(ex) = &existing {
                            ex.transport
                        } else {
                            McpTransport::Http
                        }
                    }
                };
                let args_vec = args
                    .get("args")
                    .and_then(JsonValue::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(JsonValue::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_else(|| existing.as_ref().map(|e| e.args.clone()).unwrap_or_default());
                let headers = args
                    .get("headers")
                    .and_then(JsonValue::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|item| {
                                let k = item
                                    .get("name")
                                    .or_else(|| item.get("key"))
                                    .and_then(JsonValue::as_str)?;
                                let v = item.get("value").and_then(JsonValue::as_str)?;
                                (!v.is_empty()).then(|| McpKeyValue {
                                    key: k.to_string(),
                                    value: v.to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        existing
                            .as_ref()
                            .map(|e| e.headers.clone())
                            .unwrap_or_default()
                    });
                let saved = self.register_server(RegisterMcpServerInput {
                    id: existing.as_ref().map(|e| e.id.clone()),
                    name: name.to_string(),
                    transport,
                    enabled: args
                        .get("enabled")
                        .and_then(JsonValue::as_bool)
                        .or_else(|| existing.as_ref().map(|e| e.enabled)),
                    command: args
                        .get("command")
                        .and_then(JsonValue::as_str)
                        .map(str::to_string)
                        .or_else(|| existing.as_ref().map(|e| e.command.clone())),
                    args: args_vec,
                    env: existing.as_ref().map(|e| e.env.clone()).unwrap_or_default(),
                    env_passthrough: existing
                        .as_ref()
                        .map(|e| e.env_passthrough.clone())
                        .unwrap_or_default(),
                    working_directory: args
                        .get("workingDirectory")
                        .and_then(JsonValue::as_str)
                        .map(str::to_string)
                        .or_else(|| existing.as_ref().map(|e| e.working_directory.clone())),
                    url: args
                        .get("url")
                        .and_then(JsonValue::as_str)
                        .map(str::to_string)
                        .or_else(|| existing.as_ref().map(|e| e.url.clone())),
                    headers,
                })?;
                Ok(json!({
                    "status": if saved.enabled { "saved" } else { "disabled" },
                    "mcpServer": saved,
                    "appliesFrom": "the next turn of each agent",
                }))
            }
            "set_mcp_server_enabled" => {
                let name = args
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation(
                            "set_mcp_server_enabled requires name.".to_string(),
                        )
                    })?;
                let enabled = args
                    .get("enabled")
                    .and_then(JsonValue::as_bool)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation(
                            "set_mcp_server_enabled requires enabled.".to_string(),
                        )
                    })?;
                let srv = self.get_server(name)?.ok_or_else(|| {
                    OpenBotRuntimeError::NotFound(format!("Unknown MCP server '{name}'."))
                })?;
                let updated = self.set_server_enabled(&srv.id, enabled)?;
                Ok(json!({
                    "name": updated.name,
                    "enabled": updated.enabled,
                    "appliesFrom": "the next turn of each agent",
                }))
            }
            "remove_mcp_server" => {
                let name = args
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation("remove_mcp_server requires name.".to_string())
                    })?;
                let srv = self.get_server(name)?.ok_or_else(|| {
                    OpenBotRuntimeError::NotFound(format!("Unknown MCP server '{name}'."))
                })?;
                let removed = self.remove_server(&srv.id)?;
                Ok(json!({
                    "name": srv.name,
                    "removed": removed,
                    "appliesFrom": "the next turn of each agent",
                }))
            }
            "test_mcp_server" => {
                let name = args
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation("test_mcp_server requires name.".to_string())
                    })?;
                let srv = self.get_server(name)?.ok_or_else(|| {
                    OpenBotRuntimeError::NotFound(format!("Unknown MCP server '{name}'."))
                })?;
                Ok(json!({
                    "server": {
                        "id": srv.id,
                        "name": srv.name,
                        "transport": srv.transport.as_str(),
                        "enabled": srv.enabled,
                    },
                    "status": if srv.enabled { "ready" } else { "disabled" },
                }))
            }
            _ => Err(OpenBotRuntimeError::Validation(format!(
                "Unknown MCP management tool '{tool_name}'."
            ))),
        }
    }

    /// Executes an MCP tool on a registered `stdio` or `http` MCP server (or built-in
    /// `openbot_computer_use` when permitted by the agent's `computer_use` flag) using
    /// JSON-RPC 2.0 framing (`initialize` + `tools/call`).
    pub async fn execute_mcp_tool(
        &self,
        server_id_or_name: &str,
        tool_name: &str,
        arguments: JsonValue,
        agent: Option<&AgentSummary>,
    ) -> RuntimeResult<McpToolCallOutcome> {
        let trimmed_tool = tool_name.trim();
        if trimmed_tool.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "MCP tool name must not be empty.".to_string(),
            ));
        }
        let args_object = match arguments {
            JsonValue::Object(map) => JsonValue::Object(map),
            JsonValue::Null => JsonValue::Object(JsonMap::new()),
            _ => {
                return Err(OpenBotRuntimeError::Validation(
                    "MCP tool arguments must be a JSON object.".to_string(),
                ));
            }
        };

        let is_cua = server_id_or_name.eq_ignore_ascii_case(COMPUTER_USE_MCP_SERVER_ID)
            || server_id_or_name.eq_ignore_ascii_case(COMPUTER_USE_MCP_SERVER_NAME)
            || server_id_or_name.eq_ignore_ascii_case(OPENBOT_COMPUTER_USE_NAME);

        if is_cua {
            let allowed = agent.is_none_or(AgentSummary::computer_use_enabled);
            let Some(desc) = Self::computer_use_descriptor_if_enabled(allowed) else {
                return Err(OpenBotRuntimeError::Forbidden(
                    "Computer Use is disabled for this agent.".to_string(),
                ));
            };
            if !desc.tools.iter().any(|t| t == trimmed_tool) {
                return Err(OpenBotRuntimeError::NotFound(format!(
                    "Unknown computer_use tool '{trimmed_tool}'."
                )));
            }
            return Err(OpenBotRuntimeError::Execution(format!(
                "Computer Use driver ({OPENBOT_COMPUTER_USE_NAME}.{trimmed_tool}) requires an active desktop CUA lease."
            )));
        }

        let server = self
            .get_server(server_id_or_name)?
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!(
                    "MCP server '{server_id_or_name}' not found."
                ))
            })?;
        if !server.enabled {
            return Err(OpenBotRuntimeError::Forbidden(format!(
                "MCP server '{}' is disabled.",
                server.name
            )));
        }
        if let Some(ag) = agent {
            if !self.allows_agent(&server.id, &ag.id) {
                return Err(OpenBotRuntimeError::Forbidden(format!(
                    "Agent '{}' does not have access to MCP server '{}'.",
                    ag.id, server.name
                )));
            }
        }

        let params = json!({
            "name": trimmed_tool,
            "arguments": args_object,
        });
        let result_val = match server.transport {
            McpTransport::Stdio => self.call_stdio_jsonrpc(&server, "tools/call", params).await?,
            McpTransport::Http => self.call_http_jsonrpc(&server, "tools/call", params).await?,
        };

        let is_error = result_val
            .get("isError")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let structured_content = result_val.get("structuredContent").cloned();
        let content = result_val
            .get("content")
            .cloned()
            .unwrap_or_else(|| result_val.clone());

        Ok(McpToolCallOutcome {
            server_id: server.id,
            server_name: server.name,
            tool_name: trimmed_tool.to_string(),
            is_error,
            content,
            structured_content,
        })
    }

    /// Probes an MCP server via JSON-RPC 2.0 `tools/list` and classifies auth/key failures.
    pub async fn probe_server(&self, server_id_or_name: &str) -> RuntimeResult<McpProbeOutcome> {
        let server = self
            .get_server(server_id_or_name)?
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!(
                    "MCP server '{server_id_or_name}' not found."
                ))
            })?;
        let res = match server.transport {
            McpTransport::Stdio => self.call_stdio_jsonrpc(&server, "tools/list", json!({})).await,
            McpTransport::Http => self.call_http_jsonrpc(&server, "tools/list", json!({})).await,
        };
        match res {
            Ok(val) => {
                let tools: Vec<String> = val
                    .get("tools")
                    .and_then(JsonValue::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|t| t.get("name").and_then(JsonValue::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(McpProbeOutcome {
                    server_id: server.id,
                    name: server.name,
                    status: "ready".to_string(),
                    tool_count: tools.len(),
                    tools,
                    error: None,
                })
            }
            Err(OpenBotRuntimeError::Unauthorized(msg)) => Ok(McpProbeOutcome {
                server_id: server.id,
                name: server.name,
                status: if server.headers.is_empty() {
                    "needs_sign_in".to_string()
                } else {
                    "needs_key".to_string()
                },
                tool_count: 0,
                tools: Vec::new(),
                error: Some(self.redact_secrets(&msg)),
            }),
            Err(err) => Ok(McpProbeOutcome {
                server_id: server.id,
                name: server.name,
                status: "error".to_string(),
                tool_count: 0,
                tools: Vec::new(),
                error: Some(self.redact_secrets(&err.to_string())),
            }),
        }
    }

    async fn call_stdio_jsonrpc(
        &self,
        server: &McpServerConfig,
        method: &str,
        params: JsonValue,
    ) -> RuntimeResult<JsonValue> {
        let cfg = server.clone();
        let method_owned = method.to_string();
        let raw_out = tokio::task::spawn_blocking(move || -> RuntimeResult<String> {
            let mut cmd = Command::new(&cfg.command);
            cmd.args(&cfg.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if !cfg.working_directory.trim().is_empty() {
                cmd.current_dir(&cfg.working_directory);
            }
            for pair in &cfg.env {
                cmd.env(&pair.key, &pair.value);
            }
            for key in &cfg.env_passthrough {
                if let Ok(val) = std::env::var(key) {
                    cmd.env(key, val);
                }
            }
            let mut child = cmd.spawn().map_err(|e| {
                OpenBotRuntimeError::Execution(format!(
                    "Failed to launch stdio MCP server '{}': {e}",
                    cfg.name
                ))
            })?;

            let init_req = frame_jsonrpc_request(
                1,
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "openbot", "version": "1.0.0" }
                }),
            );
            let call_req = frame_jsonrpc_request(2, &method_owned, params);
            if let Some(mut stdin) = child.stdin.take() {
                let payload = format!(
                    "{}\n{}\n",
                    serde_json::to_string(&init_req)?,
                    serde_json::to_string(&call_req)?
                );
                let _ = stdin.write_all(payload.as_bytes());
                let _ = stdin.flush();
            }

            let output = child.wait_with_output().map_err(|e| {
                OpenBotRuntimeError::Execution(format!(
                    "Stdio MCP server '{}' I/O failure: {e}",
                    cfg.name
                ))
            })?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if stdout.trim().is_empty() && !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(OpenBotRuntimeError::Execution(format!(
                    "Stdio MCP server '{}' exited with status {}: {}",
                    cfg.name,
                    output.status,
                    stderr.trim()
                )));
            }
            Ok(stdout)
        })
        .await
        .map_err(|e| OpenBotRuntimeError::Execution(format!("MCP stdio task panicked: {e}")))??;

        parse_jsonrpc_response(&raw_out)
            .map_err(|e| OpenBotRuntimeError::Execution(self.redact_secrets(&e.to_string())))
    }

    async fn call_http_jsonrpc(
        &self,
        server: &McpServerConfig,
        method: &str,
        params: JsonValue,
    ) -> RuntimeResult<JsonValue> {
        let req_body = frame_jsonrpc_request(1, method, params);
        let mut req = self
            .http_client
            .post(&server.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        let mut has_auth_header = false;
        for pair in &server.headers {
            if pair.key.eq_ignore_ascii_case("authorization") {
                has_auth_header = true;
            }
            req = req.header(&pair.key, &pair.value);
        }
        if !has_auth_header {
            if let Some(token) = self.oauth_access_token(&server.id) {
                req = req.bearer_auth(token);
            }
        }

        let resp = req
            .json(&req_body)
            .send()
            .await
            .map_err(|e| OpenBotRuntimeError::Http(self.redact_secrets(&e.to_string())))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| OpenBotRuntimeError::Http(self.redact_secrets(&e.to_string())))?;
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(OpenBotRuntimeError::Unauthorized(self.redact_secrets(&format!(
                "MCP HTTP server '{}' rejected authorization ({}): {}",
                server.name,
                status.as_u16(),
                text.trim()
            ))));
        }
        if !status.is_success() {
            return Err(OpenBotRuntimeError::Http(self.redact_secrets(&format!(
                "MCP HTTP server '{}' returned HTTP {}: {}",
                server.name,
                status.as_u16(),
                text.trim()
            ))));
        }
        parse_jsonrpc_response(&text)
            .map_err(|e| OpenBotRuntimeError::Execution(self.redact_secrets(&e.to_string())))
    }
}

fn strip_url_query(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_) => raw.to_string(),
    }
}

fn read_json_or_default<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> RuntimeResult<T> {
    match fs::read_to_string(path) {
        Ok(raw) if !raw.trim().is_empty() => Ok(serde_json::from_str(&raw).unwrap_or_default()),
        Ok(_) => Ok(T::default()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(err) => Err(OpenBotRuntimeError::Io(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_db_and_dir() -> (TempDir, OpenBotDatabase) {
        let dir = TempDir::new().unwrap();
        let db = OpenBotDatabase::open_in_data_dir(dir.path()).unwrap();
        (dir, db)
    }

    #[test]
    fn skill_service_frontmatter_manifest_and_32_limit() {
        let (dir, db) = test_db_and_dir();
        let svc = SkillService::open(db, dir.path()).unwrap();
        let agent_id = "agent-11111111-1111-4111-8111-111111111111";

        let out = svc
            .save_skill(SaveSkillInput {
                agent_id: agent_id.to_string(),
                slug: "rust-audit".to_string(),
                name: "Rust Audit".to_string(),
                description: "Check unsafe blocks".to_string(),
                instructions: "# Audit\nRun cargo check.".to_string(),
                triggers: vec!["audit".to_string(), "unsafe".to_string()],
                tools: vec!["openbot.list_tables".to_string()],
                origin: None,
                enabled: Some(true),
            })
            .unwrap();
        assert_eq!(out.state, "installed");
        assert_eq!(out.assigned_skill_count, 1);
        assert_eq!(out.skill.skill_id, "local:rust-audit");

        let disk_md = fs::read_to_string(&out.path).unwrap();
        let fm = svc.read_skill_frontmatter(&disk_md, Some("rust-audit"));
        assert!(fm.problem.is_none());
        assert_eq!(fm.name, "rust-audit");
        assert_eq!(fm.description, "Check unsafe blocks");
        assert_eq!(fm.triggers, vec!["audit", "unsafe"]);
        assert_eq!(fm.tools, vec!["openbot.list_tables"]);
        assert!(fm.body.contains("Run cargo check."));

        // Verify shared manifest JSON matches SCOPE.md § 3.3
        let raw_manifest: JsonValue =
            serde_json::from_str(&fs::read_to_string(svc.manifest_path()).unwrap()).unwrap();
        assert_eq!(raw_manifest["version"], 1);
        assert_eq!(raw_manifest["byAgent"][agent_id][0]["skillId"], "local:rust-audit");
        assert_eq!(raw_manifest["byAgent"][agent_id][0]["origin"], "local");

        // Builtin and home origins must NOT count toward assigned_skill_count
        svc.save_skill(SaveSkillInput {
            agent_id: agent_id.to_string(),
            slug: "openbot-data".to_string(),
            name: "OpenBot Data".to_string(),
            description: "Builtin data skill".to_string(),
            instructions: "Use SQLite.".to_string(),
            triggers: Vec::new(),
            tools: Vec::new(),
            origin: Some("builtin".to_string()),
            enabled: Some(true),
        })
        .unwrap();
        svc.save_skill(SaveSkillInput {
            agent_id: agent_id.to_string(),
            slug: "home-helper".to_string(),
            name: "Home Helper".to_string(),
            description: "Home skill".to_string(),
            instructions: "Home rules.".to_string(),
            triggers: Vec::new(),
            tools: Vec::new(),
            origin: Some("home".to_string()),
            enabled: Some(true),
        })
        .unwrap();
        assert_eq!(svc.assigned_skill_count(agent_id).unwrap(), 1);

        // Fill up to 32 assigned skills, then verify 33rd is rejected
        for i in 2..=32 {
            svc.save_skill(SaveSkillInput {
                agent_id: agent_id.to_string(),
                slug: format!("sk-{i:02}"),
                name: format!("Skill {i}"),
                description: "ok".to_string(),
                instructions: "# ok".to_string(),
                triggers: Vec::new(),
                tools: Vec::new(),
                origin: None,
                enabled: Some(true),
            })
            .unwrap();
        }
        assert_eq!(svc.assigned_skill_count(agent_id).unwrap(), 32);

        let over = svc
            .save_skill(SaveSkillInput {
                agent_id: agent_id.to_string(),
                slug: "sk-33".to_string(),
                name: "Skill 33".to_string(),
                description: "over".to_string(),
                instructions: "# over".to_string(),
                triggers: Vec::new(),
                tools: Vec::new(),
                origin: None,
                enabled: Some(true),
            })
            .unwrap_err();
        assert!(over.to_string().contains("32"));

        let toggled = svc
            .set_skill_enabled(agent_id, "local:rust-audit", false)
            .unwrap();
        assert!(!toggled.enabled);
        assert!(svc.uninstall_skill(agent_id, "local:rust-audit").unwrap());
        assert_eq!(svc.assigned_skill_count(agent_id).unwrap(), 31);
    }

    #[test]
    fn mcp_gateway_caps_reserved_names_pkce_and_computer_use_gate() {
        let (dir, db) = test_db_and_dir();
        let gw = McpGateway::open(db, dir.path()).unwrap();

        for reserved in ["openbot", "openbot_browser", "computer_use"] {
            let err = gw
                .register_server(RegisterMcpServerInput {
                    id: None,
                    name: reserved.to_string(),
                    transport: McpTransport::Stdio,
                    enabled: Some(true),
                    command: Some("echo".to_string()),
                    args: Vec::new(),
                    env: Vec::new(),
                    env_passthrough: Vec::new(),
                    working_directory: None,
                    url: None,
                    headers: Vec::new(),
                })
                .unwrap_err();
            let lower = err.to_string().to_lowercase();
            assert!(lower.contains("reserved") || lower.contains("already uses"));
        }

        let stdio = gw
            .register_server(RegisterMcpServerInput {
                id: None,
                name: "local-fs".to_string(),
                transport: McpTransport::Stdio,
                enabled: Some(true),
                command: Some("echo".to_string()),
                args: vec!["mcp".to_string()],
                env: Vec::new(),
                env_passthrough: Vec::new(),
                working_directory: None,
                url: None,
                headers: Vec::new(),
            })
            .unwrap();
        assert!(stdio.id.starts_with("mcp-"));

        let http = gw
            .register_server(RegisterMcpServerInput {
                id: None,
                name: "remote-sse".to_string(),
                transport: McpTransport::Http,
                enabled: Some(true),
                command: None,
                args: Vec::new(),
                env: Vec::new(),
                env_passthrough: Vec::new(),
                working_directory: None,
                url: Some("https://mcp.example.test/sse".to_string()),
                headers: vec![McpKeyValue {
                    key: "Authorization".to_string(),
                    value: "Bearer super-secret-token-xyz".to_string(),
                }],
            })
            .unwrap();
        assert!(http.id.starts_with("mcp-"));
        assert!(!gw
            .redact_secrets("error with Bearer super-secret-token-xyz")
            .contains("super-secret-token-xyz"));

        // PKCE S256 generation + verification
        let (auth_url, pkce) = gw
            .build_oauth_authorize_url(
                &http.id,
                "https://auth.example.test/authorize",
                "openbot-client",
                "http://127.0.0.1:4312/callback",
                Some("mcp:tools"),
            )
            .unwrap();
        assert_eq!(pkce.code_challenge_method, "S256");
        assert!(verify_pkce_s256(&pkce.code_verifier, &pkce.code_challenge));
        assert!(auth_url.contains("code_challenge_method=S256"));

        // Computer use permission flag gating
        let ws = dir.path().join("ws");
        let mut agent = AgentSummary::new_default("agent-1", "Ada", ws.to_string_lossy());
        agent.computer_use = Some(true);
        assert!(gw.computer_use_descriptor(&agent).is_some());
        agent.computer_use = Some(false);
        assert!(gw.computer_use_descriptor(&agent).is_none());

        // JSON-RPC 2.0 SSE parsing
        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        let parsed = parse_jsonrpc_response(sse).unwrap();
        assert_eq!(parsed["ok"], true);
    }
}
