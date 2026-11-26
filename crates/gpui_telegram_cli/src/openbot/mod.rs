//! Complete 9-subcommand OpenBot CLI (`gpui-telegram-cli openbot <subcmd> ...`) (`OB-F20`).
//!
//! Entry points:
//! 1. `avatar-eval` — Bloub/Plush avatar evaluator (`OB-F01`, `OB-F03`, `OB-F04`)
//! 2. `theme-eval` — OpenBot theme/OKLab/backdrop evaluator (`OB-F05`)
//! 3. `ui-probe` — Deterministic 15-scenario headless GPUI UI probe (`OB-F03`..`OB-F20`)
//! 4. `agent` — 9 agent lifecycle & provider harness actions (`OB-F06`)
//! 5. `chat` — 17 scheduler, queue, thread, reaction, pin, DM, mailbox & interrupt actions (`OB-F07`)
//! 6. `channel` — 17 multi-bot group-chat & delegation actions (`OB-F08`, `OB-F09`, `OB-F17`)
//! 7. `tools` — 14 tool catalog, memory, SQL, skill, MCP, browser & vault actions (`OB-F10`)
//! 8. `platform` — 13 workflow, routine, webhook, CRM, site, marketplace, HTML, voice & bridge actions (`OB-F11`..`OB-F13`)
//! 9. `serve` — Live `/v1/*` Team API HTTP/SSE + loopback automation server (`OB-F11`, `OB-F13`, `OB-F20`)

pub mod agent_cmd;
pub mod channel_cmd;
pub mod chat_cmd;
pub mod eval_and_probe;
pub mod platform_cmd;
pub mod serve_cmd;
pub mod tools_cmd;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openbot_contracts::agents::{AgentAccess, AgentReasoningEffort};
use openbot_contracts::avatar::AvatarHue;
use openbot_contracts::providers::AgentProviderId;
use openbot_runtime::{OpenBotHostRuntime, OpenBotRuntimeError};
use openbot_storage::StorageError;
use serde_json::Value;

use crate::cli_parser::{CliCommand, GlobalOptions, OutputFormat, ParsedCli, split_flag_eq};
use crate::envelope::{CliOutcome, format_json, ok_envelope};
use crate::error::CliError;

#[derive(Debug, Clone, PartialEq)]
pub struct OpenBotCommand {
    pub subcommand: String,
    pub action: Option<String>,
    pub flags: ParsedFlags,
    pub data_dir: Option<PathBuf>,
    pub json: bool,
    pub pretty: bool,
    pub help: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct FlagSpec {
    pub value_flags: &'static [&'static str],
    pub bool_flags: &'static [&'static str],
    pub opt_bool_flags: &'static [&'static str],
    pub required_flags: &'static [&'static str],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedFlags {
    pub values: BTreeMap<String, String>,
    pub bools: BTreeSet<String>,
    pub opt_bools: BTreeMap<String, String>,
    pub data_dir: Option<PathBuf>,
    pub json: bool,
    pub compact: bool,
    pub pretty: bool,
    pub help: bool,
}

impl ParsedFlags {
    pub fn get(&self, flag: &str) -> Option<&str> {
        self.values.get(flag).map(String::as_str)
    }

    pub fn require(&self, flag: &str) -> Result<&str, CliError> {
        self.get(flag)
            .ok_or_else(|| CliError::Usage(format!("Missing required argument '{flag}'")))
    }

    pub fn has(&self, flag: &str) -> bool {
        self.bools.contains(flag)
    }

    pub fn get_opt_bool(&self, flag: &str, label: &str) -> Result<Option<bool>, CliError> {
        match self.opt_bools.get(flag) {
            Some(raw) => Ok(Some(parse_bool_domain(raw, label)?)),
            None => Ok(None),
        }
    }
}

pub fn parse_flags(args: &[String], spec: &FlagSpec) -> Result<ParsedFlags, CliError> {
    let mut out = ParsedFlags::default();
    let mut idx = 0usize;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);

        if flag == "-h" || flag == "--help" {
            out.help = true;
            idx += 1;
            continue;
        }
        if flag == "--json" {
            if eq_val.is_some() {
                return Err(CliError::Usage("'--json' does not take a value".into()));
            }
            out.json = true;
            idx += 1;
            continue;
        }
        if flag == "--compact" {
            if eq_val.is_some() {
                return Err(CliError::Usage("'--compact' does not take a value".into()));
            }
            out.compact = true;
            idx += 1;
            continue;
        }
        if flag == "--pretty" {
            if eq_val.is_some() {
                return Err(CliError::Usage("'--pretty' does not take a value".into()));
            }
            out.pretty = true;
            idx += 1;
            continue;
        }
        if flag == "--data-dir" {
            let val = consume_flag_value(args, &mut idx, flag, eq_val)?;
            out.data_dir = Some(PathBuf::from(val));
            idx += 1;
            continue;
        }

        if spec.value_flags.contains(&flag) {
            let val = consume_flag_value(args, &mut idx, flag, eq_val)?;
            out.values.insert(flag.to_string(), val);
            idx += 1;
            continue;
        }

        if spec.bool_flags.contains(&flag) {
            match eq_val {
                Some(v) => {
                    if parse_bool_domain(v, flag.trim_start_matches('-'))? {
                        out.bools.insert(flag.to_string());
                    } else {
                        out.bools.remove(flag);
                    }
                }
                None => {
                    out.bools.insert(flag.to_string());
                }
            }
            idx += 1;
            continue;
        }

        if spec.opt_bool_flags.contains(&flag) {
            let val = if let Some(v) = eq_val {
                v.to_string()
            } else if idx + 1 < args.len() && !args[idx + 1].starts_with('-') {
                idx += 1;
                args[idx].clone()
            } else {
                "true".to_string()
            };
            out.opt_bools.insert(flag.to_string(), val);
            idx += 1;
            continue;
        }

        if raw.starts_with('-') {
            return Err(CliError::Usage(format!("Unknown flag '{flag}'")));
        }
        return Err(CliError::Usage(format!("Unexpected argument '{raw}'")));
    }

    if !out.help {
        for req in spec.required_flags {
            if !out.values.contains_key(*req) {
                return Err(CliError::Usage(format!(
                    "Missing required argument '{req}'"
                )));
            }
        }
    }

    Ok(out)
}

fn consume_flag_value(
    args: &[String],
    idx: &mut usize,
    flag: &str,
    eq_val: Option<&str>,
) -> Result<String, CliError> {
    if let Some(v) = eq_val {
        return Ok(v.to_string());
    }
    if *idx + 1 >= args.len() {
        return Err(CliError::Usage(format!("Option '{flag}' requires a value")));
    }
    *idx += 1;
    // Unconditionally accept values starting with '-' (e.g. -1.0,-1.0, -100987654321) or "".
    Ok(args[*idx].clone())
}

fn flag_spec_for(subcommand: &str, action: Option<&str>) -> Result<FlagSpec, CliError> {
    match (subcommand, action) {
        ("avatar-eval", None) => Ok(eval_and_probe::avatar_eval_spec()),
        ("theme-eval", None) => Ok(eval_and_probe::theme_eval_spec()),
        ("ui-probe", None) => Ok(eval_and_probe::ui_probe_spec()),
        ("serve", None) => Ok(serve_cmd::serve_spec()),
        ("agent", Some(act)) => agent_cmd::flag_spec(act),
        ("chat", Some(act)) => chat_cmd::flag_spec(act),
        ("channel", Some(act)) => channel_cmd::flag_spec(act),
        ("tools", Some(act)) => tools_cmd::flag_spec(act),
        ("platform", Some(act)) => platform_cmd::flag_spec(act),
        (sub, None) => Err(CliError::Usage(format!(
            "Subcommand 'openbot {sub}' requires an action subcommand"
        ))),
        (sub, Some(act)) => Err(CliError::UnknownCommand(format!(
            "Unknown action '{act}' for 'openbot {sub}'"
        ))),
    }
}

fn subcommand_requires_action(sub: &str) -> bool {
    matches!(sub, "agent" | "chat" | "channel" | "tools" | "platform")
}

fn is_known_subcommand(sub: &str) -> bool {
    matches!(
        sub,
        "avatar-eval"
            | "theme-eval"
            | "ui-probe"
            | "agent"
            | "chat"
            | "channel"
            | "tools"
            | "platform"
            | "serve"
    )
}

pub fn parse_openbot_args(
    args: &[String],
    mut idx: usize,
    mut opts: GlobalOptions,
    mut help_requested: bool,
) -> Result<ParsedCli, CliError> {
    let mut pre_data_dir: Option<PathBuf> = None;
    let mut json_flag = false;
    let mut compact_flag = false;
    let mut pretty_flag = false;

    while idx < args.len() && args[idx].starts_with('-') {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "-h" | "--help" => {
                help_requested = true;
                idx += 1;
            }
            "--json" => {
                if eq_val.is_some() {
                    return Err(CliError::Usage("'--json' does not take a value".into()));
                }
                json_flag = true;
                idx += 1;
            }
            "--compact" => {
                compact_flag = true;
                idx += 1;
            }
            "--pretty" => {
                pretty_flag = true;
                idx += 1;
            }
            "--data-dir" => {
                let val = consume_flag_value(args, &mut idx, flag, eq_val)?;
                pre_data_dir = Some(PathBuf::from(val));
                idx += 1;
            }
            _ => {
                return Err(CliError::Usage(format!("Unknown flag '{flag}'")));
            }
        }
    }

    if idx >= args.len() {
        return Ok(ParsedCli {
            options: opts,
            command: CliCommand::OpenBot(OpenBotCommand {
                subcommand: "help".to_string(),
                action: None,
                flags: ParsedFlags::default(),
                data_dir: pre_data_dir,
                json: json_flag,
                pretty: !compact_flag,
                help: true,
            }),
        });
    }

    let subcommand = args[idx].clone();
    idx += 1;

    if subcommand == "help" {
        return Ok(ParsedCli {
            options: opts,
            command: CliCommand::OpenBot(OpenBotCommand {
                subcommand: "help".to_string(),
                action: args.get(idx).cloned(),
                flags: ParsedFlags::default(),
                data_dir: pre_data_dir,
                json: json_flag,
                pretty: !compact_flag,
                help: true,
            }),
        });
    }

    if !is_known_subcommand(&subcommand) {
        return Err(CliError::UnknownCommand(format!(
            "Unknown openbot subcommand '{subcommand}'"
        )));
    }

    let mut action: Option<String> = None;
    if subcommand_requires_action(&subcommand) {
        while idx < args.len() && args[idx].starts_with('-') {
            let raw = &args[idx];
            let (flag, eq_val) = split_flag_eq(raw);
            match flag {
                "-h" | "--help" => {
                    help_requested = true;
                    idx += 1;
                }
                "--json" => {
                    json_flag = true;
                    idx += 1;
                }
                "--compact" => {
                    compact_flag = true;
                    idx += 1;
                }
                "--pretty" => {
                    pretty_flag = true;
                    idx += 1;
                }
                "--data-dir" => {
                    let val = consume_flag_value(args, &mut idx, flag, eq_val)?;
                    pre_data_dir = Some(PathBuf::from(val));
                    idx += 1;
                }
                _ => {
                    return Err(CliError::Usage(format!("Unknown flag '{flag}'")));
                }
            }
        }
        if idx >= args.len() {
            if help_requested {
                return Ok(ParsedCli {
                    options: opts,
                    command: CliCommand::OpenBot(OpenBotCommand {
                        subcommand,
                        action: None,
                        flags: ParsedFlags::default(),
                        data_dir: pre_data_dir,
                        json: json_flag,
                        pretty: !compact_flag,
                        help: true,
                    }),
                });
            }
            return Err(CliError::Usage(format!(
                "Subcommand 'openbot {subcommand}' requires an action subcommand"
            )));
        }
        action = Some(args[idx].clone());
        idx += 1;
    }

    let spec = flag_spec_for(&subcommand, action.as_deref())?;
    let mut flags = parse_flags(&args[idx..], &spec)?;
    let data_dir = flags.data_dir.take().or(pre_data_dir);
    let help = help_requested || flags.help;
    let json = json_flag || flags.json || opts.output_format == Some(OutputFormat::Json);
    let pretty = if compact_flag || flags.compact {
        false
    } else if pretty_flag || flags.pretty {
        true
    } else {
        opts.pretty
    };
    opts.pretty = pretty;
    if json {
        opts.output_format = Some(OutputFormat::Json);
    }

    Ok(ParsedCli {
        options: opts,
        command: CliCommand::OpenBot(OpenBotCommand {
            subcommand,
            action,
            flags,
            data_dir,
            json,
            pretty,
            help,
        }),
    })
}

pub async fn execute_openbot(
    _opts: &GlobalOptions,
    cmd: OpenBotCommand,
) -> Result<CliOutcome, CliError> {
    if cmd.help || cmd.subcommand == "help" {
        return Ok(CliOutcome::ok(openbot_help_text(
            Some(cmd.subcommand.as_str()),
            cmd.action.as_deref(),
        )));
    }

    let data_dir = resolve_data_dir(cmd.data_dir.as_deref());

    match cmd.subcommand.as_str() {
        "avatar-eval" => {
            let val = eval_and_probe::run_avatar_eval(&cmd.flags, &data_dir)?;
            ok_openbot_json(val, cmd.pretty)
        }
        "theme-eval" => {
            let val = eval_and_probe::run_theme_eval(&cmd.flags, &data_dir)?;
            ok_openbot_json(val, cmd.pretty)
        }
        "ui-probe" => {
            let val = eval_and_probe::run_ui_probe(&cmd.flags, &data_dir)?;
            ok_openbot_json(val, cmd.pretty)
        }
        "serve" => serve_cmd::run_serve(&cmd.flags, &data_dir).await,
        "agent" => {
            let act = cmd.action.as_deref().unwrap_or("");
            let val = agent_cmd::run_agent(act, &cmd.flags, &data_dir).await?;
            ok_openbot_json(val, cmd.pretty)
        }
        "chat" => {
            let act = cmd.action.as_deref().unwrap_or("");
            let val = chat_cmd::run_chat(act, &cmd.flags, &data_dir)?;
            ok_openbot_json(val, cmd.pretty)
        }
        "channel" => {
            let act = cmd.action.as_deref().unwrap_or("");
            let val = channel_cmd::run_channel(act, &cmd.flags, &data_dir)?;
            ok_openbot_json(val, cmd.pretty)
        }
        "tools" => {
            let act = cmd.action.as_deref().unwrap_or("");
            let val = tools_cmd::run_tools(act, &cmd.flags, &data_dir)?;
            ok_openbot_json(val, cmd.pretty)
        }
        "platform" => {
            let act = cmd.action.as_deref().unwrap_or("");
            let val = platform_cmd::run_platform(act, &cmd.flags, &data_dir)?;
            ok_openbot_json(val, cmd.pretty)
        }
        other => Err(CliError::UnknownCommand(format!(
            "Unknown openbot subcommand '{other}'"
        ))),
    }
}

pub fn resolve_data_dir(explicit: Option<&Path>) -> PathBuf {
    if let Some(dir) = explicit
        && !dir.as_os_str().is_empty()
    {
        return dir.to_path_buf();
    }
    if let Ok(env_dir) = std::env::var("OPENBOT_DATA_DIR") {
        let trimmed = env_dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        let trimmed = xdg.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".openbot-data")
}

pub fn open_host_runtime(data_dir: &Path) -> Result<Arc<OpenBotHostRuntime>, CliError> {
    OpenBotHostRuntime::open(data_dir).map_err(map_runtime_error)
}

pub fn ok_openbot_json(data: Value, pretty: bool) -> Result<CliOutcome, CliError> {
    let env = ok_envelope(data);
    Ok(CliOutcome::ok(format_json(&env, pretty)?))
}

pub fn map_storage_error(err: StorageError) -> CliError {
    match err {
        StorageError::Validation(m)
        | StorageError::NotFound(m)
        | StorageError::Conflict(m)
        | StorageError::LimitExceeded(m) => CliError::Validation(m),
        StorageError::Forbidden(m) => CliError::Forbidden(m),
        other => CliError::Execution(other.to_string()),
    }
}

pub fn map_runtime_error(err: OpenBotRuntimeError) -> CliError {
    match err {
        OpenBotRuntimeError::Contract(e) => CliError::Validation(e.to_string()),
        OpenBotRuntimeError::Json(e) => CliError::Validation(format!("invalid JSON syntax: {e}")),
        OpenBotRuntimeError::Validation(m)
        | OpenBotRuntimeError::NotFound(m)
        | OpenBotRuntimeError::Conflict(m)
        | OpenBotRuntimeError::LimitExceeded(m) => CliError::Validation(m),
        OpenBotRuntimeError::Unauthorized(m) => CliError::Unauthorized(m),
        OpenBotRuntimeError::Forbidden(m) => CliError::Forbidden(m),
        OpenBotRuntimeError::ApprovalRequired {
            action,
            gate_id,
            expires_at_ms,
            message,
        } => CliError::ApprovalRequired {
            action,
            gate_id,
            expires_at_ms,
            message,
        },
        OpenBotRuntimeError::Storage(s) => map_storage_error(s),
        OpenBotRuntimeError::Http(m) => CliError::Http(m),
        OpenBotRuntimeError::Io(e) => CliError::Execution(e.to_string()),
        other => CliError::Execution(other.to_string()),
    }
}

pub fn read_inline_or_file(
    inline: Option<&str>,
    file_path: Option<&str>,
    label: &str,
) -> Result<Option<String>, CliError> {
    if let Some(path) = file_path {
        let content = fs::read_to_string(path).map_err(|e| {
            CliError::Validation(format!("failed to read {label} file '{path}': {e}"))
        })?;
        return Ok(Some(content));
    }
    if let Some(raw) = inline {
        if let Some(path) = raw.strip_prefix('@')
            && !path.is_empty()
            && Path::new(path).is_file()
        {
            let content = fs::read_to_string(path).map_err(|e| {
                CliError::Validation(format!("failed to read {label} file '{path}': {e}"))
            })?;
            return Ok(Some(content));
        }
        return Ok(Some(raw.to_string()));
    }
    Ok(None)
}

pub fn parse_json_value(raw: &str, label: &str) -> Result<Value, CliError> {
    let loaded = read_inline_or_file(Some(raw), None, label)?.unwrap_or_default();
    serde_json::from_str::<Value>(&loaded)
        .map_err(|e| CliError::Validation(format!("malformed JSON in {label}: {e}")))
}

pub fn parse_bool_domain(raw: &str, label: &str) -> Result<bool, CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        other => Err(CliError::Validation(format!(
            "invalid boolean value '{other}' for {label}: expected true or false"
        ))),
    }
}

pub fn parse_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn parse_u16_domain(raw: &str, label: &str) -> Result<u16, CliError> {
    raw.trim()
        .parse::<u16>()
        .map_err(|_| CliError::Validation(format!("invalid integer '{raw}' for {label}")))
}

pub fn parse_u32_domain(raw: &str, label: &str) -> Result<u32, CliError> {
    raw.trim()
        .parse::<u32>()
        .map_err(|_| CliError::Validation(format!("invalid integer '{raw}' for {label}")))
}

pub fn parse_u64_domain(raw: &str, label: &str) -> Result<u64, CliError> {
    raw.trim()
        .parse::<u64>()
        .map_err(|_| CliError::Validation(format!("invalid integer '{raw}' for {label}")))
}

pub fn parse_i64_domain(raw: &str, label: &str) -> Result<i64, CliError> {
    raw.trim()
        .parse::<i64>()
        .map_err(|_| CliError::Validation(format!("invalid integer '{raw}' for {label}")))
}

pub fn parse_usize_domain(raw: &str, label: &str) -> Result<usize, CliError> {
    raw.trim()
        .parse::<usize>()
        .map_err(|_| CliError::Validation(format!("invalid integer '{raw}' for {label}")))
}

pub fn parse_f64_domain(raw: &str, label: &str) -> Result<f64, CliError> {
    let val = raw
        .trim()
        .parse::<f64>()
        .map_err(|_| CliError::Validation(format!("invalid number '{raw}' for {label}")))?;
    if !val.is_finite() {
        return Err(CliError::Validation(format!(
            "non-finite number '{raw}' for {label}"
        )));
    }
    Ok(val)
}

pub fn parse_provider(raw: &str) -> Result<AgentProviderId, CliError> {
    AgentProviderId::parse(raw.trim()).ok_or_else(|| {
        CliError::Validation(format!(
            "unsupported agent provider '{}': expected one of the 18 built-in providers",
            raw.trim()
        ))
    })
}

pub fn parse_reasoning_effort(raw: &str) -> Result<AgentReasoningEffort, CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(AgentReasoningEffort::None),
        "minimal" => Ok(AgentReasoningEffort::Minimal),
        "low" => Ok(AgentReasoningEffort::Low),
        "medium" => Ok(AgentReasoningEffort::Medium),
        "high" => Ok(AgentReasoningEffort::High),
        "xhigh" => Ok(AgentReasoningEffort::XHigh),
        "max" => Ok(AgentReasoningEffort::Max),
        "ultra" => Ok(AgentReasoningEffort::Ultra),
        other => Err(CliError::Validation(format!(
            "unsupported reasoning effort '{other}': expected none|minimal|low|medium|high|xhigh|max|ultra"
        ))),
    }
}

pub fn parse_access(raw: &str) -> Result<AgentAccess, CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "full" => Ok(AgentAccess::Full),
        "workspace" => Ok(AgentAccess::Workspace),
        other => Err(CliError::Validation(format!(
            "unsupported agent access '{other}': expected full or workspace"
        ))),
    }
}

pub fn parse_avatar_hue(raw: &str) -> Result<AvatarHue, CliError> {
    let deg = parse_u16_domain(raw, "avatar hue")?;
    AvatarHue::from_degrees(deg).ok_or_else(|| {
        CliError::Validation(format!(
            "unsupported avatar hue {deg}: expected one of the 10 supported palette hues (0, 30, 55, 100, 150, 185, 215, 245, 280, 320)"
        ))
    })
}

fn openbot_help_text(subcommand: Option<&str>, action: Option<&str>) -> String {
    let target = match (subcommand, action) {
        (Some(s), Some(a)) if s != "help" => format!("openbot {s} {a}"),
        (Some(s), None) if s != "help" => format!("openbot {s}"),
        _ => "openbot".to_string(),
    };
    format!(
        "Usage: gpui-telegram-cli {target} [flags] [--data-dir <dir>] [--json]\n\
         \n\
         Subcommands:\n\
           avatar-eval   Evaluate Bloub/Plush avatar profiles, geometry, motion, clusters & images\n\
           theme-eval    Evaluate OpenBot dark/light/custom palettes, OKLab chat colors & backdrops\n\
           ui-probe      Run any of the 15 headless OpenBot GPUI UI & dual-surface probes\n\
           agent         Manage agents (create|list|get|update|delete|duplicate|generate-profile|export|import)\n\
           chat          1:1 chat, queue, branching, side-question, reactions, pins, DMs & mailbox\n\
           channel       Multi-bot group channels (create|list|get|update|archive|restore|delete|post|assign|transfer|reply|result|stop|cancel|continue|reassign|history-packet)\n\
           tools         Tool catalog, memory, shared agent-data SQL, skills, MCP, browser & vault\n\
           platform      Workflows, routines, webhooks, CRM, static sites, marketplace, HTML, voice & bridges\n\
           serve         Start live /v1/* Team API HTTP/SSE and loopback automation servers\n"
    )
}
