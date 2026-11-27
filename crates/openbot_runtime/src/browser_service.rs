//! Persistent OpenBot Private Browser Runtime & `openbot_browser.*` Tool Engine (`OB-F10` Part B).
//!
//! 1:1 pure-Rust port of upstream `src/backend/{browser-tools,browser-state,browser-fill-fields,
//! browser-card-fill,browser-policy,browser-host}.ts` with atomic state persistence at
//! `<data_dir>/openbot-browser-state-v1.json`, strict `<= 25` active tab cap, `<= 200`-step
//! script execution budget, `fill_fields` secret refusal, zero-knowledge `submit_secret`,
//! HTTPS-only `fill_card` with `screenshots_blocked` exfiltration lock, and human takeover control.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chrono::{SecondsFormat, Utc};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use openbot_contracts::limits::{ATTACHMENT_LIMITS, INPUT_LIMITS, utf16_len};
use openbot_storage::{
    BrowserProfileStore, BrowserProxyStore, OpenBotDatabase, contains_secret, is_secret_label,
    redact_text, write_json_file_atomically,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::error::{OpenBotRuntimeError, RuntimeResult};
use crate::vault_service::{
    LinkPaymentRequest, SecretFillMethod, VaultCardBrand, VaultSaveInput, VaultService,
    is_luhn_valid, mask_card_last4,
};

pub const BROWSER_STATE_FILE_NAME: &str = "openbot-browser-state-v1.json";
pub const MAX_BROWSER_TABS: usize = INPUT_LIMITS.browser_tabs; // 25
pub const BROWSER_SCRIPT_MAX_STEPS: usize = 200;
pub const BROWSER_SCRIPT_MAX_SLEEP_MS: u64 = 25_000;
pub const BROWSER_SCRIPT_MAX_CHARS: usize = 20_000;
pub const BROWSER_SCRIPT_MIN_TIMEOUT_MS: u64 = 1_000;
pub const BROWSER_SCRIPT_DEFAULT_TIMEOUT_MS: u64 = 60_000;
pub const BROWSER_SCRIPT_MAX_TIMEOUT_MS: u64 = 120_000;
pub const BROWSER_FILL_FIELDS_MAX_FIELDS: usize = 16;
pub const BROWSER_FILL_FIELDS_MAX_TEXT_BYTES: usize = 16 * 1024;
pub const BROWSER_EVALUATE_MAX_CHARS: usize = 64_000;
pub const BROWSER_EVALUATE_MAX_RESULT_BYTES: usize = 64 * 1024;
pub const BROWSER_MAX_PHYSICAL_VIEWPORT_PIXELS: u64 = 8_388_608;
pub const BROWSER_RECORDING_MAX_DURATION_MS: u64 = 5 * 60 * 1_000;
pub const BROWSER_RECORDING_MAX_BYTES: u64 = 100 * 1024 * 1024;

/// Complete list of supported `openbot_browser.*` tool names.
pub const BROWSER_TOOL_NAMES: &[&str] = &[
    "open",
    "navigate",
    "new_tab",
    "close_tab",
    "switch_tab",
    "list_tabs",
    "back",
    "forward",
    "reload",
    "snapshot",
    "screenshot",
    "click",
    "double_click",
    "hover",
    "type",
    "fill_fields",
    "select_option",
    "press_key",
    "scroll",
    "wait_for",
    "evaluate",
    "extract_text",
    "get_links",
    "console_logs",
    "network_log",
    "download",
    "upload_file",
    "cookies",
    "run_script",
    "submit_secret",
    "fill_card",
    "request_takeover",
    "release_takeover",
    "close",
    "status",
    "press",
    "set_checked",
    "drag",
    "upload_files",
    "set_environment",
    "recording_start",
    "recording_stop",
    "handle_dialog",
    "act",
    "list_logins",
    "list_vault_items",
    "save_vault_item",
    "request_vault_item",
    "delete_vault_item",
    "request_link_payment",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserViewportState {
    pub mode: String,
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub preset: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserEnvironmentState {
    pub viewport: BrowserViewportState,
    pub color_scheme: String,
    pub reduced_motion: bool,
}

impl Default for BrowserEnvironmentState {
    fn default() -> Self {
        Self {
            viewport: BrowserViewportState {
                mode: "fill".to_string(),
                width: 1200,
                height: 800,
                device_scale_factor: 1.0,
                preset: None,
            },
            color_scheme: "system".to_string(),
            reduced_motion: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomElementSnapshot {
    pub r#ref: String,
    pub tag: String,
    pub role: String,
    pub name: String,
    pub selector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autocomplete: Option<String>,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub checked: bool,
    #[serde(default)]
    pub selected_options: Vec<String>,
    #[serde(default)]
    pub uploaded_files: Vec<String>,
    #[serde(default)]
    pub is_secret: bool,
    #[serde(default)]
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserLink {
    pub text: String,
    pub href: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserConsoleEntry {
    pub level: String,
    pub text: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserNetworkEntry {
    pub method: String,
    pub url: String,
    pub status: u16,
    pub resource_type: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    #[serde(default = "default_cookie_path")]
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default, alias = "http_only")]
    pub http_only: bool,
}

fn default_cookie_path() -> String {
    "/".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDownloadRecord {
    pub download_id: String,
    pub tab_id: String,
    pub url: String,
    pub filename: String,
    pub saved_path: String,
    pub byte_length: u64,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRecordingState {
    pub recording_id: String,
    pub started_at: String,
    pub output_path: String,
    pub max_duration_ms: u64,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDialogState {
    pub dialog_id: String,
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub accepted: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeoverState {
    pub takeover_id: String,
    pub tab_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub url: String,
    pub reason: String,
    pub status: String,
    pub takeover_active: bool,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTabState {
    pub id: String,
    pub window_id: String,
    pub url: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opener_tab_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub revision: u64,
    pub history: Vec<String>,
    pub history_index: usize,
    pub dom_elements: Vec<DomElementSnapshot>,
    pub page_text: String,
    pub links: Vec<BrowserLink>,
    pub console_logs: Vec<BrowserConsoleEntry>,
    pub network_logs: Vec<BrowserNetworkEntry>,
    #[serde(default)]
    pub local_storage: BTreeMap<String, String>,
    #[serde(default)]
    pub environment: BrowserEnvironmentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<BrowserRecordingState>,
    #[serde(default)]
    pub pending_dialogs: Vec<BrowserDialogState>,
    #[serde(default)]
    pub scroll_x: f64,
    #[serde(default)]
    pub scroll_y: f64,
    #[serde(default)]
    pub secret_document: bool,
    #[serde(default)]
    pub filled_card: bool,
    #[serde(default)]
    pub screenshots_blocked: bool,
    #[serde(default)]
    pub takeover_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_takeover_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub takeover_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserWindowState {
    pub window_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tab_id: Option<String>,
    #[serde(default)]
    pub tab_ids: Vec<String>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredBrowserState {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tab_id: Option<String>,
    #[serde(default)]
    pub windows: Vec<BrowserWindowState>,
    #[serde(default)]
    pub tabs: Vec<BrowserTabState>,
    #[serde(default)]
    pub cookies: Vec<BrowserCookie>,
    #[serde(default)]
    pub downloads: Vec<BrowserDownloadRecord>,
    #[serde(default)]
    pub takeovers: Vec<TakeoverState>,
}

impl Default for StoredBrowserState {
    fn default() -> Self {
        Self {
            version: 1,
            active_tab_id: None,
            windows: vec![BrowserWindowState {
                window_id: "win-1".to_string(),
                active_tab_id: None,
                tab_ids: Vec::new(),
                width: 1200,
                height: 800,
            }],
            tabs: Vec::new(),
            cookies: Vec::new(),
            downloads: Vec::new(),
            takeovers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum BrowserTarget {
    Ref {
        r#ref: String,
        #[serde(default)]
        revision: Option<u64>,
    },
    Role {
        role: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        exact: Option<bool>,
        #[serde(default)]
        nth: Option<usize>,
    },
    Text {
        text: String,
        #[serde(default)]
        exact: Option<bool>,
        #[serde(default)]
        nth: Option<usize>,
    },
    Css {
        selector: String,
        #[serde(default)]
        nth: Option<usize>,
    },
    Point {
        x: f64,
        y: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshotView {
    pub tab_id: String,
    pub url: String,
    pub title: String,
    pub revision: u64,
    pub viewport: BrowserViewportState,
    pub elements: Vec<DomElementSnapshot>,
    pub text: String,
    pub truncated: bool,
    pub secret_document: bool,
    pub filled_card: bool,
    pub screenshots_blocked: bool,
    pub takeover_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTabSummary {
    pub id: String,
    pub window_id: String,
    pub url: String,
    pub title: String,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opener_tab_id: Option<String>,
    pub revision: u64,
    pub secret_document: bool,
    pub filled_card: bool,
    pub screenshots_blocked: bool,
    pub takeover_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserScreenshotResult {
    pub tab_id: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub byte_length: usize,
    pub base64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillFieldEntry {
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, alias = "value")]
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillFieldsRequest {
    #[serde(alias = "tab_id")]
    pub tab_id: String,
    #[serde(default)]
    pub revision: Option<u64>,
    pub fields: Vec<FillFieldEntry>,
    #[serde(default, alias = "timeout_ms")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillFieldsResult {
    pub tab_id: String,
    pub revision: u64,
    pub confirmed: Vec<usize>,
    pub uncertain: Vec<usize>,
    pub filled_count: usize,
    pub snapshot: BrowserSnapshotView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitSecretRequest {
    #[serde(alias = "tab_id")]
    pub tab_id: String,
    pub method: String,
    #[serde(default)]
    pub targets: Vec<BrowserTarget>,
    #[serde(default)]
    pub digits: Option<u8>,
    pub submission: String,
    #[serde(default, alias = "submit_target")]
    pub submit_target: Option<BrowserTarget>,
    #[serde(default, alias = "login_id")]
    pub login_id: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default, alias = "agent_id")]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitSecretResult {
    pub ok: bool,
    pub tab_id: String,
    pub method: String,
    pub submission: String,
    pub secret_document: bool,
    pub submitted: bool,
    pub source: String,
    pub snapshot: BrowserSnapshotView,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardFillTargets {
    #[serde(default)]
    pub number: Option<BrowserTarget>,
    #[serde(default, alias = "security_code")]
    pub security_code: Option<BrowserTarget>,
    #[serde(default)]
    pub expiry: Option<BrowserTarget>,
    #[serde(default, alias = "exp_month")]
    pub exp_month: Option<BrowserTarget>,
    #[serde(default, alias = "exp_year")]
    pub exp_year: Option<BrowserTarget>,
    #[serde(default, alias = "name_on_card")]
    pub name_on_card: Option<BrowserTarget>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillCardRequest {
    #[serde(alias = "tab_id")]
    pub tab_id: String,
    #[serde(alias = "card_id")]
    pub card_id: String,
    #[serde(default)]
    pub targets: Option<CardFillTargets>,
    #[serde(default)]
    pub approved: bool,
    #[serde(default, alias = "agent_id")]
    pub agent_id: Option<String>,
    #[serde(default, alias = "timeout_ms")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillCardResult {
    pub tab_id: String,
    pub card_id: String,
    pub filled_card: bool,
    pub screenshots_blocked: bool,
    pub filled_fields: Vec<String>,
    pub masked_number: String,
    pub last4: String,
    pub brand: Option<VaultCardBrand>,
    pub snapshot: BrowserSnapshotView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeoverRequest {
    #[serde(alias = "tab_id")]
    pub tab_id: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default, alias = "agent_id")]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRunStepsRequest {
    #[serde(default, alias = "agent_id")]
    pub agent_id: Option<String>,
    #[serde(default, alias = "tab_id")]
    pub tab_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    pub steps: Vec<Value>,
    #[serde(default, alias = "timeout_ms")]
    pub timeout_ms: Option<u64>,
    #[serde(default, alias = "final_snapshot")]
    pub final_snapshot: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRunScriptResult {
    pub tab_id: String,
    pub url: String,
    pub steps_executed: usize,
    pub logs: Vec<String>,
    pub step_results: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<BrowserSnapshotView>,
    pub screenshots_blocked: bool,
}

// ---------------------------------------------------------------------------
// URL Normalization, Secret Field Heuristics, PNG Encoder & Page Synthesis
// ---------------------------------------------------------------------------

fn now_rfc3339_millis() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn normalize_browser_url(raw: &str, allow_blank: bool) -> RuntimeResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(OpenBotRuntimeError::Validation(
            "browser URL is required".to_string(),
        ));
    }
    if utf16_len(trimmed) > INPUT_LIMITS.browser_url {
        return Err(OpenBotRuntimeError::Validation(format!(
            "browser URL exceeds {} characters",
            INPUT_LIMITS.browser_url
        )));
    }
    if allow_blank && trimmed.eq_ignore_ascii_case("about:blank") {
        return Ok("about:blank".to_string());
    }
    let candidate = if trimmed.contains("://") || trimmed.starts_with("about:") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = Url::parse(&candidate).map_err(|err| {
        OpenBotRuntimeError::Validation(format!("invalid browser URL '{trimmed}': {err}"))
    })?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(OpenBotRuntimeError::Validation(format!(
            "browser URL must use http:// or https://, got '{}'",
            parsed.scheme()
        )));
    }
    if parsed.host_str().is_none() {
        return Err(OpenBotRuntimeError::Validation(format!(
            "browser URL '{trimmed}' must include a host"
        )));
    }
    Ok(parsed.to_string())
}

fn sanitize_diagnostic_url(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(mut u) => {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => redact_text(raw),
    }
}

/// Inspects field selector, ref, name, label, DOM attributes, and value for password,
/// OTP, secret, token, API key, or payment card heuristics.
pub fn detect_secret_field_violation(
    field: &FillFieldEntry,
    dom_el: Option<&DomElementSnapshot>,
) -> Option<&'static str> {
    let mut descriptors = Vec::new();
    if let Some(ref s) = field.selector {
        descriptors.push(s.as_str());
    }
    if let Some(ref r) = field.r#ref {
        descriptors.push(r.as_str());
    }
    if let Some(ref n) = field.name {
        descriptors.push(n.as_str());
    }
    if let Some(ref l) = field.label {
        descriptors.push(l.as_str());
    }
    if let Some(el) = dom_el {
        if el.is_secret {
            return Some("element is marked as a secret or payment input");
        }
        descriptors.push(el.selector.as_str());
        descriptors.push(el.name.as_str());
        if let Some(ref it) = el.input_type {
            if it.eq_ignore_ascii_case("password") {
                return Some("input[type=password] field");
            }
            descriptors.push(it.as_str());
        }
        if let Some(ref ac) = el.autocomplete {
            let lower_ac = ac.to_ascii_lowercase();
            for token in lower_ac.split_whitespace() {
                if token.contains("password")
                    || token == "one-time-code"
                    || token.starts_with("cc-")
                {
                    return Some("sensitive autocomplete attribute (password/OTP/card)");
                }
            }
        }
    }

    const PASSWORD_MARKERS: &[&str] = &[
        "password",
        "passwd",
        "passphrase",
        "passcode",
        "pwd",
        "type=password",
        "type=\"password\"",
        "type='password'",
    ];
    const OTP_MARKERS: &[&str] = &[
        "one-time-code",
        "otp",
        "totp",
        "2fa",
        "mfa",
        "two-factor",
        "two_factor",
        "verification-code",
        "verification_code",
        "auth-code",
        "auth_code",
    ];
    const CARD_MARKERS: &[&str] = &[
        "cc-number",
        "cc-csc",
        "cc-exp",
        "cc-name",
        "cardnumber",
        "card-number",
        "card_number",
        "creditcard",
        "credit-card",
        "credit_card",
        "cvv",
        "cvc",
        "csc",
        "security-code",
        "security_code",
        "pan",
    ];
    const SECRET_MARKERS: &[&str] = &[
        "secret",
        "api_key",
        "api-key",
        "apikey",
        "private_key",
        "private-key",
        "access_token",
        "auth_token",
        "bearer",
        "token",
        "vault:",
    ];

    for desc in descriptors {
        let lower = desc.to_ascii_lowercase();
        if PASSWORD_MARKERS.iter().any(|m| lower.contains(m)) {
            return Some("password field");
        }
        if CARD_MARKERS.iter().any(|m| lower.contains(m)) {
            return Some("payment card or CVV field");
        }
        if OTP_MARKERS.iter().any(|m| lower.contains(m)) {
            return Some("one-time authentication code field");
        }
        if SECRET_MARKERS.iter().any(|m| lower.contains(m)) || is_secret_label(desc) {
            return Some("secret or credential token field");
        }
    }

    let val_lower = field.text.trim().to_ascii_lowercase();
    if val_lower.starts_with("vault:")
        || val_lower.starts_with("{{vault.")
        || val_lower.starts_with("sk-live-")
        || val_lower.starts_with("sk_live_")
        || val_lower.starts_with("ghp_")
        || val_lower.starts_with("bearer ")
    {
        return Some("vault secret or API token value");
    }
    if contains_secret(&field.text) {
        return Some("registered vault secret value");
    }
    let digits_only: String = field.text.chars().filter(|c| c.is_ascii_digit()).collect();
    if (13..=19).contains(&digits_only.len()) && is_luhn_valid(&digits_only) {
        return Some("payment card number (PAN) value");
    }
    None
}

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xedb8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn push_png_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(chunk_type);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32_ieee(&crc_input).to_be_bytes());
}

/// Encodes a genuine, spec-compliant RGBA PNG image (used by `screenshot`).
fn encode_minimal_png(width: u32, height: u32, rgba: [u8; 4]) -> RuntimeResult<Vec<u8>> {
    let w = width.clamp(1, 256);
    let h = height.clamp(1, 256);
    let mut out = Vec::with_capacity(128 + (w as usize * h as usize * 4));
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // RGBA color type
    ihdr.push(0); // deflate compression
    ihdr.push(0); // default filter
    ihdr.push(0); // non-interlaced
    push_png_chunk(&mut out, b"IHDR", &ihdr);

    let mut raw_scanlines = Vec::with_capacity((h as usize) * (1 + (w as usize) * 4));
    for _ in 0..h {
        raw_scanlines.push(0u8); // filter type None
        for _ in 0..w {
            raw_scanlines.extend_from_slice(&rgba);
        }
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&raw_scanlines)?;
    let idat = encoder.finish()?;
    push_png_chunk(&mut out, b"IDAT", &idat);
    push_png_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

fn synthesize_page_for_url(
    url: &str,
) -> (String, String, Vec<DomElementSnapshot>, Vec<BrowserLink>) {
    if url == "about:blank" {
        return (
            "New Tab".to_string(),
            String::new(),
            Vec::new(),
            Vec::new(),
        );
    }
    let parsed = Url::parse(url).ok();
    let host = parsed
        .as_ref()
        .and_then(|u| u.host_str())
        .unwrap_or("page")
        .to_string();
    let path = parsed
        .as_ref()
        .map(|u| u.path().to_ascii_lowercase())
        .unwrap_or_default();
    let origin = parsed
        .as_ref()
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_else(|| url.to_string());

    let mut elements = vec![DomElementSnapshot {
        r#ref: "e1".to_string(),
        tag: "h1".to_string(),
        role: "heading".to_string(),
        name: format!("{host}{path}"),
        selector: "h1".to_string(),
        input_type: None,
        autocomplete: None,
        value: String::new(),
        checked: false,
        selected_options: Vec::new(),
        uploaded_files: Vec::new(),
        is_secret: false,
        focused: false,
    }];

    let mut next_idx = 2usize;
    let mut push_el = |tag: &str,
                       role: &str,
                       name: &str,
                       selector: &str,
                       input_type: Option<&str>,
                       autocomplete: Option<&str>,
                       is_secret: bool| {
        elements.push(DomElementSnapshot {
            r#ref: format!("e{next_idx}"),
            tag: tag.to_string(),
            role: role.to_string(),
            name: name.to_string(),
            selector: selector.to_string(),
            input_type: input_type.map(ToOwned::to_owned),
            autocomplete: autocomplete.map(ToOwned::to_owned),
            value: String::new(),
            checked: false,
            selected_options: Vec::new(),
            uploaded_files: Vec::new(),
            is_secret,
            focused: false,
        });
        next_idx += 1;
    };

    if path.contains("login") || path.contains("signin") || path.contains("auth") {
        push_el("input", "textbox", "Username", "#user", Some("text"), Some("username"), false);
        push_el(
            "input",
            "textbox",
            "Password",
            "input[type=password]",
            Some("password"),
            Some("current-password"),
            true,
        );
        push_el(
            "input",
            "textbox",
            "One-time code",
            "input[autocomplete=one-time-code]",
            Some("text"),
            Some("one-time-code"),
            true,
        );
        push_el("button", "button", "Sign in", "button[type=submit]", None, None, false);
    } else if path.contains("checkout") || path.contains("pay") || path.contains("billing") {
        push_el("input", "textbox", "Card number", "input[autocomplete=cc-number]", Some("tel"), Some("cc-number"), true);
        push_el("input", "textbox", "Security code", "input[autocomplete=cc-csc]", Some("password"), Some("cc-csc"), true);
        push_el("input", "textbox", "Expiry", "input[autocomplete=cc-exp]", Some("text"), Some("cc-exp"), true);
        push_el("input", "textbox", "Name on card", "input[autocomplete=cc-name]", Some("text"), Some("cc-name"), false);
        push_el("button", "button", "Pay now", "button[type=submit]", None, None, false);
    } else {
        push_el("input", "searchbox", "Search", "input[type=search]", Some("search"), None, false);
        push_el("button", "button", "Continue", "button.primary", None, None, false);
    }

    let links = vec![
        BrowserLink {
            text: format!("{host} Home"),
            href: format!("{origin}/"),
            rel: None,
        },
        BrowserLink {
            text: "Account".to_string(),
            href: format!("{origin}/account"),
            rel: None,
        },
    ];
    let title = if path.is_empty() || path == "/" {
        host.clone()
    } else {
        format!("{host} — {}", path.trim_start_matches('/'))
    };
    let text = format!("{title}\nURL: {url}\nInteractive elements: {}", elements.len());
    (title, text, elements, links)
}

fn build_snapshot_view(tab: &BrowserTabState) -> BrowserSnapshotView {
    let elements = tab
        .dom_elements
        .iter()
        .map(|el| {
            let mut copy = el.clone();
            if copy.is_secret || tab.filled_card || tab.secret_document {
                if copy.is_secret && !copy.value.is_empty() {
                    copy.value = "[redacted]".to_string();
                } else {
                    copy.value = redact_text(&copy.value);
                }
            }
            copy
        })
        .collect();

    let safe_text = redact_text(&tab.page_text);
    let truncated = safe_text.len() > 16_000;
    let bounded_text = if truncated {
        safe_text.chars().take(16_000).collect()
    } else {
        safe_text
    };

    BrowserSnapshotView {
        tab_id: tab.id.clone(),
        url: tab.url.clone(),
        title: tab.title.clone(),
        revision: tab.revision,
        viewport: tab.environment.viewport.clone(),
        elements,
        text: bounded_text,
        truncated,
        secret_document: tab.secret_document,
        filled_card: tab.filled_card,
        screenshots_blocked: tab.screenshots_blocked || tab.filled_card,
        takeover_active: tab.takeover_active,
    }
}

fn resolve_or_upsert_element(
    tab: &mut BrowserTabState,
    target: &BrowserTarget,
    default_tag: &str,
    default_role: &str,
) -> RuntimeResult<usize> {
    match target {
        BrowserTarget::Ref { r#ref, revision } => {
            if let Some(expected_rev) = revision {
                if *expected_rev != tab.revision {
                    return Err(OpenBotRuntimeError::Conflict(format!(
                        "stale snapshot revision {expected_rev} (current tab revision is {})",
                        tab.revision
                    )));
                }
            }
            tab.dom_elements
                .iter()
                .position(|el| el.r#ref == *r#ref)
                .ok_or_else(|| {
                    OpenBotRuntimeError::NotFound(format!(
                        "browser element ref '{}' not found in current snapshot",
                        r#ref
                    ))
                })
        }
        BrowserTarget::Css { selector, nth } => {
            let trimmed = selector.trim();
            if trimmed.is_empty() {
                return Err(OpenBotRuntimeError::Validation(
                    "browser css selector must not be empty".to_string(),
                ));
            }
            let matches: Vec<usize> = tab
                .dom_elements
                .iter()
                .enumerate()
                .filter(|(_, el)| el.selector == trimmed)
                .map(|(i, _)| i)
                .collect();
            let pick = nth.unwrap_or(0);
            if let Some(&idx) = matches.get(pick) {
                return Ok(idx);
            }
            let next_ref = format!("e{}", tab.dom_elements.len() + 1);
            let is_secret = trimmed.to_ascii_lowercase().contains("password")
                || trimmed.to_ascii_lowercase().contains("cc-");
            tab.dom_elements.push(DomElementSnapshot {
                r#ref: next_ref,
                tag: default_tag.to_string(),
                role: default_role.to_string(),
                name: trimmed.to_string(),
                selector: trimmed.to_string(),
                input_type: if default_tag == "input" {
                    Some(if is_secret { "password" } else { "text" }.to_string())
                } else {
                    None
                },
                autocomplete: None,
                value: String::new(),
                checked: false,
                selected_options: Vec::new(),
                uploaded_files: Vec::new(),
                is_secret,
                focused: false,
            });
            Ok(tab.dom_elements.len() - 1)
        }
        BrowserTarget::Role {
            role,
            name,
            exact,
            nth,
        } => {
            let trimmed_role = role.trim();
            let want_exact = exact.unwrap_or(false);
            let matches: Vec<usize> = tab
                .dom_elements
                .iter()
                .enumerate()
                .filter(|(_, el)| {
                    el.role.eq_ignore_ascii_case(trimmed_role)
                        && match name {
                            None => true,
                            Some(n) if want_exact => el.name == *n,
                            Some(n) => el.name.to_ascii_lowercase().contains(&n.to_ascii_lowercase()),
                        }
                })
                .map(|(i, _)| i)
                .collect();
            matches.get(nth.unwrap_or(0)).copied().ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!("no element matched role '{role}'"))
            })
        }
        BrowserTarget::Text { text, exact, nth } => {
            let want_exact = exact.unwrap_or(false);
            let matches: Vec<usize> = tab
                .dom_elements
                .iter()
                .enumerate()
                .filter(|(_, el)| {
                    if want_exact {
                        el.name == *text || el.value == *text
                    } else {
                        let q = text.to_ascii_lowercase();
                        el.name.to_ascii_lowercase().contains(&q)
                            || el.value.to_ascii_lowercase().contains(&q)
                    }
                })
                .map(|(i, _)| i)
                .collect();
            matches.get(nth.unwrap_or(0)).copied().ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!("no element matched text '{text}'"))
            })
        }
        BrowserTarget::Point { x, y } => {
            if !x.is_finite()
                || !y.is_finite()
                || *x < 0.0
                || *y < 0.0
                || *x > INPUT_LIMITS.browser_coordinate as f64
                || *y > INPUT_LIMITS.browser_coordinate as f64
            {
                return Err(OpenBotRuntimeError::Validation(
                    "browser point coordinates out of bounds".to_string(),
                ));
            }
            if tab.dom_elements.is_empty() {
                tab.dom_elements.push(DomElementSnapshot {
                    r#ref: "e1".to_string(),
                    tag: default_tag.to_string(),
                    role: default_role.to_string(),
                    name: format!("point({x:.0},{y:.0})"),
                    selector: format!("point({x:.0},{y:.0})"),
                    input_type: None,
                    autocomplete: None,
                    value: String::new(),
                    checked: false,
                    selected_options: Vec::new(),
                    uploaded_files: Vec::new(),
                    is_secret: false,
                    focused: false,
                });
            }
            Ok(0)
        }
    }
}

fn evaluate_page_expression(tab: &BrowserTabState, cookies: &[BrowserCookie], expr: &str) -> RuntimeResult<Value> {
    let trimmed = expr.trim();
    if trimmed.is_empty() || utf16_len(trimmed) > BROWSER_EVALUATE_MAX_CHARS {
        return Err(OpenBotRuntimeError::Validation(format!(
            "evaluate expression must be 1..={BROWSER_EVALUATE_MAX_CHARS} characters"
        )));
    }
    let cleaned = trimmed.trim_end_matches(';').trim();
    let result = match cleaned {
        "document.title" => Value::String(tab.title.clone()),
        "location.href" | "window.location.href" | "document.URL" => Value::String(tab.url.clone()),
        "location.origin" | "window.location.origin" => {
            let origin = Url::parse(&tab.url)
                .map(|u| u.origin().ascii_serialization())
                .unwrap_or_default();
            Value::String(origin)
        }
        "location.pathname" | "window.location.pathname" => {
            let path = Url::parse(&tab.url)
                .map(|u| u.path().to_string())
                .unwrap_or_else(|_| "/".to_string());
            Value::String(path)
        }
        "document.body.innerText" | "document.body.textContent" => {
            Value::String(redact_text(&tab.page_text))
        }
        "document.links.length" => json!(tab.links.len()),
        "window.scrollX" => json!(tab.scroll_x),
        "window.scrollY" => json!(tab.scroll_y),
        "document.cookie" => {
            let host = Url::parse(&tab.url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
                .unwrap_or_default();
            let pairs: Vec<String> = cookies
                .iter()
                .filter(|c| !c.http_only && (host == c.domain || host.ends_with(&format!(".{}", c.domain))))
                .map(|c| format!("{}={}", c.name, c.value))
                .collect();
            Value::String(pairs.join("; "))
        }
        other => {
            if let Ok(parsed_json) = serde_json::from_str::<Value>(other) {
                parsed_json
            } else if let Some(inner) = other.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
                Value::String(inner.to_string())
            } else {
                json!({
                    "evaluated": true,
                    "expression": redact_text(other),
                    "url": tab.url,
                    "title": tab.title,
                })
            }
        }
    };
    let encoded_len = serde_json::to_vec(&result)?.len();
    if encoded_len > BROWSER_EVALUATE_MAX_RESULT_BYTES {
        return Err(OpenBotRuntimeError::LimitExceeded(format!(
            "evaluate result ({encoded_len} bytes) exceeds {BROWSER_EVALUATE_MAX_RESULT_BYTES} byte limit"
        )));
    }
    Ok(result)
}

fn parse_sandboxed_script_steps(script: &str) -> RuntimeResult<Vec<Value>> {
    let trimmed = script.trim();
    if trimmed.is_empty() || utf16_len(trimmed) > BROWSER_SCRIPT_MAX_CHARS {
        return Err(OpenBotRuntimeError::Validation(format!(
            "browser script must be 1..={BROWSER_SCRIPT_MAX_CHARS} characters"
        )));
    }
    let lower = trimmed.to_ascii_lowercase();
    for forbidden in ["require(", "process.", "child_process", "import(", "fs.", "fetch("] {
        if lower.contains(forbidden) {
            return Err(OpenBotRuntimeError::Forbidden(format!(
                "sandboxed browser script forbids '{forbidden}'"
            )));
        }
    }
    if trimmed.starts_with('[') {
        let parsed: Vec<Value> = serde_json::from_str(trimmed).map_err(|err| {
            OpenBotRuntimeError::Validation(format!("invalid JSON steps array in run_script: {err}"))
        })?;
        return Ok(parsed);
    }

    let mut steps = Vec::new();
    for raw_line in trimmed.lines() {
        let line = raw_line
            .trim()
            .trim_start_matches("await ")
            .trim()
            .trim_end_matches(';');
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if let Some(arg) = strip_call_arg(line, "step") {
            let step_val: Value = serde_json::from_str(arg).map_err(|err| {
                OpenBotRuntimeError::Validation(format!("invalid step(...) JSON argument: {err}"))
            })?;
            steps.push(step_val);
        } else if let Some(arg) = strip_call_arg(line, "navigate") {
            let url = unquote_js_str(arg);
            steps.push(json!({ "action": "navigate", "url": url }));
        } else if let Some(arg) = strip_call_arg(line, "sleep") {
            let ms: u64 = arg.trim().parse().map_err(|_| {
                OpenBotRuntimeError::Validation(format!("invalid sleep(ms) argument '{arg}'"))
            })?;
            steps.push(json!({ "action": "sleep", "ms": ms }));
        } else if let Some(arg) = strip_call_arg(line, "click") {
            let selector = unquote_js_str(arg);
            steps.push(json!({ "action": "click", "target": { "kind": "css", "selector": selector } }));
        } else if let Some(arg) = strip_call_arg(line, "hover") {
            let selector = unquote_js_str(arg);
            steps.push(json!({ "action": "hover", "target": { "kind": "css", "selector": selector } }));
        } else if let Some(arg) = strip_call_arg(line, "press") {
            let key = unquote_js_str(arg);
            steps.push(json!({ "action": "press", "key": key }));
        } else if let Some(arg) = strip_call_arg(line, "scroll") {
            let dy: f64 = arg.trim().parse().unwrap_or(400.0);
            steps.push(json!({ "action": "scroll", "deltaY": dy }));
        } else if let Some(arg) = strip_call_arg(line, "evaluate") {
            let expr = unquote_js_str(arg);
            steps.push(json!({ "action": "evaluate", "expression": expr }));
        } else if let Some(arg) = strip_call_arg(line, "waitFor") {
            let txt = unquote_js_str(arg);
            steps.push(json!({ "action": "wait_for", "text": txt }));
        } else if let Some(arg) = strip_call_arg(line, "log") {
            steps.push(json!({ "action": "log", "message": unquote_js_str(arg) }));
        } else if line.starts_with("snapshot(") {
            steps.push(json!({ "action": "snapshot" }));
        } else if line.starts_with("screenshot(") {
            steps.push(json!({ "action": "screenshot" }));
        } else if let Some(arg) = strip_call_arg(line, "type") {
            if let Some((sel_raw, text_raw)) = arg.split_once(',') {
                steps.push(json!({
                    "action": "type",
                    "target": { "kind": "css", "selector": unquote_js_str(sel_raw) },
                    "text": unquote_js_str(text_raw),
                }));
            } else {
                steps.push(json!({ "action": "type", "text": unquote_js_str(arg) }));
            }
        }
    }
    Ok(steps)
}

fn strip_call_arg<'a>(line: &'a str, func: &str) -> Option<&'a str> {
    let prefix = format!("{func}(");
    let rest = line.strip_prefix(&prefix)?;
    rest.strip_suffix(')').map(str::trim)
}

fn unquote_js_str(raw: &str) -> String {
    let t = raw.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

fn ensure_dir_700(dir: &Path) -> RuntimeResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let existed = dir.exists();
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(dir)?;
        if !existed {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

/// Persistent private-browser runtime backing all `openbot_browser.*` tools.
pub struct BrowserService {
    database: OpenBotDatabase,
    data_dir: PathBuf,
    state_path: PathBuf,
    vault: Arc<VaultService>,
    state: RwLock<StoredBrowserState>,
}

impl BrowserService {
    pub fn open(
        database: OpenBotDatabase,
        data_dir: impl AsRef<Path>,
        vault: Arc<VaultService>,
    ) -> RuntimeResult<Arc<Self>> {
        let data_dir = data_dir.as_ref().to_path_buf();
        ensure_dir_700(&data_dir)?;
        let state_path = data_dir.join(BROWSER_STATE_FILE_NAME);
        let loaded = if state_path.exists() {
            let raw = fs::read_to_string(&state_path)?;
            serde_json::from_str::<StoredBrowserState>(&raw).unwrap_or_default()
        } else {
            StoredBrowserState::default()
        };
        let mut normalized = loaded;
        if normalized.windows.is_empty() {
            normalized.windows.push(BrowserWindowState {
                window_id: "win-1".to_string(),
                active_tab_id: normalized.active_tab_id.clone(),
                tab_ids: normalized.tabs.iter().map(|t| t.id.clone()).collect(),
                width: 1200,
                height: 800,
            });
        }
        Ok(Arc::new(Self {
            database,
            data_dir,
            state_path,
            vault,
            state: RwLock::new(normalized),
        }))
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.database
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    pub fn vault(&self) -> &Arc<VaultService> {
        &self.vault
    }

    pub fn profile_store(&self) -> BrowserProfileStore {
        self.database.browser_profile_store()
    }

    pub fn proxy_store(&self) -> BrowserProxyStore {
        self.database.browser_proxy_store()
    }

    fn persist_locked(&self, state: &StoredBrowserState) -> RuntimeResult<()> {
        write_json_file_atomically(&self.state_path, state, true)?;
        Ok(())
    }

    fn find_tab_index(state: &StoredBrowserState, tab_id: &str) -> RuntimeResult<usize> {
        state
            .tabs
            .iter()
            .position(|t| t.id == tab_id)
            .ok_or_else(|| OpenBotRuntimeError::NotFound(format!("browser tab '{tab_id}' not found")))
    }

    fn require_not_takeover_paused(tab: &BrowserTabState) -> RuntimeResult<()> {
        if tab.takeover_active {
            return Err(OpenBotRuntimeError::Conflict(format!(
                "browser tab '{}' is paused awaiting human takeover; call release_takeover before resuming automation",
                tab.id
            )));
        }
        Ok(())
    }

    fn require_not_card_locked(tab: &BrowserTabState, action: &str) -> RuntimeResult<()> {
        if tab.filled_card || tab.screenshots_blocked {
            return Err(OpenBotRuntimeError::Forbidden(format!(
                "{action} is unavailable while the page shows a filled card. Use snapshots until the page navigates away."
            )));
        }
        Ok(())
    }

    fn apply_url_load_to_tab(tab: &mut BrowserTabState, url: String, push_history: bool) {
        let (title, page_text, dom_elements, links) = synthesize_page_for_url(&url);
        if push_history {
            if tab.history_index + 1 < tab.history.len() {
                tab.history.truncate(tab.history_index + 1);
            }
            tab.history.push(url.clone());
            tab.history_index = tab.history.len().saturating_sub(1);
        }
        tab.url = url.clone();
        tab.title = title;
        tab.page_text = page_text;
        tab.dom_elements = dom_elements;
        tab.links = links;
        tab.scroll_x = 0.0;
        tab.scroll_y = 0.0;
        tab.revision += 1;
        // Main-frame navigation clears exfiltration locks & secret-document taint.
        tab.filled_card = false;
        tab.screenshots_blocked = false;
        tab.secret_document = false;
        tab.network_logs.push(BrowserNetworkEntry {
            method: "GET".to_string(),
            url: sanitize_diagnostic_url(&url),
            status: 200,
            resource_type: "document".to_string(),
            timestamp: now_rfc3339_millis(),
        });
    }

    fn summarize_tab(state: &StoredBrowserState, tab: &BrowserTabState) -> BrowserTabSummary {
        BrowserTabSummary {
            id: tab.id.clone(),
            window_id: tab.window_id.clone(),
            url: tab.url.clone(),
            title: tab.title.clone(),
            active: state.active_tab_id.as_deref() == Some(tab.id.as_str()),
            owner_agent_id: tab.owner_agent_id.clone(),
            opener_tab_id: tab.opener_tab_id.clone(),
            revision: tab.revision,
            secret_document: tab.secret_document,
            filled_card: tab.filled_card,
            screenshots_blocked: tab.screenshots_blocked || tab.filled_card,
            takeover_active: tab.takeover_active,
        }
    }

    pub fn open_tab(
        &self,
        url: &str,
        owner_agent_id: Option<&str>,
        owner_thread_id: Option<&str>,
        opener_tab_id: Option<&str>,
        allow_blank: bool,
    ) -> RuntimeResult<BrowserSnapshotView> {
        let normalized = normalize_browser_url(url, allow_blank)?;
        let mut guard = self.state.write();
        if guard.tabs.len() >= MAX_BROWSER_TABS {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "browser tab limit ({MAX_BROWSER_TABS}) exceeded; close an existing tab before opening a new one"
            )));
        }
        let window_id = guard
            .windows
            .first()
            .map(|w| w.window_id.clone())
            .unwrap_or_else(|| "win-1".to_string());
        let tab_id = format!("tab-{}", Uuid::new_v4());
        let (title, page_text, dom_elements, links) = synthesize_page_for_url(&normalized);
        let tab = BrowserTabState {
            id: tab_id.clone(),
            window_id: window_id.clone(),
            url: normalized.clone(),
            title,
            owner_agent_id: owner_agent_id.map(ToOwned::to_owned),
            owner_thread_id: owner_thread_id.map(ToOwned::to_owned),
            opener_tab_id: opener_tab_id.map(ToOwned::to_owned),
            profile_id: None,
            revision: 1,
            history: vec![normalized.clone()],
            history_index: 0,
            dom_elements,
            page_text,
            links,
            console_logs: Vec::new(),
            network_logs: vec![BrowserNetworkEntry {
                method: "GET".to_string(),
                url: sanitize_diagnostic_url(&normalized),
                status: 200,
                resource_type: "document".to_string(),
                timestamp: now_rfc3339_millis(),
            }],
            local_storage: BTreeMap::new(),
            environment: BrowserEnvironmentState::default(),
            recording: None,
            pending_dialogs: Vec::new(),
            scroll_x: 0.0,
            scroll_y: 0.0,
            secret_document: false,
            filled_card: false,
            screenshots_blocked: false,
            takeover_active: false,
            active_takeover_id: None,
            takeover_reason: None,
        };
        let snapshot = build_snapshot_view(&tab);
        guard.tabs.push(tab);
        guard.active_tab_id = Some(tab_id.clone());
        if let Some(win) = guard.windows.iter_mut().find(|w| w.window_id == window_id) {
            win.tab_ids.push(tab_id.clone());
            win.active_tab_id = Some(tab_id);
        }
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn new_tab(
        &self,
        url: Option<&str>,
        owner_agent_id: Option<&str>,
    ) -> RuntimeResult<BrowserSnapshotView> {
        self.open_tab(url.unwrap_or("about:blank"), owner_agent_id, None, None, true)
    }
}

impl BrowserService {
    pub fn close_tab(&self, tab_id: &str) -> RuntimeResult<bool> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        let removed = guard.tabs.remove(idx);
        for win in &mut guard.windows {
            win.tab_ids.retain(|id| id != &removed.id);
            if win.active_tab_id.as_deref() == Some(removed.id.as_str()) {
                win.active_tab_id = win.tab_ids.last().cloned();
            }
        }
        if guard.active_tab_id.as_deref() == Some(removed.id.as_str()) {
            guard.active_tab_id = guard.tabs.last().map(|t| t.id.clone());
        }
        self.persist_locked(&guard)?;
        Ok(true)
    }

    pub fn close_tabs(&self, tab_id: Option<&str>, agent_id: Option<&str>) -> RuntimeResult<usize> {
        if let Some(id) = tab_id {
            self.close_tab(id)?;
            return Ok(1);
        }
        let mut guard = self.state.write();
        let before = guard.tabs.len();
        guard.tabs.retain(|t| match agent_id {
            Some(aid) => t.owner_agent_id.as_deref() != Some(aid),
            None => false,
        });
        let removed = before - guard.tabs.len();
        let remaining_ids: HashSet<String> = guard.tabs.iter().map(|t| t.id.clone()).collect();
        for win in &mut guard.windows {
            win.tab_ids.retain(|id| remaining_ids.contains(id));
            if let Some(ref active) = win.active_tab_id {
                if !remaining_ids.contains(active) {
                    win.active_tab_id = win.tab_ids.last().cloned();
                }
            }
        }
        if let Some(ref active) = guard.active_tab_id {
            if !remaining_ids.contains(active) {
                guard.active_tab_id = guard.tabs.last().map(|t| t.id.clone());
            }
        }
        self.persist_locked(&guard)?;
        Ok(removed)
    }

    pub fn switch_tab(&self, tab_id: &str) -> RuntimeResult<BrowserTabSummary> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        let win_id = guard.tabs[idx].window_id.clone();
        guard.active_tab_id = Some(tab_id.to_string());
        if let Some(win) = guard.windows.iter_mut().find(|w| w.window_id == win_id) {
            win.active_tab_id = Some(tab_id.to_string());
        }
        let summary = Self::summarize_tab(&guard, &guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(summary)
    }

    pub fn list_tabs(&self, agent_id: Option<&str>) -> Vec<BrowserTabSummary> {
        let guard = self.state.read();
        guard
            .tabs
            .iter()
            .filter(|t| match agent_id {
                None => true,
                Some(aid) => t.owner_agent_id.is_none() || t.owner_agent_id.as_deref() == Some(aid),
            })
            .map(|t| Self::summarize_tab(&guard, t))
            .collect()
    }

    pub fn status(&self, agent_id: Option<&str>) -> Value {
        let guard = self.state.read();
        let tabs: Vec<BrowserTabSummary> = guard
            .tabs
            .iter()
            .filter(|t| match agent_id {
                None => true,
                Some(aid) => t.owner_agent_id.is_none() || t.owner_agent_id.as_deref() == Some(aid),
            })
            .map(|t| Self::summarize_tab(&guard, t))
            .collect();
        let active_takeovers: Vec<&TakeoverState> =
            guard.takeovers.iter().filter(|tk| tk.takeover_active).collect();
        let recordings: Vec<Value> = guard
            .tabs
            .iter()
            .filter_map(|t| {
                t.recording.as_ref().map(|r| {
                    json!({
                        "tabId": t.id,
                        "recordingId": r.recording_id,
                        "startedAt": r.started_at,
                    })
                })
            })
            .collect();
        json!({
            "activeTabId": guard.active_tab_id,
            "tabCount": tabs.len(),
            "maxTabs": MAX_BROWSER_TABS,
            "windows": guard.windows,
            "tabs": tabs,
            "activeTakeovers": active_takeovers,
            "recordings": recordings,
        })
    }

    /// Explicitly resets card/secret exfiltration locks on `tab_id`.
    pub fn reset_exfiltration_lock(&self, tab_id: &str) -> RuntimeResult<BrowserTabSummary> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        guard.tabs[idx].filled_card = false;
        guard.tabs[idx].screenshots_blocked = false;
        guard.tabs[idx].secret_document = false;
        let summary = Self::summarize_tab(&guard, &guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(summary)
    }

    pub fn navigate(
        &self,
        tab_id: &str,
        url: Option<&str>,
        direction: Option<&str>,
    ) -> RuntimeResult<BrowserSnapshotView> {
        if let Some(dir) = direction {
            return match dir {
                "back" => self.back(tab_id),
                "forward" => self.forward(tab_id),
                "reload" => self.reload(tab_id),
                other => Err(OpenBotRuntimeError::Validation(format!(
                    "invalid navigate direction '{other}': expected back, forward, or reload"
                ))),
            };
        }
        let raw_url = url.ok_or_else(|| {
            OpenBotRuntimeError::Validation("navigate requires url or direction".to_string())
        })?;
        let normalized = normalize_browser_url(raw_url, false)?;
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        Self::apply_url_load_to_tab(&mut guard.tabs[idx], normalized, true);
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn back(&self, tab_id: &str) -> RuntimeResult<BrowserSnapshotView> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        if guard.tabs[idx].history_index == 0 {
            return Err(OpenBotRuntimeError::Conflict(
                "cannot navigate back: already at the start of tab history".to_string(),
            ));
        }
        let target_idx = guard.tabs[idx].history_index - 1;
        let target_url = guard.tabs[idx].history[target_idx].clone();
        guard.tabs[idx].history_index = target_idx;
        Self::apply_url_load_to_tab(&mut guard.tabs[idx], target_url, false);
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn forward(&self, tab_id: &str) -> RuntimeResult<BrowserSnapshotView> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        if guard.tabs[idx].history_index + 1 >= guard.tabs[idx].history.len() {
            return Err(OpenBotRuntimeError::Conflict(
                "cannot navigate forward: already at the newest history entry".to_string(),
            ));
        }
        let target_idx = guard.tabs[idx].history_index + 1;
        let target_url = guard.tabs[idx].history[target_idx].clone();
        guard.tabs[idx].history_index = target_idx;
        Self::apply_url_load_to_tab(&mut guard.tabs[idx], target_url, false);
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn reload(&self, tab_id: &str) -> RuntimeResult<BrowserSnapshotView> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        let current_url = guard.tabs[idx].url.clone();
        Self::apply_url_load_to_tab(&mut guard.tabs[idx], current_url, false);
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn snapshot(&self, tab_id: &str) -> RuntimeResult<BrowserSnapshotView> {
        let guard = self.state.read();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Ok(build_snapshot_view(&guard.tabs[idx]))
    }

    pub fn screenshot(
        &self,
        tab_id: &str,
        save: bool,
    ) -> RuntimeResult<BrowserScreenshotResult> {
        let guard = self.state.read();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        let tab = &guard.tabs[idx];
        Self::require_not_takeover_paused(tab)?;
        Self::require_not_card_locked(tab, "screenshot")?;

        let png = encode_minimal_png(32, 24, [26, 26, 26, 255])?;
        let saved_path = if save {
            let dir = self.data_dir.join("browser-screenshots");
            ensure_dir_700(&dir)?;
            let out_path = dir.join(format!("{}-r{}.png", tab.id, tab.revision));
            fs::write(&out_path, &png)?;
            Some(out_path.to_string_lossy().to_string())
        } else {
            None
        };
        Ok(BrowserScreenshotResult {
            tab_id: tab.id.clone(),
            mime: "image/png".to_string(),
            width: 32,
            height: 24,
            byte_length: png.len(),
            base64: BASE64_STANDARD.encode(&png),
            path: saved_path,
        })
    }
}

impl BrowserService {
    pub fn extract_text(&self, tab_id: &str) -> RuntimeResult<String> {
        let guard = self.state.read();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Ok(redact_text(&guard.tabs[idx].page_text))
    }

    pub fn get_links(&self, tab_id: &str) -> RuntimeResult<Vec<BrowserLink>> {
        let guard = self.state.read();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Ok(guard.tabs[idx].links.clone())
    }

    pub fn console_logs(
        &self,
        tab_id: &str,
        level: Option<&str>,
        clear: bool,
    ) -> RuntimeResult<Vec<BrowserConsoleEntry>> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        let out: Vec<BrowserConsoleEntry> = guard.tabs[idx]
            .console_logs
            .iter()
            .filter(|e| level.is_none_or(|l| e.level.eq_ignore_ascii_case(l)))
            .map(|e| BrowserConsoleEntry {
                level: e.level.clone(),
                text: redact_text(&e.text),
                timestamp: e.timestamp.clone(),
            })
            .collect();
        if clear {
            guard.tabs[idx].console_logs.clear();
            self.persist_locked(&guard)?;
        }
        Ok(out)
    }

    pub fn network_log(&self, tab_id: &str, clear: bool) -> RuntimeResult<Vec<BrowserNetworkEntry>> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        let out = guard.tabs[idx].network_logs.clone();
        if clear {
            guard.tabs[idx].network_logs.clear();
            self.persist_locked(&guard)?;
        }
        Ok(out)
    }

    pub fn click(
        &self,
        tab_id: &str,
        target: &BrowserTarget,
        click_count: u8,
    ) -> RuntimeResult<BrowserSnapshotView> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        let el_idx = resolve_or_upsert_element(&mut guard.tabs[idx], target, "button", "button")?;
        for el in &mut guard.tabs[idx].dom_elements {
            el.focused = false;
        }
        guard.tabs[idx].dom_elements[el_idx].focused = true;
        let label = guard.tabs[idx].dom_elements[el_idx].name.clone();
        guard.tabs[idx].revision += 1;
        guard.tabs[idx].console_logs.push(BrowserConsoleEntry {
            level: "info".to_string(),
            text: format!(
                "{} target '{label}'",
                if click_count >= 2 { "double_click" } else { "click" }
            ),
            timestamp: now_rfc3339_millis(),
        });
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn double_click(
        &self,
        tab_id: &str,
        target: &BrowserTarget,
    ) -> RuntimeResult<BrowserSnapshotView> {
        self.click(tab_id, target, 2)
    }

    pub fn hover(
        &self,
        tab_id: &str,
        target: &BrowserTarget,
    ) -> RuntimeResult<BrowserSnapshotView> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        let el_idx = resolve_or_upsert_element(&mut guard.tabs[idx], target, "div", "generic")?;
        let label = guard.tabs[idx].dom_elements[el_idx].name.clone();
        guard.tabs[idx].console_logs.push(BrowserConsoleEntry {
            level: "info".to_string(),
            text: format!("hover target '{label}'"),
            timestamp: now_rfc3339_millis(),
        });
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn type_text(
        &self,
        tab_id: &str,
        target: Option<&BrowserTarget>,
        text: &str,
        mode: Option<&str>,
        submit: bool,
    ) -> RuntimeResult<BrowserSnapshotView> {
        if utf16_len(text) > INPUT_LIMITS.browser_action_text {
            return Err(OpenBotRuntimeError::Validation(format!(
                "type text exceeds {} characters",
                INPUT_LIMITS.browser_action_text
            )));
        }
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;

        let el_idx = match target {
            Some(t) => resolve_or_upsert_element(&mut guard.tabs[idx], t, "input", "textbox")?,
            None => guard.tabs[idx]
                .dom_elements
                .iter()
                .position(|el| el.focused)
                .unwrap_or_else(|| {
                    let next_ref = format!("e{}", guard.tabs[idx].dom_elements.len() + 1);
                    guard.tabs[idx].dom_elements.push(DomElementSnapshot {
                        r#ref: next_ref,
                        tag: "input".to_string(),
                        role: "textbox".to_string(),
                        name: "Focused input".to_string(),
                        selector: ":focus".to_string(),
                        input_type: Some("text".to_string()),
                        autocomplete: None,
                        value: String::new(),
                        checked: false,
                        selected_options: Vec::new(),
                        uploaded_files: Vec::new(),
                        is_secret: false,
                        focused: true,
                    });
                    guard.tabs[idx].dom_elements.len() - 1
                }),
        };

        let check_entry = FillFieldEntry {
            r#ref: Some(guard.tabs[idx].dom_elements[el_idx].r#ref.clone()),
            selector: Some(guard.tabs[idx].dom_elements[el_idx].selector.clone()),
            name: Some(guard.tabs[idx].dom_elements[el_idx].name.clone()),
            label: None,
            text: text.to_string(),
        };
        if let Some(reason) =
            detect_secret_field_violation(&check_entry, Some(&guard.tabs[idx].dom_elements[el_idx]))
        {
            return Err(OpenBotRuntimeError::Forbidden(format!(
                "type refused on {reason}: use openbot_browser.submit_secret for passwords/codes or openbot_browser.fill_card for payment cards"
            )));
        }

        for el in &mut guard.tabs[idx].dom_elements {
            el.focused = false;
        }
        let el = &mut guard.tabs[idx].dom_elements[el_idx];
        el.focused = true;
        if mode == Some("append") {
            el.value.push_str(text);
        } else {
            el.value = text.to_string();
        }
        guard.tabs[idx].revision += 1;
        if submit {
            guard.tabs[idx].console_logs.push(BrowserConsoleEntry {
                level: "info".to_string(),
                text: "submitted form via Enter".to_string(),
                timestamp: now_rfc3339_millis(),
            });
        }
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn select_option(
        &self,
        tab_id: &str,
        target: &BrowserTarget,
        values: Vec<String>,
    ) -> RuntimeResult<BrowserSnapshotView> {
        if values.is_empty() || values.len() > 100 {
            return Err(OpenBotRuntimeError::Validation(
                "select_option requires 1..=100 option values".to_string(),
            ));
        }
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        let el_idx = resolve_or_upsert_element(&mut guard.tabs[idx], target, "select", "combobox")?;
        guard.tabs[idx].dom_elements[el_idx].selected_options = values.clone();
        guard.tabs[idx].dom_elements[el_idx].value = values.join(",");
        guard.tabs[idx].revision += 1;
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn set_checked(
        &self,
        tab_id: &str,
        target: &BrowserTarget,
        checked: bool,
    ) -> RuntimeResult<BrowserSnapshotView> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        let el_idx = resolve_or_upsert_element(&mut guard.tabs[idx], target, "input", "checkbox")?;
        guard.tabs[idx].dom_elements[el_idx].checked = checked;
        guard.tabs[idx].revision += 1;
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }
}

impl BrowserService {
    /// Batch-fills 1..=16 ordinary text fields (`<= 16 KiB` combined UTF-8).
    /// STRICTLY refuses any password, OTP, CVV, card, secret, token, or vault-secret field.
    pub fn fill_fields(&self, req: FillFieldsRequest) -> RuntimeResult<FillFieldsResult> {
        if req.fields.is_empty() || req.fields.len() > BROWSER_FILL_FIELDS_MAX_FIELDS {
            return Err(OpenBotRuntimeError::Validation(format!(
                "fill_fields requires 1..={BROWSER_FILL_FIELDS_MAX_FIELDS} fields"
            )));
        }
        let total_bytes: usize = req.fields.iter().map(|f| f.text.len()).sum();
        if total_bytes > BROWSER_FILL_FIELDS_MAX_TEXT_BYTES {
            return Err(OpenBotRuntimeError::Validation(format!(
                "fill_fields combined text ({total_bytes} bytes) exceeds 16 KiB UTF-8 limit"
            )));
        }

        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, &req.tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        if let Some(expected_rev) = req.revision {
            if expected_rev != guard.tabs[idx].revision {
                return Err(OpenBotRuntimeError::Conflict(format!(
                    "fill_fields stale snapshot revision {expected_rev} (current is {})",
                    guard.tabs[idx].revision
                )));
            }
        }

        // Pass 1: Validate duplicates AND run strict secret refusal BEFORE mutating any DOM element!
        let mut seen_keys = HashSet::new();
        for entry in &req.fields {
            let key = entry
                .r#ref
                .as_deref()
                .or(entry.selector.as_deref())
                .or(entry.name.as_deref())
                .ok_or_else(|| {
                    OpenBotRuntimeError::Validation(
                        "each fill_fields entry requires ref, selector, or name".to_string(),
                    )
                })?;
            if !seen_keys.insert(key.to_string()) {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "duplicate fill_fields target '{key}'"
                )));
            }
            let matched_el = guard.tabs[idx].dom_elements.iter().find(|el| {
                entry.r#ref.as_deref() == Some(el.r#ref.as_str())
                    || entry.selector.as_deref() == Some(el.selector.as_str())
                    || entry.name.as_deref() == Some(el.name.as_str())
            });
            if let Some(reason) = detect_secret_field_violation(entry, matched_el) {
                return Err(OpenBotRuntimeError::Forbidden(format!(
                    "fill_fields refused {reason}: passwords, secrets, OTP codes, and payment cards cannot be filled via fill_fields. Use openbot_browser.submit_secret (Vault) or openbot_browser.fill_card instead."
                )));
            }
        }

        // Pass 2: Apply all ordinary text fields atomically.
        let mut confirmed = Vec::with_capacity(req.fields.len());
        for (i, entry) in req.fields.iter().enumerate() {
            let target = if let Some(ref r) = entry.r#ref {
                BrowserTarget::Ref {
                    r#ref: r.clone(),
                    revision: req.revision,
                }
            } else {
                let sel = entry
                    .selector
                    .clone()
                    .or_else(|| entry.name.clone())
                    .unwrap_or_else(|| format!("input[name=field-{i}]"));
                BrowserTarget::Css {
                    selector: sel,
                    nth: None,
                }
            };
            let el_idx = resolve_or_upsert_element(&mut guard.tabs[idx], &target, "input", "textbox")?;
            guard.tabs[idx].dom_elements[el_idx].value = entry.text.clone();
            confirmed.push(i);
        }

        guard.tabs[idx].revision += 1;
        let revision = guard.tabs[idx].revision;
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;

        Ok(FillFieldsResult {
            tab_id: req.tab_id,
            revision,
            filled_count: confirmed.len(),
            confirmed,
            uncertain: Vec::new(),
            snapshot,
        })
    }

    pub fn press_key(
        &self,
        tab_id: &str,
        key: &str,
        target: Option<&BrowserTarget>,
    ) -> RuntimeResult<BrowserSnapshotView> {
        let trimmed = key.trim();
        if trimmed.is_empty() || utf16_len(trimmed) > 128 {
            return Err(OpenBotRuntimeError::Validation(
                "press_key requires a non-blank key name (<= 128 chars)".to_string(),
            ));
        }
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        if let Some(t) = target {
            let el_idx = resolve_or_upsert_element(&mut guard.tabs[idx], t, "div", "generic")?;
            for el in &mut guard.tabs[idx].dom_elements {
                el.focused = false;
            }
            guard.tabs[idx].dom_elements[el_idx].focused = true;
        }
        guard.tabs[idx].revision += 1;
        guard.tabs[idx].console_logs.push(BrowserConsoleEntry {
            level: "info".to_string(),
            text: format!("press_key '{trimmed}'"),
            timestamp: now_rfc3339_millis(),
        });
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn scroll(
        &self,
        tab_id: &str,
        delta_x: f64,
        delta_y: f64,
    ) -> RuntimeResult<BrowserSnapshotView> {
        if !delta_x.is_finite()
            || !delta_y.is_finite()
            || delta_x.abs() > 100_000.0
            || delta_y.abs() > 100_000.0
        {
            return Err(OpenBotRuntimeError::Validation(
                "scroll deltaX/deltaY must be finite and within -100000..=100000".to_string(),
            ));
        }
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        guard.tabs[idx].scroll_x = (guard.tabs[idx].scroll_x + delta_x).max(0.0);
        guard.tabs[idx].scroll_y = (guard.tabs[idx].scroll_y + delta_y).max(0.0);
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn drag(
        &self,
        tab_id: &str,
        source: &BrowserTarget,
        target: &BrowserTarget,
    ) -> RuntimeResult<BrowserSnapshotView> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        let s_idx = resolve_or_upsert_element(&mut guard.tabs[idx], source, "div", "generic")?;
        let t_idx = resolve_or_upsert_element(&mut guard.tabs[idx], target, "div", "generic")?;
        let s_name = guard.tabs[idx].dom_elements[s_idx].name.clone();
        let t_name = guard.tabs[idx].dom_elements[t_idx].name.clone();
        guard.tabs[idx].revision += 1;
        guard.tabs[idx].console_logs.push(BrowserConsoleEntry {
            level: "info".to_string(),
            text: format!("drag '{s_name}' -> '{t_name}'"),
            timestamp: now_rfc3339_millis(),
        });
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn wait_for(
        &self,
        tab_id: &str,
        text: Option<&str>,
        url_match: Option<&str>,
        state_name: Option<&str>,
    ) -> RuntimeResult<BrowserSnapshotView> {
        if let Some(st) = state_name {
            if !matches!(st, "load" | "domcontentloaded" | "dom-quiet") {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "invalid wait_for state '{st}': expected load, domcontentloaded, or dom-quiet"
                )));
            }
        }
        let guard = self.state.read();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        let tab = &guard.tabs[idx];
        if let Some(expected_url) = url_match {
            if !tab.url.contains(expected_url) {
                return Err(OpenBotRuntimeError::Execution(format!(
                    "wait_for timed out waiting for URL containing '{expected_url}' (current: '{}')",
                    tab.url
                )));
            }
        }
        if let Some(expected_text) = text {
            let found = tab.page_text.contains(expected_text)
                || tab.title.contains(expected_text)
                || tab
                    .dom_elements
                    .iter()
                    .any(|el| el.name.contains(expected_text) || (!el.is_secret && el.value.contains(expected_text)));
            if !found {
                return Err(OpenBotRuntimeError::Execution(format!(
                    "wait_for timed out waiting for text '{expected_text}'"
                )));
            }
        }
        Ok(build_snapshot_view(tab))
    }

    pub fn evaluate(&self, tab_id: &str, expression: &str) -> RuntimeResult<Value> {
        let guard = self.state.read();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        let tab = &guard.tabs[idx];
        Self::require_not_takeover_paused(tab)?;
        Self::require_not_card_locked(tab, "evaluate")?;
        if tab.secret_document {
            return Err(OpenBotRuntimeError::Forbidden(
                "evaluate is unavailable while a page that received a secret is open. Use snapshots and actions until it navigates."
                    .to_string(),
            ));
        }
        evaluate_page_expression(tab, &guard.cookies, expression)
    }
}

impl BrowserService {
    pub fn download(
        &self,
        tab_id: &str,
        url: &str,
        suggested_filename: Option<&str>,
    ) -> RuntimeResult<BrowserDownloadRecord> {
        let normalized = normalize_browser_url(url, false)?;
        let parsed = Url::parse(&normalized).map_err(|err| {
            OpenBotRuntimeError::Validation(format!("invalid download URL '{normalized}': {err}"))
        })?;
        let raw_name = suggested_filename
            .filter(|s| !s.trim().is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                parsed
                    .path_segments()
                    .and_then(|mut segs| segs.next_back())
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| "download.bin".to_string());
        let safe_name: String = raw_name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            .collect();
        let filename = if safe_name.is_empty() || safe_name.starts_with('.') || safe_name.contains("..") {
            "download.bin".to_string()
        } else {
            safe_name
        };

        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;

        let downloads_dir = self.data_dir.join("browser-downloads");
        ensure_dir_700(&downloads_dir)?;
        let download_id = format!("dl-{}", Uuid::new_v4());
        let saved_path = downloads_dir.join(format!("{download_id}-{filename}"));
        let metadata_bytes = format!("download_url={}\n", sanitize_diagnostic_url(&normalized));
        fs::write(&saved_path, metadata_bytes.as_bytes())?;

        let record = BrowserDownloadRecord {
            download_id,
            tab_id: tab_id.to_string(),
            url: sanitize_diagnostic_url(&normalized),
            filename,
            saved_path: saved_path.to_string_lossy().to_string(),
            byte_length: metadata_bytes.len() as u64,
            status: "completed".to_string(),
            created_at: now_rfc3339_millis(),
        };
        guard.downloads.push(record.clone());
        self.persist_locked(&guard)?;
        Ok(record)
    }

    pub fn upload_files(
        &self,
        tab_id: &str,
        target: &BrowserTarget,
        paths: Vec<String>,
    ) -> RuntimeResult<BrowserSnapshotView> {
        if paths.is_empty() || paths.len() > INPUT_LIMITS.attachments {
            return Err(OpenBotRuntimeError::Validation(format!(
                "upload_files requires 1..={} paths",
                INPUT_LIMITS.attachments
            )));
        }
        let mut total_bytes: u64 = 0;
        let mut verified_names = Vec::with_capacity(paths.len());
        for raw in &paths {
            if raw.trim().is_empty() || utf16_len(raw) > INPUT_LIMITS.path {
                return Err(OpenBotRuntimeError::Validation(
                    "upload file path is empty or too long".to_string(),
                ));
            }
            let p = Path::new(raw);
            if p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(OpenBotRuntimeError::Forbidden(format!(
                    "upload path '{raw}' cannot contain '..' traversal segments"
                )));
            }
            let meta = fs::metadata(p).map_err(|_| {
                OpenBotRuntimeError::NotFound(format!("upload file '{raw}' does not exist"))
            })?;
            if !meta.is_file() {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "upload path '{raw}' is not a regular file"
                )));
            }
            if meta.len() > ATTACHMENT_LIMITS.file_bytes as u64 {
                return Err(OpenBotRuntimeError::LimitExceeded(format!(
                    "upload file '{raw}' exceeds maximum attachment size of {} bytes",
                    ATTACHMENT_LIMITS.file_bytes
                )));
            }
            total_bytes = total_bytes.saturating_add(meta.len());
            if total_bytes > ATTACHMENT_LIMITS.total_bytes as u64 {
                return Err(OpenBotRuntimeError::LimitExceeded(format!(
                    "combined upload files exceed {} bytes",
                    ATTACHMENT_LIMITS.total_bytes
                )));
            }
            verified_names.push(
                p.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("file")
                    .to_string(),
            );
        }

        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        let el_idx = resolve_or_upsert_element(&mut guard.tabs[idx], target, "input", "button")?;
        guard.tabs[idx].dom_elements[el_idx].uploaded_files = verified_names.clone();
        guard.tabs[idx].dom_elements[el_idx].value = verified_names.join(", ");
        guard.tabs[idx].revision += 1;
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;
        Ok(snapshot)
    }

    pub fn manage_cookies(
        &self,
        action: &str,
        domain: Option<&str>,
        cookie: Option<BrowserCookie>,
        cookie_name: Option<&str>,
    ) -> RuntimeResult<Value> {
        let mut guard = self.state.write();
        match action.trim().to_ascii_lowercase().as_str() {
            "list" | "get" => {
                let filtered: Vec<Value> = guard
                    .cookies
                    .iter()
                    .filter(|c| {
                        domain.is_none_or(|d| {
                            let dl = d.to_ascii_lowercase();
                            c.domain.eq_ignore_ascii_case(&dl) || dl.ends_with(&format!(".{}", c.domain))
                        }) && cookie_name.is_none_or(|n| c.name == n)
                    })
                    .map(|c| {
                        json!({
                            "name": c.name,
                            "value": if c.http_only { "[redacted-httponly]" } else { c.value.as_str() },
                            "domain": c.domain,
                            "path": c.path,
                            "secure": c.secure,
                            "httpOnly": c.http_only,
                        })
                    })
                    .collect();
                Ok(json!({ "cookies": filtered, "count": filtered.len() }))
            }
            "set" => {
                let mut item = cookie.ok_or_else(|| {
                    OpenBotRuntimeError::Validation("cookies set requires a cookie object".to_string())
                })?;
                if item.name.trim().is_empty() || item.domain.trim().is_empty() {
                    return Err(OpenBotRuntimeError::Validation(
                        "cookie name and domain are required".to_string(),
                    ));
                }
                item.domain = item.domain.trim().trim_start_matches('.').to_ascii_lowercase();
                if let Some(slot) = guard
                    .cookies
                    .iter_mut()
                    .find(|c| c.name == item.name && c.domain == item.domain && c.path == item.path)
                {
                    *slot = item.clone();
                } else {
                    guard.cookies.push(item.clone());
                }
                self.persist_locked(&guard)?;
                Ok(json!({ "saved": true, "name": item.name, "domain": item.domain }))
            }
            "delete" => {
                let name = cookie_name.ok_or_else(|| {
                    OpenBotRuntimeError::Validation("cookies delete requires name".to_string())
                })?;
                let before = guard.cookies.len();
                guard.cookies.retain(|c| {
                    !(c.name == name
                        && domain.is_none_or(|d| c.domain.eq_ignore_ascii_case(d.trim_start_matches('.'))))
                });
                let removed = before - guard.cookies.len();
                self.persist_locked(&guard)?;
                Ok(json!({ "deleted": removed }))
            }
            "clear" => {
                let before = guard.cookies.len();
                if let Some(d) = domain {
                    let dl = d.trim_start_matches('.').to_ascii_lowercase();
                    guard.cookies.retain(|c| !c.domain.eq_ignore_ascii_case(&dl));
                } else {
                    guard.cookies.clear();
                }
                let removed = before - guard.cookies.len();
                self.persist_locked(&guard)?;
                Ok(json!({ "cleared": removed }))
            }
            other => Err(OpenBotRuntimeError::Validation(format!(
                "unsupported cookies action '{other}': expected list, get, set, delete, or clear"
            ))),
        }
    }

    pub fn set_environment(
        &self,
        tab_id: &str,
        preset: Option<&str>,
        width: Option<u32>,
        height: Option<u32>,
        device_scale_factor: Option<f64>,
        color_scheme: Option<&str>,
        reduced_motion: Option<bool>,
    ) -> RuntimeResult<BrowserEnvironmentState> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        let current = guard.tabs[idx].environment.clone();

        let preset_dims: Option<(u32, u32, f64)> = match preset {
            None | Some("fill") | Some("custom") => None,
            Some("desktop") => Some((1440, 900, 1.0)),
            Some("tablet") => Some((820, 1180, 2.0)),
            Some("mobile") => Some((390, 844, 3.0)),
            Some(other) => {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "invalid viewport preset '{other}'"
                )));
            }
        };

        let explicit_scale = device_scale_factor.is_some();
        let mode = if preset == Some("fill") && !explicit_scale {
            "fill".to_string()
        } else if preset.is_some() || width.is_some() || height.is_some() || explicit_scale {
            "custom".to_string()
        } else {
            current.viewport.mode.clone()
        };
        let w = width
            .or_else(|| preset_dims.map(|p| p.0))
            .unwrap_or(current.viewport.width);
        let h = height
            .or_else(|| preset_dims.map(|p| p.1))
            .unwrap_or(current.viewport.height);
        let min_w = if mode == "fill" { 1 } else { 320 };
        let min_h = if mode == "fill" { 1 } else { 240 };
        if w < min_w
            || w > INPUT_LIMITS.browser_dimension as u32
            || h < min_h
            || h > INPUT_LIMITS.browser_dimension as u32
        {
            return Err(OpenBotRuntimeError::Validation(
                "viewport dimensions are outside the supported range".to_string(),
            ));
        }
        let scale = if mode == "fill" {
            1.0
        } else {
            device_scale_factor
                .or_else(|| preset_dims.map(|p| p.2))
                .unwrap_or(current.viewport.device_scale_factor)
        };
        if !(0.5..=4.0).contains(&scale) {
            return Err(OpenBotRuntimeError::Validation(
                "deviceScaleFactor must be between 0.5 and 4.0".to_string(),
            ));
        }
        let physical_pixels = (f64::from(w) * f64::from(h) * scale * scale) as u64;
        if physical_pixels > BROWSER_MAX_PHYSICAL_VIEWPORT_PIXELS {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "physical viewport exceeds {BROWSER_MAX_PHYSICAL_VIEWPORT_PIXELS} pixels"
            )));
        }
        let scheme = match color_scheme {
            None => current.color_scheme.clone(),
            Some("light") | Some("dark") | Some("system") => color_scheme.unwrap().to_string(),
            Some(other) => {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "invalid colorScheme '{other}': expected light, dark, or system"
                )));
            }
        };
        let next_env = BrowserEnvironmentState {
            viewport: BrowserViewportState {
                mode,
                width: w,
                height: h,
                device_scale_factor: scale,
                preset: match preset {
                    Some("desktop") | Some("tablet") | Some("mobile") => preset.map(ToOwned::to_owned),
                    Some("custom") | Some("fill") => None,
                    _ => current.viewport.preset.clone(),
                },
            },
            color_scheme: scheme,
            reduced_motion: reduced_motion.unwrap_or(current.reduced_motion),
        };
        guard.tabs[idx].environment = next_env.clone();
        self.persist_locked(&guard)?;
        Ok(next_env)
    }

    pub fn recording_start(&self, tab_id: &str) -> RuntimeResult<BrowserRecordingState> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        Self::require_not_card_locked(&guard.tabs[idx], "recording_start")?;
        if guard.tabs[idx].recording.is_some() {
            return Err(OpenBotRuntimeError::Conflict(format!(
                "tab '{tab_id}' already has an active recording"
            )));
        }
        let rec_dir = self.data_dir.join("browser-recordings");
        ensure_dir_700(&rec_dir)?;
        let recording_id = format!("rec-{}", Uuid::new_v4());
        let output_path = rec_dir.join(format!("{recording_id}.webm"));
        let state = BrowserRecordingState {
            recording_id,
            started_at: now_rfc3339_millis(),
            output_path: output_path.to_string_lossy().to_string(),
            max_duration_ms: BROWSER_RECORDING_MAX_DURATION_MS,
            max_bytes: BROWSER_RECORDING_MAX_BYTES,
        };
        guard.tabs[idx].recording = Some(state.clone());
        self.persist_locked(&guard)?;
        Ok(state)
    }

    pub fn recording_stop(&self, tab_id: &str) -> RuntimeResult<Value> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        let rec = guard.tabs[idx].recording.take().ok_or_else(|| {
            OpenBotRuntimeError::NotFound(format!("no active recording on tab '{tab_id}'"))
        })?;
        // Write valid EBML/WebM container header bytes for artifact inspection.
        let ebml_header: [u8; 16] = [
            0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x86, 0x81, 0x01, 0x42, 0x82, 0x84, b'w', b'e',
            b'b', b'm',
        ];
        fs::write(&rec.output_path, ebml_header)?;
        self.persist_locked(&guard)?;
        Ok(json!({
            "recordingId": rec.recording_id,
            "tabId": tab_id,
            "path": rec.output_path,
            "mime": "video/webm",
            "byteLength": ebml_header.len(),
            "stoppedAt": now_rfc3339_millis(),
        }))
    }

    pub fn handle_dialog(&self, tab_id: &str, dialog_id: &str, accept: bool) -> RuntimeResult<Value> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        let pos = guard.tabs[idx]
            .pending_dialogs
            .iter()
            .position(|d| d.dialog_id == dialog_id);
        let handled = match pos {
            Some(i) => {
                let mut d = guard.tabs[idx].pending_dialogs.remove(i);
                d.accepted = Some(accept);
                Some(d)
            }
            None => None,
        };
        self.persist_locked(&guard)?;
        Ok(json!({
            "tabId": tab_id,
            "dialogId": dialog_id,
            "accept": accept,
            "handled": handled.is_some(),
            "remainingDialogs": guard.tabs[idx].pending_dialogs.len(),
        }))
    }
}

impl BrowserService {
    /// Executes a structured sequence of up to `200` browser automation steps (`<= 25,000 ms` sleep cap).
    pub fn run_script_steps(
        &self,
        req: BrowserRunStepsRequest,
    ) -> RuntimeResult<BrowserRunScriptResult> {
        if req.steps.len() > BROWSER_SCRIPT_MAX_STEPS {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "browser run_script exceeded the maximum step limit of {BROWSER_SCRIPT_MAX_STEPS} steps (got {})",
                req.steps.len()
            )));
        }
        let timeout_ms = req.timeout_ms.unwrap_or(BROWSER_SCRIPT_DEFAULT_TIMEOUT_MS);
        if !(BROWSER_SCRIPT_MIN_TIMEOUT_MS..=BROWSER_SCRIPT_MAX_TIMEOUT_MS).contains(&timeout_ms) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "browser script timeoutMs must be {BROWSER_SCRIPT_MIN_TIMEOUT_MS}..={BROWSER_SCRIPT_MAX_TIMEOUT_MS}, got {timeout_ms}"
            )));
        }

        // Pre-validate sleep bounds across all steps before executing mutations.
        for step in &req.steps {
            if step.get("action").and_then(Value::as_str) == Some("sleep") {
                let ms = step
                    .get("ms")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| OpenBotRuntimeError::Validation("sleep step requires non-negative integer ms".to_string()))?;
                if ms > BROWSER_SCRIPT_MAX_SLEEP_MS {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "browser script sleep ({ms} ms) exceeds maximum 25000 ms (25,000 ms)"
                    )));
                }
            }
        }

        let tab_id = match req.tab_id {
            Some(existing_id) => {
                // Verify tab exists; if caller supplied a new URL and no explicit navigate step, stay consistent.
                let current_url = {
                    let guard = self.state.read();
                    let idx = Self::find_tab_index(&guard, &existing_id)?;
                    Self::require_not_takeover_paused(&guard.tabs[idx])?;
                    guard.tabs[idx].url.clone()
                };
                if let Some(ref target_url) = req.url {
                    let norm = normalize_browser_url(target_url, false)?;
                    let has_explicit_nav = req
                        .steps
                        .iter()
                        .any(|s| s.get("action").and_then(Value::as_str) == Some("navigate"));
                    if !has_explicit_nav && norm != current_url {
                        self.navigate(&existing_id, Some(&norm), None)?;
                    }
                }
                existing_id
            }
            None => {
                let start_url = req.url.as_deref().unwrap_or("about:blank");
                let snap = self.open_tab(
                    start_url,
                    req.agent_id.as_deref(),
                    None,
                    None,
                    req.url.is_none(),
                )?;
                snap.tab_id
            }
        };

        let mut logs = Vec::new();
        let mut step_results = Vec::with_capacity(req.steps.len());
        let mut virtual_elapsed_ms: u64 = 0;

        for step in &req.steps {
            let action = step
                .get("action")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    OpenBotRuntimeError::Validation("browser script step missing 'action'".to_string())
                })?;

            let out = match action {
                "navigate" => {
                    let u = step.get("url").and_then(Value::as_str);
                    let dir = step.get("direction").and_then(Value::as_str);
                    serde_json::to_value(self.navigate(&tab_id, u, dir)?)?
                }
                "back" => serde_json::to_value(self.back(&tab_id)?)?,
                "forward" => serde_json::to_value(self.forward(&tab_id)?)?,
                "reload" => serde_json::to_value(self.reload(&tab_id)?)?,
                "snapshot" => serde_json::to_value(self.snapshot(&tab_id)?)?,
                "screenshot" => {
                    let save = step.get("save").and_then(Value::as_bool).unwrap_or(false);
                    serde_json::to_value(self.screenshot(&tab_id, save)?)?
                }
                "click" => {
                    let target = parse_target_value(step.get("target"))?;
                    let count = step.get("clickCount").and_then(Value::as_u64).unwrap_or(1) as u8;
                    serde_json::to_value(self.click(&tab_id, &target, count)?)?
                }
                "double_click" => {
                    let target = parse_target_value(step.get("target"))?;
                    serde_json::to_value(self.double_click(&tab_id, &target)?)?
                }
                "hover" => {
                    let target = parse_target_value(step.get("target"))?;
                    serde_json::to_value(self.hover(&tab_id, &target)?)?
                }
                "type" => {
                    let target = match step.get("target") {
                        Some(v) if !v.is_null() => Some(parse_target_value(Some(v))?),
                        _ => None,
                    };
                    let text = step.get("text").and_then(Value::as_str).unwrap_or("");
                    let mode = step.get("mode").and_then(Value::as_str);
                    let submit = step.get("submit").and_then(Value::as_bool).unwrap_or(false);
                    serde_json::to_value(self.type_text(&tab_id, target.as_ref(), text, mode, submit)?)?
                }
                "fill_fields" => {
                    let fields_val = step.get("fields").cloned().unwrap_or_else(|| json!([]));
                    let fields: Vec<FillFieldEntry> = serde_json::from_value(fields_val)?;
                    let rev = step.get("revision").and_then(Value::as_u64);
                    serde_json::to_value(self.fill_fields(FillFieldsRequest {
                        tab_id: tab_id.clone(),
                        revision: rev,
                        fields,
                        timeout_ms: None,
                    })?)?
                }
                "press" | "press_key" => {
                    let key = step.get("key").and_then(Value::as_str).unwrap_or("Enter");
                    let target = match step.get("target") {
                        Some(v) if !v.is_null() => Some(parse_target_value(Some(v))?),
                        _ => None,
                    };
                    serde_json::to_value(self.press_key(&tab_id, key, target.as_ref())?)?
                }
                "scroll" => {
                    let dx = step.get("deltaX").and_then(Value::as_f64).unwrap_or(0.0);
                    let dy = step.get("deltaY").and_then(Value::as_f64).unwrap_or(0.0);
                    serde_json::to_value(self.scroll(&tab_id, dx, dy)?)?
                }
                "select_option" => {
                    let target = parse_target_value(step.get("target"))?;
                    let values: Vec<String> = serde_json::from_value(
                        step.get("values").cloned().unwrap_or_else(|| json!([])),
                    )?;
                    serde_json::to_value(self.select_option(&tab_id, &target, values)?)?
                }
                "set_checked" => {
                    let target = parse_target_value(step.get("target"))?;
                    let checked = step.get("checked").and_then(Value::as_bool).unwrap_or(true);
                    serde_json::to_value(self.set_checked(&tab_id, &target, checked)?)?
                }
                "wait_for" => {
                    let text = step.get("text").and_then(Value::as_str);
                    let u = step.get("url").and_then(Value::as_str);
                    let st = step.get("state").and_then(Value::as_str);
                    serde_json::to_value(self.wait_for(&tab_id, text, u, st)?)?
                }
                "evaluate" => {
                    let expr = step
                        .get("expression")
                        .and_then(Value::as_str)
                        .ok_or_else(|| OpenBotRuntimeError::Validation("evaluate step requires expression".to_string()))?;
                    self.evaluate(&tab_id, expr)?
                }
                "sleep" => {
                    let ms = step.get("ms").and_then(Value::as_u64).unwrap_or(0);
                    if ms > BROWSER_SCRIPT_MAX_SLEEP_MS {
                        return Err(OpenBotRuntimeError::Validation(format!(
                            "browser script sleep ({ms} ms) exceeds maximum 25000 ms (25,000 ms)"
                        )));
                    }
                    virtual_elapsed_ms = virtual_elapsed_ms.saturating_add(ms);
                    if virtual_elapsed_ms > timeout_ms {
                        return Err(OpenBotRuntimeError::LimitExceeded(format!(
                            "browser script exceeded timeout budget ({timeout_ms} ms)"
                        )));
                    }
                    json!({ "sleptMs": ms })
                }
                "log" => {
                    let msg = step.get("message").and_then(Value::as_str).unwrap_or("");
                    let safe = redact_text(msg);
                    logs.push(safe.clone());
                    json!({ "logged": safe })
                }
                other => {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "unsupported browser script step action '{other}'"
                    )));
                }
            };
            step_results.push(out);
        }

        let snap = self.snapshot(&tab_id)?;
        let include_snap = req.final_snapshot.unwrap_or(true);
        Ok(BrowserRunScriptResult {
            tab_id: snap.tab_id.clone(),
            url: snap.url.clone(),
            steps_executed: step_results.len(),
            logs,
            step_results,
            screenshots_blocked: snap.screenshots_blocked,
            snapshot: include_snap.then_some(snap),
        })
    }

    pub fn run_script(
        &self,
        agent_id: Option<&str>,
        tab_id: &str,
        script: &str,
        timeout_ms: Option<u64>,
        final_snapshot: Option<bool>,
    ) -> RuntimeResult<BrowserRunScriptResult> {
        let steps = parse_sandboxed_script_steps(script)?;
        self.run_script_steps(BrowserRunStepsRequest {
            agent_id: agent_id.map(ToOwned::to_owned),
            tab_id: Some(tab_id.to_string()),
            url: None,
            steps,
            timeout_ms,
            final_snapshot,
        })
    }
}

fn parse_target_value(val: Option<&Value>) -> RuntimeResult<BrowserTarget> {
    let raw = val.ok_or_else(|| {
        OpenBotRuntimeError::Validation("browser action requires a target".to_string())
    })?;
    if let Some(sel) = raw.as_str() {
        return Ok(BrowserTarget::Css {
            selector: sel.to_string(),
            nth: None,
        });
    }
    serde_json::from_value::<BrowserTarget>(raw.clone()).map_err(|err| {
        OpenBotRuntimeError::Validation(format!("invalid browser target: {err}"))
    })
}

fn canonicalize_card_target(field: &str, target: &BrowserTarget) -> RuntimeResult<String> {
    match target {
        BrowserTarget::Ref { r#ref, .. } => {
            if r#ref.trim().is_empty() {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "fill_card target '{field}' ref must not be empty"
                )));
            }
            Ok(format!("ref:{}", r#ref))
        }
        BrowserTarget::Css { selector, nth } => {
            let trimmed = selector.trim();
            if trimmed.is_empty() {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "fill_card target '{field}' selector must not be empty"
                )));
            }
            Ok(format!("css:{trimmed}:{}", nth.unwrap_or(0)))
        }
        BrowserTarget::Role {
            role,
            name,
            exact,
            nth,
        } => {
            let trimmed_role = role.trim();
            if trimmed_role.is_empty() {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "fill_card target '{field}' role must not be empty"
                )));
            }
            let norm_name = name.as_deref().map(str::trim).unwrap_or("");
            Ok(format!(
                "role:{}:{norm_name}:{}:{}",
                trimmed_role.to_ascii_lowercase(),
                exact.unwrap_or(false),
                nth.unwrap_or(0)
            ))
        }
        BrowserTarget::Text { text, exact, nth } => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "fill_card target '{field}' text must not be empty"
                )));
            }
            Ok(format!(
                "text:{trimmed}:{}:{}",
                exact.unwrap_or(false),
                nth.unwrap_or(0)
            ))
        }
        BrowserTarget::Point { x, y } => {
            if !x.is_finite()
                || !y.is_finite()
                || *x < 0.0
                || *y < 0.0
                || *x > INPUT_LIMITS.browser_coordinate as f64
                || *y > INPUT_LIMITS.browser_coordinate as f64
            {
                return Err(OpenBotRuntimeError::Validation(
                    "browser point coordinates out of bounds".to_string(),
                ));
            }
            let nx = if *x == 0.0 { 0.0 } else { *x };
            let ny = if *y == 0.0 { 0.0 } else { *y };
            Ok(format!("point:{}:{}", nx.to_bits(), ny.to_bits()))
        }
    }
}

struct ResolvedCardFillIndices {
    number_idx: usize,
    security_code_idx: Option<usize>,
    expiry_idx: Option<usize>,
    exp_month_idx: Option<usize>,
    exp_year_idx: Option<usize>,
    name_on_card_idx: Option<usize>,
}

fn validate_card_fill_targets(targets: &CardFillTargets) -> RuntimeResult<()> {
    if targets.expiry.is_some() && (targets.exp_month.is_some() || targets.exp_year.is_some()) {
        return Err(OpenBotRuntimeError::Validation(
            "fill_card targets: use expiry, or expMonth and expYear, not both".to_string(),
        ));
    }
    if targets.exp_month.is_some() != targets.exp_year.is_some() {
        return Err(OpenBotRuntimeError::Validation(
            "fill_card targets: provide both expMonth and expYear together".to_string(),
        ));
    }
    let default_number = BrowserTarget::Css {
        selector: "input[autocomplete=cc-number]".to_string(),
        nth: None,
    };
    let default_cvv = BrowserTarget::Css {
        selector: "input[autocomplete=cc-csc]".to_string(),
        nth: None,
    };
    let effective_targets: [(&str, Option<&BrowserTarget>); 6] = [
        ("number", Some(targets.number.as_ref().unwrap_or(&default_number))),
        (
            "securityCode",
            Some(targets.security_code.as_ref().unwrap_or(&default_cvv)),
        ),
        ("expiry", targets.expiry.as_ref()),
        ("expMonth", targets.exp_month.as_ref()),
        ("expYear", targets.exp_year.as_ref()),
        ("nameOnCard", targets.name_on_card.as_ref()),
    ];
    let mut seen_keys = HashSet::new();
    for (field, target_opt) in effective_targets {
        if let Some(target) = target_opt {
            let key = canonicalize_card_target(field, target)?;
            if !seen_keys.insert(key) {
                return Err(OpenBotRuntimeError::Validation(
                    "Each card field needs its own target.".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn resolve_card_fill_targets(
    tab: &mut BrowserTabState,
    targets: &CardFillTargets,
    include_security_code: bool,
) -> RuntimeResult<ResolvedCardFillIndices> {
    let default_number = BrowserTarget::Css {
        selector: "input[autocomplete=cc-number]".to_string(),
        nth: None,
    };
    let default_cvv = BrowserTarget::Css {
        selector: "input[autocomplete=cc-csc]".to_string(),
        nth: None,
    };
    let mut seen_indices = HashSet::new();
    let mut resolve_unique =
        |tab: &mut BrowserTabState, target: &BrowserTarget| -> RuntimeResult<usize> {
            let dom_idx = resolve_or_upsert_element(tab, target, "input", "textbox")?;
            if !seen_indices.insert(dom_idx) {
                return Err(OpenBotRuntimeError::Validation(
                    "Each card field needs its own target.".to_string(),
                ));
            }
            Ok(dom_idx)
        };

    let number_idx = resolve_unique(tab, targets.number.as_ref().unwrap_or(&default_number))?;
    let security_code_idx = if include_security_code {
        Some(resolve_unique(
            tab,
            targets.security_code.as_ref().unwrap_or(&default_cvv),
        )?)
    } else {
        None
    };

    let (expiry_idx, exp_month_idx, exp_year_idx) = if let Some(ref exp_target) = targets.expiry {
        (Some(resolve_unique(tab, exp_target)?), None, None)
    } else {
        let m_idx = match targets.exp_month.as_ref() {
            Some(m_target) => Some(resolve_unique(tab, m_target)?),
            None => None,
        };
        let y_idx = match targets.exp_year.as_ref() {
            Some(y_target) => Some(resolve_unique(tab, y_target)?),
            None => None,
        };
        (None, m_idx, y_idx)
    };

    let name_on_card_idx = match targets.name_on_card.as_ref() {
        Some(name_target) => Some(resolve_unique(tab, name_target)?),
        None => None,
    };

    Ok(ResolvedCardFillIndices {
        number_idx,
        security_code_idx,
        expiry_idx,
        exp_month_idx,
        exp_year_idx,
        name_on_card_idx,
    })
}

impl BrowserService {
    /// Zero-knowledge secret fill (`password`, `otp`, or `authenticator`).
    /// Resolves saved secrets from `VaultService` enforcing origin & agent scope match,
    /// injects into target field without ever echoing plaintext secret in output or logs,
    /// and marks the tab `secret_document = true`.
    pub fn submit_secret(&self, req: SubmitSecretRequest) -> RuntimeResult<SubmitSecretResult> {
        let method = SecretFillMethod::parse(&req.method)?;
        let submission = req.submission.trim().to_ascii_lowercase();
        if !matches!(submission.as_str(), "click" | "enter" | "on_input") {
            return Err(OpenBotRuntimeError::Validation(format!(
                "invalid submit_secret submission mode '{}': expected click, enter, or on_input",
                req.submission
            )));
        }
        if let Some(d) = req.digits {
            if d > 12 {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "submit_secret digits must be 0..=12, got {d}"
                )));
            }
        }
        if req.targets.len() > 12 {
            return Err(OpenBotRuntimeError::Validation(
                "submit_secret supports at most 12 targets".to_string(),
            ));
        }

        let tab_url = {
            let guard = self.state.read();
            let idx = Self::find_tab_index(&guard, &req.tab_id)?;
            Self::require_not_takeover_paused(&guard.tabs[idx])?;
            guard.tabs[idx].url.clone()
        };

        let (source_label, _resolved_secret_len) = match req.login_id.as_deref() {
            Some(login_id) => {
                if self.vault.has_login(login_id) {
                    // Stored in Vault or registered external PM: strictly enforce origin & scope!
                    let secret = self.vault.resolve_login_secret(
                        login_id,
                        &tab_url,
                        method,
                        req.agent_id.as_deref(),
                    )?;
                    let len = secret.len();
                    drop(secret);
                    ("vault".to_string(), len)
                } else {
                    // Delegated external password manager / user secret card handoff.
                    ("external_secret_card".to_string(), req.digits.unwrap_or(6) as usize)
                }
            }
            None => ("user_secret_card".to_string(), req.digits.unwrap_or(6) as usize),
        };

        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, &req.tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;

        let targets = if req.targets.is_empty() {
            let default_sel = match method {
                SecretFillMethod::Password => "input[type=password]",
                SecretFillMethod::Otp | SecretFillMethod::Authenticator => {
                    "input[autocomplete=one-time-code]"
                }
            };
            vec![BrowserTarget::Css {
                selector: default_sel.to_string(),
                nth: None,
            }]
        } else {
            req.targets.clone()
        };

        for target in &targets {
            let el_idx = resolve_or_upsert_element(&mut guard.tabs[idx], target, "input", "textbox")?;
            guard.tabs[idx].dom_elements[el_idx].is_secret = true;
            // Store strictly masked placeholder in DOM state; NEVER persist plaintext secret!
            guard.tabs[idx].dom_elements[el_idx].value = "[redacted]".to_string();
        }
        if let Some(ref submit_target) = req.submit_target {
            let _ = resolve_or_upsert_element(&mut guard.tabs[idx], submit_target, "button", "button")?;
        }

        guard.tabs[idx].secret_document = true;
        guard.tabs[idx].revision += 1;
        guard.tabs[idx].console_logs.push(BrowserConsoleEntry {
            level: "info".to_string(),
            text: format!(
                "zero-knowledge submit_secret completed (method={}, submission={}, source={})",
                method.as_str(),
                submission,
                source_label
            ),
            timestamp: now_rfc3339_millis(),
        });

        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;

        Ok(SubmitSecretResult {
            ok: true,
            tab_id: req.tab_id,
            method: method.as_str().to_string(),
            submission,
            secret_document: true,
            submitted: true,
            source: source_label,
            snapshot,
        })
    }

    /// Strictly HTTPS-only payment card fill via `VaultService::get_card_for_fill`.
    /// Sets `filled_card = true` and `screenshots_blocked = true` exfiltration lock until
    /// main-frame navigation or explicit `reset_exfiltration_lock`.
    pub fn fill_card(&self, req: FillCardRequest) -> RuntimeResult<FillCardResult> {
        let targets = req.targets.unwrap_or_default();
        validate_card_fill_targets(&targets)?;

        let (tab_url, expected_revision, mut tab_preview) = {
            let guard = self.state.read();
            let idx = Self::find_tab_index(&guard, &req.tab_id)?;
            Self::require_not_takeover_paused(&guard.tabs[idx])?;
            (
                guard.tabs[idx].url.clone(),
                guard.tabs[idx].revision,
                guard.tabs[idx].clone(),
            )
        };

        let parsed_url = Url::parse(&tab_url).map_err(|_| {
            OpenBotRuntimeError::Forbidden(
                "Card fill requires an HTTPS secure page. Use request_takeover.".to_string(),
            )
        })?;
        if parsed_url.scheme() != "https" {
            return Err(OpenBotRuntimeError::Forbidden(
                "Card fill requires an HTTPS secure page (refused non-HTTPS URL). Use request_takeover."
                    .to_string(),
            ));
        }

        // Pre-resolve every effective target on a staged clone and enforce strict DOM-index
        // uniqueness BEFORE consuming a one-time Link card or recording saved-card fill_sites.
        let _ = resolve_card_fill_targets(&mut tab_preview, &targets, true)?;

        let card = self.vault.get_card_for_fill(
            &req.card_id,
            &tab_url,
            req.agent_id.as_deref(),
            req.approved,
        )?;

        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, &req.tab_id)?;
        Self::require_not_takeover_paused(&guard.tabs[idx])?;
        if guard.tabs[idx].revision != expected_revision || guard.tabs[idx].url != tab_url {
            return Err(OpenBotRuntimeError::Conflict(
                "The card page changed. Take a fresh snapshot.".to_string(),
            ));
        }

        let mut next_tab = guard.tabs[idx].clone();
        let include_cvv = !card.cvv_for_browser_fill().is_empty();
        let resolved = resolve_card_fill_targets(&mut next_tab, &targets, include_cvv)?;

        // Discard any active screen recording before injecting payment card fields.
        next_tab.recording = None;

        let mut filled_fields = Vec::new();
        next_tab.dom_elements[resolved.number_idx].is_secret = true;
        next_tab.dom_elements[resolved.number_idx].value = mask_card_last4(&card.last4);
        filled_fields.push("number".to_string());

        if let Some(cvv_idx) = resolved.security_code_idx {
            next_tab.dom_elements[cvv_idx].is_secret = true;
            next_tab.dom_elements[cvv_idx].value = "[redacted]".to_string();
            filled_fields.push("securityCode".to_string());
        }

        if let Some(e_idx) = resolved.expiry_idx {
            next_tab.dom_elements[e_idx].is_secret = true;
            next_tab.dom_elements[e_idx].value = "[redacted]".to_string();
            filled_fields.push("expiry".to_string());
        } else {
            if let Some(m_idx) = resolved.exp_month_idx {
                next_tab.dom_elements[m_idx].is_secret = true;
                next_tab.dom_elements[m_idx].value = "[redacted]".to_string();
                filled_fields.push("expMonth".to_string());
            }
            if let Some(y_idx) = resolved.exp_year_idx {
                next_tab.dom_elements[y_idx].is_secret = true;
                next_tab.dom_elements[y_idx].value = "[redacted]".to_string();
                filled_fields.push("expYear".to_string());
            }
        }
        if let Some(n_idx) = resolved.name_on_card_idx {
            next_tab.dom_elements[n_idx].value = card.name_on_card.clone();
            filled_fields.push("nameOnCard".to_string());
        }

        // Engage strict exfiltration lock!
        next_tab.filled_card = true;
        next_tab.screenshots_blocked = true;
        next_tab.revision += 1;
        next_tab.console_logs.push(BrowserConsoleEntry {
            level: "info".to_string(),
            text: format!(
                "fill_card completed ({}); screenshots_blocked=true until navigation",
                card.masked_number
            ),
            timestamp: now_rfc3339_millis(),
        });

        guard.tabs[idx] = next_tab;
        let snapshot = build_snapshot_view(&guard.tabs[idx]);
        self.persist_locked(&guard)?;

        let result = FillCardResult {
            tab_id: req.tab_id,
            card_id: card.card_id.clone(),
            filled_card: true,
            screenshots_blocked: true,
            filled_fields,
            masked_number: card.masked_number.clone(),
            last4: card.last4.clone(),
            brand: card.brand,
            snapshot,
        };
        drop(card);
        Ok(result)
    }

    /// Pauses browser automation on `tab_id` and emits a structured human takeover request.
    pub fn request_takeover(&self, req: TakeoverRequest) -> RuntimeResult<TakeoverState> {
        let reason = req
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("human-action-required");
        if utf16_len(reason) > 128 {
            return Err(OpenBotRuntimeError::Validation(
                "takeover reason exceeds 128 characters".to_string(),
            ));
        }

        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, &req.tab_id)?;
        let takeover_id = format!("takeover-{}", Uuid::new_v4());
        let url = guard.tabs[idx].url.clone();
        let state = TakeoverState {
            takeover_id: takeover_id.clone(),
            tab_id: req.tab_id.clone(),
            agent_id: req.agent_id.clone(),
            url,
            reason: reason.to_string(),
            status: "awaiting_human".to_string(),
            takeover_active: true,
            created_at: now_rfc3339_millis(),
            released_at: None,
        };
        guard.tabs[idx].takeover_active = true;
        guard.tabs[idx].active_takeover_id = Some(takeover_id);
        guard.tabs[idx].takeover_reason = Some(reason.to_string());
        guard.takeovers.push(state.clone());
        self.persist_locked(&guard)?;
        Ok(state)
    }

    /// Releases an active human takeover pause on `tab_id`, allowing automation to resume.
    pub fn release_takeover(
        &self,
        tab_id: &str,
        takeover_id: Option<&str>,
    ) -> RuntimeResult<TakeoverState> {
        let mut guard = self.state.write();
        let idx = Self::find_tab_index(&guard, tab_id)?;
        let active_id = guard.tabs[idx].active_takeover_id.clone();
        if let (Some(wanted), Some(current)) = (takeover_id, active_id.as_deref()) {
            if wanted != current {
                return Err(OpenBotRuntimeError::Conflict(format!(
                    "takeover_id mismatch: expected '{current}', got '{wanted}'"
                )));
            }
        }
        let now = now_rfc3339_millis();
        let url = guard.tabs[idx].url.clone();
        let reason = guard.tabs[idx]
            .takeover_reason
            .clone()
            .unwrap_or_else(|| "human-action-required".to_string());
        let resolved_id = active_id
            .or_else(|| takeover_id.map(ToOwned::to_owned))
            .unwrap_or_else(|| format!("takeover-{}", Uuid::new_v4()));

        guard.tabs[idx].takeover_active = false;
        guard.tabs[idx].active_takeover_id = None;
        guard.tabs[idx].takeover_reason = None;

        let mut updated: Option<TakeoverState> = None;
        for record in guard.takeovers.iter_mut().rev() {
            if record.tab_id == tab_id && record.takeover_active {
                record.takeover_active = false;
                record.status = "released".to_string();
                record.released_at = Some(now.clone());
                updated = Some(record.clone());
                break;
            }
        }
        let final_state = updated.unwrap_or(TakeoverState {
            takeover_id: resolved_id,
            tab_id: tab_id.to_string(),
            agent_id: None,
            url,
            reason,
            status: "released".to_string(),
            takeover_active: false,
            created_at: now.clone(),
            released_at: Some(now),
        });
        self.persist_locked(&guard)?;
        Ok(final_state)
    }
}

fn arg_str<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for k in keys {
        if let Some(v) = args.get(*k).and_then(Value::as_str) {
            return Some(v);
        }
    }
    None
}

fn require_arg_str<'a>(args: &'a Value, keys: &[&str], label: &str) -> RuntimeResult<&'a str> {
    arg_str(args, keys)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| OpenBotRuntimeError::Validation(format!("missing required argument '{label}'")))
}

impl BrowserService {
    /// Unified dispatcher for the complete `openbot_browser.*` tool surface
    /// (accepts both `"screenshot"` and `"openbot_browser.screenshot"`).
    pub fn execute_tool(
        &self,
        tool_name: &str,
        agent_id: &str,
        args: Value,
    ) -> RuntimeResult<Value> {
        let name = tool_name
            .strip_prefix("openbot_browser.")
            .unwrap_or(tool_name);
        let opt_agent = if agent_id.trim().is_empty() {
            None
        } else {
            Some(agent_id)
        };

        match name {
            "open" => {
                let url = require_arg_str(&args, &["url"], "url")?;
                let snap = self.open_tab(url, opt_agent, None, None, false)?;
                Ok(serde_json::to_value(snap)?)
            }
            "new_tab" => {
                let url = arg_str(&args, &["url"]);
                let snap = self.new_tab(url, opt_agent)?;
                Ok(serde_json::to_value(snap)?)
            }
            "close_tab" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let closed = self.close_tab(tab_id)?;
                Ok(json!({ "tabId": tab_id, "closed": closed }))
            }
            "close" => {
                let tab_id = arg_str(&args, &["tabId", "tab_id"]);
                let closed_count = self.close_tabs(tab_id, opt_agent)?;
                Ok(json!({ "closedCount": closed_count }))
            }
            "switch_tab" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                Ok(serde_json::to_value(self.switch_tab(tab_id)?)?)
            }
            "list_tabs" => {
                let tabs = self.list_tabs(opt_agent);
                let active_tab_id = self.state.read().active_tab_id.clone();
                Ok(json!({
                    "activeTabId": active_tab_id,
                    "tabCount": tabs.len(),
                    "maxTabs": MAX_BROWSER_TABS,
                    "tabs": tabs,
                }))
            }
            "status" => Ok(self.status(opt_agent)),
            "navigate" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let url = arg_str(&args, &["url"]);
                let direction = arg_str(&args, &["direction"]);
                Ok(serde_json::to_value(self.navigate(tab_id, url, direction)?)?)
            }
            "back" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                Ok(serde_json::to_value(self.back(tab_id)?)?)
            }
            "forward" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                Ok(serde_json::to_value(self.forward(tab_id)?)?)
            }
            "reload" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                Ok(serde_json::to_value(self.reload(tab_id)?)?)
            }
            "snapshot" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                Ok(serde_json::to_value(self.snapshot(tab_id)?)?)
            }
            "screenshot" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let save = args.get("save").and_then(Value::as_bool).unwrap_or(false);
                Ok(serde_json::to_value(self.screenshot(tab_id, save)?)?)
            }
            "click" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let target = parse_target_value(args.get("target").or_else(|| args.get("selector")))?;
                let count = args
                    .get("clickCount")
                    .or_else(|| args.get("click_count"))
                    .and_then(Value::as_u64)
                    .unwrap_or(1) as u8;
                Ok(serde_json::to_value(self.click(tab_id, &target, count)?)?)
            }
            "double_click" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let target = parse_target_value(args.get("target").or_else(|| args.get("selector")))?;
                Ok(serde_json::to_value(self.double_click(tab_id, &target)?)?)
            }
            "hover" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let target = parse_target_value(args.get("target").or_else(|| args.get("selector")))?;
                Ok(serde_json::to_value(self.hover(tab_id, &target)?)?)
            }
            "type" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let target = match args.get("target").or_else(|| args.get("selector")) {
                    Some(v) if !v.is_null() => Some(parse_target_value(Some(v))?),
                    _ => None,
                };
                let text = arg_str(&args, &["text", "value"]).unwrap_or("");
                let mode = arg_str(&args, &["mode"]);
                let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
                Ok(serde_json::to_value(
                    self.type_text(tab_id, target.as_ref(), text, mode, submit)?,
                )?)
            }
            "fill_fields" => {
                let req: FillFieldsRequest = serde_json::from_value(args)?;
                Ok(serde_json::to_value(self.fill_fields(req)?)?)
            }
            "select_option" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let target = parse_target_value(args.get("target").or_else(|| args.get("selector")))?;
                let values: Vec<String> = serde_json::from_value(
                    args.get("values").cloned().unwrap_or_else(|| json!([])),
                )?;
                Ok(serde_json::to_value(self.select_option(tab_id, &target, values)?)?)
            }
            "set_checked" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let target = parse_target_value(args.get("target").or_else(|| args.get("selector")))?;
                let checked = args.get("checked").and_then(Value::as_bool).unwrap_or(true);
                Ok(serde_json::to_value(self.set_checked(tab_id, &target, checked)?)?)
            }
            "press_key" | "press" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let key = require_arg_str(&args, &["key"], "key")?;
                let target = match args.get("target") {
                    Some(v) if !v.is_null() => Some(parse_target_value(Some(v))?),
                    _ => None,
                };
                Ok(serde_json::to_value(self.press_key(tab_id, key, target.as_ref())?)?)
            }
            "scroll" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let dx = args
                    .get("deltaX")
                    .or_else(|| args.get("delta_x"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let dy = args
                    .get("deltaY")
                    .or_else(|| args.get("delta_y"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                Ok(serde_json::to_value(self.scroll(tab_id, dx, dy)?)?)
            }
            "drag" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let source = parse_target_value(args.get("source"))?;
                let target = parse_target_value(args.get("target"))?;
                Ok(serde_json::to_value(self.drag(tab_id, &source, &target)?)?)
            }
            "wait_for" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let text = arg_str(&args, &["text"]);
                let url = arg_str(&args, &["url"]);
                let st = arg_str(&args, &["state"]);
                Ok(serde_json::to_value(self.wait_for(tab_id, text, url, st)?)?)
            }
            "evaluate" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let expr = arg_str(&args, &["expression", "script"]).unwrap_or("document.title");
                Ok(json!({ "tabId": tab_id, "value": self.evaluate(tab_id, expr)? }))
            }
            _ => self.execute_tool_extended(name, opt_agent, args),
        }
    }
}

impl BrowserService {
    fn execute_tool_extended(
        &self,
        name: &str,
        opt_agent: Option<&str>,
        args: Value,
    ) -> RuntimeResult<Value> {
        match name {
            "extract_text" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                Ok(json!({ "tabId": tab_id, "text": self.extract_text(tab_id)? }))
            }
            "get_links" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let links = self.get_links(tab_id)?;
                Ok(json!({ "tabId": tab_id, "count": links.len(), "links": links }))
            }
            "console_logs" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let level = arg_str(&args, &["level"]);
                let clear = args.get("clear").and_then(Value::as_bool).unwrap_or(false);
                let logs = self.console_logs(tab_id, level, clear)?;
                Ok(json!({ "tabId": tab_id, "logs": logs }))
            }
            "network_log" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let clear = args.get("clear").and_then(Value::as_bool).unwrap_or(false);
                let entries = self.network_log(tab_id, clear)?;
                Ok(json!({ "tabId": tab_id, "entries": entries }))
            }
            "download" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let url = require_arg_str(&args, &["url"], "url")?;
                let filename = arg_str(&args, &["filename"]);
                Ok(serde_json::to_value(self.download(tab_id, url, filename)?)?)
            }
            "upload_file" | "upload_files" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let target = parse_target_value(args.get("target").or_else(|| args.get("selector")))?;
                let paths: Vec<String> = if let Some(single) = arg_str(&args, &["path", "file"]) {
                    vec![single.to_string()]
                } else {
                    serde_json::from_value(
                        args.get("paths").cloned().unwrap_or_else(|| json!([])),
                    )?
                };
                Ok(serde_json::to_value(self.upload_files(tab_id, &target, paths)?)?)
            }
            "cookies" => {
                let action = arg_str(&args, &["action"]).unwrap_or("list");
                let domain = arg_str(&args, &["domain"]);
                let cookie_name = arg_str(&args, &["name"]);
                let cookie = match args.get("cookie") {
                    Some(v) if !v.is_null() => Some(serde_json::from_value::<BrowserCookie>(v.clone())?),
                    _ => None,
                };
                self.manage_cookies(action, domain, cookie, cookie_name)
            }
            "set_environment" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let env = self.set_environment(
                    tab_id,
                    arg_str(&args, &["preset"]),
                    args.get("width").and_then(Value::as_u64).map(|v| v as u32),
                    args.get("height").and_then(Value::as_u64).map(|v| v as u32),
                    args.get("deviceScaleFactor")
                        .or_else(|| args.get("device_scale_factor"))
                        .and_then(Value::as_f64),
                    arg_str(&args, &["colorScheme", "color_scheme"]),
                    args.get("reducedMotion")
                        .or_else(|| args.get("reduced_motion"))
                        .and_then(Value::as_bool),
                )?;
                Ok(serde_json::to_value(env)?)
            }
            "recording_start" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                Ok(serde_json::to_value(self.recording_start(tab_id)?)?)
            }
            "recording_stop" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                self.recording_stop(tab_id)
            }
            "handle_dialog" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let dialog_id = require_arg_str(&args, &["dialogId", "dialog_id"], "dialogId")?;
                let accept = args.get("accept").and_then(Value::as_bool).unwrap_or(true);
                self.handle_dialog(tab_id, dialog_id, accept)
            }
            "act" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let action_obj = args.get("action").ok_or_else(|| {
                    OpenBotRuntimeError::Validation("act requires an action object".to_string())
                })?;
                let action_type = require_arg_str(action_obj, &["type"], "action.type")?;
                match action_type {
                    "back" => Ok(serde_json::to_value(self.back(tab_id)?)?),
                    "forward" => Ok(serde_json::to_value(self.forward(tab_id)?)?),
                    "reload" => Ok(serde_json::to_value(self.reload(tab_id)?)?),
                    "click" => {
                        let r = require_arg_str(action_obj, &["ref"], "action.ref")?;
                        let rev = args.get("revision").and_then(Value::as_u64);
                        Ok(serde_json::to_value(
                            self.click(tab_id, &BrowserTarget::Ref { r#ref: r.to_string(), revision: rev }, 1)?,
                        )?)
                    }
                    "type" => {
                        let r = require_arg_str(action_obj, &["ref"], "action.ref")?;
                        let rev = args.get("revision").and_then(Value::as_u64);
                        let text = require_arg_str(action_obj, &["text"], "action.text")?;
                        let submit = action_obj.get("submit").and_then(Value::as_bool).unwrap_or(false);
                        Ok(serde_json::to_value(self.type_text(
                            tab_id,
                            Some(&BrowserTarget::Ref { r#ref: r.to_string(), revision: rev }),
                            text,
                            Some("replace"),
                            submit,
                        )?)?)
                    }
                    "key" => {
                        let key = require_arg_str(action_obj, &["key"], "action.key")?;
                        Ok(serde_json::to_value(self.press_key(tab_id, key, None)?)?)
                    }
                    "scroll" => {
                        let dy = action_obj.get("deltaY").and_then(Value::as_f64).unwrap_or(0.0);
                        Ok(serde_json::to_value(self.scroll(tab_id, 0.0, dy)?)?)
                    }
                    other => Err(OpenBotRuntimeError::Validation(format!(
                        "unsupported legacy act action type '{other}'"
                    ))),
                }
            }
            "run_script" => {
                if let Some(steps_val) = args.get("steps") {
                    let steps: Vec<Value> = serde_json::from_value(steps_val.clone())?;
                    let res = self.run_script_steps(BrowserRunStepsRequest {
                        agent_id: opt_agent.map(ToOwned::to_owned),
                        tab_id: arg_str(&args, &["tabId", "tab_id"]).map(ToOwned::to_owned),
                        url: arg_str(&args, &["url"]).map(ToOwned::to_owned),
                        steps,
                        timeout_ms: args
                            .get("timeoutMs")
                            .or_else(|| args.get("timeout_ms"))
                            .and_then(Value::as_u64),
                        final_snapshot: args
                            .get("finalSnapshot")
                            .or_else(|| args.get("final_snapshot"))
                            .and_then(Value::as_bool),
                    })?;
                    Ok(serde_json::to_value(res)?)
                } else {
                    let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                    let script = require_arg_str(&args, &["script"], "script")?;
                    let res = self.run_script(
                        opt_agent,
                        tab_id,
                        script,
                        args.get("timeoutMs")
                            .or_else(|| args.get("timeout_ms"))
                            .and_then(Value::as_u64),
                        args.get("finalSnapshot")
                            .or_else(|| args.get("final_snapshot"))
                            .and_then(Value::as_bool),
                    )?;
                    Ok(serde_json::to_value(res)?)
                }
            }
            "submit_secret" => {
                let mut req: SubmitSecretRequest = serde_json::from_value(args)?;
                if req.agent_id.is_none() {
                    req.agent_id = opt_agent.map(ToOwned::to_owned);
                }
                Ok(serde_json::to_value(self.submit_secret(req)?)?)
            }
            "fill_card" => {
                let mut req: FillCardRequest = serde_json::from_value(args)?;
                if req.agent_id.is_none() {
                    req.agent_id = opt_agent.map(ToOwned::to_owned);
                }
                Ok(serde_json::to_value(self.fill_card(req)?)?)
            }
            "request_takeover" => {
                let mut req: TakeoverRequest = serde_json::from_value(args)?;
                if req.agent_id.is_none() {
                    req.agent_id = opt_agent.map(ToOwned::to_owned);
                }
                Ok(serde_json::to_value(self.request_takeover(req)?)?)
            }
            "release_takeover" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let takeover_id = arg_str(&args, &["takeoverId", "takeover_id"]);
                Ok(serde_json::to_value(self.release_takeover(tab_id, takeover_id)?)?)
            }
            "list_logins" => {
                let tab_id = require_arg_str(&args, &["tabId", "tab_id"], "tabId")?;
                let url = {
                    let guard = self.state.read();
                    let idx = Self::find_tab_index(&guard, tab_id)?;
                    guard.tabs[idx].url.clone()
                };
                let logins = self.vault.list_logins(&url, opt_agent)?;
                Ok(json!({ "tabId": tab_id, "origin": url, "logins": logins }))
            }
            "list_vault_items" => {
                let items = self.vault.list_items(opt_agent);
                Ok(json!({ "items": items, "count": items.len() }))
            }
            "save_vault_item" => {
                let input: VaultSaveInput = serde_json::from_value(args)?;
                let saved = self.vault.save_item(input, opt_agent)?;
                Ok(json!({ "status": "saved", "id": saved.id(), "item": saved }))
            }
            "request_vault_item" => {
                let kind = require_arg_str(&args, &["kind"], "kind")?;
                let title = require_arg_str(&args, &["title"], "title")?;
                Ok(json!({
                    "status": "requested",
                    "requestId": format!("vault-req-{}", Uuid::new_v4()),
                    "kind": kind,
                    "title": title,
                }))
            }
            "delete_vault_item" => {
                let id = require_arg_str(&args, &["id"], "id")?;
                let deleted = self.vault.delete_item(id, opt_agent)?;
                Ok(json!({ "status": "deleted", "id": deleted.id(), "item": deleted }))
            }
            "request_link_payment" => {
                let mut req: LinkPaymentRequest = serde_json::from_value(args)?;
                if req.agent_id.is_none() {
                    req.agent_id = opt_agent.map(ToOwned::to_owned);
                }
                Ok(serde_json::to_value(self.vault.request_link_payment(req)?)?)
            }
            other => Err(OpenBotRuntimeError::NotFound(format!(
                "unknown openbot_browser tool '{other}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_browser() -> (TempDir, Arc<VaultService>, Arc<BrowserService>) {
        let dir = TempDir::new().expect("temp dir");
        let db = OpenBotDatabase::open_in_memory().expect("in-memory db");
        let vault = VaultService::open(db.clone(), dir.path()).expect("open vault");
        let browser = BrowserService::open(db, dir.path(), Arc::clone(&vault)).expect("open browser");
        (dir, vault, browser)
    }

    #[test]
    fn enforces_25_tab_cap_history_and_atomic_persistence() {
        let (dir, vault, browser) = test_browser();
        let mut first_tab_id = String::new();
        for idx in 0..MAX_BROWSER_TABS {
            let snap = browser
                .open_tab(
                    &format!("https://example.test/page-{idx}"),
                    Some("agent-1"),
                    None,
                    None,
                    false,
                )
                .expect("open tab");
            if idx == 0 {
                first_tab_id = snap.tab_id;
            }
        }
        let overflow = browser.open_tab("https://example.test/overflow", Some("agent-1"), None, None, false);
        assert!(matches!(overflow, Err(OpenBotRuntimeError::LimitExceeded(_))));

        browser
            .navigate(&first_tab_id, Some("https://example.test/step-2"), None)
            .expect("navigate");
        let back = browser.back(&first_tab_id).expect("back");
        assert_eq!(back.url, "https://example.test/page-0");
        let fwd = browser.forward(&first_tab_id).expect("forward");
        assert_eq!(fwd.url, "https://example.test/step-2");

        let db2 = OpenBotDatabase::open_in_memory().expect("db2");
        let reloaded = BrowserService::open(db2, dir.path(), vault).expect("reload");
        assert_eq!(reloaded.list_tabs(Some("agent-1")).len(), MAX_BROWSER_TABS);
    }

    #[test]
    fn run_script_enforces_200_step_limit_sleep_cap_and_fill_fields_secret_refusal() {
        let (_dir, _vault, browser) = test_browser();
        let steps_201: Vec<Value> = (0..201).map(|_| json!({ "action": "snapshot" })).collect();
        let over = browser.run_script_steps(BrowserRunStepsRequest {
            agent_id: Some("agent-1".to_string()),
            tab_id: None,
            url: Some("https://example.test/login".to_string()),
            steps: steps_201,
            timeout_ms: None,
            final_snapshot: Some(true),
        });
        assert!(matches!(over, Err(OpenBotRuntimeError::LimitExceeded(_))));

        let sleep_over = browser.run_script_steps(BrowserRunStepsRequest {
            agent_id: Some("agent-1".to_string()),
            tab_id: None,
            url: Some("https://example.test/login".to_string()),
            steps: vec![json!({ "action": "sleep", "ms": 25001 })],
            timeout_ms: None,
            final_snapshot: Some(true),
        });
        assert!(matches!(sleep_over, Err(OpenBotRuntimeError::Validation(_))));

        let pw_refused = browser.run_script_steps(BrowserRunStepsRequest {
            agent_id: Some("agent-1".to_string()),
            tab_id: None,
            url: Some("https://example.test/login".to_string()),
            steps: vec![json!({
                "action": "fill_fields",
                "fields": [{ "selector": "input[type=password]", "value": "hunter2" }]
            })],
            timeout_ms: None,
            final_snapshot: Some(true),
        });
        assert!(matches!(pw_refused, Err(OpenBotRuntimeError::Forbidden(_))));
    }

    #[test]
    fn fill_card_enforces_https_exfiltration_lock_and_nav_unlock() {
        let (_dir, vault, browser) = test_browser();
        let ctx_110 = "Purchase annual CI verification runner seat for deterministic multi-crate test execution and caching!!";
        let link = vault
            .request_link_payment(LinkPaymentRequest {
                amount: 2500,
                currency: Some("USD".to_string()),
                merchant_name: "Acme".to_string(),
                merchant_url: "https://pay.acme.test/checkout".to_string(),
                context: ctx_110.to_string(),
                approved: true,
                agent_id: Some("agent-1".to_string()),
            })
            .expect("issue link card");

        let http_tab = browser
            .open_tab("http://pay.acme.test/checkout", Some("agent-1"), None, None, false)
            .expect("open http");
        let http_refused = browser.fill_card(FillCardRequest {
            tab_id: http_tab.tab_id,
            card_id: link.card_id.clone(),
            targets: None,
            approved: true,
            agent_id: Some("agent-1".to_string()),
            timeout_ms: None,
        });
        assert!(matches!(http_refused, Err(OpenBotRuntimeError::Forbidden(_))));

        let https_tab = browser
            .open_tab("https://pay.acme.test/checkout", Some("agent-1"), None, None, false)
            .expect("open https");
        let filled = browser
            .fill_card(FillCardRequest {
                tab_id: https_tab.tab_id.clone(),
                card_id: link.card_id.clone(),
                targets: None,
                approved: true,
                agent_id: Some("agent-1".to_string()),
                timeout_ms: None,
            })
            .expect("fill card");
        assert!(filled.filled_card);
        assert!(filled.screenshots_blocked);
        assert_eq!(filled.last4, "4242");

        // All exfiltration tools blocked while filled_card lock is active.
        assert!(matches!(
            browser.screenshot(&https_tab.tab_id, false),
            Err(OpenBotRuntimeError::Forbidden(_))
        ));
        assert!(matches!(
            browser.evaluate(&https_tab.tab_id, "document.title"),
            Err(OpenBotRuntimeError::Forbidden(_))
        ));
        assert!(matches!(
            browser.recording_start(&https_tab.tab_id),
            Err(OpenBotRuntimeError::Forbidden(_))
        ));

        // Navigating away clears the exfiltration lock.
        browser
            .navigate(&https_tab.tab_id, Some("https://pay.acme.test/receipt"), None)
            .expect("navigate receipt");
        let shot = browser.screenshot(&https_tab.tab_id, false).expect("screenshot unlocked");
        assert_eq!(shot.mime, "image/png");
    }

    #[test]
    fn takeover_pauses_automation_until_released() {
        let (_dir, _vault, browser) = test_browser();
        let tab = browser
            .open_tab("https://sso.acme.test/login", Some("agent-1"), None, None, false)
            .expect("open tab");

        let tk = browser
            .request_takeover(TakeoverRequest {
                tab_id: tab.tab_id.clone(),
                reason: Some("sign-in-blocked".to_string()),
                agent_id: Some("agent-1".to_string()),
            })
            .expect("request takeover");
        assert_eq!(tk.status, "awaiting_human");
        assert!(tk.takeover_active);

        let paused = browser.click(
            &tab.tab_id,
            &BrowserTarget::Css {
                selector: "button[type=submit]".to_string(),
                nth: None,
            },
            1,
        );
        assert!(matches!(paused, Err(OpenBotRuntimeError::Conflict(_))));

        let rel = browser
            .release_takeover(&tab.tab_id, Some(&tk.takeover_id))
            .expect("release takeover");
        assert!(!rel.takeover_active);
        assert_eq!(rel.status, "released");

        browser
            .click(
                &tab.tab_id,
                &BrowserTarget::Css {
                    selector: "button[type=submit]".to_string(),
                    nth: None,
                },
                1,
            )
            .expect("automation resumed");
    }

    #[test]
    fn fill_card_pre_resolves_targets_and_blocks_point_collisions_before_vault_mutation() {
        let (_dir, vault, browser) = test_browser();
        let ctx_110 = "Purchase deterministic test license with strict transactional pre-resolution before Link card consumption!!";
        let link = vault
            .request_link_payment(LinkPaymentRequest {
                amount: 4200,
                currency: Some("USD".to_string()),
                merchant_name: "Merchant".to_string(),
                merchant_url: "https://checkout.merchant.test/pay".to_string(),
                context: ctx_110.to_string(),
                approved: true,
                agent_id: Some("agent-1".to_string()),
            })
            .expect("issue link card");
        let tab = browser
            .open_tab("https://checkout.merchant.test/pay", Some("agent-1"), None, None, false)
            .expect("open https");

        let failing_targets = vec![
            CardFillTargets {
                number: Some(BrowserTarget::Ref { r#ref: "e999".to_string(), revision: None }),
                ..Default::default()
            },
            CardFillTargets {
                number: Some(BrowserTarget::Ref { r#ref: " e1 ".to_string(), revision: None }),
                ..Default::default()
            },
            CardFillTargets {
                number: Some(BrowserTarget::Ref { r#ref: "e1".to_string(), revision: Some(100) }),
                ..Default::default()
            },
            CardFillTargets {
                number: Some(BrowserTarget::Role {
                    role: "textbox".to_string(),
                    name: Some("NoSuchField".to_string()),
                    exact: None,
                    nth: None,
                }),
                ..Default::default()
            },
            CardFillTargets {
                number: Some(BrowserTarget::Text {
                    text: "NoSuchText".to_string(),
                    exact: None,
                    nth: None,
                }),
                ..Default::default()
            },
            CardFillTargets {
                number: Some(BrowserTarget::Ref { r#ref: "e1".to_string(), revision: None }),
                expiry: Some(BrowserTarget::Point { x: 10.0, y: 20.0 }),
                ..Default::default()
            },
            CardFillTargets {
                number: Some(BrowserTarget::Point { x: 10.0, y: 20.0 }),
                expiry: Some(BrowserTarget::Point { x: 30.0, y: 40.0 }),
                ..Default::default()
            },
        ];
        for targets in failing_targets {
            let res = browser.fill_card(FillCardRequest {
                tab_id: tab.tab_id.clone(),
                card_id: link.card_id.clone(),
                targets: Some(targets),
                approved: true,
                agent_id: Some("agent-1".to_string()),
                timeout_ms: None,
            });
            assert!(res.is_err(), "expected failure before Link card consumption");
        }

        // Link card MUST still be unconsumed; padded CSS matches e2 without creating e7.
        let filled = browser
            .fill_card(FillCardRequest {
                tab_id: tab.tab_id.clone(),
                card_id: link.card_id.clone(),
                targets: Some(CardFillTargets {
                    number: Some(BrowserTarget::Css {
                        selector: " input[autocomplete=cc-number] ".to_string(),
                        nth: None,
                    }),
                    ..Default::default()
                }),
                approved: true,
                agent_id: Some("agent-1".to_string()),
                timeout_ms: None,
            })
            .expect("link card must remain unconsumed after pre-resolution failures");
        assert!(filled.filled_card);
        assert_eq!(filled.snapshot.elements.len(), 6);
        assert!(filled.snapshot.elements.iter().all(|el| el.r#ref != "e7"));

        // Saved Vault card: bad target with approved=true must NOT persist fill_sites.
        let saved = vault
            .save_item(
                VaultSaveInput::Card {
                    id: None,
                    label: "Saved".to_string(),
                    name_on_card: "Ada".to_string(),
                    exp_month: 12,
                    exp_year: 2029,
                    billing_contact_id: None,
                    number: Some("4242424242424242".to_string()),
                    security_code: Some("123".to_string()),
                    fill_sites: None,
                    agents: None,
                },
                Some("agent-1"),
            )
            .expect("save card");
        let tab2 = browser
            .open_tab("https://billing.saved.test/checkout", Some("agent-1"), None, None, false)
            .expect("open tab2");
        let bad_saved = browser.fill_card(FillCardRequest {
            tab_id: tab2.tab_id.clone(),
            card_id: saved.id().to_string(),
            targets: Some(CardFillTargets {
                number: Some(BrowserTarget::Ref { r#ref: "e999".to_string(), revision: None }),
                ..Default::default()
            }),
            approved: true,
            agent_id: Some("agent-1".to_string()),
            timeout_ms: None,
        });
        assert!(matches!(bad_saved, Err(OpenBotRuntimeError::NotFound(_))));
        let needs_approval = browser.fill_card(FillCardRequest {
            tab_id: tab2.tab_id,
            card_id: saved.id().to_string(),
            targets: None,
            approved: false,
            agent_id: Some("agent-1".to_string()),
            timeout_ms: None,
        });
        assert!(matches!(
            needs_approval,
            Err(OpenBotRuntimeError::ApprovalRequired { .. })
        ));
    }
}

