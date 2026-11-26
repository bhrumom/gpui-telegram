//! `openbot agent <action>` handler covering all 9 lifecycle actions (`OB-F01`, `OB-F02`, `OB-F06`, `OB-F11`).

use std::path::Path;

use openbot_contracts::agents::{
    AGENT_BACKUP_MODEL_LIMIT, AgentBackupModels, AgentModelChoice, AgentSummary, UpdateAgentInput,
};
use openbot_contracts::providers::{
    AgentProviderId, CustomProviderEndpoint, CustomProviderHarness,
};
use openbot_runtime::{CreateAgentRequest, GenerateProfileRequest};
use openbot_storage::OrchestrationEventInput;
use serde_json::{Map, Value, json};

use super::{
    FlagSpec, ParsedFlags, map_runtime_error, map_storage_error, open_host_runtime, parse_access,
    parse_avatar_hue, parse_csv, parse_json_value, parse_provider, parse_reasoning_effort,
};
use crate::error::CliError;

pub fn flag_spec(action: &str) -> Result<FlagSpec, CliError> {
    match action {
        "create" => Ok(FlagSpec {
            value_flags: &[
                "--name",
                "--id",
                "--title",
                "--description",
                "--provider",
                "--model",
                "--reasoning-effort",
                "--access",
                "--avatar-seed",
                "--avatar-hue",
                "--avatar-color",
                "--backup-models",
                "--custom-endpoint",
                "--probe-servers",
                "--command-id",
                "--initial-message",
                "--provider-account-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &[
                "--computer-use",
                "--allow-automation",
                "--fast-mode",
                "--ultrafast",
            ],
            required_flags: &["--name"],
        }),
        "list" => Ok(FlagSpec {
            value_flags: &[],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &[],
        }),
        "get" | "delete" => Ok(FlagSpec {
            value_flags: &["--id"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--id"],
        }),
        "update" => Ok(FlagSpec {
            value_flags: &[
                "--id",
                "--name",
                "--title",
                "--description",
                "--provider",
                "--model",
                "--reasoning-effort",
                "--access",
                "--avatar-seed",
                "--avatar-hue",
                "--avatar-color",
                "--backup-models",
                "--provider-account-id",
            ],
            bool_flags: &["--leave-usage-limit"],
            opt_bool_flags: &[
                "--computer-use",
                "--allow-automation",
                "--fast-mode",
                "--ultrafast",
            ],
            required_flags: &["--id"],
        }),
        "duplicate" => Ok(FlagSpec {
            value_flags: &["--id", "--operation-id"],
            bool_flags: &["--simulate-concurrent-workspace-mutation"],
            opt_bool_flags: &[],
            required_flags: &["--id", "--operation-id"],
        }),
        "generate-profile" => Ok(FlagSpec {
            value_flags: &[
                "--prompt",
                "--provider",
                "--model",
                "--operation-id",
                "--language",
                "--section-id",
            ],
            bool_flags: &["--save"],
            opt_bool_flags: &[],
            required_flags: &["--prompt"],
        }),
        "export" => Ok(FlagSpec {
            value_flags: &["--ids", "--out"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--ids", "--out"],
        }),
        "import" => Ok(FlagSpec {
            value_flags: &["--archive", "--keys"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--archive"],
        }),
        other => Err(CliError::UnknownCommand(format!(
            "Unknown action '{other}' for 'openbot agent'"
        ))),
    }
}

pub fn agent_summary_json(agent: &AgentSummary) -> Value {
    let mut val = serde_json::to_value(agent).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.remove("temperature");
        obj.insert(
            "concurrent_turns".to_string(),
            Value::Bool(agent.provider.descriptor().concurrent_turns),
        );
        obj.entry("allowAutomation".to_string())
            .or_insert_with(|| Value::Bool(agent.allow_automation.unwrap_or(false)));
        obj.entry("fastMode".to_string())
            .or_insert_with(|| Value::Bool(agent.fast_mode.unwrap_or(false)));
        obj.entry("ultrafast".to_string())
            .or_insert_with(|| Value::Bool(agent.ultrafast.unwrap_or(false)));
    }
    val
}

fn parse_backup_models_json(raw: &str) -> Result<AgentBackupModels, CliError> {
    let parsed = parse_json_value(raw, "backup-models")?;
    let (automatic, items): (bool, &Vec<Value>) = match &parsed {
        Value::Array(arr) => (true, arr),
        Value::Object(map) => {
            let auto = map
                .get("automatic")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let arr = map
                .get("order")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    CliError::Validation(
                        "backupModels object must contain an 'order' array".to_string(),
                    )
                })?;
            (auto, arr)
        }
        _ => {
            return Err(CliError::Validation(
                "backupModels must be a JSON array or {automatic, order} object".to_string(),
            ));
        }
    };

    if items.len() > AGENT_BACKUP_MODEL_LIMIT {
        return Err(CliError::Validation(format!(
            "backupModels exceeds maximum limit of {AGENT_BACKUP_MODEL_LIMIT} entries"
        )));
    }

    let mut order = Vec::with_capacity(items.len());
    for entry in items {
        let obj = entry.as_object().ok_or_else(|| {
            CliError::Validation("each backup model entry must be a JSON object".to_string())
        })?;
        let prov_raw = obj
            .get("provider")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CliError::Validation("backup model entry missing string 'provider'".to_string())
            })?;
        let provider = parse_provider(prov_raw)?;
        let model = obj
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .ok_or_else(|| {
                CliError::Validation("backup model entry missing non-empty 'model'".to_string())
            })?
            .to_string();
        let provider_account_id = obj
            .get("providerAccountId")
            .or_else(|| obj.get("provider_account_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        order.push(AgentModelChoice {
            provider,
            model,
            provider_account_id,
        });
    }

    Ok(AgentBackupModels { automatic, order })
}

fn parse_custom_endpoint(raw: &str) -> Result<CustomProviderEndpoint, CliError> {
    let mut val = parse_json_value(raw, "custom-endpoint")?;
    let Some(obj) = val.as_object_mut() else {
        return Err(CliError::Validation(
            "custom-endpoint must be a JSON object".to_string(),
        ));
    };
    if !obj.contains_key("baseUrl")
        && let Some(v) = obj.remove("base_url")
    {
        obj.insert("baseUrl".to_string(), v);
    }
    if !obj.contains_key("apiKey") {
        if let Some(v) = obj.remove("api_key") {
            obj.insert("apiKey".to_string(), v);
        } else {
            obj.insert("apiKey".to_string(), Value::Null);
        }
    }
    if let Some(Value::Array(models)) = obj.get_mut("models") {
        for m in models.iter_mut() {
            if let Some(mobj) = m.as_object_mut() {
                if !mobj.contains_key("contextWindow")
                    && let Some(v) = mobj.remove("context_window")
                {
                    mobj.insert("contextWindow".to_string(), v);
                }
                if !mobj.contains_key("maxOutput")
                    && let Some(v) = mobj.remove("max_output")
                {
                    mobj.insert("maxOutput".to_string(), v);
                }
            }
        }
    }
    serde_json::from_value(val)
        .map_err(|e| CliError::Validation(format!("invalid custom-endpoint specification: {e}")))
}

fn sanitize_detected_servers(servers: Value) -> Value {
    match servers {
        Value::Array(arr) => Value::Array(arr.into_iter().map(sanitize_detected_servers).collect()),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                let low = k.to_ascii_lowercase();
                if low == "apikey" || low == "api_key" || low == "secret" {
                    continue;
                }
                out.insert(k, sanitize_detected_servers(v));
            }
            Value::Object(out)
        }
        other => other,
    }
}

pub async fn run_agent(
    action: &str,
    flags: &ParsedFlags,
    data_dir: &Path,
) -> Result<Value, CliError> {
    let runtime = open_host_runtime(data_dir)?;

    match action {
        "create" => {
            let command_id = flags.get("--command-id").map(str::trim).filter(|s| !s.is_empty());
            if let Some(cid) = command_id
                && let Some((_, cached)) = runtime
                    .database()
                    .command_result::<Value>(cid)
                    .map_err(map_storage_error)?
            {
                return Ok(cached);
            }

            let mut provider = match flags.get("--provider") {
                Some(raw) => Some(parse_provider(raw)?),
                None => None,
            };
            let mut model = flags.get("--model").map(str::to_string);

            if let Some(raw_ep) = flags.get("--custom-endpoint") {
                let endpoint = parse_custom_endpoint(raw_ep)?;
                let first_model_id = endpoint.models.first().map(|m| m.id.clone());
                let endpoint_id = endpoint.id.clone();
                let harness = endpoint.harness;
                runtime
                    .providers()
                    .save_custom_provider(endpoint)
                    .map_err(map_runtime_error)?;
                if provider.is_none() {
                    provider = match harness {
                        Some(CustomProviderHarness::Pi) => Some(AgentProviderId::Pi),
                        Some(CustomProviderHarness::OpenCode) => Some(AgentProviderId::OpenCode),
                        None => Some(AgentProviderId::Pi),
                    };
                }
                if model.is_none()
                    && let Some(mid) = first_model_id
                {
                    model = runtime
                        .providers()
                        .resolve_custom_model_id(&endpoint_id, &mid)
                        .or(Some(mid));
                }
            }

            let reasoning_effort = match flags.get("--reasoning-effort") {
                Some(raw) => Some(parse_reasoning_effort(raw)?),
                None => None,
            };
            let access = match flags.get("--access") {
                Some(raw) => Some(parse_access(raw)?),
                None => None,
            };
            let avatar_hue = match flags.get("--avatar-hue") {
                Some(raw) => Some(parse_avatar_hue(raw)?),
                None => None,
            };
            let avatar_color = flags.get("--avatar-color").map(|raw| {
                let t = raw.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            });
            let backup_models = match flags.get("--backup-models") {
                Some(raw) => Some(parse_backup_models_json(raw)?),
                None => None,
            };

            let req = CreateAgentRequest {
                name: flags.require("--name")?.to_string(),
                title: flags.get("--title").unwrap_or("").to_string(),
                description: flags.get("--description").unwrap_or("").to_string(),
                provider,
                model,
                reasoning_effort,
                provider_account_id: flags.get("--provider-account-id").map(str::to_string),
                access,
                computer_use: flags.get_opt_bool("--computer-use", "computer-use")?,
                allow_automation: flags.get_opt_bool("--allow-automation", "allow-automation")?,
                fast_mode: flags.get_opt_bool("--fast-mode", "fast-mode")?,
                ultrafast: flags.get_opt_bool("--ultrafast", "ultrafast")?,
                backup_models,
                avatar_seed: flags.get("--avatar-seed").map(str::to_string),
                avatar_hue,
                avatar_color,
                initial_message: flags.get("--initial-message").unwrap_or("").to_string(),
                operation_id: command_id.map(str::to_string),
            };

            let agent = runtime
                .agents()
                .create_agent_full(req)
                .map_err(map_runtime_error)?;

            let detected_servers = if let Some(raw_urls) = flags.get("--probe-servers") {
                let urls = parse_csv(raw_urls);
                let found = runtime
                    .providers()
                    .scan_model_servers(&urls)
                    .await
                    .map_err(map_runtime_error)?;
                let raw_json = serde_json::to_value(found).unwrap_or_else(|_| json!([]));
                Some(sanitize_detected_servers(raw_json))
            } else {
                None
            };

            let mut out = json!({ "agent": agent_summary_json(&agent) });
            if let Some(servers) = detected_servers
                && let Some(obj) = out.as_object_mut()
            {
                obj.insert("detected_servers".to_string(), servers);
            }

            if let Some(cid) = command_id {
                let ev = OrchestrationEventInput::new(
                    "agent",
                    &agent.id,
                    "agent.command.created",
                    out.clone(),
                );
                let _ = runtime
                    .database()
                    .dispatch_with_projection(cid, Some(cid), &[ev], |_, _| Ok(out.clone()))
                    .map_err(map_storage_error)?;
            }

            Ok(out)
        }
        "list" => {
            let agents: Vec<Value> = runtime
                .agents()
                .list_agents()
                .iter()
                .map(agent_summary_json)
                .collect();
            Ok(json!({ "agents": agents }))
        }
        "get" => {
            let id = flags.require("--id")?;
            let agent = runtime
                .agents()
                .require_agent(id)
                .map_err(map_runtime_error)?;
            Ok(json!({ "agent": agent_summary_json(&agent) }))
        }
        "update" => run_agent_update(flags, &runtime),
        "delete" => {
            let id = flags.require("--id")?;
            let deleted = runtime
                .agents()
                .delete_agent(id)
                .map_err(map_runtime_error)?;
            if deleted.is_none() {
                return Err(CliError::Validation(format!("agent '{id}' not found")));
            }
            Ok(json!({ "deleted": true, "id": id }))
        }
        "duplicate" => {
            let id = flags.require("--id")?;
            let op_id = flags.require("--operation-id")?;
            let sim = flags.has("--simulate-concurrent-workspace-mutation");
            let agent = runtime
                .agents()
                .duplicate_agent(id, op_id, sim)
                .map_err(map_runtime_error)?;
            Ok(json!({ "agent": agent_summary_json(&agent) }))
        }
        "generate-profile" => {
            let provider = match flags.get("--provider") {
                Some(raw) => Some(parse_provider(raw)?),
                None => None,
            };
            let req = GenerateProfileRequest {
                prompt: flags.require("--prompt")?.to_string(),
                provider,
                model: flags.get("--model").map(str::to_string),
                agent_id: None,
                draft: None,
                save: flags.has("--save"),
                operation_id: flags.get("--operation-id").map(str::to_string),
                language: flags.get("--language").unwrap_or("en").to_string(),
                section_id: flags.get("--section-id").map(str::to_string),
                first_request_follows: false,
            };
            let out = runtime
                .agents()
                .generate_profile(&req)
                .map_err(map_runtime_error)?;
            Ok(json!({
                "draft": out.draft,
                "avatar_profile": out.avatar_profile,
                "welcome_message": out.welcome_message,
                "agent": out.agent.as_ref().map(agent_summary_json),
                "welcome_delivery_id": out.welcome_delivery_id,
            }))
        }
        "export" => {
            let ids = parse_csv(flags.require("--ids")?);
            let out_path = flags.require("--out")?;
            let res = runtime
                .agents()
                .export_archive(&ids, Path::new(out_path))
                .map_err(map_runtime_error)?;
            Ok(json!({
                "exported_count": res.exported_count,
                "out": res.path.display().to_string(),
                "agent_ids": res.agent_ids,
            }))
        }
        "import" => {
            let archive = flags.require("--archive")?;
            let selected_keys = flags.get("--keys").and_then(|raw| {
                let t = raw.trim();
                if t.is_empty() || t.eq_ignore_ascii_case("all") {
                    None
                } else {
                    Some(parse_csv(t))
                }
            });
            let res = runtime
                .agents()
                .import_archive(Path::new(archive), selected_keys.as_deref())
                .map_err(map_runtime_error)?;
            let agents: Vec<Value> = res.agents.iter().map(agent_summary_json).collect();
            Ok(json!({
                "agents": agents,
                "warnings": res.warnings,
                "imported_memories": res.imported_memories,
                "imported_skills": res.imported_skills,
                "imported_files": res.imported_files,
            }))
        }
        other => Err(CliError::UnknownCommand(format!(
            "Unknown action '{other}' for 'openbot agent'"
        ))),
    }
}

fn run_agent_update(
    flags: &ParsedFlags,
    runtime: &openbot_runtime::OpenBotHostRuntime,
) -> Result<Value, CliError> {
    let id = flags.require("--id")?.to_string();
    let provider = match flags.get("--provider") {
        Some(raw) => Some(parse_provider(raw)?),
        None => None,
    };
    let reasoning_effort = match flags.get("--reasoning-effort") {
        Some(raw) => Some(parse_reasoning_effort(raw)?),
        None => None,
    };
    let access = match flags.get("--access") {
        Some(raw) => Some(parse_access(raw)?),
        None => None,
    };
    let avatar_hue = match flags.get("--avatar-hue") {
        Some(raw) => {
            if raw.trim().is_empty() {
                Some(None)
            } else {
                Some(Some(parse_avatar_hue(raw)?))
            }
        }
        None => None,
    };
    let avatar_color = flags.get("--avatar-color").map(|raw| {
        let t = raw.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    });
    let backup_models = match flags.get("--backup-models") {
        Some(raw) => Some(Some(parse_backup_models_json(raw)?)),
        None => None,
    };

    let input = UpdateAgentInput {
        agent_id: id,
        name: flags.get("--name").map(str::to_string),
        title: flags.get("--title").map(str::to_string),
        description: flags.get("--description").map(str::to_string),
        provider,
        model: flags.get("--model").map(str::to_string),
        reasoning_effort,
        access,
        computer_use: flags.get_opt_bool("--computer-use", "computer-use")?,
        allow_automation: flags
            .get_opt_bool("--allow-automation", "allow-automation")?
            .map(Some),
        fast_mode: flags.get_opt_bool("--fast-mode", "fast-mode")?,
        ultrafast: flags.get_opt_bool("--ultrafast", "ultrafast")?,
        provider_account_id: flags
            .get("--provider-account-id")
            .map(|s| (!s.trim().is_empty()).then(|| s.to_string())),
        backup_models,
        leave_usage_limit: flags.has("--leave-usage-limit").then_some(true),
        avatar_seed: flags.get("--avatar-seed").map(str::to_string),
        avatar_hue,
        avatar_color,
        ..Default::default()
    };

    let updated = runtime
        .agents()
        .update_agent(&input, None)
        .map_err(map_runtime_error)?;
    Ok(json!({ "agent": agent_summary_json(&updated) }))
}
