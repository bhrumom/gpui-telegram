//! Agent-ergonomic `--json` `fabushi` CLI commands for Orca Bot OS, Multi-Agent
//! Orchestration, Webhooks/Automations, Worktrees, and Agent IDE (`PROJECT.md §3`).
//!
//! Invariants enforced here:
//! - Direct structured JSON on `stdout` on exit `0` (zero human banners/logs).
//! - Structured `{"error": {"code": "...", "message": "..."}}` on `stderr` on failure.
//! - Exit code `3` (`PLUGIN_APPROVAL_REQUIRED_EXIT_CODE`) whenever `code == "approval_required"`.
//! - Defense-in-depth ZERO-LEAK sanitization for webhooks (never prints `ingest_url`,
//!   `secret_key`, `copy_card`, `orcawh_`, `whsec_`, `whk_`) and artifacts (never `edit_token`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use orca_core::orchestration_protocol::PLUGIN_APPROVAL_REQUIRED_EXIT_CODE;
use orca_core::triggers_webhooks::{redact_webhook_keys, REDACTED_WEBHOOK_KEY};
use reqwest::{Client, Method};
use serde_json::{json, Value};

use crate::cli_parser::{
    split_flag_eq, try_consume_global, GlobalConsumeResult, GlobalOptions, OutputFormat,
};
use crate::envelope::{format_json, CliOutcome};
use crate::error::CliError;
use crate::ipc_client::FabushiBackendClient;
use crate::unified_parser::{UnifiedCommand, UnifiedParseOutcome};

/// Parsed Orca CLI invocation routed via `UnifiedCommand::Orca`.
#[derive(Debug, Clone, PartialEq)]
pub struct OrcaCliCommand {
    pub namespace: String,
    pub subcommand: String,
    pub args: OrcaArgBag,
}

/// Collected positional arguments, multi-value flags, boolean flags, and ordered flag sequence.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OrcaArgBag {
    pub positionals: Vec<String>,
    pub values: BTreeMap<String, Vec<String>>,
    pub bools: BTreeSet<String>,
    pub ordered_flags: Vec<(String, Option<String>)>,
}

impl OrcaArgBag {
    pub fn pos(&self, idx: usize) -> Option<&str> {
        self.positionals.get(idx).map(String::as_str)
    }

    pub fn first(&self, names: &[&str]) -> Option<&str> {
        for name in names {
            if let Some(v) = self.values.get(*name).and_then(|list| list.last()) {
                return Some(v.as_str());
            }
        }
        None
    }

    pub fn all(&self, names: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        for (k, v_opt) in &self.ordered_flags {
            if names.iter().any(|n| n == k)
                && let Some(val) = v_opt
            {
                out.push(val.clone());
            }
        }
        out
    }

    pub fn has_bool(&self, names: &[&str]) -> bool {
        names.iter().any(|n| self.bools.contains(*n))
    }

    pub fn opt_bool(&self, names: &[&str]) -> Result<Option<bool>, CliError> {
        if let Some(raw) = self.first(names) {
            return parse_bool_flag(names[0], raw).map(Some);
        }
        if self.has_bool(names) {
            return Ok(Some(true));
        }
        Ok(None)
    }

    pub fn opt_u64(&self, names: &[&str]) -> Result<Option<u64>, CliError> {
        match self.first(names) {
            Some(raw) => raw
                .trim()
                .parse::<u64>()
                .map(Some)
                .map_err(|_| orca_usage(format!("Invalid unsigned integer for '{}': '{raw}'", names[0]))),
            None => Ok(None),
        }
    }

    pub fn opt_i64(&self, names: &[&str]) -> Result<Option<i64>, CliError> {
        match self.first(names) {
            Some(raw) => raw
                .trim()
                .parse::<i64>()
                .map(Some)
                .map_err(|_| orca_usage(format!("Invalid integer for '{}': '{raw}'", names[0]))),
            None => Ok(None),
        }
    }

    pub fn opt_u32(&self, names: &[&str]) -> Result<Option<u32>, CliError> {
        match self.first(names) {
            Some(raw) => raw
                .trim()
                .parse::<u32>()
                .map(Some)
                .map_err(|_| orca_usage(format!("Invalid u32 integer for '{}': '{raw}'", names[0]))),
            None => Ok(None),
        }
    }

    pub fn opt_usize(&self, names: &[&str]) -> Result<Option<usize>, CliError> {
        match self.first(names) {
            Some(raw) => raw
                .trim()
                .parse::<usize>()
                .map(Some)
                .map_err(|_| orca_usage(format!("Invalid integer for '{}': '{raw}'", names[0]))),
            None => Ok(None),
        }
    }
}

pub(crate) fn orca_usage(msg: impl Into<String>) -> CliError {
    CliError::OrcaError {
        code: "invalid_argument".to_string(),
        message: msg.into(),
        exit_code: 2,
    }
}

pub(crate) fn parse_bool_flag(flag: &str, raw: &str) -> Result<bool, CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(orca_usage(format!(
            "Invalid boolean value '{other}' for '{flag}' (expected true/false)"
        ))),
    }
}

/// Reads either an existing UTF-8 file at `raw` or returns `raw` directly as JSON text.
pub(crate) fn load_json_file_or_inline(raw: &str, label: &str) -> Result<Value, CliError> {
    let trimmed = raw.trim();
    let content = if !trimmed.starts_with('{')
        && !trimmed.starts_with('[')
        && Path::new(trimmed).is_file()
    {
        std::fs::read_to_string(trimmed)
            .map_err(|e| orca_usage(format!("Failed to read {label} file '{trimmed}': {e}")))?
    } else {
        trimmed.to_string()
    };
    serde_json::from_str(&content)
        .map_err(|e| orca_usage(format!("Invalid JSON for {label}: {e}")))
}

/// Percent-encodes a single URL path segment or query value strictly per RFC 3986.
pub(crate) fn pct_encode(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len() * 3);
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0F) as usize] as char);
        }
    }
    out
}

pub(crate) fn build_query_url(base: &str, query: &[(&str, String)]) -> String {
    if query.is_empty() {
        return base.to_string();
    }
    let mut out = String::from(base);
    let mut sep = if base.contains('?') { '&' } else { '?' };
    for (k, v) in query {
        out.push(sep);
        out.push_str(&pct_encode(k));
        out.push('=');
        out.push_str(&pct_encode(v));
        sep = '&';
    }
    out
}

/// Defense-in-depth redactor for `whsec_...` and `whk_...` tokens alongside `orca_core::redact_webhook_keys`.
pub(crate) fn redact_webhook_secret_text(input: &str) -> String {
    let mut out = redact_webhook_keys(input);
    for prefix in ["whsec_", "whk_"] {
        while let Some(pos) = out.find(prefix) {
            let tail = &out[pos + prefix.len()..];
            let token_len = tail
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                .count();
            let end = pos + prefix.len() + token_len;
            out.replace_range(pos..end, REDACTED_WEBHOOK_KEY);
        }
    }
    out
}

const WEBHOOK_FORBIDDEN_KEYS: &[&str] = &[
    "url",
    "secret",
    "ingest_url",
    "ingestUrl",
    "secret_key",
    "secretKey",
    "copy_card",
    "copyCard",
    "webhook_url",
    "webhookUrl",
    "webhook_key",
    "webhookKey",
    "signing_secret",
    "signingSecret",
    "raw_url",
    "rawUrl",
];

/// Recursively strips forbidden webhook URL/secret fields and redacts `orcawh_`/`whsec_`/`whk_`
/// tokens from every string before printing webhook CLI output.
pub(crate) fn sanitize_webhook_value(val: &mut Value) {
    match val {
        Value::Object(map) => {
            for forbidden in WEBHOOK_FORBIDDEN_KEYS {
                map.remove(*forbidden);
            }
            for v in map.values_mut() {
                sanitize_webhook_value(v);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                sanitize_webhook_value(item);
            }
        }
        Value::String(s) => {
            let cleaned = redact_webhook_secret_text(s);
            if cleaned != *s {
                *s = cleaned;
            }
        }
        _ => {}
    }
}

/// Recursively strips `edit_token` / `editToken` from artifact CLI output (defense-in-depth).
pub(crate) fn sanitize_artifact_value(val: &mut Value) {
    match val {
        Value::Object(map) => {
            map.remove("edit_token");
            map.remove("editToken");
            for v in map.values_mut() {
                sanitize_artifact_value(v);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                sanitize_artifact_value(item);
            }
        }
        _ => {}
    }
}

/// Direct HTTP transport to `/api/v1/orca/*` preserving exact nested `error.code` and
/// mapping `approval_required` -> exit code `3`.
pub(crate) struct OrcaCliHttp {
    base_url: String,
    token: Option<String>,
    client: Client,
}

impl OrcaCliHttp {
    pub fn from_backend(backend: &FabushiBackendClient) -> Result<Self, CliError> {
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| CliError::OrcaError {
                code: "http_error".to_string(),
                message: format!("Failed to initialize HTTP client: {e}"),
                exit_code: 1,
            })?;
        Ok(Self {
            base_url: backend.server_url().trim_end_matches('/').to_string(),
            token: backend.token().map(ToOwned::to_owned),
            client,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, CliError> {
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else if path.starts_with('/') {
            format!("{}{path}", self.base_url)
        } else {
            format!("{}/{path}", self.base_url)
        };

        let mut req = self.client.request(method.clone(), &url);
        if let Some(ref tok) = self.token {
            req = req.bearer_auth(tok);
        }
        if let Some(payload) = body {
            req = req.json(payload);
        }

        let resp = req.send().await.map_err(|e| CliError::OrcaError {
            code: "network_error".to_string(),
            message: redact_webhook_secret_text(&format!("{method} {url} failed: {e}")),
            exit_code: 1,
        })?;

        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| CliError::OrcaError {
            code: "network_error".to_string(),
            message: redact_webhook_secret_text(&format!("Failed reading response: {e}")),
            exit_code: 1,
        })?;

        let parsed: Value = if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }))
        };

        if (200..300).contains(&status) {
            return Ok(parsed);
        }

        let code = parsed
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(Value::as_str)
            .or_else(|| {
                parsed
                    .get("error_detail")
                    .and_then(|e| e.get("code"))
                    .and_then(Value::as_str)
            })
            .or_else(|| parsed.get("error").and_then(Value::as_str))
            .or_else(|| parsed.get("code").and_then(Value::as_str))
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(match status {
                400 => "invalid_argument",
                401 => "unauthorized",
                403 => "forbidden",
                404 => "not_found",
                408 => "wait_timeout",
                409 => "conflict",
                410 => "expired",
                413 => "payload_too_large",
                422 => "unprocessable_entity",
                429 => "rate_limited",
                _ => "server_error",
            })
            .to_string();

        let message = parsed
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .or_else(|| {
                parsed
                    .get("error_detail")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
            })
            .or_else(|| parsed.get("message").and_then(Value::as_str))
            .filter(|s| !s.trim().is_empty())
            .map(redact_webhook_secret_text)
            .unwrap_or_else(|| format!("Server returned HTTP {status} ({code})"));

        let exit_code = if code == "approval_required" {
            PLUGIN_APPROVAL_REQUIRED_EXIT_CODE
        } else {
            1
        };

        Err(CliError::OrcaError {
            code,
            message,
            exit_code,
        })
    }

    pub async fn get(&self, path: &str) -> Result<Value, CliError> {
        self.request(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, CliError> {
        self.request(Method::POST, path, Some(body)).await
    }

    pub async fn patch(&self, path: &str, body: &Value) -> Result<Value, CliError> {
        self.request(Method::PATCH, path, Some(body)).await
    }

    pub async fn put(&self, path: &str, body: &Value) -> Result<Value, CliError> {
        self.request(Method::PUT, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value, CliError> {
        self.request(Method::DELETE, path, None).await
    }
}

fn is_pure_bool_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--multi"
            | "--force"
            | "--undo"
            | "--enable"
            | "--disable"
            | "--include-closed"
            | "--include-snoozed"
            | "--include-memory"
            | "--include-memory-index"
            | "--include-memory-topics"
            | "--include-skills"
            | "--include-avatar"
            | "--weekdays-only"
            | "--leave-thread"
            | "--create-new"
            | "--require-signature"
            | "--once"
            | "--confirm"
            | "--user-confirmed-share"
            | "--allow-system-shortcut"
            | "--approve-target"
            | "--allow-foreground-restore"
            | "--remove-dir"
            | "--recover-status"
            | "--remove-directory"
            | "--keep-open"
            | "--setup-failed"
            | "--succeeded"
            | "--failed"
            | "--local-only"
            | "--reject"
            | "--decline"
            | "--deny"
            | "--run"
            | "--unsnooze"
            | "--batch"
            | "--include-resolved"
            | "--ci-finished-supported"
            | "--allow-deletions"
            | "--allow-sleep-catchup"
            | "--host-mcp-servers"
            | "--force-discard-unsaved"
            | "--is-forwarded"
            | "--is-quoted-reply"
            | "--from-owner"
            | "--on-owner-message"
            | "--mailbox-local-only"
            | "--supports-mail"
            | "--ci-passed"
            | "--conflicts-resolved"
            | "--human-approved"
            | "--require-human-approval"
            | "--auto-approve"
            | "--preview"
            | "--text-stdin"
            | "--value-stdin"
            | "--export"
    )
}

fn is_dual_bool_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--paused"
            | "--enabled"
            | "--first-run"
            | "--approve"
            | "--yes"
            | "-y"
            | "--enforce-approval"
            | "--tab-visible"
            | "--agent-state-is-done"
            | "--mobile-driver"
            | "--mobile-driver-attached"
            | "--unsettled-orchestration-dispatch"
            | "--git-operation-in-progress"
            | "--active-port-forward"
            | "--pending-bot-approval"
            | "--unsynced-local-snapshot"
            | "--pinned-no-hibernate"
    )
}

fn looks_like_bool_literal(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "true" | "false" | "1" | "0"
    )
}

fn looks_like_hex_fingerprint(raw: &str) -> bool {
    let t = raw.trim();
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Peeks ahead past leading global flags to return the first positional subcommand token.
pub fn peek_subcommand<'a>(args: &'a [String], mut idx: usize) -> Option<&'a str> {
    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        if !raw.starts_with('-') {
            return Some(raw.as_str());
        }
        match flag {
            "-s" | "--server" | "--server-url" | "-t" | "--token" | "--socket" | "--user-id"
            | "--approval-token" | "--output" => {
                idx += if eq_val.is_some() { 1 } else { 2 };
            }
            _ => idx += 1,
        }
    }
    None
}

fn canonicalize_namespace_and_sub(
    head: &str,
    sub: Option<String>,
) -> Result<(String, String, Option<String>), CliError> {
    match head {
        "bot" => {
            let subcmd = sub.ok_or_else(|| {
                orca_usage("Subcommand 'bot' requires a subcommand (e.g., list, create, item-add)")
            })?;
            Ok(("bot".to_string(), subcmd, None))
        }
        "orchestration" => {
            let subcmd = sub.ok_or_else(|| {
                orca_usage("Subcommand 'orchestration' requires a subcommand (e.g., run-create)")
            })?;
            Ok(("orchestration".to_string(), subcmd, None))
        }
        "webhooks" | "webhook" => {
            let subcmd = sub.unwrap_or_else(|| "list".to_string());
            Ok(("webhooks".to_string(), subcmd, None))
        }
        "automations" | "automation" => {
            let subcmd = sub.unwrap_or_else(|| "list".to_string());
            Ok(("automations".to_string(), subcmd, None))
        }
        "worktree" | "worktrees" => {
            let subcmd = sub.unwrap_or_else(|| "list".to_string());
            Ok(("worktree".to_string(), subcmd, None))
        }
        "ide" => {
            let subcmd = sub.ok_or_else(|| {
                orca_usage("Subcommand 'ide' requires a subcommand (e.g., diff-annotate, usage)")
            })?;
            Ok(("ide".to_string(), subcmd, None))
        }
        "serve" => Ok(("ide".to_string(), "serve".to_string(), sub)),
        "profile" => match sub.as_deref() {
            None | Some("state") | Some("profile-state") => {
                Ok(("ide".to_string(), "profile-state".to_string(), None))
            }
            Some("get") => Ok(("ide".to_string(), "profile-get".to_string(), None)),
            Some("put") | Some("set") => Ok(("ide".to_string(), "profile-put".to_string(), None)),
            Some(other) => Ok((
                "ide".to_string(),
                "profile-state".to_string(),
                Some(other.to_string()),
            )),
        },
        "plugin" | "plugins" => {
            let mapped = match sub.as_deref() {
                None | Some("list") => "plugin-list".to_string(),
                Some("validate") => "plugin-validate".to_string(),
                Some("install") => "plugin-install".to_string(),
                Some("approve") => "plugin-approve".to_string(),
                Some("evaluate") => "plugin-evaluate".to_string(),
                Some(other) => format!("plugin-{other}"),
            };
            Ok(("ide".to_string(), mapped, None))
        }
        "artifacts" | "artifact" => {
            let mapped = match sub.as_deref() {
                None | Some("list") | Some("get") => "artifact-list".to_string(),
                Some("share") | Some("publish") => "artifact-share".to_string(),
                Some(other) => format!("artifact-{other}"),
            };
            Ok(("ide".to_string(), mapped, None))
        }
        "computer" => {
            let mapped = match sub.as_deref() {
                None | Some("guard") => "computer-guard".to_string(),
                Some("hotkey") => "computer-hotkey".to_string(),
                Some(other) => format!("computer-{other}"),
            };
            Ok(("ide".to_string(), mapped, None))
        }
        "browser" => {
            let mapped = match sub.as_deref() {
                None | Some("status") | Some("design-mode") | Some("capture") => {
                    "browser-design-mode".to_string()
                }
                Some(other) => format!("browser-{other}"),
            };
            Ok(("ide".to_string(), mapped, None))
        }
        other => Err(CliError::UnknownCommand(format!(
            "Unknown Orca namespace '{other}'"
        ))),
    }
}

/// Parses an Orca CLI namespace & subcommand, consuming interleaved/trailing `--json` and
/// global flags seamlessly.
pub fn parse_orca_command(
    head: &str,
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub_token: Option<String> = None;
    let mut bag = OrcaArgBag::default();
    let take_sub = head != "serve";

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);

        if flag == "--json" && eq_val.is_none() {
            opts.output_format = Some(OutputFormat::Json);
            idx += 1;
            continue;
        }
        if matches!(flag, "-h" | "--help") {
            help_requested = true;
            idx += 1;
            continue;
        }
        // Handle --approve / -y / --yes locally so subcommands can inspect both boolean
        // and `--approve <fingerprint>` forms without GlobalOptions swallowing the value.
        if matches!(flag, "--approve" | "--yes" | "-y") {
            if let Some(v) = eq_val {
                bag.values
                    .entry("--approve".to_string())
                    .or_default()
                    .push(v.to_string());
                bag.bools.insert("--approve".to_string());
                bag.ordered_flags
                    .push(("--approve".to_string(), Some(v.to_string())));
            } else if idx + 1 < args.len()
                && !args[idx + 1].starts_with('-')
                && (looks_like_hex_fingerprint(&args[idx + 1])
                    || looks_like_bool_literal(&args[idx + 1])
                    || (flag == "--approve"
                        && matches!(
                            sub_token.as_deref(),
                            Some("plugin-install") | Some("plugin-approve") | Some("install")
                        )))
            {
                idx += 1;
                let v = args[idx].clone();
                bag.values
                    .entry("--approve".to_string())
                    .or_default()
                    .push(v.clone());
                bag.bools.insert("--approve".to_string());
                bag.ordered_flags.push(("--approve".to_string(), Some(v)));
            } else {
                bag.bools.insert("--approve".to_string());
                bag.ordered_flags.push(("--approve".to_string(), None));
            }
            idx += 1;
            continue;
        }

        if raw.starts_with('-') {
            match try_consume_global(args, &mut idx, opts)? {
                GlobalConsumeResult::Consumed => {
                    idx += 1;
                    continue;
                }
                GlobalConsumeResult::HelpFlag => {
                    help_requested = true;
                    idx += 1;
                    continue;
                }
                GlobalConsumeResult::VersionFlag => {
                    return Ok(UnifiedParseOutcome::Version);
                }
                GlobalConsumeResult::NotGlobal => {}
            }
            if raw == "--" {
                idx += 1;
                while idx < args.len() {
                    bag.positionals.push(args[idx].clone());
                    idx += 1;
                }
                break;
            }
            if is_pure_bool_flag(flag)
                && !(flag == "--run" && matches!(head, "orchestration" | "orch"))
            {
                if let Some(v) = eq_val {
                    bag.values
                        .entry(flag.to_string())
                        .or_default()
                        .push(v.to_string());
                    bag.ordered_flags
                        .push((flag.to_string(), Some(v.to_string())));
                } else {
                    bag.bools.insert(flag.to_string());
                    bag.ordered_flags.push((flag.to_string(), None));
                }
                idx += 1;
                continue;
            }
            if is_dual_bool_flag(flag) {
                if let Some(v) = eq_val {
                    bag.values
                        .entry(flag.to_string())
                        .or_default()
                        .push(v.to_string());
                    bag.ordered_flags
                        .push((flag.to_string(), Some(v.to_string())));
                } else if idx + 1 < args.len() && looks_like_bool_literal(&args[idx + 1]) {
                    idx += 1;
                    let v = args[idx].clone();
                    bag.values
                        .entry(flag.to_string())
                        .or_default()
                        .push(v.clone());
                    bag.ordered_flags.push((flag.to_string(), Some(v)));
                } else {
                    bag.bools.insert(flag.to_string());
                    bag.ordered_flags.push((flag.to_string(), None));
                }
                idx += 1;
                continue;
            }
            let val = match eq_val {
                Some(v) => v.to_string(),
                None => {
                    if idx + 1 >= args.len() {
                        return Err(orca_usage(format!("Option '{flag}' requires a value")));
                    }
                    idx += 1;
                    args[idx].clone()
                }
            };
            bag.values
                .entry(flag.to_string())
                .or_default()
                .push(val.clone());
            bag.ordered_flags.push((flag.to_string(), Some(val)));
            idx += 1;
            continue;
        }

        if take_sub && sub_token.is_none() {
            sub_token = Some(raw.clone());
        } else {
            bag.positionals.push(raw.clone());
        }
        idx += 1;
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help(head.to_string()));
    }

    let (namespace, subcommand, extra_pos) = canonicalize_namespace_and_sub(head, sub_token)?;
    if let Some(p) = extra_pos {
        bag.positionals.insert(0, p);
    }

    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Orca(
        OrcaCliCommand {
            namespace,
            subcommand,
            args: bag,
        },
    )))
}

/// Executes a parsed Orca CLI command and formats direct structured JSON to stdout.
pub async fn execute_orca(
    opts: &GlobalOptions,
    backend: &FabushiBackendClient,
    cmd: OrcaCliCommand,
) -> Result<CliOutcome, CliError> {
    let http = OrcaCliHttp::from_backend(backend)?;
    let mut val = match cmd.namespace.as_str() {
        "bot" => crate::orca_commands_bot::execute_bot(&http, &cmd.subcommand, &cmd.args).await?,
        "orchestration" => {
            crate::orca_commands_orch::execute_orchestration(&http, &cmd.subcommand, &cmd.args)
                .await?
        }
        "webhooks" => {
            let mut v =
                crate::orca_commands_orch::execute_webhooks(&http, &cmd.subcommand, &cmd.args)
                    .await?;
            sanitize_webhook_value(&mut v);
            v
        }
        "automations" => {
            crate::orca_commands_orch::execute_automations(&http, &cmd.subcommand, &cmd.args)
                .await?
        }
        "worktree" => {
            crate::orca_commands_ide::execute_worktree(&http, &cmd.subcommand, &cmd.args).await?
        }
        "ide" => {
            let mut v =
                crate::orca_commands_ide::execute_ide(&http, opts, &cmd.subcommand, &cmd.args)
                    .await?;
            if cmd.subcommand.starts_with("artifact") {
                sanitize_artifact_value(&mut v);
            }
            v
        }
        other => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown Orca namespace '{other}'"
            )));
        }
    };
    if cmd.namespace == "webhooks" {
        sanitize_webhook_value(&mut val);
    }
    let out = format_json(&val, opts.pretty)?;
    Ok(CliOutcome::ok(out))
}
