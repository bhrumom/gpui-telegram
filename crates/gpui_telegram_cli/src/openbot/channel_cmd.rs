//! `openbot channel <action>` handler covering all 17 multi-bot group-chat actions (`OB-F02`, `OB-F08`, `OB-F09`, `OB-F17`).

use std::collections::HashMap;
use std::path::Path;

use openbot_contracts::channels::{
    ChannelBrowser, ChannelIcon, ChannelIconName, ChannelMember, ChannelMessage,
};
use openbot_runtime::{
    ChannelAssignInput, ChannelCancelInput, ChannelContinueInput, ChannelHistoryPacketInput,
    ChannelReassignInput, ChannelReplyInput, ChannelResultInput, ChannelTransferInput,
    CreateChannelInput, PostChannelMessageInput, UpdateChannelInput,
};
use serde_json::{Value, json};

use super::{
    FlagSpec, ParsedFlags, map_runtime_error, open_host_runtime, parse_csv, parse_json_value,
    parse_usize_domain,
};
use crate::error::CliError;

pub fn flag_spec(action: &str) -> Result<FlagSpec, CliError> {
    match action {
        "create" => Ok(FlagSpec {
            value_flags: &[
                "--name",
                "--members",
                "--title",
                "--instructions",
                "--lead",
                "--browser",
                "--icon",
                "--folders",
                "--image-url",
                "--operation-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &["--isolated"],
            required_flags: &["--name", "--members"],
        }),
        "list" => Ok(FlagSpec {
            value_flags: &["--channel-id", "--query"],
            bool_flags: &["--rank-mentions"],
            opt_bool_flags: &[],
            required_flags: &[],
        }),
        "get" | "archive" | "restore" | "delete" => Ok(FlagSpec {
            value_flags: &["--channel-id", "--operation-id"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--channel-id"],
        }),
        "update" => Ok(FlagSpec {
            value_flags: &[
                "--channel-id",
                "--name",
                "--title",
                "--instructions",
                "--members",
                "--lead",
                "--browser",
                "--icon",
                "--folders",
                "--image-url",
                "--operation-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &["--isolated"],
            required_flags: &["--channel-id"],
        }),
        "post" => Ok(FlagSpec {
            value_flags: &[
                "--channel-id",
                "--text",
                "--reply-to",
                "--recipient-agent-id",
                "--origin-routine",
                "--router-mock",
                "--resources-map",
                "--actor-id",
                "--actor-name",
                "--operation-id",
            ],
            bool_flags: &["--hold-running", "--simulate-stale-routing-state"],
            opt_bool_flags: &[],
            required_flags: &["--channel-id", "--text"],
        }),
        "assign" => Ok(FlagSpec {
            value_flags: &[
                "--channel-id",
                "--caller-task-id",
                "--agent-id",
                "--recipient-agent-id",
                "--task",
                "--expected-result",
                "--source-message-ids",
                "--dependencies",
                "--resources",
                "--operation-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &[
                "--channel-id",
                "--caller-task-id",
                "--agent-id",
                "--recipient-agent-id",
                "--task",
                "--source-message-ids",
            ],
        }),
        "transfer" => Ok(FlagSpec {
            value_flags: &[
                "--channel-id",
                "--task-id",
                "--agent-id",
                "--recipient-agent-id",
                "--task",
                "--expected-result",
                "--source-message-ids",
                "--dependencies",
                "--resources",
                "--operation-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &[
                "--channel-id",
                "--task-id",
                "--agent-id",
                "--recipient-agent-id",
                "--task",
                "--source-message-ids",
            ],
        }),
        "reply" => Ok(FlagSpec {
            value_flags: &[
                "--channel-id",
                "--task-id",
                "--agent-id",
                "--message-id",
                "--text",
                "--turn-id",
                "--operation-id",
            ],
            bool_flags: &["--simulate-peer-failure"],
            opt_bool_flags: &[],
            required_flags: &[
                "--channel-id",
                "--task-id",
                "--agent-id",
                "--message-id",
                "--text",
            ],
        }),
        "result" => Ok(FlagSpec {
            value_flags: &[
                "--channel-id",
                "--task-id",
                "--agent-id",
                "--text",
                "--turn-id",
                "--operation-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--channel-id", "--task-id", "--agent-id", "--text"],
        }),
        "stop" => Ok(FlagSpec {
            value_flags: &["--channel-id", "--task-id", "--operation-id"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--channel-id", "--task-id"],
        }),
        "cancel" => Ok(FlagSpec {
            value_flags: &[
                "--channel-id",
                "--task-id",
                "--caller-agent-id",
                "--caller-task-id",
                "--origin-routine",
                "--operation-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--channel-id", "--task-id"],
        }),
        "continue" => Ok(FlagSpec {
            value_flags: &[
                "--channel-id",
                "--task-id",
                "--recipient-agent-id",
                "--operation-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--channel-id", "--task-id"],
        }),
        "reassign" => Ok(FlagSpec {
            value_flags: &[
                "--channel-id",
                "--task-id",
                "--recipient-agent-id",
                "--operation-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--channel-id", "--task-id", "--recipient-agent-id"],
        }),
        "history-packet" => Ok(FlagSpec {
            value_flags: &[
                "--channel-id",
                "--task-id",
                "--agent-id",
                "--requested-budget",
            ],
            bool_flags: &["--second-turn", "--simulate-large-transcript", "--resumed"],
            opt_bool_flags: &[],
            required_flags: &["--channel-id", "--agent-id"],
        }),
        other => Err(CliError::UnknownCommand(format!(
            "Unknown action '{other}' for 'openbot channel'"
        ))),
    }
}

fn parse_members_csv(raw: &str) -> Vec<ChannelMember> {
    parse_csv(raw)
        .into_iter()
        .map(|agent_id| ChannelMember { agent_id })
        .collect()
}

fn parse_channel_browser(raw: &str) -> Result<ChannelBrowser, CliError> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("none") {
        return Ok(ChannelBrowser::None);
    }
    if trimmed.eq_ignore_ascii_case("members") {
        return Ok(ChannelBrowser::Members);
    }
    let mut val = parse_json_value(raw, "channel browser")?;
    if let Some(obj) = val.as_object_mut()
        && !obj.contains_key("agentId")
        && let Some(v) = obj.remove("agent_id")
    {
        obj.insert("agentId".to_string(), v);
    }
    serde_json::from_value(val)
        .map_err(|e| CliError::Validation(format!("invalid channel browser configuration: {e}")))
}

fn parse_channel_icon(raw: &str) -> Result<ChannelIcon, CliError> {
    let val = parse_json_value(raw, "channel icon")?;
    let icon: ChannelIcon = serde_json::from_value(val)
        .map_err(|e| CliError::Validation(format!("invalid channel icon specification: {e}")))?;
    if ChannelIconName::parse(&icon.name).is_none() {
        return Err(CliError::Validation(format!(
            "unsupported channel icon name '{}': expected one of the 20 built-in icon names",
            icon.name
        )));
    }
    if icon.hue >= 360 {
        return Err(CliError::Validation(format!(
            "invalid channel icon hue {}: expected hue in range 0..360",
            icon.hue
        )));
    }
    Ok(icon)
}

fn parse_string_list_or_csv(raw: &str, label: &str) -> Result<Vec<String>, CliError> {
    let trimmed = raw.trim();
    if trimmed.starts_with('[') {
        let val = parse_json_value(trimmed, label)?;
        serde_json::from_value::<Vec<String>>(val)
            .map_err(|e| CliError::Validation(format!("invalid {label} string array: {e}")))
    } else {
        Ok(parse_csv(trimmed))
    }
}

fn parse_resources_map(raw: &str) -> Result<HashMap<String, Vec<String>>, CliError> {
    let val = parse_json_value(raw, "resources-map")?;
    serde_json::from_value(val)
        .map_err(|e| CliError::Validation(format!("invalid resources-map JSON object: {e}")))
}

fn flatten_channel_message(msg: &ChannelMessage) -> Value {
    let mut val = serde_json::to_value(msg).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("text".to_string(), Value::String(msg.message.text.clone()));
        obj.insert(
            "item_type".to_string(),
            match &msg.message.item_type {
                Some(it) => Value::String(it.clone()),
                None => Value::Null,
            },
        );
        let reply_to = msg
            .message
            .reply_to_message_id
            .as_ref()
            .and_then(|inner| inner.as_ref())
            .cloned();
        obj.insert(
            "reply_to_message_id".to_string(),
            match reply_to {
                Some(r) => Value::String(r),
                None => Value::Null,
            },
        );
    }
    val
}

fn to_json_val<T: serde::Serialize>(val: T, label: &str) -> Result<Value, CliError> {
    serde_json::to_value(val)
        .map_err(|e| CliError::Execution(format!("failed to serialize {label}: {e}")))
}

pub fn run_channel(action: &str, flags: &ParsedFlags, data_dir: &Path) -> Result<Value, CliError> {
    let runtime = open_host_runtime(data_dir)?;
    let chs = runtime.channels();

    match action {
        "create" => {
            let browser = match flags.get("--browser") {
                Some(raw) => Some(parse_channel_browser(raw)?),
                None => None,
            };
            let icon = match flags.get("--icon") {
                Some(raw) => Some(parse_channel_icon(raw)?),
                None => None,
            };
            let folders = match flags.get("--folders") {
                Some(raw) => Some(parse_string_list_or_csv(raw, "folders")?),
                None => None,
            };
            let input = CreateChannelInput {
                channel_id: None,
                name: flags.require("--name")?.to_string(),
                title: flags.get("--title").unwrap_or("").to_string(),
                instructions: flags.get("--instructions").unwrap_or("").to_string(),
                members: parse_members_csv(flags.require("--members")?),
                lead_agent_id: flags
                    .get("--lead")
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                isolated: flags.get_opt_bool("--isolated", "isolated")?,
                browser,
                icon,
                image_url: flags.get("--image-url").map(str::to_string),
                folders,
                operation_id: flags.get("--operation-id").map(str::to_string),
            };
            let ch = chs.create(input).map_err(map_runtime_error)?;
            Ok(json!({ "channel": ch }))
        }
        "list" => {
            if flags.has("--rank-mentions") {
                let cid = flags.require("--channel-id")?;
                let query = flags.get("--query").unwrap_or("");
                let ranked = chs.rank_mentions(cid, query).map_err(map_runtime_error)?;
                Ok(json!({ "ranked_members": ranked }))
            } else {
                let channels = chs.list(None, false).map_err(map_runtime_error)?;
                Ok(json!({ "channels": channels }))
            }
        }
        "get" => {
            let cid = flags.require("--channel-id")?;
            let detail = chs.get_detail(cid).map_err(map_runtime_error)?;
            let messages: Vec<Value> = detail
                .messages
                .iter()
                .map(flatten_channel_message)
                .collect();
            Ok(json!({
                "channel": detail.channel,
                "tasks": detail.tasks,
                "messages": messages,
                "assignments": detail.assignments,
                "waits": detail.waits,
                "contexts": detail.contexts,
                "memories": detail.memories,
                "routines": detail.routines,
                "summary": detail.summary,
            }))
        }
        "update" => run_channel_update(flags, chs),
        "archive" => {
            let cid = flags.require("--channel-id")?;
            let ch = chs
                .archive(cid, flags.get("--operation-id"))
                .map_err(map_runtime_error)?;
            Ok(json!({ "channel": ch }))
        }
        "restore" => {
            let cid = flags.require("--channel-id")?;
            let ch = chs
                .restore(cid, flags.get("--operation-id"))
                .map_err(map_runtime_error)?;
            Ok(json!({ "channel": ch }))
        }
        "delete" => {
            let cid = flags.require("--channel-id")?;
            chs.delete(cid, flags.get("--operation-id"))
                .map_err(map_runtime_error)?;
            Ok(json!({ "deleted": true, "channel_id": cid }))
        }
        _ => run_channel_workflow_action(action, flags, chs),
    }
}

fn run_channel_update(
    flags: &ParsedFlags,
    chs: &openbot_runtime::ChannelService,
) -> Result<Value, CliError> {
    let browser = match flags.get("--browser") {
        Some(raw) => Some(parse_channel_browser(raw)?),
        None => None,
    };
    let icon = match flags.get("--icon") {
        Some(raw) => {
            if raw.trim().is_empty() || raw.trim().eq_ignore_ascii_case("null") {
                Some(None)
            } else {
                Some(Some(parse_channel_icon(raw)?))
            }
        }
        None => None,
    };
    let lead_agent_id = flags.get("--lead").map(|raw| {
        let t = raw.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("null") {
            None
        } else {
            Some(t.to_string())
        }
    });
    let image_url = flags.get("--image-url").map(|raw| {
        let t = raw.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("null") {
            None
        } else {
            Some(t.to_string())
        }
    });
    let folders = match flags.get("--folders") {
        Some(raw) => Some(parse_string_list_or_csv(raw, "folders")?),
        None => None,
    };
    let members = flags.get("--members").map(parse_members_csv);

    let input = UpdateChannelInput {
        channel_id: flags.require("--channel-id")?.to_string(),
        name: flags.get("--name").map(str::to_string),
        title: flags.get("--title").map(str::to_string),
        instructions: flags.get("--instructions").map(str::to_string),
        members,
        lead_agent_id,
        isolated: flags.get_opt_bool("--isolated", "isolated")?,
        browser,
        icon,
        image_url,
        folders,
        operation_id: flags.get("--operation-id").map(str::to_string),
    };
    let ch = chs.update(input).map_err(map_runtime_error)?;
    Ok(json!({ "channel": ch }))
}

fn run_channel_workflow_action(
    action: &str,
    flags: &ParsedFlags,
    chs: &openbot_runtime::ChannelService,
) -> Result<Value, CliError> {
    match action {
        "post" => {
            let resources_map = match flags.get("--resources-map") {
                Some(raw) => parse_resources_map(raw)?,
                None => HashMap::new(),
            };
            let input = PostChannelMessageInput {
                channel_id: flags.require("--channel-id")?.to_string(),
                text: flags.require("--text")?.to_string(),
                actor_id: flags.get("--actor-id").map(str::to_string),
                actor_name: flags.get("--actor-name").map(str::to_string),
                recipient_agent_id: flags.get("--recipient-agent-id").map(str::to_string),
                reply_to_message_id: flags.get("--reply-to").map(str::to_string),
                attachment_draft_ids: Vec::new(),
                origin_routine_id: flags.get("--origin-routine").map(str::to_string),
                origin_routine_name: None,
                origin_agent_id: None,
                sender_task_id: None,
                operation_id: flags.get("--operation-id").map(str::to_string),
                router_mock: flags.get("--router-mock").map(str::to_string),
                hold_running: flags.has("--hold-running"),
                resources_map,
                simulate_stale_routing_state: flags.has("--simulate-stale-routing-state"),
            };
            let out = chs.post(input).map_err(map_runtime_error)?;
            to_json_val(out, "channel post")
        }
        "assign" => {
            let resources = match flags.get("--resources") {
                Some(raw) => parse_string_list_or_csv(raw, "resources")?,
                None => Vec::new(),
            };
            let dependencies = flags.get("--dependencies").map(parse_csv).unwrap_or_default();
            let input = ChannelAssignInput {
                channel_id: flags.require("--channel-id")?.to_string(),
                caller_task_id: flags.require("--caller-task-id")?.to_string(),
                agent_id: flags.require("--agent-id")?.to_string(),
                recipient_agent_id: flags.require("--recipient-agent-id")?.to_string(),
                task: flags.require("--task")?.to_string(),
                expected_result: flags.get("--expected-result").map(str::to_string),
                source_message_ids: parse_csv(flags.require("--source-message-ids")?),
                resources,
                dependencies,
                operation_id: flags.get("--operation-id").map(str::to_string),
            };
            let out = chs.assign(input).map_err(map_runtime_error)?;
            to_json_val(out, "channel assign")
        }
        "transfer" => {
            let resources = match flags.get("--resources") {
                Some(raw) => parse_string_list_or_csv(raw, "resources")?,
                None => Vec::new(),
            };
            let dependencies = flags.get("--dependencies").map(parse_csv).unwrap_or_default();
            let input = ChannelTransferInput {
                channel_id: flags.require("--channel-id")?.to_string(),
                task_id: flags.require("--task-id")?.to_string(),
                agent_id: flags.require("--agent-id")?.to_string(),
                recipient_agent_id: flags.require("--recipient-agent-id")?.to_string(),
                task: flags.require("--task")?.to_string(),
                expected_result: flags.get("--expected-result").map(str::to_string),
                source_message_ids: parse_csv(flags.require("--source-message-ids")?),
                resources,
                dependencies,
                operation_id: flags.get("--operation-id").map(str::to_string),
            };
            let out = chs.transfer(input).map_err(map_runtime_error)?;
            to_json_val(out, "channel transfer")
        }
        "reply" => {
            let input = ChannelReplyInput {
                channel_id: flags.require("--channel-id")?.to_string(),
                task_id: flags.require("--task-id")?.to_string(),
                agent_id: flags.require("--agent-id")?.to_string(),
                message_id: flags.require("--message-id")?.to_string(),
                text: flags.require("--text")?.to_string(),
                turn_id: flags.get("--turn-id").map(str::to_string),
                simulate_peer_failure: flags.has("--simulate-peer-failure"),
                operation_id: flags.get("--operation-id").map(str::to_string),
            };
            let out = chs.reply(input).map_err(map_runtime_error)?;
            to_json_val(out, "channel reply")
        }
        "result" => {
            let input = ChannelResultInput {
                channel_id: flags.require("--channel-id")?.to_string(),
                task_id: flags.require("--task-id")?.to_string(),
                agent_id: flags.require("--agent-id")?.to_string(),
                text: flags.require("--text")?.to_string(),
                turn_id: flags.get("--turn-id").map(str::to_string),
                operation_id: flags.get("--operation-id").map(str::to_string),
            };
            let out = chs.result(input).map_err(map_runtime_error)?;
            to_json_val(out, "channel result")
        }
        "stop" => {
            let cid = flags.require("--channel-id")?;
            let tid = flags.require("--task-id")?;
            let op_id = flags.get("--operation-id");
            let task = chs.stop_task(cid, tid, op_id).map_err(map_runtime_error)?;
            Ok(json!({ "task": task }))
        }
        "cancel" => {
            let input = ChannelCancelInput {
                channel_id: flags.require("--channel-id")?.to_string(),
                task_id: flags.require("--task-id")?.to_string(),
                caller_agent_id: flags.get("--caller-agent-id").map(str::to_string),
                caller_task_id: flags.get("--caller-task-id").map(str::to_string),
                origin_routine_id: flags.get("--origin-routine").map(str::to_string),
                operation_id: flags.get("--operation-id").map(str::to_string),
            };
            let out = chs.cancel(input).map_err(map_runtime_error)?;
            to_json_val(out, "channel cancel")
        }
        "continue" => {
            let input = ChannelContinueInput {
                channel_id: flags.require("--channel-id")?.to_string(),
                task_id: flags.require("--task-id")?.to_string(),
                recipient_agent_id: flags.get("--recipient-agent-id").map(str::to_string),
                operation_id: flags.get("--operation-id").map(str::to_string),
            };
            let task = chs.continue_task(input).map_err(map_runtime_error)?;
            Ok(json!({ "task": task }))
        }
        "reassign" => {
            let input = ChannelReassignInput {
                channel_id: flags.require("--channel-id")?.to_string(),
                task_id: flags.require("--task-id")?.to_string(),
                recipient_agent_id: flags.require("--recipient-agent-id")?.to_string(),
                operation_id: flags.get("--operation-id").map(str::to_string),
            };
            let task = chs.reassign(input).map_err(map_runtime_error)?;
            Ok(json!({ "task": task }))
        }
        "history-packet" => {
            let requested_budget = match flags.get("--requested-budget") {
                Some(raw) => Some(parse_usize_domain(raw, "requested budget")?),
                None => None,
            };
            let input = ChannelHistoryPacketInput {
                channel_id: flags.require("--channel-id")?.to_string(),
                task_id: flags.get("--task-id").map(str::to_string),
                agent_id: flags.require("--agent-id")?.to_string(),
                second_turn: flags.has("--second-turn"),
                simulate_large_transcript: flags.has("--simulate-large-transcript"),
                resumed: flags.has("--resumed"),
                requested_budget,
                mode_override: None,
            };
            let packet = chs.history_packet(input).map_err(map_runtime_error)?;
            Ok(json!({ "packet": packet }))
        }
        other => Err(CliError::UnknownCommand(format!(
            "Unknown action '{other}' for 'openbot channel'"
        ))),
    }
}
