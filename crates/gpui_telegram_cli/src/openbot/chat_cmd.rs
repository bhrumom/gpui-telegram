//! `openbot chat <action>` handler covering all 17 actions (`OB-F02`, `OB-F06`, `OB-F07`, `OB-F15`, `OB-F16`, `OB-F17`).

use std::path::Path;

use openbot_contracts::conversation::ConversationReactionActor;
use openbot_contracts::queue::{AttachmentKind, AttachmentPreviewKind, AttachmentSummary};
use openbot_runtime::{
    ActiveTurnOwnerHint, BranchThreadInput, CompactionReason, InterruptAgentInput,
    MailboxSendInput, PinActor, PinMessageInput, QueueAddInput, QueueSteerInput, ReactMessageInput,
    SendChatMessageInput, SideQuestionInput, SideQuestionTurn,
};
use openbot_storage::SendDirectMessageInput;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    FlagSpec, ParsedFlags, map_runtime_error, open_host_runtime, parse_csv, parse_json_value,
    parse_usize_domain,
};
use crate::error::CliError;

pub fn flag_spec(action: &str) -> Result<FlagSpec, CliError> {
    match action {
        "send" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--text",
                "--task-id",
                "--attachments",
                "--client-message-id",
            ],
            bool_flags: &["--simulate-primary-rate-limit"],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--text"],
        }),
        "queue-add" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--text",
                "--task-id",
                "--attachments",
                "--idempotency-key",
            ],
            bool_flags: &["--welcome"],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--text"],
        }),
        "queue-list" | "queue-pause" | "queue-resume" => Ok(FlagSpec {
            value_flags: &["--agent-id"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--agent-id"],
        }),
        "queue-reorder" => Ok(FlagSpec {
            value_flags: &["--agent-id", "--delivery-ids", "--task-id"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--delivery-ids"],
        }),
        "queue-steer" => Ok(FlagSpec {
            value_flags: &["--agent-id", "--delivery-id", "--expected-turn-id"],
            bool_flags: &["--routine-turn"],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--delivery-id", "--expected-turn-id"],
        }),
        "branch" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--message-id",
                "--request-id",
                "--task-id",
                "--title",
            ],
            bool_flags: &["--simulate-source-busy"],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--message-id", "--request-id"],
        }),
        "side-question" => Ok(FlagSpec {
            value_flags: &["--agent-id", "--question", "--task-id", "--earlier"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--question"],
        }),
        "react" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--task-id",
                "--channel-id",
                "--message-id",
                "--emoji",
                "--actor",
            ],
            bool_flags: &["--remove"],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--message-id", "--emoji"],
        }),
        "pin" => Ok(FlagSpec {
            value_flags: &[
                "--agent-id",
                "--task-id",
                "--channel-id",
                "--message-id",
                "--pinned-by",
            ],
            bool_flags: &["--unpin"],
            opt_bool_flags: &[],
            required_flags: &["--agent-id", "--message-id"],
        }),
        "history" => Ok(FlagSpec {
            value_flags: &["--agent-id", "--task-id", "--limit"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--agent-id"],
        }),
        "compact" => Ok(FlagSpec {
            value_flags: &["--agent-id", "--task-id", "--simulate-usage"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--agent-id"],
        }),
        "dm-send" => Ok(FlagSpec {
            value_flags: &["--sender", "--recipient", "--client-message-id", "--text"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--sender", "--recipient", "--client-message-id", "--text"],
        }),
        "dm-list" => Ok(FlagSpec {
            value_flags: &["--member"],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--member"],
        }),
        "mailbox-send" => Ok(FlagSpec {
            value_flags: &[
                "--sender-agent-id",
                "--recipient-agent-ids",
                "--text",
                "--reply-to",
                "--idempotency-key",
                "--sender-task-id",
            ],
            bool_flags: &[],
            opt_bool_flags: &["--expects-reply"],
            required_flags: &["--sender-agent-id", "--recipient-agent-ids", "--text"],
        }),
        "interrupt" => Ok(FlagSpec {
            value_flags: &[
                "--caller-agent-id",
                "--target-agent-id",
                "--reason",
                "--active-turn-owner",
                "--idempotency-key",
            ],
            bool_flags: &[],
            opt_bool_flags: &[],
            required_flags: &["--caller-agent-id", "--target-agent-id"],
        }),
        other => Err(CliError::UnknownCommand(format!(
            "Unknown action '{other}' for 'openbot chat'"
        ))),
    }
}

fn parse_attachments_json(raw: &str) -> Result<Vec<AttachmentSummary>, CliError> {
    let val = parse_json_value(raw, "attachments")?;
    let arr = val
        .as_array()
        .ok_or_else(|| CliError::Validation("attachments must be a JSON array".to_string()))?;
    let mut out = Vec::with_capacity(arr.len());
    for (idx, item) in arr.iter().enumerate() {
        let obj = item.as_object().ok_or_else(|| {
            CliError::Validation("each attachment entry must be a JSON object".to_string())
        })?;
        let id = obj
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("att-{}-{}", idx + 1, Uuid::new_v4()));
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("attachment.bin")
            .to_string();
        let size = obj
            .get("size")
            .or_else(|| obj.get("bytes"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let kind = match obj
            .get("kind")
            .and_then(Value::as_str)
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("image") => AttachmentKind::Image,
            _ => AttachmentKind::File,
        };
        let mime_type = obj
            .get("mimeType")
            .or_else(|| obj.get("mime_type"))
            .and_then(Value::as_str)
            .unwrap_or(match kind {
                AttachmentKind::Image => "image/png",
                AttachmentKind::File => "application/octet-stream",
            })
            .to_string();
        let preview_kind = match obj
            .get("previewKind")
            .or_else(|| obj.get("preview_kind"))
            .and_then(Value::as_str)
            .map(|s| s.to_ascii_lowercase())
            .as_deref()
        {
            Some("image") => AttachmentPreviewKind::Image,
            Some("pdf") => AttachmentPreviewKind::Pdf,
            Some("text") => AttachmentPreviewKind::Text,
            Some("none") => AttachmentPreviewKind::None,
            _ if kind == AttachmentKind::Image => AttachmentPreviewKind::Image,
            _ => AttachmentPreviewKind::None,
        };
        let preview_url = obj
            .get("previewUrl")
            .or_else(|| obj.get("preview_url"))
            .and_then(Value::as_str)
            .map(str::to_string);
        out.push(AttachmentSummary {
            id,
            name,
            size,
            kind,
            mime_type,
            preview_kind,
            preview_url,
        });
    }
    Ok(out)
}

fn parse_earlier_turns(raw: &str) -> Result<Vec<SideQuestionTurn>, CliError> {
    let val = parse_json_value(raw, "earlier turns")?;
    let arr = val
        .as_array()
        .ok_or_else(|| CliError::Validation("earlier turns must be a JSON array".to_string()))?;
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        let obj = entry.as_object().ok_or_else(|| {
            CliError::Validation("each earlier turn must be a JSON object".to_string())
        })?;
        let question = obj
            .get("question")
            .or_else(|| obj.get("q"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let answer = obj
            .get("answer")
            .or_else(|| obj.get("a"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        out.push(SideQuestionTurn { question, answer });
    }
    Ok(out)
}

fn to_json_val<T: serde::Serialize>(val: T, label: &str) -> Result<Value, CliError> {
    serde_json::to_value(val)
        .map_err(|e| CliError::Execution(format!("failed to serialize {label}: {e}")))
}

pub fn run_chat(action: &str, flags: &ParsedFlags, data_dir: &Path) -> Result<Value, CliError> {
    let runtime = open_host_runtime(data_dir)?;
    let sched = runtime.scheduler();

    match action {
        "send" => {
            let attachments = match flags.get("--attachments") {
                Some(raw) => parse_attachments_json(raw)?,
                None => Vec::new(),
            };
            let input = SendChatMessageInput {
                agent_id: flags.require("--agent-id")?.to_string(),
                text: flags.require("--text")?.to_string(),
                task_id: flags.get("--task-id").map(str::to_string),
                attachments,
                client_message_id: flags.get("--client-message-id").map(str::to_string),
                sender_member: None,
                simulate_primary_rate_limit: flags.has("--simulate-primary-rate-limit"),
            };
            let out = sched.send_message(input).map_err(map_runtime_error)?;
            to_json_val(out, "chat send")
        }
        "queue-add" => {
            let attachments = match flags.get("--attachments") {
                Some(raw) => parse_attachments_json(raw)?,
                None => Vec::new(),
            };
            let input = QueueAddInput {
                agent_id: flags.require("--agent-id")?.to_string(),
                text: flags.require("--text")?.to_string(),
                task_id: flags.get("--task-id").map(str::to_string),
                attachments,
                welcome: flags.has("--welcome"),
                sender: None,
                sender_member: None,
                idempotency_key: flags.get("--idempotency-key").map(str::to_string),
            };
            let out = sched.queue_add(input).map_err(map_runtime_error)?;
            to_json_val(out, "queue-add")
        }
        "queue-list" => {
            let agent_id = flags.require("--agent-id")?;
            let out = sched.queue_list(agent_id).map_err(map_runtime_error)?;
            to_json_val(out, "queue-list")
        }
        "queue-reorder" => {
            let agent_id = flags.require("--agent-id")?;
            let ids = parse_csv(flags.require("--delivery-ids")?);
            let out = sched
                .queue_reorder(agent_id, &ids, flags.get("--task-id"))
                .map_err(map_runtime_error)?;
            to_json_val(out, "queue-reorder")
        }
        "queue-pause" => {
            let agent_id = flags.require("--agent-id")?;
            let out = sched.queue_pause(agent_id).map_err(map_runtime_error)?;
            to_json_val(out, "queue-pause")
        }
        "queue-resume" => {
            let agent_id = flags.require("--agent-id")?;
            let out = sched.queue_resume(agent_id).map_err(map_runtime_error)?;
            to_json_val(out, "queue-resume")
        }
        "queue-steer" => {
            let agent_id = flags.require("--agent-id")?.to_string();
            let input = QueueSteerInput {
                agent_id: agent_id.clone(),
                delivery_id: flags.require("--delivery-id")?.to_string(),
                expected_turn_id: flags.require("--expected-turn-id")?.to_string(),
                routine_turn: flags.has("--routine-turn"),
            };
            let out = sched.queue_steer(input).map_err(map_runtime_error)?;
            if out.steered {
                sched
                    .complete_active_turn(&agent_id, &out.turn_id)
                    .map_err(map_runtime_error)?;
            }
            to_json_val(out, "queue-steer")
        }
        "branch" => {
            let input = BranchThreadInput {
                agent_id: flags.require("--agent-id")?.to_string(),
                message_id: flags.require("--message-id")?.to_string(),
                request_id: flags.require("--request-id")?.to_string(),
                source_task_id: flags.get("--task-id").map(str::to_string),
                title: flags.get("--title").map(str::to_string),
                simulate_source_busy: flags.has("--simulate-source-busy"),
            };
            let out = sched.branch_thread(input).map_err(map_runtime_error)?;
            to_json_val(out, "branch")
        }
        "side-question" => {
            let earlier = match flags.get("--earlier") {
                Some(raw) => parse_earlier_turns(raw)?,
                None => Vec::new(),
            };
            let input = SideQuestionInput {
                agent_id: flags.require("--agent-id")?.to_string(),
                question: flags.require("--question")?.to_string(),
                task_id: flags.get("--task-id").map(str::to_string),
                earlier,
            };
            let out = sched.ask_side_question(input).map_err(map_runtime_error)?;
            to_json_val(out, "side-question")
        }
        "react" => {
            let agent_id = flags.require("--agent-id")?.to_string();
            let actor = match flags
                .get("--actor")
                .map(|s| s.trim().to_ascii_lowercase())
                .as_deref()
            {
                None | Some("user") | Some("person") => ConversationReactionActor::User,
                Some("agent") => ConversationReactionActor::Agent {
                    agent_id: agent_id.clone(),
                },
                Some(other) => {
                    return Err(CliError::Validation(format!(
                        "unsupported reaction actor '{other}': expected user or agent"
                    )));
                }
            };
            let input = ReactMessageInput {
                agent_id,
                task_id: flags.get("--task-id").map(str::to_string),
                channel_id: flags.get("--channel-id").map(str::to_string),
                message_id: flags.require("--message-id")?.to_string(),
                emoji: flags.require("--emoji")?.to_string(),
                actor,
                remove: flags.has("--remove"),
            };
            let out = sched.react_message(input).map_err(map_runtime_error)?;
            to_json_val(out, "react")
        }
        "pin" => {
            let actor = match flags
                .get("--pinned-by")
                .map(|s| s.trim().to_ascii_lowercase())
                .as_deref()
            {
                None | Some("user") | Some("person") => PinActor::Person { sender: None },
                Some("agent") => PinActor::Agent,
                Some(other) => {
                    return Err(CliError::Validation(format!(
                        "unsupported pin actor '{other}': expected user or agent"
                    )));
                }
            };
            let input = PinMessageInput {
                agent_id: flags.require("--agent-id")?.to_string(),
                task_id: flags.get("--task-id").map(str::to_string),
                channel_id: flags.get("--channel-id").map(str::to_string),
                message_id: flags.require("--message-id")?.to_string(),
                actor,
                unpin: flags.has("--unpin"),
            };
            let out = sched.pin_message(input).map_err(map_runtime_error)?;
            to_json_val(out, "pin")
        }
        "history" => {
            let agent_id = flags.require("--agent-id")?;
            let limit = match flags.get("--limit") {
                Some(raw) => Some(parse_usize_domain(raw, "history limit")?),
                None => None,
            };
            let out = sched
                .history(agent_id, flags.get("--task-id"), limit)
                .map_err(map_runtime_error)?;
            to_json_val(out, "history")
        }
        "compact" => {
            let agent_id = flags.require("--agent-id")?;
            let out = sched
                .compact_context(
                    agent_id,
                    flags.get("--task-id"),
                    flags.get("--simulate-usage"),
                    CompactionReason::Manual,
                )
                .map_err(map_runtime_error)?;
            Ok(json!({
                "compaction": out,
                "status": out.status,
                "before_tokens": out.before_tokens,
                "after_tokens": out.after_tokens,
                "ratio": out.ratio,
            }))
        }
        "dm-send" => {
            let input = SendDirectMessageInput {
                client_message_id: flags.require("--client-message-id")?.to_string(),
                sender_member_id: flags.require("--sender")?.to_string(),
                recipient_member_id: flags.require("--recipient")?.to_string(),
                text: flags.require("--text")?.to_string(),
                created_at: None,
            };
            let dm = sched.dm_send(input).map_err(map_runtime_error)?;
            Ok(json!({
                "message_id": dm.id,
                "thread_id": dm.thread_id,
                "sender_member_id": dm.sender_member_id,
                "recipient_member_id": dm.recipient_member_id,
                "text": dm.text,
                "sequence": dm.sequence,
                "created_at": dm.created_at,
            }))
        }
        "dm-list" => {
            let member = flags.require("--member")?;
            let list = sched.dm_list(member).map_err(map_runtime_error)?;
            let threads: Vec<Value> = list
                .into_iter()
                .map(|t| {
                    json!({
                        "thread_id": t.thread_id,
                        "peer_member_id": t.other_member_id,
                        "other_member_id": t.other_member_id,
                        "unread_count": t.unread_count,
                        "last_message": t.last_message,
                        "updated_at": t.updated_at,
                    })
                })
                .collect();
            Ok(json!({ "threads": threads }))
        }
        "mailbox-send" => {
            let recipients = parse_csv(flags.require("--recipient-agent-ids")?);
            let expects_reply = flags.get_opt_bool("--expects-reply", "expects-reply")?;
            let input = MailboxSendInput {
                sender_agent_id: flags.require("--sender-agent-id")?.to_string(),
                recipient_agent_ids: recipients,
                text: flags.require("--text")?.to_string(),
                expects_reply,
                reply_to_message_id: flags.get("--reply-to").map(str::to_string),
                sender_task_id: flags.get("--sender-task-id").map(str::to_string),
                requester_ids: Vec::new(),
                idempotency_key: flags.get("--idempotency-key").map(str::to_string),
            };
            let out = sched.mailbox_send(input).map_err(map_runtime_error)?;
            to_json_val(out, "mailbox-send")
        }
        "interrupt" => {
            let owner_hint = match flags.get("--active-turn-owner") {
                Some(raw) => Some(ActiveTurnOwnerHint::parse(raw).ok_or_else(|| {
                    CliError::Validation(format!(
                        "unsupported active-turn-owner '{raw}': expected caller|user|channel|shared"
                    ))
                })?),
                None => None,
            };
            let input = InterruptAgentInput {
                caller_agent_id: flags.require("--caller-agent-id")?.to_string(),
                target_agent_id: flags.require("--target-agent-id")?.to_string(),
                reason: flags.get("--reason").map(str::to_string),
                active_turn_owner: owner_hint,
                requester_ids: Vec::new(),
                idempotency_key: flags.get("--idempotency-key").map(str::to_string),
            };
            let out = sched.interrupt_agent(input).map_err(map_runtime_error)?;
            to_json_val(out, "interrupt")
        }
        other => Err(CliError::UnknownCommand(format!(
            "Unknown action '{other}' for 'openbot chat'"
        ))),
    }
}
