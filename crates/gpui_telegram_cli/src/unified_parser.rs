use crate::cli_parser::{
    next_arg_value, parse_bool_str, split_flag_eq, try_consume_global, GlobalConsumeResult,
    GlobalOptions,
};
use crate::error::CliError;
use crate::mcp_config::McpClientTarget;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSelector {
    Id(String),
    Match(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnifiedCommand {
    // System & Daemon
    Status,
    Echo {
        message: String,
    },
    Daemon {
        op: DaemonOp,
    },
    App {
        op: AppOp,
    },

    // Auth & Account
    Auth {
        op: AuthOp,
    },
    User {
        op: UserOp,
    },
    VaultStatus,

    // Remote Devices
    Device {
        op: DeviceOp,
    },

    // Cloud PC
    CloudPc {
        op: CloudPcOp,
    },

    // Remote Assist
    Assist {
        op: AssistOp,
    },

    // Remote & Local Persistent Terminals
    Term {
        op: TermOp,
    },
    Lterm {
        op: LtermOp,
    },

    // Screen, Input, Clipboard, Files
    ScreenCapture {
        target: TargetSelector,
        out_path: Option<String>,
        display_index: Option<u32>,
    },
    Input {
        op: InputOp,
    },
    Clipboard {
        op: ClipboardOp,
    },
    File {
        op: FileOp,
    },

    // Full-App IM / Search / Call / Bots / MiniApps / Tasks
    Chat {
        op: ChatOp,
    },
    Msg {
        op: MsgOp,
    },
    Search {
        query: String,
        chat_id: Option<i64>,
        limit: u32,
    },
    CallStatus {
        call_id: Option<String>,
    },
    Bot {
        op: BotOp,
    },
    MiniApp {
        op: MiniAppOp,
    },
    Task {
        op: TaskOp,
    },

    // Official MCP
    Mcp {
        op: McpOp,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonOp {
    Status,
    Start { foreground: bool },
    Stop,
    InstallService,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppOp {
    Open,
    Permissions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthOp {
    Login {
        username: Option<String>,
        password: Option<String>,
        device_name: Option<String>,
    },
    Logout,
    Whoami,
    Sessions,
    Revoke {
        session_id: Option<String>,
        others: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserOp {
    Info { user_id: Option<u64> },
    Wallet,
    Search { query: String, limit: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceOp {
    List {
        online_only: bool,
    },
    Info {
        target: TargetSelector,
    },
    Connect {
        target: TargetSelector,
        mode: String,
    },
    Disconnect {
        target: Option<TargetSelector>,
        session_id: Option<String>,
    },
    Status,
    Power {
        action: String,
        target: Option<TargetSelector>,
        mac_address: Option<String>,
        broadcast_ip: Option<String>,
        delay_minutes: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudPcOp {
    List,
    Launch { target: TargetSelector },
    Shutdown { target: TargetSelector },
    Connect { target: TargetSelector },
    Disconnect { target: Option<TargetSelector> },
    Status { target: Option<TargetSelector> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistOp {
    Id,
    ResetCode,
    SetCode { code: String },
    Allow { enabled: bool },
    Start { assist_id: String, code: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermOp {
    List {
        target: Option<TargetSelector>,
    },
    Open {
        target: TargetSelector,
        session_name: Option<String>,
        shell: Option<String>,
    },
    Exit {
        target: TargetSelector,
        session_name: Option<String>,
    },
    Exec {
        target: TargetSelector,
        session_name: Option<String>,
        timeout_ms: Option<u64>,
        command: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LtermOp {
    Ls,
    New {
        name: Option<String>,
        shell: Option<String>,
        cols: Option<u16>,
        rows: Option<u16>,
        detach: bool,
    },
    Attach {
        name: Option<String>,
    },
    Has {
        name: String,
    },
    Kill {
        name: String,
    },
    Rename {
        old_name: String,
        new_name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputOp {
    Click {
        target: TargetSelector,
        x: Option<i32>,
        y: Option<i32>,
        button: String,
        clicks: u8,
    },
    Move {
        target: TargetSelector,
        x: Option<i32>,
        y: Option<i32>,
        dx: Option<i32>,
        dy: Option<i32>,
        relative: bool,
    },
    Drag {
        target: TargetSelector,
        x: Option<i32>,
        y: Option<i32>,
        dx: Option<i32>,
        dy: Option<i32>,
        button: String,
    },
    Scroll {
        target: TargetSelector,
        x: Option<i32>,
        y: Option<i32>,
        scroll_x: i32,
        scroll_y: i32,
    },
    Type {
        target: TargetSelector,
        text: String,
    },
    Hotkey {
        target: TargetSelector,
        key: String,
        modifiers: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardOp {
    Get {
        target: TargetSelector,
    },
    Set {
        target: TargetSelector,
        text: Option<String>,
        image_b64: Option<String>,
        mime_type: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    Ls {
        target: TargetSelector,
        path: String,
    },
    Read {
        target: TargetSelector,
        path: String,
        max_bytes: Option<usize>,
    },
    Write {
        target: TargetSelector,
        path: String,
        content: Option<String>,
        from_local: Option<String>,
        create_dirs: bool,
    },
    Push {
        target: TargetSelector,
        local_path: String,
        remote_path: String,
    },
    Pull {
        target: TargetSelector,
        remote_path: String,
        local_path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatOp {
    List {
        limit: u32,
    },
    Info {
        chat_id: i64,
    },
    Create {
        title: String,
        chat_type: String,
        member_ids: Vec<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsgOp {
    Send {
        chat_id: i64,
        text: String,
        reply_to_id: Option<i64>,
    },
    History {
        chat_id: i64,
        limit: u32,
        offset_id: Option<i64>,
    },
    Search {
        query: String,
        chat_id: Option<i64>,
        limit: u32,
    },
    Delete {
        chat_id: i64,
        message_ids: Vec<i64>,
        for_everyone: bool,
    },
    React {
        chat_id: i64,
        message_id: i64,
        emoji: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotOp {
    List,
    BindComputer {
        bot_id: u64,
        target: TargetSelector,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum MiniAppOp {
    List,
    WebMcpCall {
        bot_username: String,
        short_name: String,
        tool_name: String,
        args: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOp {
    List,
    Create {
        title: String,
        target_node_id: Option<String>,
    },
    Approve {
        task_id: String,
        decision: String,
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpOp {
    Status {
        url: Option<String>,
    },
    Grants,
    Revoke {
        family_id: String,
    },
    Config {
        client: McpClientTarget,
        url: Option<String>,
    },
    ServeStdio {
        url: Option<String>,
    },
}

pub enum UnifiedParseOutcome {
    Command(UnifiedCommand),
    Help(String),
    Version,
}

fn consume_global_or_help(
    args: &[String],
    idx: &mut usize,
    opts: &mut GlobalOptions,
    help_requested: &mut bool,
    version_requested: &mut bool,
) -> Result<bool, CliError> {
    match try_consume_global(args, idx, opts)? {
        GlobalConsumeResult::Consumed => {
            *idx += 1;
            Ok(true)
        }
        GlobalConsumeResult::HelpFlag => {
            *help_requested = true;
            *idx += 1;
            Ok(true)
        }
        GlobalConsumeResult::VersionFlag => {
            *version_requested = true;
            *idx += 1;
            Ok(true)
        }
        GlobalConsumeResult::NotGlobal => Ok(false),
    }
}

fn parse_i32_arg(flag: &str, raw: &str) -> Result<i32, CliError> {
    raw.trim()
        .parse::<i32>()
        .map_err(|_| CliError::Validation(format!("Invalid integer '{raw}' for {flag}")))
}

fn parse_i64_arg(flag: &str, raw: &str) -> Result<i64, CliError> {
    raw.trim()
        .parse::<i64>()
        .map_err(|_| CliError::Validation(format!("Invalid integer '{raw}' for {flag}")))
}

fn parse_u64_arg(flag: &str, raw: &str) -> Result<u64, CliError> {
    raw.trim()
        .parse::<u64>()
        .map_err(|_| CliError::Validation(format!("Invalid unsigned integer '{raw}' for {flag}")))
}

fn parse_u32_arg(flag: &str, raw: &str) -> Result<u32, CliError> {
    raw.trim()
        .parse::<u32>()
        .map_err(|_| CliError::Validation(format!("Invalid unsigned integer '{raw}' for {flag}")))
}

fn parse_u16_arg(flag: &str, raw: &str) -> Result<u16, CliError> {
    raw.trim()
        .parse::<u16>()
        .map_err(|_| CliError::Validation(format!("Invalid u16 '{raw}' for {flag}")))
}

fn parse_u8_arg(flag: &str, raw: &str) -> Result<u8, CliError> {
    raw.trim()
        .parse::<u8>()
        .map_err(|_| CliError::Validation(format!("Invalid u8 '{raw}' for {flag}")))
}

fn set_target_id(target: &mut Option<TargetSelector>, id: String) -> Result<(), CliError> {
    if target.is_some() {
        return Err(CliError::Usage(
            "Multiple target selectors specified; pass a single <ID> or --m <pattern>".into(),
        ));
    }
    *target = Some(TargetSelector::Id(id));
    Ok(())
}

fn set_target_match(target: &mut Option<TargetSelector>, pattern: String) -> Result<(), CliError> {
    if target.is_some() {
        return Err(CliError::Usage(
            "Pass either a target ID or --m <pattern>, not both".into(),
        ));
    }
    *target = Some(TargetSelector::Match(pattern));
    Ok(())
}

fn require_target(target: Option<TargetSelector>, cmd: &str) -> Result<TargetSelector, CliError> {
    target.ok_or_else(|| {
        CliError::Usage(format!(
            "Command '{cmd}' requires a target device <ID> or --m <pattern>"
        ))
    })
}

fn try_parse_match_flag(
    args: &[String],
    idx: &mut usize,
    target: &mut Option<TargetSelector>,
) -> Result<bool, CliError> {
    let raw = &args[*idx];
    let (flag, eq_val) = split_flag_eq(raw);
    if flag == "--m" || flag == "-m" || flag == "--match" {
        let pat = match eq_val {
            Some(v) => v.to_string(),
            None => next_arg_value(args, idx, flag)?,
        };
        set_target_match(target, pat)?;
        *idx += 1;
        return Ok(true);
    }
    Ok(false)
}

pub fn parse_unified_command(
    head: &str,
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<Option<UnifiedParseOutcome>, CliError> {
    let mut version_requested = false;

    let outcome = match head {
        "status" => {
            while idx < args.len() {
                if !consume_global_or_help(
                    args,
                    &mut idx,
                    opts,
                    &mut help_requested,
                    &mut version_requested,
                )? {
                    return Err(CliError::Usage(format!(
                        "Unexpected argument '{}' for 'status'",
                        args[idx]
                    )));
                }
            }
            if help_requested {
                return Ok(Some(UnifiedParseOutcome::Help("status".into())));
            }
            UnifiedParseOutcome::Command(UnifiedCommand::Status)
        }
        "echo" => {
            let mut parts = Vec::new();
            while idx < args.len() {
                if args[idx].starts_with('-')
                    && consume_global_or_help(
                        args,
                        &mut idx,
                        opts,
                        &mut help_requested,
                        &mut version_requested,
                    )?
                {
                    continue;
                }
                if args[idx].starts_with('-') {
                    return Err(CliError::Usage(format!(
                        "Unknown flag '{}' for 'echo'",
                        args[idx]
                    )));
                }
                parts.push(args[idx].clone());
                idx += 1;
            }
            if help_requested {
                return Ok(Some(UnifiedParseOutcome::Help("echo".into())));
            }
            UnifiedParseOutcome::Command(UnifiedCommand::Echo {
                message: if parts.is_empty() {
                    "ok".to_string()
                } else {
                    parts.join(" ")
                },
            })
        }
        "daemon" => parse_daemon(args, idx, opts, help_requested)?,
        "app" => parse_app(args, idx, opts, help_requested)?,
        "auth" => parse_auth(args, idx, opts, help_requested)?,
        "user" => {
            let sub = args.get(idx).map(|s| s.as_str());
            if matches!(sub, Some("info" | "wallet" | "search")) || sub.is_none() || help_requested
            {
                parse_user(args, idx, opts, help_requested)?
            } else if sub.is_some_and(|s| s.starts_with('-')) {
                parse_user(args, idx, opts, help_requested)?
            } else {
                return Ok(None);
            }
        }
        "vault" => {
            let sub = args.get(idx).map(|s| s.as_str());
            if matches!(sub, Some("status")) || sub.is_none() || help_requested {
                parse_vault(args, idx, opts, help_requested)?
            } else if sub.is_some_and(|s| s.starts_with('-')) {
                parse_vault(args, idx, opts, help_requested)?
            } else {
                return Ok(None);
            }
        }
        "device" => {
            let sub = args.get(idx).map(|s| s.as_str());
            if matches!(
                sub,
                Some("list" | "info" | "connect" | "disconnect" | "status" | "power")
            ) || sub.is_none()
                || help_requested
                || sub.is_some_and(|s| s.starts_with('-'))
            {
                parse_device(args, idx, opts, help_requested)?
            } else {
                return Ok(None);
            }
        }
        "cloudpc" => parse_cloudpc(args, idx, opts, help_requested)?,
        "assist" => {
            let sub = args.get(idx).map(|s| s.as_str());
            if matches!(
                sub,
                Some("id" | "reset-code" | "set-code" | "allow" | "start")
            ) || sub.is_none()
                || help_requested
                || sub.is_some_and(|s| s.starts_with('-'))
            {
                parse_assist(args, idx, opts, help_requested)?
            } else {
                return Ok(None);
            }
        }
        "term" => parse_term(args, idx, opts, help_requested)?,
        "lterm" => parse_lterm(args, idx, opts, help_requested)?,
        "screen" => parse_screen(args, idx, opts, help_requested)?,
        "input" => parse_input(args, idx, opts, help_requested)?,
        "clipboard" => parse_clipboard(args, idx, opts, help_requested)?,
        "file" => parse_file(args, idx, opts, help_requested)?,
        "chat" => {
            let sub = args.get(idx).map(|s| s.as_str());
            if matches!(sub, Some("list" | "info" | "create"))
                || sub.is_none()
                || help_requested
                || sub.is_some_and(|s| s.starts_with('-'))
            {
                parse_chat(args, idx, opts, help_requested)?
            } else {
                return Ok(None);
            }
        }
        "msg" => parse_msg(args, idx, opts, help_requested)?,
        "search" => {
            let sub = args.get(idx).map(|s| s.as_str());
            if sub == Some("search-messages") || sub == Some("search_messages") {
                return Ok(None);
            }
            parse_search(args, idx, opts, help_requested)?
        }
        "call" => {
            let sub = args.get(idx).map(|s| s.as_str());
            if sub == Some("status") || sub.is_none() || help_requested || sub.is_some_and(|s| s.starts_with('-')) {
                parse_call(args, idx, opts, help_requested)?
            } else {
                return Ok(None);
            }
        }
        "bot" => {
            let sub = args.get(idx).map(|s| s.as_str());
            if matches!(sub, Some("list" | "bind-computer"))
                || sub.is_none()
                || help_requested
                || sub.is_some_and(|s| s.starts_with('-'))
            {
                parse_bot(args, idx, opts, help_requested)?
            } else {
                return Ok(None);
            }
        }
        "miniapp" => {
            let sub = args.get(idx).map(|s| s.as_str());
            if matches!(sub, Some("list" | "webmcp-call"))
                || sub.is_none()
                || help_requested
                || sub.is_some_and(|s| s.starts_with('-'))
            {
                parse_miniapp(args, idx, opts, help_requested)?
            } else {
                return Ok(None);
            }
        }
        "task" => {
            let sub = args.get(idx).map(|s| s.as_str());
            if matches!(sub, Some("list" | "create" | "approve"))
                || sub.is_none()
                || help_requested
                || sub.is_some_and(|s| s.starts_with('-'))
            {
                parse_task(args, idx, opts, help_requested)?
            } else {
                return Ok(None);
            }
        }
        "mcp" => {
            let sub = args.get(idx).map(|s| s.as_str());
            if matches!(
                sub,
                Some("status" | "grants" | "revoke" | "config" | "serve-stdio")
            ) || sub.is_none()
                || help_requested
                || sub.is_some_and(|s| s.starts_with('-'))
            {
                parse_mcp(args, idx, opts, help_requested)?
            } else {
                return Ok(None);
            }
        }
        _ => return Ok(None),
    };

    if version_requested {
        return Ok(Some(UnifiedParseOutcome::Version));
    }

    Ok(Some(outcome))
}

fn parse_daemon(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut foreground = false;
    let mut ver = false;
    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        if flag == "--foreground" {
            foreground = true;
            idx += 1;
            continue;
        }
        if raw.starts_with('-') {
            if consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                continue;
            }
            return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'daemon'")));
        }
        if let Some(v) = eq_val {
            return Err(CliError::Usage(format!("Unexpected argument '{v}'")));
        }
        if sub.is_none() {
            sub = Some(raw.clone());
            idx += 1;
        } else {
            return Err(CliError::Usage(format!(
                "Unexpected argument '{raw}' for 'daemon'"
            )));
        }
    }
    if help_requested {
        return Ok(UnifiedParseOutcome::Help("daemon".into()));
    }
    let op = match sub.as_deref() {
        Some("status") => DaemonOp::Status,
        Some("start") => DaemonOp::Start { foreground },
        Some("stop") => DaemonOp::Stop,
        Some("install-service") => DaemonOp::InstallService,
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown daemon subcommand '{other}' (expected status|start|stop|install-service)"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'daemon' requires <status|start|stop|install-service>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Daemon { op }))
}

fn parse_app(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut ver = false;
    while idx < args.len() {
        let raw = &args[idx];
        if raw.starts_with('-') {
            if consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                continue;
            }
            return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'app'")));
        }
        if sub.is_none() {
            sub = Some(raw.clone());
            idx += 1;
        } else {
            return Err(CliError::Usage(format!("Unexpected argument '{raw}' for 'app'")));
        }
    }
    if help_requested {
        return Ok(UnifiedParseOutcome::Help("app".into()));
    }
    let op = match sub.as_deref() {
        Some("open") => AppOp::Open,
        Some("permissions") => AppOp::Permissions,
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown app subcommand '{other}' (expected open|permissions)"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'app' requires <open|permissions>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::App { op }))
}

fn parse_auth(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut username: Option<String> = None;
    let mut password: Option<String> = None;
    let mut device_name: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut others = false;
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--username" | "-u" => {
                username = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--password" | "-p" => {
                password = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--device-name" => {
                device_name = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--others" => {
                others = true;
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'auth'")));
                }
            }
            _ => {
                if sub.is_none() {
                    sub = Some(raw.clone());
                } else if session_id.is_none() {
                    session_id = Some(raw.clone());
                } else {
                    return Err(CliError::Usage(format!(
                        "Unexpected extra argument '{raw}' for 'auth'"
                    )));
                }
                idx += 1;
            }
        }
    }
    if help_requested {
        return Ok(UnifiedParseOutcome::Help("auth".into()));
    }
    let op = match sub.as_deref() {
        Some("login") => AuthOp::Login {
            username,
            password,
            device_name,
        },
        Some("logout") => AuthOp::Logout,
        Some("whoami") => AuthOp::Whoami,
        Some("sessions") => AuthOp::Sessions,
        Some("revoke") => {
            if session_id.as_deref() == Some("others") {
                AuthOp::Revoke {
                    session_id: None,
                    others: true,
                }
            } else {
                AuthOp::Revoke { session_id, others }
            }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown auth subcommand '{other}' (expected login|logout|whoami|sessions|revoke)"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'auth' requires <login|logout|whoami|sessions|revoke>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Auth { op }))
}

fn parse_user(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut pos: Vec<String> = Vec::new();
    let mut limit: u32 = 20;
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        if flag == "--limit" {
            let val = match eq_val {
                Some(v) => v.to_string(),
                None => next_arg_value(args, &mut idx, flag)?,
            };
            limit = parse_u32_arg(flag, &val)?;
            idx += 1;
            continue;
        }
        if raw.starts_with('-') {
            if consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                continue;
            }
            return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'user'")));
        }
        if sub.is_none() {
            sub = Some(raw.clone());
        } else {
            pos.push(raw.clone());
        }
        idx += 1;
    }
    if help_requested {
        return Ok(UnifiedParseOutcome::Help("user".into()));
    }
    let op = match sub.as_deref() {
        Some("info") => {
            let user_id = match pos.first() {
                Some(v) => Some(parse_u64_arg("user_id", v)?),
                None => None,
            };
            UserOp::Info { user_id }
        }
        Some("wallet") => UserOp::Wallet,
        Some("search") => {
            if pos.is_empty() {
                return Err(CliError::Usage("'user search' requires a <query>".into()));
            }
            UserOp::Search {
                query: pos.join(" "),
                limit,
            }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown user subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'user' requires <info|wallet|search>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::User { op }))
}

fn parse_vault(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut ver = false;
    while idx < args.len() {
        let raw = &args[idx];
        if raw.starts_with('-') {
            if consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                continue;
            }
            return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'vault'")));
        }
        if sub.is_none() {
            sub = Some(raw.clone());
            idx += 1;
        } else {
            return Err(CliError::Usage(format!(
                "Unexpected argument '{raw}' for 'vault'"
            )));
        }
    }
    if help_requested {
        return Ok(UnifiedParseOutcome::Help("vault".into()));
    }
    match sub.as_deref() {
        Some("status") => Ok(UnifiedParseOutcome::Command(UnifiedCommand::VaultStatus)),
        Some(other) => Err(CliError::UnknownCommand(format!(
            "Unknown vault subcommand '{other}' (expected status)"
        ))),
        None => Err(CliError::Usage("Subcommand 'vault' requires 'status'".into())),
    }
}

fn parse_device(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut online_only = false;
    let mut target: Option<TargetSelector> = None;
    let mut mode = "desktop".to_string();
    let mut session_id: Option<String> = None;
    let mut power_action: Option<String> = None;
    let mut mac_address: Option<String> = None;
    let mut broadcast_ip: Option<String> = None;
    let mut delay_minutes: Option<u32> = None;
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        if try_parse_match_flag(args, &mut idx, &mut target)? {
            continue;
        }
        match flag {
            "--online" => {
                online_only = true;
                idx += 1;
            }
            "--mode" => {
                mode = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                idx += 1;
            }
            "--session" => {
                session_id = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--mac" => {
                mac_address = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--broadcast" => {
                broadcast_ip = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--delay" => {
                let val = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                delay_minutes = Some(parse_u32_arg(flag, &val)?);
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    return Err(CliError::Usage(format!(
                        "Unknown flag '{raw}' for 'device'"
                    )));
                }
            }
            _ => {
                if sub.is_none() {
                    sub = Some(raw.clone());
                } else if sub.as_deref() == Some("power") && power_action.is_none() {
                    power_action = Some(raw.clone());
                } else {
                    set_target_id(&mut target, raw.clone())?;
                }
                idx += 1;
            }
        }
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("device".into()));
    }

    let op = match sub.as_deref() {
        Some("list") => DeviceOp::List { online_only },
        Some("info") => DeviceOp::Info {
            target: require_target(target, "device info")?,
        },
        Some("connect") => DeviceOp::Connect {
            target: require_target(target, "device connect")?,
            mode,
        },
        Some("disconnect") => DeviceOp::Disconnect { target, session_id },
        Some("status") => DeviceOp::Status,
        Some("power") => {
            let action = power_action.ok_or_else(|| {
                CliError::Usage(
                    "Subcommand 'device power' requires <lock|sleep|reboot|shutdown|wol>".into(),
                )
            })?;
            let normalized = action.to_ascii_lowercase();
            if !matches!(
                normalized.as_str(),
                "lock" | "sleep" | "reboot" | "shutdown" | "wol"
            ) {
                return Err(CliError::Validation(format!(
                    "Unsupported power action '{action}' (expected lock|sleep|reboot|shutdown|wol)"
                )));
            }
            if normalized != "wol" && target.is_none() {
                return Err(CliError::Usage(format!(
                    "'device power {normalized}' requires a target <ID> or --m <pattern>"
                )));
            }
            if normalized == "wol" && target.is_none() && mac_address.is_none() {
                return Err(CliError::Usage(
                    "'device power wol' requires <ID|--m <pattern>> or --mac <MAC>".into(),
                ));
            }
            DeviceOp::Power {
                action: normalized,
                target,
                mac_address,
                broadcast_ip,
                delay_minutes,
            }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown device subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'device' requires <list|info|connect|disconnect|status|power>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Device { op }))
}

fn parse_cloudpc(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut target: Option<TargetSelector> = None;
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        if try_parse_match_flag(args, &mut idx, &mut target)? {
            continue;
        }
        if raw.starts_with('-') {
            if consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                continue;
            }
            return Err(CliError::Usage(format!(
                "Unknown flag '{raw}' for 'cloudpc'"
            )));
        }
        if sub.is_none() {
            sub = Some(raw.clone());
        } else {
            set_target_id(&mut target, raw.clone())?;
        }
        idx += 1;
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("cloudpc".into()));
    }

    let op = match sub.as_deref() {
        Some("list") => CloudPcOp::List,
        Some("launch") => CloudPcOp::Launch {
            target: require_target(target, "cloudpc launch")?,
        },
        Some("shutdown") => CloudPcOp::Shutdown {
            target: require_target(target, "cloudpc shutdown")?,
        },
        Some("connect") => CloudPcOp::Connect {
            target: require_target(target, "cloudpc connect")?,
        },
        Some("disconnect") => CloudPcOp::Disconnect { target },
        Some("status") => CloudPcOp::Status { target },
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown cloudpc subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'cloudpc' requires <list|launch|shutdown|connect|disconnect|status>"
                    .into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::CloudPc { op }))
}

fn parse_assist(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut pos: Vec<String> = Vec::new();
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        if raw.starts_with('-') {
            if consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                continue;
            }
            return Err(CliError::Usage(format!(
                "Unknown flag '{raw}' for 'assist'"
            )));
        }
        if sub.is_none() {
            sub = Some(raw.clone());
        } else {
            pos.push(raw.clone());
        }
        idx += 1;
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("assist".into()));
    }

    let op = match sub.as_deref() {
        Some("id") => AssistOp::Id,
        Some("reset-code") => AssistOp::ResetCode,
        Some("set-code") => {
            let code = pos
                .first()
                .cloned()
                .ok_or_else(|| CliError::Usage("'assist set-code' requires <CODE>".into()))?;
            AssistOp::SetCode { code }
        }
        Some("allow") => {
            let val = pos
                .first()
                .map(|s| s.to_ascii_lowercase())
                .ok_or_else(|| CliError::Usage("'assist allow' requires <on|off>".into()))?;
            let enabled = match val.as_str() {
                "on" | "true" | "1" | "yes" => true,
                "off" | "false" | "0" | "no" => false,
                _ => {
                    return Err(CliError::Validation(format!(
                        "Invalid value '{val}' for 'assist allow' (expected on|off)"
                    )))
                }
            };
            AssistOp::Allow { enabled }
        }
        Some("start") => {
            let assist_id = pos
                .first()
                .cloned()
                .ok_or_else(|| CliError::Usage("'assist start' requires <ID> [CODE]".into()))?;
            let code = pos.get(1).cloned();
            AssistOp::Start { assist_id, code }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown assist subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'assist' requires <id|reset-code|set-code|allow|start>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Assist { op }))
}

fn parse_term(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut target: Option<TargetSelector> = None;
    let mut session_name: Option<String> = None;
    let mut shell: Option<String> = None;
    let mut timeout_ms: Option<u64> = None;
    let mut cmd_parts: Vec<String> = Vec::new();
    let mut after_double_dash = false;
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        if after_double_dash {
            cmd_parts.push(raw.clone());
            idx += 1;
            continue;
        }
        if raw == "--" {
            after_double_dash = true;
            idx += 1;
            continue;
        }
        if try_parse_match_flag(args, &mut idx, &mut target)? {
            continue;
        }
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--session" | "-n" => {
                session_name = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--shell" => {
                shell = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--timeout" => {
                let val = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                timeout_ms = Some(parse_u64_arg(flag, &val)?);
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'term'")));
                }
            }
            _ => {
                if sub.is_none() {
                    if matches!(raw.as_str(), "open" | "exit" | "exec" | "ls") {
                        sub = Some(raw.clone());
                    } else {
                        // Optional `open` omitted: `fabushi term <ID>`
                        sub = Some("open".to_string());
                        set_target_id(&mut target, raw.clone())?;
                    }
                } else if target.is_none() {
                    set_target_id(&mut target, raw.clone())?;
                } else if sub.as_deref() == Some("exec") {
                    cmd_parts.push(raw.clone());
                } else {
                    return Err(CliError::Usage(format!(
                        "Unexpected argument '{raw}' for 'term'"
                    )));
                }
                idx += 1;
            }
        }
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("term".into()));
    }

    let op = match sub.as_deref() {
        Some("ls") => TermOp::List { target },
        Some("open") | None => {
            if sub.is_none() && target.is_none() {
                return Err(CliError::Usage(
                    "Subcommand 'term' requires [open|exit|exec] <ID|--m <pattern>>".into(),
                ));
            }
            TermOp::Open {
                target: require_target(target, "term open")?,
                session_name,
                shell,
            }
        }
        Some("exit") => TermOp::Exit {
            target: require_target(target, "term exit")?,
            session_name,
        },
        Some("exec") => {
            let target = require_target(target, "term exec")?;
            let command = cmd_parts.join(" ");
            if command.trim().is_empty() {
                return Err(CliError::Usage(
                    "'term exec' requires a command after '--' (e.g. fabushi term exec <ID> -- uname -a)"
                        .into(),
                ));
            }
            TermOp::Exec {
                target,
                session_name,
                timeout_ms,
                command,
            }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown term subcommand '{other}'"
            )))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Term { op }))
}

fn parse_lterm(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut pos: Vec<String> = Vec::new();
    let mut shell: Option<String> = None;
    let mut cols: Option<u16> = None;
    let mut rows: Option<u16> = None;
    let mut detach = false;
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--shell" => {
                shell = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--cols" => {
                let val = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                cols = Some(parse_u16_arg(flag, &val)?);
                idx += 1;
            }
            "--rows" => {
                let val = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                rows = Some(parse_u16_arg(flag, &val)?);
                idx += 1;
            }
            "--detach" | "-d" => {
                detach = true;
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'lterm'")));
                }
            }
            _ => {
                if sub.is_none() {
                    sub = Some(raw.clone());
                } else {
                    pos.push(raw.clone());
                }
                idx += 1;
            }
        }
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("lterm".into()));
    }

    let op = match sub.as_deref() {
        Some("ls") => LtermOp::Ls,
        Some("new") => LtermOp::New {
            name: pos.first().cloned(),
            shell,
            cols,
            rows,
            detach,
        },
        Some("attach") => LtermOp::Attach {
            name: pos.first().cloned(),
        },
        Some("has") => {
            let name = pos
                .first()
                .cloned()
                .ok_or_else(|| CliError::Usage("'lterm has' requires <NAME>".into()))?;
            LtermOp::Has { name }
        }
        Some("kill") => {
            let name = pos
                .first()
                .cloned()
                .ok_or_else(|| CliError::Usage("'lterm kill' requires <NAME>".into()))?;
            LtermOp::Kill { name }
        }
        Some("rename") => {
            if pos.len() < 2 {
                return Err(CliError::Usage(
                    "'lterm rename' requires <OLD> <NEW>".into(),
                ));
            }
            LtermOp::Rename {
                old_name: pos[0].clone(),
                new_name: pos[1].clone(),
            }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown lterm subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'lterm' requires <ls|new|attach|has|kill|rename>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Lterm { op }))
}

fn parse_screen(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut target: Option<TargetSelector> = None;
    let mut out_path: Option<String> = None;
    let mut display_index: Option<u32> = None;
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        if try_parse_match_flag(args, &mut idx, &mut target)? {
            continue;
        }
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--out" | "-o" => {
                out_path = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--display" => {
                let val = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                display_index = Some(parse_u32_arg(flag, &val)?);
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    return Err(CliError::Usage(format!(
                        "Unknown flag '{raw}' for 'screen'"
                    )));
                }
            }
            _ => {
                if sub.is_none() {
                    sub = Some(raw.clone());
                } else {
                    set_target_id(&mut target, raw.clone())?;
                }
                idx += 1;
            }
        }
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("screen".into()));
    }

    match sub.as_deref() {
        Some("capture") => Ok(UnifiedParseOutcome::Command(UnifiedCommand::ScreenCapture {
            target: require_target(target, "screen capture")?,
            out_path,
            display_index,
        })),
        Some(other) => Err(CliError::UnknownCommand(format!(
            "Unknown screen subcommand '{other}' (expected capture)"
        ))),
        None => Err(CliError::Usage(
            "Subcommand 'screen' requires 'capture <ID|--m <pattern>> [--out PATH]'".into(),
        )),
    }
}

fn parse_input(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut target: Option<TargetSelector> = None;
    let mut x: Option<i32> = None;
    let mut y: Option<i32> = None;
    let mut dx: Option<i32> = None;
    let mut dy: Option<i32> = None;
    let mut scroll_x: Option<i32> = None;
    let mut scroll_y: Option<i32> = None;
    let mut button = "left".to_string();
    let mut clicks: u8 = 1;
    let mut relative = false;
    let mut text: Option<String> = None;
    let mut key: Option<String> = None;
    let mut modifiers: Vec<String> = Vec::new();
    let mut pos: Vec<String> = Vec::new();
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        if try_parse_match_flag(args, &mut idx, &mut target)? {
            continue;
        }
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--x" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                x = Some(parse_i32_arg(flag, &v)?);
                idx += 1;
            }
            "--y" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                y = Some(parse_i32_arg(flag, &v)?);
                idx += 1;
            }
            "--dx" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                dx = Some(parse_i32_arg(flag, &v)?);
                idx += 1;
            }
            "--dy" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                dy = Some(parse_i32_arg(flag, &v)?);
                idx += 1;
            }
            "--scroll-x" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                scroll_x = Some(parse_i32_arg(flag, &v)?);
                idx += 1;
            }
            "--scroll-y" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                scroll_y = Some(parse_i32_arg(flag, &v)?);
                idx += 1;
            }
            "--button" => {
                button = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                idx += 1;
            }
            "--clicks" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                clicks = parse_u8_arg(flag, &v)?;
                idx += 1;
            }
            "--relative" => {
                relative = true;
                idx += 1;
            }
            "--text" => {
                text = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--key" => {
                key = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--modifiers" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                modifiers.extend(
                    v.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from),
                );
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    // Negative coordinate like `-10` after target/subcommand
                    if raw[1..].parse::<i32>().is_ok() && sub.is_some() {
                        pos.push(raw.clone());
                        idx += 1;
                    } else {
                        return Err(CliError::Usage(format!(
                            "Unknown flag '{raw}' for 'input'"
                        )));
                    }
                }
            }
            _ => {
                if sub.is_none() {
                    sub = Some(raw.clone());
                } else if target.is_none() {
                    set_target_id(&mut target, raw.clone())?;
                } else {
                    pos.push(raw.clone());
                }
                idx += 1;
            }
        }
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("input".into()));
    }

    let op = match sub.as_deref() {
        Some("click") => {
            let target = require_target(target, "input click")?;
            if x.is_none() && pos.len() >= 2 {
                x = Some(parse_i32_arg("x", &pos[0])?);
                y = Some(parse_i32_arg("y", &pos[1])?);
            }
            InputOp::Click {
                target,
                x,
                y,
                button,
                clicks,
            }
        }
        Some("move") => {
            let target = require_target(target, "input move")?;
            if x.is_none() && pos.len() >= 2 {
                x = Some(parse_i32_arg("x", &pos[0])?);
                y = Some(parse_i32_arg("y", &pos[1])?);
            }
            InputOp::Move {
                target,
                x,
                y,
                dx,
                dy,
                relative,
            }
        }
        Some("drag") => {
            let target = require_target(target, "input drag")?;
            if x.is_none() && pos.len() >= 4 {
                x = Some(parse_i32_arg("x", &pos[0])?);
                y = Some(parse_i32_arg("y", &pos[1])?);
                dx = Some(parse_i32_arg("dx", &pos[2])?);
                dy = Some(parse_i32_arg("dy", &pos[3])?);
            }
            InputOp::Drag {
                target,
                x,
                y,
                dx,
                dy,
                button,
            }
        }
        Some("scroll") => {
            let target = require_target(target, "input scroll")?;
            if scroll_y.is_none() && pos.len() == 1 {
                scroll_y = Some(parse_i32_arg("scroll_y", &pos[0])?);
            } else if scroll_x.is_none() && scroll_y.is_none() && pos.len() >= 2 {
                scroll_x = Some(parse_i32_arg("scroll_x", &pos[0])?);
                scroll_y = Some(parse_i32_arg("scroll_y", &pos[1])?);
            }
            InputOp::Scroll {
                target,
                x,
                y,
                scroll_x: scroll_x.unwrap_or(0),
                scroll_y: scroll_y.unwrap_or(0),
            }
        }
        Some("type") => {
            let target = require_target(target, "input type")?;
            let final_text = text.unwrap_or_else(|| pos.join(" "));
            if final_text.is_empty() {
                return Err(CliError::Usage("'input type' requires text to type".into()));
            }
            InputOp::Type {
                target,
                text: final_text,
            }
        }
        Some("hotkey") => {
            let target = require_target(target, "input hotkey")?;
            let (final_key, final_mods) = if let Some(k) = key {
                (k, modifiers)
            } else {
                if pos.is_empty() {
                    return Err(CliError::Usage(
                        "'input hotkey' requires a key combo (e.g. Cmd+C)".into(),
                    ));
                }
                let joined = pos.join("+");
                let mut parts: Vec<String> = joined
                    .split('+')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                let main_key = parts.pop().unwrap_or_default();
                let mut all_mods = modifiers;
                all_mods.extend(parts);
                (main_key, all_mods)
            };
            InputOp::Hotkey {
                target,
                key: final_key,
                modifiers: final_mods,
            }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown input subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'input' requires <click|move|drag|scroll|type|hotkey>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Input { op }))
}

fn parse_clipboard(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut target: Option<TargetSelector> = None;
    let mut text: Option<String> = None;
    let mut image_b64: Option<String> = None;
    let mut mime_type: Option<String> = None;
    let mut pos: Vec<String> = Vec::new();
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        if try_parse_match_flag(args, &mut idx, &mut target)? {
            continue;
        }
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--text" => {
                text = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--image-b64" => {
                image_b64 = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--mime" => {
                mime_type = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    return Err(CliError::Usage(format!(
                        "Unknown flag '{raw}' for 'clipboard'"
                    )));
                }
            }
            _ => {
                if sub.is_none() {
                    sub = Some(raw.clone());
                } else if target.is_none() {
                    set_target_id(&mut target, raw.clone())?;
                } else {
                    pos.push(raw.clone());
                }
                idx += 1;
            }
        }
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("clipboard".into()));
    }

    let op = match sub.as_deref() {
        Some("get") => ClipboardOp::Get {
            target: require_target(target, "clipboard get")?,
        },
        Some("set") => {
            let target = require_target(target, "clipboard set")?;
            let final_text = text.or_else(|| {
                if pos.is_empty() {
                    None
                } else {
                    Some(pos.join(" "))
                }
            });
            if final_text.is_none() && image_b64.is_none() {
                return Err(CliError::Usage(
                    "'clipboard set' requires <TEXT> or --text / --image-b64".into(),
                ));
            }
            ClipboardOp::Set {
                target,
                text: final_text,
                image_b64,
                mime_type,
            }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown clipboard subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'clipboard' requires <get|set>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Clipboard { op }))
}

fn parse_file(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut target: Option<TargetSelector> = None;
    let mut max_bytes: Option<usize> = None;
    let mut from_local: Option<String> = None;
    let mut create_dirs = false;
    let mut pos: Vec<String> = Vec::new();
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        if try_parse_match_flag(args, &mut idx, &mut target)? {
            continue;
        }
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--max-bytes" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                max_bytes = Some(parse_u64_arg(flag, &v)? as usize);
                idx += 1;
            }
            "--from" => {
                from_local = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--create-dirs" => {
                create_dirs = true;
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'file'")));
                }
            }
            _ => {
                if sub.is_none() {
                    sub = Some(raw.clone());
                } else if target.is_none() {
                    set_target_id(&mut target, raw.clone())?;
                } else {
                    pos.push(raw.clone());
                }
                idx += 1;
            }
        }
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("file".into()));
    }

    let op = match sub.as_deref() {
        Some("ls") => FileOp::Ls {
            target: require_target(target, "file ls")?,
            path: pos.first().cloned().unwrap_or_else(|| ".".to_string()),
        },
        Some("read") => {
            let target = require_target(target, "file read")?;
            let path = pos
                .first()
                .cloned()
                .ok_or_else(|| CliError::Usage("'file read' requires <PATH>".into()))?;
            FileOp::Read {
                target,
                path,
                max_bytes,
            }
        }
        Some("write") => {
            let target = require_target(target, "file write")?;
            let path = pos
                .first()
                .cloned()
                .ok_or_else(|| CliError::Usage("'file write' requires <PATH>".into()))?;
            let content = if pos.len() > 1 {
                Some(pos[1..].join(" "))
            } else {
                None
            };
            if content.is_none() && from_local.is_none() {
                return Err(CliError::Usage(
                    "'file write' requires inline <CONTENT> or --from <LOCAL_FILE>".into(),
                ));
            }
            FileOp::Write {
                target,
                path,
                content,
                from_local,
                create_dirs,
            }
        }
        Some("push") => {
            let target = require_target(target, "file push")?;
            if pos.len() < 2 {
                return Err(CliError::Usage(
                    "'file push' requires <LOCAL_PATH> <REMOTE_PATH>".into(),
                ));
            }
            FileOp::Push {
                target,
                local_path: pos[0].clone(),
                remote_path: pos[1].clone(),
            }
        }
        Some("pull") => {
            let target = require_target(target, "file pull")?;
            if pos.len() < 2 {
                return Err(CliError::Usage(
                    "'file pull' requires <REMOTE_PATH> <LOCAL_PATH>".into(),
                ));
            }
            FileOp::Pull {
                target,
                remote_path: pos[0].clone(),
                local_path: pos[1].clone(),
            }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown file subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'file' requires <ls|read|write|push|pull>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::File { op }))
}

fn parse_chat(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut limit: u32 = 20;
    let mut chat_type = "group".to_string();
    let mut member_ids: Vec<u64> = Vec::new();
    let mut pos: Vec<String> = Vec::new();
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--limit" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                limit = parse_u32_arg(flag, &v)?;
                idx += 1;
            }
            "--type" => {
                chat_type = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                idx += 1;
            }
            "--members" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                for part in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    member_ids.push(parse_u64_arg("--members", part)?);
                }
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    if raw[1..].parse::<i64>().is_ok() && sub.is_some() {
                        pos.push(raw.clone());
                        idx += 1;
                    } else {
                        return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'chat'")));
                    }
                }
            }
            _ => {
                if sub.is_none() {
                    sub = Some(raw.clone());
                } else {
                    pos.push(raw.clone());
                }
                idx += 1;
            }
        }
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("chat".into()));
    }

    let op = match sub.as_deref() {
        Some("list") => ChatOp::List { limit },
        Some("info") => {
            let cid = pos
                .first()
                .ok_or_else(|| CliError::Usage("'chat info' requires <chat_id>".into()))?;
            ChatOp::Info {
                chat_id: parse_i64_arg("chat_id", cid)?,
            }
        }
        Some("create") => {
            if pos.is_empty() {
                return Err(CliError::Usage("'chat create' requires <title>".into()));
            }
            ChatOp::Create {
                title: pos.join(" "),
                chat_type,
                member_ids,
            }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown chat subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'chat' requires <list|info|create>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Chat { op }))
}

fn parse_msg(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut limit: u32 = 20;
    let mut offset_id: Option<i64> = None;
    let mut reply_to_id: Option<i64> = None;
    let mut chat_id_flag: Option<i64> = None;
    let mut for_everyone = false;
    let mut pos: Vec<String> = Vec::new();
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--limit" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                limit = parse_u32_arg(flag, &v)?;
                idx += 1;
            }
            "--offset-id" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                offset_id = Some(parse_i64_arg(flag, &v)?);
                idx += 1;
            }
            "--reply-to" | "--reply-to-id" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                reply_to_id = Some(parse_i64_arg(flag, &v)?);
                idx += 1;
            }
            "--chat-id" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                chat_id_flag = Some(parse_i64_arg(flag, &v)?);
                idx += 1;
            }
            "--for-everyone" | "--revoke" => {
                for_everyone = match eq_val {
                    Some(v) => parse_bool_str(flag, v)?,
                    None => true,
                };
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    if raw[1..].parse::<i64>().is_ok() && sub.is_some() {
                        pos.push(raw.clone());
                        idx += 1;
                    } else {
                        return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'msg'")));
                    }
                }
            }
            _ => {
                if sub.is_none() {
                    sub = Some(raw.clone());
                } else {
                    pos.push(raw.clone());
                }
                idx += 1;
            }
        }
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("msg".into()));
    }

    let op = match sub.as_deref() {
        Some("send") => {
            if pos.len() < 2 {
                return Err(CliError::Usage(
                    "'msg send' requires <chat_id> <text>".into(),
                ));
            }
            let chat_id = parse_i64_arg("chat_id", &pos[0])?;
            MsgOp::Send {
                chat_id,
                text: pos[1..].join(" "),
                reply_to_id,
            }
        }
        Some("history") => {
            let cid = pos
                .first()
                .ok_or_else(|| CliError::Usage("'msg history' requires <chat_id>".into()))?;
            MsgOp::History {
                chat_id: parse_i64_arg("chat_id", cid)?,
                limit,
                offset_id,
            }
        }
        Some("search") => {
            if pos.is_empty() {
                return Err(CliError::Usage("'msg search' requires <query>".into()));
            }
            MsgOp::Search {
                query: pos.join(" "),
                chat_id: chat_id_flag,
                limit,
            }
        }
        Some("delete") => {
            if pos.len() < 2 {
                return Err(CliError::Usage(
                    "'msg delete' requires <chat_id> <message_ids...>".into(),
                ));
            }
            let chat_id = parse_i64_arg("chat_id", &pos[0])?;
            let mut ids = Vec::new();
            for raw_id in &pos[1..] {
                for part in raw_id.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    ids.push(parse_i64_arg("message_id", part)?);
                }
            }
            MsgOp::Delete {
                chat_id,
                message_ids: ids,
                for_everyone,
            }
        }
        Some("react") => {
            if pos.len() < 3 {
                return Err(CliError::Usage(
                    "'msg react' requires <chat_id> <message_id> <emoji>".into(),
                ));
            }
            MsgOp::React {
                chat_id: parse_i64_arg("chat_id", &pos[0])?,
                message_id: parse_i64_arg("message_id", &pos[1])?,
                emoji: pos[2].clone(),
            }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown msg subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'msg' requires <send|history|search|delete|react>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Msg { op }))
}

fn parse_search(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut chat_id: Option<i64> = None;
    let mut limit: u32 = 20;
    let mut pos: Vec<String> = Vec::new();
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--chat-id" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                chat_id = Some(parse_i64_arg(flag, &v)?);
                idx += 1;
            }
            "--limit" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                limit = parse_u32_arg(flag, &v)?;
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    return Err(CliError::Usage(format!(
                        "Unknown flag '{raw}' for 'search'"
                    )));
                }
            }
            _ => {
                pos.push(raw.clone());
                idx += 1;
            }
        }
    }
    if help_requested {
        return Ok(UnifiedParseOutcome::Help("search".into()));
    }
    if pos.is_empty() {
        return Err(CliError::Usage("Subcommand 'search' requires <query>".into()));
    }
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Search {
        query: pos.join(" "),
        chat_id,
        limit,
    }))
}

fn parse_call(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut call_id: Option<String> = None;
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        if raw.starts_with('-') {
            if consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                continue;
            }
            return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'call'")));
        }
        if sub.is_none() {
            sub = Some(raw.clone());
        } else if call_id.is_none() {
            call_id = Some(raw.clone());
        } else {
            return Err(CliError::Usage(format!(
                "Unexpected argument '{raw}' for 'call'"
            )));
        }
        idx += 1;
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("call".into()));
    }

    match sub.as_deref() {
        Some("status") => Ok(UnifiedParseOutcome::Command(UnifiedCommand::CallStatus {
            call_id,
        })),
        Some(other) => Err(CliError::UnknownCommand(format!(
            "Unknown call subcommand '{other}' (expected status)"
        ))),
        None => Err(CliError::Usage("Subcommand 'call' requires 'status'".into())),
    }
}

fn parse_bot(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut bot_id: Option<u64> = None;
    let mut target: Option<TargetSelector> = None;
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        if try_parse_match_flag(args, &mut idx, &mut target)? {
            continue;
        }
        if raw.starts_with('-') {
            if consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                continue;
            }
            return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'bot'")));
        }
        if sub.is_none() {
            sub = Some(raw.clone());
        } else if bot_id.is_none() {
            bot_id = Some(parse_u64_arg("bot_id", raw)?);
        } else {
            set_target_id(&mut target, raw.clone())?;
        }
        idx += 1;
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("bot".into()));
    }

    let op = match sub.as_deref() {
        Some("list") => BotOp::List,
        Some("bind-computer") => BotOp::BindComputer {
            bot_id: bot_id.ok_or_else(|| {
                CliError::Usage("'bot bind-computer' requires <bot_id> <ID|--m <pattern>>".into())
            })?,
            target: require_target(target, "bot bind-computer")?,
        },
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown bot subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'bot' requires <list|bind-computer>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Bot { op }))
}

fn parse_miniapp(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut pos: Vec<String> = Vec::new();
    let mut json_args: Value = Value::Object(Default::default());
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        if flag == "--args" || flag == "--json" {
            let val = match eq_val {
                Some(v) => v.to_string(),
                None => next_arg_value(args, &mut idx, flag)?,
            };
            json_args = serde_json::from_str(&val).map_err(|e| {
                CliError::Validation(format!("Invalid JSON for '{flag}': {e}"))
            })?;
            idx += 1;
            continue;
        }
        if raw.starts_with('-') {
            if consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                continue;
            }
            return Err(CliError::Usage(format!(
                "Unknown flag '{raw}' for 'miniapp'"
            )));
        }
        if sub.is_none() {
            sub = Some(raw.clone());
        } else {
            pos.push(raw.clone());
        }
        idx += 1;
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("miniapp".into()));
    }

    let op = match sub.as_deref() {
        Some("list") => MiniAppOp::List,
        Some("webmcp-call") => {
            if pos.len() < 3 {
                return Err(CliError::Usage(
                    "'miniapp webmcp-call' requires <bot_username> <short_name> <tool_name>".into(),
                ));
            }
            MiniAppOp::WebMcpCall {
                bot_username: pos[0].clone(),
                short_name: pos[1].clone(),
                tool_name: pos[2].clone(),
                args: json_args,
            }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown miniapp subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'miniapp' requires <list|webmcp-call>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::MiniApp { op }))
}

fn parse_task(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut pos: Vec<String> = Vec::new();
    let mut target_node_id: Option<String> = None;
    let mut decision = "approve".to_string();
    let mut reason: Option<String> = None;
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--node" | "--node-id" => {
                target_node_id = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            "--decision" => {
                decision = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                idx += 1;
            }
            "--reason" => {
                reason = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'task'")));
                }
            }
            _ => {
                if sub.is_none() {
                    sub = Some(raw.clone());
                } else {
                    pos.push(raw.clone());
                }
                idx += 1;
            }
        }
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("task".into()));
    }

    let op = match sub.as_deref() {
        Some("list") => TaskOp::List,
        Some("create") => {
            if pos.is_empty() {
                return Err(CliError::Usage("'task create' requires <title>".into()));
            }
            TaskOp::Create {
                title: pos.join(" "),
                target_node_id,
            }
        }
        Some("approve") => {
            let task_id = pos
                .first()
                .cloned()
                .ok_or_else(|| CliError::Usage("'task approve' requires <task_id>".into()))?;
            TaskOp::Approve {
                task_id,
                decision,
                reason,
            }
        }
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown task subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'task' requires <list|create|approve>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Task { op }))
}

fn parse_mcp(
    args: &[String],
    mut idx: usize,
    opts: &mut GlobalOptions,
    mut help_requested: bool,
) -> Result<UnifiedParseOutcome, CliError> {
    let mut sub: Option<String> = None;
    let mut pos: Vec<String> = Vec::new();
    let mut client = McpClientTarget::All;
    let mut url: Option<String> = None;
    let mut ver = false;

    while idx < args.len() {
        let raw = &args[idx];
        let (flag, eq_val) = split_flag_eq(raw);
        match flag {
            "--client" | "-c" => {
                let v = match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                };
                client = McpClientTarget::parse(&v)?;
                idx += 1;
            }
            "--url" => {
                url = Some(match eq_val {
                    Some(v) => v.to_string(),
                    None => next_arg_value(args, &mut idx, flag)?,
                });
                idx += 1;
            }
            _ if raw.starts_with('-') => {
                if !consume_global_or_help(args, &mut idx, opts, &mut help_requested, &mut ver)? {
                    return Err(CliError::Usage(format!("Unknown flag '{raw}' for 'mcp'")));
                }
            }
            _ => {
                if sub.is_none() {
                    sub = Some(raw.clone());
                } else {
                    pos.push(raw.clone());
                }
                idx += 1;
            }
        }
    }

    if help_requested {
        return Ok(UnifiedParseOutcome::Help("mcp".into()));
    }

    let op = match sub.as_deref() {
        Some("status") => McpOp::Status { url },
        Some("grants") => McpOp::Grants,
        Some("revoke") => {
            let family_id = pos
                .first()
                .cloned()
                .ok_or_else(|| CliError::Usage("'mcp revoke' requires <family_id>".into()))?;
            McpOp::Revoke { family_id }
        }
        Some("config") => McpOp::Config { client, url },
        Some("serve-stdio") => McpOp::ServeStdio { url },
        Some(other) => {
            return Err(CliError::UnknownCommand(format!(
                "Unknown mcp subcommand '{other}'"
            )))
        }
        None => {
            return Err(CliError::Usage(
                "Subcommand 'mcp' requires <status|grants|revoke|config|serve-stdio>".into(),
            ))
        }
    };
    Ok(UnifiedParseOutcome::Command(UnifiedCommand::Mcp { op }))
}
