use crate::registry::{resolve_action, resolve_category, resolve_category_action, DEFAULT_SERVER_URL};
use gpui_telegram_core::action_bus::{ActionCategory, CliSchemaExport, CliSubcommandSpec};
use serde_json::Value;
use std::fmt::Write as _;

pub fn format_top_level_help(schema: &CliSchemaExport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} {} — Unified Fabushi Desktop, Remote Control, IM & MCP CLI",
        schema.program_name, schema.version
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "USAGE:");
    let _ = writeln!(
        out,
        "  {} [GLOBAL OPTIONS] <COMMAND> [SUBCOMMAND] [ARGS...]",
        schema.program_name
    );
    let _ = writeln!(
        out,
        "  {} [GLOBAL OPTIONS] <ACTION> [ARGS...]",
        schema.program_name
    );
    let _ = writeln!(
        out,
        "  {} [GLOBAL OPTIONS] <CATEGORY> <ACTION> [ARGS...]",
        schema.program_name
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "GLOBAL OPTIONS:");
    let _ = writeln!(
        out,
        "  -s, --server, --server-url <URL>  Server base URL [env: FABUSHI_SERVER_URL, GPUI_TELEGRAM_SERVER_URL] (default: {DEFAULT_SERVER_URL})"
    );
    let _ = writeln!(
        out,
        "  -t, --token <TOKEN>               Bearer auth token [env: FABUSHI_TOKEN, GPUI_TELEGRAM_TOKEN, GPUI_TG_TOKEN]"
    );
    let _ = writeln!(
        out,
        "      --socket <PATH>               Local IPC socket/pipe override [env: FABUSHI_IPC_SOCKET]"
    );
    let _ = writeln!(
        out,
        "      --local                       Execute Action Bus locally in-memory instead of remote server"
    );
    let _ = writeln!(
        out,
        "      --remote                      Prefer remote HTTP server over local Desktop/Daemon IPC"
    );
    let _ = writeln!(
        out,
        "      --user-id <U64>               Caller user_id for --local ActionContext"
    );
    let _ = writeln!(
        out,
        "  -y, --yes, --approve              Approve High/CriticalDestructive action gates automatically"
    );
    let _ = writeln!(
        out,
        "      --approval-token <TOKEN>      Pre-authorized X-Approval-Token for High-risk remote actions"
    );
    let _ = writeln!(
        out,
        "      --non-interactive             Never prompt on stdin; fail closed when approval is required"
    );
    let _ = writeln!(
        out,
        "      --output <json|text>          Output format (default: json for unified commands, text for list)"
    );
    let _ = writeln!(
        out,
        "      --json-output                 Alias for --output json"
    );
    let _ = writeln!(out, "      --compact                     Print compact single-line JSON");
    let _ = writeln!(out, "  -h, --help                        Show help information");
    let _ = writeln!(out, "  -V, --version                     Show version information");
    let _ = writeln!(out);
    let _ = writeln!(out, "SYSTEM & DAEMON:");
    let _ = writeln!(out, "  version                           Print Fabushi CLI version (JSON envelope)");
    let _ = writeln!(out, "  status                            Inspect local IPC daemon, desktop, and session status");
    let _ = writeln!(out, "  echo [msg]                        Echo test payload through local IPC");
    let _ = writeln!(out, "  daemon status|start|stop|install-service");
    let _ = writeln!(out, "  app open|permissions              Open Fabushi Desktop or inspect macOS/OS permissions");
    let _ = writeln!(out);
    let _ = writeln!(out, "AUTH & ACCOUNT:");
    let _ = writeln!(out, "  auth login|logout|whoami|sessions|revoke");
    let _ = writeln!(out, "  user info|wallet|search");
    let _ = writeln!(out, "  vault status                      Inspect encrypted User Vault status");
    let _ = writeln!(out);
    let _ = writeln!(out, "REMOTE DEVICES, CLOUD PC & ASSIST (supports <ID> or --m <pattern>):");
    let _ = writeln!(out, "  device list|info|connect|disconnect|status|power <lock|sleep|reboot|shutdown|wol>");
    let _ = writeln!(out, "  cloudpc list|launch|shutdown|connect|disconnect|status");
    let _ = writeln!(out, "  assist id|reset-code|set-code <CODE>|allow <on|off>|start <ID> [CODE]");
    let _ = writeln!(out);
    let _ = writeln!(out, "PERSISTENT TERMINALS:");
    let _ = writeln!(out, "  term [open] <ID|--m>|exit <ID|--m>|exec <ID|--m> -- <CMD>");
    let _ = writeln!(out, "  lterm ls|new [NAME] [--shell]|attach [NAME]|has <NAME>|kill <NAME>|rename <OLD> <NEW>");
    let _ = writeln!(out);
    let _ = writeln!(out, "SCREEN, INPUT, CLIPBOARD & FILES:");
    let _ = writeln!(out, "  screen capture <ID|--m> [--out PATH] [--display INDEX]");
    let _ = writeln!(out, "  input click|move|drag|scroll|type|hotkey <ID|--m> ...");
    let _ = writeln!(out, "  clipboard get|set <ID|--m> ...");
    let _ = writeln!(out, "  file ls|read|write|push|pull <ID|--m> ...");
    let _ = writeln!(out);
    let _ = writeln!(out, "IM, BOTS, MINIAPPS, TASKS & OFFICIAL MCP:");
    let _ = writeln!(out, "  chat list|info|create");
    let _ = writeln!(out, "  msg send|history|search|delete|react");
    let _ = writeln!(out, "  search <query> [--chat-id ID] [--limit N]");
    let _ = writeln!(out, "  call status [call_id]");
    let _ = writeln!(out, "  bot list|bind-computer <bot_id> <ID|--m>");
    let _ = writeln!(out, "  miniapp list|webmcp-call <bot> <short> <tool> [--args JSON]");
    let _ = writeln!(out, "  task list|create|approve");
    let _ = writeln!(out, "  mcp status|grants|revoke <family_id>|config [--client antigravity|claude|cursor|vscode]|serve-stdio");
    let _ = writeln!(out);
    let _ = writeln!(out, "ACTION BUS META SUBCOMMANDS:");
    let _ = writeln!(
        out,
        "  list [--category <cat>] [--remote] [--local]  List available Platform Actions"
    );
    let _ = writeln!(
        out,
        "  export-schema [--format cli|openapi|mcp|hermes]  Export Quad-Export Action Bus schema"
    );
    let _ = writeln!(
        out,
        "  schema <action> [--remote] [--local]          Inspect input/output JSON Schema for an action"
    );
    let _ = writeln!(
        out,
        "  approve <gate_id> [--decision approve_once|deny] [--reason <text>]  Decide a pending ApprovalGate"
    );
    let _ = writeln!(out, "  help [topic]                                  Show help for a command or category");
    let _ = writeln!(out);
    let _ = writeln!(out, "ACTION SUBCOMMANDS (generated from Action Bus schema):");
    for sub in &schema.subcommands {
        let approval_badge = if sub.requires_approval {
            " [approval required]"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  {:<24} [{:<9}] ({:?}){} - {}",
            sub.kebab_name, sub.category, sub.risk_level, approval_badge, sub.description
        );
    }
    let _ = writeln!(out);
    let categories: Vec<&str> = ActionCategory::ALL.iter().map(|c| c.as_str()).collect();
    let _ = writeln!(out, "CATEGORIES: {}", categories.join(", "));
    let _ = writeln!(
        out,
        "Run '{} <COMMAND> --help' or '{} <ACTION> --help' for detailed options.",
        schema.program_name, schema.program_name
    );
    out
}

pub fn format_action_help(program_name: &str, sub: &CliSubcommandSpec) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{} {} ({})", program_name, sub.kebab_name, sub.name);
    let _ = writeln!(out, "  {}", sub.description);
    let _ = writeln!(
        out,
        "  Category: {} | Risk: {:?} | Requires Approval: {}",
        sub.category, sub.risk_level, sub.requires_approval
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "USAGE:");
    let _ = writeln!(out, "  {program_name} {} [OPTIONS]", sub.kebab_name);
    let _ = writeln!(
        out,
        "  {program_name} {} {} [OPTIONS]",
        sub.category, sub.kebab_name
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "ACTION FLAGS:");
    let _ = writeln!(
        out,
        "      --json <JSON|->               JSON object payload (or '-' for stdin), overlaid by typed flags"
    );
    for arg in &sub.arguments {
        let req_tag = if arg.required { "required" } else { "optional" };
        let default_tag = arg
            .default_value
            .as_ref()
            .map(|d| format!(", default: {d}"))
            .unwrap_or_default();
        let desc = if arg.description.is_empty() {
            String::new()
        } else {
            format!(" — {}", arg.description)
        };
        let _ = writeln!(
            out,
            "      {:<26} <{}> [{}{}]{}",
            arg.flag, arg.value_type, req_tag, default_tag, desc
        );
    }
    out
}

pub fn format_category_help(schema: &CliSchemaExport, category: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} {} — Category '{}'",
        schema.program_name, schema.version, category
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "USAGE:");
    let _ = writeln!(
        out,
        "  {} {} <ACTION> [OPTIONS]",
        schema.program_name, category
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "ACTIONS IN CATEGORY '{category}':");
    let mut count = 0usize;
    for sub in schema
        .subcommands
        .iter()
        .filter(|s| s.category == category)
    {
        count += 1;
        let approval_badge = if sub.requires_approval {
            " [approval required]"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  {:<24} ({:?}){} - {}",
            sub.kebab_name, sub.risk_level, approval_badge, sub.description
        );
    }
    if count == 0 {
        let _ = writeln!(out, "  (no actions registered in this category)");
    }
    out
}

pub fn format_topic_help(schema: &CliSchemaExport, topic: Option<&str>) -> Option<String> {
    let Some(topic) = topic else {
        return Some(format_top_level_help(schema));
    };

    if let Some((cat, act)) = topic.split_once(' ') {
        return resolve_category_action(schema, cat, act)
            .map(|sub| format_action_help(&schema.program_name, sub));
    }

    if let Some(sub) = resolve_action(schema, topic) {
        return Some(format_action_help(&schema.program_name, sub));
    }

    let p = &schema.program_name;
    let unified_help = match topic {
        "status" => Some(format!("Usage: {p} status [--output json|text]\nInspect local Fabushi IPC daemon, desktop, and session status.\n")),
        "echo" => Some(format!("Usage: {p} echo [message...]\nEcho a message via local Fabushi IPC.\n")),
        "daemon" => Some(format!("Usage: {p} daemon <status|start [--foreground]|stop|install-service>\nManage the Fabushi remote background daemon/LaunchAgent.\n")),
        "app" => Some(format!("Usage: {p} app <open|permissions>\nLaunch Fabushi Desktop or inspect macOS/OS remote-control permissions.\n")),
        "auth" => Some(format!("Usage: {p} auth <login [--username U] [--password P]|logout|whoami|sessions|revoke <id|others>>\nAuthenticate and manage ~/.fabushi/cli_session.json.\n")),
        "user" => Some(format!("Usage: {p} user <info [user_id]|wallet|search <query>>\nInspect user profile, wallet balance, or search contacts.\n")),
        "vault" => Some(format!("Usage: {p} vault status\nInspect encrypted User Vault status.\n")),
        "device" => Some(format!("Usage: {p} device <list [--online]|info <ID|--m>|connect <ID|--m>|disconnect [ID|--m]|status|power <lock|sleep|reboot|shutdown|wol> <ID|--m>>\nManage and control registered devices.\n")),
        "cloudpc" => Some(format!("Usage: {p} cloudpc <list|launch <ID|--m>|shutdown <ID|--m>|connect <ID|--m>|disconnect [ID|--m]|status [ID|--m]>\nManage cloud servers and sandboxes.\n")),
        "assist" => Some(format!("Usage: {p} assist <id|reset-code|set-code <CODE>|allow <on|off>|start <ID> [CODE]>\nManage 9-digit Remote Assist ID and verification codes.\n")),
        "term" => Some(format!("Usage: {p} term <[open] <ID|--m>|exit <ID|--m>|exec <ID|--m> -- <CMD>|ls>\nOpen or execute commands in persistent remote PTY sessions.\n")),
        "lterm" => Some(format!("Usage: {p} lterm <ls|new [NAME] [--shell SHELL]|attach [NAME]|has <NAME>|kill <NAME>|rename <OLD> <NEW>>\nManage local persistent PTY multiplexer sessions (refuses nested mux when FABUSHI_IN_MUX=1).\n")),
        "screen" => Some(format!("Usage: {p} screen capture <ID|--m> [--out PATH] [--display INDEX]\nCapture remote or local display frame.\n")),
        "input" => Some(format!("Usage: {p} input <click|move|drag|scroll|type|hotkey> <ID|--m> ...\nSend native mouse/keyboard HID input.\n")),
        "clipboard" => Some(format!("Usage: {p} clipboard <get|set <TEXT>> <ID|--m>\nRead or set clipboard contents.\n")),
        "file" => Some(format!("Usage: {p} file <ls|read|write|push|pull> <ID|--m> ...\nBrowse, read, write, push, and pull remote files.\n")),
        "chat" => Some(format!("Usage: {p} chat <list|info <chat_id>|create <title> [--members ...]>\nList, inspect, or create IM chats.\n")),
        "msg" => Some(format!("Usage: {p} msg <send|history|search|delete|react> ...\nSend, read, search, delete, or react to IM messages.\n")),
        "search" => Some(format!("Usage: {p} search <query> [--chat-id ID] [--limit N]\nSearch messages across chats.\n")),
        "call" => Some(format!("Usage: {p} call status [call_id]\nInspect active voice/video call state.\n")),
        "bot" => Some(format!("Usage: {p} bot <list|bind-computer <bot_id> <ID|--m>>\nList AI bots and bind them to computer nodes.\n")),
        "miniapp" => Some(format!("Usage: {p} miniapp <list|webmcp-call <bot_username> <short_name> <tool> [--args JSON]>\nList mini-apps and invoke WebMCP tools.\n")),
        "task" => Some(format!("Usage: {p} task <list|create <title>|approve <task_id>>\nList, create, and approve automation tasks.\n")),
        "mcp" => Some(format!("Usage: {p} mcp <status|grants|revoke <family_id>|config [--client antigravity|claude|cursor|vscode] [--url URL]|serve-stdio>\nInspect official OAuth 2.1 MCP server, grants, and generate client configs.\n")),
        "list" => Some(format!(
            "Usage: {p} list [--category <cat>] [--remote] [--local] [--output json|text]\nList available Platform Actions.\n"
        )),
        "export-schema" => Some(format!(
            "Usage: {p} export-schema [--format cli|openapi|mcp|hermes]\nExport Action Bus schema in the requested format.\n"
        )),
        "schema" => Some(format!(
            "Usage: {p} schema <action> [--remote] [--local]\nPrint input and output JSON schemas for an action.\n"
        )),
        "approve" => Some(format!(
            "Usage: {p} approve <gate_id> [--decision approve_once|deny] [--reason <text>] --token <TOKEN>\nApprove or deny a pending server ApprovalGate.\n"
        )),
        "help" => Some(format_top_level_help(schema)),
        _ => None,
    };

    if unified_help.is_some() {
        return unified_help;
    }

    if let Some(cat) = resolve_category(topic) {
        return Some(format_category_help(schema, cat.as_str()));
    }

    None
}

pub fn format_actions_table(actions: &[Value]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<24} {:<12} {:<20} {:<10} DESCRIPTION",
        "ACTION", "CATEGORY", "RISK", "APPROVAL"
    );
    let _ = writeln!(out, "{}", "-".repeat(94));
    for item in actions {
        let name = item
            .get("kebab_name")
            .or_else(|| item.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let category = item
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let risk = item
            .get("risk_level")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let requires_approval = item
            .get("requires_approval")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let desc = item
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let _ = writeln!(
            out,
            "{:<24} {:<12} {:<20} {:<10} {}",
            name,
            category,
            risk,
            if requires_approval { "required" } else { "no" },
            desc
        );
    }
    out
}
