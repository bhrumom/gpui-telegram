//! Pure-Rust domain contracts and security guards for Diff Annotations & AI
//! Line Attribution, Browser Design Mode, 4-Tier Plugin System V2, 8-Check
//! Background Hibernation Policy, and Safe ComputerUse / Mobile Emulator Guards.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::workspace_layout::{KeyChord, is_system_shortcut_blocked};
use crate::{OrcaIdeError, OrcaIdeErrorCode};

// ---------------------------------------------------------------------------
// Diff Annotations & Per-Line AI Attribution
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffSide {
    Left,
    Right,
    Unified,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LineAttribution {
    Human,
    Bot { bot_id: String },
    Worker { dispatch_id: String },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffAnnotation {
    pub annotation_id: String,
    pub file_path: String,
    pub side: DiffSide,
    pub line_start: u32,
    pub line_end: u32,
    pub body: String,
    pub author: LineAttribution,
    pub resolved: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum DiffAnnotationError {
    #[error("annotation_id cannot be empty")]
    EmptyId,
    #[error("duplicate annotation_id '{0}'")]
    DuplicateId(String),
    #[error("annotation file_path cannot be empty")]
    EmptyFilePath,
    #[error("annotation body cannot be empty")]
    EmptyBody,
    #[error("invalid line range {start}..={end}")]
    InvalidLineRange { start: u32, end: u32 },
    #[error("annotation '{0}' not found")]
    NotFound(String),
}

/// Pure in-memory diff annotation store supporting line-shift re-anchoring and
/// single-batch "Send to agent" prompt composition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffAnnotationStore {
    pub annotations: BTreeMap<String, DiffAnnotation>,
}

impl DiffAnnotationStore {
    pub fn add(&mut self, annotation: DiffAnnotation) -> Result<(), DiffAnnotationError> {
        if annotation.annotation_id.trim().is_empty() {
            return Err(DiffAnnotationError::EmptyId);
        }
        if annotation.file_path.trim().is_empty() {
            return Err(DiffAnnotationError::EmptyFilePath);
        }
        if annotation.body.trim().is_empty() {
            return Err(DiffAnnotationError::EmptyBody);
        }
        if annotation.line_start == 0 || annotation.line_end < annotation.line_start {
            return Err(DiffAnnotationError::InvalidLineRange {
                start: annotation.line_start,
                end: annotation.line_end,
            });
        }
        if self.annotations.contains_key(&annotation.annotation_id) {
            return Err(DiffAnnotationError::DuplicateId(annotation.annotation_id));
        }
        self.annotations
            .insert(annotation.annotation_id.clone(), annotation);
        Ok(())
    }

    pub fn resolve(
        &mut self,
        annotation_id: &str,
        now_ms: i64,
    ) -> Result<(), DiffAnnotationError> {
        let item = self
            .annotations
            .get_mut(annotation_id)
            .ok_or_else(|| DiffAnnotationError::NotFound(annotation_id.to_string()))?;
        item.resolved = true;
        item.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn reopen(
        &mut self,
        annotation_id: &str,
        now_ms: i64,
    ) -> Result<(), DiffAnnotationError> {
        let item = self
            .annotations
            .get_mut(annotation_id)
            .ok_or_else(|| DiffAnnotationError::NotFound(annotation_id.to_string()))?;
        item.resolved = false;
        item.updated_at_ms = now_ms;
        Ok(())
    }

    pub fn filter_by_file(
        &self,
        file_path: &str,
        include_resolved: bool,
    ) -> Vec<&DiffAnnotation> {
        let mut out: Vec<&DiffAnnotation> = self
            .annotations
            .values()
            .filter(|a| a.file_path == file_path && (include_resolved || !a.resolved))
            .collect();
        out.sort_by(|a, b| {
            a.line_start
                .cmp(&b.line_start)
                .then_with(|| a.annotation_id.cmp(&b.annotation_id))
        });
        out
    }

    pub fn filter_by_line(
        &self,
        file_path: &str,
        side: DiffSide,
        line: u32,
        include_resolved: bool,
    ) -> Vec<&DiffAnnotation> {
        self.filter_by_file(file_path, include_resolved)
            .into_iter()
            .filter(|a| a.side == side && line >= a.line_start && line <= a.line_end)
            .collect()
    }

    /// Re-anchors annotations at or below `at_line` when edits insert/delete lines.
    pub fn apply_line_shift(
        &mut self,
        file_path: &str,
        side: DiffSide,
        at_line: u32,
        delta: i32,
    ) {
        for ann in self.annotations.values_mut() {
            if ann.file_path == file_path && ann.side == side && ann.line_start >= at_line {
                let new_start = (ann.line_start as i64 + delta as i64).max(1) as u32;
                let span = ann.line_end.saturating_sub(ann.line_start);
                ann.line_start = new_start;
                ann.line_end = new_start.saturating_add(span);
            }
        }
    }

    /// Formats all unresolved annotations (optionally scoped to one file) into a
    /// single coherent line-anchored prompt for batched "Send to agent".
    pub fn build_agent_batch_prompt(&self, file_path: Option<&str>) -> String {
        let mut items: Vec<&DiffAnnotation> = self
            .annotations
            .values()
            .filter(|a| !a.resolved && file_path.is_none_or(|fp| a.file_path == fp))
            .collect();
        items.sort_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then_with(|| a.line_start.cmp(&b.line_start))
                .then_with(|| a.annotation_id.cmp(&b.annotation_id))
        });
        let mut lines = Vec::with_capacity(items.len());
        for a in items {
            if a.line_start == a.line_end {
                lines.push(format!("- {}:L{}: {}", a.file_path, a.line_start, a.body));
            } else {
                lines.push(format!(
                    "- {}:L{}-L{}: {}",
                    a.file_path, a.line_start, a.line_end, a.body
                ));
            }
        }
        lines.join("\n")
    }
}

/// Local per-line AI vs Human attribution ledger (never pollutes commits; human
/// edits flip affected lines back to `LineAttribution::Human`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAttributionMap {
    pub by_file: BTreeMap<String, BTreeMap<u32, LineAttribution>>,
}

impl FileAttributionMap {
    pub fn record_range(
        &mut self,
        file_path: impl Into<String>,
        line_start: u32,
        line_end: u32,
        attribution: LineAttribution,
    ) {
        if line_start == 0 || line_end < line_start {
            return;
        }
        let entry = self.by_file.entry(file_path.into()).or_default();
        for line in line_start..=line_end {
            entry.insert(line, attribution.clone());
        }
    }

    pub fn record_human_edit(&mut self, file_path: impl Into<String>, line_start: u32, line_end: u32) {
        self.record_range(file_path, line_start, line_end, LineAttribution::Human);
    }

    pub fn line_attribution(&self, file_path: &str, line: u32) -> LineAttribution {
        self.by_file
            .get(file_path)
            .and_then(|m| m.get(&line))
            .cloned()
            .unwrap_or(LineAttribution::Unknown)
    }
}

// ---------------------------------------------------------------------------
// Browser Design Mode Domain Contracts (F18 / F22 / F23)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignViewport {
    pub width_px: u32,
    pub height_px: u32,
    /// Device scale factor scaled by 100 (e.g., `200` == 2.0x Retina).
    pub device_scale_factor_x100: u16,
    pub is_mobile: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignPin {
    pub pin_id: String,
    /// Normalized X coordinate in basis points (`0..=10_000`).
    pub x_norm_bp: u16,
    /// Normalized Y coordinate in basis points (`0..=10_000`).
    pub y_norm_bp: u16,
    pub selector: String,
    pub note: String,
    pub resolved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesignDiffMode {
    Overlay,
    SplitSwipe,
    OnionSkin,
    SideBySide,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignSourceMapRef {
    pub file_path: String,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignCaptureContract {
    pub url: String,
    pub viewport: DesignViewport,
    pub outer_html: String,
    #[serde(default)]
    pub computed_styles: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_map: Option<DesignSourceMapRef>,
    pub captured_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignOverlaySpec {
    pub enabled: bool,
    pub diff_mode: DesignDiffMode,
    pub opacity_percent: u8,
    pub show_spacing_guides: bool,
    #[serde(default)]
    pub pins: Vec<DesignPin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignModeSpec {
    pub workspace_id: String,
    pub viewport: DesignViewport,
    pub overlay: DesignOverlaySpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_capture: Option<DesignCaptureContract>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum DesignModeError {
    #[error("viewport dimensions and scale factor must be non-zero")]
    InvalidViewport,
    #[error("overlay opacity {0} exceeds 100%")]
    InvalidOpacity(u8),
    #[error("design pin '{0}' has normalized coordinate outside 0..=10000")]
    PinOutOfBounds(String),
    #[error("design capture URL cannot be empty")]
    EmptyCaptureUrl,
}

impl DesignModeSpec {
    pub fn validate(&self) -> Result<(), DesignModeError> {
        if self.viewport.width_px == 0
            || self.viewport.height_px == 0
            || self.viewport.device_scale_factor_x100 == 0
        {
            return Err(DesignModeError::InvalidViewport);
        }
        if self.overlay.opacity_percent > 100 {
            return Err(DesignModeError::InvalidOpacity(self.overlay.opacity_percent));
        }
        for pin in &self.overlay.pins {
            if pin.x_norm_bp > 10_000 || pin.y_norm_bp > 10_000 {
                return Err(DesignModeError::PinOutOfBounds(pin.pin_id.clone()));
            }
        }
        if let Some(cap) = &self.last_capture
            && cap.url.trim().is_empty()
        {
            return Err(DesignModeError::EmptyCaptureUrl);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 4-Tier Plugin System V2 Manifest & Capability Guard
// ---------------------------------------------------------------------------

pub const PLUGIN_MANIFEST_V2_SCHEMA_VERSION: u32 = 2;
pub const PLUGIN_ACTION_FALLBACK_TIMEOUT_MS: u32 = 2_000;
pub const PLUGIN_APPROVAL_REQUIRED_EXIT_CODE: i32 = 3;
pub const PLUGIN_MAX_FILES: usize = 2_000;
pub const PLUGIN_MAX_TOTAL_BYTES: u64 = 52_428_800;

/// Exact 4-tier plugin execution taxonomy matching `PLUGIN_TIERS_4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginTier {
    Data,
    Declarative,
    SandboxedUi,
    TrustedWorker,
}

impl PluginTier {
    pub const ALL: [Self; 4] = [
        Self::Data,
        Self::Declarative,
        Self::SandboxedUi,
        Self::TrustedWorker,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Declarative => "declarative",
            Self::SandboxedUi => "sandboxed-ui",
            Self::TrustedWorker => "trusted-worker",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == raw)
    }

    pub const fn requires_approval(self) -> bool {
        !matches!(self, Self::Data)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginHookErrorPolicy {
    Pass,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHookSpec {
    pub event: String,
    pub on_error: PluginHookErrorPolicy,
    pub timeout_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifestV2 {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    pub version: String,
    pub schema_version: u32,
    pub tier: PluginTier,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub hooks: Vec<PluginHookSpec>,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum PluginValidationError {
    #[error("plugin id '{0}' is invalid or reserved")]
    ReservedOrInvalidId(String),
    #[error("plugin version cannot be empty")]
    EmptyVersion,
    #[error("unsupported plugin schema_version {0} (expected {PLUGIN_MANIFEST_V2_SCHEMA_VERSION})")]
    UnsupportedSchemaVersion(u32),
    #[error("plugin sha256 must be 64 lowercase hex characters")]
    InvalidSha256,
    #[error("plugin tier '{0}' cannot declare worker hooks")]
    HooksNotAllowedForTier(&'static str),
}

impl PluginManifestV2 {
    pub fn validate(&self) -> Result<(), PluginValidationError> {
        let id = self.id.trim();
        let lower = id.to_ascii_lowercase();
        let reserved = matches!(lower.as_str(), "constructor" | "prototype" | "__proto__")
            || lower.starts_with("orca-");
        let valid_chars = !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
        if reserved || !valid_chars {
            return Err(PluginValidationError::ReservedOrInvalidId(self.id.clone()));
        }
        if self.version.trim().is_empty() {
            return Err(PluginValidationError::EmptyVersion);
        }
        if self.schema_version != PLUGIN_MANIFEST_V2_SCHEMA_VERSION {
            return Err(PluginValidationError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(PluginValidationError::InvalidSha256);
        }
        if !self.hooks.is_empty() && self.tier != PluginTier::TrustedWorker {
            return Err(PluginValidationError::HooksNotAllowedForTier(
                self.tier.as_str(),
            ));
        }
        Ok(())
    }

    /// Computes a deterministic capability fingerprint used for `--approve <fingerprint>`.
    pub fn capability_fingerprint(&self) -> String {
        let mut h = Sha256::new();
        h.update(b"orca-plugin-v2-fingerprint\0");
        h.update(self.id.as_bytes());
        h.update([0]);
        h.update(self.tier.as_str().as_bytes());
        h.update([0]);
        h.update(self.sha256.as_bytes());
        let mut perms = self.permissions.clone();
        perms.sort();
        for p in &perms {
            h.update([1]);
            h.update(p.as_bytes());
        }
        let mut tools = self.tools.clone();
        tools.sort();
        for t in &tools {
            h.update([2]);
            h.update(t.as_bytes());
        }
        for hook in &self.hooks {
            h.update([3]);
            h.update(hook.event.as_bytes());
            h.update([match hook.on_error {
                PluginHookErrorPolicy::Pass => 0,
                PluginHookErrorPolicy::Deny => 1,
            }]);
        }
        hex::encode(h.finalize())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSystemConfig {
    pub enabled: bool,
    #[serde(default)]
    pub approved_plugin_ids: BTreeSet<String>,
    #[serde(default)]
    pub approved_fingerprints: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginExecutionDecision {
    pub allowed: bool,
    pub fingerprint: String,
    pub cli_exit_code: i32,
}

/// Evaluates plugin execution policy enforcing `PluginSystemDisabled` and
/// `ApprovalRequired` (CLI exit code 3).
pub fn evaluate_plugin_execution(
    manifest: &PluginManifestV2,
    config: &PluginSystemConfig,
) -> Result<PluginExecutionDecision, OrcaIdeError> {
    if !config.enabled {
        return Err(OrcaIdeError::new(
            OrcaIdeErrorCode::PluginSystemDisabled,
            format!("cannot run plugin '{}': plugin system is disabled", manifest.id),
        ));
    }
    let fingerprint = manifest.capability_fingerprint();
    let needs_approval = manifest.tier.requires_approval()
        || !manifest.permissions.is_empty()
        || !manifest.tools.is_empty()
        || !manifest.hooks.is_empty();

    if needs_approval {
        let id_approved = config.approved_plugin_ids.contains(&manifest.id);
        let fp_ok = match config.approved_fingerprints.get(&manifest.id) {
            Some(expected_fp) => expected_fp == &fingerprint,
            None => true,
        };
        if !id_approved || !fp_ok {
            return Err(OrcaIdeError::new(
                OrcaIdeErrorCode::ApprovalRequired,
                format!(
                    "plugin '{}' requires approval (--approve {fingerprint}, exit {PLUGIN_APPROVAL_REQUIRED_EXIT_CODE})",
                    manifest.id
                ),
            ));
        }
    }

    Ok(PluginExecutionDecision {
        allowed: true,
        fingerprint,
        cli_exit_code: 0,
    })
}

// ---------------------------------------------------------------------------
// 8-Check Background Hibernation Policy
// ---------------------------------------------------------------------------

pub const DEFAULT_HIBERNATION_IDLE_MS: u64 = 30 * 60 * 1_000;

/// All 8 invariants that MUST be clear before background hibernation may proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HibernationBlocker {
    ActivePtyProcess,
    RunningAgentTurn,
    GitOperationInProgress,
    ActivePortForward,
    ActiveReviewOrMergeGate,
    PendingBotApproval,
    UnsyncedLocalSnapshot,
    PinnedNoHibernate,
}

impl HibernationBlocker {
    pub const ALL: [Self; 8] = [
        Self::ActivePtyProcess,
        Self::RunningAgentTurn,
        Self::GitOperationInProgress,
        Self::ActivePortForward,
        Self::ActiveReviewOrMergeGate,
        Self::PendingBotApproval,
        Self::UnsyncedLocalSnapshot,
        Self::PinnedNoHibernate,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ActivePtyProcess => "active_pty_process",
            Self::RunningAgentTurn => "running_agent_turn",
            Self::GitOperationInProgress => "git_operation_in_progress",
            Self::ActivePortForward => "active_port_forward",
            Self::ActiveReviewOrMergeGate => "active_review_or_merge_gate",
            Self::PendingBotApproval => "pending_bot_approval",
            Self::UnsyncedLocalSnapshot => "unsynced_local_snapshot",
            Self::PinnedNoHibernate => "pinned_no_hibernate",
        }
    }

    pub const fn reason(self) -> &'static str {
        match self {
            Self::ActivePtyProcess => "foreground or active PTY process running",
            Self::RunningAgentTurn => "agent turn or live subagent still running",
            Self::GitOperationInProgress => "git operation (rebase/merge/checkout) in progress",
            Self::ActivePortForward => "active port forward or localhost proxy tunnel",
            Self::ActiveReviewOrMergeGate => "unsettled review/merge gate or orchestration dispatch",
            Self::PendingBotApproval => "pending bot access or tool approval",
            Self::UnsyncedLocalSnapshot => "unsynced local snapshot or dirty unpersisted state",
            Self::PinnedNoHibernate => "workspace/tab explicitly pinned against hibernation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HibernationCheckInput {
    pub active_pty_process: bool,
    pub running_agent_turn: bool,
    pub git_operation_in_progress: bool,
    pub active_port_forward: bool,
    pub active_review_or_merge_gate: bool,
    pub pending_bot_approval: bool,
    pub unsynced_local_snapshot: bool,
    pub pinned_no_hibernate: bool,
    pub idle_duration_ms: u64,
    pub min_idle_threshold_ms: u64,
}

impl Default for HibernationCheckInput {
    fn default() -> Self {
        Self {
            active_pty_process: false,
            running_agent_turn: false,
            git_operation_in_progress: false,
            active_port_forward: false,
            active_review_or_merge_gate: false,
            pending_bot_approval: false,
            unsynced_local_snapshot: false,
            pinned_no_hibernate: false,
            idle_duration_ms: DEFAULT_HIBERNATION_IDLE_MS,
            min_idle_threshold_ms: DEFAULT_HIBERNATION_IDLE_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum HibernationDecision {
    HibernateNow,
    DeferNotIdleEnough { remaining_ms: u64 },
}

/// Collects all active [`HibernationBlocker`] conditions in canonical order.
pub fn collect_hibernation_blockers(input: &HibernationCheckInput) -> Vec<HibernationBlocker> {
    let mut blockers = Vec::new();
    if input.active_pty_process {
        blockers.push(HibernationBlocker::ActivePtyProcess);
    }
    if input.running_agent_turn {
        blockers.push(HibernationBlocker::RunningAgentTurn);
    }
    if input.git_operation_in_progress {
        blockers.push(HibernationBlocker::GitOperationInProgress);
    }
    if input.active_port_forward {
        blockers.push(HibernationBlocker::ActivePortForward);
    }
    if input.active_review_or_merge_gate {
        blockers.push(HibernationBlocker::ActiveReviewOrMergeGate);
    }
    if input.pending_bot_approval {
        blockers.push(HibernationBlocker::PendingBotApproval);
    }
    if input.unsynced_local_snapshot {
        blockers.push(HibernationBlocker::UnsyncedLocalSnapshot);
    }
    if input.pinned_no_hibernate {
        blockers.push(HibernationBlocker::PinnedNoHibernate);
    }
    blockers
}

/// Evaluates all 8 hibernation safety checks. If ANY check fails, returns
/// `OrcaIdeErrorCode::HibernationBlocked` listing every active blocker code and reason.
pub fn evaluate_hibernation(
    input: &HibernationCheckInput,
) -> Result<HibernationDecision, OrcaIdeError> {
    let blockers = collect_hibernation_blockers(input);
    if !blockers.is_empty() {
        let detail = blockers
            .iter()
            .map(|b| format!("{} ({})", b.as_str(), b.reason()))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(OrcaIdeError::new(
            OrcaIdeErrorCode::HibernationBlocked,
            format!("hibernation blocked by {} check(s): {detail}", blockers.len()),
        ));
    }
    if input.idle_duration_ms < input.min_idle_threshold_ms {
        return Ok(HibernationDecision::DeferNotIdleEnough {
            remaining_ms: input.min_idle_threshold_ms - input.idle_duration_ms,
        });
    }
    Ok(HibernationDecision::HibernateNow)
}

// ---------------------------------------------------------------------------
// ComputerUse & Mobile Emulator Safety Guard
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmulatorKind {
    IosSimulator,
    AndroidEmulator,
    AndroidPhysicalDevice,
    Unsupported { name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseTargetKind {
    NormalWindow,
    PasswordField,
    SystemSecurityDialog,
    KeychainPrompt,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ComputerUseAction {
    Click {
        x_norm_bp: u16,
        y_norm_bp: u16,
        requires_foreground: bool,
    },
    TypeText {
        text_len: usize,
        via_stdin: bool,
    },
    Hotkey {
        chord: KeyChord,
    },
    AccessibilityQuery {
        query: Option<String>,
    },
    Screenshot,
    EmulatorTap {
        emulator: EmulatorKind,
        x_norm_bp: u16,
        y_norm_bp: u16,
    },
    EmulatorType {
        emulator: EmulatorKind,
        text_len: usize,
    },
    EmulatorHardwareButton {
        emulator: EmulatorKind,
        button: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUsePolicy {
    pub allow_system_shortcut: bool,
    pub allow_foreground_restore: bool,
    #[serde(default)]
    pub approved_targets: BTreeSet<String>,
    pub emulator_enabled: bool,
    pub emulator_input_allowed: bool,
    pub host_supports_ios_simulator: bool,
    pub host_supports_android_emulator: bool,
}

impl Default for ComputerUsePolicy {
    fn default() -> Self {
        Self {
            allow_system_shortcut: false,
            allow_foreground_restore: false,
            approved_targets: BTreeSet::new(),
            emulator_enabled: true,
            emulator_input_allowed: true,
            host_supports_ios_simulator: false,
            host_supports_android_emulator: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseRequest {
    pub target_id: String,
    pub target_kind: ComputerUseTargetKind,
    pub action: ComputerUseAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerUseGuardDecision {
    pub background_delivery: bool,
}

fn check_emulator_support(
    emulator: &EmulatorKind,
    policy: &ComputerUsePolicy,
) -> Result<(), OrcaIdeError> {
    if !policy.emulator_enabled {
        return Err(OrcaIdeError::new(
            OrcaIdeErrorCode::EmulatorUnsupported,
            "mobile emulator automation is disabled on this host",
        ));
    }
    match emulator {
        EmulatorKind::IosSimulator if !policy.host_supports_ios_simulator => Err(OrcaIdeError::new(
            OrcaIdeErrorCode::EmulatorUnsupported,
            "iOS Simulator is unsupported on this host platform",
        )),
        EmulatorKind::AndroidEmulator | EmulatorKind::AndroidPhysicalDevice
            if !policy.host_supports_android_emulator =>
        {
            Err(OrcaIdeError::new(
                OrcaIdeErrorCode::EmulatorUnsupported,
                "Android emulator/device bridge is unsupported on this host",
            ))
        }
        EmulatorKind::Unsupported { name } => Err(OrcaIdeError::new(
            OrcaIdeErrorCode::EmulatorUnsupported,
            format!("unsupported emulator kind '{name}'"),
        )),
        _ => Ok(()),
    }
}

/// Evaluates ComputerUse & Mobile Emulator safety policy, enforcing:
/// - `EmulatorUnsupported` for unsupported/disabled emulators,
/// - `EmulatorInputBlocked` for password fields, system security/keychain prompts,
///   disabled emulator input, or out-of-range normalized coordinates,
/// - `SystemShortcutBlocked` for OS-reserved hotkeys (`Cmd+Q`, `Alt+F4`, `Win+L`, etc.)
///   unless explicitly allowed,
/// - `ApprovalRequired` for unapproved targets or foreground focus-stealing clicks.
pub fn evaluate_computer_use_guard(
    req: &ComputerUseRequest,
    policy: &ComputerUsePolicy,
) -> Result<ComputerUseGuardDecision, OrcaIdeError> {
    // 1. Sensitive security UI / password targets are ALWAYS blocked.
    if matches!(
        req.target_kind,
        ComputerUseTargetKind::PasswordField
            | ComputerUseTargetKind::SystemSecurityDialog
            | ComputerUseTargetKind::KeychainPrompt
    ) {
        return Err(OrcaIdeError::new(
            OrcaIdeErrorCode::EmulatorInputBlocked,
            format!(
                "input/capture blocked on protected target kind {:?}",
                req.target_kind
            ),
        ));
    }

    // 2. Action-specific capability & safety checks.
    let (is_mutating, background_delivery) = match &req.action {
        ComputerUseAction::EmulatorTap {
            emulator,
            x_norm_bp,
            y_norm_bp,
        } => {
            check_emulator_support(emulator, policy)?;
            if !policy.emulator_input_allowed || *x_norm_bp > 10_000 || *y_norm_bp > 10_000 {
                return Err(OrcaIdeError::new(
                    OrcaIdeErrorCode::EmulatorInputBlocked,
                    "emulator tap blocked by policy or out-of-range normalized coordinates",
                ));
            }
            (true, true)
        }
        ComputerUseAction::EmulatorType { emulator, .. }
        | ComputerUseAction::EmulatorHardwareButton { emulator, .. } => {
            check_emulator_support(emulator, policy)?;
            if !policy.emulator_input_allowed {
                return Err(OrcaIdeError::new(
                    OrcaIdeErrorCode::EmulatorInputBlocked,
                    "emulator input is disabled by policy",
                ));
            }
            (true, true)
        }
        ComputerUseAction::Hotkey { chord } => {
            if is_system_shortcut_blocked(chord) && !policy.allow_system_shortcut {
                return Err(OrcaIdeError::new(
                    OrcaIdeErrorCode::SystemShortcutBlocked,
                    format!(
                        "system shortcut '{}' blocked unless --allow-system-shortcut is approved",
                        chord.canonical_string()
                    ),
                ));
            }
            (true, true)
        }
        ComputerUseAction::Click {
            x_norm_bp,
            y_norm_bp,
            requires_foreground,
        } => {
            if *x_norm_bp > 10_000 || *y_norm_bp > 10_000 {
                return Err(OrcaIdeError::new(
                    OrcaIdeErrorCode::EmulatorInputBlocked,
                    "click coordinates outside normalized 0..=10000 range",
                ));
            }
            if *requires_foreground && !policy.allow_foreground_restore {
                return Err(OrcaIdeError::new(
                    OrcaIdeErrorCode::ApprovalRequired,
                    "foreground pointer action requires explicit --restore-window approval",
                ));
            }
            (true, !*requires_foreground)
        }
        ComputerUseAction::TypeText { .. } => (true, true),
        ComputerUseAction::AccessibilityQuery { .. } | ComputerUseAction::Screenshot => {
            (false, true)
        }
    };

    // 3. Target approval enforcement for mutating actions (and unknown targets).
    let approved =
        !req.target_id.trim().is_empty() && policy.approved_targets.contains(&req.target_id);
    if (is_mutating || req.target_kind == ComputerUseTargetKind::Unknown) && !approved {
        return Err(OrcaIdeError::new(
            OrcaIdeErrorCode::ApprovalRequired,
            format!("computer use target '{}' requires explicit approval", req.target_id),
        ));
    }

    Ok(ComputerUseGuardDecision {
        background_delivery,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_annotations_line_shift_batch_prompt_and_ai_attribution() {
        let mut store = DiffAnnotationStore::default();
        store
            .add(DiffAnnotation {
                annotation_id: "ann_1".to_string(),
                file_path: "src/main.rs".to_string(),
                side: DiffSide::Right,
                line_start: 10,
                line_end: 12,
                body: "Avoid unwrap here".to_string(),
                author: LineAttribution::Human,
                resolved: false,
                created_at_ms: 100,
                updated_at_ms: 100,
            })
            .expect("add ann_1");
        store
            .add(DiffAnnotation {
                annotation_id: "ann_2".to_string(),
                file_path: "src/main.rs".to_string(),
                side: DiffSide::Right,
                line_start: 30,
                line_end: 30,
                body: "Use saturating_add".to_string(),
                author: LineAttribution::Bot {
                    bot_id: "scout".to_string(),
                },
                resolved: false,
                created_at_ms: 200,
                updated_at_ms: 200,
            })
            .expect("add ann_2");

        // Line shift at line 15 (+5) moves ann_2 (30 -> 35) without touching ann_1 (10..=12)
        store.apply_line_shift("src/main.rs", DiffSide::Right, 15, 5);
        assert_eq!(
            store.filter_by_line("src/main.rs", DiffSide::Right, 11, false).len(),
            1
        );
        assert_eq!(
            store.filter_by_line("src/main.rs", DiffSide::Right, 35, false).len(),
            1
        );

        let batch = store.build_agent_batch_prompt(Some("src/main.rs"));
        assert!(batch.contains("src/main.rs:L10-L12: Avoid unwrap here"));
        assert!(batch.contains("src/main.rs:L35: Use saturating_add"));

        store.resolve("ann_1", 300).expect("resolve ann_1");
        assert_eq!(store.filter_by_file("src/main.rs", false).len(), 1);
        assert_eq!(store.filter_by_file("src/main.rs", true).len(), 2);

        // AI attribution flips back to Human on human edit
        let mut attr = FileAttributionMap::default();
        attr.record_range(
            "src/main.rs",
            10,
            15,
            LineAttribution::Worker {
                dispatch_id: "d1".to_string(),
            },
        );
        attr.record_human_edit("src/main.rs", 12, 13);
        assert_eq!(attr.line_attribution("src/main.rs", 12), LineAttribution::Human);
        assert_eq!(
            attr.line_attribution("src/main.rs", 14),
            LineAttribution::Worker {
                dispatch_id: "d1".to_string()
            }
        );
    }

    #[test]
    fn design_mode_and_plugin_v2_4_tiers_and_approval_guards() {
        let mut dm = DesignModeSpec {
            workspace_id: "ws_1".to_string(),
            viewport: DesignViewport {
                width_px: 1440,
                height_px: 900,
                device_scale_factor_x100: 200,
                is_mobile: false,
            },
            overlay: DesignOverlaySpec {
                enabled: true,
                diff_mode: DesignDiffMode::OnionSkin,
                opacity_percent: 60,
                show_spacing_guides: true,
                pins: vec![DesignPin {
                    pin_id: "p1".to_string(),
                    x_norm_bp: 5_000,
                    y_norm_bp: 2_500,
                    selector: "button.primary".to_string(),
                    note: "Increase padding".to_string(),
                    resolved: false,
                }],
            },
            last_capture: Some(DesignCaptureContract {
                url: "http://feat.orca.localhost:3000".to_string(),
                viewport: DesignViewport {
                    width_px: 1440,
                    height_px: 900,
                    device_scale_factor_x100: 200,
                    is_mobile: false,
                },
                outer_html: "<button class=\"primary\">Go</button>".to_string(),
                computed_styles: BTreeMap::from([("color".to_string(), "rgb(0, 0, 0)".to_string())]),
                screenshot_sha256: Some("a".repeat(64)),
                source_map: Some(DesignSourceMapRef {
                    file_path: "src/Button.tsx".to_string(),
                    line: 42,
                    column: Some(8),
                    component_name: Some("PrimaryButton".to_string()),
                }),
                captured_at_ms: 1_000,
            }),
        };
        assert!(dm.validate().is_ok());
        dm.overlay.opacity_percent = 101;
        assert_eq!(dm.validate(), Err(DesignModeError::InvalidOpacity(101)));

        // Exact 4 plugin tiers matching PLUGIN_TIERS_4
        let expected_tiers = ["data", "declarative", "sandboxed-ui", "trusted-worker"];
        assert_eq!(PluginTier::ALL.len(), 4);
        for (t, wire) in PluginTier::ALL.into_iter().zip(expected_tiers) {
            assert_eq!(t.as_str(), wire);
            assert_eq!(PluginTier::parse(wire), Some(t));
        }

        let manifest = PluginManifestV2 {
            id: "acme.theme-helper".to_string(),
            publisher: Some("acme".to_string()),
            version: "1.0.0".to_string(),
            schema_version: PLUGIN_MANIFEST_V2_SCHEMA_VERSION,
            tier: PluginTier::TrustedWorker,
            permissions: vec!["workspace.read".to_string()],
            tools: vec!["lint-check".to_string()],
            hooks: vec![PluginHookSpec {
                event: "pre_agent_launch".to_string(),
                on_error: PluginHookErrorPolicy::Deny,
                timeout_ms: PLUGIN_ACTION_FALLBACK_TIMEOUT_MS,
            }],
            sha256: "b".repeat(64),
        };
        assert!(manifest.validate().is_ok());

        let mut cfg = PluginSystemConfig::default();
        assert_eq!(
            evaluate_plugin_execution(&manifest, &cfg).expect_err("disabled").code,
            OrcaIdeErrorCode::PluginSystemDisabled
        );

        cfg.enabled = true;
        let err = evaluate_plugin_execution(&manifest, &cfg).expect_err("unapproved");
        assert_eq!(err.code, OrcaIdeErrorCode::ApprovalRequired);
        assert!(err.message.contains("exit 3"));

        cfg.approved_plugin_ids.insert(manifest.id.clone());
        cfg.approved_fingerprints
            .insert(manifest.id.clone(), manifest.capability_fingerprint());
        let decision = evaluate_plugin_execution(&manifest, &cfg).expect("approved");
        assert!(decision.allowed);
        assert_eq!(decision.cli_exit_code, 0);

        // Capability escalation invalidates fingerprint approval
        let mut escalated = manifest.clone();
        escalated.permissions.push("terminal.write".to_string());
        assert_eq!(
            evaluate_plugin_execution(&escalated, &cfg)
                .expect_err("fingerprint mismatch")
                .code,
            OrcaIdeErrorCode::ApprovalRequired
        );
    }

    #[test]
    fn all_8_hibernation_blockers_and_computer_use_guards() {
        assert_eq!(HibernationBlocker::ALL.len(), 8);

        // Clean default input hibernates cleanly
        assert_eq!(
            evaluate_hibernation(&HibernationCheckInput::default()).expect("hibernate"),
            HibernationDecision::HibernateNow
        );

        // Below idle threshold defers without blocking
        let not_idle = HibernationCheckInput {
            idle_duration_ms: 10_000,
            min_idle_threshold_ms: 60_000,
            ..Default::default()
        };
        assert_eq!(
            evaluate_hibernation(&not_idle).expect("defer"),
            HibernationDecision::DeferNotIdleEnough {
                remaining_ms: 50_000
            }
        );

        // Every single one of the 8 checks individually blocks with HibernationBlocked
        let cases: Vec<(HibernationCheckInput, HibernationBlocker)> = vec![
            (HibernationCheckInput { active_pty_process: true, ..Default::default() }, HibernationBlocker::ActivePtyProcess),
            (HibernationCheckInput { running_agent_turn: true, ..Default::default() }, HibernationBlocker::RunningAgentTurn),
            (HibernationCheckInput { git_operation_in_progress: true, ..Default::default() }, HibernationBlocker::GitOperationInProgress),
            (HibernationCheckInput { active_port_forward: true, ..Default::default() }, HibernationBlocker::ActivePortForward),
            (HibernationCheckInput { active_review_or_merge_gate: true, ..Default::default() }, HibernationBlocker::ActiveReviewOrMergeGate),
            (HibernationCheckInput { pending_bot_approval: true, ..Default::default() }, HibernationBlocker::PendingBotApproval),
            (HibernationCheckInput { unsynced_local_snapshot: true, ..Default::default() }, HibernationBlocker::UnsyncedLocalSnapshot),
            (HibernationCheckInput { pinned_no_hibernate: true, ..Default::default() }, HibernationBlocker::PinnedNoHibernate),
        ];
        for (input, expected_blocker) in cases {
            assert_eq!(collect_hibernation_blockers(&input), vec![expected_blocker]);
            let err = evaluate_hibernation(&input).expect_err("blocked");
            assert_eq!(err.code, OrcaIdeErrorCode::HibernationBlocked);
            assert!(err.message.contains(expected_blocker.as_str()));
        }

        // ComputerUse / Emulator Guard tests
        let mut policy = ComputerUsePolicy::default();
        policy.approved_targets.insert("app:browser".to_string());

        // Unsupported iOS Simulator on Linux host
        let ios_req = ComputerUseRequest {
            target_id: "app:browser".to_string(),
            target_kind: ComputerUseTargetKind::NormalWindow,
            action: ComputerUseAction::EmulatorTap {
                emulator: EmulatorKind::IosSimulator,
                x_norm_bp: 5_000,
                y_norm_bp: 5_000,
            },
        };
        assert_eq!(
            evaluate_computer_use_guard(&ios_req, &policy).expect_err("ios unsupported").code,
            OrcaIdeErrorCode::EmulatorUnsupported
        );

        // Password field blocked with EmulatorInputBlocked
        let pwd_req = ComputerUseRequest {
            target_id: "app:browser".to_string(),
            target_kind: ComputerUseTargetKind::PasswordField,
            action: ComputerUseAction::TypeText {
                text_len: 12,
                via_stdin: true,
            },
        };
        assert_eq!(
            evaluate_computer_use_guard(&pwd_req, &policy).expect_err("password blocked").code,
            OrcaIdeErrorCode::EmulatorInputBlocked
        );

        // System shortcut Cmd+Q blocked with SystemShortcutBlocked
        let quit_req = ComputerUseRequest {
            target_id: "app:browser".to_string(),
            target_kind: ComputerUseTargetKind::NormalWindow,
            action: ComputerUseAction::Hotkey {
                chord: KeyChord::parse("Cmd+Q").expect("parse"),
            },
        };
        assert_eq!(
            evaluate_computer_use_guard(&quit_req, &policy).expect_err("cmd+q blocked").code,
            OrcaIdeErrorCode::SystemShortcutBlocked
        );

        // Unapproved target requires approval
        let unapproved_req = ComputerUseRequest {
            target_id: "app:unknown".to_string(),
            target_kind: ComputerUseTargetKind::NormalWindow,
            action: ComputerUseAction::Click {
                x_norm_bp: 1_000,
                y_norm_bp: 2_000,
                requires_foreground: false,
            },
        };
        assert_eq!(
            evaluate_computer_use_guard(&unapproved_req, &policy).expect_err("approval required").code,
            OrcaIdeErrorCode::ApprovalRequired
        );

        // Approved background click succeeds
        let ok_req = ComputerUseRequest {
            target_id: "app:browser".to_string(),
            target_kind: ComputerUseTargetKind::NormalWindow,
            action: ComputerUseAction::Click {
                x_norm_bp: 1_000,
                y_norm_bp: 2_000,
                requires_foreground: false,
            },
        };
        assert_eq!(
            evaluate_computer_use_guard(&ok_req, &policy).expect("allowed"),
            ComputerUseGuardDecision {
                background_delivery: true
            }
        );
    }
}
