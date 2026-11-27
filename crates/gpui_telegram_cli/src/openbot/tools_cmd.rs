//! `openbot tools <action>` handler covering all 14 tool, memory, SQL, skill, MCP, browser & vault actions (`OB-F02`, `OB-F08`, `OB-F10`, `OB-F11`).

use std::path::Path;

use openbot_runtime::{
    BrowserRunStepsRequest, BrowserTarget, CardFillTargets, FillCardRequest, LinkPaymentRequest,
    MemoryPutInput, MemorySearchInput, RegisterMcpServerInput, SaveSkillInput, SubmitSecretRequest,
    TakeoverRequest, ToolCallContext,
};
use openbot_storage::{
    AgentSchemaChange, McpKeyValue, McpTransport, MemoryInclusion, SharedAgentDataDb, schema_change,
};
use serde_json::{Value, json};

use super::{
    FlagSpec, ParsedFlags, map_runtime_error, map_storage_error, open_host_runtime, parse_csv,
    parse_i64_domain, parse_json_value, parse_provider, parse_u16_domain, parse_u64_domain,
    parse_usize_domain, read_inline_or_file,
};
use crate::error::CliError;

pub fn flag_spec(action: &str) -> Result<FlagSpec, CliError> {
    match action {
        "list" => Ok(FlagSpec {
            value_flags: &["--provider"],
            bool_flags: &["--channel-turn"],
            opt_bool_flags: &[],
            required_flags: &[],
        }),
        "search" => Ok(FlagSpec {
            value_flags: &["--query", "--limit"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--query"],
        }),
        "describe" => Ok(FlagSpec {
            value_flags: &["--names"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--names"],
        }),
        "call" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--name",
                "--arguments",
                "--raw-arguments",
                "--tab-id",
                "--channel-id",
                "--task-id",
                "--thread-id",
                "--turn-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--name"],
        }),
        "memory-put" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--text",
                "--inclusion",
                "--site",
                "--memory-id",
                "--channel-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &["--essential", "--user-controlled"],
            required_flags: &["--agent-id", "--text"],
        }),
        "memory-search" => Ok(FlagSpec {
            value_flags: &["--agent-id", "--query", "--host", "--limit", "--channel-id"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--agent-id"],
        }),
        "agent-data-exec" => Ok(FlagSpec {
            value_flags: &["--agent-id", "--mode", "--sql", "--params"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--mode", "--sql"],
        }),
        "skill-save" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--slug",
                "--name",
                "--description",
                "--instructions",
                "--triggers",
                "--tools",
                "--origin",
            ],
            bool_flags: &[],
            opt_bool_flags: &["--enabled"],
            required_flags: &[
                "--agent-id",
                "--slug",
                "--name",
                "--description",
                "--instructions",
            ],
        }),
        "mcp-register" => Ok(FlagSpec {
            value_flags: &[
                "--id",
                "--name",
                "--transport",
                "--command",
                "--args",
                "--url",
                "--headers",
                "--env",
                "--working-directory",
            ],
            bool_flags: &[],
            opt_bool_flags: &["--enabled"],
            required_flags: &["--name", "--transport"],
        }),
        "browser-run" => Ok(FlagSpec {
            value_flags: &["--agent-id", "--tab-id", "--url", "--steps", "--timeout-ms"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--steps"],
        }),
        "vault-secret" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--tab-id",
                "--method",
                "--login-id",
                "--digits",
                "--submission",
                "--submit-target",
                "--targets",
                "--username",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--tab-id", "--method", "--submission"],
        }),
        "link-payment" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--amount",
                "--currency",
                "--merchant-name",
                "--merchant-url",
                "--context",
            ],
            bool_flags: &["--approve"],
            opt_bool_flags: &[],
            required_flags: &[
                "--agent-id",
                "--amount",
                "--merchant-name",
                "--merchant-url",
                "--context",
            ],
        }),
        "fill-card" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--tab-id",
                "--card-id",
                "--expiry",
                "--exp-month",
                "--exp-year",
                "--targets",
                "--timeout-ms",
            ],
            bool_flags: &["--approve"],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--tab-id", "--card-id"],
        }),
        "takeover" => Ok(FlagSpec {
            value_flags: &["--agent-id", "--tab-id", "--reason"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--tab-id"],
        }),
        other => Err(CliError::UnknownCommand(format!(
            "Unknown action '{other}' for 'openbot tools'"
        ))),
    }
}

fn parse_kv_list(raw: &str, label: &str) -> Result<Vec<McpKeyValue>, CliError> {
    let val = parse_json_value(raw, label)?;
    match val {
        Value::Object(map) => Ok(map
            .into_iter()
            .map(|(key, v)| McpKeyValue {
                key,
                value: match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                },
            })
            .collect()),
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let kv: McpKeyValue = serde_json::from_value(item).map_err(|e| {
                    CliError::Validation(format!("invalid {label} key/value entry: {e}"))
                })?;
                out.push(kv);
            }
            Ok(out)
        }
        _ => Err(CliError::Validation(format!(
            "{label} must be a JSON object or array of {{key, value}} entries"
        ))),
    }
}

fn to_json_val<T: serde::Serialize>(val: T, label: &str) -> Result<Value, CliError> {
    serde_json::to_value(val)
        .map_err(|e| CliError::Execution(format!("failed to serialize {label}: {e}")))
}

pub fn run_tools(action: &str, flags: &ParsedFlags, data_dir: &Path) -> Result<Value, CliError> {
    if action == "agent-data-exec" {
        return run_agent_data_exec(flags, data_dir);
    }

    let runtime = open_host_runtime(data_dir)?;
    let router = runtime.tool_router();

    match action {
        "list" => {
            let provider = match flags.get("--provider") {
                Some(raw) => Some(parse_provider(raw)?),
                None => None,
            };
            let listed = router.list_tools(provider, flags.has("--channel-turn"));
            let mut combined_tools = Vec::with_capacity(
                listed.direct_tools.len() + listed.discoverable_tools.len(),
            );
            combined_tools.extend(listed.direct_tools.iter().cloned());
            combined_tools.extend(listed.discoverable_tools.iter().cloned());
            let mut val = to_json_val(&listed, "tools list")?;
            if let Some(obj) = val.as_object_mut() {
                obj.insert(
                    "tools".to_string(),
                    to_json_val(combined_tools, "tools list combined")?,
                );
            }
            Ok(val)
        }
        "search" => {
            let query = flags.require("--query")?;
            let limit = match flags.get("--limit") {
                Some(raw) => parse_usize_domain(raw, "search limit")?,
                None => 10,
            };
            let res = router
                .search_tools(query, limit)
                .map_err(map_runtime_error)?;
            to_json_val(res, "tools search")
        }
        "describe" => {
            let names = parse_csv(flags.require("--names")?);
            let res = router.describe_tools(&names).map_err(map_runtime_error)?;
            to_json_val(res, "tools describe")
        }
        "call" => {
            let agent_id = flags.require("--agent-id")?.to_string();
            let name = flags.require("--name")?;
            let (raw_args, is_raw) = if let Some(raw) = flags.get("--raw-arguments") {
                let text = read_inline_or_file(Some(raw), None, "raw-arguments")?
                    .unwrap_or_default();
                (text, true)
            } else if let Some(args) = flags.get("--arguments") {
                let text =
                    read_inline_or_file(Some(args), None, "arguments")?.unwrap_or_default();
                (text, false)
            } else {
                ("{}".to_string(), false)
            };
            let ctx = ToolCallContext {
                agent_id,
                thread_id: flags.get("--thread-id").map(str::to_string),
                task_id: flags.get("--task-id").map(str::to_string),
                turn_id: flags.get("--turn-id").map(str::to_string),
                call_id: None,
                channel_id: flags.get("--channel-id").map(str::to_string),
                tab_id: flags.get("--tab-id").map(str::to_string),
                origin_routine_id: None,
                user_initiated: false,
            };
            let out = router
                .call_tool(&ctx, name, &raw_args, is_raw)
                .map_err(map_runtime_error)?;
            to_json_val(out, "tools call")
        }
        "memory-put" => {
            let inclusion = match flags.get("--inclusion") {
                Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                    "essential" => Some(MemoryInclusion::Essential),
                    "searchable" => Some(MemoryInclusion::Searchable),
                    other => {
                        return Err(CliError::Validation(format!(
                            "unsupported memory inclusion '{other}': expected essential or searchable"
                        )));
                    }
                },
                None => None,
            };
            let input = MemoryPutInput {
                agent_id: flags.require("--agent-id")?.to_string(),
                text: flags.require("--text")?.to_string(),
                memory_id: flags.get("--memory-id").map(str::to_string),
                inclusion,
                essential: flags.get_opt_bool("--essential", "essential")?,
                site: flags.get("--site").map(str::to_string),
                user_controlled: flags
                    .get_opt_bool("--user-controlled", "user-controlled")?
                    .unwrap_or(false),
                source_turn_id: None,
                channel_id: flags.get("--channel-id").map(str::to_string),
            };
            let out = router.memory_put(input).map_err(map_runtime_error)?;
            Ok(json!({
                "id": out.memory.entry.id,
                "memory": out.memory,
                "inclusion": out.inclusion,
                "user_controlled": out.user_controlled,
                "revision": out.revision,
                "essential_bytes": out.essential_bytes,
                "budget_bytes": out.budget_bytes,
                "stored_count": out.stored_count,
            }))
        }
        "memory-search" => {
            let limit = match flags.get("--limit") {
                Some(raw) => Some(parse_usize_domain(raw, "memory search limit")?),
                None => None,
            };
            let input = MemorySearchInput {
                agent_id: flags.require("--agent-id")?.to_string(),
                query: flags.get("--query").map(str::to_string),
                host: flags.get("--host").map(str::to_string),
                limit,
                channel_id: flags.get("--channel-id").map(str::to_string),
            };
            let out = router.memory_search(input).map_err(map_runtime_error)?;
            to_json_val(out, "memory-search")
        }
        "skill-save" => {
            let triggers = flags
                .get("--triggers")
                .map(parse_csv)
                .unwrap_or_default();
            let tools = flags.get("--tools").map(parse_csv).unwrap_or_default();
            let input = SaveSkillInput {
                agent_id: flags.require("--agent-id")?.to_string(),
                slug: flags.require("--slug")?.to_string(),
                name: flags.require("--name")?.to_string(),
                description: flags.require("--description")?.to_string(),
                instructions: flags.require("--instructions")?.to_string(),
                triggers,
                tools,
                origin: flags.get("--origin").map(str::to_string),
                enabled: flags.get_opt_bool("--enabled", "enabled")?,
            };
            let out = runtime
                .skills()
                .save_skill(input)
                .map_err(map_runtime_error)?;
            to_json_val(out, "skill-save")
        }
        "mcp-register" => {
            let transport = match flags
                .require("--transport")?
                .trim()
                .to_ascii_lowercase()
                .as_str()
            {
                "stdio" => McpTransport::Stdio,
                "http" => McpTransport::Http,
                other => {
                    return Err(CliError::Validation(format!(
                        "unsupported MCP transport '{other}': expected stdio or http"
                    )));
                }
            };
            let args = match flags.get("--args") {
                Some(raw) => {
                    let val = parse_json_value(raw, "mcp args")?;
                    serde_json::from_value::<Vec<String>>(val).map_err(|e| {
                        CliError::Validation(format!("mcp args must be a JSON string array: {e}"))
                    })?
                }
                None => Vec::new(),
            };
            let headers = match flags.get("--headers") {
                Some(raw) => parse_kv_list(raw, "mcp headers")?,
                None => Vec::new(),
            };
            let env = match flags.get("--env") {
                Some(raw) => parse_kv_list(raw, "mcp env")?,
                None => Vec::new(),
            };
            let input = RegisterMcpServerInput {
                id: flags.get("--id").map(str::to_string),
                name: flags.require("--name")?.to_string(),
                transport,
                enabled: flags.get_opt_bool("--enabled", "enabled")?,
                command: flags.get("--command").map(str::to_string),
                args,
                env,
                env_passthrough: Vec::new(),
                working_directory: flags.get("--working-directory").map(str::to_string),
                url: flags.get("--url").map(str::to_string),
                headers,
            };
            let server = runtime
                .mcp()
                .register_server(input)
                .map_err(map_runtime_error)?;
            Ok(json!({ "mcp_server": server }))
        }
        _ => run_browser_or_vault_action(action, flags, &runtime),
    }
}

fn run_agent_data_exec(flags: &ParsedFlags, data_dir: &Path) -> Result<Value, CliError> {
    let agent_id = flags.require("--agent-id")?;
    let mode = flags.require("--mode")?.trim().to_ascii_lowercase();
    let sql = flags.require("--sql")?;
    let params = match flags.get("--params") {
        Some(raw) => {
            let val = parse_json_value(raw, "sql params")?;
            let arr = val.as_array().ok_or_else(|| {
                CliError::Validation("sql params must be a JSON array".to_string())
            })?;
            arr.clone()
        }
        None => Vec::new(),
    };

    let shared_path = data_dir.join("Shared").join("Data").join("agent-data.db");
    let db = SharedAgentDataDb::open(&shared_path).map_err(map_storage_error)?;

    match mode.as_str() {
        "read" => {
            let res = db
                .query(agent_id, sql, &params)
                .map_err(map_storage_error)?;
            Ok(json!({
                "columns": res.columns,
                "rows": res.rows,
                "truncated": res.truncated,
                "changes": 0,
            }))
        }
        "write" => {
            let is_create = schema_change(sql) == Some(AgentSchemaChange::Create);
            let res = db
                .execute(agent_id, sql, &params)
                .map_err(map_storage_error)?;
            let mut out = json!({
                "rows": [],
                "changes": res.changes,
                "last_insert_rowid": res.last_insert_rowid,
            });
            if is_create && let Some(obj) = out.as_object_mut() {
                obj.insert(
                    "owner_agent_id".to_string(),
                    Value::String(agent_id.to_string()),
                );
            }
            Ok(out)
        }
        other => Err(CliError::Validation(format!(
            "unsupported agent-data-exec mode '{other}': expected read or write"
        ))),
    }
}

fn run_browser_or_vault_action(
    action: &str,
    flags: &ParsedFlags,
    runtime: &openbot_runtime::OpenBotHostRuntime,
) -> Result<Value, CliError> {
    match action {
        "browser-run" => {
            let steps_val = parse_json_value(flags.require("--steps")?, "browser steps")?;
            let steps = steps_val.as_array().cloned().ok_or_else(|| {
                CliError::Validation("browser steps must be a JSON array".to_string())
            })?;
            let timeout_ms = match flags.get("--timeout-ms") {
                Some(raw) => Some(parse_u64_domain(raw, "timeout-ms")?),
                None => None,
            };
            let req = BrowserRunStepsRequest {
                agent_id: Some(flags.require("--agent-id")?.to_string()),
                tab_id: flags.get("--tab-id").map(str::to_string),
                url: flags.get("--url").map(str::to_string),
                steps,
                timeout_ms,
                final_snapshot: Some(true),
            };
            let out = runtime
                .browser()
                .run_script_steps(req)
                .map_err(map_runtime_error)?;
            to_json_val(out, "browser-run")
        }
        "vault-secret" => {
            let digits = match flags.get("--digits") {
                Some(raw) => {
                    let parsed = parse_u16_domain(raw, "digits")?;
                    if parsed > u8::MAX as u16 {
                        return Err(CliError::Validation(
                            "digits value exceeds supported range".to_string(),
                        ));
                    }
                    Some(parsed as u8)
                }
                None => None,
            };
            let submit_target = match flags.get("--submit-target") {
                Some(raw) => {
                    let val = parse_json_value(raw, "submit-target")?;
                    let target: BrowserTarget = serde_json::from_value(val).map_err(|e| {
                        CliError::Validation(format!("invalid submit-target specification: {e}"))
                    })?;
                    Some(target)
                }
                None => None,
            };
            let targets = match flags.get("--targets") {
                Some(raw) => {
                    let val = parse_json_value(raw, "targets")?;
                    serde_json::from_value::<Vec<BrowserTarget>>(val).map_err(|e| {
                        CliError::Validation(format!("invalid targets specification: {e}"))
                    })?
                }
                None => Vec::new(),
            };
            let req = SubmitSecretRequest {
                tab_id: flags.require("--tab-id")?.to_string(),
                method: flags.require("--method")?.to_string(),
                targets,
                digits,
                submission: flags.require("--submission")?.to_string(),
                submit_target,
                login_id: flags.get("--login-id").map(str::to_string),
                username: flags.get("--username").map(str::to_string),
                agent_id: Some(flags.require("--agent-id")?.to_string()),
            };
            let out = runtime
                .browser()
                .submit_secret(req)
                .map_err(map_runtime_error)?;
            to_json_val(out, "vault-secret")
        }
        "link-payment" => {
            let amount = parse_i64_domain(flags.require("--amount")?, "payment amount")?;
            let req = LinkPaymentRequest {
                amount,
                currency: flags.get("--currency").map(str::to_string),
                merchant_name: flags.require("--merchant-name")?.to_string(),
                merchant_url: flags.require("--merchant-url")?.to_string(),
                context: flags.require("--context")?.to_string(),
                approved: flags.has("--approve"),
                agent_id: Some(flags.require("--agent-id")?.to_string()),
            };
            let out = runtime
                .vault()
                .request_link_payment(req)
                .map_err(map_runtime_error)?;
            to_json_val(out, "link-payment")
        }
        "fill-card" => {
            let mut targets = match flags.get("--targets") {
                Some(raw) => {
                    let val = parse_json_value(raw, "card fill targets")?;
                    let parsed: CardFillTargets = serde_json::from_value(val).map_err(|e| {
                        CliError::Validation(format!("invalid card fill targets: {e}"))
                    })?;
                    Some(parsed)
                }
                None => None,
            };
            if let Some(raw_expiry) = flags.get("--expiry") {
                validate_cli_card_expiry(raw_expiry)?;
                targets
                    .get_or_insert_with(CardFillTargets::default)
                    .expiry
                    .get_or_insert_with(|| BrowserTarget::Css {
                        selector: "input[autocomplete=cc-exp]".to_string(),
                        nth: None,
                    });
            }
            if let Some(raw_exp_month) = flags.get("--exp-month") {
                validate_cli_card_exp_month(raw_exp_month)?;
                targets
                    .get_or_insert_with(CardFillTargets::default)
                    .exp_month
                    .get_or_insert_with(|| BrowserTarget::Css {
                        selector: "input[autocomplete=cc-exp-month]".to_string(),
                        nth: None,
                    });
            }
            if let Some(raw_exp_year) = flags.get("--exp-year") {
                validate_cli_card_exp_year(raw_exp_year)?;
                targets
                    .get_or_insert_with(CardFillTargets::default)
                    .exp_year
                    .get_or_insert_with(|| BrowserTarget::Css {
                        selector: "input[autocomplete=cc-exp-year]".to_string(),
                        nth: None,
                    });
            }
            let timeout_ms = match flags.get("--timeout-ms") {
                Some(raw) => Some(parse_u64_domain(raw, "timeout-ms")?),
                None => None,
            };
            let req = FillCardRequest {
                tab_id: flags.require("--tab-id")?.to_string(),
                card_id: flags.require("--card-id")?.to_string(),
                targets,
                approved: flags.has("--approve"),
                agent_id: Some(flags.require("--agent-id")?.to_string()),
                timeout_ms,
            };
            let out = runtime
                .browser()
                .fill_card(req)
                .map_err(map_runtime_error)?;
            to_json_val(out, "fill-card")
        }
        "takeover" => {
            let req = TakeoverRequest {
                tab_id: flags.require("--tab-id")?.to_string(),
                reason: flags.get("--reason").map(str::to_string),
                agent_id: Some(flags.require("--agent-id")?.to_string()),
            };
            let out = runtime
                .browser()
                .request_takeover(req)
                .map_err(map_runtime_error)?;
            to_json_val(out, "takeover")
        }
        other => Err(CliError::UnknownCommand(format!(
            "Unknown action '{other}' for 'openbot tools'"
        ))),
    }
}

fn is_valid_card_exp_month_part(part: &str) -> bool {
    (1..=2).contains(&part.len())
        && part.bytes().all(|b| b.is_ascii_digit())
        && part
            .parse::<u16>()
            .ok()
            .is_some_and(|m| (1..=12).contains(&m))
}

fn is_valid_card_exp_year_part(part: &str) -> bool {
    if !part.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    match part.len() {
        2 => part.parse::<u16>().ok().is_some_and(|y| y <= 99),
        4 => part
            .parse::<u16>()
            .ok()
            .is_some_and(|y| (2000..=2099).contains(&y)),
        _ => false,
    }
}

fn validate_cli_card_expiry(raw: &str) -> Result<(), CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::Validation(
            "invalid --expiry: value must not be empty".to_string(),
        ));
    }
    let mut parts = trimmed.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(mm), Some(yy), None)
            if is_valid_card_exp_month_part(mm) && is_valid_card_exp_year_part(yy) =>
        {
            Ok(())
        }
        _ => Err(CliError::Validation(format!(
            "invalid --expiry '{raw}': expected MM/YY or MM/YYYY with month 1..=12 and year 00..=99 or 2000..=2099"
        ))),
    }
}

fn validate_cli_card_exp_month(raw: &str) -> Result<(), CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::Validation(
            "invalid --exp-month: value must not be empty".to_string(),
        ));
    }
    if !is_valid_card_exp_month_part(trimmed) {
        return Err(CliError::Validation(format!(
            "invalid --exp-month '{raw}': expected 1..=12"
        )));
    }
    Ok(())
}

fn validate_cli_card_exp_year(raw: &str) -> Result<(), CliError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CliError::Validation(
            "invalid --exp-year: value must not be empty".to_string(),
        ));
    }
    if !is_valid_card_exp_year_part(trimmed) {
        return Err(CliError::Validation(format!(
            "invalid --exp-year '{raw}': expected YY (00..=99) or YYYY (2000..=2099)"
        )));
    }
    Ok(())
}

