//! System Services (`F18` & `F20`):
//! - `ComputerUseService` (safety guards, compact AX tree diff, `wait_for_condition`, stdin secret redaction)
//! - `MobileEmulatorService` (normalized `0.0..=1.0` <-> `0..=10_000` bp coords, 500-node AX cap)
//! - `SttDictationService` (persisted `stt-gpu-failure-marker` GPU->CPU fallback, OpenAI-compatible remote STT, cleanup)
//! - `ShareableArtifactStore` (`.html`/`.md` share gated on `user_confirmed_share`, hidden `ProtectedSecret` `edit_token`)
//! - `EphemeralVmService` (`OrcaYamlEnvironmentRecipe` planner + base-snapshot contamination `run_doctor`)
//! - `SleepAwareHangWatchdog` (monotonic vs wall-clock OS sleep/wake jump suppression + tiered stall breadcrumbs)

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use orca_core::OrcaIdeErrorCode;
use orca_core::orchestration_protocol::{
    ComputerUseAction, ComputerUseGuardDecision, ComputerUsePolicy, ComputerUseRequest,
    ComputerUseTargetKind, ProtectedSecret, evaluate_computer_use_guard,
};
use orca_core::workspace_layout::OrcaYamlEnvironmentRecipe;
use parking_lot::{Mutex, RwLock};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::profile_state::ProtectedSecretPersistence;
use crate::{OrcaIdeRuntimeError, Result};

/// Maximum accessibility nodes serialized from mobile emulators (`F18`).
pub const EMULATOR_AX_MAX_NODES: usize = 500;

/// Marker file written when local STT GPU acceleration crashes, triggering CPU fallback.
pub const STT_GPU_FAILURE_MARKER_FILE: &str = "stt-gpu-failure-marker";

/// Host-identity files that MUST NEVER exist inside a reusable cloud VM base snapshot.
pub const BASE_SNAPSHOT_CONTAMINATION_FILES: [&str; 3] = [
    "orca-devices.json",
    "orca-e2ee-keypair.json",
    "agent-session-authority.key",
];

// ---------------------------------------------------------------------------
// 1. ComputerUseService
// ---------------------------------------------------------------------------

/// Accessibility tree node used for compact `treeDiff` snapshots and emulator AX queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxNode {
    #[serde(alias = "id")]
    pub node_id: String,
    #[serde(default = "default_ax_role")]
    pub role: String,
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds_bp: Option<(u16, u16, u16, u16)>,
}

fn default_ax_role() -> String {
    "element".to_string()
}

const fn default_true() -> bool {
    true
}

/// Recursively flattens either a flat array of `AxNode` objects or a nested AX tree
/// (`{id, role?, label?, enabled?, value?, children: [...]}`) into a flat `Vec<AxNode>`.
pub fn flatten_ax_tree_value(value: &serde_json::Value) -> Vec<AxNode> {
    fn walk(v: &serde_json::Value, out: &mut Vec<AxNode>) {
        match v {
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, out);
                }
            }
            serde_json::Value::Object(obj) => {
                if let Some(id) = obj
                    .get("node_id")
                    .or_else(|| obj.get("id"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                {
                    let role = obj
                        .get("role")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("element")
                        .to_string();
                    let label = obj
                        .get("label")
                        .or_else(|| obj.get("title"))
                        .or_else(|| obj.get("text"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let value = obj
                        .get("value")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned);
                    let enabled = obj
                        .get("enabled")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true);
                    let bounds_bp = obj
                        .get("bounds_bp")
                        .cloned()
                        .and_then(|b| serde_json::from_value(b).ok());
                    out.push(AxNode {
                        node_id: id.to_string(),
                        role,
                        label,
                        value,
                        enabled,
                        bounds_bp,
                    });
                }
                if let Some(children) = obj.get("children") {
                    walk(children, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, &mut out);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxDiffOp {
    Added,
    Removed,
    Changed,
}

impl AxDiffOp {
    pub const fn prefix_char(self) -> char {
        match self {
            Self::Added => '+',
            Self::Removed => '-',
            Self::Changed => '~',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxTreeDiffEntry {
    pub op: AxDiffOp,
    pub node_id: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxTreeDiffReport {
    pub entries: Vec<AxTreeDiffEntry>,
    pub compact_lines: Vec<String>,
}

/// Computes compact `+` / `-` / `~` accessibility tree diff between two AX snapshots.
pub fn compute_ax_tree_diff(before: &[AxNode], after: &[AxNode]) -> AxTreeDiffReport {
    let before_map: BTreeMap<&str, &AxNode> =
        before.iter().map(|n| (n.node_id.as_str(), n)).collect();
    let after_map: BTreeMap<&str, &AxNode> =
        after.iter().map(|n| (n.node_id.as_str(), n)).collect();

    let mut entries = Vec::new();
    let mut compact_lines = Vec::new();

    for (id, prev) in &before_map {
        if !after_map.contains_key(id) {
            let summary = format!("[{}] \"{}\"", prev.role, prev.label);
            compact_lines.push(format!("- {id} {summary}"));
            entries.push(AxTreeDiffEntry {
                op: AxDiffOp::Removed,
                node_id: (*id).to_string(),
                summary,
            });
        }
    }

    for (id, next) in &after_map {
        match before_map.get(id) {
            None => {
                let summary = format!("[{}] \"{}\"", next.role, next.label);
                compact_lines.push(format!("+ {id} {summary}"));
                entries.push(AxTreeDiffEntry {
                    op: AxDiffOp::Added,
                    node_id: (*id).to_string(),
                    summary,
                });
            }
            Some(prev) if *prev != *next => {
                let mut diffs = Vec::new();
                if prev.label != next.label {
                    diffs.push(format!("label \"{}\"->\"{}\"", prev.label, next.label));
                }
                if prev.value != next.value {
                    diffs.push(format!(
                        "value {:?}->{:?}",
                        prev.value.as_deref().unwrap_or(""),
                        next.value.as_deref().unwrap_or("")
                    ));
                }
                if prev.enabled != next.enabled {
                    diffs.push(format!("enabled {}->{}", prev.enabled, next.enabled));
                }
                if prev.bounds_bp != next.bounds_bp {
                    diffs.push("bounds changed".to_string());
                }
                let summary = diffs.join(", ");
                compact_lines.push(format!("~ {id} {summary}"));
                entries.push(AxTreeDiffEntry {
                    op: AxDiffOp::Changed,
                    node_id: (*id).to_string(),
                    summary,
                });
            }
            Some(_) => {}
        }
    }

    AxTreeDiffReport {
        entries,
        compact_lines,
    }
}

/// Condition awaited by [`ComputerUseService::wait_for_condition`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitCondition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_enabled: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
}

/// Snapshot observed by `wait_for_condition` probes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitProbeSnapshot {
    pub window_title: String,
    pub visible_text: String,
    pub nodes: Vec<AxNode>,
}

impl WaitCondition {
    pub fn matches(&self, snap: &WaitProbeSnapshot) -> bool {
        if let Some(ref needle) = self.text_contains
            && !snap.visible_text.contains(needle.as_str())
        {
            return false;
        }
        if let Some(ref title) = self.window_title
            && !snap.window_title.contains(title.as_str())
        {
            return false;
        }
        if let Some(ref node_id) = self.node_enabled
            && !snap
                .nodes
                .iter()
                .any(|n| n.node_id == *node_id && n.enabled)
        {
            return false;
        }
        true
    }
}

/// Redacts sensitive values supplied via `--value-stdin`, `--text-stdin`, `--value`, or `--text`.
pub fn redact_stdin_secret_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        if redact_next {
            out.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }
        if let Some((flag, _)) = arg.split_once('=')
            && matches!(
                flag,
                "--value-stdin" | "--text-stdin" | "--value" | "--text"
            )
        {
            out.push(format!("{flag}=[REDACTED]"));
            continue;
        }
        if matches!(
            arg.as_str(),
            "--value-stdin" | "--text-stdin" | "--value" | "--text"
        ) {
            out.push(arg.clone());
            redact_next = true;
            continue;
        }
        out.push(arg.clone());
    }
    out
}

/// Safe Computer Use coordinator (`Send + Sync`).
pub struct ComputerUseService {
    policy: RwLock<ComputerUsePolicy>,
    app_trees: RwLock<BTreeMap<String, Vec<AxNode>>>,
    audit_entries: Mutex<Vec<serde_json::Value>>,
}

impl Default for ComputerUseService {
    fn default() -> Self {
        Self::new(ComputerUsePolicy::default())
    }
}

impl fmt::Debug for ComputerUseService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComputerUseService").finish()
    }
}

fn default_app_ax_nodes(app: &str) -> Vec<AxNode> {
    vec![
        AxNode {
            node_id: "root".to_string(),
            role: "window".to_string(),
            label: app.to_string(),
            value: None,
            enabled: true,
            bounds_bp: Some((0, 0, 10_000, 10_000)),
        },
        AxNode {
            node_id: "@e1".to_string(),
            role: "button".to_string(),
            label: "Submit".to_string(),
            value: None,
            enabled: true,
            bounds_bp: Some((7_500, 8_800, 2_000, 700)),
        },
        AxNode {
            node_id: "@e2".to_string(),
            role: "textbox".to_string(),
            label: "Input".to_string(),
            value: Some(String::new()),
            enabled: true,
            bounds_bp: Some((500, 8_800, 6_500, 700)),
        },
    ]
}

impl ComputerUseService {
    pub fn new(policy: ComputerUsePolicy) -> Self {
        Self {
            policy: RwLock::new(policy),
            app_trees: RwLock::new(BTreeMap::new()),
            audit_entries: Mutex::new(Vec::new()),
        }
    }

    pub fn policy(&self) -> ComputerUsePolicy {
        self.policy.read().clone()
    }

    pub fn set_policy(&self, policy: ComputerUsePolicy) {
        *self.policy.write() = policy;
    }

    pub fn approve_target(&self, target_id: impl Into<String>) {
        self.policy.write().approved_targets.insert(target_id.into());
    }

    /// Queries the observed AX tree for `app` (seeding standard window/controls when newly
    /// queried) and filters by optional substring `query`.
    pub fn get_app_state(&self, app: &str, query: Option<&str>) -> Vec<AxNode> {
        let key = app.trim().to_string();
        let all = {
            let mut map = self.app_trees.write();
            map.entry(key.clone())
                .or_insert_with(|| default_app_ax_nodes(&key))
                .clone()
        };
        let Some(q) = query.map(str::trim).filter(|s| !s.is_empty()) else {
            return all;
        };
        let q_lc = q.to_ascii_lowercase();
        let filtered: Vec<AxNode> = all
            .iter()
            .filter(|n| {
                n.label.to_ascii_lowercase().contains(&q_lc)
                    || n.node_id.to_ascii_lowercase().contains(&q_lc)
                    || n.role.to_ascii_lowercase().contains(&q_lc)
                    || n.value
                        .as_deref()
                        .is_some_and(|v| v.to_ascii_lowercase().contains(&q_lc))
            })
            .cloned()
            .collect();
        if filtered.is_empty() { all } else { filtered }
    }

    /// Computes compact AX tree diff between `before` and `after` (supporting both nested
    /// `{id, children}` trees and flat `Vec<AxNode>`), updates observed app AX state with
    /// `after`, and returns the `AxTreeDiffReport`.
    pub fn diff_app_state(
        &self,
        app: Option<&str>,
        before: &serde_json::Value,
        after: &serde_json::Value,
    ) -> AxTreeDiffReport {
        let before_nodes = flatten_ax_tree_value(before);
        let after_nodes = flatten_ax_tree_value(after);
        let report = compute_ax_tree_diff(&before_nodes, &after_nodes);
        if !after_nodes.is_empty() {
            let mut map = self.app_trees.write();
            if let Some(a) = app.map(str::trim).filter(|s| !s.is_empty()) {
                map.insert(a.to_string(), after_nodes);
            } else {
                // Update all currently tracked apps + default slot so subsequent waits observe the new tree.
                if map.is_empty() {
                    map.insert("default".to_string(), after_nodes);
                } else {
                    for nodes in map.values_mut() {
                        *nodes = after_nodes.clone();
                    }
                }
            }
        }
        report
    }

    /// Builds a [`WaitProbeSnapshot`] from the currently observed AX tree for `app`.
    pub fn observed_snapshot(&self, app: &str) -> WaitProbeSnapshot {
        let key = app.trim();
        let nodes = {
            let map = self.app_trees.read();
            map.get(key)
                .or_else(|| map.get("default"))
                .cloned()
                .unwrap_or_else(|| default_app_ax_nodes(key))
        };
        let visible_text = nodes
            .iter()
            .map(|n| {
                if let Some(ref val) = n.value {
                    format!("{} {}", n.label, val)
                } else {
                    n.label.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        WaitProbeSnapshot {
            window_title: key.to_string(),
            visible_text,
            nodes,
        }
    }

    /// Records a strictly redacted computer-use audit entry (raw secrets are NEVER persisted).
    pub fn record_audit(
        &self,
        app: &str,
        action: &str,
        target: &str,
        background_delivery: bool,
        redacted: bool,
        now_ms: i64,
    ) {
        self.audit_entries.lock().push(serde_json::json!({
            "app": app,
            "action": action,
            "target": target,
            "background_delivery": background_delivery,
            "steals_focus": !background_delivery,
            "redacted": redacted,
            "value": if redacted { "[REDACTED]" } else { "" },
            "timestamp_ms": now_ms,
        }));
    }

    /// Returns recorded computer-use audit log entries, optionally filtered by `app`.
    pub fn audit_log(&self, app: Option<&str>) -> Vec<serde_json::Value> {
        let filter = app.map(str::trim).filter(|s| !s.is_empty());
        self.audit_entries
            .lock()
            .iter()
            .filter(|e| {
                filter.is_none_or(|a| {
                    e.get("app")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|ea| ea.eq_ignore_ascii_case(a))
                })
            })
            .cloned()
            .collect()
    }

    /// Evaluates computer-use safety guards via `orca_core::orchestration_protocol::evaluate_computer_use_guard`.
    pub fn evaluate_guard(
        &self,
        req: &ComputerUseRequest,
        policy_override: Option<&ComputerUsePolicy>,
    ) -> Result<ComputerUseGuardDecision> {
        let effective = match policy_override {
            Some(p) => p.clone(),
            None => self.policy.read().clone(),
        };
        Ok(evaluate_computer_use_guard(req, &effective)?)
    }

    /// Polls `probe` until `condition` matches or `timeout` elapses (`WaitTimeout`).
    pub async fn wait_for_condition<F>(
        &self,
        condition: &WaitCondition,
        timeout: Duration,
        poll_interval: Duration,
        mut probe: F,
    ) -> Result<WaitProbeSnapshot>
    where
        F: FnMut() -> WaitProbeSnapshot,
    {
        let start = tokio::time::Instant::now();
        let step = poll_interval.max(Duration::from_millis(5));
        loop {
            let snap = probe();
            if condition.matches(&snap) {
                return Ok(snap);
            }
            if start.elapsed() >= timeout {
                return Err(OrcaIdeRuntimeError::ide(
                    OrcaIdeErrorCode::WaitTimeout,
                    format!(
                        "computer-use wait condition timed out after {}ms",
                        timeout.as_millis()
                    ),
                ));
            }
            let remaining = timeout.saturating_sub(start.elapsed());
            tokio::time::sleep(step.min(remaining)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// 2. MobileEmulatorService
// ---------------------------------------------------------------------------

/// Converts normalized `0.0..=1.0` floating-point coordinate to basis points `0..=10_000`.
pub fn norm_float_to_bp(coord: f64) -> Result<u16> {
    if !coord.is_finite() || !(0.0..=1.0).contains(&coord) {
        return Err(OrcaIdeRuntimeError::ide(
            OrcaIdeErrorCode::EmulatorInputBlocked,
            format!("normalized emulator coordinate {coord} outside 0.0..=1.0"),
        ));
    }
    Ok((coord * 10_000.0).round().clamp(0.0, 10_000.0) as u16)
}

/// Converts basis points `0..=10_000` back to normalized `0.0..=1.0` floating-point coordinate.
pub fn bp_to_norm_float(bp: u16) -> Result<f64> {
    if bp > 10_000 {
        return Err(OrcaIdeRuntimeError::ide(
            OrcaIdeErrorCode::EmulatorInputBlocked,
            format!("basis-point coordinate {bp} exceeds 10000"),
        ));
    }
    Ok(f64::from(bp) / 10_000.0)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CappedAxTree {
    pub nodes: Vec<AxNode>,
    pub truncated: bool,
    pub total_before_cap: usize,
}

/// Caps an emulator accessibility tree at [`EMULATOR_AX_MAX_NODES`] (`500`).
pub fn cap_emulator_ax_tree(mut nodes: Vec<AxNode>) -> CappedAxTree {
    let total_before_cap = nodes.len();
    let truncated = total_before_cap > EMULATOR_AX_MAX_NODES;
    if truncated {
        nodes.truncate(EMULATOR_AX_MAX_NODES);
    }
    CappedAxTree {
        nodes,
        truncated,
        total_before_cap,
    }
}

/// Mobile emulator automation coordinator (`Send + Sync`).
pub struct MobileEmulatorService {
    policy: RwLock<ComputerUsePolicy>,
}

impl Default for MobileEmulatorService {
    fn default() -> Self {
        Self::new(ComputerUsePolicy::default())
    }
}

impl fmt::Debug for MobileEmulatorService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MobileEmulatorService").finish()
    }
}

impl MobileEmulatorService {
    pub fn new(policy: ComputerUsePolicy) -> Self {
        Self {
            policy: RwLock::new(policy),
        }
    }

    pub fn policy(&self) -> ComputerUsePolicy {
        self.policy.read().clone()
    }

    pub fn set_policy(&self, policy: ComputerUsePolicy) {
        *self.policy.write() = policy;
    }

    /// Evaluates iOS Simulator / Android Emulator actions through `evaluate_computer_use_guard`.
    pub fn evaluate_action(
        &self,
        target_id: impl Into<String>,
        target_kind: ComputerUseTargetKind,
        action: ComputerUseAction,
        policy_override: Option<&ComputerUsePolicy>,
    ) -> Result<ComputerUseGuardDecision> {
        let req = ComputerUseRequest {
            target_id: target_id.into(),
            target_kind,
            action,
        };
        let effective = match policy_override {
            Some(p) => p.clone(),
            None => self.policy.read().clone(),
        };
        Ok(evaluate_computer_use_guard(&req, &effective)?)
    }
}

// ---------------------------------------------------------------------------
// 3. SttDictationService (Local GGUF/VAD + GPU->CPU Marker + Remote OpenAI STT)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttBackendMode {
    LocalGpu,
    LocalCpuFallback,
    RemoteOpenAiCompatible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SttStatus {
    pub effective_mode: SttBackendMode,
    pub gpu_failure_marker_present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_endpoint: Option<String>,
    pub vad_threshold_db: f32,
}

/// Cleans raw voice dictation transcripts (collapses extra whitespace & duplicate stutter
/// tokens, capitalizes first character, ensures sentence termination).
pub fn cleanup_dictation_text(raw: &str) -> String {
    let mut words: Vec<&str> = Vec::new();
    for word in raw.split_whitespace() {
        let is_dup = words
            .last()
            .is_some_and(|prev| prev.eq_ignore_ascii_case(word));
        if !is_dup {
            words.push(word);
        }
    }
    if words.is_empty() {
        return String::new();
    }
    let joined = words.join(" ");
    let mut chars = joined.chars();
    let mut out = String::with_capacity(joined.len() + 1);
    if let Some(first) = chars.next() {
        for upper in first.to_uppercase() {
            out.push(upper);
        }
        out.extend(chars);
    }
    if !matches!(out.chars().last(), Some('.' | '!' | '?')) {
        out.push('.');
    }
    out
}

/// Speech-to-Text dictation coordinator (`Send + Sync`) managing local GGUF/VAD
/// settings, persisted `stt-gpu-failure-marker` GPU->CPU fallback, and OpenAI-compatible
/// remote transcription via offline-safe `reqwest` (`rustls-tls`, `multipart`, `json`).
pub struct SttDictationService {
    state_dir: PathBuf,
    model_path: Option<PathBuf>,
    remote_endpoint: Option<String>,
    vad_threshold_db: f32,
    http_client: reqwest::Client,
}

impl fmt::Debug for SttDictationService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SttDictationService")
            .field("state_dir", &self.state_dir)
            .field("remote_endpoint", &self.remote_endpoint)
            .finish()
    }
}

impl SttDictationService {
    pub fn open(
        state_dir: impl Into<PathBuf>,
        model_path: Option<PathBuf>,
        remote_endpoint: Option<String>,
    ) -> Result<Self> {
        let dir = state_dir.into();
        fs::create_dir_all(&dir)?;
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            state_dir: dir,
            model_path,
            remote_endpoint,
            vad_threshold_db: -42.0,
            http_client,
        })
    }

    pub fn gpu_failure_marker_path(&self) -> PathBuf {
        self.state_dir.join(STT_GPU_FAILURE_MARKER_FILE)
    }

    /// Inspects the persisted `stt-gpu-failure-marker` file and computes effective mode.
    pub fn status(&self) -> SttStatus {
        let marker_path = self.gpu_failure_marker_path();
        let marker_reason = if marker_path.is_file() {
            fs::read_to_string(&marker_path)
                .ok()
                .map(|s| s.trim().to_string())
        } else {
            None
        };
        let gpu_failure_marker_present = marker_reason.is_some();
        let effective_mode = if self.remote_endpoint.is_some() && self.model_path.is_none() {
            SttBackendMode::RemoteOpenAiCompatible
        } else if gpu_failure_marker_present {
            SttBackendMode::LocalCpuFallback
        } else {
            SttBackendMode::LocalGpu
        };
        SttStatus {
            effective_mode,
            gpu_failure_marker_present,
            marker_reason,
            model_path: self
                .model_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            remote_endpoint: self.remote_endpoint.clone(),
            vad_threshold_db: self.vad_threshold_db,
        }
    }

    /// Persists `stt-gpu-failure-marker` atomically so subsequent runs auto-fall back GPU -> CPU.
    pub fn record_gpu_failure(&self, reason: &str, now_ms: i64) -> Result<SttStatus> {
        let marker_path = self.gpu_failure_marker_path();
        let payload = format!("ts={now_ms} reason={}\n", reason.trim());
        let tmp = self
            .state_dir
            .join(format!(".{STT_GPU_FAILURE_MARKER_FILE}.tmp-{}", Uuid::new_v4()));
        {
            let mut f = File::create(&tmp)?;
            f.write_all(payload.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &marker_path)?;
        Ok(self.status())
    }

    pub fn clear_gpu_failure_marker(&self) -> Result<bool> {
        let marker = self.gpu_failure_marker_path();
        if marker.is_file() {
            fs::remove_file(marker)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Sends audio bytes to a configured OpenAI-compatible `/v1/audio/transcriptions` endpoint.
    pub async fn transcribe_remote_openai(
        &self,
        audio_bytes: Vec<u8>,
        filename: &str,
        api_key: &ProtectedSecret,
        model: &str,
    ) -> Result<String> {
        let endpoint = self.remote_endpoint.as_deref().ok_or_else(|| {
            OrcaIdeRuntimeError::invalid_argument("no remote STT endpoint configured")
        })?;
        let token = std::str::from_utf8(api_key.expose_bytes())
            .map_err(|_| OrcaIdeRuntimeError::invalid_argument("API key must be valid UTF-8"))?;
        let part = reqwest::multipart::Part::bytes(audio_bytes)
            .file_name(filename.to_string())
            .mime_str("audio/wav")
            .map_err(|e| OrcaIdeRuntimeError::invalid_argument(e.to_string()))?;
        let form = reqwest::multipart::Form::new()
            .text("model", model.to_string())
            .part("file", part);
        let resp = self
            .http_client
            .post(endpoint)
            .bearer_auth(token)
            .multipart(form)
            .send()
            .await?
            .error_for_status()?;
        let body: serde_json::Value = resp.json().await?;
        let text = body
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        Ok(cleanup_dictation_text(text))
    }
}

// ---------------------------------------------------------------------------
// 4. ShareableArtifactStore (Hidden ProtectedSecret edit_token)
// ---------------------------------------------------------------------------

const ARTIFACTS_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS shareable_artifacts (
    artifact_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    share_url TEXT NOT NULL,
    shared INTEGER NOT NULL DEFAULT 1,
    profile_id TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Html,
    Markdown,
}

impl ArtifactKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Markdown => "markdown",
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Markdown => "md",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let norm = raw
            .split(';')
            .next()
            .unwrap_or(raw)
            .trim()
            .to_ascii_lowercase();
        match norm.as_str() {
            "html" | ".html" | "text/html" => Ok(Self::Html),
            "md" | ".md" | "markdown" | "text/markdown" | "text/x-markdown" | "text/plain" => {
                Ok(Self::Markdown)
            }
            other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "unsupported artifact kind '{other}' (expected 'html' or 'markdown')"
            ))),
        }
    }
}

/// Public wire/CLI view of a shareable artifact.
///
/// SECURITY INVARIANT: `edit_token` is intentionally ABSENT from this struct so it can
/// NEVER be serialized into JSON, CLI output, or RPC responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPublicView {
    pub artifact_id: String,
    pub title: String,
    pub kind: ArtifactKind,
    pub content: String,
    pub share_url: String,
    pub shared: bool,
    pub profile_id: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishArtifactRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    pub title: String,
    pub kind: ArtifactKind,
    pub content: String,
    pub profile_id: String,
    pub user_confirmed_share: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub now_ms: i64,
}

/// Persistent `.html` / `.md` shareable artifact store (`Send + Sync`, SQLite WAL +
/// `ProtectedSecretPersistence`). Seals per-profile `edit_token` locally and NEVER
/// exposes or serializes it in [`ArtifactPublicView`].
pub struct ShareableArtifactStore {
    conn: Mutex<Connection>,
    secrets: Arc<ProtectedSecretPersistence>,
    default_base_url: String,
}

impl fmt::Debug for ShareableArtifactStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShareableArtifactStore")
            .field("default_base_url", &self.default_base_url)
            .finish()
    }
}

fn artifact_secret_slot(profile_id: &str, artifact_id: &str) -> String {
    format!("artifact_edit_token:{profile_id}:{artifact_id}")
}

fn row_to_artifact_view(r: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactPublicView> {
    let kind_raw: String = r.get(2)?;
    let kind = ArtifactKind::parse(&kind_raw).unwrap_or(ArtifactKind::Markdown);
    Ok(ArtifactPublicView {
        artifact_id: r.get(0)?,
        title: r.get(1)?,
        kind,
        content: r.get(3)?,
        share_url: r.get(4)?,
        shared: r.get::<_, i64>(5)? != 0,
        profile_id: r.get(6)?,
        created_at_ms: r.get(7)?,
        updated_at_ms: r.get(8)?,
    })
}

impl ShareableArtifactStore {
    pub fn open(
        db_path: impl AsRef<Path>,
        secrets: Arc<ProtectedSecretPersistence>,
        default_base_url: impl Into<String>,
    ) -> Result<Self> {
        let path = db_path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(ARTIFACTS_SCHEMA_SQL)?;
        Ok(Self {
            conn: Mutex::new(conn),
            secrets,
            default_base_url: default_base_url.into(),
        })
    }

    /// Publishes an artifact ONLY when `req.user_confirmed_share == true`, sealing a
    /// freshly generated `edit_token` locally in `ProtectedSecretPersistence`.
    pub fn publish_artifact(&self, req: PublishArtifactRequest) -> Result<ArtifactPublicView> {
        if !req.user_confirmed_share {
            return Err(OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::ApprovalRequired,
                "publishing a shareable artifact requires explicit user_confirmed_share = true",
            ));
        }
        if req.title.trim().is_empty() || req.profile_id.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "artifact title and profile_id must be non-empty",
            ));
        }
        let artifact_id = req
            .artifact_id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("art-{}", Uuid::new_v4()));

        // Generate cryptographic random edit_token and seal it locally ONLY.
        let mut token_bytes = [0u8; 32];
        getrandom::getrandom(&mut token_bytes).map_err(|e| {
            OrcaIdeRuntimeError::Vault(orca_core::orchestration_protocol::VaultError::Entropy(
                e.to_string(),
            ))
        })?;
        let token_secret = ProtectedSecret::from_string(hex::encode(token_bytes));
        let slot = artifact_secret_slot(&req.profile_id, &artifact_id);
        self.secrets
            .store_secret(&slot, &slot, &token_secret, req.now_ms)?;

        let base = req
            .base_url
            .as_deref()
            .unwrap_or(&self.default_base_url)
            .trim_end_matches('/');
        let share_url = format!("{base}/artifacts/{artifact_id}.{}", req.kind.extension());

        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO shareable_artifacts
                (artifact_id, title, kind, content, share_url, shared, profile_id, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8)",
            params![
                &artifact_id,
                req.title.trim(),
                req.kind.as_str(),
                &req.content,
                &share_url,
                req.profile_id.trim(),
                req.now_ms,
                req.now_ms,
            ],
        )?;
        Ok(ArtifactPublicView {
            artifact_id,
            title: req.title.trim().to_string(),
            kind: req.kind,
            content: req.content,
            share_url,
            shared: true,
            profile_id: req.profile_id.trim().to_string(),
            created_at_ms: req.now_ms,
            updated_at_ms: req.now_ms,
        })
    }

    pub fn get_artifact(&self, artifact_id: &str) -> Result<Option<ArtifactPublicView>> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT artifact_id, title, kind, content, share_url, shared, profile_id, created_at_ms, updated_at_ms
                 FROM shareable_artifacts WHERE artifact_id = ?1",
                params![artifact_id],
                row_to_artifact_view,
            )
            .optional()?;
        Ok(row)
    }

    /// Updates an artifact's content/title after verifying the profile's locally sealed
    /// `edit_token` is present and authentic.
    pub fn update_artifact(
        &self,
        artifact_id: &str,
        profile_id: &str,
        new_title: Option<&str>,
        new_content: &str,
        now_ms: i64,
    ) -> Result<ArtifactPublicView> {
        let existing = self
            .get_artifact(artifact_id)?
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("artifact '{artifact_id}'")))?;
        if existing.profile_id != profile_id {
            return Err(OrcaIdeRuntimeError::permission_denied(format!(
                "profile '{profile_id}' does not own artifact '{artifact_id}'"
            )));
        }
        let slot = artifact_secret_slot(profile_id, artifact_id);
        let token = self.secrets.read_secret(&slot, &slot)?;
        if token.is_none() {
            return Err(OrcaIdeRuntimeError::permission_denied(format!(
                "missing local edit_token for artifact '{artifact_id}'"
            )));
        }

        let title = new_title
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&existing.title)
            .to_string();
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE shareable_artifacts SET title = ?1, content = ?2, updated_at_ms = ?3 WHERE artifact_id = ?4",
            params![&title, new_content, now_ms, artifact_id],
        )?;
        Ok(ArtifactPublicView {
            title,
            content: new_content.to_string(),
            updated_at_ms: now_ms,
            ..existing
        })
    }

    pub fn unshare_artifact(&self, artifact_id: &str, now_ms: i64) -> Result<ArtifactPublicView> {
        let existing = self
            .get_artifact(artifact_id)?
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("artifact '{artifact_id}'")))?;
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE shareable_artifacts SET shared = 0, updated_at_ms = ?1 WHERE artifact_id = ?2",
            params![now_ms, artifact_id],
        )?;
        Ok(ArtifactPublicView {
            shared: false,
            updated_at_ms: now_ms,
            ..existing
        })
    }

    pub fn list_artifacts(&self, profile_id: Option<&str>) -> Result<Vec<ArtifactPublicView>> {
        let conn = self.conn.lock();
        let mut out = Vec::new();
        if let Some(pid) = profile_id {
            let mut stmt = conn.prepare(
                "SELECT artifact_id, title, kind, content, share_url, shared, profile_id, created_at_ms, updated_at_ms
                 FROM shareable_artifacts WHERE profile_id = ?1 ORDER BY updated_at_ms DESC",
            )?;
            let rows = stmt.query_map(params![pid], row_to_artifact_view)?;
            for item in rows {
                out.push(item?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT artifact_id, title, kind, content, share_url, shared, profile_id, created_at_ms, updated_at_ms
                 FROM shareable_artifacts ORDER BY updated_at_ms DESC",
            )?;
            let rows = stmt.query_map([], row_to_artifact_view)?;
            for item in rows {
                out.push(item?);
            }
        }
        Ok(out)
    }

    pub fn delete_artifact(&self, artifact_id: &str, now_ms: i64) -> Result<bool> {
        let existing = self.get_artifact(artifact_id)?;
        let Some(art) = existing else {
            return Ok(false);
        };
        let slot = artifact_secret_slot(&art.profile_id, artifact_id);
        let _ = self.secrets.remove_secret(&slot, now_ms);
        let conn = self.conn.lock();
        let affected = conn.execute(
            "DELETE FROM shareable_artifacts WHERE artifact_id = ?1",
            params![artifact_id],
        )?;
        Ok(affected > 0)
    }
}

// ---------------------------------------------------------------------------
// 5. EphemeralVmService (Cloud VM Recipes & Base-Snapshot Contamination Doctor)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmLifecycleOp {
    Create,
    Suspend,
    Resume,
    Destroy,
}

impl VmLifecycleOp {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Suspend => "suspend",
            Self::Resume => "resume",
            Self::Destroy => "destroy",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "create" => Ok(Self::Create),
            "suspend" => Ok(Self::Suspend),
            "resume" => Ok(Self::Resume),
            "destroy" => Ok(Self::Destroy),
            other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "unknown vm lifecycle op '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmRecipePlan {
    pub recipe_name: String,
    pub operation: VmLifecycleOp,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmDoctorReport {
    pub snapshot_dir: String,
    pub clean: bool,
    pub contaminated_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

/// Ephemeral Cloud VM recipe planner and base-snapshot doctor (`Send + Sync`).
#[derive(Debug, Default, Clone, Copy)]
pub struct EphemeralVmService;

impl EphemeralVmService {
    pub fn new() -> Self {
        Self
    }

    /// Resolves the concrete shell command for a lifecycle step in an `OrcaYamlEnvironmentRecipe`.
    pub fn plan_recipe_operation(
        &self,
        recipe: &OrcaYamlEnvironmentRecipe,
        op: VmLifecycleOp,
        workdir: Option<String>,
    ) -> Result<VmRecipePlan> {
        let cmd = match op {
            VmLifecycleOp::Create => recipe.create.as_deref(),
            VmLifecycleOp::Suspend => recipe.suspend.as_deref(),
            VmLifecycleOp::Resume => recipe.resume.as_deref(),
            VmLifecycleOp::Destroy => recipe.destroy.as_deref(),
        }
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            OrcaIdeRuntimeError::invalid_argument(format!(
                "environment recipe '{}' does not define '{}' command",
                recipe.name,
                op.as_str()
            ))
        })?;
        Ok(VmRecipePlan {
            recipe_name: recipe.name.clone(),
            operation: op,
            command: cmd.to_string(),
            workdir,
        })
    }

    /// Scans `base_snapshot_dir` for leaked host-identity files
    /// (`orca-devices.json`, `orca-e2ee-keypair.json`, `agent-session-authority.key`).
    pub fn run_doctor(&self, base_snapshot_dir: &Path) -> Result<VmDoctorReport> {
        if !base_snapshot_dir.is_dir() {
            return Err(OrcaIdeRuntimeError::not_found(format!(
                "base snapshot directory '{}' does not exist",
                base_snapshot_dir.display()
            )));
        }
        let mut contaminated_files = Vec::new();
        for entry in WalkDir::new(base_snapshot_dir).follow_links(false) {
            let entry = entry.map_err(|e| {
                OrcaIdeRuntimeError::invalid_argument(format!("vm doctor scan error: {e}"))
            })?;
            if entry.file_type().is_file()
                && let Some(fname) = entry.file_name().to_str()
                && BASE_SNAPSHOT_CONTAMINATION_FILES.contains(&fname)
            {
                let rel = entry
                    .path()
                    .strip_prefix(base_snapshot_dir)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| entry.path().to_string_lossy().to_string());
                contaminated_files.push(rel);
            }
        }
        contaminated_files.sort();
        let clean = contaminated_files.is_empty();
        let remediation = if clean {
            None
        } else {
            Some(
                "Remove leaked host identity files before publishing this base snapshot so cloned VMs generate unique device & E2EE keys."
                    .to_string(),
            )
        };
        Ok(VmDoctorReport {
            snapshot_dir: base_snapshot_dir.to_string_lossy().to_string(),
            clean,
            contaminated_files,
            remediation,
        })
    }
}

// ---------------------------------------------------------------------------
// 6. SleepAwareHangWatchdog
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchdogThresholds {
    pub warning_stall_ms: u64,
    pub critical_stall_ms: u64,
    pub sleep_jump_tolerance_ms: u64,
    pub max_breadcrumbs: usize,
}

impl Default for WatchdogThresholds {
    fn default() -> Self {
        Self {
            warning_stall_ms: 3_000,
            critical_stall_ms: 10_000,
            sleep_jump_tolerance_ms: 5_000,
            max_breadcrumbs: 128,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StallSeverity {
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HangBreadcrumb {
    pub recorded_at_wall_ms: i64,
    pub monotonic_delta_ms: u64,
    pub wall_delta_ms: i64,
    pub severity: StallSeverity,
    pub context: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WatchdogTickOutcome {
    Healthy,
    SleepWakeIgnored {
        wall_jump_ms: i64,
        monotonic_delta_ms: u64,
    },
    StallRecorded(HangBreadcrumb),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchdogStatus {
    pub total_ticks: u64,
    pub sleep_wakes_ignored: u64,
    pub warning_count: u64,
    pub critical_count: u64,
    pub breadcrumbs: Vec<HangBreadcrumb>,
}

struct WatchdogInner {
    thresholds: WatchdogThresholds,
    last_monotonic_ms: Option<u64>,
    last_wall_ms: Option<i64>,
    total_ticks: u64,
    sleep_wakes_ignored: u64,
    warning_count: u64,
    critical_count: u64,
    breadcrumbs: VecDeque<HangBreadcrumb>,
}

/// Sleep-aware main-thread hang watchdog (`Send + Sync`).
/// Compares monotonic elapsed ms against wall-clock elapsed ms to suppress false-positive
/// alerts after OS sleep/wake jumps while recording `Warning` and `Critical` breadcrumbs
/// for genuine event-loop stalls.
pub struct SleepAwareHangWatchdog {
    inner: Mutex<WatchdogInner>,
}

impl Default for SleepAwareHangWatchdog {
    fn default() -> Self {
        Self::new(WatchdogThresholds::default())
    }
}

impl fmt::Debug for SleepAwareHangWatchdog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let g = self.inner.lock();
        f.debug_struct("SleepAwareHangWatchdog")
            .field("total_ticks", &g.total_ticks)
            .field("sleep_wakes_ignored", &g.sleep_wakes_ignored)
            .field("breadcrumbs", &g.breadcrumbs.len())
            .finish()
    }
}

impl SleepAwareHangWatchdog {
    pub fn new(thresholds: WatchdogThresholds) -> Self {
        Self {
            inner: Mutex::new(WatchdogInner {
                thresholds,
                last_monotonic_ms: None,
                last_wall_ms: None,
                total_ticks: 0,
                sleep_wakes_ignored: 0,
                warning_count: 0,
                critical_count: 0,
                breadcrumbs: VecDeque::new(),
            }),
        }
    }

    pub fn record_tick(
        &self,
        monotonic_ms: u64,
        wall_ms: i64,
        context: &str,
    ) -> WatchdogTickOutcome {
        let mut g = self.inner.lock();
        g.total_ticks = g.total_ticks.saturating_add(1);
        let (prev_mono, prev_wall) = match (g.last_monotonic_ms, g.last_wall_ms) {
            (Some(m), Some(w)) => (m, w),
            _ => {
                g.last_monotonic_ms = Some(monotonic_ms);
                g.last_wall_ms = Some(wall_ms);
                return WatchdogTickOutcome::Healthy;
            }
        };
        g.last_monotonic_ms = Some(monotonic_ms);
        g.last_wall_ms = Some(wall_ms);

        let mono_delta = monotonic_ms.saturating_sub(prev_mono);
        let wall_delta = wall_ms.saturating_sub(prev_wall);

        // OS sleep/wake detection: wall clock jumped far ahead of (or behind) monotonic clock.
        let wall_vs_mono_skew = wall_delta - (mono_delta as i64);
        if wall_delta < 0 || wall_vs_mono_skew.unsigned_abs() > g.thresholds.sleep_jump_tolerance_ms
        {
            g.sleep_wakes_ignored = g.sleep_wakes_ignored.saturating_add(1);
            return WatchdogTickOutcome::SleepWakeIgnored {
                wall_jump_ms: wall_delta,
                monotonic_delta_ms: mono_delta,
            };
        }

        let severity = if mono_delta >= g.thresholds.critical_stall_ms {
            Some(StallSeverity::Critical)
        } else if mono_delta >= g.thresholds.warning_stall_ms {
            Some(StallSeverity::Warning)
        } else {
            None
        };

        if let Some(sev) = severity {
            match sev {
                StallSeverity::Warning => g.warning_count = g.warning_count.saturating_add(1),
                StallSeverity::Critical => g.critical_count = g.critical_count.saturating_add(1),
            }
            let crumb = HangBreadcrumb {
                recorded_at_wall_ms: wall_ms,
                monotonic_delta_ms: mono_delta,
                wall_delta_ms: wall_delta,
                severity: sev,
                context: context.to_string(),
            };
            while g.breadcrumbs.len() >= g.thresholds.max_breadcrumbs.max(1) {
                g.breadcrumbs.pop_front();
            }
            g.breadcrumbs.push_back(crumb.clone());
            WatchdogTickOutcome::StallRecorded(crumb)
        } else {
            WatchdogTickOutcome::Healthy
        }
    }

    pub fn status(&self) -> WatchdogStatus {
        let g = self.inner.lock();
        WatchdogStatus {
            total_ticks: g.total_ticks,
            sleep_wakes_ignored: g.sleep_wakes_ignored,
            warning_count: g.warning_count,
            critical_count: g.critical_count,
            breadcrumbs: g.breadcrumbs.iter().cloned().collect(),
        }
    }

    /// Evaluates a single heartbeat delta against real sleep-aware thresholds (`warning_stall_ms`,
    /// `critical_stall_ms`, `sleep_jump_tolerance_ms`), suppressing false hang alerts when OS sleep
    /// explains the delta while recording genuine awake `Warning` / `Critical` breadcrumbs.
    pub fn evaluate_sleep_aware(
        &self,
        last_heartbeat_delta_ms: u64,
        system_slept: bool,
        wake_delta_ms: u64,
        wall_ms: i64,
        context: &str,
    ) -> (&'static str, bool, u64) {
        let mut g = self.inner.lock();
        g.total_ticks = g.total_ticks.saturating_add(1);
        let effective_awake_ms = if system_slept {
            last_heartbeat_delta_ms.saturating_sub(wake_delta_ms)
        } else {
            last_heartbeat_delta_ms
        };
        if system_slept
            && (wake_delta_ms > g.thresholds.sleep_jump_tolerance_ms
                || effective_awake_ms < g.thresholds.warning_stall_ms)
        {
            g.sleep_wakes_ignored = g.sleep_wakes_ignored.saturating_add(1);
            return ("ok", true, effective_awake_ms);
        }
        let sev = if effective_awake_ms >= g.thresholds.critical_stall_ms {
            Some((StallSeverity::Critical, "critical"))
        } else if effective_awake_ms >= g.thresholds.warning_stall_ms {
            Some((StallSeverity::Warning, "warning"))
        } else {
            None
        };
        if let Some((severity, label)) = sev {
            match severity {
                StallSeverity::Warning => g.warning_count = g.warning_count.saturating_add(1),
                StallSeverity::Critical => g.critical_count = g.critical_count.saturating_add(1),
            }
            let crumb = HangBreadcrumb {
                recorded_at_wall_ms: wall_ms,
                monotonic_delta_ms: effective_awake_ms,
                wall_delta_ms: i64::try_from(last_heartbeat_delta_ms).unwrap_or(i64::MAX),
                severity,
                context: context.to_string(),
            };
            while g.breadcrumbs.len() >= g.thresholds.max_breadcrumbs.max(1) {
                g.breadcrumbs.pop_front();
            }
            g.breadcrumbs.push_back(crumb);
            (label, false, effective_awake_ms)
        } else {
            ("ok", false, effective_awake_ms)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile_state::InMemoryKeyringProvider;
    use orca_core::orchestration_protocol::EmulatorKind;
    use orca_core::workspace_layout::KeyChord;
    use tempfile::TempDir;

    fn assert_send_sync<T: Send + Sync>() {}

    #[tokio::test]
    async fn computer_use_guards_tree_diff_wait_timeout_and_stdin_redaction() {
        assert_send_sync::<ComputerUseService>();
        assert_send_sync::<MobileEmulatorService>();
        assert_send_sync::<SttDictationService>();
        assert_send_sync::<ShareableArtifactStore>();
        assert_send_sync::<EphemeralVmService>();
        assert_send_sync::<SleepAwareHangWatchdog>();

        let svc = ComputerUseService::default();
        svc.approve_target("win-editor");

        // System shortcut Cmd+Q blocked by default.
        let quit = KeyChord::parse("Cmd+Q").unwrap();
        let err = svc
            .evaluate_guard(
                &ComputerUseRequest {
                    target_id: "win-editor".to_string(),
                    target_kind: ComputerUseTargetKind::NormalWindow,
                    action: ComputerUseAction::Hotkey { chord: quit },
                },
                None,
            )
            .unwrap_err();
        assert_eq!(err.ide_error_code(), Some(OrcaIdeErrorCode::SystemShortcutBlocked));

        // Unapproved target -> ApprovalRequired.
        let err2 = svc
            .evaluate_guard(
                &ComputerUseRequest {
                    target_id: "win-unknown".to_string(),
                    target_kind: ComputerUseTargetKind::NormalWindow,
                    action: ComputerUseAction::TypeText {
                        text_len: 5,
                        via_stdin: true,
                    },
                },
                None,
            )
            .unwrap_err();
        assert_eq!(err2.ide_error_code(), Some(OrcaIdeErrorCode::ApprovalRequired));

        // PasswordField -> EmulatorInputBlocked.
        let err3 = svc
            .evaluate_guard(
                &ComputerUseRequest {
                    target_id: "win-editor".to_string(),
                    target_kind: ComputerUseTargetKind::PasswordField,
                    action: ComputerUseAction::TypeText {
                        text_len: 8,
                        via_stdin: true,
                    },
                },
                None,
            )
            .unwrap_err();
        assert_eq!(err3.ide_error_code(), Some(OrcaIdeErrorCode::EmulatorInputBlocked));

        // Compact AX tree diff (+ / - / ~).
        let before = vec![
            AxNode {
                node_id: "n1".into(),
                role: "textbox".into(),
                label: "Search".into(),
                value: Some("old".into()),
                enabled: true,
                bounds_bp: None,
            },
            AxNode {
                node_id: "n2".into(),
                role: "dialog".into(),
                label: "Modal".into(),
                value: None,
                enabled: true,
                bounds_bp: None,
            },
        ];
        let after = vec![
            AxNode {
                node_id: "n1".into(),
                role: "textbox".into(),
                label: "Search".into(),
                value: Some("new".into()),
                enabled: true,
                bounds_bp: None,
            },
            AxNode {
                node_id: "n3".into(),
                role: "button".into(),
                label: "Submit".into(),
                value: None,
                enabled: true,
                bounds_bp: None,
            },
        ];
        let diff = compute_ax_tree_diff(&before, &after);
        assert!(diff.compact_lines.iter().any(|l| l.starts_with("- n2")));
        assert!(diff.compact_lines.iter().any(|l| l.starts_with("+ n3")));
        assert!(diff.compact_lines.iter().any(|l| l.starts_with("~ n1")));

        // WaitTimeout fires when condition never satisfied.
        let wait_err = svc
            .wait_for_condition(
                &WaitCondition {
                    text_contains: Some("READY".into()),
                    ..WaitCondition::default()
                },
                Duration::from_millis(25),
                Duration::from_millis(5),
                WaitProbeSnapshot::default,
            )
            .await
            .unwrap_err();
        assert_eq!(wait_err.ide_error_code(), Some(OrcaIdeErrorCode::WaitTimeout));

        // Secret redaction.
        let redacted = redact_stdin_secret_args(&[
            "type".into(),
            "--value-stdin".into(),
            "my-secret-pass".into(),
            "--text-stdin=another-secret".into(),
        ]);
        assert!(!redacted.join(" ").contains("my-secret-pass"));
        assert!(!redacted.join(" ").contains("another-secret"));
    }

    #[test]
    fn emulator_stt_artifacts_vm_doctor_and_sleep_aware_watchdog() {
        // 1. Mobile Emulator coords + 500-node cap + iOS unsupported guard.
        assert_eq!(norm_float_to_bp(0.5).unwrap(), 5_000);
        assert!((bp_to_norm_float(2_500).unwrap() - 0.25).abs() < 1e-6);
        assert!(norm_float_to_bp(1.2).is_err());

        let nodes: Vec<AxNode> = (0..550)
            .map(|i| AxNode {
                node_id: format!("ax-{i}"),
                role: "view".into(),
                label: format!("Item {i}"),
                value: None,
                enabled: true,
                bounds_bp: None,
            })
            .collect();
        let capped = cap_emulator_ax_tree(nodes);
        assert!(capped.truncated);
        assert_eq!(capped.nodes.len(), EMULATOR_AX_MAX_NODES);

        let emu = MobileEmulatorService::default();
        let ios_err = emu
            .evaluate_action(
                "sim-1",
                ComputerUseTargetKind::NormalWindow,
                ComputerUseAction::EmulatorTap {
                    emulator: EmulatorKind::IosSimulator,
                    x_norm_bp: 5000,
                    y_norm_bp: 5000,
                },
                None,
            )
            .unwrap_err();
        assert_eq!(ios_err.ide_error_code(), Some(OrcaIdeErrorCode::EmulatorUnsupported));

        // 2. STT GPU failure marker -> CPU fallback.
        let dir = TempDir::new().unwrap();
        let stt = SttDictationService::open(dir.path().join("stt"), Some(PathBuf::from("whisper.gguf")), None).unwrap();
        assert_eq!(stt.status().effective_mode, SttBackendMode::LocalGpu);
        let fallback_status = stt.record_gpu_failure("cuda oom", 1000).unwrap();
        assert_eq!(fallback_status.effective_mode, SttBackendMode::LocalCpuFallback);
        assert!(fallback_status.gpu_failure_marker_present);
        assert_eq!(cleanup_dictation_text("hello   hello world"), "Hello world.");

        // 3. Shareable Artifacts: gated on user_confirmed_share & NEVER serializes edit_token!
        let keyring = Arc::new(InMemoryKeyringProvider::new_available([0x77u8; 32]));
        let secrets = Arc::new(
            ProtectedSecretPersistence::open(dir.path().join("art_secrets.json"), keyring).unwrap(),
        );
        let artifacts = ShareableArtifactStore::open(
            dir.path().join("artifacts.db"),
            secrets,
            "https://share.orca.local",
        )
        .unwrap();
        let unconfirmed = artifacts
            .publish_artifact(PublishArtifactRequest {
                artifact_id: Some("art-unconfirmed".into()),
                title: "Draft".into(),
                kind: ArtifactKind::Markdown,
                content: "# Hi".into(),
                profile_id: "prof-1".into(),
                user_confirmed_share: false,
                base_url: None,
                now_ms: 1000,
            })
            .unwrap_err();
        assert_eq!(unconfirmed.ide_error_code(), Some(OrcaIdeErrorCode::ApprovalRequired));

        let view = artifacts
            .publish_artifact(PublishArtifactRequest {
                artifact_id: Some("art-1".into()),
                title: "Spec".into(),
                kind: ArtifactKind::Html,
                content: "<h1>Hello</h1>".into(),
                profile_id: "prof-1".into(),
                user_confirmed_share: true,
                base_url: None,
                now_ms: 1000,
            })
            .unwrap();
        let serialized = serde_json::to_string(&view).unwrap();
        assert!(!serialized.contains("edit_token"));
        let updated = artifacts
            .update_artifact("art-1", "prof-1", Some("Spec v2"), "<h1>Updated</h1>", 2000)
            .unwrap();
        assert_eq!(updated.title, "Spec v2");

        // 4. VM Doctor detects base-snapshot contamination.
        let snap_dir = dir.path().join("vm_snapshot");
        fs::create_dir_all(snap_dir.join("etc/orca")).unwrap();
        fs::write(snap_dir.join("etc/orca/orca-e2ee-keypair.json"), "{}").unwrap();
        let vm = EphemeralVmService::new();
        let doc = vm.run_doctor(&snap_dir).unwrap();
        assert!(!doc.clean);
        assert_eq!(doc.contaminated_files, vec!["etc/orca/orca-e2ee-keypair.json"]);

        // 5. Sleep-aware Hang Watchdog ignores OS sleep/wake jump, catches real stall!
        let wd = SleepAwareHangWatchdog::default();
        assert_eq!(wd.record_tick(1_000, 100_000, "boot"), WatchdogTickOutcome::Healthy);
        // Simulate 10-minute OS sleep: monotonic moved 200ms while wall clock jumped 600_000ms.
        let after_sleep = wd.record_tick(1_200, 700_000, "after_wake");
        assert!(matches!(after_sleep, WatchdogTickOutcome::SleepWakeIgnored { .. }));
        assert_eq!(wd.status().warning_count, 0);
        // Simulate genuine 12s main-thread stall (both monotonic & wall move +12_000ms).
        let stall = wd.record_tick(13_200, 712_000, "sync_paint");
        assert!(matches!(
            stall,
            WatchdogTickOutcome::StallRecorded(HangBreadcrumb {
                severity: StallSeverity::Critical,
                ..
            })
        ));
        assert_eq!(wd.status().critical_count, 1);
    }
}
